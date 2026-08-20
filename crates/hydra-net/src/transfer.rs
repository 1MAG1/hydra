//! The transfer loop: drive a [`Scheduler`] over live connections until the
//! object is complete.
//!
//! `hydra-net` owns sockets, the caller owns pixels: the `observe` callback is
//! how the CLI renders per-connection state without this crate depending on a
//! UI.

use crate::http::fetch_range;
use crate::polite::Pace;
use crate::sink::SparseSink;
use crate::{Arrival, Connector, Target};
use hya_core::{Action, Scheduler};
use std::io;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;

/// Drive a transfer to completion. Returns (elapsed seconds, requests issued).
pub async fn run_transfer<C: Connector>(
    connector: Arc<C>,
    targets: Vec<Target>,
    conns_per_target: &[usize],
    size: u64,
    out_path: &str,
    sched: Scheduler,
) -> io::Result<(f64, u64)> {
    run_transfer_tick(
        connector,
        targets,
        conns_per_target,
        size,
        out_path,
        sched,
        20,
    )
    .await
}

/// As [`run_transfer`], with an explicit scheduler tick period in milliseconds.
///
/// The tick period bounds repair latency: a rate collapse can go unnoticed for
/// one tick plus one EWMA time-constant. On short transfers that is a
/// measurable fraction of the makespan, which is why it is a parameter and not
/// a constant.
#[allow(clippy::too_many_arguments)]
pub async fn run_transfer_tick<C: Connector>(
    connector: Arc<C>,
    targets: Vec<Target>,
    conns_per_target: &[usize],
    size: u64,
    out_path: &str,
    sched: Scheduler,
    tick_ms: u64,
) -> io::Result<(f64, u64)> {
    run_transfer_observed(
        connector,
        targets,
        conns_per_target,
        size,
        out_path,
        sched,
        tick_ms,
        &mut |_: &Scheduler, _: u64| {},
    )
    .await
}

/// As [`run_transfer_tick`], calling `observe(&sched, bytes_done)` once per tick.
///
/// The callback is how the CLI renders per-connection state without this crate
/// depending on a UI: `hydra-net` owns sockets, the caller owns pixels.
#[allow(clippy::too_many_arguments)]
pub async fn run_transfer_observed<C: Connector>(
    connector: Arc<C>,
    targets: Vec<Target>,
    conns_per_target: &[usize],
    size: u64,
    out_path: &str,
    sched: Scheduler,
    tick_ms: u64,
    // `Send` is required, not incidental: the queue manager spawns each transfer
    // onto a task, and a non-Send observer makes the whole future non-Send.
    observe: &mut (dyn FnMut(&Scheduler, u64) + Send),
) -> io::Result<(f64, u64)> {
    run_transfer_paced(
        connector,
        targets,
        conns_per_target,
        size,
        out_path,
        sched,
        tick_ms,
        observe,
        Pace::unlimited(),
    )
    .await
}

/// As [`run_transfer_observed`], with an aggregate rate cap.
///
/// Separate from [`run_transfer_observed`] rather than an extra argument on it
/// because almost every caller — the cross-validation harness, the benchmark
/// runner, the e2e tests — has no cap to apply, and threading `Pace::unlimited()`
/// through all of them buys nothing. `--limit-rate` is the one caller that does.
#[allow(clippy::too_many_arguments)]
pub async fn run_transfer_paced<C: Connector>(
    connector: Arc<C>,
    targets: Vec<Target>,
    conns_per_target: &[usize],
    size: u64,
    out_path: &str,
    sched: Scheduler,
    tick_ms: u64,
    observe: &mut (dyn FnMut(&Scheduler, u64) + Send),
    pace: Pace,
) -> io::Result<(f64, u64)> {
    let sink = Arc::new(SparseSink::create(out_path, size)?);
    run_transfer_into(
        connector,
        targets,
        conns_per_target,
        size,
        sink,
        sched,
        tick_ms,
        observe,
        pace,
    )
    .await
}

/// As [`run_transfer_observed`], but the caller supplies the sink.
///
/// This is what `--no-save` needs: it passes [`SparseSink::discarding`] so no
/// file is ever created. The earlier implementation wrote a real file and
/// deleted it at the end, which left the bytes on disk for the whole transfer
/// and survived an interrupted run.
///
/// A caller that also wants the object's digest attaches one to the sink with
/// [`SparseSink::with_digest`] before handing it over — the sink is the one place
/// every fragment passes through, so it is where a stream observer belongs.
#[allow(clippy::too_many_arguments)]
pub async fn run_transfer_into<C: Connector>(
    connector: Arc<C>,
    targets: Vec<Target>,
    conns_per_target: &[usize],
    size: u64,
    sink: Arc<SparseSink>,
    sched: Scheduler,
    tick_ms: u64,
    observe: &mut (dyn FnMut(&Scheduler, u64) + Send),
    pace: Pace,
) -> io::Result<(f64, u64)> {
    run_transfer_cancellable(
        connector,
        targets,
        conns_per_target,
        size,
        sink,
        sched,
        tick_ms,
        observe,
        pace,
        None,
    )
    .await
}

/// As [`run_transfer_into`], with an external stop signal.
///
/// This is the GUI's Pause/Cancel: setting `cancel` makes the loop abort every
/// in-flight fetch and return `ErrorKind::Interrupted` at the next tick, so the
/// caller gets the socket teardown the scheduler's own `Action::Cancel` path
/// performs — not detached tasks streaming on. Bytes already written through
/// the sink stay valid at their offsets; a later run resumes by `mark_done`.
/// `None` is exactly the previous behaviour, which is how every existing entry
/// point calls it.
#[allow(clippy::too_many_arguments)]
pub async fn run_transfer_cancellable<C: Connector>(
    connector: Arc<C>,
    targets: Vec<Target>,
    conns_per_target: &[usize],
    size: u64,
    sink: Arc<SparseSink>,
    mut sched: Scheduler,
    tick_ms: u64,
    observe: &mut (dyn FnMut(&Scheduler, u64) + Send),
    pace: Pace,
    cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
) -> io::Result<(f64, u64)> {
    let (tx, mut rx) = mpsc::unbounded_channel::<Arrival>();
    let t0 = Instant::now();

    // conn index -> target index
    let mut owner = Vec::new();
    for (i, &k) in conns_per_target.iter().enumerate() {
        for _ in 0..k {
            owner.push(i);
        }
    }

    // Live fetch tasks, so a Cancel action can actually stop one. Without this
    // a black-holed connection's task lives forever holding its socket, and the
    // scheduler's reclaim has no effect on the wire.
    let mut inflight: std::collections::HashMap<usize, tokio::task::JoinHandle<()>> =
        std::collections::HashMap::new();

    // The live far end of each connection's in-flight range, shared with the task
    // streaming it. A repair lowers the victim's entry and the running task sees
    // it on its next read, which is what makes range preemption cost what the
    // theory says it costs — see `crate::Watermark`.
    let mut bounds: std::collections::HashMap<usize, crate::Watermark> =
        std::collections::HashMap::new();

    // One pool for the whole transfer, shared by every connection. This is the
    // case connection reuse was missing from most: `n` ranges against one origin
    // used to mean `n` handshakes, and every repair another. The pool only ever
    // receives connections whose response ended where the client predicted — see
    // `crate::pool` — so a shrunk connection is dropped rather than reused.
    //
    // Taken from the connector when it offers one, so a caller that already spoke
    // to this origin — the CLI's size probe does, on every run — can hand over the
    // connection it is holding instead of letting the transfer redial. Measured on
    // a live TLS path: 1.6-2.0 s of setup before the first byte on a short transfer.
    // The gap was almost entirely handshakes that had already been paid for once.
    let pool: crate::pool::SharedPool<C::Stream> = connector
        .pool()
        .unwrap_or_else(|| Arc::new(crate::pool::ConnPool::new()));
    // Reported through the same channel as everything else measurable about a
    // transfer: a reuse rate that is claimed rather than counted is not a
    // measurement. `HYDRA_POOL_STATS=1` prints it after the transfer.
    let report_pool = std::env::var_os("HYDRA_POOL_STATS").is_some();

    // ---- no-progress watchdog --------------------------------------------
    //
    // The scheduler detects a stalled connection and reclaims its range, but
    // reclaiming is not recovering: it returns the bytes to the unassigned set,
    // and only a *different* live source can turn that into progress. With one
    // source — or with every source dead — the reclaimed range is handed back to
    // the same silent connection on the next tick and the loop spins.
    //
    // This is invisible to scheduler core invariants (bytes are tracked, and
    // reclaimable stalls allow state machine transitions). The higher-level
    // transport loop owns the wall clock and enforces real-world progress deadlines.
    //
    // The deadline is derived rather than hardcoded: a request costs `delta` to
    // set up, and the scheduler needs a few reclaim-and-retry rounds before
    // giving up is fair. Expressing it in units of the measured `delta` and the
    // configured stall timeout means a slow TLS-through-a-proxy path is granted
    // proportionally more patience than a LAN mirror, with no constant to
    // retune. The floor keeps a fast path from failing during normal setup
    // jitter; the ceiling keeps a pathological `delta` estimate from
    // reintroducing the original hang.
    let no_progress_deadline = {
        const RECLAIM_ROUNDS: f64 = 4.0;
        let d = sched.worst_delta().max(0.0);
        let st = sched.stall_timeout().max(0.0);
        (RECLAIM_ROUNDS * (st + d)).clamp(5.0, 60.0)
    };
    let mut last_progress_at = Instant::now();
    let mut last_held: u64 = sched.bytes_held();

    // Extra patience earned by DELIBERATE scheduler pauses, and the ceiling on it.
    //
    // `backoff_grace` accumulates only while every source is suspended — silence the
    // scheduler chose — and is added to the no-progress deadline. The cap is what
    // keeps this from becoming unbounded patience: a source that black-holes from the
    // first byte generates a fresh suspension after every stall, so an uncapped grace
    // would forgive deadline after deadline and the transfer would never fail.
    //
    // One extra deadline's worth is enough to cover a real backoff (`stall_timeout`
    // through 30s) once, which is the case worth surviving, while still bounding the
    // total wait at roughly twice the deadline.
    let mut backoff_grace = 0.0f64;
    let backoff_grace_cap = no_progress_deadline;
    let mut last_tick_at = Instant::now();

    let mut ticker = tokio::time::interval(tokio::time::Duration::from_millis(tick_ms.max(1)));
    // In-band concurrency ramp.
    //
    // Enabled by the caller starting the scheduler below its full connection count
    // (`Scheduler::with_active_limit`). The ramp then admits connections while the
    // aggregate rate says they pay for themselves, measuring on the real transfer
    // instead of on probe traffic — see `hya_core::ramp` for why the probe was a
    // net loss (1.96x slower on a 3 MB object, p = 0.004).
    let mut ramp = if sched.active_limit() < sched.n_conns() {
        // Start at the scheduler's active limit, which the caller set deliberately,
        // rather than at one. The CLI sets it to 1 for `--adaptive`, so the search
        // begins at the configuration that measured fastest in the field and only
        // admits more on evidence of headroom — see `ConcurrencyRamp::starting_at`.
        let mut r = hya_core::ConcurrencyRamp::starting_at(
            0.15,
            sched.active_limit().max(1),
            sched.n_conns(),
        );
        r.start(0.0, sched.worst_delta().max(1e-3));
        // Measure levels, not handshakes. `delta` is the per-request cost on a
        // pooled connection; opening a new one costs a TCP and a TLS handshake on
        // top of it, which on a high-RTT path outlasts the whole measurement window
        // — so the level read as no better than the one below it and the search
        // settled at one connection. The gate holds the window shut until the
        // level's connections are actually on the wire.
        r.arm_warmup(0.0, sched.worst_delta().max(1e-3));
        Some(r)
    } else {
        None
    };
    let mut ramp_last_held: u64 = sched.bytes_held();

    loop {
        // 0. external stop. Checked before arrivals are drained so a cancel
        // observed mid-tick still credits the bytes below it; abort teardown
        // mirrors the watchdog path.
        if let Some(c) = &cancel {
            if c.load(std::sync::atomic::Ordering::Relaxed) {
                // Report the final credited state so the caller's last snapshot
                // (held ranges, bytes done) includes everything on disk.
                observe(&sched, sched.bytes_held());
                for (_, h) in inflight.drain() {
                    h.abort();
                }
                return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
            }
        }

        // 1. drain arrivals into the scheduler
        while let Ok(a) = rx.try_recv() {
            sched.on_bytes_at(a.conn, a.off, a.bytes, a.at, a.dt);
        }

        // 1b. feed the ramp the AGGREGATE delivery, and let it decide.
        //
        // Aggregate rather than per-connection on purpose: on a saturated link each
        // connection's own rate falls as connections are added while the total stays
        // flat, so a per-connection view would read saturation as healthy scaling.
        if let Some(r) = ramp.as_mut() {
            let now = t0.elapsed().as_secs_f64();
            let held = sched.bytes_held();
            r.observe(held.saturating_sub(ramp_last_held), now);
            ramp_last_held = held;
            // A connection counts as delivering once its cursor has moved off the
            // start of its range: bytes have arrived on THIS request, so its
            // handshake, its request and its first byte are all behind it. The
            // scheduler's rate estimate is not the same test — it survives a
            // connection going idle, so it would report a connection as warm
            // before its replacement request has produced anything.
            let live = (0..sched.n_conns())
                .filter(|&j| {
                    sched
                        .conn_range(j)
                        .map(|(lo, pos, _hi)| pos > lo)
                        .unwrap_or(false)
                })
                .count();
            r.note_delivering(live);
            match r.poll(now, sched.worst_delta().max(1e-3)) {
                hya_core::Ramp::Raise(n) => sched.set_active_limit(n),
                hya_core::Ramp::Settled(n) => {
                    sched.set_active_limit(n);
                    // Stop polling: the search is over and re-running it would
                    // re-pay its cost on a decision already made.
                    ramp = None;
                }
                hya_core::Ramp::Hold => {}
            }
        }
        if sched.is_complete() {
            // Observe the FINAL state before leaving. The loop breaks here, above
            // the per-tick `observe` call, so without this the last arrivals — the
            // ones that completed the transfer — are never reported: the progress
            // bar stops short of 100%, and a caller deriving completeness from the
            // observed count concludes the transfer is short by whatever landed in
            // the final tick. Measured on an 11 200 900-byte resume: 2 876 bytes
            // unobserved, a byte-exact file reported as incomplete.
            observe(&sched, sched.bytes_held());
            break;
        }

        // 2. let the scheduler decide, and act on what it returns
        let now = t0.elapsed().as_secs_f64();
        for act in sched.tick(now) {
            match act {
                Action::Cancel { conn } => {
                    if let Some(h) = inflight.remove(&conn) {
                        h.abort();
                    }
                    bounds.remove(&conn);
                }
                // A repair moved this connection's far end down. Publishing it is
                // the entire mechanism: the victim's own loop stops at the new
                // boundary, so the span handed to the taker crosses the wire once
                // rather than twice. No `abort` and no request — that is the point,
                // the connection keeps streaming the part it still owns.
                Action::Shrink { conn, hi } => {
                    if let Some(b) = bounds.get(&conn) {
                        b.shrink_to(hi);
                    }
                }
                Action::Request { conn, range } => {
                    if let Some(h) = inflight.remove(&conn) {
                        h.abort(); // supersede: never two live responses per conn
                    }
                    let t = targets[owner[conn]].clone();
                    let (sk, txc, cc) = (sink.clone(), tx.clone(), connector.clone());
                    // Every connection shares ONE limiter, so the cap applies to the
                    // aggregate. Cloning a `Pace` clones the `Arc`, not the bucket.
                    let pc = pace.clone();
                    // A fresh bound per request: a stale one from a superseded
                    // request may already have been shrunk, which would truncate
                    // this range before it started.
                    let bound = crate::Watermark::fixed(range.hi);
                    bounds.insert(conn, bound.clone());
                    let pl = pool.clone();
                    let h = tokio::spawn(async move {
                        let _ =
                            fetch_range(cc, conn, t, range.lo, bound, sk, txc, t0, pc, Some(pl))
                                .await;
                    });
                    inflight.insert(conn, h);
                }
            }
        }

        tokio::select! {
            _ = ticker.tick() => {}
            Some(a) = rx.recv() => { sched.on_bytes_at(a.conn, a.off, a.bytes, a.at, a.dt); }
        }
        observe(&sched, sched.bytes_held());

        // Watchdog: any byte of progress resets the clock, so this fires only
        // when the whole transfer — not merely one connection — has been silent.
        // A slow-but-moving source is never killed by it, however slow.
        // Accrue grace for time spent under a deliberate suspension. Measured as
        // elapsed wall clock since the previous iteration rather than as the
        // suspension's nominal length, so a pause that ends early costs only what it
        // actually took.
        {
            let dt = last_tick_at.elapsed().as_secs_f64();
            last_tick_at = Instant::now();
            if sched
                .all_sources_suspended_until(t0.elapsed().as_secs_f64())
                .is_some()
            {
                backoff_grace = (backoff_grace + dt).min(backoff_grace_cap);
            }
        }

        let held_now = sched.bytes_held();
        if held_now > last_held {
            last_held = held_now;
            last_progress_at = Instant::now();
        } else if last_progress_at.elapsed().as_secs_f64() > no_progress_deadline + backoff_grace {
            // The deadline is extended by `backoff_grace`: the time the scheduler
            // has DELIBERATELY spent with every source suspended, capped.
            //
            // Silence the scheduler asked for is not a stall. After repeated stalls a
            // source is suspended for a backoff interval, and with a single source —
            // one URL, one CDN, the common case — nothing can move until it expires.
            // Charging that against the no-progress deadline made hydra abort
            // transfers it had itself paused: on a 121.7 MiB GitHub release asset, 4
            // of 8 multi-connection runs died at "no progress for 16s", three of them
            // holding 126.9-127.0 MB of 127.6 MB — 99.6% fetched, reported as failed.
            //
            // The grace is CAPPED, and the cap is the whole design. An uncapped
            // version (reset the clock whenever every source is suspended) hangs
            // forever on a source that black-holes from the first byte: each stall
            // triggers another suspension, which forgives another deadline. That is
            // the failure `a_lone_black_holing_source_fails_instead_of_idling_forever`
            // exists to catch, and it caught it.
            for (_, h) in inflight.drain() {
                h.abort();
            }
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "no progress for {no_progress_deadline:.0}s: {} of {} bytes received, \
                     every source stalled or unreachable",
                    held_now, size
                ),
            ));
        }

        // Wall-clock ceiling scaled to the object: a fixed 120 s aborts a large
        // download on a slow link, which is a bug rather than a safety net. The
        // floor keeps small transfers from hanging indefinitely.
        let budget = (size as f64 / 32_768.0).clamp(120.0, 7200.0);
        if t0.elapsed().as_secs_f64() > budget {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("transfer exceeded {budget:.0}s budget"),
            ));
        }
    }
    for (_, h) in inflight.drain() {
        h.abort();
    }
    if report_pool {
        let (hits, misses) = pool.stats();
        eprintln!(
            "pool: {hits} reused, {misses} fresh ({} requests, {} repairs)",
            sched.stats.requests, sched.stats.repairs
        );
    }
    Ok((t0.elapsed().as_secs_f64(), sched.stats.requests))
}
