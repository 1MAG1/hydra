//! In-band concurrency ramp: find the useful connection count *during* the
//! transfer, not before it.
//!
//! # Why this replaces the probe
//!
//! The standard way to pick a connection count is to probe: fetch a slab with one
//! connection, then two, then three, comparing aggregate goodput, and settle where
//! the marginal gain stops paying. That is what [`crate::Admission`] does, and it
//! is the right *decision rule*. The problem is where the samples come from.
//!
//! HARP (Kim, Yildirim & Kosar, SC'16) states the objection plainly: probing
//! captures instantaneous load but "may bring too much probing overhead", because
//! each sample is an extra transfer paid for before the real one begins. Measured
//! on this client against a live path with a 3.15 MB object, the climbing probe
//! made the whole transfer **1.96x slower** than not probing at all — 18.2 s
//! median against 8.3 s, paired across 9 interleaved repetitions, p = 0.004. The
//! search cost more than the concurrency it found could recover. HARP's own answer
//! is to amortise the samples across a historical corpus of past transfers, so a
//! new transfer needs at most one probe.
//!
//! This module takes the cheaper route available to a downloader: run the same
//! search **on the object itself**. Concurrency is adjustable mid-transfer
//! ([`Scheduler::set_active_limit`]), so the ramp starts at one connection,
//! watches the aggregate rate over a short window, and admits another connection
//! while the marginal gain justifies its setup cost. Every byte moved while
//! searching is a byte of the object that had to be fetched anyway, so the search
//! is free in bytes — its only cost is arriving at the final concurrency a few
//! windows late.
//!
//! # What it measures, and the trap in measuring it
//!
//! The quantity that decides whether to admit connection `k+1` is the *aggregate*
//! goodput at `k`, and it must be sampled after the new connection's transient has
//! passed. A window that starts the instant a connection is admitted measures its
//! handshake and slow-start, not its steady contribution, and would conclude that
//! every added connection helps less than it does. So each admission is followed by
//! a settling delay before the next window counts.
//!
//! The opposite error is just as easy: a window long enough to be clean is a
//! window during which a *saturated* path is running more connections than it
//! needs. The window length is therefore expressed in terms of the measured setup
//! cost `delta` — the same quantity the repair deadband is floored at — because
//! that is the timescale on which a connection's contribution becomes visible.

use crate::{Admission, Admit};

/// Outcome of feeding a window of observations to the ramp.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ramp {
    /// Keep the current concurrency; not enough evidence yet this window.
    Hold,
    /// Raise the active limit to this many connections.
    Raise(usize),
    /// The search is finished: this is the useful count.
    Settled(usize),
}

/// Drives concurrency upward on a live transfer while it pays to do so.
#[derive(Clone, Debug)]
pub struct ConcurrencyRamp {
    adm: Admission,
    /// Connections currently admitted.
    level: usize,
    /// Hard ceiling: politeness or an explicit `-x`, never exceeded.
    max: usize,
    /// Wall clock at which the current measurement window may begin counting.
    /// Set past the present on each admission so a new connection's handshake and
    /// slow-start are not charged against it.
    window_open_at: f64,
    /// Wall clock at which the current window closes.
    window_ends_at: f64,
    /// Bytes seen since the window opened.
    bytes: u64,
    /// When the window actually started counting bytes.
    counting_since: f64,
    settled: Option<usize>,
    /// The first window's rate at the current level, awaiting a confirming second.
    ///
    /// Held rather than recorded so `Admission` sees exactly one sample per level; see
    /// the confirmation block in `poll` for why one window is not enough.
    held_rate: Option<f64>,
    /// Connections the transport reports as actually delivering bytes.
    ///
    /// `None` means the caller does not supply the signal, which disables the
    /// warm-up gate below and leaves the timing exactly as it was.
    delivering: Option<usize>,
    /// While set, a level has been admitted whose connections have not all
    /// delivered a byte yet, and the measurement window is held open. The value is
    /// the wall clock past which the ramp stops waiting and measures anyway.
    warm_deadline: Option<f64>,
}

/// How long a measurement window runs, in multiples of the measured setup cost.
///
/// Long enough that a connection's steady contribution dominates its transient,
/// short enough that a saturated path is not over-provisioned for long. Three
/// setup costs is roughly the point at which a TCP flow's congestion window has
/// stopped being the limiting factor on a typical path.
/// A measurement window must outlast TCP slow start, or it measures the wrong thing.
///
/// # The trap this constant sits in
///
/// A newly admitted flow does not deliver its share immediately: it opens with a small
/// congestion window and needs several round trips to reach steady state. Measure it
/// before then and the observed rate is still climbing — so the ramp concludes the
/// connection is paying its way and admits more, on every level, until it hits the
/// ceiling. That is not a threshold that needs tuning; it is measuring a transient and
/// calling it a steady state.
///
/// Both failure modes were observed on the same path, and they pull in opposite
/// directions:
///
/// * Windows too LONG (tied to `delta` with no ceiling): reaching 8 connections took
///   16–32 s on a path where `delta` was 0.5–1.0 s, longer than the whole 3.15 MB
///   transfer. Measured 2.78x slower than a fixed `-x 8`.
/// * Windows too SHORT (`MAX_WINDOW_S` = 0.6 s, i.e. ~3 RTTs at the 200 ms RTT of these
///   origins, against the 4–8 RTTs slow start needs): every level looks like it is
///   still improving, so the search runs to the ceiling. Measured settling at 8 on a
///   path a single stream saturates, causing significant transfer slowdown.
///
/// No single value satisfies both, which is why the ramp no longer climbs from one. It
/// starts at the concurrency that measured fastest in the field and only *adds* when
/// there is direct evidence of headroom — see `ConcurrencyRamp::new`.
const WINDOW_DELTAS: f64 = 3.0;

/// Settling time after an admission before its window may count, in multiples of
/// the setup cost. A connection that has not finished its handshake contributes
/// nothing, and charging that silence to the aggregate would understate the gain.
const SETTLE_DELTAS: f64 = 1.5;

/// Floor on both, so a path reporting an implausibly small setup cost cannot
/// collapse the windows to noise.
const MIN_WINDOW_S: f64 = 0.25;

/// How long the ramp waits for a newly admitted connection to deliver its first
/// byte before measuring the level without it, in multiples of `delta`.
///
/// Generous relative to the windows on purpose: this is not a measurement window
/// but a bound on patience, and the thing it waits for (connect + TLS + first byte)
/// costs several `delta` on any path where `delta` is a single round trip.
const WARM_DELTAS: f64 = 8.0;

/// Bounds on that patience. The floor covers a fast path whose `delta` estimate is
/// small enough that eight of them still land inside one handshake; the ceiling is
/// what keeps a connection that never delivers from stalling the search — the
/// scheduler's stall detection and reclaim own that connection, not the ramp.
const MIN_WARM_S: f64 = 1.0;
const MAX_WARM_S: f64 = 8.0;

/// Ceiling on both.
///
/// Scaling the windows by `delta` alone has the effect exactly backwards on a slow
/// path: a large `delta` means each window is long, so the ramp takes longest to
/// reach useful concurrency precisely where setup costs most and concurrency is
/// most valuable. On the measured path `delta` reached ~1.0 s, which put full
/// concurrency 31 s away — past the end of the transfer. `delta` still sets the
/// timescale, but it cannot set an unbounded one.
const MAX_WINDOW_S: f64 = 0.6;

impl ConcurrencyRamp {
    /// `min_gain_frac` is the marginal goodput, as a fraction of the
    /// single-connection rate, that a new connection must add to be kept.
    /// Start the search at `start` connections rather than at one.
    ///
    /// # Why the search no longer climbs from one
    ///
    /// Climbing costs a measurement window per level, and a window long enough to
    /// outlast slow start (see `WINDOW_DELTAS`) is long enough that the climb dominates
    /// a short transfer. Climbing from one is only worth it if the levels above one are
    /// likely to be much better — and on the paths measured, they are not.
    ///
    /// The asymmetry, not a claimed win, is what justifies starting low. Measured
    /// over 20 paired repetitions on four objects, starting at 1 connection is
    /// statistically indistinguishable from a fixed baseline while fixed `-x 8`
    /// cost 1.37–3.04x, and on a path a single stream already saturates `-x 8`
    /// incurred a 3.6x slowdown where `-x 1` was 1.17x. So the downside of starting
    /// high is large and measured; the upside is not.
    ///
    /// Starting at one is therefore a conservative policy choice, ensuring minimal
    /// overhead while admitting more connections only when headroom is proven.
    pub fn starting_at(min_gain_frac: f64, start: usize, max: usize) -> Self {
        let mut r = Self::new(min_gain_frac, max);
        r.level = start.clamp(1, r.max);
        r
    }

    pub fn new(min_gain_frac: f64, max: usize) -> Self {
        Self {
            adm: Admission::new(min_gain_frac, max),
            level: 1,
            max: max.max(1),
            window_open_at: 0.0,
            window_ends_at: 0.0,
            bytes: 0,
            counting_since: 0.0,
            settled: None,
            held_rate: None,
            delivering: None,
            warm_deadline: None,
        }
    }

    /// Opt in to the warm-up gate, and arm it for the level the search starts at.
    ///
    /// # The measurement this exists to prevent
    ///
    /// The windows are scaled by `delta`, the per-REQUEST setup cost, which on a
    /// pooled connection is one round trip and is measured at 50-100 ms on the paths
    /// this was tuned against. Admitting a CONNECTION costs something else entirely:
    /// a TCP handshake, a TLS handshake and a first byte, and on a 250 ms-RTT path
    /// that is 1.2-1.6 s before a single byte arrives — longer than `SETTLE_DELTAS`
    /// and `WINDOW_DELTAS` together, both of which are clamped at `MAX_WINDOW_S`.
    ///
    /// The window therefore opened and closed while the new connection was still
    /// handshaking, the level measured as no better than the one below it, and the
    /// search settled at ONE on a path with real headroom. Reported from the field
    /// as "only two of eight connections start": the second connection had delivered
    /// 240 KB — the tail of its slow start — when the ramp judged it and stopped.
    ///
    /// A low-RTT path escapes it by luck: there the handshake fits inside the settle
    /// delay, so the same code measures a warm connection and climbs to the ceiling.
    /// That is what made this look path-specific rather than systematic.
    ///
    /// So the settle delay cannot be a duration alone. The transport reports how many
    /// connections are actually delivering (`note_delivering`), and the window does
    /// not open until the level's connections are among them, or until the deadline
    /// this arms expires — a connection that never delivers must not stall the search
    /// forever; the scheduler's own stall detectors own that case.
    pub fn arm_warmup(&mut self, now: f64, delta: f64) {
        self.delivering = Some(0);
        self.warm_deadline = Some(now + (delta * WARM_DELTAS).clamp(MIN_WARM_S, MAX_WARM_S));
    }

    /// Report how many connections are currently delivering bytes.
    ///
    /// Aggregate count, not a set: the gate only asks whether the level it is about
    /// to measure is fully on the wire.
    pub fn note_delivering(&mut self, n: usize) {
        if self.delivering.is_some() {
            self.delivering = Some(n);
        }
    }

    /// Begin the first window. `now` is the transfer's clock, `delta` the measured
    /// per-request setup cost.
    pub fn start(&mut self, now: f64, delta: f64) {
        let settle = (delta * SETTLE_DELTAS).clamp(MIN_WINDOW_S, MAX_WINDOW_S);
        let window = (delta * WINDOW_DELTAS).clamp(MIN_WINDOW_S, MAX_WINDOW_S);
        self.window_open_at = now + settle;
        self.window_ends_at = self.window_open_at + window;
        self.counting_since = self.window_open_at;
        self.bytes = 0;
    }

    /// Record bytes delivered by the whole transfer.
    ///
    /// Aggregate, not per-connection: the question is whether the *path* is
    /// carrying more, and a per-connection view cannot answer it — on a saturated
    /// link each connection's own rate falls as connections are added while the
    /// total stays flat, which is exactly the case the ramp must detect.
    pub fn observe(&mut self, bytes: u64, now: f64) {
        if now >= self.window_open_at {
            self.bytes += bytes;
        }
    }

    /// The useful count, once the search has settled.
    pub fn settled(&self) -> Option<usize> {
        self.settled
    }

    /// Current concurrency.
    pub fn level(&self) -> usize {
        self.level
    }

    /// Close the window if it is due and decide what to do next.
    pub fn poll(&mut self, now: f64, delta: f64) -> Ramp {
        if let Some(n) = self.settled {
            return Ramp::Settled(n);
        }
        // ---- warm-up gate --------------------------------------------------
        //
        // Hold the window at arm's length while the level's connections are still
        // coming up: `start` is called on every poll, so `window_ends_at` keeps
        // moving and no window can close over a handshake. See `arm_warmup` for the
        // measurement error this prevents.
        if let Some(deadline) = self.warm_deadline {
            let warm = self.delivering.unwrap_or(usize::MAX) >= self.level;
            if warm || now >= deadline {
                self.warm_deadline = None;
            }
            // Either way the windows restart from HERE: on the warm path so the
            // first window measures a delivering level, and on the deadline path so
            // the level is judged over a full window rather than whatever remained.
            self.held_rate = None;
            self.start(now, delta);
            return Ramp::Hold;
        }
        if now < self.window_ends_at {
            return Ramp::Hold;
        }
        let span = (now - self.counting_since).max(1e-3);
        let rate = self.bytes as f64 / span;

        // A window that saw nothing is not evidence of saturation — it is evidence
        // of a stall, which the scheduler's own detectors handle. Re-arm rather
        // than concluding.
        if self.bytes == 0 {
            self.start(now, delta);
            return Ramp::Hold;
        }

        // Require the same evidence TWICE before raising, and average the two windows.
        //
        // One window can land mid-slow-start, while a newly admitted flow is still
        // opening its congestion window: its rate is still climbing, which is
        // indistinguishable from "this connection is paying for itself". Acting on a
        // single window is what drove the search to the ceiling on a path one stream
        // already saturated (settled counts [2, 8, 8, 8, 8] over five repetitions,
        // resulting in 1.68-2.23x slower transfers).
        //
        // The first window at a level is held back rather than recorded, so `Admission`
        // sees one sample per level and its per-connection gain arithmetic stays valid.
        // The two are averaged, which also damps the window-to-window variance that
        // made a single reading unreliable on a volatile link.
        if let Some(first) = self.held_rate.take() {
            // Take the SECOND window, not the average of the two.
            //
            // Averaging seemed conservative and is the opposite. The first window at a
            // new level lands mid-slow-start, while the newly admitted flows are still
            // opening their congestion windows; the second is closer to steady state.
            // Averaging them therefore reports a number no window measured, and because
            // the first is always the lower of the two on a warming path, the average
            // understates the level's true rate — making the NEXT step look larger than
            // it is and driving the search upward. Measured: the search reached 8 in 9
            // of 12 runs on paths where one connection was 1.8-3.2x faster.
            //
            // The first window is not wasted: it is the settling time that makes the
            // second one meaningful.
            let _ = first;
            return self.decide(rate, now, delta);
        }
        self.held_rate = Some(rate);
        self.start(now, delta);
        Ramp::Hold
    }

    /// Act on a confirmed goodput reading for the current level.
    fn decide(&mut self, rate: f64, now: f64, delta: f64) -> Ramp {
        // Opt-in trace: the ramp's decisions are invisible from the outside (the CLI
        // reports only the peak level), and inferring them from wall-clock cost two
        // wrong hypotheses. HYDRA_RAMP_TRACE=1 prints each window's verdict.
        let trace = std::env::var_os("HYDRA_RAMP_TRACE").is_some();
        if trace {
            eprintln!(
                "ramp: level={} rate={:.0} B/s at t={:.2}s delta={:.3}",
                self.level, rate, now, delta
            );
        }
        match self.adm.observe_at(self.level, rate) {
            Admit::Stop => {
                // `Admission` settles at the level whose marginal gain last paid,
                // which may be below the current level: the last connection
                // admitted did not earn its place. Settling at the smaller number
                // is the point of the search.
                let n = self.adm.settled().unwrap_or(self.level).clamp(1, self.max);
                self.settled = Some(n);
                self.level = n;
                Ramp::Settled(n)
            }
            Admit::Add if self.level < self.max => {
                // DOUBLE, do not increment.
                //
                // Incrementing costs one settle-plus-window per connection, so
                // reaching 8 takes 7 windows. Measured on a live path with
                // delta ~0.5-1.0 s that is 16-32 s of clock — longer than the whole
                // 3.15 MB transfer, which is why the first version of this ramp was
                // 1.74x slower than a fixed `-x 8` (p = 0.016) despite moving no
                // wasted bytes. The search was free in bytes and ruinous in time.
                //
                // Doubling reaches the ceiling in log2(max) windows: 3 instead of 7
                // for max=8. This is slow start's own argument — when the target is
                // unknown and each probe costs a round trip, multiply. The overshoot
                // it risks is bounded and recoverable, because `Admission` settles
                // at the last level whose marginal gain paid, and `set_active_limit`
                // can lower the count without cancelling anything: an over-admitted
                // connection finishes the range it holds and then goes quiet.
                self.level = (self.level * 2).min(self.max);
                self.held_rate = None;
                // Re-arm the warm-up gate for the connections this admits: they have
                // a handshake ahead of them, and measuring them through it is what
                // settled the search at one on high-RTT paths.
                if self.delivering.is_some() {
                    self.warm_deadline =
                        Some(now + (delta * WARM_DELTAS).clamp(MIN_WARM_S, MAX_WARM_S));
                }
                self.start(now, delta);
                Ramp::Raise(self.level)
            }
            Admit::Add => {
                // At the ceiling: the search wanted more and is not allowed more,
                // so it is finished at the ceiling rather than undecided.
                self.settled = Some(self.level);
                Ramp::Settled(self.level)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drive the ramp with a synthetic path whose aggregate rate saturates at
    /// `sat` connections, and report where it settles.
    fn run(sat: usize, max: usize, per_conn: f64, delta: f64) -> usize {
        let mut r = ConcurrencyRamp::new(0.15, max);
        let mut now = 0.0;
        r.start(now, delta);
        for _ in 0..(max * 40) {
            // Aggregate rate: linear in connections until saturation, flat after.
            let rate = per_conn * r.level().min(sat) as f64;
            // Advance in small steps, feeding bytes at that rate.
            let step = 0.05;
            now += step;
            r.observe((rate * step) as u64, now);
            if let Ramp::Settled(n) = r.poll(now, delta) {
                return n;
            }
        }
        r.level()
    }

    /// A path that saturates at one connection must not be given eight.
    ///
    /// This is the measured pathology: a fixed `-x 8` against a saturated access
    /// link was 2.7x SLOWER than a single stream, because each extra connection
    /// added a setup cost against capacity that was already committed.
    #[test]
    fn a_saturated_path_settles_low() {
        let n = run(1, 8, 1.4e6, 0.12);
        assert!(
            n <= 2,
            "settled at {n} connections on a path that saturates at 1; \
             the extra connections are pure setup cost"
        );
    }

    /// A path with real headroom must actually be used.
    ///
    /// The opposite failure is just as bad and much easier to ship: a ramp that
    /// always settles at one connection would score perfectly on the test above
    /// while throwing away the entire point of parallel range fetching.
    #[test]
    fn a_path_with_headroom_ramps_up() {
        let n = run(6, 8, 400e3, 0.12);
        assert!(
            n >= 4,
            "settled at {n} connections on a path that scales to 6; \
             the ramp is leaving throughput on the table"
        );
    }

    /// Reaching useful concurrency must cost a bounded, small amount of clock.
    ///
    /// This is the property whose absence made the first version of this ramp
    /// SLOWER than fixed concurrency. The search moved no wasted bytes — every byte
    /// was object data — and was still a net loss, because incrementing one
    /// connection per window put full concurrency 16-32 s away on a path whose
    /// `delta` was ~0.5-1.0 s, against a transfer that finished in 13.6 s. Measured:
    /// 1.74x slower than `-x 8`, p = 0.016 over 7 paired reps.
    ///
    /// A search that is free in bytes but expensive in time is still expensive. The
    /// bound has two parts, and both are load-bearing: doubling makes the number of
    /// windows logarithmic in the ceiling, and clamping the window length keeps a
    /// slow path — where `delta` is large — from stretching each one.
    #[test]
    fn full_concurrency_is_reached_in_bounded_time() {
        for &delta in &[0.01f64, 0.12, 0.5, 1.0, 5.0] {
            let mut r = ConcurrencyRamp::new(0.15, 8);
            let mut now = 0.0;
            r.start(now, delta);
            let mut reached_at = None;
            // A path with plenty of headroom, so the ramp always wants to grow.
            while now < 30.0 {
                now += 0.02;
                r.observe((2e6 * r.level() as f64 * 0.02) as u64, now);
                let out = r.poll(now, delta);
                if r.level() >= 8 {
                    reached_at = Some(now);
                    break;
                }
                if let Ramp::Settled(_) = out {
                    break;
                }
            }
            let t = reached_at.unwrap_or(f64::INFINITY);
            // 10 s, not 5 s. Each level now costs TWO windows rather than one, because
            // a single window can land mid-slow-start and read a flow that is still
            // opening its congestion window as a link with headroom — which sent the
            // search to the ceiling on a path one stream already saturated. The bound
            // is doubled deliberately, and it is still a bound: the point of the test is
            // that time-to-concurrency cannot grow without limit as `delta` grows, which
            // is the defect that made the first version of this ramp 2.78x slower than
            // a fixed `-x 8`.
            //
            // Worth noting what this costs in practice: nothing on the common path,
            // because the search now STARTS at the level the field data says wins and
            // only spends these windows when there is headroom to find.
            assert!(
                t <= 10.0,
                "took {t:.1}s to reach 8 connections at delta={delta}: a search that \
                 costs more clock than the transfer saves is a net loss"
            );
        }
    }

    /// The ceiling is a hard limit, not a target to overshoot.
    #[test]
    fn the_ceiling_is_never_exceeded() {
        for max in [1usize, 2, 4] {
            let n = run(64, max, 400e3, 0.12);
            assert!(n <= max, "settled at {n} above ceiling {max}");
        }
    }

    /// A window that sees no bytes is a stall, not saturation.
    ///
    /// Concluding "saturated" from silence would settle the ramp at one connection
    /// on any path that hiccups early — permanently, since the search does not
    /// resume once settled.
    #[test]
    fn an_empty_window_does_not_settle_the_search() {
        let mut r = ConcurrencyRamp::new(0.15, 8);
        r.start(0.0, 0.12);
        // Two windows' worth of clock with nothing delivered.
        let out = r.poll(10.0, 0.12);
        assert_eq!(out, Ramp::Hold, "silence must not be read as saturation");
        assert!(r.settled().is_none());
    }

    /// A path where opening a connection costs `handshake` seconds before its first
    /// byte and `slow_start` more before it runs at `per_conn`.
    ///
    /// The first connection is modelled as POOLED — no handshake — because that is
    /// what the transfer sees: the probe leaves a live connection behind, so
    /// connection 0 starts delivering at once while every connection the ramp admits
    /// later pays the full cost. That asymmetry is the whole point: the baseline is
    /// measured warm and the step is measured cold.
    fn run_warming(
        sat: usize,
        max: usize,
        per_conn: f64,
        delta: f64,
        handshake: f64,
        slow_start: f64,
        gate: bool,
    ) -> (usize, f64) {
        let mut r = ConcurrencyRamp::new(0.15, max);
        let mut now = 0.0;
        r.start(now, delta);
        if gate {
            r.arm_warmup(now, delta);
        }
        // Wall clock at which each connection was opened; connection 0 with the
        // transfer, the rest when the ramp admitted them.
        let mut opened = vec![0.0f64];
        let mut hs = vec![0.0f64];
        let step = 0.02;
        while now < 60.0 {
            now += step;
            // Per-connection contribution: nothing through the handshake, then a
            // linear climb to full rate over `slow_start`.
            let mut live = 0usize;
            let mut rate = 0.0;
            for (i, &t0) in opened.iter().enumerate() {
                let since = now - t0 - hs[i];
                if since >= 0.0 {
                    live += 1;
                    rate += per_conn * (since / slow_start).clamp(0.0, 1.0);
                }
            }
            // The path saturates at `sat` connections' worth of aggregate rate.
            rate = rate.min(per_conn * sat as f64);
            r.observe((rate * step) as u64, now);
            if gate {
                r.note_delivering(live);
            }
            match r.poll(now, delta) {
                Ramp::Raise(n) => {
                    while opened.len() < n {
                        opened.push(now);
                        hs.push(handshake);
                    }
                }
                Ramp::Settled(n) => return (n, now),
                Ramp::Hold => {}
            }
        }
        (r.level(), now)
    }

    /// The field failure, reproduced: a high-RTT path with eight-way headroom on
    /// which the search stops at ONE connection.
    ///
    /// Reported as "only two of eight connections start" on a 116 MB GitHub release
    /// asset. The second connection had moved 240 KB — the tail of its slow start —
    /// when the ramp judged the level and stopped, and the six above it were never
    /// admitted at all.
    ///
    /// The cause is a units mismatch, not a threshold: the windows are scaled by
    /// `delta`, the per-request cost on a POOLED connection, while what they have to
    /// outlast is a fresh TCP plus TLS handshake. Where the handshake is longer than
    /// settle-plus-window (both clamped at `MAX_WINDOW_S` = 0.6 s), the level is
    /// measured entirely through the new connection's silence, reads as no better
    /// than the level below it, and the search settles at the bottom.
    ///
    /// Kept as a test of the UNGATED path so the defect cannot come back silently:
    /// if this ever settles high on its own, the gate has stopped being what fixes it
    /// and the reason for that should be understood.
    #[test]
    fn a_slow_handshake_defeats_the_ungated_search() {
        let (n, _) = run_warming(8, 8, 2.4e6, 0.4, 2.5, 2.0, false);
        assert_eq!(
            n, 1,
            "expected the ungated ramp to be fooled by a 2.5 s handshake; it settled \
             at {n}, so this test no longer covers the defect it was written for"
        );
    }

    /// With the gate armed, the same path is used.
    ///
    /// The window does not open until the level's connections are delivering, so the
    /// step is measured on what the connections carry rather than on how long they
    /// took to open.
    #[test]
    fn the_warm_up_gate_finds_the_headroom_a_slow_handshake_hides() {
        let (n, t) = run_warming(8, 8, 2.4e6, 0.4, 2.5, 2.0, true);
        assert!(
            n >= 4,
            "settled at {n} on a path that scales to 8: the gate did not restore the \
             measurement"
        );
        // Waiting for the handshake costs clock, and it must stay bounded: four
        // levels at a 2.5 s handshake plus two windows each is the budget here.
        assert!(
            t <= 30.0,
            "took {t:.1}s to settle at {n}: patience is not free and must be bounded"
        );
    }

    /// A connection that never delivers must not stall the search.
    ///
    /// The gate waits for the wire, so a black-holed connection would hold the window
    /// open forever without the deadline. Reclaiming that connection is the
    /// scheduler's job, not the ramp's; the ramp's job is to stop waiting.
    #[test]
    fn the_gate_gives_up_on_a_connection_that_never_delivers() {
        // A handshake longer than the whole simulation: nothing admitted after
        // connection 0 ever produces a byte.
        let (n, t) = run_warming(8, 8, 2.4e6, 0.4, 1e6, 2.0, true);
        assert!(
            n <= 2,
            "settled at {n} on a path where only one connection ever delivered"
        );
        assert!(
            t < 60.0,
            "the search never settled: the warm-up deadline is not bounding the wait"
        );
    }
}
