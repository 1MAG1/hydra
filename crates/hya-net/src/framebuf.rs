//! A read buffer for protocol framing that consumes in O(1) and scans once.
//!
//! Extracted from the chunked-body decoder, where the straightforward
//! `Vec`-with-`drain` shape is quadratic twice over. Both costs are per-read on
//! the byte path, and both are invisible at large chunk sizes and severe at
//! small ones — the framing work scales with the number of tokens, while the
//! payload scales with bytes, so a body delivered in 1 KiB chunks pays roughly a
//! thousand times the framing cost of the same body in 1 MiB chunks.
//!
//! Two properties fix that:
//!
//! * **Consuming is an integer add.** `drain(..n)` memmoves the whole residual
//!   tail to offset 0; a chunked body consumes three tokens per chunk, so the
//!   tail is moved three times per chunk. Here a consumed prefix is tracked by
//!   `head` and space is reclaimed only when the prefix is large enough to be
//!   worth reclaiming, which amortizes the move to O(1) per byte.
//! * **A failed search is not repeated.** `windows(2).position(...)` restarts at
//!   offset 0 every call, so a size line still in flight costs a full rescan of
//!   everything buffered, once per read. Here the CRLF search resumes from a
//!   saved cursor, so no byte is examined twice, and it locates the `\r` with
//!   `memchr` (SIMD-accelerated, runtime-dispatched: AVX2/SSE2 on x86-64, NEON
//!   on aarch64) rather than stepping a two-byte window in scalar code.
//!
//! The reclaim threshold is a deliberate trade: compacting too eagerly restores
//! the memmove cost, and never compacting makes the buffer grow without bound on
//! a long body. Reclaiming when the consumed prefix exceeds both half the buffer
//! and a floor bounds retained memory to roughly twice the live frame while
//! keeping the amortized move cost constant.

/// Reclaim space once the consumed prefix passes this many bytes AND exceeds
/// half the allocation. The floor stops tiny frames from triggering a memmove.
const COMPACT_FLOOR: usize = 16 * 1024;

/// A growable byte buffer with a consumed-prefix cursor and a resumable scan
/// position.
pub struct FrameBuf {
    buf: Vec<u8>,
    /// Bytes before this offset are consumed and may be reclaimed.
    head: usize,
    /// Absolute offset into `buf` where the next CRLF search resumes.
    /// Invariant: `scan >= head`.
    scan: usize,
}

impl FrameBuf {
    pub fn new() -> Self {
        FrameBuf {
            buf: Vec::new(),
            head: 0,
            scan: 0,
        }
    }

    pub fn with_initial(initial: &[u8]) -> Self {
        FrameBuf {
            buf: initial.to_vec(),
            head: 0,
            scan: 0,
        }
    }

    /// The unconsumed bytes.
    #[inline]
    pub fn data(&self) -> &[u8] {
        &self.buf[self.head..]
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.buf.len() - self.head
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Append freshly read bytes.
    pub fn extend(&mut self, bytes: &[u8]) {
        self.maybe_compact();
        self.buf.extend_from_slice(bytes);
    }

    /// Mark the first `n` unconsumed bytes as consumed.
    ///
    /// O(1): no data moves. The scan cursor advances with the head so a search
    /// never revisits consumed bytes.
    #[inline]
    pub fn consume(&mut self, n: usize) {
        debug_assert!(n <= self.len(), "consume {n} past end of {}", self.len());
        self.head += n.min(self.len());
        if self.scan < self.head {
            self.scan = self.head;
        }
    }

    /// Find the next `\r\n`, returning its offset relative to the unconsumed
    /// start.
    ///
    /// Resumable: a `None` leaves the cursor where the search gave up, so the
    /// next call after more bytes arrive examines only the new ones. The cursor
    /// is parked one byte back when the buffer ends on a bare `\r`, since the
    /// `\n` may be the first byte of the next read.
    pub fn find_crlf(&mut self) -> Option<usize> {
        loop {
            if self.scan + 1 >= self.buf.len() {
                // Not enough bytes for a CRLF. Park on a trailing lone `\r` so a
                // boundary split across two reads is still found.
                self.scan = self.buf.len().saturating_sub(1).max(self.head);
                return None;
            }
            let hay = &self.buf[self.scan..];
            match memchr::memchr(b'\r', hay) {
                None => {
                    // No `\r` at all: everything scanned is settled.
                    self.scan = self.buf.len();
                    return None;
                }
                Some(i) => {
                    let at = self.scan + i;
                    if at + 1 >= self.buf.len() {
                        self.scan = at;
                        return None;
                    }
                    if self.buf[at + 1] == b'\n' {
                        return Some(at - self.head);
                    }
                    // A bare `\r` inside the line: keep looking after it.
                    self.scan = at + 1;
                }
            }
        }
    }

    /// Drop the consumed prefix when it is worth the memmove.
    fn maybe_compact(&mut self) {
        if self.head == 0 {
            return;
        }
        if self.head >= COMPACT_FLOOR && self.head * 2 >= self.buf.len() {
            self.buf.drain(..self.head);
            self.scan -= self.head;
            self.head = 0;
        }
    }

    /// Bytes currently held by the allocation, for tests asserting the buffer
    /// does not grow without bound.
    #[cfg(test)]
    pub fn allocated(&self) -> usize {
        self.buf.len()
    }
}

impl Default for FrameBuf {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scalar reference for `find_crlf`, to differentially test the memchr path.
    fn reference_crlf(data: &[u8]) -> Option<usize> {
        data.windows(2).position(|w| w == b"\r\n")
    }

    #[test]
    fn find_crlf_matches_the_scalar_reference_on_every_shape() {
        let cases: Vec<&[u8]> = vec![
            b"",
            b"\r",
            b"\n",
            b"\r\n",
            b"a\r\n",
            b"\r\r\n",
            b"\r\r\r\n",
            b"abc\rdef\r\n",
            b"no crlf here",
            b"trailing\r",
            b"\n\r",
            b"400\r\nbody",
            b"1a; ext=1\r\n",
        ];
        for c in cases {
            let mut fb = FrameBuf::with_initial(c);
            assert_eq!(
                fb.find_crlf(),
                reference_crlf(c),
                "mismatch on {:?}",
                std::str::from_utf8(c)
            );
        }
    }

    /// The resumable cursor must not miss a CRLF split across two reads — the
    /// exact case that makes a naive "scan only the new bytes" wrong.
    #[test]
    fn a_crlf_split_across_two_reads_is_still_found() {
        let mut fb = FrameBuf::with_initial(b"200\r");
        assert_eq!(fb.find_crlf(), None, "incomplete boundary is not a match");
        fb.extend(b"\nPAYLOAD");
        assert_eq!(fb.find_crlf(), Some(3), "boundary must be found after join");
    }

    /// Byte-at-a-time delivery is the adversarial case for a resumable scan.
    #[test]
    fn byte_at_a_time_delivery_finds_the_boundary_exactly_once() {
        let body = b"1f;x\r\n";
        let mut fb = FrameBuf::new();
        let mut found = None;
        for (i, b) in body.iter().enumerate() {
            fb.extend(&[*b]);
            if let Some(at) = fb.find_crlf() {
                found = Some((i, at));
                break;
            }
        }
        assert_eq!(
            found,
            Some((5, 4)),
            "must match only when the full CRLF has arrived"
        );
    }

    #[test]
    fn consume_does_not_move_data_and_shifts_the_search_origin() {
        let mut fb = FrameBuf::with_initial(b"5\r\nhello\r\n");
        let nl = fb.find_crlf().expect("size line");
        assert_eq!(nl, 1);
        fb.consume(nl + 2);
        assert_eq!(fb.data(), b"hello\r\n");
        assert_eq!(fb.find_crlf(), Some(5), "next boundary is relative to head");
    }

    /// The point of the cursor: a long body must not grow the allocation
    /// without bound just because it was consumed in small pieces.
    #[test]
    fn allocation_stays_bounded_across_many_small_tokens() {
        let mut fb = FrameBuf::new();
        let block = vec![0xABu8; 4096];
        let mut peak = 0usize;
        for _ in 0..2000 {
            fb.extend(&block);
            fb.consume(block.len());
            peak = peak.max(fb.allocated());
        }
        assert!(fb.is_empty(), "everything was consumed");
        assert!(
            peak < 8 * COMPACT_FLOOR,
            "allocation grew to {peak} bytes; the compaction threshold is not reclaiming"
        );
    }

    /// A `\r` that is not followed by `\n` must not be treated as a boundary,
    /// and must not stall the scan either.
    #[test]
    fn bare_carriage_returns_are_skipped_not_matched() {
        let mut fb = FrameBuf::with_initial(b"\r\r\r\rok\r\n");
        assert_eq!(fb.find_crlf(), Some(6));
    }
}
