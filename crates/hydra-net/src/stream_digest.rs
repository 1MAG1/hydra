//! Digest and classify a byte stream that arrives out of order, without storing it.
//!
//! This exists for `--no-save`, and the difficulty is not obvious until you try
//! it. Discarding bytes is trivial; discarding them while still reporting the
//! object's SHA-256 is not, because **SHA-256 is order-dependent and positioned
//! writes land ranges out of order**. Feeding the hasher in arrival order would
//! produce a digest that is stable, plausible, and wrong — the worst failure
//! mode this project keeps finding, and the reason the earlier implementation
//! wrote a real file and hashed it afterwards.
//!
//! The fix is to hash the *contiguous prefix* and hold everything ahead of it in
//! a bounded reorder buffer:
//!
//! * a fragment landing exactly at the write frontier is hashed immediately, and
//!   then any buffered fragments that have become contiguous are drained after it;
//! * a fragment landing beyond the frontier is buffered;
//! * if buffering would exceed the cap, the digest is **abandoned and reported as
//!   unavailable** rather than completed incorrectly.
//!
//! Abandoning is the load-bearing decision. The alternative — an unbounded buffer
//! — would silently reintroduce the whole-object memory cost that positioned
//! writes exist to avoid, turning a 2.9 MB resident footprint into the object
//! size. A `None` digest with a stated reason is honest; a correct digest bought
//! by quietly buffering a gigabyte is not.
//!
//! # What this can and cannot digest
//!
//! Only an effectively SEQUENTIAL transfer completes a streaming digest, which
//! in practice means one connection. That is not a tuning problem and raising
//! the cap does not fix it: the scheduler opens a parallel transfer by splitting
//! the object into one contiguous span per connection, so with `n` connections
//! the second span starts at `size/n` and every byte of it is ahead of a
//! frontier still sitting in the first span. The buffer would have to hold
//! `(n-1)/n` of the whole object — 256 MiB of a 512 MiB object at just two
//! connections, against a cap of 8 MiB.
//!
//! So `-x 1` digests inline, and anything above it abandons the digest on the
//! first span boundary. Callers that need a digest for a parallel transfer must
//! hash the finished file instead; `--no-save` has no file, which is why it
//! reports the digest as unavailable rather than wrong.
//!
//! An earlier version of this comment claimed the opposite — that ordinary
//! transfers stay well under the cap because preemption only moves the end of a
//! range. Preemption does only move the end, but the INITIAL split is what
//! defeats the frontier, and measured at `-x 2` through `-x 8` the digest was
//! abandoned every time.

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Bytes retained for format classification. `detect_format` reads at most this
/// much, and it is a prefix, so it costs nothing to keep.
const HEAD_MAX: usize = 8192;

/// Default reorder budget: enough for many in-flight ranges, small enough to
/// preserve the flat-memory property the client measures.
pub const DEFAULT_REORDER_CAP: usize = 8 * 1024 * 1024;

pub struct StreamDigest {
    hasher: Sha256,
    /// Offset the hasher has consumed up to; the contiguous write frontier.
    frontier: u64,
    /// Fragments ahead of the frontier, keyed by offset.
    pending: BTreeMap<u64, Vec<u8>>,
    pending_bytes: usize,
    cap: usize,
    /// Set when the reorder budget was exceeded; the digest is then unavailable.
    abandoned: bool,
    /// First `HEAD_MAX` bytes, for format detection.
    head: Vec<u8>,
    head_seen: u64,
}

impl StreamDigest {
    pub fn new(cap: usize) -> Self {
        StreamDigest {
            hasher: Sha256::new(),
            frontier: 0,
            pending: BTreeMap::new(),
            pending_bytes: 0,
            cap,
            abandoned: false,
            head: Vec::new(),
            head_seen: 0,
        }
    }

    /// Record `buf` as the object's bytes at absolute offset `off`.
    pub fn write(&mut self, off: u64, buf: &[u8]) {
        self.capture_head(off, buf);
        if self.abandoned {
            return;
        }
        if off < self.frontier {
            // Behind the frontier: already hashed. The scheduler's coverage
            // invariant makes ranges disjoint, so this means a retry re-sent
            // bytes we have consumed. Ignoring is correct; re-hashing would
            // corrupt the digest.
            return;
        }
        if off == self.frontier {
            self.hasher.update(buf);
            self.frontier += buf.len() as u64;
            self.drain_pending();
            return;
        }
        // Ahead of the frontier: buffer it, or give up if that costs too much.
        if self.pending_bytes + buf.len() > self.cap {
            self.abandon();
            return;
        }
        self.pending_bytes += buf.len();
        self.pending.insert(off, buf.to_vec());
    }

    fn drain_pending(&mut self) {
        while let Some((&off, _)) = self.pending.iter().next() {
            if off != self.frontier {
                break;
            }
            let frag = self.pending.remove(&off).expect("key just observed");
            self.pending_bytes -= frag.len();
            self.hasher.update(&frag);
            self.frontier += frag.len() as u64;
        }
    }

    fn abandon(&mut self) {
        self.abandoned = true;
        self.pending.clear();
        self.pending_bytes = 0;
    }

    fn capture_head(&mut self, off: u64, buf: &[u8]) {
        if off >= HEAD_MAX as u64 {
            return;
        }
        let start = off as usize;
        let end = (start + buf.len()).min(HEAD_MAX);
        if end <= start {
            return;
        }
        if self.head.len() < end {
            self.head.resize(end, 0);
        }
        let take = end - start;
        self.head[start..end].copy_from_slice(&buf[..take]);
        self.head_seen += take as u64;
    }

    /// The object's SHA-256, or `None` when it cannot be stated.
    ///
    /// `None` means one of: the reorder budget was exceeded, or the stream did
    /// not cover `[0, size)` contiguously. Both are reasons the digest is
    /// unknown, never a reason to report a partial one.
    pub fn finish(self, size: u64) -> Option<String> {
        if self.abandoned || self.frontier != size {
            return None;
        }
        Some(crate::digest::to_lower_hex(&self.hasher.finalize()))
    }

    /// Leading bytes for format classification.
    pub fn head(&self) -> &[u8] {
        &self.head
    }

    /// True when the digest has been given up on.
    pub fn is_abandoned(&self) -> bool {
        self.abandoned
    }

    /// Why the digest is unavailable, for a user-facing note.
    pub fn unavailable_reason(&self, size: u64) -> Option<String> {
        if self.abandoned {
            return Some(format!(
                "digest unavailable: ranges arrived too far out of order to hash without \
                 buffering more than {} MiB, and --no-save keeps no file to hash afterwards",
                self.cap / (1024 * 1024)
            ));
        }
        if self.frontier != size {
            return Some(format!(
                "digest unavailable: only {} of {size} bytes were seen contiguously",
                self.frontier
            ));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference(data: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(data);
        crate::digest::to_lower_hex(&h.finalize())
    }

    #[test]
    fn in_order_matches_a_plain_hash() {
        let data: Vec<u8> = (0..10_000u32).map(|i| (i % 251) as u8).collect();
        let mut sd = StreamDigest::new(DEFAULT_REORDER_CAP);
        for chunk in data.chunks(1000) {
            let off = (chunk.as_ptr() as usize - data.as_ptr() as usize) as u64;
            sd.write(off, chunk);
        }
        assert_eq!(sd.finish(data.len() as u64), Some(reference(&data)));
    }

    /// The case that makes this module necessary: ranges arriving in the wrong
    /// order must still produce the object's true digest.
    #[test]
    fn out_of_order_arrival_still_yields_the_true_digest() {
        let data: Vec<u8> = (0..8192u32).map(|i| (i % 251) as u8).collect();
        let want = reference(&data);
        // Deliberately adversarial order: last, middle, first.
        let order = [(4096u64, 4096usize), (2048, 2048), (0, 2048)];
        let mut sd = StreamDigest::new(DEFAULT_REORDER_CAP);
        for (off, len) in order {
            sd.write(off, &data[off as usize..off as usize + len]);
        }
        assert_eq!(sd.finish(data.len() as u64), Some(want));
    }

    /// Hashing in ARRIVAL order would pass the previous test's length check and
    /// return a wrong answer. This pins that the implementation is not doing that.
    #[test]
    fn arrival_order_hashing_would_differ_which_is_the_whole_point() {
        let data: Vec<u8> = (0..4096u32).map(|i| (i % 251) as u8).collect();
        let mut naive = Sha256::new();
        naive.update(&data[2048..]);
        naive.update(&data[..2048]);
        let arrival_order = crate::digest::to_lower_hex(&naive.finalize());
        assert_ne!(
            arrival_order,
            reference(&data),
            "if these were equal the ordering problem would not exist"
        );

        let mut sd = StreamDigest::new(DEFAULT_REORDER_CAP);
        sd.write(2048, &data[2048..]);
        sd.write(0, &data[..2048]);
        assert_eq!(sd.finish(4096), Some(reference(&data)));
    }

    #[test]
    fn exceeding_the_reorder_budget_reports_unavailable_not_wrong() {
        let data: Vec<u8> = vec![7u8; 64 * 1024];
        // Cap smaller than the fragment held ahead of the frontier.
        let mut sd = StreamDigest::new(1024);
        sd.write(32 * 1024, &data[32 * 1024..]); // ahead of frontier, too big
        assert!(sd.is_abandoned());
        sd.write(0, &data[..32 * 1024]);
        assert!(sd.unavailable_reason(data.len() as u64).is_some());
        assert_eq!(sd.finish(data.len() as u64), None);
    }

    #[test]
    fn an_incomplete_stream_has_no_digest() {
        let data = vec![1u8; 4096];
        let mut sd = StreamDigest::new(DEFAULT_REORDER_CAP);
        sd.write(0, &data[..2048]);
        assert_eq!(sd.finish(4096), None, "half a stream has no whole digest");
    }

    #[test]
    fn the_head_is_captured_even_when_it_arrives_last() {
        let mut data = vec![0u8; 4096];
        data[..4].copy_from_slice(b"\x1f\x8b\x08\x00"); // gzip magic
        let mut sd = StreamDigest::new(DEFAULT_REORDER_CAP);
        sd.write(2048, &data[2048..]);
        sd.write(0, &data[..2048]);
        assert_eq!(&sd.head()[..4], b"\x1f\x8b\x08\x00");
    }

    /// A retried range can re-deliver bytes already consumed; hashing them twice
    /// would corrupt the digest.
    #[test]
    fn replayed_bytes_behind_the_frontier_are_ignored() {
        let data: Vec<u8> = (0..4096u32).map(|i| (i % 251) as u8).collect();
        let mut sd = StreamDigest::new(DEFAULT_REORDER_CAP);
        sd.write(0, &data[..2048]);
        sd.write(0, &data[..2048]); // duplicate delivery
        sd.write(2048, &data[2048..]);
        assert_eq!(sd.finish(4096), Some(reference(&data)));
    }
}
