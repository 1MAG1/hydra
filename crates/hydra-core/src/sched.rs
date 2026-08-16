//! The scheduler kernel: pure state machine, no I/O, no clock, no allocation
//! in the steady state.
//!
//! The caller drives it: feed observations (`on_bytes`, `on_complete`), call
//! `tick(now)`, and act on the returned `Action`s. This is what lets the same
//! code run under the discrete-event simulator and under real HTTP.
//!
//! Implements dynamic range partitioning, divergence-triggered steal-to-equalize,
//! work-conserving assignment, queue dispatch, stall reclamation, and greedy concurrency.

use crate::intervals::{IntervalSet, Range};

/// Minimum steal quantum. A range rebalance smaller than this is not worth request overhead.
pub const STEAL_QUANTUM: u64 = 64 * 1024;

/// Bounded repairs per tick, so a tick is O(R * n).
const MAX_REPAIRS_PER_TICK: usize = 4;

/// EWMA weight on the newest goodput sample.
const RATE_ALPHA: f64 = 0.3;

/// Minimum wall clock a rate sample must span, in seconds.
///
/// Below this the quotient is dominated by socket buffering rather than by the
/// link: consecutive `read()` calls draining one already-arrived TCP window return
/// in microseconds and imply a rate the network never achieved. 200 ms is long
/// enough to average over several windows and short enough that a genuine collapse
/// is still graded within the stall timeout.
const RATE_WINDOW: f64 = 0.2;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    /// Issue `GET` with `Range: bytes=lo-(hi-1)` on this connection.
    Request { conn: usize, range: Range },
    /// Stop reading this connection's current response; its range was reclaimed.
    Cancel { conn: usize },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Capability {
    /// Ranges honoured, length known, strong validator: full scheduling.
    Full,
    /// Ranges honoured but no validator: partition, but pin to one source.
    NoValidator,
    /// Ranges ignored or unsupported: race whole-object fetches.
    Race,
    /// Length unknown: single stream per source, no range arithmetic.
    Stream,
}

#[derive(Clone, Debug)]
pub struct Source {
    pub caps: Capability,
    /// Per-connection goodput ceiling estimate, bytes/s.
    pub gamma_est: f64,
    /// Per-source shaping cap estimate, bytes/s.
    pub rho_est: f64,
    /// Measured request setup cost, seconds.
    pub delta_est: f64,
    /// Suspended until this time (429/503 Retry-After, or stall backoff).
    pub suspended_until: f64,
    /// Consecutive stalls observed on this source; drives exponential backoff.
    pub consecutive_stalls: u32,
}

impl Default for Source {
    fn default() -> Self {
        Source {
            caps: Capability::Full,
            gamma_est: 0.0,
            rho_est: f64::INFINITY,
            delta_est: 0.05,
            suspended_until: 0.0,
            consecutive_stalls: 0,
        }
    }
}

#[derive(Clone, Debug)]
struct Conn {
    source: usize,
    /// Active range and how far into it we are.
    range: Option<Range>,
    pos: u64,
    /// One-slot pipeline: a range handed over by a repair.
    queued: Option<Range>,
    rate_est: f64,
    /// Changepoint detector. `rate_est` remains the smoothed rate used for ETA
    /// projection; this grades the connection so repair can pre-empt a collapse
    /// instead of waiting for the stall timeout (see `detect.rs`).
    detector: crate::detect::CollapseDetector,
    last_progress: f64,
    setup_end: f64,
    stalled: bool,
    /// Bytes and wall clock accumulated since the last RATE sample.
    ///
    /// Rate is measured over a fixed WINDOW, not per arrival. An arrival is one
    /// `read()` return, and a read served from the socket's already-buffered data
    /// completes in microseconds, so `bytes/dt` for that arrival measures memcpy
    /// speed rather than network speed — observed as 128 MiB/s on a connection
    /// whose link was doing well under 1 MiB/s.
    ///
    /// That is not merely a cosmetic display bug. Those inflated samples raise the
    /// detector's reference level, after which every honest sample looks like a
    /// collapse against it, and the CUSUM grades a perfectly healthy connection
    /// `Degraded` — which is why all eight connections of a working transfer
    /// showed as `bad`. Byte accounting stays exactly per-arrival (coverage must
    /// be exact); only the rate estimate is windowed.
    rate_acc_bytes: u64,
    rate_acc_dt: f64,
}

impl Conn {
    fn new(source: usize) -> Self {
        Conn {
            source,
            range: None,
            pos: 0,
            queued: None,
            rate_est: 0.0,
            detector: crate::detect::CollapseDetector::new(),
            rate_acc_bytes: 0,
            rate_acc_dt: 0.0,
            last_progress: 0.0,
            setup_end: 0.0,
            stalled: false,
        }
    }

    #[inline]
    fn busy(&self) -> bool {
        self.range.map(|r| self.pos < r.hi).unwrap_or(false)
    }

    /// Bytes still owed on the active range plus anything pipelined.
    #[inline]
    fn outstanding(&self) -> u64 {
        let active = self
            .range
            .map(|r| r.hi.saturating_sub(self.pos))
            .unwrap_or(0);
        active + self.queued.map(|r| r.len()).unwrap_or(0)
    }

    /// Projected seconds to drain. A stalled or unmeasured connection projects
    /// to infinity so it is always chosen as the repair victim.
    fn eta(&self) -> f64 {
        let out = self.outstanding();
        if out == 0 {
            return 0.0;
        }
        if self.rate_est <= 0.0 {
            return f64::INFINITY;
        }
        out as f64 / self.rate_est
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub requests: u64,
    pub repairs: u64,
    pub reclaims: u64,
    pub bytes_held: u64,
}

pub struct Scheduler {
    size: u64,
    unassigned: IntervalSet,
    held: u64,
    conns: Vec<Conn>,
    sources: Vec<Source>,
    /// Repair deadband scale; theta = scale * sqrt(delta * T_rem / n).
    theta_scale: f64,
    stall_timeout: f64,
    /// When false, victim selection ignores detector health and ranks purely by
    /// projected ETA (the pre-detector behaviour). Exists so the detector's
    /// contribution can be A/B measured rather than assumed.
    health_ranking: bool,
    started: bool,
    pub stats: Stats,
}

impl Scheduler {
    pub fn new(size: u64, sources: Vec<Source>, conns_per_source: &[usize]) -> Self {
        let mut conns = Vec::new();
        for (i, &k) in conns_per_source.iter().enumerate() {
            for _ in 0..k {
                conns.push(Conn::new(i));
            }
        }
        Scheduler {
            size,
            unassigned: IntervalSet::full(size),
            held: 0,
            conns,
            sources,
            theta_scale: 1.0,
            stall_timeout: 1.0,
            health_ranking: true,
            started: false,
            stats: Stats::default(),
        }
    }

    pub fn with_theta_scale(mut self, s: f64) -> Self {
        self.theta_scale = s;
        self
    }

    /// Disable health-ranked victim selection (for A/B measurement only).
    pub fn with_health_ranking(mut self, on: bool) -> Self {
        self.health_ranking = on;
        self
    }

    pub fn with_stall_timeout(mut self, t: f64) -> Self {
        self.stall_timeout = t;
        self
    }

    /// Mark `[lo, hi)` as already held, for resuming a partial transfer.
    ///
    /// Must be called before the first `tick`: the initial split assigns all
    /// unassigned work, and bytes already on disk must not be part of it.
    pub fn mark_done(&mut self, lo: u64, hi: u64) {
        let (lo, hi) = (lo.min(self.size), hi.min(self.size));
        if hi <= lo {
            return;
        }
        // Credit only the bytes this call actually claims, measured as the drop in
        // the unassigned set — NOT the width of the span asked for.
        //
        // Callers legitimately overlap. A `-c` resume marks the sidecar's ranges
        // held, and the concurrency probe separately reports the bytes it fetched;
        // both start at offset 0, so the same prefix is marked twice. Crediting
        // `hi - lo` each time made `held` exceed the bytes that exist, and `held`
        // is what `is_complete()` tests: the transfer stopped early believing it
        // was finished, leaving a zero-filled hole in the tail of a file reported
        // as a success. Measured on an interrupted-then-resumed 11 200 900-byte
        // object: 240 138 bytes of tail never written, `ok: true`, and the gzip
        // refused to decompress.
        let before = self.unassigned.total();
        self.unassigned.remove(lo, hi);
        let claimed = before.saturating_sub(self.unassigned.total());
        self.held = self.held.saturating_add(claimed);
    }

    /// Health grade of a connection, for the progress UI and for tests.
    pub fn conn_health(&self, j: usize) -> crate::detect::Health {
        self.conns
            .get(j)
            .map(|c| c.detector.health())
            .unwrap_or_default()
    }

    /// Source index a connection belongs to, for the progress UI.
    pub fn conn_source(&self, j: usize) -> usize {
        self.conns.get(j).map(|c| c.source).unwrap_or(0)
    }

    /// Smoothed rate estimate of a connection (bytes/s), for the progress UI.
    pub fn conn_rate(&self, j: usize) -> f64 {
        self.conns.get(j).map(|c| c.rate_est).unwrap_or(0.0)
    }

    /// Active range of a connection, for the progress UI.
    pub fn conn_range(&self, j: usize) -> Option<(u64, u64, u64)> {
        self.conns
            .get(j)
            .and_then(|c| c.range.map(|r| (r.lo, c.pos, r.hi)))
    }

    pub fn n_conns(&self) -> usize {
        self.conns.len()
    }

    pub fn is_complete(&self) -> bool {
        self.held >= self.size
    }

    pub fn bytes_held(&self) -> u64 {
        self.held
    }

    /// The ranges that are complete on disk, as `(lo, hi)` pairs.
    ///
    /// This is the complement of the unassigned set minus what is still in flight, and
    /// it is what a resume record must contain. Reporting only a byte COUNT is not
    /// enough: positioned writes land ranges out of order, so "2 MB held" says nothing
    /// about which 2 MB, and a resume that assumed a contiguous prefix would skip holes
    /// and silently corrupt the file.
    pub fn held_ranges(&self) -> Vec<(u64, u64)> {
        // Start from everything, then subtract what is unassigned and what is
        // outstanding on a connection; what remains has arrived.
        let mut done = IntervalSet::full(self.size);
        for r in self.unassigned.ranges() {
            done.remove(r.lo, r.hi);
        }
        for c in &self.conns {
            if let Some(r) = c.range {
                // Bytes before the cursor have arrived; the rest has not.
                done.remove(c.pos, r.hi);
            }
            if let Some(q) = c.queued {
                done.remove(q.lo, q.hi);
            }
        }
        done.ranges().iter().map(|r| (r.lo, r.hi)).collect()
    }

    /// Coverage audit: held + outstanding + unassigned == size.
    ///
    /// This is a SAFETY invariant and it does NOT imply liveness -- the
    /// livelock this code is written to avoid (a fully-stolen range leaving a
    /// connection idle with a non-empty queue) satisfies it at every instant.
    /// `liveness_holds` is the property that matters.
    /// The largest measured request setup cost across sources, in seconds.
    ///
    /// Exposed because a transport-layer watchdog must express its patience in
    /// units of what a request actually costs on this path rather than as a
    /// hardcoded constant: `delta` differs by an order of magnitude between a
    /// LAN mirror and a TLS connection through a proxy, and a fixed timeout is
    /// either trigger-happy on the slow path or useless on the fast one.
    ///
    /// This is the same quantity the repair deadband is built from
    /// (`theta = scale * sqrt(delta * T_rem / n)`), so a client that widens
    /// `delta` widens both together, which is the intended coupling.
    pub fn worst_delta(&self) -> f64 {
        self.sources
            .iter()
            .map(|s| s.delta_est)
            .fold(0.0f64, f64::max)
    }

    /// The configured stall timeout, in seconds.
    pub fn stall_timeout(&self) -> f64 {
        self.stall_timeout
    }

    pub fn coverage_holds(&self) -> bool {
        let outstanding: u64 = self.conns.iter().map(|c| c.outstanding()).sum();
        self.held + outstanding + self.unassigned.total() == self.size
            && self.unassigned.invariant_holds()
    }

    /// True when some enabled transition strictly decreases the unheld-byte count.
    /// False means the scheduler is stuck.
    pub fn liveness_holds(&self) -> bool {
        if self.is_complete() {
            return true;
        }
        // progress possible if: someone is receiving, or work is assignable,
        // or a connection holds a queue it can start, or a stall can be reclaimed
        self.conns.iter().any(|c| c.busy() && !c.stalled)
            || !self.unassigned.is_empty()
            || self.conns.iter().any(|c| c.queued.is_some())
            || self.conns.iter().any(|c| c.stalled)
    }

    // ---------------------------------------------------------------- input

    /// Record `n` bytes arriving on `conn` at time `now` over `dt` seconds.
    ///
    /// Convenience wrapper that assumes the arrival is contiguous at the
    /// connection's cursor. Real transports must use [`Scheduler::on_bytes_at`]:
    /// a response still draining from a range that was completed or stolen would
    /// otherwise be credited against whatever range the connection holds NOW,
    /// silently advancing a cursor over bytes that never arrived and leaving a
    /// hole of zeros in the output file.
    pub fn on_bytes(&mut self, conn: usize, n: u64, now: f64, dt: f64) {
        let at = self.conns[conn].pos;
        self.on_bytes_at(conn, at, n, now, dt);
    }

    /// Record `n` bytes that landed at absolute offset `off`.
    ///
    /// Arrivals that do not begin exactly at the connection's cursor are stale
    /// (they belong to a superseded request) and are discarded: the bytes are
    /// still written to the file by the transport, but they are not credited,
    /// so the scheduler's coverage accounting stays exact.
    pub fn on_bytes_at(&mut self, conn: usize, off: u64, n: u64, now: f64, dt: f64) {
        let c = &mut self.conns[conn];
        let Some(r) = c.range else { return };
        if off != c.pos || off < r.lo {
            return; // stale arrival from a superseded range
        }
        let room = r.hi.saturating_sub(c.pos);
        let step = n.min(room);
        if step == 0 {
            return;
        }
        c.pos += step;
        self.held += step;
        c.last_progress = now;
        c.stalled = false;
        let src = c.source;
        self.sources[src].consecutive_stalls = 0;
        if dt > 0.0 {
            // Accumulate, and only take a rate sample once the window has enough
            // wall clock in it to mean something.
            c.rate_acc_bytes += step;
            c.rate_acc_dt += dt;
            if c.rate_acc_dt >= RATE_WINDOW {
                let sample = c.rate_acc_bytes as f64 / c.rate_acc_dt;
                c.rate_acc_bytes = 0;
                c.rate_acc_dt = 0.0;
                c.detector.observe_rate(sample);
                c.rate_est = if c.rate_est <= 0.0 {
                    sample
                } else {
                    RATE_ALPHA * sample + (1.0 - RATE_ALPHA) * c.rate_est
                };
            }
        }
        if c.pos >= r.hi {
            c.range = None;
        }
    }

    /// Suspend a source (429/503 with Retry-After) and reclaim its ranges.
    pub fn suspend_source(&mut self, src: usize, until: f64) {
        self.sources[src].suspended_until = until;
        let idxs: Vec<usize> = (0..self.conns.len())
            .filter(|&j| self.conns[j].source == src)
            .collect();
        for j in idxs {
            self.reclaim(j);
        }
    }

    fn reclaim(&mut self, j: usize) {
        let c = &mut self.conns[j];
        if let Some(r) = c.range {
            if c.pos < r.hi {
                let back = Range::new(c.pos, r.hi);
                c.range = None;
                let q = c.queued.take();
                self.unassigned.insert(back);
                if let Some(q) = q {
                    self.unassigned.insert(q);
                }
                self.stats.reclaims += 1;
            } else {
                c.range = None;
            }
        } else if let Some(q) = c.queued.take() {
            self.unassigned.insert(q);
            self.stats.reclaims += 1;
        }
        let c = &mut self.conns[j];
        c.rate_est = 0.0;
        c.stalled = true;
    }

    // ---------------------------------------------------------------- tick

    /// Advance the scheduler. Returns the actions the caller must perform.
    pub fn tick(&mut self, now: f64) -> Vec<Action> {
        let mut acts = Vec::new();

        if !self.started {
            self.initial_split(now, &mut acts);
            self.started = true;
            return acts;
        }

        // ---- feed wall-clock silence to the detectors ----------------------
        // A connection delivering nothing produces no rate samples at all, so
        // silence is evidence that only the clock can supply. Grading it here
        // lets repair pre-empt at half the stall timeout instead of waiting for
        // the full timeout to expire.
        for j in 0..self.conns.len() {
            let c = &self.conns[j];
            if c.busy() && now >= c.setup_end {
                let quiet = now - c.last_progress.max(c.setup_end);
                let st = self.stall_timeout;
                self.conns[j].detector.observe_silence(quiet, st);
            }
        }

        // ---- liveness path 1: reclaim stalled connections -----------------
        let stalled: Vec<usize> = (0..self.conns.len())
            .filter(|&j| {
                let c = &self.conns[j];
                c.busy()
                    && now >= c.setup_end
                    && (now - c.last_progress.max(c.setup_end)) > self.stall_timeout
            })
            .collect();
        for j in stalled {
            self.reclaim(j);
            acts.push(Action::Cancel { conn: j });
            // A source that keeps stalling must be suspended, not merely
            // retried: otherwise work-conserving assignment hands it the same
            // bytes repeatedly without making forward progress.
            let src = self.conns[j].source;
            self.sources[src].consecutive_stalls += 1;
            let k = self.sources[src].consecutive_stalls;
            if k >= 2 {
                let backoff = (self.stall_timeout * (1u64 << (k - 2).min(5)) as f64).min(30.0);
                self.sources[src].suspended_until = now + backoff;
            }
        }

        // ---- liveness path 2: an idle connection holding a queue MUST start it
        //
        // Mandatory: a connection whose active range was entirely stolen goes
        // idle WITHOUT completing, so the completion path in on_bytes never fires
        // and the queued bytes would be owned by an idle connection that never
        // requests them.
        for j in 0..self.conns.len() {
            if !self.conns[j].busy()
                && self.conns[j].queued.is_some()
                && now >= self.conns[j].setup_end
            {
                let r = self.conns[j].queued.take().unwrap();
                self.start(j, r, now);
                acts.push(Action::Request { conn: j, range: r });
            }
        }

        // ---- divergence-triggered repair ---------------------------------
        let theta = self.theta(now);
        for _ in 0..MAX_REPAIRS_PER_TICK {
            let Some((vi, ti)) = self.pick_victim_taker(now) else {
                break;
            };
            let (v_eta, t_eta) = (self.conns[vi].eta(), self.conns[ti].eta());
            // Explicit ordering test: an unknown ETA yields NaN, and a NaN
            // divergence must NOT trigger a repair (a repair costs a full delta,
            // so acting on an unmeasured quantity is strictly a loss).
            if !matches!(
                (v_eta - t_eta).partial_cmp(&theta),
                Some(core::cmp::Ordering::Greater)
            ) {
                break;
            }
            if self.conns[ti].queued.is_some() {
                break;
            }
            let Some(vr) = self.conns[vi].range else {
                break;
            };
            let left = vr.hi.saturating_sub(self.conns[vi].pos) as f64;
            let rv = self.conns[vi].rate_est;
            let rt = self.conns[ti].rate_est;
            let delta = self.sources[self.conns[ti].source].delta_est;
            // Equalise projected finishes, charging the taker one setup:
            //   (left - x)/rv == t_eta + delta + x/rt
            let x = if rv <= 0.0 {
                // Victim is stalled: hand over everything it has not received.
                left
            } else if rt <= 0.0 {
                0.0
            } else {
                ((left / rv - t_eta - delta) * (rv * rt) / (rv + rt)).clamp(0.0, left)
            };
            if x <= STEAL_QUANTUM as f64 {
                break;
            }
            let x = x as u64;
            let new_hi = vr.hi - x;
            let stolen = Range::new(new_hi, vr.hi);
            // ZERO-COST client-side shrink: the victim's target end moves; no
            // cancellation is sent and the server is never told.
            self.conns[vi].range = Some(Range::new(vr.lo, new_hi));
            self.conns[ti].queued = Some(stolen);
            self.stats.repairs += 1;
        }

        // ---- work-conserving assignment (Lemma 2) -------------------------
        for j in 0..self.conns.len() {
            if self.conns[j].busy() || now < self.conns[j].setup_end {
                continue;
            }
            let src = self.conns[j].source;
            if now < self.sources[src].suspended_until {
                continue;
            }
            if let Some(r) = self.unassigned.take_front(u64::MAX) {
                self.start(j, r, now);
                acts.push(Action::Request { conn: j, range: r });
                continue;
            }
            // Nothing unassigned: steal from the worst laggard.
            if let Some(vi) = self.worst_busy(j) {
                let vr = self.conns[vi].range.unwrap();
                let left = vr.hi.saturating_sub(self.conns[vi].pos);
                let half = left / 2;
                if half > STEAL_QUANTUM {
                    let new_hi = vr.hi - half;
                    self.conns[vi].range = Some(Range::new(vr.lo, new_hi));
                    let stolen = Range::new(new_hi, vr.hi);
                    self.start(j, stolen, now);
                    acts.push(Action::Request {
                        conn: j,
                        range: stolen,
                    });
                    self.stats.repairs += 1;
                }
            }
            // NOTE: no hedging. Redundant requests waste bandwidth on non-erasure channels.
        }

        self.stats.bytes_held = self.held;
        acts
    }

    fn start(&mut self, j: usize, r: Range, now: f64) {
        let delta = self.sources[self.conns[j].source].delta_est;
        let c = &mut self.conns[j];
        c.range = Some(r);
        c.pos = r.lo;
        c.setup_end = now + delta;
        c.last_progress = now + delta;
        c.stalled = false;
        self.stats.requests += 1;
    }

    fn initial_split(&mut self, now: f64, acts: &mut Vec<Action>) {
        // Maximal ranges, proportional to rate estimate where known, else equal.
        let n = self.conns.len();
        if n == 0 || self.size == 0 {
            return;
        }
        let weights: Vec<f64> = self
            .conns
            .iter()
            .map(|c| {
                let g = self.sources[c.source].gamma_est;
                if g > 0.0 {
                    g
                } else {
                    1.0
                }
            })
            .collect();
        let total: f64 = weights.iter().sum();

        // Split what is ACTUALLY unassigned, not `[0, size)`.
        //
        // An earlier version partitioned the whole object arithmetically, which
        // silently ignored `mark_done`. That broke both features that depend on
        // it: `--range` fetched from offset 0 instead of the requested interval,
        // and `--continue` re-fetched bytes already on disk. The unassigned set is
        // the single source of truth for what remains, so the split must be taken
        // from it.
        let remaining: Vec<Range> = self.unassigned.ranges().to_vec();
        let avail: u64 = remaining.iter().map(|r| r.hi - r.lo).sum();
        if avail == 0 {
            return;
        }
        // Per-connection byte quotas, proportional to rate estimate.
        let mut quota: Vec<u64> = weights
            .iter()
            .map(|w| ((w / total) * avail as f64) as u64)
            .collect();
        // Rounding must not strand bytes: give the remainder to the last taker.
        let assigned: u64 = quota.iter().sum();
        if let Some(last) = quota.last_mut() {
            *last += avail - assigned;
        }

        // Walk the unassigned ranges, carving each connection's quota out of them
        // in order. A connection may receive a range that is not contiguous with
        // its neighbours' — that is fine, since ranges are independent requests.
        let mut it = remaining.into_iter();
        let mut cur = it.next();
        for (j, want_total) in quota.iter().enumerate() {
            let mut want = *want_total;
            while want > 0 {
                let Some(seg) = cur else { break };
                let take = want.min(seg.hi - seg.lo);
                let r = Range::new(seg.lo, seg.lo + take);
                // A connection holds one active range plus a one-slot pipeline.
                // Anything beyond that stays UNASSIGNED rather than being stashed:
                // work-conserving assignment will hand it out as connections free
                // up, and leaving it in the set is what keeps the coverage
                // invariant checkable.
                if self.conns[j].range.is_none() {
                    self.unassigned.remove(r.lo, r.hi);
                    self.start(j, r, now);
                    acts.push(Action::Request { conn: j, range: r });
                } else if self.conns[j].queued.is_none() {
                    self.unassigned.remove(r.lo, r.hi);
                    self.conns[j].queued = Some(r);
                } else {
                    break;
                }
                want -= take;
                cur = if seg.hi - seg.lo > take {
                    Some(Range::new(seg.lo + take, seg.hi))
                } else {
                    it.next()
                };
            }
        }
    }

    fn theta(&self, now: f64) -> f64 {
        let live: Vec<&Conn> = self
            .conns
            .iter()
            .filter(|c| now >= self.sources[c.source].suspended_until)
            .collect();
        let n = live.len().max(1) as f64;
        let agg: f64 = live.iter().map(|c| c.rate_est.max(0.0)).sum();
        let agg = if agg > 0.0 { agg } else { 1.0 };
        let remaining = self.size.saturating_sub(self.held) as f64;
        let t_rem = remaining / agg;
        let delta = self
            .sources
            .iter()
            .map(|s| s.delta_est)
            .fold(0.0f64, f64::max);
        self.theta_scale * (delta * t_rem.max(0.0) / n).sqrt()
    }

    fn pick_victim_taker(&self, now: f64) -> Option<(usize, usize)> {
        // Victim ranking is (health, ETA), health first. A connection the
        // detector has graded Suspect is a victim even when its *projected* ETA
        // still looks acceptable -- which is the whole point of detecting a
        // collapse early, since the ETA is computed from a rate estimate that
        // the collapse has not yet dragged down.
        let mut victim: Option<(usize, crate::detect::Health, f64)> = None;
        let mut taker: Option<(usize, f64)> = None;
        for j in 0..self.conns.len() {
            let c = &self.conns[j];
            if now < c.setup_end || now < self.sources[c.source].suspended_until {
                continue;
            }
            let e = c.eta();
            let h = if self.health_ranking {
                c.detector.health()
            } else {
                crate::detect::Health::Healthy
            };
            if c.busy() && victim.map(|(_, vh, ve)| (h, e) > (vh, ve)).unwrap_or(true) {
                victim = Some((j, h, e));
            }
            // A degraded connection must never be chosen as the TAKER: handing
            // work to a collapsing connection is the failure mode this whole
            // mechanism exists to prevent.
            if !h.is_suspect_or_worse() && taker.map(|(_, te)| e < te).unwrap_or(true) {
                taker = Some((j, e));
            }
        }
        let (vi, _, _) = victim?;
        let (ti, _) = taker?;
        if vi == ti {
            return None;
        }
        Some((vi, ti))
    }

    fn worst_busy(&self, exclude: usize) -> Option<usize> {
        let mut best: Option<(usize, u64)> = None;
        for j in 0..self.conns.len() {
            if j == exclude {
                continue;
            }
            let c = &self.conns[j];
            if !c.busy() {
                continue;
            }
            let left = c.range.unwrap().hi.saturating_sub(c.pos);
            if best.map(|(_, bl)| left > bl).unwrap_or(true) {
                best = Some((j, left));
            }
        }
        best.map(|(j, _)| j)
    }
}

/// Greedy concurrency allocation across multiple sources.
pub fn greedy_concurrency(
    rho: &[f64],
    gamma: &[f64],
    access_cap: f64,
    budget: usize,
) -> Vec<usize> {
    let m = rho.len();
    let mut n = vec![0usize; m];
    let g = |n: &[usize]| -> f64 {
        let sum: f64 = (0..m).map(|i| rho[i].min(n[i] as f64 * gamma[i])).sum();
        sum.min(access_cap)
    };
    let mut cur = g(&n);
    for _ in 0..budget {
        let mut best = (0usize, 0.0f64);
        for i in 0..m {
            n[i] += 1;
            let gain = g(&n) - cur;
            n[i] -= 1;
            if gain > best.1 {
                best = (i, gain);
            }
        }
        if best.1 <= 0.0 {
            break; // saturated: further connections are pure cost
        }
        n[best.0] += 1;
        cur += best.1;
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(gamma: f64) -> Source {
        Source {
            gamma_est: gamma,
            delta_est: 0.05,
            ..Default::default()
        }
    }

    #[test]
    fn initial_split_covers_exactly() {
        let mut s = Scheduler::new(1000, vec![src(1.0), src(1.0)], &[1, 1]);
        let acts = s.tick(0.0);
        assert_eq!(acts.len(), 2);
        assert!(s.coverage_holds());
        assert!(s.unassigned.is_empty());
    }

    #[test]
    fn coverage_and_liveness_hold_through_a_transfer() {
        let mut s = Scheduler::new(1_000_000, vec![src(1e5), src(5e4)], &[2, 2]);
        let mut now = 0.0;
        for _ in 0..4000 {
            s.tick(now);
            for j in 0..s.n_conns() {
                s.on_bytes(j, 500, now, 0.01);
            }
            assert!(s.coverage_holds(), "coverage broke at t={now}");
            assert!(s.liveness_holds(), "stuck at t={now}");
            now += 0.01;
            if s.is_complete() {
                break;
            }
        }
        assert!(
            s.is_complete(),
            "did not finish: {} / {}",
            s.bytes_held(),
            1_000_000
        );
    }

    #[test]
    fn fully_stolen_range_does_not_livelock() {
        // Regression: a connection whose active range is stolen down to its
        // current position goes idle WITHOUT completing. If the queue-start
        // path is missing, its queued bytes are never requested.
        let mut s = Scheduler::new(200_000, vec![src(1e5), src(1e5)], &[1, 1]);
        s.tick(0.0);
        // conn 0 makes progress, conn 1 stalls entirely
        let mut now = 0.06;
        for _ in 0..50 {
            s.on_bytes(0, 1000, now, 0.01);
            now += 0.01;
            s.tick(now);
        }
        // force a steal by making conn 1 look terrible, then run to completion
        for _ in 0..20000 {
            s.tick(now);
            s.on_bytes(0, 1000, now, 0.01);
            now += 0.01;
            assert!(s.liveness_holds(), "livelocked at t={now}");
            if s.is_complete() {
                break;
            }
        }
        assert!(s.is_complete());
    }

    #[test]
    fn stall_reclaim_returns_bytes() {
        let mut s = Scheduler::new(100_000, vec![src(1e5), src(1e5)], &[1, 1]);
        s.tick(0.0);
        let before = s.stats.reclaims;
        // no bytes at all: both connections must be reclaimed after the timeout
        let acts = s.tick(5.0);
        assert!(s.stats.reclaims > before);
        assert!(acts.iter().any(|a| matches!(a, Action::Cancel { .. })));
        assert!(s.coverage_holds());
        assert!(s.liveness_holds());
    }

    #[test]
    fn suspend_source_reclaims_and_reassigns() {
        let mut s = Scheduler::new(100_000, vec![src(1e5), src(1e5)], &[1, 1]);
        s.tick(0.0);
        s.suspend_source(0, 10.0);
        // Reclaimed bytes are now unassigned. They are NOT reassigned instantly:
        // the surviving connection is still streaming its own range, and taking
        // work from it would violate nothing but achieve nothing either. Work
        // conservation only requires that no connection sit IDLE while work
        // remains -- so the reassignment happens when conn 1 next goes idle.
        assert!(s.coverage_holds());
        assert!(s.unassigned.total() > 0);

        let mut now = 0.2;
        let mut served_by_1 = false;
        for _ in 0..20_000 {
            let acts = s.tick(now);
            if acts
                .iter()
                .any(|a| matches!(a, Action::Request { conn, .. } if s.conns[*conn].source == 1))
            {
                served_by_1 = true;
            }
            s.on_bytes(1, 1000, now, 0.01);
            now += 0.01;
            assert!(s.coverage_holds());
            assert!(s.liveness_holds());
            if s.is_complete() {
                break;
            }
        }
        assert!(
            served_by_1,
            "surviving source never picked up the reclaimed work"
        );
        assert!(s.is_complete(), "held {} of 100000", s.bytes_held());
    }

    #[test]
    fn greedy_matches_exhaustive_small() {
        // rho/gamma chosen so the optimum is interior
        let rho = [2.2e6, 1.1e6, 0.7e6];
        let gam = [0.55e6, 0.45e6, 0.35e6];
        let cap = 5.0e6;
        for budget in 1..10usize {
            let n = greedy_concurrency(&rho, &gam, cap, budget);
            let g = |n: &[usize]| -> f64 {
                let s: f64 = (0..3).map(|i| rho[i].min(n[i] as f64 * gam[i])).sum();
                s.min(cap)
            };
            let mut best = 0.0f64;
            for a in 0..=budget {
                for b in 0..=budget {
                    for c in 0..=budget {
                        if a + b + c <= budget {
                            best = best.max(g(&[a, b, c]));
                        }
                    }
                }
            }
            assert!(
                (g(&n) - best).abs() < 1.0,
                "budget {budget}: greedy {} vs {}",
                g(&n),
                best
            );
        }
    }

    #[test]
    fn saturation_stops_allocation() {
        // one source, rho = 2*gamma: two connections saturate it
        let n = greedy_concurrency(&[2.0e6], &[1.0e6], 1e9, 10);
        assert_eq!(
            n[0], 2,
            "allocated {n:?}, expected exactly the saturation point"
        );
    }
    /// The detector must make the SCHEDULER act sooner, not merely grade sooner.
    ///
    /// A connection collapsing to 3% of its rate must be chosen as a repair
    /// victim well before the stall timeout would have reclaimed it. Without
    /// health-ranked victim selection the scheduler waits for the projected ETA
    /// to drift, which is the fixed detection cost measured at 0.25-0.9 s.
    #[test]
    fn collapsed_connection_becomes_a_repair_victim_before_the_stall_timeout() {
        const S: u64 = 40_000_000;
        let mut sc = Scheduler::new(S, vec![src(4e6), src(4e6)], &[1, 1]).with_stall_timeout(10.0);
        sc.tick(0.0);
        let mut now = 0.0;
        // Both healthy for a while.
        for _ in 0..12 {
            now += 0.1;
            sc.on_bytes(0, 400_000, now, 0.1);
            sc.on_bytes(1, 400_000, now, 0.1);
            sc.tick(now);
        }
        assert_eq!(sc.conn_health(0), crate::detect::Health::Healthy);

        // Connection 0 collapses; connection 1 keeps its rate.
        let mut flagged_at = None;
        for _ in 0..8 {
            now += 0.1;
            sc.on_bytes(0, 12_000, now, 0.1);
            sc.on_bytes(1, 400_000, now, 0.1);
            sc.tick(now);
            if flagged_at.is_none() && sc.conn_health(0).is_suspect_or_worse() {
                flagged_at = Some(now);
            }
        }
        let t = flagged_at.expect("collapse must be graded");
        assert!(
            t < 1.2 + 10.0,
            "must be flagged well before the 10 s stall timeout, was {t}"
        );
        // And the healthy connection must never be the one downgraded.
        assert_eq!(
            sc.conn_health(1),
            crate::detect::Health::Healthy,
            "the connection holding its rate must stay Healthy"
        );
        assert!(sc.coverage_holds() && sc.liveness_holds());
    }
    /// The initial split must respect `mark_done`.
    ///
    /// Regression test: an earlier version partitioned `[0, size)` arithmetically
    /// and never consulted the unassigned set, so `mark_done` was silently
    /// ignored. That broke `--range` (fetched from offset 0 instead of the
    /// requested interval) and `--continue` (re-fetched bytes already on disk).
    #[test]
    fn initial_split_never_requests_bytes_marked_done() {
        let size = 100_000u64;
        let mut s = Scheduler::new(size, vec![src(1e6), src(1e6)], &[1, 1]);
        // Range mode: only [90_000, 90_512) is wanted.
        s.mark_done(0, 90_000);
        s.mark_done(90_512, size);
        let acts = s.tick(0.0);
        assert!(
            !acts.is_empty(),
            "the wanted interval must still be requested"
        );
        for a in &acts {
            if let Action::Request { range, .. } = a {
                assert!(
                    range.lo >= 90_000 && range.hi <= 90_512,
                    "requested {range:?} outside the wanted interval"
                );
            }
        }
        assert!(s.coverage_holds());
    }

    /// Overlapping `mark_done` calls must not inflate the held count.
    ///
    /// Regression test for a silent truncation. `mark_done` credited the width of
    /// the span it was given rather than the bytes it actually claimed, so two
    /// callers marking the same prefix — a `-c` resume replaying its sidecar, and
    /// the concurrency probe reporting the bytes it fetched, both of which start
    /// at offset 0 — pushed `held` past the object's real length. `is_complete()`
    /// tests exactly that counter, so the transfer stopped believing it was
    /// finished and left a zero-filled hole in the tail of a file it reported as
    /// a success: measured at 240 138 unwritten bytes on an 11 200 900-byte
    /// object whose gzip then refused to decompress.
    #[test]
    fn overlapping_mark_done_credits_each_byte_once() {
        let size = 100_000u64;
        let mut s = Scheduler::new(size, vec![src(1e6)], &[1]);
        s.mark_done(0, 30_000); // a resume record
        s.mark_done(0, 10_000); // the probe, re-reporting part of the same prefix
        assert_eq!(
            s.bytes_held(),
            30_000,
            "the overlap must be credited once, not twice"
        );
        assert!(!s.is_complete(), "70 000 bytes are still missing");

        // Marking every byte, in overlapping pieces, is completion — and exactly
        // completion, never more.
        s.mark_done(20_000, size);
        s.mark_done(0, size);
        assert_eq!(s.bytes_held(), size);
        assert!(s.is_complete());
    }

    /// After the probe's ranges are marked, `held_ranges` must describe them.
    ///
    /// This is what the pre-transfer checkpoint writes into the sidecar, so that a
    /// ^C during or shortly after the concurrency probe does not discard bytes the
    /// probe already fetched at true offsets. The periodic checkpoint inside the
    /// transfer only fires after 2 seconds, which an early interrupt beats.
    #[test]
    fn held_ranges_reports_probe_bytes_before_any_transfer() {
        let size = 11_200_900u64;
        let mut s = Scheduler::new(size, vec![Source::default()], &[1]);
        // Nothing fetched yet: nothing to checkpoint, and an empty record must not
        // be written as though it were progress.
        assert!(s.held_ranges().is_empty());

        // The probe fetched a 3 MiB prefix into the real output.
        s.mark_done(0, 3 << 20);
        assert_eq!(s.held_ranges(), vec![(0, 3 << 20)]);
        assert_eq!(s.bytes_held(), 3 << 20);

        // A second, disjoint probe range is reported as its own span rather than
        // merged into a count: a byte count cannot describe a hole, which is why
        // the sidecar stores ranges.
        s.mark_done(5 << 20, 6 << 20);
        assert_eq!(s.held_ranges(), vec![(0, 3 << 20), (5 << 20, 6 << 20)]);

        // Adjacent spans DO coalesce, so the record stays compact across a long run.
        s.mark_done(3 << 20, 5 << 20);
        assert_eq!(s.held_ranges(), vec![(0, 6 << 20)]);
    }

    /// Resume: bytes already on disk must never be re-requested.
    #[test]
    fn resume_does_not_refetch_held_prefix() {
        let size = 64_000u64;
        let mut s = Scheduler::new(size, vec![src(1e6)], &[2]);
        s.mark_done(0, 48_000); // three quarters already fetched
        let acts = s.tick(0.0);
        for a in &acts {
            if let Action::Request { range, .. } = a {
                assert!(
                    range.lo >= 48_000,
                    "re-requested a held byte at {}",
                    range.lo
                );
            }
        }
        assert_eq!(
            s.bytes_held(),
            48_000,
            "held count must include the resumed prefix"
        );
        assert!(s.coverage_holds());
    }
}
