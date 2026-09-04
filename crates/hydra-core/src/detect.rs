//! Fast collapse detection for connection goodput.
//!
//! # Why this exists
//!
//! The scheduler cannot repair a divergence it has not observed. Measurement of
//! the earlier EWMA-only estimator showed a *fixed* detection cost of roughly
//! 0.25–0.9 s when a source's rate collapsed by ~97%: the makespan ratio against
//! the fluid oracle was 2.44× on a 12 MB object and only fell to 1.02× at 192 MB,
//! because the excess is a constant that amortises rather than a per-byte
//! inefficiency. An EWMA is a *smoother*; asking it to detect a step change is
//! asking the wrong question of it, and the lag is structural: after a collapse,
//! the very samples the estimator needs arrive at the collapsed rate.
//!
//! # What this does instead
//!
//! Two independent mechanisms, because they fail in different directions.
//!
//! * **Dual-window ratio.** A short window (recent arrivals) against a long
//!   window (the connection's established rate). Responds within the short
//!   window, but is noisy.
//! * **Two-sided CUSUM.** Accumulates normalised deviations from the established
//!   rate and fires when the running sum passes a threshold `h`. Slower than the
//!   ratio test on a hard collapse but far more resistant to variance, so it
//!   catches slow degradation the ratio test smooths over.
//!
//! A connection is graded on the strongest evidence available, and the grade —
//! not a raw rate — is what the scheduler acts on. Repair may fire on `Suspect`
//! long before the stall timeout would expire.
//!
//! # The false-positive cost is real and asymmetric
//!
//! A missed collapse costs the transfer up to a stall timeout. A *spurious*
//! detection costs one `delta` per unnecessary repair, and on a high-RTT path
//! `delta` is hundreds of milliseconds. The thresholds here are therefore set to
//! tolerate ordinary jitter — the `stable_noisy_connection_is_not_flagged` test
//! pins that behaviour, and it is as load-bearing as the detection tests.

/// How healthy a connection looks, on the evidence so far.
/// `short / long` above which a rate is still climbing; see
/// [`CollapseDetector::rising`].
const RISING_RATIO: f64 = 1.15;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub enum Health {
    /// Delivering at or near its established rate.
    #[default]
    Healthy,
    /// Evidence of a drop, not yet conclusive. Repair may pre-empt on this.
    Suspect,
    /// Confirmed sustained collapse. Prefer moving work away.
    Degraded,
    /// Nothing arriving for longer than the stall timeout.
    Stalled,
    /// Failed and not worth retrying within this transfer.
    Dead,
}

impl Health {
    /// Should the scheduler treat this connection as a repair victim on sight?
    pub fn is_suspect_or_worse(self) -> bool {
        self >= Health::Suspect
    }
}

/// Per-connection collapse detector.
#[derive(Clone, Debug)]
pub struct CollapseDetector {
    /// Established rate estimate (bytes/s), slow EWMA — the reference level.
    long: f64,
    /// Recent rate estimate (bytes/s), fast EWMA — the test level.
    short: f64,
    /// Two-sided CUSUM accumulator for downward shifts.
    cusum_down: f64,
    /// Samples observed; the detector abstains until it has a reference.
    n: u32,
    /// Fraction of the long rate below which the short rate is suspicious.
    ratio_suspect: f64,
    /// Fraction below which a collapse is confirmed.
    ratio_degraded: f64,
    /// CUSUM decision threshold, in units of the long rate.
    cusum_h: f64,
    /// CUSUM slack: shifts smaller than this fraction are ignored as noise.
    cusum_k: f64,
    health: Health,
}

impl Default for CollapseDetector {
    fn default() -> Self {
        Self::new()
    }
}

/// Samples required before the detector will grade anything. Below this it has
/// no reference level, and grading noise as collapse is worse than not grading.
pub const WARMUP: u32 = 4;

impl CollapseDetector {
    pub fn new() -> Self {
        Self {
            long: 0.0,
            short: 0.0,
            cusum_down: 0.0,
            n: 0,
            ratio_suspect: 0.55,
            ratio_degraded: 0.30,
            cusum_h: 2.0,
            cusum_k: 0.25,
            health: Health::Healthy,
        }
    }

    /// Record an observed instantaneous rate (bytes/s) for this connection.
    pub fn observe_rate(&mut self, rate: f64) {
        let r = rate.max(0.0);
        self.n = self.n.saturating_add(1);
        if self.n == 1 {
            self.long = r;
            self.short = r;
            return;
        }
        self.short = 0.45 * r + 0.55 * self.short;
        // The reference level is FROZEN while evidence is accumulating. This is
        // textbook CUSUM and getting it wrong is subtle: an adaptive reference
        // chases the drop, the normalised deviation shrinks toward the slack `k`,
        // and evidence never accumulates. Measured on a sustained 45% drop, an
        // adaptive reference plateaus at 0.93 against a threshold of 2.0 — it
        // never fires, because the decline has quietly become "normal".
        if self.cusum_down <= 0.0 {
            self.long = 0.08 * r + 0.92 * self.long;
        }

        if self.n <= WARMUP || self.long <= 0.0 {
            return;
        }

        // CUSUM on the normalised downward deviation, with slack k.
        let dev = (self.long - r) / self.long - self.cusum_k;
        self.cusum_down = (self.cusum_down + dev).max(0.0);

        let ratio = self.short / self.long;
        self.health = if ratio <= self.ratio_degraded || self.cusum_down >= 2.0 * self.cusum_h {
            Health::Degraded
        } else if ratio <= self.ratio_suspect || self.cusum_down >= self.cusum_h {
            Health::Suspect
        } else {
            // Deliberately does NOT zero the accumulator. An earlier version
            // reset it here, which is a subtle self-defeat: this branch is taken
            // on every sample where the evidence has not YET crossed the
            // threshold, i.e. throughout accumulation, so the sum was wiped each
            // time and a slow decline could never be detected at all.
            //
            // Recovery needs no special case. When the rate returns to the
            // reference, `dev` goes negative (-k per sample) and the `max(0.0)`
            // above walks the accumulator back down to zero on its own.
            Health::Healthy
        };
    }

    /// Escalate on wall-clock silence, which no rate sample can express: a
    /// connection delivering nothing produces no observations at all.
    pub fn observe_silence(&mut self, since_progress_s: f64, stall_timeout_s: f64) {
        if since_progress_s >= stall_timeout_s {
            self.health = Health::Stalled;
        } else if since_progress_s >= 0.5 * stall_timeout_s && self.health < Health::Suspect {
            // Halfway to the stall timeout with nothing arriving is already
            // evidence, and waiting for the full timeout is what cost the
            // earlier implementation its detection lag.
            self.health = Health::Suspect;
        }
    }

    pub fn mark_dead(&mut self) {
        self.health = Health::Dead;
    }

    pub fn health(&self) -> Health {
        self.health
    }

    /// Established rate, for scheduling decisions that need a number.
    pub fn rate(&self) -> f64 {
        // Once a collapse is evident the SHORT window is the honest estimate:
        // projecting a laggard's finish time from its pre-collapse rate is what
        // makes a scheduler fail to repair.
        if self.health.is_suspect_or_worse() {
            self.short
        } else {
            self.long
        }
    }

    pub fn samples(&self) -> u32 {
        self.n
    }

    /// Is the rate still climbing?
    ///
    /// The short average follows a rising rate within a couple of samples and
    /// the long one trails it, so while a connection is still in TCP slow start
    /// the two disagree by a wide margin. That disagreement is the honest
    /// answer to "is this connection slow?": not yet known. A repair that reads
    /// a climbing rate as a settled one moves work off a connection that was a
    /// second away from matching its peers — measured on a 100 ms path, one
    /// flow at 16 MB/s against its twin at 49 had 256 MB taken from it, and
    /// both were at 90 MB/s before the stolen bytes had been re-requested. The
    /// margin is wide enough that steady-state jitter does not trip it and
    /// narrow enough that a connection settled at half its peers' rate is
    /// reported as settled, since its two averages agree.
    pub fn rising(&self) -> bool {
        self.n >= 2 && self.short > self.long * RISING_RATIO
    }

    /// Clear detection state after work has been moved away, so the next
    /// episode is judged on fresh evidence.
    pub fn reset_after_repair(&mut self) {
        self.cusum_down = 0.0;
        if self.health == Health::Suspect {
            self.health = Health::Healthy;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The measured failure case: a connection collapsing to 3% of its rate.
    #[test]
    fn hard_collapse_is_detected_within_a_few_arrivals() {
        let mut d = CollapseDetector::new();
        for _ in 0..10 {
            d.observe_rate(4.0e6);
        }
        assert_eq!(d.health(), Health::Healthy);

        let mut arrivals_to_suspect = None;
        for i in 1..=10 {
            d.observe_rate(0.12e6); // 3% of 4 MB/s
            if arrivals_to_suspect.is_none() && d.health().is_suspect_or_worse() {
                arrivals_to_suspect = Some(i);
            }
        }
        let k = arrivals_to_suspect.expect("a 97% collapse must be detected");
        assert!(
            k <= 3,
            "collapse must be flagged within 3 arrivals, took {k}"
        );
        assert_eq!(
            d.health(),
            Health::Degraded,
            "sustained collapse must confirm"
        );
    }

    /// The guard that matters just as much: ordinary jitter is not a collapse.
    /// Each spurious detection costs a full `delta`.
    #[test]
    fn stable_noisy_connection_is_not_flagged() {
        let mut d = CollapseDetector::new();
        // +/-30% multiplicative jitter around a stable mean, deterministic.
        let jitter = [
            1.0, 0.72, 1.28, 0.85, 1.15, 0.78, 1.22, 0.93, 1.07, 0.80, 1.20, 1.0,
        ];
        for rep in 0..6 {
            for j in jitter {
                d.observe_rate(4.0e6 * j * if rep % 2 == 0 { 1.0 } else { 0.98 });
            }
        }
        assert_eq!(
            d.health(),
            Health::Healthy,
            "30% jitter must not be graded as collapse (false positives cost a delta each)"
        );
    }

    #[test]
    fn slow_degradation_is_caught_by_cusum() {
        let mut d = CollapseDetector::new();
        for _ in 0..12 {
            d.observe_rate(4.0e6);
        }
        // A 45% drop. The short/long ratio settles near 0.60 -- ABOVE the
        // suspect floor -- so the ratio test alone never fires and CUSUM is the
        // only mechanism that can accumulate the evidence. Measured: the
        // accumulator crosses h at the 12th post-drop sample.
        let mut k = None;
        for i in 1..=20 {
            d.observe_rate(2.2e6);
            if k.is_none() && d.health().is_suspect_or_worse() {
                k = Some(i);
            }
        }
        let k = k.expect("a sustained 45% drop must eventually be flagged by CUSUM");
        assert!(
            (8..=16).contains(&k),
            "slow degradation should be caught in ~12 samples, took {k}"
        );
    }

    #[test]
    fn recovery_clears_suspicion() {
        let mut d = CollapseDetector::new();
        for _ in 0..10 {
            d.observe_rate(4.0e6);
        }
        for _ in 0..3 {
            d.observe_rate(0.2e6);
        }
        assert!(d.health().is_suspect_or_worse());
        for _ in 0..25 {
            d.observe_rate(4.0e6);
        }
        assert_eq!(
            d.health(),
            Health::Healthy,
            "a recovered connection must be usable again"
        );
    }

    #[test]
    fn silence_escalates_before_the_stall_timeout() {
        let mut d = CollapseDetector::new();
        for _ in 0..8 {
            d.observe_rate(4.0e6);
        }
        d.observe_silence(2.0, 8.0);
        assert_eq!(
            d.health(),
            Health::Healthy,
            "a quarter of the timeout is not evidence"
        );
        d.observe_silence(4.5, 8.0);
        assert_eq!(
            d.health(),
            Health::Suspect,
            "past half the stall timeout must pre-empt, not wait for the full timeout"
        );
        d.observe_silence(8.1, 8.0);
        assert_eq!(d.health(), Health::Stalled);
    }

    #[test]
    fn detector_abstains_during_warmup() {
        let mut d = CollapseDetector::new();
        d.observe_rate(4.0e6);
        d.observe_rate(0.01e6);
        assert_eq!(
            d.health(),
            Health::Healthy,
            "with no reference level established, grading is guesswork"
        );
    }

    #[test]
    fn collapsed_rate_estimate_is_the_short_window() {
        let mut d = CollapseDetector::new();
        for _ in 0..12 {
            d.observe_rate(4.0e6);
        }
        let before = d.rate();
        for _ in 0..4 {
            d.observe_rate(0.1e6);
        }
        assert!(before > 3.0e6);
        assert!(
            d.rate() < 1.0e6,
            "once collapse is evident the estimate must follow the SHORT window, got {}",
            d.rate()
        );
    }
}
