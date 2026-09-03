//! Per-chunk digest manifests for verification and integrity checking.
//!
//! Chunk positions are verified against manifests. Erasure positions for
//! Reed-Solomon repair require validation against trusted chunk hashes.
//! Targeted refetches are self-correcting by verifying against target hashes.
//!
//! The chunk grid is fixed over the object and deliberately independent of how
//! the scheduler divides work. Preemption reshapes a connection's range
//! mid-transfer, so digests over scheduler ranges would be digests over a moving
//! target — different on every run and verifiable by nobody else.

use crate::digest::Algo;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const FORMAT_VERSION: u32 = 1;
/// Default chunk grid. Large enough that the manifest is a rounding error
/// (0.012% of the object), small enough that a refetch is cheap.
pub const DEFAULT_CHUNK: u64 = 4 * 1024 * 1024;

/// How a manifest was obtained, which decides what it licenses.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Trust {
    /// From the user, or a channel authenticated independently of the object.
    Trusted,
    /// From the origin or a sidecar beside the object. Detection only.
    Advertised,
}

impl Trust {
    /// May erasure positions derived from this manifest drive an RS repair?
    ///
    /// Only for `Trusted`. See the module note: a decode trusts its positions
    /// absolutely, so taking them from whoever served the bytes hands them
    /// control of the output.
    pub fn may_drive_repair(self) -> bool {
        matches!(self, Trust::Trusted)
    }
}

/// Digest algorithms permitted in a manifest.
///
/// Narrower than [`Algo`] on purpose: CRC32 is an error-detecting code whose
/// collisions are arithmetic, and MD5 is broken cheaply enough that a manifest
/// built on it would license repairs an attacker chooses. A manifest is exactly
/// where that matters.
///
/// # Why SHA-1 is in this list and MD5 is not
///
/// Not because SHA-1 is sound — it is not; chosen-prefix collisions against it
/// are a purchased commodity. It is here because Metalink 3.0 `<pieces>` are
/// overwhelmingly SHA-1 (the format inherited the choice from BitTorrent), so
/// refusing it does not make anything safer, it just deletes per-chunk
/// verification for most real mirror lists and leaves the whole-object SHA-256
/// as the only check — which localises nothing.
///
/// The distinction is drawn where it belongs instead: [`Self::is_collision_resistant`]
/// is false for SHA-1, and [`ChunkVerifier::new`] refuses to grant
/// [`Trust::Trusted`] to a manifest built on an algorithm that is not. So SHA-1
/// pieces can *detect* a bad chunk and drive a refetch from another mirror —
/// which is what they are for, and what a transmission fault needs — while never
/// being allowed to name erasure positions for a Reed-Solomon decode, where a
/// forged position rewrites bytes the attacker chooses.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChunkAlgo {
    Blake3,
    Sha256,
    Sha512,
    Sha1,
}

impl ChunkAlgo {
    pub fn as_str(self) -> &'static str {
        match self {
            ChunkAlgo::Blake3 => "blake3",
            ChunkAlgo::Sha256 => "sha256",
            ChunkAlgo::Sha512 => "sha512",
            ChunkAlgo::Sha1 => "sha1",
        }
    }

    /// Is finding two inputs with this digest infeasible?
    ///
    /// False for SHA-1. See the type note: this is what keeps a Metalink piece
    /// list useful for detection without letting it drive a parity repair.
    pub fn is_collision_resistant(self) -> bool {
        !matches!(self, ChunkAlgo::Sha1)
    }

    pub fn hash(self, bytes: &[u8]) -> String {
        use sha2::Digest as _;
        match self {
            ChunkAlgo::Blake3 => blake3::hash(bytes).to_hex().to_string(),
            ChunkAlgo::Sha256 => {
                let mut h = sha2::Sha256::new();
                h.update(bytes);
                crate::digest::to_lower_hex(&h.finalize())
            }
            ChunkAlgo::Sha512 => {
                let mut h = sha2::Sha512::new();
                h.update(bytes);
                crate::digest::to_lower_hex(&h.finalize())
            }
            ChunkAlgo::Sha1 => {
                let mut h = sha1::Sha1::new();
                h.update(bytes);
                crate::digest::to_lower_hex(&h.finalize())
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ObjectMeta {
    pub size: u64,
    pub chunk_size: u64,
    /// Algorithm-prefixed, e.g. `sha256:4f07…`.
    pub digest: Option<String>,
    /// The ETag or Last-Modified the object was served with, verbatim.
    pub validator: Option<String>,
    pub url: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChunkList {
    pub algo: ChunkAlgo,
    /// Lowercase hex, in object order.
    pub digests: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ParityMeta {
    pub algo: String,
    pub field: String,
    pub k: usize,
    pub parity_shards: usize,
    pub window: u64,
    pub file: String,
    pub shard_digests: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub hydra_manifest: u32,
    pub object: ObjectMeta,
    pub chunks: ChunkList,
    #[serde(default)]
    pub parity: Option<ParityMeta>,
}

impl Manifest {
    /// Chunk count implied by the object's size and grid.
    pub fn chunk_count(size: u64, chunk_size: u64) -> usize {
        if chunk_size == 0 {
            return 0;
        }
        size.div_ceil(chunk_size) as usize
    }

    /// Byte span of chunk `i`.
    pub fn span(&self, i: usize) -> (u64, u64) {
        let lo = i as u64 * self.object.chunk_size;
        let hi = (lo + self.object.chunk_size).min(self.object.size);
        (lo, hi)
    }

    /// Parse and structurally validate.
    ///
    /// A manifest whose chunk count disagrees with its own size and grid is
    /// **malformed, not merely suspicious** — it describes a different object,
    /// and using it would attribute mismatches to the wrong chunks.
    pub fn parse(s: &str) -> Result<Manifest, String> {
        let m: Manifest = serde_json::from_str(s).map_err(|e| format!("unparsable: {e}"))?;
        if m.hydra_manifest != FORMAT_VERSION {
            return Err(format!(
                "manifest format version {} is not supported (this build reads {FORMAT_VERSION})",
                m.hydra_manifest
            ));
        }
        if m.object.chunk_size == 0 {
            return Err("chunk_size is zero".into());
        }
        let want = Manifest::chunk_count(m.object.size, m.object.chunk_size);
        if m.chunks.digests.len() != want {
            return Err(format!(
                "manifest describes {} chunks but its size {} at chunk_size {} implies {want}; \
                 this manifest is for a different object",
                m.chunks.digests.len(),
                m.object.size,
                m.object.chunk_size
            ));
        }
        if let Some(p) = &m.parity {
            // Alignment rule: a download chunk straddling two RS shards means one
            // corrupt chunk destroys TWO shards, halving effective parity.
            if p.window == 0 {
                return Err("parity window is zero".into());
            }
            if p.k == 0 {
                return Err("parity k is zero".into());
            }
        }
        Ok(m)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Does this manifest describe the object currently being served?
    ///
    /// A validator disagreement means the manifest is **stale, not wrong**: the
    /// user may legitimately be checking an old copy against an old manifest. The
    /// caller says so and ignores it rather than failing.
    pub fn matches_validator(&self, live: Option<&str>) -> bool {
        match (&self.object.validator, live) {
            (Some(a), Some(b)) => a == b,
            // No validator on one side is not a disagreement.
            _ => true,
        }
    }
}

/// Verifies chunks against a manifest as bytes arrive.
///
/// A chunk is checked the moment its last byte lands, whichever connection
/// delivered which part of it — that is what the fixed grid buys. Bytes are held
/// only for chunks that are still incomplete, so the memory bound is
/// `O(in-flight chunks x chunk_size)` rather than the object size.
pub struct ChunkVerifier {
    manifest: Manifest,
    trust: Trust,
    /// Partially-filled chunks: index -> (bytes, filled count).
    staging: BTreeMap<usize, (Vec<u8>, u64)>,
    verified: Vec<bool>,
    failed: Vec<usize>,
}

impl ChunkVerifier {
    /// # Trust is granted by the CALLER and capped by the ALGORITHM
    ///
    /// `trust` says where the manifest came from, which is the caller's
    /// question. Whether that provenance can license a Reed-Solomon repair is
    /// also a function of the digest itself, and that is not the caller's to
    /// judge — so a manifest built on an algorithm that is not collision
    /// resistant is capped at [`Trust::Advertised`] here, however it arrived.
    ///
    /// The case this exists for is a Metalink `<pieces type="sha1">` handed over
    /// as a local file. The file is trusted; SHA-1 is not. Detection and targeted
    /// refetch still work — that is what piece digests are for — while an
    /// erasure decode, which trusts its positions absolutely, does not get to
    /// take them from a digest whose collisions can be purchased.
    pub fn new(manifest: Manifest, trust: Trust) -> Self {
        let n = manifest.chunks.digests.len();
        let trust = if manifest.chunks.algo.is_collision_resistant() {
            trust
        } else {
            Trust::Advertised
        };
        ChunkVerifier {
            manifest,
            trust,
            staging: BTreeMap::new(),
            verified: vec![false; n],
            failed: Vec::new(),
        }
    }

    pub fn trust(&self) -> Trust {
        self.trust
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// Record bytes at an absolute offset. Returns chunks that just FAILED.
    ///
    /// Returning only failures rather than every completion keeps the caller's
    /// hot path free of per-chunk bookkeeping: a verified chunk needs no action.
    pub fn write(&mut self, off: u64, buf: &[u8]) -> Vec<usize> {
        let cs = self.manifest.object.chunk_size;
        let mut newly_failed = Vec::new();
        let mut pos = off;
        let mut rest = buf;
        while !rest.is_empty() {
            let idx = (pos / cs) as usize;
            if idx >= self.verified.len() {
                break;
            }
            let (lo, hi) = self.manifest.span(idx);
            let take = ((hi - pos) as usize).min(rest.len());
            let entry = self
                .staging
                .entry(idx)
                .or_insert_with(|| (vec![0u8; (hi - lo) as usize], 0));
            let at = (pos - lo) as usize;
            entry.0[at..at + take].copy_from_slice(&rest[..take]);
            entry.1 += take as u64;
            if entry.1 >= hi - lo {
                let (bytes, _) = self.staging.remove(&idx).expect("just inserted");
                let got = self.manifest.chunks.algo.hash(&bytes);
                if got == self.manifest.chunks.digests[idx] {
                    self.verified[idx] = true;
                } else {
                    if !self.failed.contains(&idx) {
                        self.failed.push(idx);
                    }
                    newly_failed.push(idx);
                }
            }
            pos += take as u64;
            rest = &rest[take..];
        }
        newly_failed
    }

    /// Feed an entire reader through the verifier in chunk-size blocks,
    /// starting at offset 0.
    ///
    /// The read loop must refill until a full chunk (or EOF) is in hand:
    /// `Read::read` may return short counts, and handing a short block to
    /// [`write`](Self::write) mid-file would stage bytes against the wrong
    /// span. Every caller that verifies a file on disk needs exactly this
    /// loop, so it lives here rather than in each of them.
    pub fn write_reader<R: std::io::Read>(&mut self, r: &mut R) -> std::io::Result<()> {
        let cs = self.manifest.object.chunk_size as usize;
        let mut buf = vec![0u8; cs];
        let mut off = 0u64;
        loop {
            let mut filled = 0usize;
            while filled < cs {
                match r.read(&mut buf[filled..])? {
                    0 => break,
                    n => filled += n,
                }
            }
            if filled == 0 {
                return Ok(());
            }
            self.write(off, &buf[..filled]);
            off += filled as u64;
            if filled < cs {
                return Ok(());
            }
        }
    }

    /// Chunks whose digest did not match, as byte spans to refetch.
    pub fn failed_spans(&self) -> Vec<(u64, u64)> {
        self.failed.iter().map(|&i| self.manifest.span(i)).collect()
    }

    pub fn failed_indices(&self) -> &[usize] {
        &self.failed
    }

    /// Erasure positions for an RS repair, or `None` when the manifest may not
    /// be used for one.
    ///
    /// `None` is not "no corruption" — it is "this manifest does not license a
    /// repair". Callers must not read it as the former.
    pub fn erasure_positions(&self) -> Option<Vec<usize>> {
        self.trust.may_drive_repair().then(|| self.failed.clone())
    }

    /// Clear a chunk's failure after a successful refetch.
    pub fn retry(&mut self, idx: usize) {
        self.failed.retain(|&i| i != idx);
        self.staging.remove(&idx);
    }

    pub fn all_verified(&self) -> bool {
        self.verified.iter().all(|&v| v) && self.failed.is_empty()
    }

    pub fn verified_count(&self) -> usize {
        self.verified.iter().filter(|&&v| v).count()
    }
}

/// Build a manifest from a complete local file.
pub fn from_file(
    path: &str,
    chunk_size: u64,
    algo: ChunkAlgo,
    url: Option<String>,
    validator: Option<String>,
) -> std::io::Result<Manifest> {
    use std::io::Read as _;
    let mut f = std::fs::File::open(path)?;
    let size = f.metadata()?.len();
    let mut digests = Vec::new();
    let mut whole = sha2::Sha256::new();
    let mut buf = vec![0u8; chunk_size as usize];
    loop {
        let mut filled = 0usize;
        while filled < buf.len() {
            match f.read(&mut buf[filled..])? {
                0 => break,
                n => filled += n,
            }
        }
        if filled == 0 {
            break;
        }
        use sha2::Digest as _;
        whole.update(&buf[..filled]);
        digests.push(algo.hash(&buf[..filled]));
        if (filled as u64) < chunk_size {
            break;
        }
    }
    use sha2::Digest as _;
    Ok(Manifest {
        hydra_manifest: FORMAT_VERSION,
        object: ObjectMeta {
            size,
            chunk_size,
            digest: Some(format!(
                "sha256:{}",
                crate::digest::to_lower_hex(&whole.finalize())
            )),
            validator,
            url,
        },
        chunks: ChunkList { algo, digests },
        parity: None,
    })
}

/// Map an [`Algo`] to a manifest algorithm, rejecting the ones a manifest must
/// not use.
pub fn algo_for(a: Algo) -> Option<ChunkAlgo> {
    match a {
        Algo::Sha256 => Some(ChunkAlgo::Sha256),
        Algo::Sha512 => Some(ChunkAlgo::Sha512),
        // Detection only; see the `ChunkAlgo` note on why it is admitted at all.
        Algo::Sha1 => Some(ChunkAlgo::Sha1),
        Algo::Md5 | Algo::Crc32 | Algo::Crc32c => None,
    }
}

/// Build a chunk manifest from a Metalink `<file>` entry.
///
/// # Why this conversion is the point of the whole feature
///
/// `<pieces>` is a per-chunk digest list published by whoever built the object,
/// on a host that is usually not one of the mirrors. That is precisely the input
/// [`ChunkVerifier`] was written for, and it is the difference between "the
/// download is corrupt, start again" and "chunk 412 is corrupt, refetch 4 MiB
/// from a different mirror". On a multi-gigabyte image over a flaky mirror set
/// that is the difference between finishing and not.
///
/// The grid comes from the document and is NOT reconciled with
/// [`DEFAULT_CHUNK`]: a digest is a function of an exact byte span, so the only
/// grid that can be verified is the one the digests were computed over.
///
/// Fails rather than approximates when the document is internally inconsistent —
/// a piece list that does not tile the stated size describes a different object,
/// and applying it anyway reports every chunk as corrupt.
pub fn from_metalink(f: &crate::metalink::MetalinkFile) -> Result<Manifest, String> {
    let size = f
        .size
        .ok_or("the document states no <size>, so its pieces cannot be placed")?;
    let p = f
        .pieces
        .as_ref()
        .ok_or("the document publishes no <pieces>")?;
    let algo = algo_for(p.algo).ok_or_else(|| {
        format!(
            "<pieces type={:?}> is not an algorithm a manifest may be built on",
            p.algo.as_str()
        )
    })?;
    if !p.covers(size) {
        return Err(format!(
            "the document lists {} pieces of {} bytes, which does not tile a {size}-byte object ({} expected); these pieces describe a different file",
            p.hashes.len(),
            p.length,
            Manifest::chunk_count(size, p.length)
        ));
    }
    // A v3 document numbers its pieces with `piece="N"` and the reader sizes
    // the grid to the largest index it sees — so a document that SKIPS an index
    // yields a grid of the right length with empty strings in the holes. An
    // empty digest matches nothing: every affected chunk would "fail
    // verification", be refetched, and fail again, reporting mirror corruption
    // about a malformed document. Refusing here keeps the investigation pointed
    // at the document, and the whole-file digest still verifies the object.
    if let Some(gap) = p.hashes.iter().position(String::is_empty) {
        return Err(format!(
            "the document numbers its pieces but skips index {gap}; a grid with holes \
             cannot verify anything"
        ));
    }
    Ok(Manifest {
        hydra_manifest: FORMAT_VERSION,
        object: ObjectMeta {
            size,
            chunk_size: p.length,
            digest: f.best_hash().map(|h| h.spec()),
            // A Metalink says nothing about ETags, and inventing one here would
            // make `matches_validator` compare a fabricated value.
            validator: None,
            url: f.urls.first().map(|u| u.url.clone()),
        },
        chunks: ChunkList {
            algo,
            digests: p.hashes.clone(),
        },
        parity: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(n: usize, cs: u64, size: u64) -> Manifest {
        Manifest {
            hydra_manifest: FORMAT_VERSION,
            object: ObjectMeta {
                size,
                chunk_size: cs,
                digest: None,
                validator: None,
                url: None,
            },
            chunks: ChunkList {
                algo: ChunkAlgo::Blake3,
                digests: vec!["0".repeat(64); n],
            },
            parity: None,
        }
    }

    #[test]
    fn a_manifest_round_trips() {
        let m = obj(3, 1024, 3000);
        let back = Manifest::parse(&m.to_json()).expect("round trip");
        assert_eq!(back.chunks.digests.len(), 3);
        assert_eq!(back.object.size, 3000);
    }

    /// A chunk count that disagrees with size/grid describes a different object.
    #[test]
    fn a_chunk_count_that_disagrees_with_the_grid_is_refused() {
        let mut m = obj(3, 1024, 3000);
        m.chunks.digests.pop();
        let e = Manifest::parse(&m.to_json()).expect_err("must refuse");
        assert!(e.contains("different object"), "got: {e}");
    }

    #[test]
    fn an_unknown_format_version_is_refused_not_guessed_at() {
        let mut m = obj(1, 1024, 512);
        m.hydra_manifest = 99;
        assert!(Manifest::parse(&m.to_json()).is_err());
    }

    #[test]
    fn the_last_chunk_is_short_not_padded() {
        let m = obj(3, 1024, 2500);
        assert_eq!(m.span(0), (0, 1024));
        assert_eq!(m.span(2), (2048, 2500), "the tail chunk is 452 bytes");
    }

    fn real_manifest(data: &[u8], cs: u64) -> Manifest {
        let n = Manifest::chunk_count(data.len() as u64, cs);
        let mut digests = Vec::new();
        for i in 0..n {
            let lo = i * cs as usize;
            let hi = ((i + 1) * cs as usize).min(data.len());
            digests.push(ChunkAlgo::Blake3.hash(&data[lo..hi]));
        }
        Manifest {
            hydra_manifest: FORMAT_VERSION,
            object: ObjectMeta {
                size: data.len() as u64,
                chunk_size: cs,
                digest: None,
                validator: None,
                url: None,
            },
            chunks: ChunkList {
                algo: ChunkAlgo::Blake3,
                digests,
            },
            parity: None,
        }
    }

    #[test]
    fn clean_chunks_all_verify() {
        let data: Vec<u8> = (0..4096u32).map(|i| (i % 251) as u8).collect();
        let m = real_manifest(&data, 1024);
        let mut v = ChunkVerifier::new(m, Trust::Trusted);
        for c in data.chunks(256) {
            let off = (c.as_ptr() as usize - data.as_ptr() as usize) as u64;
            assert!(v.write(off, c).is_empty());
        }
        assert!(v.all_verified());
        assert_eq!(v.verified_count(), 4);
    }

    /// One flipped byte must localize to exactly one chunk — that localization
    /// is the entire point, since it turns a whole-file failure into a 1 KiB
    /// refetch.
    #[test]
    fn a_single_corrupt_byte_localizes_to_one_chunk() {
        let data: Vec<u8> = (0..4096u32).map(|i| (i % 251) as u8).collect();
        let m = real_manifest(&data, 1024);
        let mut bad = data.clone();
        bad[2000] ^= 0xff; // inside chunk 1
        let mut v = ChunkVerifier::new(m, Trust::Trusted);
        let mut failures = Vec::new();
        for (i, c) in bad.chunks(1024).enumerate() {
            failures.extend(v.write(i as u64 * 1024, c));
        }
        assert_eq!(failures, vec![1], "only chunk 1 is damaged");
        assert_eq!(v.failed_spans(), vec![(1024, 2048)]);
        assert!(!v.all_verified());
    }

    /// Out-of-order delivery is the normal case here, not an edge case.
    #[test]
    fn chunks_verify_regardless_of_arrival_order() {
        let data: Vec<u8> = (0..4096u32).map(|i| (i % 251) as u8).collect();
        let m = real_manifest(&data, 1024);
        let mut v = ChunkVerifier::new(m, Trust::Trusted);
        for i in [3usize, 1, 0, 2] {
            let (lo, hi) = (i * 1024, (i + 1) * 1024);
            assert!(v.write(lo as u64, &data[lo..hi]).is_empty());
        }
        assert!(v.all_verified());
    }

    /// A write spanning a chunk boundary must be split across both.
    #[test]
    fn a_write_crossing_a_boundary_is_attributed_to_both_chunks() {
        let data: Vec<u8> = (0..2048u32).map(|i| (i % 251) as u8).collect();
        let m = real_manifest(&data, 1024);
        let mut v = ChunkVerifier::new(m, Trust::Trusted);
        v.write(0, &data[..512]);
        v.write(512, &data[512..1536]); // straddles the 1024 boundary
        v.write(1536, &data[1536..]);
        assert!(v.all_verified());
    }

    /// The rule that keeps an integrity feature from becoming a corruption
    /// feature: an advertised manifest may detect, but may not drive a repair.
    #[test]
    fn an_advertised_manifest_never_licenses_a_repair() {
        let data: Vec<u8> = (0..2048u32).map(|i| (i % 251) as u8).collect();
        let m = real_manifest(&data, 1024);
        let mut bad = data.clone();
        bad[100] ^= 0xff;
        let mut v = ChunkVerifier::new(m, Trust::Advertised);
        for i in 0..2 {
            v.write(
                i * 1024,
                &bad[(i * 1024) as usize..((i + 1) * 1024) as usize],
            );
        }
        assert_eq!(v.failed_indices(), &[0], "detection still works");
        assert_eq!(
            v.erasure_positions(),
            None,
            "an advertised manifest must not supply erasure positions"
        );
    }

    #[test]
    fn a_trusted_manifest_supplies_erasure_positions() {
        let data: Vec<u8> = (0..2048u32).map(|i| (i % 251) as u8).collect();
        let m = real_manifest(&data, 1024);
        let mut bad = data.clone();
        bad[100] ^= 0xff;
        let mut v = ChunkVerifier::new(m, Trust::Trusted);
        for i in 0..2 {
            v.write(
                i * 1024,
                &bad[(i * 1024) as usize..((i + 1) * 1024) as usize],
            );
        }
        assert_eq!(v.erasure_positions(), Some(vec![0]));
    }

    #[test]
    fn a_refetched_chunk_clears_its_failure() {
        let data: Vec<u8> = (0..2048u32).map(|i| (i % 251) as u8).collect();
        let m = real_manifest(&data, 1024);
        let mut bad = data.clone();
        bad[100] ^= 0xff;
        let mut v = ChunkVerifier::new(m, Trust::Trusted);
        v.write(0, &bad[..1024]);
        v.write(1024, &data[1024..]);
        assert_eq!(v.failed_indices(), &[0]);
        v.retry(0);
        v.write(0, &data[..1024]); // clean this time
        assert!(v.all_verified(), "a good refetch must clear the failure");
    }

    #[test]
    fn a_stale_validator_is_detectable_without_being_fatal() {
        let mut m = obj(1, 1024, 512);
        m.object.validator = Some("\"v1\"".into());
        assert!(!m.matches_validator(Some("\"v2\"")));
        assert!(m.matches_validator(Some("\"v1\"")));
        assert!(m.matches_validator(None), "absence is not disagreement");
    }

    #[test]
    fn every_algorithm_produces_stable_hex_of_its_own_width() {
        for (a, width) in [
            (ChunkAlgo::Blake3, 64),
            (ChunkAlgo::Sha256, 64),
            (ChunkAlgo::Sha512, 128),
            (ChunkAlgo::Sha1, 40),
        ] {
            let h = a.hash(b"hydra");
            assert_eq!(h.len(), width, "{} hex width", a.as_str());
            assert_eq!(h, a.hash(b"hydra"), "must be deterministic");
            assert!(h.bytes().all(|b| b.is_ascii_hexdigit()));
        }
        assert_ne!(ChunkAlgo::Blake3.hash(b"x"), ChunkAlgo::Sha256.hash(b"x"));
        // The well-known vector, so a wrong wiring of the sha1 crate is caught
        // here rather than as a chunk mismatch against a real mirror.
        assert_eq!(
            ChunkAlgo::Sha1.hash(b"abc"),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
    }

    #[test]
    fn crc_and_md5_are_not_offered_for_manifests() {
        assert!(algo_for(Algo::Crc32).is_none());
        assert!(algo_for(Algo::Crc32c).is_none());
        assert!(algo_for(Algo::Md5).is_none());
        assert_eq!(algo_for(Algo::Sha256), Some(ChunkAlgo::Sha256));
        assert_eq!(algo_for(Algo::Sha512), Some(ChunkAlgo::Sha512));
        assert_eq!(algo_for(Algo::Sha1), Some(ChunkAlgo::Sha1));
    }

    #[test]
    fn a_sha1_manifest_can_detect_but_never_drive_a_parity_repair() {
        // The Metalink 3.0 case: the document is a local file the user handed
        // over, so its provenance is Trusted, but SHA-1 collisions are a
        // purchased commodity and an erasure decode trusts its positions
        // absolutely. Detection and targeted refetch stay; repair does not.
        let mut m = obj(2, 4, 8);
        m.chunks.algo = ChunkAlgo::Sha1;
        m.chunks.digests = vec![ChunkAlgo::Sha1.hash(b"aaaa"), ChunkAlgo::Sha1.hash(b"bbbb")];
        let v = ChunkVerifier::new(m, Trust::Trusted);
        assert_eq!(v.trust(), Trust::Advertised);
        assert!(!v.trust().may_drive_repair());

        // A collision-resistant algorithm keeps the trust it was given.
        let mut m2 = obj(1, 4, 4);
        m2.chunks.digests = vec![ChunkAlgo::Blake3.hash(b"aaaa")];
        assert_eq!(
            ChunkVerifier::new(m2, Trust::Trusted).trust(),
            Trust::Trusted
        );
    }

    #[test]
    fn metalink_pieces_become_a_manifest_the_verifier_already_understands() {
        use crate::metalink;
        let src = r#"<metalink xmlns="urn:ietf:params:xml:ns:metalink"><file name="f">
            <size>10</size>
            <hash type="sha-256">d201bd1eeb17086cd3aaf82b156810a5ba3f389e10b4472c9b2c7182f771a9ef</hash>
            <pieces length="4" type="sha-1">
              <hash>1111111111111111111111111111111111111111</hash>
              <hash>2222222222222222222222222222222222222222</hash>
              <hash>3333333333333333333333333333333333333333</hash>
            </pieces>
            <url priority="1">https://a.example/f</url>
          </file></metalink>"#;
        let ml = metalink::parse(src).unwrap();
        let m = from_metalink(&ml.files[0]).unwrap();
        // The grid is the DOCUMENT's, not DEFAULT_CHUNK: a digest is a function
        // of an exact span, so any other grid verifies nothing.
        assert_eq!(m.object.chunk_size, 4);
        assert_eq!(m.object.size, 10);
        assert_eq!(m.chunks.algo, ChunkAlgo::Sha1);
        assert_eq!(m.chunks.digests.len(), 3);
        assert_eq!(
            m.object.digest.as_deref(),
            Some("sha256:d201bd1eeb17086cd3aaf82b156810a5ba3f389e10b4472c9b2c7182f771a9ef")
        );
        assert_eq!(m.object.url.as_deref(), Some("https://a.example/f"));
        // And it satisfies the same structural check a manifest read off disk does.
        Manifest::parse(&m.to_json()).expect("must round-trip through the on-disk form");
    }

    #[test]
    fn a_document_whose_pieces_do_not_tile_its_size_is_refused_with_the_reason() {
        use crate::metalink;
        // Reporting "471 chunks are corrupt" about a mismatched document sends
        // the investigation to the mirrors instead of to the document.
        let src = r#"<metalink xmlns="urn:ietf:params:xml:ns:metalink"><file name="f">
            <size>100</size>
            <pieces length="4" type="sha-1">
              <hash>1111111111111111111111111111111111111111</hash>
            </pieces></file></metalink>"#;
        let ml = metalink::parse(src).unwrap();
        let e = from_metalink(&ml.files[0]).unwrap_err();
        assert!(e.contains("describe a different file"), "{e}");

        // No pieces, and no size, each say so rather than producing an empty grid.
        let bare = r#"<metalink xmlns="urn:ietf:params:xml:ns:metalink"><file name="f">
            <size>100</size><url>https://a/f</url></file></metalink>"#;
        let ml = metalink::parse(bare).unwrap();
        assert!(from_metalink(&ml.files[0])
            .unwrap_err()
            .contains("no <pieces>"));
    }

    #[test]
    fn a_v3_piece_grid_with_a_skipped_index_is_refused_not_verified_against() {
        use crate::metalink;
        // `piece="0"` and `piece="2"`, nothing at 1: the reader sizes the grid
        // to fit and index 1 is an empty string. The COUNT is right, so
        // `covers` alone would admit it — and an empty digest matches nothing,
        // so every hole would present as a corrupt chunk refetched forever.
        let src = r#"<metalink version="3.0" xmlns="http://www.metalinker.org/"><files>
          <file name="f"><size>12</size>
            <verification><pieces length="4" type="sha1">
              <hash piece="0">1111111111111111111111111111111111111111</hash>
              <hash piece="2">3333333333333333333333333333333333333333</hash>
            </pieces></verification>
            <resources><url>https://a/f</url></resources></file></files></metalink>"#;
        let ml = metalink::parse(src).unwrap();
        let e = from_metalink(&ml.files[0]).unwrap_err();
        assert!(e.contains("skips index 1"), "{e}");
    }
}
