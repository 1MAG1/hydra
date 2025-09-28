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
use hydra_core::{Action, Scheduler};
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
    mut sched: Scheduler,
    tick_ms: u64,
    observe: &mut (dyn FnMut(&Scheduler, u64) + Send),
    pace: Pace,
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

    let mut ticker = tokio::time::interval(tokio::time::Duration::from_millis(tick_ms.max(1)));
    loop {
        // 1. drain arrivals into the scheduler
        while let Ok(a) = rx.try_recv() {
            sched.on_bytes_at(a.conn, a.off, a.bytes, a.at, a.dt);
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
                    let h = tokio::spawn(async move {
                        let _ = fetch_range(cc, conn, t, range.lo, range.hi, sk, txc, t0, pc).await;
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
        let held_now = sched.bytes_held();
        if held_now > last_held {
            last_held = held_now;
            last_progress_at = Instant::now();
        } else if last_progress_at.elapsed().as_secs_f64() > no_progress_deadline {
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
    Ok((t0.elapsed().as_secs_f64(), sched.stats.requests))
}
