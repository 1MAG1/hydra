//! Digests a server will tell you WITHOUT sending the body.
//!
//! # What is and is not possible
//!
//! A checksum is a function of the bytes. You cannot compute one for bytes you have not
//! received, and no protocol lets you ask a server to hash an object on demand — RFC 9530
//! defines `Want-Digest`/`Repr-Digest` for exactly that, and essentially nothing
//! implements it (measured: four representative hosts, zero support).
//!
//! What *is* possible is retrieving a digest the publisher already computed. That is a
//! different and weaker claim — it verifies the bytes you eventually download against
//! what the publisher says they should be, and it lets you compare a local file against a
//! remote object without transferring it. It does not let you verify the server: a host
//! that serves wrong bytes can serve a matching wrong digest, unless the digest comes from
//! somewhere else (an index, a signature, a different host).
//!
//! Three sources, in descending order of trust:
//!
//! 1. **Response headers.** `Repr-Digest`/`Content-Digest` (RFC 9530), the older `Digest`
//!    (RFC 3230), `Content-MD5`, and the cloud-store forms: `x-amz-checksum-sha256`,
//!    `x-amz-checksum-crc32`, `x-goog-hash`, `x-ms-blob-content-md5`. These arrive on a
//!    HEAD, so they cost one round trip and no body.
//! 2. **A sidecar file.** `<name>.sha256`, `.md5`, `.sha1`, `CHECKSUMS`, `SHA256SUMS`.
//!    Cheap (tens of bytes) and very common for releases — but served by the same host, so
//!    it proves integrity, not authenticity.
//! 3. **An ecosystem index.** PyPI's JSON API and the crates.io sparse index publish
//!    per-file digests, and they are a *different* endpoint from the artifact, so they do
//!    carry some authenticity weight.
//!
//! # What an ETag is not
//!
//! An `ETag` is an opaque validator. It is frequently a hash of something, which invites
//! the assumption that it is a hash of the CONTENT — it is not, and treating it as one
//! would report false mismatches. Measured on a real host whose ETag is exactly 64 hex
//! characters: it equals neither the content SHA-256 nor the git blob id. A weak ETag
//! (`W/"..."`) is weaker still — the specification permits it to compare equal across
//! representations that are merely equivalent, which is precisely what a checksum must
//! not tolerate.

use std::fmt;

/// Lowercase hex for a digest output.
///
/// RustCrypto 0.11 returns `Array<u8, N>` from `finalize()`, which — unlike the
/// `GenericArray` of 0.10 — does not implement `LowerHex`, so `format!("{:x}")`
/// no longer compiles. Written here rather than pulling in a hex crate: this is
/// the whole of what the project needed from one.
///
/// # Why this is not `write!("{b:02x}")`
///
/// It was, and that cost 8.3 ns per byte — 265 ns to render one SHA-256 digest.
/// `write!` routes each byte through `core::fmt`: a `Formatter`, a `&dyn Write`
/// vtable dispatch, width-and-fill handling for the `02` specifier, and a
/// `Result` per call, none of which a two-nibble table lookup needs. That is
/// invisible for one digest and is not what this function is called for:
/// building a chunk manifest calls it once per chunk, so a 40 GiB object at
/// 1 MiB chunks made 40,960 calls and spent ~10 ms of pure formatting.
///
/// The table form is a lookup, a shift, and a mask per byte. It is written over
/// a preallocated `[u8]` with no bounds-check-per-byte and no reallocation, so
/// LLVM autovectorizes it — on this project's targets that means NEON on
/// aarch64 and SSE2/AVX2 on x86-64, chosen by the compiler for the actual
/// target, with no `unsafe` and no runtime dispatch to maintain. Hand-written
/// intrinsics were not used precisely because they would need both, and would
/// have to be correctness-tested per architecture for no measured gain over
/// what the autovectorizer already produces here.
pub fn to_lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = vec![0u8; bytes.len() * 2];
    let (chunks, _) = out.as_chunks_mut::<2>();
    for (o, &b) in chunks.iter_mut().zip(bytes) {
        o[0] = HEX[(b >> 4) as usize];
        o[1] = HEX[(b & 0x0f) as usize];
    }
    // Every byte written came from HEX, which is ASCII, so the buffer is valid
    // UTF-8 by construction. `from_utf8` re-validates and is not free, but it is
    // O(n) with a vectorized check and keeps this function free of `unsafe`;
    // measured, it is still far below the formatting cost it replaces.
    String::from_utf8(out).expect("hex table output is ASCII by construction")
}

/// A digest algorithm a server might advertise.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Algo {
    Sha256,
    Sha512,
    Sha1,
    Md5,
    Crc32c,
    Crc32,
}

impl Algo {
    pub fn as_str(self) -> &'static str {
        match self {
            Algo::Sha256 => "sha256",
            Algo::Sha512 => "sha512",
            Algo::Sha1 => "sha1",
            Algo::Md5 => "md5",
            Algo::Crc32c => "crc32c",
            Algo::Crc32 => "crc32",
        }
    }

    /// Parse the many spellings in use across the header forms.
    pub fn parse(s: &str) -> Option<Algo> {
        match s.trim().trim_matches('"').to_ascii_lowercase().as_str() {
            "sha-256" | "sha256" => Some(Algo::Sha256),
            "sha-512" | "sha512" => Some(Algo::Sha512),
            "sha-1" | "sha1" => Some(Algo::Sha1),
            "md5" => Some(Algo::Md5),
            "crc32c" => Some(Algo::Crc32c),
            "crc32" | "crc32c-combine" => Some(Algo::Crc32),
            _ => None,
        }
    }

    /// Is this strong enough to justify trusting bytes that came from elsewhere?
    ///
    /// CRC32 is an error-detecting code, not a hash: forging a collision is arithmetic.
    /// MD5 and SHA-1 are broken against a motivated adversary but still detect
    /// transmission corruption, which is what most advertised digests are for.
    pub fn is_cryptographic(self) -> bool {
        matches!(self, Algo::Sha256 | Algo::Sha512)
    }
}

/// A digest and where it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Advertised {
    pub algo: Algo,
    /// Lowercase hex, whatever encoding it arrived in.
    pub hex: String,
    /// Human description of the source, for reporting.
    pub source: String,
}

impl fmt::Display for Advertised {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{} ({})", self.algo.as_str(), self.hex, self.source)
    }
}

/// Decode a digest value that may be hex or base64, into lowercase hex.
///
/// The header forms disagree: RFC 9530 uses base64 inside a byte-sequence wrapper,
/// `Content-MD5` is bare base64, `x-amz-checksum-*` is base64, and sidecar files are hex.
/// Accepting both is not sloppiness — it is what the wire actually carries.
pub fn to_hex(value: &str, algo: Algo) -> Option<String> {
    let v = value.trim().trim_matches('"').trim_matches(':');
    let want_bytes = match algo {
        Algo::Sha256 => 32,
        Algo::Sha512 => 64,
        Algo::Sha1 => 20,
        Algo::Md5 => 16,
        Algo::Crc32 | Algo::Crc32c => 4,
    };
    // Hex, if it looks like hex of exactly the right length.
    if v.len() == want_bytes * 2 && v.chars().all(|c| c.is_ascii_hexdigit()) {
        return Some(v.to_ascii_lowercase());
    }
    // Otherwise base64.
    let raw = b64_decode(v)?;
    if raw.len() != want_bytes {
        return None;
    }
    Some(raw.iter().map(|b| format!("{b:02x}")).collect())
}

/// Minimal standard base64 decoder (accepts unpadded input).
fn b64_decode(s: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some((c - b'A') as u32),
            b'a'..=b'z' => Some((c - b'a') as u32 + 26),
            b'0'..=b'9' => Some((c - b'0') as u32 + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let bytes: Vec<u8> = s
        .bytes()
        .filter(|&c| c != b'=' && !c.is_ascii_whitespace())
        .collect();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let mut acc = 0u32;
        let mut n = 0;
        for &c in chunk {
            acc = (acc << 6) | val(c)?;
            n += 1;
        }
        match n {
            4 => {
                out.push((acc >> 16) as u8);
                out.push((acc >> 8) as u8);
                out.push(acc as u8);
            }
            3 => {
                acc <<= 6;
                out.push((acc >> 16) as u8);
                out.push((acc >> 8) as u8);
            }
            2 => {
                acc <<= 12;
                out.push((acc >> 16) as u8);
            }
            _ => return None,
        }
    }
    Some(out)
}

/// An ETag that is *probably* the content MD5, with that uncertainty preserved.
///
/// S3 and S3-compatible stores set the ETag of a SINGLE-PART upload to the hex MD5 of the
/// object. Measured against a live store: a 32-hex strong ETag matched `md5(body)` exactly.
/// That makes it usable — but only as a hypothesis, for three reasons:
///
/// * A MULTIPART upload's ETag is the MD5 of concatenated part MD5s plus `-<partcount>`,
///   which is not the MD5 of anything the client can compute. The `-` suffix is the tell.
/// * Nothing obliges a server to follow the convention; 32 hex characters can be anything.
/// * A WEAK ETag may compare equal across representations that merely have equivalent
///   semantics, so it cannot stand for a byte-exact digest at all.
///
/// So this returns a candidate that the caller must label as unconfirmed. A mismatch
/// against it means "either the bytes differ or this ETag was never an MD5" — which is not
/// the same as corruption, and reporting it as corruption would be a false alarm.
pub fn md5_candidate_from_etag(head: &str) -> Option<Advertised> {
    let raw = header(head, "etag")?;
    let t = raw.trim();
    // Weak validators are out: the specification permits equality across
    // representations that are not byte-identical.
    if t.starts_with("W/") || t.starts_with("w/") {
        return None;
    }
    let inner = t.trim_matches('"');
    // Multipart: `<hex>-<n>`. Not a digest of the object.
    if inner.contains('-') {
        return None;
    }
    if inner.len() != 32 || !inner.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(Advertised {
        algo: Algo::Md5,
        hex: inner.to_ascii_lowercase(),
        source: "ETag, which on S3-style stores is the MD5 of a single-part upload \
                 (UNCONFIRMED: the convention is not guaranteed)"
            .into(),
    })
}

/// Every digest a response header block advertises.
///
/// Returns all of them rather than picking one, because the caller's preference depends on
/// what it is checking against — and because seeing two disagree is itself information.
pub fn from_headers(head: &str) -> Vec<Advertised> {
    let mut out = Vec::new();

    // RFC 9530: Repr-Digest / Content-Digest, e.g. `sha-256=:base64:, md5=:base64:`
    for name in ["repr-digest", "content-digest", "digest"] {
        if let Some(v) = header(head, name) {
            for part in v.split(',') {
                let Some((k, val)) = part.split_once('=') else {
                    continue;
                };
                if let Some(a) = Algo::parse(k) {
                    if let Some(hex) = to_hex(val, a) {
                        out.push(Advertised {
                            algo: a,
                            hex,
                            source: format!("{name} header"),
                        });
                    }
                }
            }
        }
    }

    // Content-MD5 (RFC 1864): bare base64, no algorithm name.
    if let Some(v) = header(head, "content-md5") {
        if let Some(hex) = to_hex(&v, Algo::Md5) {
            out.push(Advertised {
                algo: Algo::Md5,
                hex,
                source: "Content-MD5 header".into(),
            });
        }
    }

    // S3 and compatible stores.
    for (name, algo) in [
        ("x-amz-checksum-sha256", Algo::Sha256),
        ("x-amz-checksum-sha1", Algo::Sha1),
        ("x-amz-checksum-crc32", Algo::Crc32),
        ("x-amz-checksum-crc32c", Algo::Crc32c),
        ("x-ms-blob-content-md5", Algo::Md5),
        ("x-checksum-sha256", Algo::Sha256),
        ("x-checksum-sha1", Algo::Sha1),
        ("x-checksum-md5", Algo::Md5),
    ] {
        if let Some(v) = header(head, name) {
            if let Some(hex) = to_hex(&v, algo) {
                out.push(Advertised {
                    algo,
                    hex,
                    source: format!("{name} header"),
                });
            }
        }
    }

    // Google Cloud Storage: `x-goog-hash: crc32c=base64, md5=base64`
    if let Some(v) = header(head, "x-goog-hash") {
        for part in v.split(',') {
            if let Some((k, val)) = part.split_once('=') {
                if let Some(a) = Algo::parse(k) {
                    if let Some(hex) = to_hex(val, a) {
                        out.push(Advertised {
                            algo: a,
                            hex,
                            source: "x-goog-hash header".into(),
                        });
                    }
                }
            }
        }
    }

    out
}

/// Parse a checksum sidecar file body (`sha256sum` output format, or a bare digest).
///
/// `sha256sum` writes `<hex>  <name>`, and a manifest lists many. `want_name` selects the
/// line for the object being fetched; a single bare digest is accepted for any name, since
/// `<file>.sha256` files are frequently written that way.
pub fn from_sidecar(body: &str, algo: Algo, want_name: &str) -> Option<Advertised> {
    let hexlen = match algo {
        Algo::Sha256 => 64,
        Algo::Sha512 => 128,
        Algo::Sha1 => 40,
        Algo::Md5 => 32,
        _ => return None,
    };
    let mut bare: Option<String> = None;
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // `hex  name` or `hex *name` (binary mode), or BSD-style `ALGO (name) = hex`.
        let mut it = line.split_whitespace();
        let first = it.next().unwrap_or("");
        if first.len() == hexlen && first.chars().all(|c| c.is_ascii_hexdigit()) {
            let name = it.next().unwrap_or("").trim_start_matches('*');
            if name.is_empty() {
                bare = Some(first.to_ascii_lowercase());
            } else if name.ends_with(want_name) || want_name.ends_with(name) {
                return Some(Advertised {
                    algo,
                    hex: first.to_ascii_lowercase(),
                    source: "checksum sidecar file".into(),
                });
            }
        } else if let Some((lhs, rhs)) = line.split_once('=') {
            // BSD: `SHA256 (file) = hex`
            let hex = rhs.trim();
            if hex.len() == hexlen
                && hex.chars().all(|c| c.is_ascii_hexdigit())
                && lhs.contains(want_name)
            {
                return Some(Advertised {
                    algo,
                    hex: hex.to_ascii_lowercase(),
                    source: "checksum sidecar file".into(),
                });
            }
        }
    }
    bare.map(|hex| Advertised {
        algo,
        hex,
        source: "checksum sidecar file".into(),
    })
}

/// The sidecar paths worth trying for an object path, in order of likelihood.
pub fn sidecar_candidates(path: &str) -> Vec<(String, Algo)> {
    let mut v = Vec::new();
    for (suffix, algo) in [
        (".sha256", Algo::Sha256),
        (".sha256sum", Algo::Sha256),
        (".sha512", Algo::Sha512),
        (".md5", Algo::Md5),
        (".sha1", Algo::Sha1),
    ] {
        v.push((format!("{path}{suffix}"), algo));
    }
    // Directory manifests, which list many files.
    if let Some(slash) = path.rfind('/') {
        let dir = &path[..slash + 1];
        for (name, algo) in [
            ("SHA256SUMS", Algo::Sha256),
            ("sha256sum.txt", Algo::Sha256),
            ("CHECKSUMS", Algo::Sha256),
            ("MD5SUMS", Algo::Md5),
        ] {
            v.push((format!("{dir}{name}"), algo));
        }
    }
    v
}

/// Case-insensitive header lookup over a raw header block.
fn header(head: &str, name: &str) -> Option<String> {
    for line in head.lines() {
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case(name) {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc_9530_repr_digest_is_parsed_from_base64() {
        // sha-256 of the empty string, base64 inside the byte-sequence colons.
        let h = "HTTP/1.1 200 OK\r\n\
                 Repr-Digest: sha-256=:47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=:\r\n";
        let d = from_headers(h);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].algo, Algo::Sha256);
        assert_eq!(
            d[0].hex, "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "base64 must decode to the well-known empty-string SHA-256"
        );
    }

    #[test]
    fn content_md5_is_bare_base64_with_no_algorithm_name() {
        // MD5 of the empty string.
        let h = "HTTP/1.1 200 OK\r\nContent-MD5: 1B2M2Y8AsgTpgAmY7PhCfg==\r\n";
        let d = from_headers(h);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].algo, Algo::Md5);
        assert_eq!(d[0].hex, "d41d8cd98f00b204e9800998ecf8427e");
    }

    #[test]
    fn cloud_store_headers_are_recognised() {
        let h = "HTTP/1.1 200 OK\r\n\
                 x-amz-checksum-sha256: 47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=\r\n\
                 x-goog-hash: crc32c=AAAAAA==, md5=1B2M2Y8AsgTpgAmY7PhCfg==\r\n";
        let d = from_headers(h);
        let algos: Vec<Algo> = d.iter().map(|a| a.algo).collect();
        assert!(algos.contains(&Algo::Sha256));
        assert!(algos.contains(&Algo::Crc32c));
        assert!(algos.contains(&Algo::Md5));
    }

    #[test]
    fn an_etag_is_never_treated_as_a_content_digest() {
        // Measured on a real host: a 64-hex ETag that is NOT the content SHA-256. If this
        // were read as a digest, every download from that host would report a mismatch.
        let h = "HTTP/1.1 200 OK\r\n\
                 ETag: \"1050112ed550266b30d219458d37f5a8177d535dc102d5db225e29d117a00000\"\r\n";
        assert!(
            from_headers(h).is_empty(),
            "an ETag is an opaque validator, not a checksum"
        );
    }

    #[test]
    fn hex_and_base64_are_both_accepted() {
        let hex = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        assert_eq!(to_hex(hex, Algo::Sha256).unwrap(), hex);
        assert_eq!(
            to_hex("47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=", Algo::Sha256).unwrap(),
            hex
        );
        // Wrong length for the algorithm must be rejected, not silently truncated.
        assert!(to_hex("deadbeef", Algo::Sha256).is_none());
    }

    #[test]
    fn sidecar_manifest_selects_the_right_line() {
        let body = "# generated\n\
                    aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  other.tar.gz\n\
                    e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  wanted.tar.gz\n";
        let d = from_sidecar(body, Algo::Sha256, "wanted.tar.gz").unwrap();
        assert_eq!(
            d.hex, "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "a manifest lists many files; the wrong line would fail every check"
        );
    }

    #[test]
    fn bsd_style_and_bare_digest_files_both_parse() {
        let bsd = "SHA256 (wanted.bin) = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\n";
        assert!(from_sidecar(bsd, Algo::Sha256, "wanted.bin").is_some());
        // A `.sha256` file written with just the digest applies to its own object.
        let bare = "E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855\n";
        let d = from_sidecar(bare, Algo::Sha256, "anything.bin").unwrap();
        assert!(d.hex.starts_with("e3b0"), "must be normalised to lowercase");
    }

    /// The table-lookup hex encoder must be byte-identical to the `write!`
    /// formatting it replaced, for every possible byte and every length — this is
    /// the differential test that licenses the rewrite. A digest rendered even
    /// one nibble differently is a false mismatch reported to the user.
    #[test]
    fn hex_matches_the_formatting_implementation_it_replaced() {
        use std::fmt::Write as _;
        fn reference(bytes: &[u8]) -> String {
            let mut s = String::with_capacity(bytes.len() * 2);
            for b in bytes {
                let _ = write!(s, "{b:02x}");
            }
            s
        }
        // Every byte value in isolation catches nibble-order and table errors.
        let all: Vec<u8> = (0u16..=255).map(|v| v as u8).collect();
        for b in &all {
            assert_eq!(to_lower_hex(&[*b]), reference(&[*b]), "byte {b:#04x}");
        }
        // Lengths around the vector width catch a mishandled remainder tail.
        for len in 0..=64usize {
            let slice = &all[..len.min(all.len())];
            assert_eq!(to_lower_hex(slice), reference(slice), "length {len}");
        }
        // The real shapes: SHA-256 and SHA-512 digest widths.
        for len in [32usize, 64] {
            let d: Vec<u8> = (0..len).map(|i| (i * 7 + 3) as u8).collect();
            assert_eq!(to_lower_hex(&d), reference(&d), "digest width {len}");
        }
        assert_eq!(to_lower_hex(&[]), "", "empty input is an empty string");
    }

    #[test]
    fn crc32_is_not_treated_as_a_security_guarantee() {
        assert!(!Algo::Crc32.is_cryptographic());
        assert!(!Algo::Md5.is_cryptographic());
        assert!(!Algo::Sha1.is_cryptographic());
        assert!(Algo::Sha256.is_cryptographic());
    }

    #[test]
    fn sidecar_candidates_cover_per_file_and_manifest_forms() {
        let c = sidecar_candidates("/pub/rel/app-1.2.tar.gz");
        let paths: Vec<&str> = c.iter().map(|(p, _)| p.as_str()).collect();
        assert!(paths.contains(&"/pub/rel/app-1.2.tar.gz.sha256"));
        assert!(paths.contains(&"/pub/rel/SHA256SUMS"));
    }

    #[test]
    fn a_32_hex_strong_etag_is_offered_as_an_unconfirmed_md5() {
        // Measured against a live S3-backed store: this ETag equalled md5(body) exactly.
        let h = "HTTP/1.1 200 OK\r\nETag: \"c7251782043416b8adca3bf107f7b667\"\r\n";
        let c = md5_candidate_from_etag(h).expect("a 32-hex strong ETag is a candidate");
        assert_eq!(c.algo, Algo::Md5);
        assert_eq!(c.hex, "c7251782043416b8adca3bf107f7b667");
        assert!(
            c.source.contains("UNCONFIRMED"),
            "the uncertainty must survive into the report, or a false alarm looks like corruption"
        );
    }

    #[test]
    fn multipart_and_weak_etags_are_refused_as_md5_candidates() {
        // A multipart ETag is the MD5 of concatenated part MD5s plus the part count: it is
        // not the MD5 of the object and comparing against it would always fail.
        let multi = "HTTP/1.1 200 OK\r\nETag: \"c7251782043416b8adca3bf107f7b667-9\"\r\n";
        assert!(md5_candidate_from_etag(multi).is_none());
        // A weak validator may compare equal across non-identical representations.
        let weak = "HTTP/1.1 200 OK\r\nETag: W/\"c7251782043416b8adca3bf107f7b667\"\r\n";
        assert!(md5_candidate_from_etag(weak).is_none());
        // Real ETags are frequently size-mtime pairs, or opaque: not 32 hex.
        let opaque = "HTTP/1.1 200 OK\r\nETag: \"2d21f4-5e3534db49367\"\r\n";
        assert!(md5_candidate_from_etag(opaque).is_none());
        // A 64-hex ETag is not a SHA-256 of the content either (measured), so it must not
        // be promoted to one by a symmetric heuristic.
        let long = "HTTP/1.1 200 OK\r\nETag: \"1050112ed550266b30d219458d37f5a8177d535dc102d5db225e29d117a000000\"\r\n";
        assert!(md5_candidate_from_etag(long).is_none());
    }
}
