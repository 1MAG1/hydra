//! The download engine the CLI drives: probe, plan, transfer, verify.
//!
//! This is where the theory meets a user's file. The scheduler decides *which
//! bytes go where*; this module decides everything around that — how many
//! connections politeness permits, whether a partial file can be resumed, what
//! the sidecar records, and whether the delivered bytes are the bytes asked for.

use crate::progress::{ConnView, Counters, Progress};
use crate::url::{proxy_from_env, Sidecar, Url};
use hya_core::{detect_format, Admission, Admit, Category, DeltaEstimator, Scheduler, Source};
use hya_net::polite::{Politeness, RateLimiter};
use hya_net::{fetch_range_retry, probe, probe_via_get, SparseSink, Target, TlsCapableConnector};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

/// A requested byte range, kept symbolic until the object size is known.
///
/// `Suffix` cannot be resolved at parse time — "the last 512 bytes" depends on
/// the size, which only the probe reveals — so it stays an explicit variant
/// rather than a sentinel value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeSpec {
    /// `lo-hi` inclusive, as HTTP spells ranges.
    Closed(u64, u64),
    /// `lo-`: from an offset to the end.
    From(u64),
    /// `-n`: the final n bytes.
    Suffix(u64),
}

impl RangeSpec {
    /// Resolve against a known object size into a half-open `[lo, hi)`.
    pub fn resolve(self, size: u64) -> Option<(u64, u64)> {
        let (lo, hi) = match self {
            RangeSpec::Closed(lo, hi) => (lo.min(size), (hi.saturating_add(1)).min(size)),
            RangeSpec::From(lo) => (lo.min(size), size),
            RangeSpec::Suffix(n) => (size.saturating_sub(n), size),
        };
        if hi > lo {
            Some((lo, hi))
        } else {
            None
        }
    }
}

/// Everything the engine needs for one transfer.
/// Live progress from a running transfer.
///
/// The queue manager needs this because a transfer is opaque otherwise: `run()` only
/// returns when it is finished, so a UI driving several jobs had nothing to show until
/// each one completed — every row sat at `?` for the whole download. Per-connection
/// detail rides along because that is the state that makes a multi-source transfer
/// debuggable (which mirror is slow, which range is stuck).
#[derive(Clone, Debug)]
pub struct Tick {
    pub id: u64,
    pub done: u64,
    pub size: Option<u64>,
    pub rate: f64,
    pub requests: u64,
    pub repairs: u64,
    pub conns: Vec<ConnLine>,
}

/// One connection's state, flattened for display.
#[derive(Clone, Debug)]
pub struct ConnLine {
    pub host: String,
    pub lo: u64,
    pub hi: u64,
    pub pos: u64,
    pub rate: f64,
    pub health: String,
}

#[derive(Clone)]
pub struct Job {
    /// Where to send live progress, and the id to tag it with.
    pub ticks: Option<(u64, tokio::sync::mpsc::UnboundedSender<Tick>)>,
    pub urls: Vec<String>,
    pub output: Option<PathBuf>,
    pub conns: Option<usize>,
    pub resume: bool,
    pub limit_rate: u64,
    /// Redirect hops permitted before giving up. `0` refuses to follow any.
    pub max_redirs: u32,
    /// `-4` / `-6`: restrict every connection to one IP version.
    pub ip_family: hya_net::IpFamily,
    /// `--show-error`: print failure reasons to stderr even under `-q`.
    pub show_error: bool,
    /// `--logfile` (truncate) or `--logfile-append`: human output goes to this
    /// file instead of the terminal. `bool` is the append flag.
    pub logfile: Option<(PathBuf, bool)>,
    pub tries: u32,
    pub timeout_s: f64,
    pub checksum: Option<String>,
    pub headers: Vec<String>,
    pub user_agent: String,
    pub verbose: u8,
    pub quiet: bool,
    pub no_progress: bool,
    pub polite: Politeness,
    /// Probe concurrency instead of taking `conns` on faith.
    /// Measure the useful connection count instead of taking a number on faith.
    ///
    /// Only consulted when `conns` is None: an explicit `-x N` is honoured as
    /// given, because a flag whose documented behaviour is "omit to measure" must
    /// not measure when it was not omitted.
    #[allow(dead_code)]
    pub adaptive: bool,
    /// Write to stdout rather than a file.
    pub to_stdout: bool,
    /// Skip entirely if the output already exists.
    pub no_clobber: bool,
    /// Create the output directory if missing.
    pub create_dirs: bool,
    /// Directory to place the output in.
    pub output_dir: Option<PathBuf>,
    /// Probe only: report what the server says and do not fetch the body.
    pub spider: bool,
    /// Print response headers.
    pub server_response: bool,
    /// Retrieve only this byte range.
    pub range: Option<RangeSpec>,
    /// Refuse an object larger than this.
    pub max_filesize: Option<u64>,
    /// Explicit proxy, overriding the environment.
    pub proxy: Option<String>,
    /// Ignore any proxy in the environment.
    pub no_proxy: bool,
    /// Set the local mtime from the server.
    pub remote_time: bool,
    /// Write the object's ETag here.
    pub etag_save: Option<PathBuf>,
    /// Skip if the stored ETag still matches.
    pub etag_compare: Option<PathBuf>,
    /// Sort the output into a per-category subdirectory (IDM-style).
    pub sort_by_type: bool,
    /// Content-Type the server reported, for classification.
    pub content_type: Option<String>,
    /// Accept any TLS certificate.
    pub insecure: bool,
    /// Overwrite an existing file without asking.
    pub force: bool,
    /// Discard the bytes instead of saving them.
    pub no_save: bool,
    /// Write a per-chunk digest manifest for what arrived.
    pub emit_manifest: Option<PathBuf>,
    /// Verify each chunk against this manifest as it arrives.
    pub chunk_digests: Option<PathBuf>,
    /// Chunk grid for --emit-manifest.
    pub chunk_size: Option<u64>,
}

/// What happened, for `--json` and for the report.
///
/// `Default` is the canonical "nothing happened yet" value (`ok: false`, every
/// counter zero, every option `None`); construct partial outcomes with
/// struct-update syntax rather than spelling out all 22 fields.
#[derive(serde::Serialize, Clone, Default)]
pub struct Outcome {
    pub url: String,
    pub output: String,
    pub size: u64,
    /// Wall time for the whole invocation, including probing and the concurrency
    /// measurement. This is the number a user timing the command sees.
    pub elapsed_s: f64,
    /// Wall time for the byte transfer alone.
    ///
    /// Reported separately because the two differ by seconds on a slow path — the
    /// progress bar's clock starts when bytes start, so a single `elapsed_s` next to a
    /// bar reading "1.7s" looked like two clocks disagreeing. Setup is a per-path cost
    /// a real client caches; transfer is the steady-state figure.
    pub transfer_s: f64,
    /// Wall time spent probing before the transfer began.
    pub setup_s: f64,
    pub throughput_bps: f64,
    pub requests: u64,
    pub connections: usize,
    pub delta_s: f64,
    pub sha256: Option<String>,
    pub checksum_ok: Option<bool>,
    pub resumed_from: u64,
    pub ok: bool,
    pub note: Option<String>,
    /// Detected format name, e.g. "gzip".
    pub format: Option<String>,
    /// Detected category, e.g. "archive".
    pub category: Option<String>,
    /// Disagreement between the payload, the name, and the server's type.
    pub format_conflict: Option<String>,
    /// Short human label for the format, e.g. "gzip-compressed tar".
    pub format_label: Option<String>,
    /// One-sentence explanation, for a CLI hint or a GUI tooltip.
    pub format_description: Option<String>,
    /// What the category is and what to do with it.
    pub category_description: Option<String>,
}

fn targets_for(
    urls: &[String],
    headers: &[String],
    agent: &str,
    proxy: Option<&str>,
    no_proxy: bool,
) -> Result<Vec<(Url, Target)>, String> {
    // Precedence: --no-proxy beats --proxy beats the environment. A user who
    // passes --no-proxy and still egresses through one would be misled about
    // where their traffic went.
    //
    // The scheme is load-bearing and must NOT be stripped: a SOCKS proxy carries a raw
    // TCP stream, so the request stays origin-form and the target must look direct. An
    // earlier version discarded the scheme and treated every proxy as HTTP, which would
    // have sent absolute-form GETs at a SOCKS port.
    let px = if no_proxy {
        None
    } else if let Some(spec) = proxy {
        match hya_net::Proxy::parse(spec) {
            // SOCKS: handled by the connector, so the target is built as if direct.
            Ok(p) if p.kind.is_socks() => None,
            Ok(p) => Some((p.host, p.port)),
            Err(e) => return Err(format!("--proxy: {e}")),
        }
    } else {
        proxy_from_env()
    };
    let pxr = px.as_ref().map(|(h, p)| (h.as_str(), *p));
    urls.iter()
        .map(|u| {
            let parsed = Url::parse(u).ok_or_else(|| {
                // Name the scheme's own reason when there is one: "sftp is not implemented
                // yet, and here is what to use instead" is actionable where "unsupported"
                // is not.
                match u.split_once("://") {
                    Some((s, _)) => format!(
                        "{u}: {} (supported: {})",
                        hya_net::scheme::unsupported_reason(&s.to_ascii_lowercase()),
                        hya_net::scheme::supported().join(", ")
                    ),
                    None => format!(
                        "unparsable URL: {u} (supported schemes: {})",
                        hya_net::scheme::supported().join(", ")
                    ),
                }
            })?;
            // `to_target` builds an HTTP request target and rejects any other scheme. FTP
            // does not use one — it gets an Endpoint instead — so a placeholder is paired
            // here and the FTP branch replaces it. Calling to_target for an ftp:// URL
            // returned an error BEFORE the FTP branch was ever reached, which is why an
            // ftp:// fetch failed silently with exit 1 and no message.
            if parsed.is_ftp() {
                let t = hya_net::Target::direct(&parsed.host, parsed.port, &parsed.path);
                return Ok((parsed, t));
            }
            let t = parsed
                .to_target(pxr)?
                .with_headers(headers.to_vec(), Some(agent.to_string()));
            Ok((parsed, t))
        })
        .collect()
}

/// Probe every mirror, and keep only those that agree with the first on both
/// size and validator.
///
/// This is a correctness gate, not an optimisation. Assembling ranges from two
/// mirrors that serve *different* bytes produces a corrupt file that passes every
/// length check — the unsound case the capability lattice names. Mirrors that
/// disagree are dropped with a warning rather than silently mixed in.
/// Probe an object, following redirects and falling back from HEAD to a ranged GET.
///
/// Three real behaviours make this more than a single request:
///   * a redirect: GitHub release assets answer HEAD with `302` and
///     `Content-Length: 0`, so the redirect must be followed rather than described;
///   * a server that does not answer HEAD at all: a public speed-test host closes the
///     connection with no reply (and no TLS `close_notify`) on HEAD while answering
///     GET normally on the same path;
///   * a server that omits `Content-Length` on HEAD, which a ranged GET recovers from
///     `Content-Range`.
///
/// The redirect budget is bounded, so a redirect loop costs a fixed number of round
/// trips rather than running forever.
/// Proxy setting for a URL, honouring `--no-proxy` semantics via the environment.
/// How many bytes of `path` are really there.
///
/// A resume record is authoritative when present. Otherwise fall back to allocated
/// blocks, which for a sparse file is far smaller than its apparent length. On a
/// filesystem that does not report blocks, treat a file whose apparent length equals
/// its allocation as fully present.
fn bytes_present(path: &Path, sidecar: Option<&Sidecar>) -> u64 {
    if let Some(sc) = sidecar {
        return sc.bytes_done();
    }
    let Ok(md) = std::fs::metadata(path) else {
        return 0;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let allocated = md.blocks() * 512;
        // Allocation is rounded up to a block, so it can exceed the apparent length by
        // less than one block; clamp rather than report more bytes than the file has.
        allocated.min(md.len())
    }
    #[cfg(not(unix))]
    {
        md.len()
    }
}

/// An Outcome with every field at a neutral value, for tests that care about two of them.
#[cfg(test)]
pub fn stub_outcome() -> Outcome {
    Outcome::default()
}

/// Where a job's bytes are destined.
///
/// Decided before a transfer begins so that `--no-save` discards bytes in memory
/// without creating or leaving temporary files on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OutputTarget {
    /// Write to this path and keep it.
    File(String),
    /// Write nothing anywhere: measure the stream and drop the bytes.
    Discard,
    /// Assemble, then stream to stdout. Needs storage because positioned writes
    /// land out of order, so the object is only correct once complete.
    Stdout(String),
}

/// Decide the destination for a job that would otherwise write `out_path`.
pub fn output_target(job: &Job, out_path: &str) -> OutputTarget {
    // `--stdout` is checked first: it needs a real file even under `--no-save`,
    // because out-of-order ranges cannot be streamed to a pipe as they arrive.
    if job.to_stdout {
        return OutputTarget::Stdout(out_path.to_string());
    }
    if job.no_save {
        return OutputTarget::Discard;
    }
    OutputTarget::File(out_path.to_string())
}

/// A scratch filename unique to this verification.
///
/// Per-CALL, not per-process: several files verify concurrently when multiple URLs are
/// given, and a shared name has them overwriting each other's window.
fn scratch_name() -> String {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!(
        "hydra_verify_{}_{}",
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

/// Check that the bytes already on disk are a genuine prefix of the remote object.
///
/// Re-fetches a window ending at the current file length and compares it byte for byte.
/// This is what lets `hydra <url>` resume a file it did not write: an interrupted
/// download leaves a valid prefix, and one small range request turns that assumption
/// into a check. The window is the last `WINDOW` bytes rather than the whole file
/// because a mismatch anywhere earlier would have to have been written by a *different*
/// object, and the tail is where a truncated write leaves damage.
///
/// Returns the number of verified bytes, or `None` when the prefix does not match.
async fn verify_prefix<C: hya_net::Connector>(
    c: &Arc<C>,
    t: &Target,
    path: &Path,
    on_disk: u64,
    tries: u32,
    timeout_s: f64,
) -> Option<u64> {
    const WINDOW: u64 = 64 * 1024;
    if on_disk == 0 {
        return Some(0);
    }
    let lo = on_disk.saturating_sub(WINDOW);
    let want = (on_disk - lo) as usize;
    // Read what we have.
    let mut local = vec![0u8; want];
    {
        use std::io::{Read, Seek, SeekFrom};
        let mut f = std::fs::File::open(path).ok()?;
        f.seek(SeekFrom::Start(lo)).ok()?;
        f.read_exact(&mut local).ok()?;
    }
    // Fetch the same window into a scratch sink.
    //
    // The name must be unique PER VERIFICATION, not per process: with several files
    // verifying concurrently (multiple URLs on one command line) a PID-only name has
    // every one of them writing the same scratch file, so each reads back another
    // object's bytes and reports a mismatch on a file that is in fact identical.
    let tmp = std::env::temp_dir().join(scratch_name());
    let tmps = tmp.to_string_lossy().to_string();
    let sink = Arc::new(SparseSink::create(&tmps, on_disk).ok()?);
    let ok = fetch_range_retry(
        c.clone(),
        t.clone(),
        lo,
        on_disk,
        sink.clone(),
        tries,
        timeout_s,
    )
    .await
    .is_ok();
    drop(sink);
    let remote = if ok {
        let mut buf = vec![0u8; want];
        use std::io::{Read, Seek, SeekFrom};
        std::fs::File::open(&tmps).ok().and_then(|mut f| {
            f.seek(SeekFrom::Start(lo)).ok()?;
            f.read_exact(&mut buf).ok()?;
            Some(buf)
        })
    } else {
        None
    };
    let _ = std::fs::remove_file(&tmp);
    match remote {
        Some(r) if r == local => Some(on_disk),
        _ => None,
    }
}

/// Print the probe exchange the way `curl -iv` does: request with `>`, response with
/// `<`, so a pasted transcript is unambiguous about direction.
fn print_exchange(pr: &hya_net::Probe) {
    for line in pr.raw_request.lines() {
        if !line.is_empty() {
            println!("> {line}");
        }
    }
    println!(">");
    for line in pr.raw_head.lines() {
        if !line.is_empty() {
            println!("< {line}");
        }
    }
    println!("<");
}

/// Fetch over FTP: single source, sequential, with the protocol's costs made explicit.
///
/// Deliberately not routed through the scheduler. The scheduler decides how to split work
/// across sources and when to reassign it, and both decisions rest on properties FTP does
/// not have: a validator to prove two sources agree, and free preemption. Driving it anyway
/// would produce reassignment decisions priced for the wrong protocol. What FTP does support
/// is a resumable sequential transfer, which is what this does.
async fn ftp_fetch(job: &Job, u: &Url, p: &mut Progress, outs: String) -> Outcome {
    // `Outcome::stopped` records a note but prints nothing; `failed` prints. Every failure
    // below goes through this so an exit code always arrives with a reason — an earlier
    // version returned a bare stopped Outcome and an ftp:// fetch exited 1 in silence.
    macro_rules! bail {
        ($($arg:tt)*) => {{
            let why = format!($($arg)*);
            p.end_phase();
            return failed(job, 0, why);
        }};
    }
    use hya_net::scheme::Fetcher;
    let t_all = Instant::now();
    let px = proxy_for(u);
    let ep = u.to_endpoint(px.as_ref().map(|(h, pt)| (h.as_str(), *pt)));
    let conn = Arc::new(hya_net::TcpConnector);
    let f = hya_net::ftp::FtpFetcher::new(conn);

    p.phase("connecting and logging in");
    let probe = match f.probe(&ep).await {
        Ok(pr) => pr,
        Err(e) => bail!("ftp: {e}"),
    };
    p.end_phase();
    p.event(
        1,
        &format!(
            "ftp {} -> {} bytes, ranges={}, login={}",
            u.host,
            probe.size,
            if probe.ranged { "yes (REST)" } else { "no" },
            if ep.has_credentials() {
                "explicit"
            } else {
                "anonymous"
            }
        ),
    );
    p.event(
        1,
        &format!(
            "ftp: preemption costs {:.0} control round trips here (HTTP pays none), so the \
             object is fetched sequentially from one source",
            f.capabilities().preempt_cost_rtt
        ),
    );
    if job.server_response {
        for line in probe.raw.lines() {
            println!("{line}");
        }
    }
    if job.spider {
        let ok = probe.size > 0;
        return Outcome::stopped(
            job,
            outs.clone(),
            probe.size,
            ok,
            "spider: FTP object exists",
        );
    }
    if probe.size == 0 {
        bail!(
            "ftp: the server does not implement SIZE, and FTP offers no other way to learn \
             the length; refusing rather than writing a file of unknown extent"
        );
    }

    // Resume uses REST, which is exactly a ranged read — the one place FTP's range support
    // is a clean fit.
    let start = if job.resume {
        std::fs::metadata(&outs).map(|m| m.len()).unwrap_or(0)
    } else {
        0
    };
    if start >= probe.size {
        return Outcome::stopped(job, outs.clone(), probe.size, true, "already complete");
    }
    let sink = match hya_net::SparseSink::create(&outs, probe.size) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            bail!("cannot create {outs}: {e}");
        }
    };
    // Draw the same progress bar HTTP gets.
    //
    // The HTTP path renders from the scheduler's per-tick observer callback, which
    // FTP has no equivalent of: `fetch_range` is one `await` that returns when the
    // whole object has landed, so awaiting it directly meant a multi-megabyte FTP
    // download sat in silence and then printed a finished summary. There is no
    // scheduler to observe here, but there is a sink, and the sink counts every
    // byte it writes — so the bar is driven from that counter while the fetch runs.
    //
    // `select!` on a pinned future rather than a spawned task: `Progress` is a
    // `&mut` the caller owns and the fetch borrows `ep`, so neither can cross a
    // task boundary without restructuring both.
    p.end_phase();
    // The size FTP already answered has to reach the renderer, or the bar cannot
    // draw. `ftp_fetch` is handed the setup-phase `Progress`, built before any
    // request when no size was known, and a `None` total renders an empty rule, a
    // `?` percentage and a `?` ETA for the whole transfer — measured on
    // ftp.gnu.org/gnu/wget/wget-latest.tar.gz, which reported `334.9 KiB/?` while
    // SIZE had returned the exact length during the probe two lines above.
    p.set_total(probe.size);
    p.set_baseline(start);
    let t_xfer = Instant::now();
    let r = {
        let fut = f.fetch_range(&ep, start, probe.size, sink.clone());
        tokio::pin!(fut);
        // 100 ms: `draw` already rate-limits itself to ~12 fps, so a faster tick
        // buys nothing and a slower one makes the bar visibly stutter.
        let mut ticker = tokio::time::interval(std::time::Duration::from_millis(100));
        ticker.tick().await; // the first tick completes immediately
        let mut last_done = start;
        let mut last_at = Instant::now();
        let mut rate = 0.0f64;
        loop {
            tokio::select! {
                res = &mut fut => break res,
                _ = ticker.tick() => {
                    let done = start + sink.written.load(std::sync::atomic::Ordering::Relaxed);
                    let dt = last_at.elapsed().as_secs_f64();
                    if dt > 0.0 {
                        let sample = done.saturating_sub(last_done) as f64 / dt;
                        // Same smoothing the scheduler applies to its own rate
                        // estimate, so the two paths report comparably rather
                        // than FTP showing a jumpier number for the same link.
                        rate = if rate <= 0.0 { sample } else { 0.3 * sample + 0.7 * rate };
                        last_done = done;
                        last_at = Instant::now();
                    }
                    // One connection, by design: FTP has no validator, so mirrors
                    // cannot be proven to serve identical bytes and the fetch is
                    // single-source. The view says so rather than implying a fan-out.
                    // `(lo, pos, hi)`, in that order — the renderer fills the row
                    // from `(pos - lo) / (hi - lo)` and prints `lo`-`hi` as the
                    // extent. Passing `(start, size, done)` swapped the cursor
                    // with the end, so the fraction was `(size - start) /
                    // (done - start)`: greater than 1 for the whole transfer,
                    // clamped to a permanently full `[▪▪▪▪▪▪▪▪▪▪]`, with the
                    // bytes-so-far printed where the object's length belongs.
                    let views = vec![ConnView {
                        idx: 0,
                        host: u.host.clone(),
                        range: Some((start, done, probe.size)),
                        rate,
                        health: hya_core::detect::Health::Healthy,
                    }];
                    p.draw(done, &views, Counters { requests: 1, ..Default::default() });
                }
            }
        }
    };
    let xfer = t_xfer.elapsed().as_secs_f64();
    drop(sink);
    if let Err(e) = r {
        return failed(job, start, format!("ftp: {e}"));
    }

    // Classify what arrived, exactly as the HTTP path does: the payload is the only
    // trustworthy signal, and FTP supplies no content type at all.
    let head = {
        use std::io::Read as _;
        let mut buf = vec![0u8; 8192];
        match std::fs::File::open(&outs).and_then(|mut fh| fh.read(&mut buf)) {
            Ok(n) => {
                buf.truncate(n);
                buf
            }
            Err(_) => Vec::new(),
        }
    };
    let name = std::path::Path::new(&outs)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let detection = detect_format(&head, &name, None);
    let digest = {
        use sha2::{Digest as _, Sha256};
        use std::io::Read as _;
        std::fs::File::open(&outs).ok().and_then(|mut fh| {
            let mut h = Sha256::new();
            let mut buf = vec![0u8; 1 << 20];
            loop {
                match fh.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => h.update(&buf[..n]),
                    Err(_) => return None,
                }
            }
            Some(hya_net::digest::to_lower_hex(&h.finalize()))
        })
    };
    let elapsed = t_all.elapsed().as_secs_f64();
    p.finish(
        probe.size,
        true,
        crate::progress::Counters {
            requests: 1,
            ..Default::default()
        },
        digest.as_deref(),
    );
    if !job.quiet && job.verbose == 0 {
        if let Some(fm) = detection.format {
            println!("  {}", fm.hint());
        }
    }
    Outcome {
        url: job.urls[0].clone(),
        output: outs,
        size: probe.size,
        elapsed_s: elapsed,
        transfer_s: xfer,
        setup_s: (elapsed - xfer).max(0.0),
        throughput_bps: if xfer > 0.0 {
            (probe.size - start) as f64 / xfer
        } else {
            0.0
        },
        requests: 1,
        connections: 1,
        delta_s: 0.0,
        sha256: digest,
        checksum_ok: None,
        resumed_from: start,
        ok: true,
        note: None,
        format: detection.format.map(|fm| fm.name.to_string()),
        category: Some(detection.category.as_str().to_string()),
        format_conflict: detection.conflict,
        format_label: detection.format.map(|fm| fm.label().to_string()),
        format_description: detection.format.map(|fm| fm.description().to_string()),
        category_description: Some(detection.category.description().to_string()),
    }
}

/// Resolve the proxy for a URL, for callers outside the download engine.
pub fn proxy_for_public(_u: &Url, proxy: Option<&str>, no_proxy: bool) -> Option<(String, u16)> {
    if no_proxy {
        return None;
    }
    match proxy {
        Some(spec) => match hya_net::Proxy::parse(spec) {
            Ok(px) if !px.kind.is_socks() => Some((px.host, px.port)),
            _ => None,
        },
        None => crate::url::proxy_from_env(),
    }
}

/// A Job with every option at its default, for callers that only need one or two set.
pub fn default_job() -> Job {
    Job {
        ticks: None,
        urls: Vec::new(),
        output: None,
        conns: None,
        resume: false,
        limit_rate: 0,
        max_redirs: 8,
        ip_family: hya_net::IpFamily::Any,
        show_error: false,
        logfile: None,
        tries: 3,
        timeout_s: 30.0,
        checksum: None,
        headers: Vec::new(),
        user_agent: hya_net::DEFAULT_USER_AGENT.into(),
        verbose: 0,
        quiet: true,
        no_progress: true,
        polite: hya_net::polite::Politeness::default(),
        adaptive: true,
        to_stdout: false,
        no_clobber: false,
        create_dirs: false,
        output_dir: None,
        spider: false,
        server_response: false,
        range: None,
        max_filesize: None,
        proxy: None,
        no_proxy: false,
        remote_time: false,
        etag_save: None,
        etag_compare: None,
        sort_by_type: false,
        content_type: None,
        insecure: false,
        force: false,
        no_save: false,
        emit_manifest: None,
        chunk_digests: None,
        chunk_size: None,
    }
}

/// A `Progress` configured from the job: logfile attached, and stdout reserved
/// for the payload under `--stdout`.
///
/// `run` builds two — one for the setup phase (size unknown), one for the
/// transfer (size known) — and both must agree on this configuration: the
/// setup-phase instance missing the stdout reservation was what prepended 71
/// bytes to a piped archive. stdout belongs to the object, on the same
/// principle `--json` follows: a machine channel carries one thing, or it
/// carries nothing usable.
fn progress_for(job: &Job, name: &str, size: Option<u64>) -> Result<Progress, String> {
    let mut p = Progress::new(name, size, job.verbose, job.no_progress, job.quiet);
    if let Some((path, append)) = &job.logfile {
        if let Err(e) = p.set_logfile(path, *append) {
            return Err(format!("cannot open log file {}: {e}", path.display()));
        }
    }
    if job.to_stdout {
        p.reserve_stdout_for_payload();
    }
    Ok(p)
}

fn proxy_for(u: &Url) -> Option<(String, u16)> {
    let _ = u;
    crate::url::proxy_from_env()
}

/// Probe `u` for metadata, following redirects, for the reporting commands.
///
/// `hydra checksum` and `--server-response` want the headers of the FINAL
/// response — a 302's headers answer a different question than the one asked.
/// Unlike the transfer path's [`probe_resolving`] there is no `Progress` to
/// log hops to, and reporting prefers to describe what it reached over
/// refusing: on an exhausted hop budget or an unusable `Location` the last
/// probe is returned as-is rather than as an error.
///
/// A HEAD that fails or reports no size falls back to a one-byte ranged GET,
/// the same recovery the transfer path uses — CDNs that mishandle HEAD
/// (unclean TLS close, `Content-Length: 0`) answer the GET correctly.
///
/// Returns the final probe and the URL it came from.
pub async fn probe_public<C: hya_net::Connector>(
    c: &C,
    u: &Url,
    args: &crate::cli::Cli,
) -> Result<(hya_net::Probe, Url), String> {
    let mut cur = u.clone();
    let mut hops = 0u32;
    loop {
        let px = proxy_for_public(&cur, args.proxy.as_deref(), args.no_proxy);
        let target = cur
            .to_target(px.as_ref().map(|(h, p)| (h.as_str(), *p)))?
            .with_headers(args.headers.clone(), Some(args.user_agent.clone()));
        let pr = match hya_net::probe(c, &target).await {
            Ok(pr) if !pr.is_redirect() && pr.size == 0 => {
                hya_net::probe_via_get(c, &target).await.unwrap_or(pr)
            }
            Ok(pr) => pr,
            Err(e) => match hya_net::probe_via_get(c, &target).await {
                Ok(g) => g,
                Err(_) => return Err(e.to_string()),
            },
        };
        if pr.is_redirect() && hops < args.max_redirs {
            let next = pr
                .location
                .as_deref()
                .and_then(|loc| crate::url::Url::parse(loc).or_else(|| cur.join(loc)));
            if let Some(next) = next {
                cur = next;
                hops += 1;
                continue;
            }
        }
        return Ok((pr, cur));
    }
}

async fn probe_resolving<C>(
    c: &C,
    u: &Url,
    t: &Target,
    p: &mut Progress,
    max_redirs: u32,
) -> Result<(hya_net::Probe, Target), String>
where
    C: hya_net::Connector,
{
    // The hop budget is the CLI's, not a constant. `--max-redirs` was parsed,
    // validated, and listed in `--help`, but the resolver used a hardcoded
    // `MAX_HOPS = 8` that nothing could influence: `--max-redirs 0` followed the
    // redirect and downloaded the object anyway.
    //
    // `0` means refuse to follow any, as curl and wget spell it. One probe still
    // happens — that is how a redirect is discovered at all — but the hop is not
    // taken.
    let max_hops = max_redirs as usize;
    let mut target = t.clone();
    let mut current = u.clone();
    // `0..=max_hops`: the extra pass is what answers the request that the last
    // permitted hop arrived at. Without it a budget of N would resolve only N-1.
    for hop in 0..=max_hops {
        let head = probe(c, &target).await;
        let pr = match head {
            Ok(pr) if !pr.is_redirect() && pr.size > 0 => Ok(pr),
            Ok(pr) if pr.is_redirect() => Ok(pr),
            // HEAD gave nothing usable (no length, or an error such as an unclean
            // TLS close). A ranged GET is the robust probe, so try it before
            // declaring the source unusable.
            other => match probe_via_get(c, &target).await {
                Ok(g) => Ok(g),
                Err(e) => match other {
                    Ok(h) => Ok(h),
                    Err(he) => Err(format!("HEAD failed ({he}), ranged GET failed ({e})")),
                },
            },
        }
        .map_err(|e: String| e)?;

        if pr.is_redirect() {
            let loc = pr.location.clone().unwrap_or_default();
            if hop >= max_hops {
                return Err(if max_hops == 0 {
                    format!("refusing to follow a redirect to {loc:?}: --max-redirs 0")
                } else {
                    format!("too many redirects (--max-redirs {max_redirs})")
                });
            }
            let next = current
                .join(&loc)
                .ok_or_else(|| format!("unparsable redirect target {loc:?}"))?;
            p.event(1, &format!("redirect {} -> {}", current.host, next.host));
            let px = proxy_for(&next);
            target = next
                .to_target(px.as_ref().map(|(h, p)| (h.as_str(), *p)))
                .map_err(|e| format!("redirect target unusable: {e}"))?;
            current = next;
            continue;
        }
        return Ok((pr, target));
    }
    Err(format!("too many redirects (--max-redirs {max_redirs})"))
}

async fn probe_all(
    conn: &Arc<TlsCapableConnector>,
    pairs: &[(Url, Target)],
    p: &mut Progress,
    max_redirs: u32,
) -> Result<(hya_net::Probe, Vec<usize>, Vec<(usize, Target)>), String> {
    let c = conn.clone();
    let mut first: Option<hya_net::Probe> = None;
    let mut keep = Vec::new();
    // Targets after redirect resolution, paired with the index they came from.
    let mut resolved_targets: Vec<(usize, Target)> = Vec::new();
    for (i, (u, t)) in pairs.iter().enumerate() {
        match probe_resolving(c.as_ref(), u, t, p, max_redirs).await {
            Ok((pr, resolved)) => {
                // A redirect may have moved the object to a different host; the
                // transfer must use the resolved target, not the one we started from.
                resolved_targets.push((i, resolved));
                let (size, ranges, validator) = (pr.size, pr.ranges, pr.validator.clone());
                p.event(
                    1,
                    &format!(
                        "probe {} -> {} bytes, ranges={}, validator={}",
                        u.host,
                        size,
                        if ranges { "yes" } else { "no" },
                        validator.as_deref().unwrap_or("none")
                    ),
                );
                match &first {
                    None => {
                        first = Some(pr);
                        keep.push(i);
                    }
                    Some(f0) => {
                        let same_size = f0.size == size;
                        let same_val = match (&f0.validator, &validator) {
                            (Some(a), Some(b)) => a == b,
                            // Without validators on both sides, byte identity
                            // across mirrors is unverifiable. Refuse to mix.
                            _ => false,
                        };
                        if same_size && same_val {
                            keep.push(i);
                        } else {
                            p.event(
                                0,
                                &format!(
                                    "skipping {}: {} (cannot prove it serves the same bytes)",
                                    u.host,
                                    if same_size {
                                        "validator differs"
                                    } else {
                                        "size differs"
                                    }
                                ),
                            );
                        }
                    }
                }
            }
            Err(e) => p.event(0, &format!("probe failed for {}: {e}", u.host)),
        }
    }
    match first {
        Some(pr) if !keep.is_empty() => Ok((pr, keep, resolved_targets)),
        _ => Err("no usable source: every probe failed".into()),
    }
}

/// Fixed-work concurrency probe based on measured marginal goodput.
///
/// Total bytes are held constant across levels to measure true scaling.
/// Learns the optimal connection count and per-request setup cost.
///
/// The probe has to move bytes to measure anything — marginal goodput is not
/// observable without observing it. Writing those bytes to a temp file and deleting
/// them makes the measurement pure waste: on a 128 MB object over a slow path it cost
/// ~9 s and 1.5 MB before the transfer had started, which is what "sometimes takes a
/// long time to start" was. Writing them at their true offsets in the real sink turns
/// the probe into the first part of the transfer: the ranges it filled are returned so
/// the scheduler can mark them done and never re-fetch them.
///
/// This is only sound because the probe reads *aligned, exact* ranges and the transport
/// validates `Content-Range` before writing. A probe that could not prove where its
/// bytes belonged would have to keep discarding them.
async fn learn_concurrency(
    conn: &Arc<TlsCapableConnector>,
    t: &Target,
    size: u64,
    max: usize,
    tries: u32,
    timeout_s: f64,
    sink: &Arc<SparseSink>,
    p: &mut Progress,
) -> (usize, f64, Vec<(u64, u64)>) {
    const TOTAL: u64 = 768 * 1024;
    if size <= TOTAL * 2 {
        // Too small to probe: the measurement would cover most of the object, and on a
        // small object the answer is one connection anyway.
        return (1, 0.05, Vec::new());
    }
    let c = conn.clone();
    let mut adm = Admission::new(0.15, max);
    let mut de = DeltaEstimator::new(0.05);
    let mut level = 1usize;
    // Probe from the FRONT of the object, contiguously, so the bytes it keeps are the
    // bytes a sequential reader wants first and the progress bar advances from zero.
    let mut base = 0u64;
    let mut filled: Vec<(u64, u64)> = Vec::new();
    loop {
        let slice = TOTAL / level as u64;
        if base + TOTAL >= size {
            break;
        }
        let t0 = Instant::now();
        let mut hs = Vec::new();
        for k in 0..level {
            let lo = base + k as u64 * slice;
            let (cc, sk, tt) = (c.clone(), sink.clone(), t.clone());
            hs.push(tokio::spawn(async move {
                let s = Instant::now();
                let r = fetch_range_retry(cc, tt, lo, lo + slice, sk, tries, timeout_s).await;
                (r.is_ok(), s.elapsed().as_secs_f64())
            }));
        }
        let (mut got, mut per) = (0u64, Vec::new());
        for (k, h) in hs.into_iter().enumerate() {
            if let Ok((ok, el)) = h.await {
                if ok {
                    got += slice;
                    per.push(el);
                    let lo = base + k as u64 * slice;
                    filled.push((lo, lo + slice));
                }
            }
        }
        let el = t0.elapsed().as_secs_f64().max(1e-3);
        if got == 0 {
            break;
        }
        // Probe bytes are real progress now, so report them as such.
        p.add_probe_bytes(got);
        let rate = got as f64 / el;
        // delta = wall time minus the time the bytes themselves should have
        // taken, which isolates setup from streaming.
        for x in &per {
            de.observe((x - slice as f64 / rate.max(1.0)).max(1e-3));
        }
        base += TOTAL;
        p.event(
            1,
            &format!(
                "probe level {level}: {:.2} MB/s, delta ~{:.3}s",
                rate / 1.048576e6,
                de.get()
            ),
        );
        match adm.observe(rate) {
            Admit::Stop => break,
            Admit::Add => level += 1,
        }
    }
    (adm.settled().unwrap_or(1).max(1), de.get(), filled)
}

/// Set a file's modification time from a Unix timestamp.
fn set_mtime(path: &Path, secs: u64) -> std::io::Result<()> {
    let f = std::fs::File::options().write(true).open(path)?;
    f.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs))
}

/// Reduce a range-mode output to just the fetched span.
///
/// The transfer wrote `[lo, hi)` at its true offsets inside a file of the whole
/// object's length, so everything outside the span is a hole that reads as
/// zeros. This moves the span to offset 0 and truncates.
///
/// Copied in bounded blocks rather than read whole: the point of positioned
/// writes is that memory does not scale with the object, and a range can be
/// gigabytes. `lo == 0` skips the move and only truncates.
fn extract_span(path: &Path, lo: u64, hi: u64) -> std::io::Result<()> {
    use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
    let span = hi.saturating_sub(lo);
    if lo > 0 {
        let mut f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)?;
        let mut buf = vec![0u8; 1 << 20];
        let (mut src, mut dst, mut left) = (lo, 0u64, span);
        while left > 0 {
            let want = (buf.len() as u64).min(left) as usize;
            f.seek(SeekFrom::Start(src))?;
            f.read_exact(&mut buf[..want])?;
            f.seek(SeekFrom::Start(dst))?;
            f.write_all(&buf[..want])?;
            src += want as u64;
            dst += want as u64;
            left -= want as u64;
        }
        f.flush()?;
    }
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)?
        .set_len(span)
}

/// Verify a finished file chunk-by-chunk against a manifest, refetching any
/// chunk whose digest does not match.
///
/// Returns a human summary. An unrepairable mismatch is an error, not a warning:
/// the file is known-wrong at a known offset, and reporting success for it would
/// be the silent-corruption failure this project keeps designing against.
async fn verify_and_repair_chunks(
    conn: &Arc<TlsCapableConnector>,
    usable: &[(Url, Target)],
    out_path: &Path,
    manifest_path: &Path,
    job: &Job,
    p: &mut Progress,
) -> Result<String, String> {
    use hya_net::manifest::{ChunkVerifier, Manifest, Trust};

    let text = std::fs::read_to_string(manifest_path)
        .map_err(|e| format!("cannot read manifest {}: {e}", manifest_path.display()))?;
    let m = Manifest::parse(&text).map_err(|e| format!("{}: {e}", manifest_path.display()))?;

    // A manifest handed as a local file is trusted for chunk verification.
    let mut v = ChunkVerifier::new(m, Trust::Trusted);

    p.phase("verifying chunks");
    {
        let mut f = std::fs::File::open(out_path)
            .map_err(|e| format!("cannot reopen {} to verify: {e}", out_path.display()))?;
        v.write_reader(&mut f)
            .map_err(|e| format!("read failed while verifying: {e}"))?;
    }
    p.end_phase();

    if v.all_verified() {
        return Ok(format!("all {} chunks verified", v.verified_count()));
    }

    let bad = v.failed_indices().to_vec();
    p.event(
        0,
        &format!(
            "{} chunk(s) failed verification: {:?} — refetching",
            bad.len(),
            bad
        ),
    );

    let sink = Arc::new(
        hya_net::SparseSink::create(&out_path.to_string_lossy(), v.manifest().object.size)
            .map_err(|e| format!("cannot reopen {} to repair: {e}", out_path.display()))?,
    );

    let mut repaired = 0usize;
    for idx in bad {
        let (lo, hi) = v.manifest().span(idx);
        // Prefer a DIFFERENT source than the one that served it: a mirror that
        // delivered corrupt bytes once is the least likely to fix them.
        let alt = if usable.len() > 1 { 1 } else { 0 };
        let t = usable[alt.min(usable.len() - 1)].1.clone();
        p.event(1, &format!("refetching chunk {idx} [{lo},{hi})"));
        hya_net::fetch_range_retry(
            conn.clone(),
            t,
            lo,
            hi,
            sink.clone(),
            job.tries,
            job.timeout_s,
        )
        .await
        .map_err(|e| format!("chunk {idx} refetch failed: {e}"))?;

        // Re-verify from disk. A refetch that is ALSO corrupt must not be
        // accepted just because it was requested again.
        let mut fresh = vec![0u8; (hi - lo) as usize];
        {
            use std::io::{Read as _, Seek as _, SeekFrom};
            let mut f = std::fs::File::open(out_path).map_err(|e| e.to_string())?;
            f.seek(SeekFrom::Start(lo)).map_err(|e| e.to_string())?;
            f.read_exact(&mut fresh).map_err(|e| e.to_string())?;
        }
        v.retry(idx);
        if !v.write(lo, &fresh).is_empty() {
            return Err(format!(
                "chunk {idx} [{lo},{hi}) still fails its digest after refetch: the source is \
                 serving bytes that do not match the manifest, so the file cannot be completed \
                 correctly from it"
            ));
        }
        repaired += 1;
    }

    Ok(format!(
        "{} chunks verified, {repaired} repaired by targeted refetch",
        v.verified_count()
    ))
}

fn sha256_file(path: &Path) -> Option<String> {
    let data = std::fs::read(path).ok()?;
    let mut h = Sha256::new();
    h.update(&data);
    Some(hya_net::digest::to_lower_hex(&h.finalize()))
}

pub async fn run(job: Job) -> Outcome {
    let pairs = match targets_for(
        &job.urls,
        &job.headers,
        &job.user_agent,
        job.proxy.as_deref(),
        job.no_proxy,
    ) {
        Ok(v) => v,
        Err(e) => return failed(&job, 0, e),
    };
    let name = job
        .output
        .clone()
        .map(|p| {
            p.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()
        })
        .unwrap_or_else(|| pairs[0].0.suggested_filename());
    let mut out_path = job.output.clone().unwrap_or_else(|| PathBuf::from(&name));
    if let Some(dir) = &job.output_dir {
        if out_path.is_relative() {
            out_path = dir.join(&out_path);
        }
    }
    if job.create_dirs {
        if let Some(parent) = out_path.parent() {
            if !parent.as_os_str().is_empty() {
                let _ = std::fs::create_dir_all(parent);
            }
        }
    }
    // --no-clobber must short-circuit BEFORE any request: its purpose is to
    // avoid touching the network at all for a file already present.
    if job.no_clobber && out_path.exists() && !job.resume {
        if !job.quiet {
            eprintln!(
                "hydra: {} already exists; not retrieved (--no-clobber)",
                out_path.display()
            );
        }
        return Outcome::stopped(
            &job,
            out_path.to_string_lossy().to_string(),
            std::fs::metadata(&out_path).map(|m| m.len()).unwrap_or(0),
            true,
            "skipped: file exists",
        );
    }

    // Reserved on BOTH progress instances (see `progress_for`): this first one
    // covers the setup phase (probe, redirects, concurrency measurement), and
    // its setup line was what prepended 71 bytes to a piped archive.
    let mut p = match progress_for(&job, &name, None) {
        Ok(p) => p,
        Err(e) => return failed(&job, 0, e),
    };
    let t_start = Instant::now();

    // One connector for the whole job: it carries the TLS session cache, so the
    // second and later connections to a host skip a full handshake. That directly
    // lowers the per-request setup cost the scheduler is measuring.
    // Resolve the proxy spec once. A SOCKS proxy is configured on the CONNECTOR (it
    // carries the TCP stream); an HTTP proxy is configured on each TARGET (it rewrites
    // the request). Conflating them sends a CONNECT to the origin, or an absolute-form
    // request to a SOCKS port.
    let socks = match &job.proxy {
        Some(raw) => match hya_net::Proxy::parse(raw) {
            Ok(px) if px.kind.is_socks() => Some(px),
            Ok(_) => None,
            Err(e) => return failed(&job, 0, format!("--proxy: {e}")),
        },
        None => None,
    };
    let conn = match TlsCapableConnector::with_insecure(job.insecure) {
        // A SOCKS proxy belongs on the connector: it carries the TCP stream and never
        // parses HTTP, so every connection this client opens must go through it.
        Ok(c) => Arc::new(
            match socks.clone() {
                Some(px) => {
                    p.event(
                        1,
                        &format!(
                            "routing through {} proxy {}:{}",
                            px.kind.as_str(),
                            px.host,
                            px.port
                        ),
                    );
                    c.with_socks(px)
                }
                None => c,
            }
            .with_family(job.ip_family),
        ),
        Err(e) => return failed(&job, 0, format!("tls setup failed: {e}")),
    };

    // ---- FTP takes a separate path ---------------------------------------
    //
    // Not because the scheduler cannot drive it, but because two of its properties differ
    // in ways that change the right behaviour rather than just the syntax:
    //
    //  * No validator. SIZE+MDTM cannot prove two mirrors serve identical bytes (this
    //    project has already observed HTTP mirrors agreeing on size while serving
    //    different builds), so multi-source assembly is refused rather than attempted.
    //    A file spliced from two versions passes every length check and is silently wrong.
    //  * Preemption costs two control round trips (ABOR+reply, PASV+reply) against zero
    //    for HTTP, because REST names a start and nothing names an end. Reassigning as
    //    eagerly as HTTP would spend the benefit on control traffic.
    //
    // So an FTP fetch is single-source and sequential, with the reason stated.
    if pairs.first().map(|(u, _)| u.is_ftp()).unwrap_or(false) {
        if pairs.len() > 1 {
            p.event(
                0,
                "ftp: using the first source only — FTP offers no validator that can prove \
                 two servers hold identical bytes",
            );
        }
        let outs = out_path.to_string_lossy().to_string();
        return ftp_fetch(&job, &pairs[0].0, &mut p, outs).await;
    }

    p.phase("resolving and probing sources");
    let (probe_info, keep, resolved) = match probe_all(&conn, &pairs, &mut p, job.max_redirs).await
    {
        Ok(v) => v,
        Err(e) => return failed(&job, 0, e),
    };
    // Adopt the post-redirect targets: a release asset commonly redirects to a
    // different host, and fetching from the pre-redirect URL would 302 on every range.
    let mut pairs = pairs;
    for (i, t) in resolved {
        if let Some(slot) = pairs.get_mut(i) {
            slot.1 = t;
        }
    }
    // A HEAD is allowed to omit Content-Length, and CDNs do. Falling back to a
    // one-byte range request is what turns "0 bytes, success" into a real transfer.
    let mut size = probe_info.size;
    let validator = probe_info.validator.clone();
    // Kept separately from `validator`: `--remote-time` needs the DATE, and the
    // validator is whichever of the two headers is better for resume — an ETag
    // when the server sent one, which is opaque and carries no time.
    let last_modified = probe_info.last_modified.clone();
    // The server's own Content-Type, used as the weakest of the three
    // classification signals.
    let served_type = job
        .content_type
        .clone()
        .or_else(|| probe_info.content_type.clone());
    let usable: Vec<(Url, Target)> = keep.iter().map(|&i| pairs[i].clone()).collect();
    if size == 0 && !job.spider {
        // `keep` holds the indices that probed consistently; any of them can answer.
        match hya_net::probe_size_via_range(conn.as_ref(), &pairs[keep[0]].1).await {
            Ok(n) if n > 0 => {
                p.event(1, &format!("size from Content-Range: {n} bytes"));
                size = n;
            }
            Ok(_) => {}
            Err(e) => p.event(1, &format!("size fallback failed: {e}")),
        }
    }
    if size == 0 && !job.spider {
        // No knowable size. Multi-source scheduling is impossible — with no total there
        // are no ranges to divide — but FETCHING is not, and curl does this routinely
        // for dynamic pages and `Content-Range: bytes 0-0/*` replies. Degrade to a
        // single sequential stream and say so, rather than failing where every other
        // client succeeds.
        if job.server_response {
            // Clear the spinner first: it shares the row and would prefix the request
            // line with a partial frame.
            p.end_phase();
            print_exchange(&probe_info);
        }
        p.event(
            0,
            "size unknown: streaming with one connection (no parallelism, no resume)",
        );
        p.end_phase();
        let outs = out_path.to_string_lossy().to_string();
        let t0 = Instant::now();
        return match hya_net::fetch_streaming(conn.as_ref(), &usable[0].1, &outs).await {
            Ok(0) => failed(&job, 0, "the server sent no body".into()),
            Ok(n) => {
                let el = t0.elapsed().as_secs_f64();
                p.event(
                    0,
                    &format!("streamed {} in {:.1}s", crate::progress::human(n), el),
                );
                let mut o = Outcome::stopped(
                    &job,
                    out_path.to_string_lossy().to_string(),
                    n,
                    true,
                    "streamed (size was not knowable in advance)",
                );
                // Classification still applies, and matters more here: an unknown-size
                // response is very often HTML where a file was expected.
                // Read the first bytes back for classification; the file is written
                // sequentially here so its head is its head.
                let head = {
                    use std::io::Read;
                    let mut b = vec![0u8; 4096];
                    match std::fs::File::open(&out_path).and_then(|mut f| f.read(&mut b)) {
                        Ok(k) => {
                            b.truncate(k);
                            b
                        }
                        Err(_) => Vec::new(),
                    }
                };
                let det = detect_format(&head, &name, probe_info.content_type.as_deref());
                o.format = det.format.map(|f| f.name.to_string());
                o.category = Some(det.category.as_str().to_string());
                o.format_conflict = det.conflict.clone();
                if !job.quiet {
                    if let Some(c) = &det.conflict {
                        eprintln!("hydra: warning: {c}");
                    }
                    if let Some(f) = det.format {
                        println!("  {}", f.hint());
                    }
                    // An HTML body where a file was expected is the captive-portal /
                    // login-wall case, and on an unknown-size response it is the most
                    // likely outcome of all — worth saying plainly.
                    if det.category == Category::Markup && job.output.is_some() {
                        eprintln!(
                            "hydra: note: this is a web page, not a file. If you meant a \
                             release asset, use the download URL rather than the page URL."
                        );
                    }
                }
                o
            }
            Err(e) => failed(&job, 0, format!("streaming fetch failed: {e}")),
        };
    }

    // --spider / -I: report and stop. No body is requested, so this is the safe
    // way to inspect a URL (and what a link checker wants).
    if job.spider {
        // `-S`/`-i` must be honoured here too. The only other place headers are
        // printed is after the transfer, which `--spider` returns before ever
        // reaching — so `hydra --spider -S <url>` silently showed no headers,
        // despite being the most natural way to ask for exactly them.
        if job.server_response && !job.quiet {
            p.end_phase();
            print_exchange(&probe_info);
        }
        if !job.quiet {
            println!(
                "{}  {} bytes  ranges={}  validator={}",
                name,
                size,
                if usable.len() > 1 {
                    "yes (multi-source usable)"
                } else {
                    "yes"
                },
                validator.as_deref().unwrap_or("none")
            );
        }
        return Outcome::stopped(&job, String::new(), size, true, "spider: headers only");
    }

    // --max-filesize: refuse before opening a socket for the body, which is the
    // only point at which refusing actually saves anything.
    if let Some(cap) = job.max_filesize {
        if size > cap {
            return failed(
                &job,
                size,
                format!("object is {size} bytes, exceeding --max-filesize {cap}"),
            );
        }
    }

    // --etag-compare: if the stored validator still matches, the object has not
    // changed and there is nothing to retrieve.
    if let Some(path) = &job.etag_compare {
        if let (Ok(stored), Some(current)) = (std::fs::read_to_string(path), validator.as_deref()) {
            if stored.trim() == current.trim() {
                if !job.quiet {
                    eprintln!(
                        "hydra: unchanged (ETag matches {}), not retrieved",
                        path.display()
                    );
                }
                return Outcome::stopped(
                    &job,
                    out_path.to_string_lossy().to_string(),
                    size,
                    true,
                    "unchanged: ETag matches",
                );
            }
        }
    }
    if let (Some(path), Some(v)) = (&job.etag_save, validator.as_deref()) {
        let _ = std::fs::write(path, v);
    }
    // A second Progress now that the size is known. The phase line from the probe
    // stage is cleared first so the two never share a terminal row.
    p.end_phase();
    let mut p = match progress_for(&job, &name, Some(size)) {
        Ok(p) => p,
        Err(e) => return failed(&job, size, e),
    };

    // ---- existing file --------------------------------------------------
    //
    // Four outcomes are possible and none is a safe default for every case, so an
    // interactive run asks. The flags are answers and are never re-asked; a
    // non-interactive run picks the option that cannot destroy data.
    let mut resumed_from = 0u64;
    let mut prior: Option<Sidecar> = None;
    let existing_sidecar = Sidecar::load(&out_path);
    // Bytes verified as a genuine prefix of the remote object, when resuming a file
    // that has no sidecar record.
    let mut adopted_prefix: Option<u64> = None;
    // Bytes ACTUALLY present, not the file's apparent length.
    //
    // The output is a sparse file created at full length before the first byte
    // arrives. Allocated blocks and resume records determine actual progress.
    let on_disk = bytes_present(&out_path, existing_sidecar.as_ref());

    // Where the bytes are going, decided BEFORE anything touches the filesystem.
    //
    // Under `--no-save` this is `Discard`, and the whole existing-file question
    // below becomes moot: a run that will never write cannot clobber, cannot
    // resume, and must not prompt about — or rename around — a file it is not
    // going to open. The previous implementation created the file, wrote it,
    // hashed it and deleted it at the end, so all of that machinery ran and the
    // bytes sat on disk for the duration.
    let destination = output_target(&job, &out_path.to_string_lossy());
    let discarding = destination == OutputTarget::Discard;

    if out_path.exists() && !job.to_stdout && !discarding {
        let offer = match &existing_sidecar {
            Some(sc) => match sc.can_resume(size, validator.as_deref()) {
                Ok(()) => crate::prompt::ResumeOffer::Sound(sc.bytes_done()),
                Err(why) => crate::prompt::ResumeOffer::Refused(why),
            },
            // No sidecar. That is the ordinary case for a file another tool started, or
            // one hydra was killed during before writing its record, and it is NOT a
            // reason to re-fetch from zero: the bytes can be checked against the server.
            None if !probe_info.ranges => crate::prompt::ResumeOffer::Refused(
                "the server does not support byte ranges, so a partial file cannot be \
                 continued from"
                    .into(),
            ),
            None if on_disk >= size => crate::prompt::ResumeOffer::LooksComplete(on_disk),
            None if on_disk > 0 => crate::prompt::ResumeOffer::Verifiable(on_disk),
            None => crate::prompt::ResumeOffer::Refused("the existing file is empty".into()),
        };
        let flags = crate::prompt::Flags {
            resume: job.resume,
            no_clobber: job.no_clobber,
            force: job.force,
            assume_default: job.quiet,
        };
        p.end_phase();
        let choice = crate::prompt::ask(&out_path, on_disk, size, &offer, flags)
            .unwrap_or(crate::prompt::Existing::Rename);
        match choice {
            crate::prompt::Existing::Skip => {
                return Outcome::stopped(
                    &job,
                    out_path.to_string_lossy().to_string(),
                    on_disk,
                    true,
                    "kept the existing file",
                );
            }
            crate::prompt::Existing::Rename => match crate::prompt::next_free_name(&out_path) {
                Some(fresh) => {
                    p.event(0, &format!("writing to {}", fresh.display()));
                    out_path = fresh;
                }
                None => {
                    return failed(
                        &job,
                        size,
                        "no free filename beside the existing one".into(),
                    )
                }
            },
            crate::prompt::Existing::Restart => {
                Sidecar::remove(&out_path);
                let _ = std::fs::remove_file(&out_path);
            }
            crate::prompt::Existing::Verify => {
                p.phase("verifying the existing file against the server");
                let full = verify_prefix(
                    &conn,
                    &usable[0].1,
                    &out_path,
                    on_disk.min(size),
                    job.tries,
                    job.timeout_s,
                )
                .await;
                p.end_phase();
                return match full {
                    Some(_) if on_disk == size => Outcome::stopped(
                        &job,
                        out_path.to_string_lossy().to_string(),
                        on_disk,
                        true,
                        "already complete: the file matches the server",
                    ),
                    Some(_) => Outcome::stopped(
                        &job,
                        out_path.to_string_lossy().to_string(),
                        on_disk,
                        false,
                        "the file is a valid prefix but is shorter than the object; \
                         re-run with -c to continue it",
                    ),
                    None => Outcome::stopped(
                        &job,
                        out_path.to_string_lossy().to_string(),
                        on_disk,
                        false,
                        "the existing file does NOT match the server",
                    ),
                };
            }
            crate::prompt::Existing::Resume if existing_sidecar.is_none() => {
                // Resuming a file we did not write: prove the prefix first. Without
                // this the transfer would append to bytes of unknown provenance, which
                // is precisely the silent-corruption class this project keeps finding.
                p.phase("checking the existing bytes against the server");
                let v = verify_prefix(
                    &conn,
                    &usable[0].1,
                    &out_path,
                    on_disk,
                    job.tries,
                    job.timeout_s,
                )
                .await;
                p.end_phase();
                match v {
                    Some(n) => {
                        p.event(
                            0,
                            &format!(
                                "verified {} already on disk; continuing from there",
                                crate::progress::human(n)
                            ),
                        );
                        adopted_prefix = Some(n);
                    }
                    None => {
                        p.event(
                            0,
                            "the existing bytes do not match the server; starting over",
                        );
                        let _ = std::fs::remove_file(&out_path);
                    }
                }
            }
            crate::prompt::Existing::Resume => {
                // Fall through: the resume block below picks up the sidecar.
            }
        }
    }

    // ---- resume ---------------------------------------------------------
    if job.resume || existing_sidecar.is_some() {
        if let Some(sc) = Sidecar::load(&out_path) {
            match sc.can_resume(size, validator.as_deref()) {
                Ok(()) => {
                    resumed_from = sc.bytes_done();
                    p.event(
                        0,
                        &format!(
                            "resuming: {} already held",
                            crate::progress::human(resumed_from)
                        ),
                    );
                    prior = Some(sc);
                }
                Err(why) => {
                    p.event(0, &format!("cannot resume ({why}); starting over"));
                    Sidecar::remove(&out_path);
                }
            }
        }
    }

    // ---- the discarding sink, created once -------------------------------
    // Created before the concurrency probe so probe bytes are recorded by the digest sink.
    let discard_sink = discarding.then(|| {
        Arc::new(SparseSink::discarding().with_digest(hya_net::stream_digest::DEFAULT_REORDER_CAP))
    });

    // ---- concurrency ----------------------------------------------------
    // An explicit `-x N` / `-s N` is an instruction, not a hint. Measuring anyway
    // and then overriding it was a silent no-op: `-x 5` produced ONE connection
    // while the flag's own help says measurement is what happens when you OMIT
    // it. Whoever passes a number has a reason — a known-good mirror, a
    // reproduction, a comparison against another client — and a measurement that
    // quietly wins makes those impossible and looks like the flag is broken.
    //
    // `--adaptive` remains available to ask for measurement WITH a ceiling; the
    // probe is still what runs when no number is given at all.
    let (n_conns, delta, probe_filled) = match job.conns {
        Some(n) => (job.polite.allow(n), 0.05, Vec::new()),
        _ => {
            // The probe writes into the same file the transfer will use, so it must
            // exist first. `run_transfer_observed` opens it again by path; both open
            // the same sparse file and write disjoint offsets.
            //
            // Under `--no-save` there is no file at all, so the probe discards as well.
            let outs = out_path.to_string_lossy().to_string();
            let probe_sink = match &discard_sink {
                // The same sink the transfer will use, so the probe's bytes are
                // counted by the digest exactly once.
                Some(sk) => Ok(sk.clone()),
                None => SparseSink::create(&outs, size).map(Arc::new),
            };
            match probe_sink {
                Ok(sk) => {
                    p.phase("measuring useful connection count");
                    let (n, d, filled) = learn_concurrency(
                        &conn,
                        &usable[0].1,
                        size,
                        job.polite.per_host,
                        job.tries,
                        job.timeout_s,
                        &sk,
                        &mut p,
                    )
                    .await;
                    drop(sk);
                    (job.polite.allow(n), d, filled)
                }
                Err(e) => return failed(&job, size, format!("cannot create {outs}: {e}")),
            }
        }
    };
    // Split the connections across sources under BOTH ceilings: per-host, and the
    // aggregate `--max-total-connections`. The earlier arithmetic divided the
    // count over the sources and rounded UP, which consulted neither the total nor
    // the per-host limit — `--max-total-connections 2 -x 8` reported eight
    // connections and opened eight, because nothing in this path ever read
    // `Politeness.total`.
    let split = job.polite.split(n_conns, usable.len());
    let n_sources = split.iter().filter(|&&n| n > 0).count().max(1);
    let tgts: Vec<Target> = usable
        .iter()
        .take(n_sources)
        .map(|(_, t)| t.clone())
        .collect();
    let per: Vec<usize> = split.into_iter().take(n_sources).collect();
    // What the transfer will actually open, which is what `--json.connections`
    // must report: a number the run did not use is not a measurement.
    let n_conns: usize = per.iter().sum();
    // Level 1, not 0: at default verbosity the useful signal is the progress bar
    // and the one-line result. How many sources were chosen and what the measured
    // setup cost was are diagnostics — real ones, but the answer to "did it work"
    // should not be preceded by two lines of internals.
    p.event(
        1,
        &format!(
            "{} source(s), {} connection(s) total, delta ~{:.3}s",
            tgts.len(),
            n_conns,
            delta
        ),
    );

    let sources: Vec<Source> = tgts
        .iter()
        .map(|_| Source {
            gamma_est: 1.0e6,
            delta_est: delta.max(1e-3),
            ..Default::default()
        })
        .collect();
    // --range: schedule only the requested interval. Resolving it here rather
    // than in the argument parser is what lets a suffix range like `-512` mean
    // "the last 512 bytes" — that needs the object size, known only now.
    let (want_lo, want_hi) = match job.range {
        None => (0u64, size),
        Some(spec) => match spec.resolve(size) {
            Some(r) => r,
            None => {
                return failed(
                    &job,
                    size,
                    format!("{spec:?} is empty for a {size}-byte object"),
                )
            }
        },
    };
    let partial = (want_lo, want_hi) != (0, size);
    if partial {
        p.event(
            0,
            &format!(
                "range mode: bytes {}-{} of {} ({})",
                want_lo,
                want_hi - 1,
                size,
                crate::progress::human(want_hi - want_lo)
            ),
        );
    }

    let t_transfer = Instant::now();
    let mut sched =
        Scheduler::new(size, sources, &per).with_stall_timeout((12.0 * delta).clamp(4.0, 45.0));
    // Everything outside the requested range is marked held so the scheduler
    // never issues a request for it.
    if partial {
        sched.mark_done(0, want_lo);
        sched.mark_done(want_hi, size);
    }
    // Pre-mark resumed ranges as held so they are never re-fetched.
    if let Some(sc) = &prior {
        for (lo, hi) in &sc.done {
            sched.mark_done(*lo, *hi);
        }
    }
    // A verified prefix from a sidecar-less file: mark it held so it is never
    // re-fetched. This is the whole point of verifying it.
    if let Some(n) = adopted_prefix {
        let n = n.min(want_hi);
        if n > want_lo {
            sched.mark_done(want_lo, n);
            resumed_from = n;
        }
    }
    // The concurrency probe already fetched these into the real output. Marking them
    // held is what makes the probe free rather than wasted: without this the scheduler
    // would re-request bytes that are already on disk.
    for (lo, hi) in &probe_filled {
        if *lo >= want_lo && *hi <= want_hi {
            sched.mark_done(*lo, *hi);
        }
    }

    p.set_baseline(resumed_from);
    // Live byte accounting, sampled from the scheduler as the transfer runs.
    //
    // The post-transfer completeness check needs to know what is present AFTER
    // the transfer, and every cheap way of asking that question afterwards lies
    // in some case: the pre-transfer sidecar is stale by construction, a file's
    // apparent length is the whole object from the first byte because the output
    // is sparse, and allocated blocks read as the full size on a filesystem that
    // does not do sparse files. The scheduler is the one component that knows
    // which bytes actually arrived, so its own count is carried out of the
    // transfer here rather than re-derived from a record written before it.
    //
    // Seeded from the scheduler's state as it stands right now — after every
    // `mark_done` above — because a transfer with nothing left to fetch completes
    // before the first observation tick and never updates this at all.
    let progress = Arc::new(std::sync::atomic::AtomicU64::new(sched.bytes_held()));
    let progress_obs = progress.clone();

    // Checkpoint what is already held BEFORE the first byte of the transfer.
    //
    // The periodic checkpoint inside the render closure only fires once 2 seconds
    // have passed, so a ^C during the concurrency probe — or within 2s of the
    // transfer starting — left no sidecar at all, and the next `-c` restarted from
    // zero. The probe's bytes are real: it fetches at true offsets into the real
    // output and the scheduler marks those ranges held, which is what makes the
    // probe free rather than wasted. Discarding them on an interrupt throws away
    // a round trip the user already paid for.
    //
    // Written here rather than inside the probe because this is the first point
    // where the probe's ranges, a resume record, and `--range` have all been
    // folded into one authority on what is held.
    if !discarding {
        let held = sched.held_ranges();
        if !held.is_empty() && held != vec![(0, size)] {
            let rec = Sidecar {
                size,
                validator: validator.clone(),
                done: held,
                url: job.urls[0].clone(),
            };
            let _ = rec.save(&out_path);
        }
    }
    let limiter = Arc::new(if job.limit_rate > 0 {
        RateLimiter::new(job.limit_rate)
    } else {
        RateLimiter::unlimited()
    });
    let outs = out_path.to_string_lossy().to_string();
    let c = conn.clone();
    let hosts: Vec<String> = usable
        .iter()
        .take(n_sources)
        .map(|(u, _)| u.host.clone())
        .collect();
    // Digest, head bytes and any unavailability reason gathered from the stream,
    // for the `--no-save` path that has no file to read them back from.
    let mut stream_result: Option<(Option<String>, Vec<u8>, Option<String>)> = None;
    let res = {
        // Clear the phase line before the first frame: they share a terminal row.
        p.end_phase();
        p.set_baseline(resumed_from);
        // Checkpoint the resume record AS THE TRANSFER RUNS, not only at the end.
        //
        // Writing it only on exit means a ^C or a crash leaves no record, so the next
        // run cannot tell which bytes are held — and because the output is a SPARSE
        // file created at full length from the start, its apparent size is the whole
        // object even when almost nothing has arrived. Together those two facts made an
        // interrupted 2 MB transfer look like a finished 121.7 MiB download. The record
        // carries explicit ranges rather than a byte count, because positioned writes
        // land out of order and a count cannot describe a hole.
        let tick_sink = job.ticks.clone();
        let mut last_ckpt = Instant::now();
        let ckpt_path = out_path.clone();
        let ckpt_size = size;
        let ckpt_validator = validator.clone();
        let ckpt_url = job.urls[0].clone();
        let mut render = |sc: &Scheduler, done: u64| {
            // Carry the scheduler's own count out to the completeness check. A
            // monotonic max rather than a plain store: the observer is called on
            // every tick and the last tick before completion is not necessarily
            // the highest, so a bare store can report less than actually arrived.
            progress_obs.fetch_max(sc.bytes_held(), std::sync::atomic::Ordering::Relaxed);
            // No resume record under `--no-save`: there is no file to resume INTO,
            // so a sidecar would be a stray file created by the flag that promises
            // to create none. (Caught end-to-end, not by a unit test: the periodic
            // checkpoint runs inside this closure and is separate from the
            // completion path.)
            if !discarding && last_ckpt.elapsed().as_secs_f64() >= 2.0 {
                last_ckpt = Instant::now();
                let held = sc.held_ranges();
                if !held.is_empty() {
                    let rec = Sidecar {
                        size: ckpt_size,
                        validator: ckpt_validator.clone(),
                        done: held,
                        url: ckpt_url.clone(),
                    };
                    let _ = rec.save(&ckpt_path);
                }
            }
            let views = conn_views(sc, &hosts);
            // Publish live state for any UI driving this job. Done here rather than in
            // the transport because this closure already has the scheduler, so there is
            // no extra plumbing and no extra tick rate to reconcile.
            if let Some((id, tx)) = &tick_sink {
                let _ = tx.send(Tick {
                    id: *id,
                    done,
                    size: Some(size),
                    rate: views.iter().map(|v| v.rate).sum(),
                    requests: sc.stats.requests,
                    repairs: sc.stats.repairs,
                    conns: views
                        .iter()
                        .map(|v| {
                            let (lo, hi, pos) = v.range.unwrap_or((0, 0, 0));
                            ConnLine {
                                host: v.host.clone(),
                                lo,
                                hi,
                                pos,
                                rate: v.rate,
                                health: format!("{:?}", v.health).to_lowercase(),
                            }
                        })
                        .collect(),
                });
            }
            p.draw(
                done,
                &views,
                Counters {
                    requests: sc.stats.requests,
                    repairs: sc.stats.repairs,
                    reclaims: sc.stats.reclaims,
                    wasted: 0,
                    retries: 0,
                },
            );
        };
        // Under `--no-save` the sink stores nothing and carries the digest
        // instead, so the size, checksum, and format classification are all
        // computed from the STREAM. There is no file at any point.
        let sink = discard_sink.clone();
        // One limiter for the whole transfer, however many connections it opens:
        // `--limit-rate 1M` means the transfer uses 1 MB/s, not 1 MB/s per
        // connection. `--no-save` is capped too — the bytes still cross the
        // network, which is what the flag is about.
        let pace = hya_net::polite::Pace::shared(limiter.clone());
        match &sink {
            Some(sk) => {
                let r = hya_net::run_transfer_into(
                    c,
                    tgts,
                    &per,
                    size,
                    sk.clone(),
                    sched,
                    20,
                    &mut render,
                    pace,
                )
                .await;
                stream_result = sk.take_digest(size);
                r
            }
            None => {
                hya_net::run_transfer_paced(
                    c,
                    tgts,
                    &per,
                    size,
                    &outs,
                    sched,
                    20,
                    &mut render,
                    pace,
                )
                .await
            }
        }
    };
    // Two clocks, both reported. `elapsed` is the whole invocation; `transfer_elapsed`
    // is what the progress bar measured. Conflating them made a 1.7s transfer report
    // 4.0s with a throughput the network never delivered.
    let transfer_elapsed = t_transfer.elapsed().as_secs_f64();
    let elapsed = t_start.elapsed().as_secs_f64();
    drop(limiter);

    let (ok, requests) = match &res {
        Ok((_, r)) => (true, *r),
        Err(_) => (false, 0),
    };
    let counters = Counters {
        requests,
        ..Default::default()
    };

    // ---- verify ---------------------------------------------------------
    // Bytes ACTUALLY present, measured from the transfer that just ran.
    //
    // Every cheaper way of asking this question afterwards lies in some case.
    // The file's apparent length is the whole object from the very first byte,
    // because the output is a sparse file created at full length — using it here
    // once made an interrupted 2 MB transfer report "121.7 MiB on disk" and offer
    // to skip a download that had barely begun. Allocated blocks fix that but
    // read as the full size on a filesystem that does not store holes. And the
    // sidecar loaded BEFORE the transfer is stale by construction: reusing it
    // here reported a byte-exact `-c` resume as a failure (exit 1, `ok: false`,
    // `size` equal to the pre-resume byte count) while reporting a genuinely
    // incomplete resume the same way, so the tool's own output could not tell
    // the two apart.
    //
    // The scheduler is the one component that knows which bytes arrived, so its
    // live count — sampled through the observer above and seeded from its state
    // before the transfer — is what completeness is judged on. That count is on
    // the OBJECT's coordinate scale, which is the scale the comparison below
    // wants: in range mode the spans outside `[want_lo, want_hi)` are marked held
    // precisely so they are never requested, and they must keep counting as
    // present or a satisfied range would read as incomplete.
    //
    // With `--no-save` there is no file to measure, so the transfer's own success
    // is the only evidence — which is exactly what it should be, since the bytes
    // were verified as they streamed.
    let on_disk = if discarding {
        if ok {
            want_hi.min(size)
        } else {
            0
        }
    } else {
        progress.load(std::sync::atomic::Ordering::Relaxed)
    };
    // In range mode the sparse file is still `size` long but only the requested
    // span was fetched, so completion is judged on the scheduler's own accounting
    // rather than on file length.
    let complete = ok && on_disk >= want_hi.min(size);

    // ---- range mode: deliver the requested span, not the object's extent ----
    //
    // Positioned writes need a file of the OBJECT's length to write into, because
    // a range lands at its true offset. In range mode only `[want_lo, want_hi)`
    // is ever fetched, so the rest of that extent is a hole — and a hole reads
    // back as zeros. `hydra -r 0-1023` delivered a 34 041-byte file whose first
    // 1 024 bytes were correct and whose remaining 33 017 were zeros: right
    // prefix, plausible size, silently wrong file. The same failure shape as the
    // sparse-file and truncating-reopen bugs recorded elsewhere in this project.
    //
    // curl -r writes only the requested bytes, and that is the only reading of
    // "retrieve only this byte range" that makes sense. A suffix range must give
    // a 512-byte file, not a 34 041-byte file whose last 512 bytes are real.
    //
    // Done BEFORE the digest deliberately: a digest must describe the bytes the
    // user receives. Hashing the padded extent would report a checksum for a file
    // that no longer exists after truncation.
    if partial && complete && !discarding && !job.to_stdout {
        if let Err(e) = extract_span(&out_path, want_lo, want_hi) {
            return failed(
                &job,
                size,
                format!(
                    "range mode: cannot reduce {} to its span: {e}",
                    out_path.display()
                ),
            );
        }
    }

    // What the user was handed. In range mode that is the span, not the object:
    // reporting "33.2 KiB in 1.2s" for a 1 KiB range described a transfer that
    // did not happen, and the throughput derived from it was wrong by the same
    // factor.
    let delivered = if partial {
        want_hi.saturating_sub(want_lo)
    } else {
        on_disk
    };

    // ---- per-chunk verification and targeted refetch ---------------------
    //
    // Runs after the transfer rather than during it because a chunk is only
    // checkable once its last byte has landed, and the scheduler may deliver any
    // part of any chunk on any connection. Verifying here costs one sequential
    // read of a file already in page cache, and it buys the thing the whole-file
    // digest cannot: WHICH chunk is wrong.
    //
    // A mismatch is repaired by refetching that chunk alone — preferring a
    // different source than the one that served it, since a mirror that served
    // corrupt bytes once is the least likely to serve them correctly now. This is
    // what BitTorrent and Metalink already do, and while the source is reachable
    // it beats carrying parity by roughly 400x.
    let mut chunk_report: Option<String> = None;
    if let Some(mpath) = job.chunk_digests.clone() {
        if complete && !discarding {
            match verify_and_repair_chunks(&conn, &usable, &out_path, &mpath, &job, &mut p).await {
                Ok(r) => chunk_report = Some(r),
                Err(e) => return failed(&job, size, e),
            }
        }
    }

    let digest = if !complete {
        None
    } else if discarding {
        // Computed from the stream as it passed through the discarding sink.
        // `None` here means the digest genuinely could not be established (the
        // ranges arrived too far out of order to hash within the buffer budget),
        // and the reason is reported rather than a wrong value substituted.
        let d = stream_result.as_ref().and_then(|(d, _, _)| d.clone());
        if d.is_none() {
            if let Some(reason) = stream_result.as_ref().and_then(|(_, _, r)| r.clone()) {
                if !job.quiet {
                    eprintln!("hydra: {reason}");
                }
            }
        }
        d
    } else {
        sha256_file(&out_path)
    };
    let checksum_ok = match (&job.checksum, &digest) {
        (Some(want), Some(got)) => {
            let want = want.trim_start_matches("sha256:").to_ascii_lowercase();
            Some(want == *got)
        }
        (Some(_), None) => Some(false),
        _ => None,
    };

    // ---- classify what actually arrived --------------------------------
    //
    // Classification happens AFTER the transfer because the payload is the only
    // trustworthy signal, and it is read from the head of the finished file
    // rather than from a separate probe request.
    let detection = {
        let mut head = vec![0u8; 0];
        if discarding {
            // The leading bytes were retained by the stream observer; there is no
            // file to reopen. Classification is therefore unchanged by --no-save,
            // which is the point of the flag: probe what a URL serves without
            // leaving anything behind.
            if let Some((_, h, _)) = &stream_result {
                head = h.clone();
            }
        } else if complete {
            use std::io::Read as _;
            if let Ok(mut fh) = std::fs::File::open(&out_path) {
                let mut buf = vec![0u8; 8192];
                if let Ok(n) = fh.read(&mut buf) {
                    buf.truncate(n);
                    head = buf;
                }
            }
        }
        detect_format(&head, &name, served_type.as_deref())
    };
    if let Some(msg) = &detection.conflict {
        // Worth saying even though nothing failed: an HTML body delivered where an
        // archive was expected is the signature of a captive portal or an error
        // page saved as a file, and both the byte count and the status look fine.
        if !job.quiet {
            eprintln!("hydra: warning: {msg}");
            if detection.looks_intercepted() {
                eprintln!(
                    "  the saved file is a web page, not the object requested \
                     (captive portal, login wall, or an error page served with status 200)"
                );
            }
        }
    } else if let Some(f) = detection.format {
        // At -v, name the format. At -vv, explain it: the description is aimed at
        // someone deciding what to do with the file they just fetched.
        p.event(1, &format!("detected {} ({})", f.name, f.category.as_str()));
        p.event(2, &format!("  {}", f.hint()));
    }

    // ---- sort into a category directory, if asked -----------------------
    let out_path = if job.sort_by_type && complete && detection.category != Category::Unknown {
        let base = job.output_dir.clone().unwrap_or_else(|| PathBuf::from("."));
        let dir = base.join(detection.category.directory());
        match std::fs::create_dir_all(&dir) {
            Ok(()) => {
                let dest = dir.join(out_path.file_name().unwrap_or_default());
                match std::fs::rename(&out_path, &dest) {
                    Ok(()) => {
                        p.event(0, &format!("sorted into {}", dir.display()));
                        dest
                    }
                    Err(e) => {
                        eprintln!("hydra: could not move into {}: {e}", dir.display());
                        out_path
                    }
                }
            }
            Err(e) => {
                eprintln!("hydra: could not create {}: {e}", dir.display());
                out_path
            }
        }
    } else {
        out_path
    };

    if job.server_response {
        p.end_phase();
        // Verbatim first, interpretation second. curl -i prints what arrived; the
        // paraphrase below is a convenience, and when the two disagree the raw block
        // is the evidence.
        print_exchange(&probe_info);
        println!("hydra: interpretation");
        println!("  size: {size}");
        println!("  validator: {}", validator.as_deref().unwrap_or("none"));
        println!(
            "  validator strength: {}",
            if probe_info.weak_validator {
                "weak (single-source only: a weak validator may compare equal across \
                 different bytes, so cross-mirror assembly is not sound)"
            } else if validator.is_some() {
                "strong (cross-mirror assembly permitted)"
            } else {
                "none"
            }
        );
        println!(
            "  range support: {}",
            if probe_info.ranges { "yes" } else { "no" }
        );
        println!("  sources accepted: {}", usable.len());
        println!("  connections used: {n_conns}");
    }

    if discarding {
        // No file exists, so there is nothing to resume and no record to keep.
        // Writing a sidecar here would recreate exactly the litter the flag is
        // supposed to avoid.
    } else if complete && checksum_ok != Some(false) {
        Sidecar::remove(&out_path);
    } else {
        // Keep the sidecar so `-c` can pick up where this left off.
        let sc = Sidecar {
            size,
            validator: validator.clone(),
            done: vec![(0, on_disk.min(size))],
            url: job.urls[0].clone(),
        };
        let _ = sc.save(&out_path);
    }

    // `--no-save` needs no cleanup: nothing was ever created. The digest and the
    // format classification came from the stream (see `stream_digest`), which is
    // what made deleting-afterwards unnecessary. The earlier create-write-hash-
    // delete implementation is what left a 45 MB file behind when a run was
    // interrupted.

    // --stdout: stream the assembled object out, then remove the temporary file.
    //
    // This deliberately does NOT stream as bytes arrive. Positioned writes mean
    // ranges land out of order, so the file is only correct once complete;
    // emitting partial state to a pipe would hand the consumer bytes in the wrong
    // order. Saying so is better than appearing to stream and corrupting a pipe.
    if job.to_stdout && complete {
        use std::io::Write as _;
        match std::fs::read(&out_path) {
            Ok(bytes) => {
                let mut so = std::io::stdout().lock();
                let _ = so.write_all(&bytes);
                let _ = so.flush();
                let _ = std::fs::remove_file(&out_path);
            }
            Err(e) => eprintln!("hydra: cannot stream to stdout: {e}"),
        }
    }

    // --remote-time: only possible when the server offered a date-form validator.
    // An ETag is opaque and carries no time, so the flag is a silent no-op there
    // rather than a fabricated timestamp.
    if job.remote_time && complete {
        // `Last-Modified` first, and on its own terms. Reading the collapsed
        // `validator` here meant any server that also sent an ETag — GitHub, S3,
        // most CDNs — had its date thrown away before the flag ran, and the tool
        // reported "no date-form validator" about a response that carried one.
        // The validator is still consulted as a fallback, for the servers that
        // send only a date: there it IS the Last-Modified value.
        match last_modified
            .as_deref()
            .or(validator.as_deref())
            .and_then(hya_net::polite::parse_http_date)
        {
            Some(secs) => {
                let _ = set_mtime(&out_path, secs);
            }
            None => p.event(
                1,
                "--remote-time: server sent no Last-Modified header, skipped",
            ),
        }
    }

    p.finish(
        delivered,
        complete && checksum_ok != Some(false),
        counters,
        digest.as_deref(),
    );
    // At default verbosity a format note is printed only when it is a WARNING —
    // the served bytes are not what was asked for. "gzip stream — compresses a
    // single stream…" is a description of a successful download and belongs at
    // `-v`; "this is an HTML page where a file was expected" is the difference
    // between a good file and a captive-portal page saved with status 200, and
    // suppressing it would hide the failure this project cares most about.
    if !job.quiet {
        if let Some(f) = detection.format {
            // `Markup` is the tell: an HTML page delivered where a file was
            // expected is the captive-portal / login-wall / error-page-with-200
            // case. A conflict between the sniffed type and the served
            // Content-Type is the other.
            let suspicious = detection.conflict.is_some() || f.category == Category::Markup;
            // At default verbosity this is a VALUE, not a sentence: `HTML page`
            // rather than `HTML page — A web page. Where a real file was
            // expected, this usually means...`. The explanation is real and worth
            // having, but it belongs at `-v`; a user who has seen it once does not
            // need the paragraph on every subsequent run.
            let note = if job.verbose > 0 {
                f.hint()
            } else {
                f.label().to_string()
            };
            if suspicious || job.verbose > 0 {
                // Never onto stdout when the object is going there: appending a
                // hint to a piped archive corrupts it (measured: 344 extra bytes
                // on a 34 041-byte .tar.gz, which then failed to decompress).
                // Under `--logfile` it goes to the file with the rest of the run.
                p.note(&format!("  {note}"), suspicious);
            }
        }
    }
    if checksum_ok == Some(false) {
        p.note(
            "  checksum MISMATCH: the delivered bytes are not the bytes requested",
            true,
        );
    }

    // ---- emit a manifest for what arrived --------------------------------
    //
    // Only for a download that verified. A manifest over bytes we already
    // believe are wrong would record the corruption as if it were the truth,
    // and every later check against it would agree.
    if let Some(mpath) = &job.emit_manifest {
        if complete && checksum_ok != Some(false) && !discarding {
            let cs = job
                .chunk_size
                .unwrap_or(hya_net::manifest::DEFAULT_CHUNK)
                .max(1);
            match hya_net::manifest::from_file(
                &out_path.to_string_lossy(),
                cs,
                hya_net::manifest::ChunkAlgo::Blake3,
                Some(job.urls[0].clone()),
                validator.clone(),
            ) {
                Ok(m) => match std::fs::write(mpath, m.to_json()) {
                    Ok(()) => {
                        if !job.quiet {
                            eprintln!(
                                "  manifest: {} chunks of {} bytes -> {}",
                                m.chunks.digests.len(),
                                cs,
                                mpath.display()
                            );
                        }
                    }
                    Err(e) => eprintln!("hydra: cannot write manifest {}: {e}", mpath.display()),
                },
                Err(e) => eprintln!("hydra: cannot build manifest: {e}"),
            }
        } else if !job.quiet {
            eprintln!(
                "hydra: --emit-manifest skipped: a manifest is only written for a download \
                 that verified"
            );
        }
    }
    if let Some(r) = &chunk_report {
        if !job.quiet {
            eprintln!("  chunk integrity: {r}");
        }
    }

    Outcome {
        url: job.urls[0].clone(),
        output: outs,
        // The bytes delivered, which in range mode is the span rather than the
        // object's length. A consumer of `--json` comparing `size` against the
        // file it just received must find them equal.
        size: delivered,
        elapsed_s: elapsed,
        transfer_s: transfer_elapsed,
        setup_s: (elapsed - transfer_elapsed).max(0.0),
        throughput_bps: if transfer_elapsed > 0.0 {
            // Throughput of the TRANSFER, not of the invocation: dividing by setup time
            // too reports a rate the network never achieved.
            delivered as f64 / transfer_elapsed
        } else {
            0.0
        },
        requests,
        connections: n_conns,
        delta_s: delta,
        sha256: digest,
        checksum_ok,
        resumed_from,
        ok: complete && checksum_ok != Some(false),
        note: res.err().map(|e| e.to_string()),
        format: detection.format.map(|f| f.name.to_string()),
        category: Some(detection.category.as_str().to_string()),
        format_conflict: detection.conflict,
        format_label: detection.format.map(|f| f.label().to_string()),
        format_description: detection.format.map(|f| f.description().to_string()),
        category_description: Some(detection.category.description().to_string()),
    }
}

impl Outcome {
    /// An outcome for a path that ended before any bytes moved: a refusal, a skip,
    /// or an unchanged object. Keeps the four early-return sites from each
    /// carrying their own copy of every field.
    fn stopped(job: &Job, output: String, size: u64, ok: bool, note: &str) -> Self {
        Self {
            url: job.urls.first().cloned().unwrap_or_default(),
            output,
            size,
            ok,
            note: Some(note.to_string()),
            ..Self::default()
        }
    }
}

fn failed(job: &Job, size: u64, why: String) -> Outcome {
    // `-q` silences normal output; `--show-error` carves out the exception for
    // failures, as curl's `-s -S` pair does. Without it a quiet run that failed
    // was indistinguishable from a quiet run that succeeded: empty stdout, empty
    // stderr, and only the exit code to tell them apart.
    if !job.quiet || job.show_error {
        eprintln!("hydra: {why}");
    }
    Outcome {
        url: job.urls.first().cloned().unwrap_or_default(),
        size,
        note: Some(why),
        ..Outcome::default()
    }
}

/// Unused by the engine, but the renderer needs the type in scope for callers
/// that build views from a live scheduler.
pub fn conn_views(sched: &Scheduler, hosts: &[String]) -> Vec<ConnView> {
    (0..sched.n_conns())
        .map(|j| ConnView {
            idx: j,
            host: hosts.get(sched.conn_source(j)).cloned().unwrap_or_default(),
            range: sched.conn_range(j),
            rate: sched.conn_rate(j),
            health: sched.conn_health(j),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression test for a sentinel-encoding bug: "the last 512 bytes" was
    /// encoded as `u64::MAX - 512` and recognised by a `> u64::MAX / 2` test,
    /// which mis-resolved and fetched bytes from the middle of the object
    /// (measured landing at exactly size/2 instead of the tail).
    #[test]
    fn suffix_range_resolves_to_the_actual_tail() {
        let size = 2_957_812u64;
        assert_eq!(
            RangeSpec::Suffix(512).resolve(size),
            Some((size - 512, size)),
            "a suffix range must be the LAST n bytes"
        );
        // A suffix larger than the object is the whole object, not an underflow.
        assert_eq!(RangeSpec::Suffix(size + 99).resolve(size), Some((0, size)));
    }

    #[test]
    fn closed_ranges_are_inclusive_as_http_spells_them() {
        // curl -r 0-1023 is 1024 bytes.
        assert_eq!(RangeSpec::Closed(0, 1023).resolve(10_000), Some((0, 1024)));
        assert_eq!(
            RangeSpec::Closed(1000, 2023).resolve(10_000),
            Some((1000, 2024))
        );
        // Clamped to the object rather than requesting past the end.
        assert_eq!(
            RangeSpec::Closed(9990, 99_999).resolve(10_000),
            Some((9990, 10_000))
        );
    }

    #[test]
    fn open_ended_range_runs_to_the_end() {
        assert_eq!(RangeSpec::From(4096).resolve(10_000), Some((4096, 10_000)));
    }

    #[test]
    fn empty_and_degenerate_ranges_are_rejected() {
        assert_eq!(
            RangeSpec::From(10_000).resolve(10_000),
            None,
            "start at EOF is empty"
        );
        assert_eq!(RangeSpec::From(99_999).resolve(10_000), None);
        assert_eq!(RangeSpec::Closed(500, 499).resolve(10_000), None);
        assert_eq!(RangeSpec::Suffix(0).resolve(10_000), None);
    }

    /// `--no-save` must never create the output file.
    #[test]
    fn no_save_never_creates_a_file() {
        let mut job = default_job();
        job.no_save = true;
        assert_eq!(
            output_target(&job, "/tmp/hydra_should_not_exist.bin"),
            OutputTarget::Discard,
            "--no-save must resolve to a discarding sink, not a file to delete later"
        );
    }

    /// `--no-save --stdout` still needs storage: positioned writes land out of
    /// order, so the object is only correct once complete and cannot be streamed
    /// as it arrives. The temporary file is removed after streaming.
    #[test]
    fn no_save_with_stdout_still_needs_a_staging_file() {
        let mut job = default_job();
        job.no_save = true;
        job.to_stdout = true;
        assert_eq!(
            output_target(&job, "/tmp/hydra_stage.bin"),
            OutputTarget::Stdout("/tmp/hydra_stage.bin".into())
        );
    }

    #[test]
    fn a_plain_job_writes_a_file() {
        let job = default_job();
        assert_eq!(
            output_target(&job, "/tmp/hydra_plain.bin"),
            OutputTarget::File("/tmp/hydra_plain.bin".into())
        );
    }

    /// A scratch object for the span tests: `size` patterned bytes (modulus
    /// 251, a prime, so no page-aligned repeat can mask an offset error) in a
    /// private temp directory. Returns the directory (for cleanup) and the
    /// file path.
    fn span_scratch_object(size: usize) -> (std::path::PathBuf, std::path::PathBuf, Vec<u8>) {
        let dir = std::env::temp_dir().join(format!("hydra_span_{}", scratch_name()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("obj.bin");
        let mut whole = vec![0u8; size];
        for (i, b) in whole.iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }
        std::fs::write(&p, &whole).unwrap();
        (dir, p, whole)
    }

    /// Range mode must deliver the SPAN, not the object's extent.
    ///
    /// The transfer writes `[lo, hi)` at true offsets inside a file the length of
    /// the whole object, so everything outside the span is a hole reading as
    /// zeros. `hydra -r 0-1023` delivered 34 041 bytes: 1 024 correct, 33 017
    /// zeros. Right prefix, plausible size, silently wrong file.
    #[test]
    fn a_prefix_range_is_cut_down_to_its_span() {
        let (dir, p, whole) = span_scratch_object(4096);

        extract_span(&p, 0, 1024).unwrap();
        let got = std::fs::read(&p).unwrap();
        assert_eq!(
            got.len(),
            1024,
            "must be the span's length, not the object's"
        );
        assert_eq!(
            got[..],
            whole[..1024],
            "must be the object's first 1024 bytes"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A suffix range must move the span to the front, not merely truncate.
    #[test]
    fn a_suffix_range_moves_its_span_to_the_front() {
        let (dir, p, whole) = span_scratch_object(4096);

        extract_span(&p, 3584, 4096).unwrap();
        let got = std::fs::read(&p).unwrap();
        assert_eq!(got.len(), 512);
        assert_eq!(
            got[..],
            whole[3584..],
            "a suffix range must yield the LAST bytes, at offset 0"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The copy is blocked rather than read whole, so a span longer than the
    /// block size must still come out byte-exact.
    #[test]
    fn a_span_larger_than_one_copy_block_is_exact() {
        // 3 MiB object, 2.5 MiB span starting mid-block: crosses the 1 MiB
        // copy block boundary at a non-multiple offset.
        let (dir, p, whole) = span_scratch_object(3 << 20);

        let (lo, hi) = (300_000u64, 300_000 + (5 << 19));
        extract_span(&p, lo, hi).unwrap();
        let got = std::fs::read(&p).unwrap();
        assert_eq!(got.len() as u64, hi - lo);
        assert_eq!(got[..], whole[lo as usize..hi as usize]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Ensure metadata probe follows redirects to the target object.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn public_probe_follows_a_redirect_to_the_object() {
        let net = hya_net::origin::OriginSet::new();
        let (real_port, _real) = net.spawn(64 * 1024, 1_000_000);
        let (hop_port, _hop) =
            net.spawn_redirecting(0, 1_000_000, &format!("http://127.0.0.1:{real_port}/obj"));

        // `--no-proxy`: the ambient http_proxy of a CI sandbox must not divert
        // an in-memory origin lookup.
        let args = crate::cli::Cli::parse_with_queries([
            "hydra",
            "--no-proxy",
            &format!("http://127.0.0.1:{hop_port}/obj"),
        ])
        .unwrap();
        let u = Url::parse(&args.urls[0]).unwrap();
        let (pr, final_url) = probe_public(&net, &u, &args)
            .await
            .expect("probe through the redirect");
        assert_eq!(pr.size, 64 * 1024, "size must come from the object");
        assert!(
            !pr.is_redirect(),
            "the reported response must be the final one"
        );
        assert_eq!(
            final_url.port, real_port,
            "must land on the redirect target"
        );
    }

    #[test]
    fn verify_scratch_names_are_unique_per_call_not_per_process() {
        // Two concurrent verifications sharing one scratch file made each read back the
        // OTHER object's bytes and report a mismatch on a byte-identical file. The name
        // must therefore vary within a process, not just across processes.
        let mut seen = std::collections::HashSet::new();
        for _ in 0..64 {
            assert!(
                seen.insert(scratch_name()),
                "a scratch name repeated within one process"
            );
        }
    }
}
