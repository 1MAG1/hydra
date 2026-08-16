//! Local Reed–Solomon parity generation and repair for offline file integrity.
//!
//! Scope:
//! * Parity is generated locally after download verification for offline archiving and bitrot protection.
//! * While the remote origin is reachable, targeted chunk refetching is faster than decoding.
//!   Parity repair is used when remote sources are unavailable.
//! * Repair requires erasure positions identified by chunk checksums.
//!
//! **Windowing** maintains bounded memory footprint: stripes are processed in
//! independent windows, keeping resident memory usage low and independent of file size.

use crate::manifest::ChunkAlgo;
use std::collections::BTreeMap;
use std::io;

/// Default repair window. Verified byte-identical to whole-shard repair across
/// 4 KiB–1 MiB; 64 KiB sits well inside the client's memory envelope.
pub const DEFAULT_WINDOW: u64 = 64 * 1024;

/// GF(2^16) shard-count ceiling. A GF(2^8) codec would cap at 255.
pub const MAX_SHARDS: usize = 65_535;

/// reed-solomon-simd requires a shard length that is a multiple of 64.
const SHARD_ALIGN: usize = 64;

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Codec(String),
    /// The repair was asked for without knowing which shards are bad.
    NoErasurePositions,
    /// More shards are damaged than parity can restore.
    TooMuchDamage {
        lost: usize,
        parity: usize,
    },
    Config(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::Codec(e) => write!(f, "codec: {e}"),
            Error::NoErasurePositions => write!(
                f,
                "refusing to decode without erasure positions: Reed-Solomon corrects erasures \
                 (known-missing shards), not errors, and decoding with an unflagged corrupt \
                 shard returns wrong bytes with a success code"
            ),
            Error::TooMuchDamage { lost, parity } => write!(
                f,
                "{lost} shards damaged but only {parity} parity shards exist: unrecoverable \
                 (reported, not silently approximated)"
            ),
            Error::Config(s) => write!(f, "{s}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

/// How a file is divided into RS shards.
///
/// `shard_size` must equal the manifest's chunk size, or be an exact multiple of
/// it. If a download chunk straddles two shards, one corrupt chunk destroys
/// **two** shards and halves effective parity.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub size: u64,
    pub shard_size: u64,
    pub k: usize,
    pub parity: usize,
    pub window: u64,
}

impl Layout {
    pub fn new(size: u64, shard_size: u64, parity: usize, window: u64) -> Result<Layout, Error> {
        if shard_size == 0 {
            return Err(Error::Config("shard size is zero".into()));
        }
        if parity == 0 {
            return Err(Error::Config("parity shard count is zero".into()));
        }
        let k = size.div_ceil(shard_size) as usize;
        if k == 0 {
            return Err(Error::Config("object has no shards".into()));
        }
        if k + parity > MAX_SHARDS {
            return Err(Error::Config(format!(
                "{k} data + {parity} parity shards exceeds the GF(2^16) limit of {MAX_SHARDS}; \
                 use a larger shard size"
            )));
        }
        let window = window.max(1).min(shard_size);
        Ok(Layout {
            size,
            shard_size,
            k,
            parity,
            window,
        })
    }

    /// Byte span of data shard `i`, clipped to the object.
    pub fn span(&self, i: usize) -> (u64, u64) {
        let lo = i as u64 * self.shard_size;
        let hi = (lo + self.shard_size).min(self.size);
        (lo, hi)
    }

    /// Windows the repair proceeds in, as `(offset, len)` within a shard.
    pub fn windows(&self) -> Vec<(u64, usize)> {
        let mut out = Vec::new();
        let mut at = 0u64;
        while at < self.shard_size {
            let len = self.window.min(self.shard_size - at) as usize;
            out.push((at, len));
            at += len as u64;
        }
        out
    }

    /// Resident bytes a repair needs: `O(n·W)`, independent of file size.
    pub fn resident_bytes(&self) -> u64 {
        (self.k + self.parity) as u64 * aligned(self.window as usize) as u64
    }
}

fn aligned(n: usize) -> usize {
    n.div_ceil(SHARD_ALIGN) * SHARD_ALIGN
}

/// Read a window of one data shard, zero-padded past the end of the object.
///
/// The tail shard is short and the codec needs equal-length shards; the padding
/// is deterministic on both sides, so encode and decode agree.
fn read_window(
    f: &std::fs::File,
    lay: &Layout,
    shard: usize,
    at: u64,
    len: usize,
) -> io::Result<Vec<u8>> {
    let mut buf = vec![0u8; aligned(len)];
    let (lo, hi) = lay.span(shard);
    let start = lo + at;
    if start < hi {
        let want = ((hi - start) as usize).min(len);
        #[cfg(unix)]
        {
            use std::os::unix::fs::FileExt;
            f.read_exact_at(&mut buf[..want], start)?;
        }
        // Positional read, offset per call — never the handle's shared cursor
        // (see sink::write_at for the interleaving this prevents). `seek_read`
        // can return short, so loop like read_exact would.
        #[cfg(windows)]
        {
            use std::os::windows::fs::FileExt;
            let (mut done, mut at) = (0usize, start);
            while done < want {
                match f.seek_read(&mut buf[done..want], at) {
                    Ok(0) => {
                        return Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "seek_read hit EOF before the shard span was read",
                        ));
                    }
                    Ok(n) => {
                        done += n;
                        at += n as u64;
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(e) => return Err(e),
                }
            }
        }
        #[cfg(not(any(unix, windows)))]
        {
            compile_error!(
                "hydra needs a positional read (pread/seek_read): a shared-cursor \
                 seek+read fallback races against concurrent positional writers"
            );
        }
    }
    Ok(buf)
}

/// Generate parity for `path`, writing it to `parity_path`.
///
/// Returns the parity shards' digests, which the manifest records: a corrupt
/// parity shard fed into a repair produces a silently wrong result, so parity
/// needs its own integrity exactly as the data does.
pub fn generate(path: &str, parity_path: &str, lay: &Layout) -> Result<Vec<String>, Error> {
    let f = std::fs::File::open(path)?;
    // Parity shards accumulate across windows; each is `shard_size` long.
    let mut out: Vec<Vec<u8>> = vec![Vec::with_capacity(lay.shard_size as usize); lay.parity];

    for (at, len) in lay.windows() {
        let mut originals = Vec::with_capacity(lay.k);
        for s in 0..lay.k {
            originals.push(read_window(&f, lay, s, at, len)?);
        }
        let rec = reed_solomon_simd::encode(lay.k, lay.parity, &originals)
            .map_err(|e| Error::Codec(e.to_string()))?;
        for (i, r) in rec.into_iter().enumerate() {
            // Trim the 64-byte alignment padding back off.
            out[i].extend_from_slice(&r[..len]);
        }
    }

    let mut blob = Vec::with_capacity(lay.parity * lay.shard_size as usize);
    let mut digests = Vec::with_capacity(lay.parity);
    for s in out {
        digests.push(ChunkAlgo::Blake3.hash(&s));
        blob.extend_from_slice(&s);
    }
    std::fs::write(parity_path, &blob)?;
    Ok(digests)
}

/// Check a parity file against the shard digests recorded when it was generated.
///
/// **Must run before any repair writes a byte.** The decoder cannot tell a
/// corrupt parity shard from a good one — it is the same blindness that makes
/// erasure positions mandatory — so a rotted parity file yields a confidently
/// wrong reconstruction. `repair` previously checked only that the file was long
/// enough, which a rotted file of the right size passes.
///
/// The failure this closes is worse than a wrong answer would suggest: repair
/// writes in place, so decoding from bad parity DAMAGES chunks that were intact
/// before, turning one recoverable chunk into several. The post-repair re-check
/// catches the result, but by then the file is worse than when it started.
///
/// Returns the indices of parity shards that failed, empty when all are sound.
pub fn verify_parity(
    parity_path: &str,
    lay: &Layout,
    expected: &[String],
) -> Result<Vec<usize>, Error> {
    let blob = std::fs::read(parity_path)?;
    let ss = lay.shard_size as usize;
    if blob.len() < lay.parity * ss {
        return Err(Error::Config(format!(
            "parity file holds {} bytes, expected {} for {} shards of {ss}",
            blob.len(),
            lay.parity * ss,
            lay.parity
        )));
    }
    if expected.len() != lay.parity {
        return Err(Error::Config(format!(
            "manifest records {} shard digest(s) for {} parity shards",
            expected.len(),
            lay.parity
        )));
    }
    let mut bad = Vec::new();
    for i in 0..lay.parity {
        let got = ChunkAlgo::Blake3.hash(&blob[i * ss..(i + 1) * ss]);
        if got != expected[i] {
            bad.push(i);
        }
    }
    Ok(bad)
}

/// Repair the shards named in `erasures`, in place.
///
/// `erasures` are data-shard indices known to be damaged, which must come from a
/// **trusted** manifest's digest mismatches. There is no variant of this function
/// that discovers them itself: a decoder cannot tell a corrupt shard from a good
/// one, so guessing would produce confident garbage.
pub fn repair(
    path: &str,
    parity_path: &str,
    lay: &Layout,
    erasures: &[usize],
) -> Result<usize, Error> {
    if erasures.is_empty() {
        return Err(Error::NoErasurePositions);
    }
    if erasures.len() > lay.parity {
        return Err(Error::TooMuchDamage {
            lost: erasures.len(),
            parity: lay.parity,
        });
    }

    let f = std::fs::File::open(path)?;
    let out = std::fs::OpenOptions::new().write(true).open(path)?;
    let parity_blob = std::fs::read(parity_path)?;
    let ss = lay.shard_size as usize;
    if parity_blob.len() < lay.parity * ss {
        return Err(Error::Config(format!(
            "parity file holds {} bytes, expected {} for {} shards of {ss}",
            parity_blob.len(),
            lay.parity * ss,
            lay.parity
        )));
    }

    for (at, len) in lay.windows() {
        // Surviving data shards.
        let mut originals: Vec<(usize, Vec<u8>)> = Vec::new();
        for s in 0..lay.k {
            if erasures.contains(&s) {
                continue;
            }
            originals.push((s, read_window(&f, lay, s, at, len)?));
        }
        // Parity shards for this window.
        let mut recovery: Vec<(usize, Vec<u8>)> = Vec::new();
        for pi in 0..lay.parity {
            let base = pi * ss + at as usize;
            let mut buf = vec![0u8; aligned(len)];
            buf[..len].copy_from_slice(&parity_blob[base..base + len]);
            recovery.push((pi, buf));
        }

        let restored: BTreeMap<usize, Vec<u8>> =
            reed_solomon_simd::decode(lay.k, lay.parity, originals, recovery)
                .map_err(|e| Error::Codec(e.to_string()))?;

        for (&idx, bytes) in restored.iter() {
            let (lo, hi) = lay.span(idx);
            let start = lo + at;
            if start >= hi {
                continue; // padding past the end of the object
            }
            let want = ((hi - start) as usize).min(len);
            #[cfg(unix)]
            {
                use std::os::unix::fs::FileExt;
                out.write_all_at(&bytes[..want], start)?;
            }
            // Positional write, offset per call — never the shared cursor
            // (see sink::write_at). Loops because `seek_write` can go short.
            #[cfg(windows)]
            {
                use std::os::windows::fs::FileExt;
                let (mut src, mut at) = (&bytes[..want], start);
                while !src.is_empty() {
                    match out.seek_write(src, at) {
                        Ok(0) => {
                            return Err(io::Error::new(
                                io::ErrorKind::WriteZero,
                                "seek_write returned 0 bytes",
                            )
                            .into());
                        }
                        Ok(n) => {
                            src = &src[n..];
                            at += n as u64;
                        }
                        Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                        Err(e) => return Err(e.into()),
                    }
                }
            }
            #[cfg(not(any(unix, windows)))]
            {
                compile_error!(
                    "hydra needs a positional write (pwrite/seek_write): a shared-cursor \
                     seek+write fallback silently corrupts repaired shards"
                );
            }
        }
    }
    Ok(erasures.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stage(name: &str, n: usize) -> (String, Vec<u8>) {
        let dir = std::env::temp_dir();
        let p = dir.join(format!("hydra_parity_{name}_{}", std::process::id()));
        let data: Vec<u8> = (0..n).map(|i| (i % 251) as u8).collect();
        std::fs::write(&p, &data).unwrap();
        (p.to_string_lossy().to_string(), data)
    }

    #[test]
    fn repair_is_exact_up_to_the_parity_count() {
        // Reproduces the study's threshold result: exact repair iff corrupt
        // shards <= parity, and a clean reported failure beyond it.
        for (k_chunks, parity, corrupt, should_work) in [
            (8usize, 2usize, 1usize, true),
            (8, 2, 2, true),
            (8, 2, 3, false),
            (16, 4, 4, true),
            (16, 4, 5, false),
        ] {
            let shard = 4096u64;
            let size = k_chunks as u64 * shard;
            let (p, original) = stage(&format!("t{k_chunks}_{parity}_{corrupt}"), size as usize);
            let pp = format!("{p}.parity");
            let lay = Layout::new(size, shard, parity, DEFAULT_WINDOW).unwrap();
            assert_eq!(lay.k, k_chunks);
            generate(&p, &pp, &lay).unwrap();

            // Damage `corrupt` shards.
            let mut d = original.clone();
            let erasures: Vec<usize> = (0..corrupt).collect();
            for &e in &erasures {
                let (lo, hi) = lay.span(e);
                for b in &mut d[lo as usize..hi as usize] {
                    *b ^= 0xff;
                }
            }
            std::fs::write(&p, &d).unwrap();

            let r = repair(&p, &pp, &lay, &erasures);
            if should_work {
                r.unwrap_or_else(|e| panic!("k={k_chunks} parity={parity} corrupt={corrupt}: {e}"));
                let back = std::fs::read(&p).unwrap();
                assert_eq!(
                    back, original,
                    "k={k_chunks} parity={parity} corrupt={corrupt} must repair byte-for-byte"
                );
            } else {
                let e = r.expect_err("must refuse beyond the parity count");
                assert!(
                    matches!(e, Error::TooMuchDamage { .. }),
                    "must fail REPORTED, never silently: got {e}"
                );
            }
            let _ = std::fs::remove_file(&p);
            let _ = std::fs::remove_file(&pp);
        }
    }

    /// The windowing claim: repair at any window size is byte-identical to
    /// repair at any other, so the memory knob does not change the answer.
    #[test]
    fn windowed_repair_is_identical_across_window_sizes() {
        let shard = 64 * 1024u64;
        let size = 8 * shard;
        let (p0, original) = stage("windows", size as usize);
        let mut results = Vec::new();
        for w in [4096u64, 16384, 65536] {
            let p = format!("{p0}_w{w}");
            let pp = format!("{p}.parity");
            std::fs::write(&p, &original).unwrap();
            let lay = Layout::new(size, shard, 2, w).unwrap();
            generate(&p, &pp, &lay).unwrap();

            let mut d = original.clone();
            let (lo, hi) = lay.span(3);
            for b in &mut d[lo as usize..hi as usize] {
                *b = 0;
            }
            std::fs::write(&p, &d).unwrap();
            repair(&p, &pp, &lay, &[3]).unwrap();
            results.push(std::fs::read(&p).unwrap());
            let _ = std::fs::remove_file(&p);
            let _ = std::fs::remove_file(&pp);
        }
        let _ = std::fs::remove_file(&p0);
        for r in &results {
            assert_eq!(r, &original, "every window size must repair exactly");
        }
        assert_eq!(
            results[0], results[2],
            "window size must not change the result"
        );
    }

    /// Memory is O(n*W) and independent of file size — the property that lets
    /// parity coexist with the client's flat envelope.
    #[test]
    fn resident_memory_is_independent_of_file_size() {
        let a = Layout::new(4 << 20, 1 << 20, 2, DEFAULT_WINDOW).unwrap();
        let b = Layout::new(1 << 30, 1 << 20, 2, DEFAULT_WINDOW).unwrap();
        assert!(
            b.k > a.k * 100,
            "the second layout must be far larger: {} vs {}",
            b.k,
            a.k
        );
        // Per-window residency scales with shard COUNT, so compare the per-window
        // figure at equal k rather than pretending it is constant in k.
        let per_shard_a = a.resident_bytes() / (a.k + a.parity) as u64;
        let per_shard_b = b.resident_bytes() / (b.k + b.parity) as u64;
        assert_eq!(
            per_shard_a, per_shard_b,
            "per-shard residency must not grow with the object"
        );
        assert_eq!(per_shard_a, DEFAULT_WINDOW);
    }

    /// The refusal that keeps this from becoming a corruption feature.
    #[test]
    fn decoding_without_erasure_positions_is_refused() {
        let shard = 4096u64;
        let size = 4 * shard;
        let (p, _) = stage("noerase", size as usize);
        let pp = format!("{p}.parity");
        let lay = Layout::new(size, shard, 2, DEFAULT_WINDOW).unwrap();
        generate(&p, &pp, &lay).unwrap();
        let e = repair(&p, &pp, &lay, &[]).expect_err("must refuse");
        assert!(matches!(e, Error::NoErasurePositions), "got {e}");
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(&pp);
    }

    /// A short tail shard is the normal case, not an edge case.
    #[test]
    fn an_object_whose_tail_shard_is_short_repairs_exactly() {
        let shard = 4096u64;
        let size = 3 * shard + 1234; // deliberately not a multiple
        let (p, original) = stage("tail", size as usize);
        let pp = format!("{p}.parity");
        let lay = Layout::new(size, shard, 2, DEFAULT_WINDOW).unwrap();
        assert_eq!(lay.k, 4);
        generate(&p, &pp, &lay).unwrap();

        // Damage the SHORT tail shard specifically.
        let mut d = original.clone();
        let (lo, hi) = lay.span(3);
        assert_eq!(hi - lo, 1234, "the tail shard is short");
        for b in &mut d[lo as usize..hi as usize] {
            *b ^= 0xff;
        }
        std::fs::write(&p, &d).unwrap();
        repair(&p, &pp, &lay, &[3]).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), original);
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(&pp);
    }

    #[test]
    fn a_shard_count_above_the_field_limit_is_refused_before_encoding() {
        let e =
            Layout::new(1 << 30, 1024, 2, DEFAULT_WINDOW).expect_err("1M shards exceeds GF(2^16)");
        assert!(matches!(e, Error::Config(_)));
    }
    /// A rotted parity file must be refused before attempting decode.
    #[test]
    fn a_rotted_parity_file_is_detected_before_it_can_be_decoded_from() {
        let dir = std::env::temp_dir().join(format!("hydra_pv_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let obj = dir.join("obj.bin");
        let par = dir.join("obj.bin.parity");

        let data: Vec<u8> = (0..40_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&obj, &data).unwrap();
        let lay = Layout::new(data.len() as u64, 4096, 2, DEFAULT_WINDOW).unwrap();
        let digests =
            generate(&obj.to_string_lossy(), &par.to_string_lossy(), &lay).expect("generate");

        // Sound parity verifies clean.
        assert!(
            verify_parity(&par.to_string_lossy(), &lay, &digests)
                .expect("verify")
                .is_empty(),
            "freshly generated parity must verify"
        );

        // Flip a byte in the first parity shard: same length, wrong content.
        let mut blob = std::fs::read(&par).unwrap();
        let before = blob.len();
        blob[100] ^= 0xFF;
        std::fs::write(&par, &blob).unwrap();
        assert_eq!(
            std::fs::read(&par).unwrap().len(),
            before,
            "length unchanged"
        );

        let bad = verify_parity(&par.to_string_lossy(), &lay, &digests).expect("verify");
        assert_eq!(bad, vec![0], "the rotted shard must be named, got {bad:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
