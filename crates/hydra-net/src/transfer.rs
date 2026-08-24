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
    // Fetch outcomes, reported by every spawned task as it ends.
    //
    // Without this the transport SPAWNS a fetch and never looks at what happened
    // to it: `let _ = fetch_range(...).await` discarded the result, so a refused
    // connection, a closed socket, a truncated body, a 503 and a protocol
    // violation were all indistinguishable from a connection that was merely
    // slow. The only thing that eventually noticed was the scheduler's stall
    // timeout, seconds later — see `Scheduler::on_conn_error` for what that costs
    // and why the endgame is where it shows.
    //
    // The generation number is not decoration: a task can finish in the same
    // instant its connection is superseded or cancelled, and its outcome would
    // then be charged against the request that replaced it.
    let (done_tx, mut done_rx) = mpsc::unbounded_channel::<(usize, u64, io::Result<()>)>();
    let mut gen_of: std::collections::HashMap<usize, u64> = std::collections::HashMap::new();
    let mut next_gen: u64 = 0;
    // Consecutive fetch failures since the last byte of progress, and the last
    // error seen. The streak drives per-connection backoff so a broken endpoint is
    // not hammered; the error is reported if the transfer ultimately fails, since
    // "every source stalled or unreachable" describes the symptom and never the
    // cause.
    let mut fail_streak: u32 = 0;
    let mut hard_streak: u32 = 0;
    let mut last_error: Option<io::Error> = None;
    // Concurrency ceiling learned from the source's own refusals.
    //
    // A 429 answers a question the client did not think to ask: how many requests
    // at once will this origin accept? Standing the source down for `Retry-After`
    // and then returning with the SAME connection count asks the identical
    // question again, gets the identical answer, and the transfer livelocks.
    // Measured against `ash-speed.hetzner.com`, which serves exactly two
    // connections per address and refuses the rest: eight connections downloaded
    // ZERO bytes in thirty seconds, one refusal every two seconds, each aborting
    // whatever the other seven had in flight.
    //
    // The ceiling moves the way congestion control moves — down hard on a refusal,
    // back up one at a time on evidence that the limit has lifted — for the same
    // reason: the client cannot see the origin's limit, only whether it is over it.
    // The ramp is clamped to it too, since a ramp that re-raises what a refusal
    // lowered is the same livelock with a longer period.
    let mut throttle_cap = sched.n_conns().max(1);
    // One reduction per refusal ROUND, not per refusal. Refusals arrive in bursts
    // — six of eight requests, in the same instant, all reporting the one fact
    // that eight was too many. Halving once per burst converges on the limit;
    // halving six times converges on one connection and stays there, which is a
    // transfer at a fraction of the rate the origin was willing to serve.
    let mut cap_settled_until = 0.0f64;
    // A refusal-free stretch this long, with bytes actually arriving, earns one
    // connection back. Without it the first burst caps the transfer permanently,
    // including for the far more common case of a limit that is momentary.
    let probe_interval = (20.0 * sched.worst_delta().max(0.25)).clamp(10.0, 60.0);
    let mut next_probe_at = f64::INFINITY;
    // Bytes held when the last refusal landed. The probe is earned by PROGRESS,
    // not by the clock: widening a transfer that is not moving adds requests to a
    // source that is already failing to serve the ones it has.
    let mut probe_held_mark: u64 = 0;
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
    let trace_errors = std::env::var_os("HYDRA_TRACE_ERRORS").is_some();

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

        // 1a. act on fetch outcomes, at once rather than at the stall timeout.
        while let Ok((conn, g, res)) = done_rx.try_recv() {
            // Not the request this connection is running now: the task was
            // superseded or cancelled and finished on its way out. Its outcome
            // says nothing about the range now in flight.
            if gen_of.get(&conn) != Some(&g) {
                continue;
            }
            gen_of.remove(&conn);
            inflight.remove(&conn);
            bounds.remove(&conn);
            let Err(e) = res else { continue };
            let now = t0.elapsed().as_secs_f64();
            if trace_errors {
                eprintln!("[trace] t={now:.2} conn {conn}: {} — {e}", e.kind());
            }
            fail_streak = fail_streak.saturating_add(1);
            let kind = e.kind();
            // A server that ignores `Range`, mislabels a `Content-Range`, answers
            // with a status this client cannot use, or redirects a transfer that
            // is already under way will do the same to the next request; retrying
            // is how a client hammers a broken endpoint instead of reporting it.
            // A streak rather than the first one, because a single 5xx from one
            // node of a CDN is worth another attempt.
            //
            // Reporting matters as much as stopping. An expired pre-signed URL —
            // a GitHub release asset, an S3 link — starts answering 403 part-way
            // through, and every request after that fails the same way. Spinning
            // on it until the no-progress deadline tells the user only that
            // nothing is arriving; failing on it tells them what the server said.
            hard_streak = if matches!(
                kind,
                io::ErrorKind::InvalidData | io::ErrorKind::NotConnected
            ) {
                hard_streak + 1
            } else {
                0
            };
            // Per-connection backoff, doubling with the streak. Zero would be a
            // hot retry loop against an endpoint that is refusing; the range is
            // back in the unassigned set either way, so a healthy connection can
            // pick it up on the next tick without waiting for this.
            let cool = (0.05 * (1u64 << fail_streak.min(6)) as f64).min(2.0);
            match kind {
                // 429/503 with a Retry-After: the origin refusing a request
                // because too many are in flight, or because it is shedding load.
                io::ErrorKind::WouldBlock => {
                    let secs = crate::http::retry_after_secs(&e)
                        .unwrap_or(1.0)
                        .clamp(0.05, 60.0);
                    let src = sched.conn_source(conn);
                    // What this source is DELIVERING right now, refusal and all.
                    // The distinction the old code missed: a refusal while other
                    // connections stream is the origin declining one more request,
                    // not the source going away, and tearing the working ones down
                    // throws away bytes that have to be pulled again.
                    let delivering = (0..sched.n_conns())
                        .filter(|&j| {
                            j != conn
                                && sched.conn_source(j) == src
                                && sched
                                    .conn_range(j)
                                    .map(|(lo, pos, _hi)| pos > lo)
                                    .unwrap_or(false)
                        })
                        .count();
                    if delivering == 0 {
                        // Nothing is getting through: the source itself stands
                        // down, and its in-flight fetches go with it, since their
                        // bytes would no longer be credited to any range.
                        sched.suspend_source(src, now + secs);
                        for j in 0..sched.n_conns() {
                            if sched.conn_source(j) == src {
                                if let Some(h) = inflight.remove(&j) {
                                    h.abort();
                                }
                                bounds.remove(&j);
                                gen_of.remove(&j);
                            }
                        }
                    } else {
                        // Only this request was refused. Cool this connection for
                        // as long as the origin asked and leave the rest alone.
                        sched.on_conn_error(conn, now, secs);
                    }
                    // One step down per burst, and never below what is visibly
                    // working: the connections that ARE streaming are proof of a
                    // count this origin accepts, so a refusal is evidence about
                    // the excess, not about them.
                    if now >= cap_settled_until {
                        let floor = delivering.max(1);
                        throttle_cap = (throttle_cap / 2).max(floor).min(sched.n_conns().max(1));
                        cap_settled_until = now + secs + sched.worst_delta().max(0.05);
                        if trace_errors {
                            eprintln!(
                                "[trace] t={now:.2} throttled: {delivering} delivering, \
                                 concurrency ceiling -> {throttle_cap}"
                            );
                        }
                    }
                    if sched.active_limit() > throttle_cap {
                        sched.set_active_limit(throttle_cap);
                    }
                    next_probe_at = now + probe_interval;
                    probe_held_mark = sched.bytes_held();
                }
                _ => sched.on_conn_error(conn, now, cool),
            }
            if hard_streak >= 3 {
                for (_, h) in inflight.drain() {
                    h.abort();
                }
                // A redirect travels as `NotConnected` with the location in its
                // message, which is a protocol detail of the fetch path and not
                // something to hand a user verbatim.
                if kind == io::ErrorKind::NotConnected {
                    let loc = e.to_string();
                    let loc = loc.strip_prefix("redirect:").unwrap_or("").to_string();
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("server redirected mid-transfer to {loc}"),
                    ));
                }
                return Err(e);
            }
            last_error = Some(e);
        }

        // 1a2. give one connection back after a refusal-free stretch.
        //
        // A ceiling learned from one burst is a guess about a limit that may have
        // been momentary: a neighbour on the same address, a CDN node shedding
        // load for a second, a per-minute quota that has since rolled over. Left
        // alone, the first burst caps the transfer for its whole life. Probing
        // upward costs one refused request per interval where the limit is real,
        // and restores the transfer's full width where it was not.
        if throttle_cap < sched.n_conns() {
            let now = t0.elapsed().as_secs_f64();
            if now >= next_probe_at && sched.bytes_held() > probe_held_mark {
                throttle_cap += 1;
                // Under a ramp the limit is the ramp's to set; this only lifts the
                // clamp it is measured against.
                if ramp.is_none() {
                    sched.set_active_limit(throttle_cap);
                }
                next_probe_at = now + probe_interval;
                probe_held_mark = sched.bytes_held();
                if trace_errors {
                    eprintln!("[trace] t={now:.2} refusal-free: ceiling -> {throttle_cap}");
                }
            }
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
                hya_core::Ramp::Raise(n) => sched.set_active_limit(n.min(throttle_cap)),
                hya_core::Ramp::Settled(n) => {
                    sched.set_active_limit(n.min(throttle_cap));
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
                    // Forget the generation, so an outcome already in flight from
                    // the task being aborted is not charged against whatever this
                    // connection is given next.
                    gen_of.remove(&conn);
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
                    next_gen += 1;
                    let g = next_gen;
                    gen_of.insert(conn, g);
                    let dtx = done_tx.clone();
                    let h = tokio::spawn(async move {
                        let r =
                            fetch_range(cc, conn, t, range.lo, bound, sk, txc, t0, pc, Some(pl))
                                .await;
                        let _ = dtx.send((conn, g, r));
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
            // Bytes are moving, so whatever failed was transient and its backoff
            // must not accumulate against the next unrelated failure.
            fail_streak = 0;
            hard_streak = 0;
            last_error = None;
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
            // Report the last transport failure if there was one. "Every source
            // stalled or unreachable" is the symptom; the cause is the error the
            // fetches were actually returning, and a user cannot act on the first
            // without the second.
            let cause = match &last_error {
                Some(e) => format!(" (last error: {e})"),
                None => String::new(),
            };
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "no progress for {no_progress_deadline:.0}s: {} of {} bytes received, \
                     every source stalled or unreachable{cause}",
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
            "pool: {hits} reused, {misses} fresh ({} requests, {} repairs, {} reclaims)",
            sched.stats.requests, sched.stats.repairs, sched.stats.reclaims
        );
    }
    Ok((t0.elapsed().as_secs_f64(), sched.stats.requests))
}
