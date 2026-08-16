//! Byte-range set with a maintained coverage invariant.
//!
//! Ranges are half-open `[lo, hi)` over byte positions. The set is kept sorted
//! and coalesced at all times, which makes `total()` exact and makes the
//! coverage audit in `Scheduler` a cheap sum rather than a merge.

use core::cmp::Ordering;

/// A half-open byte range.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Range {
    pub lo: u64,
    pub hi: u64,
}

impl Range {
    #[inline]
    pub fn new(lo: u64, hi: u64) -> Self {
        debug_assert!(hi >= lo, "range {lo}..{hi} inverted");
        Range { lo, hi }
    }

    #[inline]
    pub fn len(&self) -> u64 {
        self.hi.saturating_sub(self.lo)
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.hi <= self.lo
    }
}

/// A sorted, coalesced set of disjoint byte ranges.
#[derive(Clone, Default, Debug)]
pub struct IntervalSet {
    iv: Vec<Range>,
}

impl IntervalSet {
    pub fn new() -> Self {
        IntervalSet { iv: Vec::new() }
    }

    /// The full range `[0, size)`.
    pub fn full(size: u64) -> Self {
        if size == 0 {
            return Self::new();
        }
        IntervalSet {
            iv: vec![Range::new(0, size)],
        }
    }

    pub fn is_empty(&self) -> bool {
        self.iv.is_empty()
    }

    pub fn len(&self) -> usize {
        self.iv.len()
    }

    pub fn ranges(&self) -> &[Range] {
        &self.iv
    }

    /// Total bytes covered.
    pub fn total(&self) -> u64 {
        self.iv.iter().map(|r| r.len()).sum()
    }

    /// Remove and return up to `n` bytes from the lowest range.
    ///
    /// Front-to-back allocation keeps the set small (usually one range) and
    /// makes sequential-write patterns friendly to the OS page cache.
    pub fn take_front(&mut self, n: u64) -> Option<Range> {
        if n == 0 {
            return None;
        }
        let first = *self.iv.first()?;
        if first.len() <= n {
            self.iv.remove(0);
            Some(first)
        } else {
            let out = Range::new(first.lo, first.lo + n);
            self.iv[0] = Range::new(first.lo + n, first.hi);
            Some(out)
        }
    }

    /// Insert a range, coalescing with neighbours. Overlapping inserts are
    /// merged rather than duplicated, so re-inserting a reclaimed range that
    /// partially overlaps an existing gap is safe.
    pub fn insert(&mut self, r: Range) {
        if r.is_empty() {
            return;
        }
        let pos = self
            .iv
            .binary_search_by(|p| {
                if p.hi < r.lo {
                    Ordering::Less
                } else if p.lo > r.hi {
                    Ordering::Greater
                } else {
                    Ordering::Equal
                }
            })
            .unwrap_or_else(|e| e);

        // Merge with any ranges touching or overlapping [r.lo, r.hi].
        let mut lo = r.lo;
        let mut hi = r.hi;
        let mut end = pos;
        while end < self.iv.len() && self.iv[end].lo <= hi {
            lo = lo.min(self.iv[end].lo);
            hi = hi.max(self.iv[end].hi);
            end += 1;
        }
        let mut start = pos;
        while start > 0 && self.iv[start - 1].hi >= lo {
            start -= 1;
            lo = lo.min(self.iv[start].lo);
            hi = hi.max(self.iv[start].hi);
        }
        self.iv.splice(start..end, [Range::new(lo, hi)]);
    }

    /// Remove `[lo, hi)` from the set, splitting ranges as needed.
    pub fn remove(&mut self, lo: u64, hi: u64) {
        if hi <= lo {
            return;
        }
        let mut out: Vec<Range> = Vec::with_capacity(self.iv.len() + 1);
        for r in self.iv.iter().copied() {
            if r.hi <= lo || r.lo >= hi {
                out.push(r);
                continue;
            }
            if r.lo < lo {
                out.push(Range::new(r.lo, lo));
            }
            if r.hi > hi {
                out.push(Range::new(hi, r.hi));
            }
        }
        self.iv = out;
    }

    /// True when every range is non-empty, sorted, and strictly disjoint from
    /// its neighbours (i.e. the coalescing invariant holds).
    pub fn invariant_holds(&self) -> bool {
        for w in self.iv.windows(2) {
            if w[0].hi >= w[1].lo || w[0].is_empty() {
                return false;
            }
        }
        self.iv.last().map(|r| !r.is_empty()).unwrap_or(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_front_splits_and_preserves_total() {
        let mut s = IntervalSet::full(1000);
        let a = s.take_front(300).unwrap();
        assert_eq!((a.lo, a.hi), (0, 300));
        assert_eq!(s.total(), 700);
        let b = s.take_front(900).unwrap();
        assert_eq!((b.lo, b.hi), (300, 1000));
        assert!(s.is_empty());
        assert!(s.take_front(10).is_none());
    }

    #[test]
    fn insert_coalesces_adjacent_and_overlapping() {
        let mut s = IntervalSet::new();
        s.insert(Range::new(10, 20));
        s.insert(Range::new(20, 30)); // adjacent
        assert_eq!(s.len(), 1);
        assert_eq!(s.total(), 20);
        s.insert(Range::new(25, 40)); // overlapping
        assert_eq!(s.len(), 1);
        assert_eq!(s.ranges()[0], Range::new(10, 40));
        s.insert(Range::new(100, 110)); // disjoint
        assert_eq!(s.len(), 2);
        assert!(s.invariant_holds());
    }

    #[test]
    fn insert_bridges_two_ranges() {
        let mut s = IntervalSet::new();
        s.insert(Range::new(0, 10));
        s.insert(Range::new(20, 30));
        s.insert(Range::new(10, 20));
        assert_eq!(s.len(), 1);
        assert_eq!(s.ranges()[0], Range::new(0, 30));
    }

    #[test]
    fn remove_splits() {
        let mut s = IntervalSet::full(100);
        s.remove(30, 40);
        assert_eq!(s.len(), 2);
        assert_eq!(s.total(), 90);
        assert!(s.invariant_holds());
    }

    #[test]
    fn empty_and_degenerate_inserts_are_noops() {
        let mut s = IntervalSet::new();
        s.insert(Range::new(5, 5));
        assert!(s.is_empty());
        assert_eq!(IntervalSet::full(0).total(), 0);
    }
}
