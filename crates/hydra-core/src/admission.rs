//! Online concurrency admission: dynamically probe and scale connection counts
//! based on measured marginal goodput.
//!
//! When per-source rate limits and per-connection capacities are unknown, opening
//! extra connections on an already saturated path adds request overhead without
//! improving throughput.
//!
//! This module implements incremental greedy admission: it probes connections
//! one at a time, checks whether throughput increases by more than a minimum gain
//! threshold, and settles at the optimal connection count upon diminishing returns.

/// Decision returned by the controller after each probe interval.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Admit {
    /// Open one more connection to this source and keep probing.
    Add,
    /// Saturated: the last admission did not pay for itself. Settle here.
    Stop,
}

/// Per-source incremental admission controller.
///
/// Feed it the aggregate goodput observed at each concurrency level. It compares
/// the marginal gain against `min_gain_frac` of the goodput achieved by the first
/// connection — a scale-free threshold, so it behaves identically on a 1 MB/s
/// and a 1 GB/s path.
#[derive(Clone, Debug)]
pub struct Admission {
    /// Goodput observed at each concurrency level, index 0 = one connection.
    samples: Vec<f64>,
    /// Marginal gain required to justify one more connection, as a fraction of
    /// the single-connection goodput.
    min_gain_frac: f64,
    /// Hard ceiling regardless of measurement (politeness, not physics).
    max_conns: usize,
    settled: Option<usize>,
}

impl Admission {
    pub fn new(min_gain_frac: f64, max_conns: usize) -> Self {
        Self {
            samples: Vec::new(),
            min_gain_frac,
            max_conns: max_conns.max(1),
            settled: None,
        }
    }

    /// Record the aggregate goodput (bytes/s) observed with `self.level() + 1`
    /// connections, and decide whether to admit another.
    pub fn observe(&mut self, goodput: f64) -> Admit {
        self.samples.push(goodput.max(0.0));
        let n = self.samples.len();
        if n >= self.max_conns {
            self.settled = Some(n);
            return Admit::Stop;
        }
        if n == 1 {
            return Admit::Add;
        }
        let base = self.samples[0].max(1.0);
        let gain = self.samples[n - 1] - self.samples[n - 2];
        if gain < self.min_gain_frac * base {
            // The previous level was as good; settle there, not here.
            // Additional connections contribute negligible gain and add request overhead.
            self.settled = Some(n - 1);
            Admit::Stop
        } else {
            Admit::Add
        }
    }

    /// Connections currently probed.
    pub fn level(&self) -> usize {
        self.samples.len()
    }

    /// The settled allocation, once probing has stopped.
    pub fn settled(&self) -> Option<usize> {
        self.settled
    }

    /// Best goodput seen, for reporting.
    pub fn best_goodput(&self) -> f64 {
        self.samples.iter().cloned().fold(0.0, f64::max)
    }
}

/// Online estimate of the per-request setup cost `delta`.
///
/// A configured constant is not good enough: the same code saw `delta = 5 ms`
/// against a loopback-equivalent origin and `420 ms` against a real proxied
/// origin, and `delta` sets the repair deadband `theta`. Underestimating it by
/// 2.8x caused measurable over-repair — each unnecessary repair costs a full
/// `delta`, so the error compounds.
#[derive(Clone, Copy, Debug)]
pub struct DeltaEstimator {
    ewma: f64,
    alpha: f64,
    n: u32,
}

impl DeltaEstimator {
    pub fn new(prior_s: f64) -> Self {
        Self {
            ewma: prior_s.max(1e-4),
            alpha: 0.3,
            n: 0,
        }
    }

    /// Record an observed request-to-first-byte latency.
    pub fn observe(&mut self, ttfb_s: f64) {
        let x = ttfb_s.clamp(1e-4, 30.0);
        if self.n == 0 {
            self.ewma = x;
        } else {
            self.ewma = (1.0 - self.alpha) * self.ewma + self.alpha * x;
        }
        self.n = self.n.saturating_add(1);
    }

    pub fn get(&self) -> f64 {
        self.ewma
    }

    pub fn samples(&self) -> u32 {
        self.n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saturated_path_settles_at_one() {
        // A path already saturated by one connection: adding more yields nothing.
        let mut a = Admission::new(0.15, 8);
        assert_eq!(a.observe(1.0e6), Admit::Add);
        assert_eq!(a.observe(1.02e6), Admit::Stop, "2% gain must be refused");
        assert_eq!(
            a.settled(),
            Some(1),
            "must settle at ONE, not at the probed 2"
        );
    }

    #[test]
    fn scalable_path_admits_until_knee() {
        // rho = 4 * gamma: goodput rises linearly to 4 connections then flattens.
        let mut a = Admission::new(0.15, 12);
        let curve = [1.0e6, 2.0e6, 3.0e6, 4.0e6, 4.0e6, 4.0e6];
        let mut last = Admit::Add;
        for g in curve {
            last = a.observe(g);
            if last == Admit::Stop {
                break;
            }
        }
        assert_eq!(last, Admit::Stop);
        assert_eq!(a.settled(), Some(4), "must find the knee at rho/gamma = 4");
    }

    #[test]
    fn respects_politeness_ceiling() {
        let mut a = Admission::new(0.01, 3);
        for g in [1.0e6, 2.0e6, 3.0e6] {
            a.observe(g);
        }
        assert_eq!(
            a.settled(),
            Some(3),
            "ceiling binds even when gains continue"
        );
    }

    #[test]
    fn delta_estimator_tracks_a_step_change() {
        let mut d = DeltaEstimator::new(0.15);
        for _ in 0..12 {
            d.observe(0.42);
        }
        assert!(
            (d.get() - 0.42).abs() < 0.02,
            "estimator must converge on the observed cost, got {}",
            d.get()
        );
        assert_eq!(d.samples(), 12);
    }

    #[test]
    fn delta_estimator_first_sample_replaces_prior() {
        let mut d = DeltaEstimator::new(0.005);
        d.observe(0.40);
        assert!(
            d.get() > 0.3,
            "a wildly wrong prior must not survive one sample"
        );
    }
}
