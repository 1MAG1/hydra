//! [`SparseSink`]: positioned writes at exact offsets, no reassembly buffer,
//! resident memory independent of object size.

use crate::stream_digest;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};

/// A file written by positioned writes: no reassembly buffer, no ordering
/// requirement, memory independent of object size.
pub struct SparseSink {
    /// `None` in discard mode: the bytes are measured and dropped, never stored.
    file: Option<std::fs::File>,
    pub written: AtomicU64,
    /// Optional stream observer. Attached when the caller needs the object's
    /// digest but will have no file to hash afterwards (`--no-save`). The sink
    /// is the right home for it: it is the single point every fragment passes
    /// through, so no byte can reach storage without being accounted for.
    digest: Option<std::sync::Mutex<stream_digest::StreamDigest>>,
}

impl SparseSink {
    /// Create or reopen the output as a sparse file of `size` bytes.
    ///
    /// Deliberately does NOT truncate. Two handles are legitimately opened on the same path
    /// in one transfer — the concurrency probe writes real bytes at true offsets, then the
    /// transfer opens the file again — and a truncating reopen silently zeroed everything
    /// the probe had written. Because the scheduler had already marked those ranges held,
    /// they were never refetched: the object arrived with a 1.5 MiB hole of zeros at offset
    /// 0, the right length, and a stable digest. Every length check passed and the file was
    /// unusable, which is the worst failure mode this project keeps finding.
    ///
    /// `set_len` still establishes the full extent (positioned writes need somewhere to
    /// land) and truncates a LONGER pre-existing file, so a stale larger download cannot
    /// leave a tail behind. Callers that want a genuinely fresh file remove it first; that
    /// is what `--force` and the restart path do.
    pub fn create(path: &str, size: u64) -> io::Result<Self> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            // Explicit, not omitted: this is the load-bearing decision of this function
            // and a future edit must have to think about it.
            .truncate(false)
            .write(true)
            .read(true)
            .open(path)?;
        file.set_len(size)?;
        Ok(SparseSink {
            file: Some(file),
            written: AtomicU64::new(0),
            digest: None,
        })
    }

    /// A sink that accounts for bytes without storing them anywhere.
    ///
    /// This is what `--no-save` needs. The flag used to be implemented as
    /// create-write-digest-delete, which meant the file existed for the whole
    /// transfer: a run interrupted at any point left it behind (measured: 45 MB
    /// surviving a kill), and the flag could not work at all in a directory the
    /// user cannot write to. Nothing about discarding requires a file — the
    /// length, digest, and format classification are all functions of the byte
    /// STREAM, not of a stored copy.
    ///
    /// `write_at` stays a no-op that still counts, so every caller measuring
    /// progress through `written` behaves identically in both modes.
    pub fn discarding() -> Self {
        SparseSink {
            file: None,
            written: AtomicU64::new(0),
            digest: None,
        }
    }

    /// True when this sink stores nothing.
    pub fn is_discarding(&self) -> bool {
        self.file.is_none()
    }

    /// Compute the object's SHA-256 from the stream as it passes through.
    ///
    /// Needed when there will be no file to hash afterwards. Note that ranges
    /// arrive OUT OF ORDER, so this is not a plain rolling hash — see
    /// [`stream_digest`] for how the contiguous prefix is tracked and what
    /// happens when reordering exceeds the buffer budget.
    pub fn with_digest(mut self, cap: usize) -> Self {
        self.digest = Some(std::sync::Mutex::new(stream_digest::StreamDigest::new(cap)));
        self
    }

    /// Take the accumulated digest state, if one was attached.
    ///
    /// Returns `(sha256, head_bytes, unavailable_reason)`. The digest is `None`
    /// when the stream could not be hashed — never a partial or arrival-order
    /// value.
    pub fn take_digest(&self, size: u64) -> Option<(Option<String>, Vec<u8>, Option<String>)> {
        let m = self.digest.as_ref()?;
        let guard = m.lock().ok()?;
        let head = guard.head().to_vec();
        let reason = guard.unavailable_reason(size);
        drop(guard);
        // `finish` consumes, so swap a fresh one in and take ownership.
        let mut g = m.lock().ok()?;
        let taken = std::mem::replace(&mut *g, stream_digest::StreamDigest::new(1));
        Some((taken.finish(size), head, reason))
    }

    /// Write `buf` at absolute offset `off`. Thread-safe without a lock: the
    /// scheduler guarantees ranges are disjoint (the coverage invariant), so no
    /// two connections ever write the same byte.
    pub fn write_at(&self, off: u64, buf: &[u8]) -> io::Result<()> {
        // Observed BEFORE storage so the digest sees every byte in both modes,
        // and sees them even when there is no file at all.
        if let Some(d) = &self.digest {
            if let Ok(mut g) = d.lock() {
                g.write(off, buf);
            }
        }
        match &self.file {
            // Discard mode: count the bytes, store nothing. Deliberately still
            // returns Ok so the transfer, the progress display, and the
            // scheduler's completion accounting are identical in both modes.
            None => {}
            Some(file) => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::FileExt;
                    file.write_all_at(buf, off)?;
                }
                // Windows: `seek_write` is a positional write (WriteFile with an
                // explicit OVERLAPPED offset) — the offset travels with each call,
                // so concurrent disjoint writes are safe, same as pwrite. A
                // seek-then-write pair is NOT: `seek` moves the ONE cursor all
                // clones of this handle share, so two connections interleave as
                // A-seek, B-seek, A-write and A's block lands at B's offset. That
                // was a real defect: every multi-connection transfer corrupted at
                // block-aligned offsets while single-connection tests stayed green.
                // `seek_write` can write short, so loop like write_all would.
                #[cfg(windows)]
                {
                    use std::os::windows::fs::FileExt;
                    let (mut buf, mut off) = (buf, off);
                    while !buf.is_empty() {
                        match file.seek_write(buf, off) {
                            Ok(0) => {
                                return Err(io::Error::new(
                                    io::ErrorKind::WriteZero,
                                    "seek_write returned 0 bytes",
                                ));
                            }
                            Ok(n) => {
                                buf = &buf[n..];
                                off += n as u64;
                            }
                            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                            Err(e) => return Err(e),
                        }
                    }
                }
                #[cfg(not(any(unix, windows)))]
                {
                    compile_error!(
                        "hydra needs a positional write (pwrite/seek_write): a shared-cursor \
                         seek+write fallback silently corrupts multi-connection transfers"
                    );
                }
            }
        }
        let _ = off;
        // `fetch_add`, not `load` + `store`. Every connection calls `write_at`
        // concurrently, and a read-modify-write split into two separate atomic
        // operations is a lost update: two connections that load the same value
        // both store their own sum, and one write's bytes vanish from the count.
        // The bug is silent and load-shaped — it needs concurrent writes to
        // appear at all, so it undercounts more the faster the transfer goes,
        // which is why a single-connection test could never see it. `written`
        // drives the progress display and the completion check, so an
        // undercount is a transfer that looks unfinished.
        self.written.fetch_add(buf.len() as u64, Ordering::Relaxed);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reopening_the_sink_does_not_erase_what_another_handle_wrote() {
        // The bug this guards: the concurrency probe writes real bytes at true offsets, the
        // transfer then opens the same path again, and a truncating reopen zeroed the
        // probe's 1.5 MiB. Those ranges were already marked held, so they were never
        // refetched -- the file had the right length, a stable digest, and a hole of zeros
        // at offset 0. `file` is a shell command away from looking fine.
        let path = std::env::temp_dir().join(format!("hydra_sink_reopen_{}", std::process::id()));
        let p = path.to_string_lossy().to_string();
        let _ = std::fs::remove_file(&p);

        let first = SparseSink::create(&p, 4096).unwrap();
        first.write_at(0, &[0xAB; 64]).unwrap();
        drop(first);

        let second = SparseSink::create(&p, 4096).unwrap();
        second.write_at(2048, &[0xCD; 64]).unwrap();
        drop(second);

        let got = std::fs::read(&p).unwrap();
        assert_eq!(got.len(), 4096);
        assert_eq!(
            &got[..64],
            &[0xAB; 64],
            "the first handle's bytes must survive the second handle's open"
        );
        assert_eq!(&got[2048..2112], &[0xCD; 64]);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn a_longer_stale_file_is_cut_back_to_the_new_size() {
        // Not truncating on open must not mean a stale, LONGER download leaves a tail: the
        // extent is set explicitly, which shortens as well as extends.
        let path = std::env::temp_dir().join(format!("hydra_sink_stale_{}", std::process::id()));
        let p = path.to_string_lossy().to_string();
        std::fs::write(&p, vec![0xFF; 8192]).unwrap();
        let s = SparseSink::create(&p, 1024).unwrap();
        drop(s);
        assert_eq!(
            std::fs::metadata(&p).unwrap().len(),
            1024,
            "set_len must cut a longer pre-existing file, or a stale tail survives"
        );
        let _ = std::fs::remove_file(&p);
    }
}
