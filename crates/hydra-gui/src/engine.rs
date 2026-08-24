// Copyright (C) 2026 Javad Rajabzadeh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The download engine bridge: GUI on one side, `hya-core`/`hya-net` on the
//! other.
//!
//! One background thread runs a tokio runtime that owns every transfer. The
//! GUI talks to it through a command channel and hears back through an event
//! channel that an iced subscription drains, so the widget tree never blocks
//! on a socket and the engine never touches a widget.
//!
//! Transfer strategy follows what the CLI settled on:
//! range-capable origins get the `hya-core` scheduler with N connections
//! (default 8) and the stall-timeout clamp measured there; servers
//! without ranges fall back to one streaming GET. Pause/cancel go through
//! `run_transfer_cancellable`'s stop flag so sockets actually close, and the
//! received spans are re-`mark_done`d on resume — including across app restarts.

use crate::model::{ConnRow, DlId};
use hya_core::{Capability, Scheduler, Source};
use hya_net::polite::RateLimiter;
use hya_net::{probe_resilient, Probe, SparseSink, Target, TlsCapableConnector};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

// ------------------------------------------------------------------ protocol

#[derive(Clone, Debug)]
pub struct StartSpec {
    pub id: DlId,
    pub url: String,
    pub auth: Option<(String, String)>,
    pub conns: usize,
    pub user_agent: String,
    pub temp_path: String,
    pub final_path: String,
    /// Spans already on disk from a previous run; resumed via `mark_done`.
    pub held: Vec<(u64, u64)>,
    /// Size the previous source reported. A mirror serving a different size
    /// is a different object: held spans are dropped instead of spliced.
    pub expected_size: Option<u64>,
    /// `Cookie:` header value, verbatim.
    pub cookies: Option<String>,
    /// Aggregate cap in bytes/sec; `None` = unlimited (can be changed live).
    pub limit: Option<u64>,
    /// Start at one connection and let the in-band ramp admit more while
    /// they pay for themselves; `conns` becomes a ceiling, not a target.
    pub adaptive: bool,
}

#[derive(Debug)]
pub enum Cmd {
    Start(Box<StartSpec>),
    /// Pause and Cancel are one engine operation (stop the sockets, report the
    /// snapshot); the GUI decides whether the item is "Paused" or removed.
    Stop(DlId),
    SetLimit(DlId, Option<u64>),
    /// Re-aim where the finished file lands. The File Info dialog edits the
    /// name/directory while the transfer is already running in the
    /// background; the new destination takes effect at completion.
    SetFinalPath(DlId, String),
    /// Internal: a transfer task ended (any outcome); drop its live handles.
    /// Without this, entries for normally-finished transfers stayed in the
    /// live map for the process lifetime. Carries the task's own cancel flag
    /// so it can only evict ITS entry — after a quick pause/resume the map
    /// already holds the successor transfer under the same id.
    Done(DlId, Arc<AtomicBool>),
}

#[derive(Clone, Debug)]
pub enum Event {
    Probed {
        id: DlId,
        size: Option<u64>,
        ranges: bool,
        file_name: Option<String>,
    },
    Status {
        id: DlId,
        line: String,
    },
    Progress {
        id: DlId,
        done: u64,
        rate: f64,
        eta: Option<u64>,
        conns: Vec<ConnRow>,
        held: Vec<(u64, u64)>,
    },
    Finished {
        id: DlId,
        elapsed: f64,
        size: u64,
    },
    /// Pause/cancel acknowledged; final byte snapshot for persistence.
    Stopped {
        id: DlId,
        done: u64,
        held: Vec<(u64, u64)>,
    },
    Failed {
        id: DlId,
        error: String,
        done: u64,
        held: Vec<(u64, u64)>,
        /// The OS refused filesystem access (TCC/permissions): the GUI offers
        /// the system folder picker, whose selection implicitly grants access.
        permission_denied: bool,
    },
}

// ------------------------------------------------------------------ handles

struct Live {
    cancel: Arc<AtomicBool>,
    limiter: Arc<RateLimiter>,
    final_path: Arc<Mutex<String>>,
}

static POWER_SAVE: AtomicBool = AtomicBool::new(false);

/// Coarser scheduler ticks and UI-event cadence; applies to transfers
/// started after the switch (running ones keep their tick, their emit rate
/// adapts live).
pub fn set_power_save(on: bool) {
    POWER_SAVE.store(on, Ordering::Relaxed);
}

fn emit_interval_ms() -> u128 {
    if POWER_SAVE.load(Ordering::Relaxed) {
        500
    } else {
        100
    }
}

static CMDS: OnceLock<UnboundedSender<Cmd>> = OnceLock::new();
static EVENTS: Mutex<Option<UnboundedReceiver<Event>>> = Mutex::new(None);

/// Send a command to the engine (started lazily on first use).
pub fn send(cmd: Cmd) {
    let tx = CMDS.get_or_init(spawn_engine);
    let _ = tx.send(cmd);
}

/// Start the engine thread without sending anything, so the event receiver
/// exists before the first subscription poll.
pub fn ensure_started() {
    let _ = CMDS.get_or_init(spawn_engine);
}

/// The subscription side: take the event receiver (once).
pub fn take_events() -> Option<UnboundedReceiver<Event>> {
    EVENTS.lock().ok().and_then(|mut g| g.take())
}

fn spawn_engine() -> UnboundedSender<Cmd> {
    let (cmd_tx, mut cmd_rx) = unbounded_channel::<Cmd>();
    let (ev_tx, ev_rx) = unbounded_channel::<Event>();
    *EVENTS.lock().unwrap() = Some(ev_rx);

    // Transfer tasks report their own completion back through the command
    // channel so the live map sheds finished entries.
    let cmd_tx2 = cmd_tx.clone();
    std::thread::Builder::new()
        .name("hydra-engine".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("engine runtime");
            rt.block_on(async move {
                let mut live: HashMap<DlId, Live> = HashMap::new();
                while let Some(cmd) = cmd_rx.recv().await {
                    match cmd {
                        Cmd::Start(spec) => {
                            let cancel = Arc::new(AtomicBool::new(false));
                            // 0 = unlimited. The limiter is handed to the
                            // transfer either way: `Pace` reads its rate live,
                            // so Speed Limiter switched on mid-download binds
                            // this transfer without restarting it.
                            let limiter = Arc::new(RateLimiter::new(spec.limit.unwrap_or(0)));
                            let final_path = Arc::new(Mutex::new(spec.final_path.clone()));
                            live.insert(
                                spec.id,
                                Live {
                                    cancel: cancel.clone(),
                                    limiter: limiter.clone(),
                                    final_path: final_path.clone(),
                                },
                            );
                            let tx = ev_tx.clone();
                            let done_tx = cmd_tx2.clone();
                            crate::log::log(&format!("start #{} {}", spec.id, spec.url));
                            tokio::spawn(async move {
                                let id = spec.id;
                                let flag = cancel.clone();
                                run_download(*spec, cancel, limiter, final_path, tx).await;
                                let _ = done_tx.send(Cmd::Done(id, flag));
                            });
                        }
                        Cmd::Stop(id) => {
                            if let Some(l) = live.get(&id) {
                                l.cancel.store(true, Ordering::Relaxed);
                            }
                            crate::log::log(&format!("stop #{id}"));
                        }
                        Cmd::SetLimit(id, limit) => {
                            crate::log::debug(&format!("#{id} limit -> {limit:?}"));
                            if let Some(l) = live.get(&id) {
                                l.limiter.set_rate(limit.unwrap_or(0));
                            }
                        }
                        Cmd::SetFinalPath(id, path) => {
                            crate::log::debug(&format!("#{id} final path -> {path}"));
                            if let Some(l) = live.get(&id) {
                                if let Ok(mut g) = l.final_path.lock() {
                                    *g = path;
                                }
                            }
                        }
                        Cmd::Done(id, flag) => {
                            if live
                                .get(&id)
                                .map(|l| Arc::ptr_eq(&l.cancel, &flag))
                                .unwrap_or(false)
                            {
                                live.remove(&id);
                            }
                        }
                    }
                    live.retain(|_, l| !l.cancel.load(Ordering::Relaxed));
                }
            });
        })
        .expect("engine thread");
    cmd_tx
}

/// The one connector for the whole process. Its connection pool, TLS
/// session cache (`Resumption::in_memory_sessions`) and parsed root store
/// are designed to outlive a single transfer — `tls.rs` documents 1.6–2.0 s
/// of setup reused when the probe's handshake feeds the transfer. Building
/// a connector per transfer (as this file used to) discarded all three.
fn shared_connector() -> Result<Arc<TlsCapableConnector>, String> {
    static CONNECTOR: OnceLock<Result<Arc<TlsCapableConnector>, String>> = OnceLock::new();
    CONNECTOR
        .get_or_init(|| {
            TlsCapableConnector::new()
                .map(Arc::new)
                .map_err(|e| e.to_string())
        })
        .clone()
}

/// What a link turns out to be, once its redirects have been resolved.
#[derive(Clone, Debug)]
pub struct LinkMeta {
    /// `None` when the server would not state one.
    pub size: Option<u64>,
    /// The name the object should be saved under: `Content-Disposition` if the
    /// server named one, else the last path segment of the FINAL URL.
    pub file_name: String,
}

/// Probe a link for the batch dialog's File Name and Size columns.
///
/// Resolves redirects first, including the ones expressed in HTML (see
/// [`hya_net::redirect`]): a referrer stripper such as `href.li/?<url>` has no
/// filename in its own path, and reading the column off the pasted URL listed
/// every such link as `index.html` with no size.
///
/// Bounded: a pasted 500-link batch must not open 500 concurrent
/// handshakes (fd limits, origin rate limiting). A small global gate keeps
/// the fan-out polite; the shared connector reuses connections per host.
pub async fn probe_link(url: String, user_agent: String) -> Option<LinkMeta> {
    static GATE: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
    let gate = GATE
        .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(6)))
        .clone();
    let _permit = gate.acquire_owned().await.ok()?;
    let connector = shared_connector().ok()?;
    let mut url = url;
    for _ in 0..10 {
        let u = parse_url(&url).ok()?;
        let base = if u.tls {
            Target::direct_tls(&u.host, u.port, &u.path)
        } else {
            Target::direct(&u.host, u.port, &u.path)
        };
        let t = base.with_headers(vec![], Some(user_agent.clone()));
        let p = probe_resilient(connector.as_ref(), &t).await.ok()?;
        if p.is_redirect() {
            url = join_url(&u, p.location.as_deref().unwrap_or(""))?;
            continue;
        }
        if p.maybe_redirector() {
            if let Some(next) = hya_net::html_redirect(connector.as_ref(), &t)
                .await
                .and_then(|loc| join_url(&u, &loc))
            {
                url = next;
                continue;
            }
        }
        if p.status >= 300 {
            return None;
        }
        return Some(LinkMeta {
            size: (p.size > 0).then_some(p.size),
            file_name: p
                .suggested_filename()
                .unwrap_or_else(|| file_name_from_url(&url)),
        });
    }
    None
}

/// Make `dir` exist and be writable, healing one specific corruption seen in
/// the field: an EMPTY directory owned by another user (root) sitting in a
/// writable parent — deletable from the parent, so drop and recreate it.
fn ensure_writable_dir(dir: &std::path::Path) {
    let _ = std::fs::create_dir_all(dir);
    let probe = dir.join(".hydra-write-probe");
    match std::fs::write(&probe, b"x") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
        }
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            let empty = std::fs::read_dir(dir)
                .map(|mut i| i.next().is_none())
                .unwrap_or(false);
            if empty && std::fs::remove_dir(dir).is_ok() {
                let _ = std::fs::create_dir_all(dir);
                crate::log::warn(&format!(
                    "recreated unwritable empty directory {}",
                    dir.display()
                ));
            }
        }
        Err(_) => {}
    }
}

/// Users see io errors verbatim; "os error 13" deserves an explanation.
fn friendly_io(e: &std::io::Error, path: &str) -> String {
    if e.kind() == std::io::ErrorKind::PermissionDenied {
        format!(
            "{} ({path})",
            crate::i18n::tr(
                "Access to the download folder was denied. Allow Hydra under System Settings > Privacy & Security > Files and Folders, or choose another folder."
            )
        )
    } else {
        e.to_string()
    }
}

// ------------------------------------------------------------------ URL bits

#[derive(Clone, Debug)]
pub struct ParsedUrl {
    pub tls: bool,
    /// `ftp://` — served by the single-connection FTP fetcher.
    pub ftp: bool,
    pub host: String,
    pub port: u16,
    pub path: String,
}

pub fn parse_url(url: &str) -> Result<ParsedUrl, String> {
    let url = url.trim();
    let (tls, ftp, rest) = if let Some(r) = url.strip_prefix("https://") {
        (true, false, r)
    } else if let Some(r) = url.strip_prefix("http://") {
        (false, false, r)
    } else if let Some(r) = url.strip_prefix("ftp://") {
        (false, true, r)
    } else {
        return Err(crate::i18n::tr(
            "Only http://, https:// and ftp:// addresses are supported",
        ));
    };
    let rest = rest.split('#').next().unwrap_or(rest);
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    // Strip userinfo if pasted in the address.
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) && !p.is_empty() => {
            (h, p.parse::<u16>().map_err(|_| "bad port".to_string())?)
        }
        _ => (
            authority,
            if ftp {
                21
            } else if tls {
                443
            } else {
                80
            },
        ),
    };
    if host.is_empty() {
        return Err(crate::i18n::tr("Address has no host name"));
    }
    Ok(ParsedUrl {
        tls,
        ftp,
        host: host.to_string(),
        port,
        path: path.to_string(),
    })
}

/// Best-effort display file name for a URL (last path segment, %-decoded).
/// Resolve a redirect target against the URL that served it.
///
/// One implementation for both redirect kinds and both probe paths: a
/// `Location` header and an HTML redirector's target are the same sort of
/// reference and are allowed the same spellings — absolute, protocol-relative
/// (`//host/path`), root-relative, or relative to the current directory.
///
/// `None` for anything this client cannot fetch, which includes the
/// `javascript:`/`data:` targets that appear in redirector markup: refusing is
/// how a caller learns the chain ends here rather than silently fetching the
/// wrong thing.
pub fn join_url(base: &ParsedUrl, loc: &str) -> Option<String> {
    let loc = loc.trim();
    if loc.is_empty() {
        return None;
    }
    let scheme = if base.tls { "https" } else { "http" };
    let lower = loc.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Some(loc.to_string());
    }
    if let Some(rest) = loc.strip_prefix("//") {
        return Some(format!("{scheme}://{rest}"));
    }
    if loc.starts_with('/') {
        return Some(format!("{scheme}://{}:{}{}", base.host, base.port, loc));
    }
    // A scheme this client has no transport for. Not an address to chase.
    if lower
        .split('/')
        .next()
        .map(|h| h.contains(':'))
        .unwrap_or(false)
    {
        return None;
    }
    let dir = base
        .path
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .rsplit_once('/')
        .map(|(d, _)| d)
        .unwrap_or("");
    Some(format!("{scheme}://{}:{}{dir}/{loc}", base.host, base.port))
}

pub fn file_name_from_url(url: &str) -> String {
    let path = parse_url(url).map(|p| p.path).unwrap_or_default();
    let seg = path
        .split('?')
        .next()
        .unwrap_or("")
        .rsplit('/')
        .next()
        .unwrap_or("");
    let name = percent_decode(seg);
    if name.is_empty() {
        "index.html".into()
    } else {
        name
    }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push(h << 4 | l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

fn target_for(u: &ParsedUrl, spec: &StartSpec) -> Target {
    let mut headers = Vec::new();
    if let Some((user, pass)) = &spec.auth {
        headers.push(format!(
            "Authorization: Basic {}",
            base64(format!("{user}:{pass}").as_bytes())
        ));
    }
    if let Some(c) = spec.cookies.as_deref().filter(|c| !c.trim().is_empty()) {
        headers.push(format!("Cookie: {}", c.trim()));
    }
    let base = if u.tls {
        Target::direct_tls(&u.host, u.port, &u.path)
    } else {
        Target::direct(&u.host, u.port, &u.path)
    };
    base.with_headers(headers, Some(spec.user_agent.clone()))
}

// ------------------------------------------------------------------ transfer

async fn run_download(
    spec: StartSpec,
    cancel: Arc<AtomicBool>,
    limiter: Arc<RateLimiter>,
    final_path: Arc<Mutex<String>>,
    tx: UnboundedSender<Event>,
) {
    let id = spec.id;
    let ev = |e: Event| {
        let _ = tx.send(e);
    };

    // HYDRA_AB_FRESH=1: measurement escape hatch — build a throwaway
    // connector per transfer (the pre-pooling behaviour) to bisect
    // throughput differences. Not for production use.
    let fresh = std::env::var_os("HYDRA_AB_FRESH").is_some();
    let connector = match if fresh {
        TlsCapableConnector::new()
            .map(Arc::new)
            .map_err(|e| e.to_string())
    } else {
        shared_connector()
    } {
        Ok(c) => c,
        Err(e) => {
            ev(Event::Failed {
                id,
                error: e,
                done: 0,
                held: spec.held.clone(),
                permission_denied: false,
            });
            return;
        }
    };

    // ---- ftp://: single-connection fetch ---------------------------------
    //
    // Deliberately ONE connection: FTP range preemption costs
    // control-channel round trips that HTTP pays nothing for, so the
    // object streams sequentially from one source.
    if let Ok(u) = parse_url(&spec.url) {
        if u.ftp {
            run_ftp_download(&spec, &u, &cancel, &limiter, &tx).await;
            return;
        }
    }

    // ---- probe, following redirects --------------------------------------
    ev(Event::Status {
        id,
        line: crate::i18n::tr("Connecting..."),
    });
    let mut url = spec.url.clone();
    let mut probed: Option<(ParsedUrl, Probe)> = None;
    // Per-request setup estimate for the scheduler. Timed on the FINAL probe
    // hop only: the old whole-loop measurement folded every redirect hop in,
    // so the origins most in need of fast repair decisions (long redirect
    // chains) got the slowest ones. Floored at the CLI's 0.05 s prior so a
    // pooled-connection probe cannot make repairs look free.
    let mut delta = 0.05f64;
    for _ in 0..10 {
        let u = match parse_url(&url) {
            Ok(u) => u,
            Err(e) => {
                ev(Event::Failed {
                    id,
                    error: e,
                    done: 0,
                    held: spec.held.clone(),
                    permission_denied: false,
                });
                return;
            }
        };
        let t = target_for(&u, &spec);
        let t_hop = std::time::Instant::now();
        match probe_resilient(connector.as_ref(), &t).await {
            Ok(p) if p.is_redirect() => {
                let loc = p.location.clone().unwrap_or_default();
                crate::log::debug(&format!("#{id} redirect {} -> {loc}", p.status));
                match join_url(&u, &loc) {
                    Some(next) => url = next,
                    None => {
                        ev(Event::Failed {
                            id,
                            error: format!("{}: {loc}", crate::i18n::tr("Unusable redirect")),
                            done: 0,
                            held: spec.held.clone(),
                            permission_denied: false,
                        });
                        return;
                    }
                }
            }
            Ok(p) => {
                // A redirect the server expressed in HTML rather than in a
                // header: a referrer stripper or link filter answering `200`
                // with a page whose whole content is "go here instead".
                // Without this hop the saved file IS that page — the
                // one-kilobyte `index.html` this resolves. Charged to the same
                // hop budget as a `3xx`, since a pair of such pages pointing at
                // each other is a loop like any other.
                let hop_to = if p.maybe_redirector() {
                    hya_net::html_redirect(connector.as_ref(), &t)
                        .await
                        .and_then(|loc| join_url(&u, &loc))
                } else {
                    None
                };
                if let Some(next) = hop_to {
                    crate::log::debug(&format!("#{id} html redirect -> {next}"));
                    url = next;
                } else {
                    crate::log::debug(&format!(
                        "#{id} probe: status={} size={} ranges={} type={:?}",
                        p.status, p.size, p.ranges, p.content_type
                    ));
                    delta = t_hop.elapsed().as_secs_f64().clamp(0.05, 45.0);
                    probed = Some((u, p));
                    break;
                }
            }
            Err(e) => {
                crate::log::error(&format!("#{id} probe failed: {e}"));
                ev(Event::Failed {
                    id,
                    error: e.to_string(),
                    done: 0,
                    held: spec.held.clone(),
                    permission_denied: false,
                });
                return;
            }
        }
        if cancel.load(Ordering::Relaxed) {
            ev(Event::Stopped {
                id,
                done: 0,
                held: spec.held.clone(),
            });
            return;
        }
    }
    let Some((u, p)) = probed else {
        ev(Event::Failed {
            id,
            error: crate::i18n::tr("Too many redirects"),
            done: 0,
            held: spec.held.clone(),
            permission_denied: false,
        });
        return;
    };
    crate::log::debug(&format!("#{id} probe delta {delta:.3}s"));

    let file_name = p.suggested_filename().or_else(|| {
        let n = file_name_from_url(&url);
        (!n.is_empty()).then_some(n)
    });
    let known_size = (p.status < 300 && p.size > 0).then_some(p.size);
    ev(Event::Probed {
        id,
        size: known_size,
        ranges: p.ranges,
        file_name,
    });

    let target = target_for(&u, &spec);
    let temp = spec.temp_path.clone();
    if let Some(dir) = std::path::Path::new(&temp).parent() {
        ensure_writable_dir(dir);
    }

    // ---- no-range / unknown-size fallback: one streaming GET -------------
    let Some(size) = known_size.filter(|_| p.ranges) else {
        crate::log::info(&format!(
            "#{id} no range support / unknown size: single stream"
        ));
        ev(Event::Status {
            id,
            line: crate::i18n::tr("Receiving data..."),
        });
        let t0 = std::time::Instant::now();
        // Driven through a poll loop rather than a bare await. This path has no
        // scheduler to observe, so a plain await reported nothing until the last
        // byte and read the stop flag never: on a large object that is an hour of
        // "Receiving data..." at 0 B with Pause and Stop doing nothing at all.
        // The byte counter and the cancel flag are what the loop is for.
        let written = Arc::new(AtomicU64::new(0));
        let pace = hya_net::polite::Pace::shared(limiter);
        let fut = hya_net::fetch_streaming_observed(
            connector.as_ref(),
            &target,
            &temp,
            &written,
            Some(cancel.as_ref()),
            &pace,
        );
        tokio::pin!(fut);
        let mut sm_rate = 0.0f64;
        let mut rate_mark: Option<(u64, std::time::Instant)> = None;
        loop {
            tokio::select! {
                r = &mut fut => {
                    match r {
                        Ok(n) => {
                            finish_file(&spec, &final_path, &tx, n, t0.elapsed().as_secs_f64());
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {
                            ev(Event::Stopped {
                                id,
                                done: written.load(Ordering::Relaxed),
                                held: vec![],
                            });
                        }
                        Err(e) => {
                            let denied = e.kind() == std::io::ErrorKind::PermissionDenied;
                            ev(Event::Failed {
                                id,
                                error: friendly_io(&e, &temp),
                                done: written.load(Ordering::Relaxed),
                                held: vec![],
                                permission_denied: denied,
                            });
                        }
                    }
                    return;
                }
                _ = tokio::time::sleep(std::time::Duration::from_millis(emit_interval_ms() as u64)) => {
                    // Dropping the future closes the socket. A stalled read is
                    // exactly when the user reaches for Stop, so the flag is
                    // acted on here too and not only between reads.
                    if cancel.load(Ordering::Relaxed) {
                        ev(Event::Stopped {
                            id,
                            done: written.load(Ordering::Relaxed),
                            held: vec![],
                        });
                        return;
                    }
                    let done = written.load(Ordering::Relaxed);
                    let now = std::time::Instant::now();
                    if let Some((prev_done, prev_t)) = rate_mark {
                        let dt = now.duration_since(prev_t).as_secs_f64();
                        if dt > 0.0 {
                            let inst = done.saturating_sub(prev_done) as f64 / dt;
                            let alpha = (-dt / 1.5f64).exp();
                            sm_rate = sm_rate * alpha + inst * (1.0 - alpha);
                        }
                    }
                    rate_mark = Some((done, now));
                    ev(Event::Progress {
                        id,
                        done,
                        rate: sm_rate,
                        // No size to subtract from: this path exists because the
                        // server would not state one, so any ETA would be invented.
                        eta: None,
                        conns: vec![ConnRow {
                            downloaded: done,
                            info: crate::i18n::tr("Receiving data..."),
                        }],
                        held: vec![],
                    });
                }
            }
        }
    };

    // ---- scheduler path ---------------------------------------------------
    let n = spec.conns.clamp(1, 32);
    let sources = vec![Source {
        caps: if p.weak_validator || p.validator.is_none() {
            Capability::NoValidator
        } else {
            Capability::Full
        },
        delta_est: delta,
        ..Source::default()
    }];
    let per = [n];
    let mut sched =
        Scheduler::new(size, sources, &per).with_stall_timeout((12.0 * delta).clamp(4.0, 45.0));
    // Adaptive: open the budget but start ONE connection active; the ramp
    // inside `run_transfer_cancellable` admits the rest only while the
    // aggregate rate says they pay. On a link the origin (or an ISP shaper)
    // saturates at one or two streams, a fixed 8 divides capacity and adds
    // setup cost — measured on this project as slower than a single stream
    // on most live objects.
    if spec.adaptive && n > 1 {
        sched.set_active_limit(1);
    }
    let mut held_spans = spec.held.clone();
    if let Some(exp) = spec.expected_size {
        if exp != size && !held_spans.is_empty() {
            crate::log::warn(&format!(
                "#{id} mirror size mismatch ({exp} -> {size}): restarting from zero"
            ));
            held_spans.clear();
        }
    }
    let mut already = 0u64;
    for &(lo, hi) in &held_spans {
        let hi = hi.min(size);
        if lo < hi {
            sched.mark_done(lo, hi);
            already += hi - lo;
        }
    }

    let sink = match SparseSink::create(&temp, size) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            crate::log::error(&format!("#{id} sink create failed at {temp}: {e}"));
            let denied = e.kind() == std::io::ErrorKind::PermissionDenied;
            ev(Event::Failed {
                id,
                error: friendly_io(&e, &temp),
                done: already,
                held: spec.held.clone(),
                permission_denied: denied,
            });
            return;
        }
    };

    ev(Event::Status {
        id,
        line: crate::i18n::tr("Receiving data..."),
    });

    // Observer: runs inside the transfer loop every tick; throttle the
    // channel to ~4 events/sec and accumulate per-connection byte counts
    // from range-cursor deltas (the scheduler reports position, not totals).
    let tx_obs = tx.clone();
    let mut last_emit = std::time::Instant::now() - std::time::Duration::from_secs(1);
    let mut per_conn: HashMap<usize, (u64, u64, u64)> = HashMap::new(); // conn -> (last_lo, last_pos, total)
    type Snapshot = (u64, Vec<(u64, u64)>);
    let snapshot: Arc<Mutex<Snapshot>> = Arc::new(Mutex::new((already, spec.held.clone())));
    let snap_obs = snapshot.clone();
    // Displayed rate: EWMA over MEASURED byte deltas, not a sum of the
    // scheduler's per-connection window rates. The per-connection figures are
    // an internal control signal and twitch by design; a readout built on
    // them made the speed and ETA jump every refresh. Bytes-over-wall-clock
    // through a ~1.5 s time constant reads as a steady counter.
    let mut sm_rate = 0.0f64;
    let mut rate_mark: Option<(u64, std::time::Instant)> = None;
    let mut observe = move |s: &Scheduler, done: u64| {
        for j in 0..s.n_conns() {
            let e = per_conn.entry(j).or_insert((0, 0, 0));
            if let Some((lo, pos, _hi)) = s.conn_range(j) {
                if e.0 == lo && pos >= e.1 {
                    e.2 += pos - e.1;
                } else if pos > lo {
                    e.2 += pos - lo;
                }
                *e = (lo, pos, e.2);
            }
        }
        // 10 Hz to the GUI: frequent enough that the bar and counters read as
        // continuous, cheap enough to be invisible in profiles. The held-span
        // snapshot (kept for the cancel path) refreshes at the same cadence:
        // `held_ranges()` allocates an IntervalSet, so computing it at the
        // 50 Hz tick rate was pure waste, and a stop can only lose the last
        // <100 ms of spans — which resume then re-fetches, safely.
        if last_emit.elapsed().as_millis() >= emit_interval_ms() || done >= size {
            last_emit = std::time::Instant::now();
            let held = s.held_ranges();
            if let Ok(mut g) = snap_obs.lock() {
                *g = (done, held.clone());
            }
            let now = std::time::Instant::now();
            if let Some((prev_done, prev_t)) = rate_mark {
                let dt = now.duration_since(prev_t).as_secs_f64();
                if dt > 0.0 {
                    let inst = done.saturating_sub(prev_done) as f64 / dt;
                    // alpha from dt so the time constant is independent of
                    // emit cadence: tau = 1.5 s.
                    let alpha = (-dt / 1.5f64).exp();
                    sm_rate = sm_rate * alpha + inst * (1.0 - alpha);
                }
            }
            rate_mark = Some((done, now));
            let conns: Vec<ConnRow> = (0..s.n_conns())
                .map(|j| {
                    let r = s.conn_rate(j);
                    ConnRow {
                        downloaded: per_conn.get(&j).map(|e| e.2).unwrap_or(0),
                        info: if s.conn_range(j).is_none() {
                            crate::i18n::tr("Disconnect.")
                        } else if r > 1.0 {
                            crate::i18n::tr("Receiving data...")
                        } else {
                            crate::i18n::tr("Send GET...")
                        },
                    }
                })
                .collect();
            // ETA from the smoothed rate, and only once it has warmed past
            // 1 KB/s — an ETA computed from startup noise counts down from
            // nonsense values.
            let eta = if sm_rate > 1024.0 {
                Some(((size - done.min(size)) as f64 / sm_rate) as u64)
            } else {
                None
            };
            let _ = tx_obs.send(Event::Progress {
                id,
                done,
                rate: sm_rate,
                eta,
                conns,
                held,
            });
        }
    };

    let pace = hya_net::polite::Pace::shared(limiter);
    let t0 = std::time::Instant::now();
    let tick_ms = if POWER_SAVE.load(Ordering::Relaxed) {
        80
    } else {
        20
    };
    let result = hya_net::run_transfer_cancellable(
        connector,
        vec![target],
        &per,
        size,
        sink,
        sched,
        tick_ms,
        &mut observe,
        pace,
        Some(cancel.clone()),
    )
    .await;

    let (done, held) = snapshot
        .lock()
        .map(|g| g.clone())
        .unwrap_or((already, vec![]));
    match result {
        Ok((elapsed, reqs)) => {
            // Requests far above the connection count means range churn
            // (repairs/steals) — the first thing to look at when a transfer
            // is slower than the pipe.
            crate::log::debug(&format!(
                "#{id} transfer {elapsed:.2}s, {reqs} requests over {n} conns"
            ));
            finish_file(&spec, &final_path, &tx, size, elapsed);
        }
        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {
            ev(Event::Stopped { id, done, held });
        }
        Err(e) => {
            crate::log::error(&format!("#{id} failed at {done}/{size} bytes: {e}"));
            let denied = e.kind() == std::io::ErrorKind::PermissionDenied;
            let msg = friendly_io(&e, &spec.final_path);
            ev(Event::Failed {
                id,
                error: msg,
                done,
                held,
                permission_denied: denied,
            });
        }
    }
    let _ = t0;
}

/// Single-connection FTP transfer: probe SIZE, resume via REST from the
/// contiguous prefix already on disk, drive progress off the sink's byte
/// counter (there is no scheduler to observe).
async fn run_ftp_download(
    spec: &StartSpec,
    u: &ParsedUrl,
    cancel: &Arc<AtomicBool>,
    limiter: &Arc<RateLimiter>,
    tx: &UnboundedSender<Event>,
) {
    use hya_net::scheme::Fetcher;
    let id = spec.id;
    let ev = |e: Event| {
        let _ = tx.send(e);
    };
    ev(Event::Status {
        id,
        line: crate::i18n::tr("Connecting..."),
    });

    let ep = hya_net::scheme::Endpoint::new(&u.host, u.port, &u.path).with_credentials(
        spec.auth.as_ref().map(|(l, _)| l.as_str()),
        spec.auth.as_ref().map(|(_, p)| p.as_str()),
    );
    // The same limiter the HTTP path answers to, so Speed Limiter means the
    // same thing on an ftp:// download — including switched on mid-transfer.
    let fetcher = hya_net::ftp::FtpFetcher::new(Arc::new(hya_net::TcpConnector))
        .with_pace(hya_net::polite::Pace::shared(limiter.clone()));

    let probe = match fetcher.probe(&ep).await {
        Ok(p) => p,
        Err(e) => {
            crate::log::error(&format!("#{id} ftp probe failed: {e}"));
            ev(Event::Failed {
                id,
                error: format!("ftp: {e}"),
                done: 0,
                held: spec.held.clone(),
                permission_denied: false,
            });
            return;
        }
    };
    crate::log::debug(&format!(
        "#{id} ftp probe: size={} ranged={}",
        probe.size, probe.ranged
    ));
    if probe.size == 0 {
        ev(Event::Failed {
            id,
            error: crate::i18n::tr("The FTP server did not report a file size"),
            done: 0,
            held: vec![],
            permission_denied: false,
        });
        return;
    }
    let file_name = {
        let n = file_name_from_url(&spec.url);
        (!n.is_empty()).then_some(n)
    };
    ev(Event::Probed {
        id,
        size: Some(probe.size),
        ranges: probe.ranged,
        file_name,
    });

    // Resume from the contiguous prefix only — REST is a start offset, not a
    // span list.
    let start = if probe.ranged {
        spec.held
            .iter()
            .find(|(lo, _)| *lo == 0)
            .map(|(_, hi)| (*hi).min(probe.size))
            .unwrap_or(0)
    } else {
        0
    };

    let temp = spec.temp_path.clone();
    if let Some(dir) = std::path::Path::new(&temp).parent() {
        ensure_writable_dir(dir);
    }
    let sink = match SparseSink::create(&temp, probe.size) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            let denied = e.kind() == std::io::ErrorKind::PermissionDenied;
            ev(Event::Failed {
                id,
                error: friendly_io(&e, &temp),
                done: start,
                held: spec.held.clone(),
                permission_denied: denied,
            });
            return;
        }
    };

    ev(Event::Status {
        id,
        line: crate::i18n::tr("Receiving data..."),
    });
    let t0 = std::time::Instant::now();
    let size = probe.size;
    let fut = fetcher.fetch_range(&ep, start, size, sink.clone());
    tokio::pin!(fut);
    let mut ticker = tokio::time::interval(std::time::Duration::from_millis(250));
    let mut sm_rate = 0.0f64;
    let mut last = (start, std::time::Instant::now());
    let result = loop {
        tokio::select! {
            res = &mut fut => break Some(res),
            _ = ticker.tick() => {
                if cancel.load(Ordering::Relaxed) {
                    break None;
                }
                let done = start + sink.written.load(Ordering::Relaxed);
                let dt = last.1.elapsed().as_secs_f64();
                if dt > 0.0 {
                    let inst = done.saturating_sub(last.0) as f64 / dt;
                    let alpha = (-dt / 1.5f64).exp();
                    sm_rate = sm_rate * alpha + inst * (1.0 - alpha);
                }
                last = (done, std::time::Instant::now());
                let eta = (sm_rate > 1024.0)
                    .then(|| ((size - done.min(size)) as f64 / sm_rate) as u64);
                let _ = tx.send(Event::Progress {
                    id,
                    done,
                    rate: sm_rate,
                    eta,
                    conns: vec![ConnRow {
                        downloaded: done - start,
                        info: crate::i18n::tr("Receiving data..."),
                    }],
                    held: vec![(0, done)],
                });
            }
        }
    };
    let done = start + sink.written.load(Ordering::Relaxed);
    match result {
        // Dropping the pinned future closes the single data connection.
        None => ev(Event::Stopped {
            id,
            done,
            held: vec![(0, done)],
        }),
        Some(Ok(())) => {
            let final_path = Arc::new(Mutex::new(spec.final_path.clone()));
            finish_file(spec, &final_path, tx, size, t0.elapsed().as_secs_f64());
        }
        Some(Err(e)) => {
            crate::log::error(&format!("#{id} ftp failed at {done}/{size}: {e}"));
            ev(Event::Failed {
                id,
                error: format!("ftp: {e}"),
                done,
                held: vec![(0, done)],
                permission_denied: false,
            });
        }
    }
}

/// Move the finished `.part` into place and report completion. The
/// destination is read at completion time so File-Info edits made while the
/// transfer ran land the file where the user finally said.
fn finish_file(
    spec: &StartSpec,
    final_path: &Arc<Mutex<String>>,
    tx: &UnboundedSender<Event>,
    size: u64,
    elapsed: f64,
) {
    let final_str = final_path
        .lock()
        .map(|g| g.clone())
        .unwrap_or_else(|_| spec.final_path.clone());
    let final_path = std::path::Path::new(&final_str);
    if let Some(dir) = final_path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let moved = std::fs::rename(&spec.temp_path, final_path).or_else(|_| {
        // Cross-device: copy then remove.
        std::fs::copy(&spec.temp_path, final_path)
            .map(|_| ())
            .map(|()| {
                let _ = std::fs::remove_file(&spec.temp_path);
            })
    });
    match moved {
        Ok(()) => {
            crate::log::log(&format!("done #{} -> {final_str}", spec.id));
            let _ = tx.send(Event::Finished {
                id: spec.id,
                elapsed,
                size,
            });
        }
        Err(e) => {
            let denied = e.kind() == std::io::ErrorKind::PermissionDenied;
            let _ = tx.send(Event::Failed {
                id: spec.id,
                error: friendly_io(&e, &final_str),
                done: size,
                held: vec![(0, size)],
                permission_denied: denied,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_url_forms() {
        let u = parse_url("https://example.com/a/b.zip?x=1").unwrap();
        assert!(u.tls);
        assert_eq!(u.host, "example.com");
        assert_eq!(u.port, 443);
        assert_eq!(u.path, "/a/b.zip?x=1");
        let u = parse_url("http://host:8080").unwrap();
        assert!(!u.tls);
        assert_eq!(u.port, 8080);
        assert_eq!(u.path, "/");
        let f = parse_url("ftp://x/y").unwrap();
        assert!(f.ftp);
        assert_eq!(f.port, 21);
        assert!(parse_url("sftp://x/y").is_err());
        assert_eq!(file_name_from_url("https://h/a/b%20c.bin"), "b c.bin");
    }

    /// Every spelling a `Location` header or a redirector page may use.
    #[test]
    fn redirect_targets_resolve_against_the_page_that_served_them() {
        let base = parse_url("https://h.org/a/b/page?q=1").unwrap();
        let j = |loc: &str| join_url(&base, loc);
        assert_eq!(
            j("https://x.org/f.zip").as_deref(),
            Some("https://x.org/f.zip")
        );
        assert_eq!(j("//x.org/f.zip").as_deref(), Some("https://x.org/f.zip"));
        assert_eq!(
            j("/dl/f.zip").as_deref(),
            Some("https://h.org:443/dl/f.zip")
        );
        // Relative to the current DIRECTORY, with the query dropped: joining
        // against `b?q=1` would have produced `/a/b?q=1/f.zip`.
        assert_eq!(j("f.zip").as_deref(), Some("https://h.org:443/a/b/f.zip"));
        // Nothing fetchable, so the chain ends rather than guessing.
        assert_eq!(j("javascript:void(0)"), None);
        assert_eq!(j("  "), None);
    }

    #[test]
    fn base64_rfc() {
        assert_eq!(base64(b"user:pass"), "dXNlcjpwYXNz");
        assert_eq!(base64(b"a"), "YQ==");
        assert_eq!(base64(b"ab"), "YWI=");
    }

    /// A/B throughput check: download the same URL twice in one process —
    /// run 1 cold, run 2 over the warm shared pool — and print both rates.
    /// Opt-in via HYDRA_GUI_LIVE_AB=<url>; read logs/gui.log (debug) for the
    /// probe delta and request counts of each run.
    #[test]
    fn live_ab_download() {
        let Some(url) = std::env::var("HYDRA_GUI_LIVE_AB")
            .ok()
            .filter(|s| !s.is_empty())
        else {
            return;
        };
        crate::log::set_level("debug");
        ensure_started();
        let mut rx = take_events().expect("events");
        let dir = std::env::temp_dir();
        for run in 1..=2u64 {
            let final_path = dir.join(format!("hydra-ab-{run}.bin"));
            let _ = std::fs::remove_file(&final_path);
            let conns: usize = std::env::var("HYDRA_AB_CONNS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(8);
            send(Cmd::Start(Box::new(StartSpec {
                id: run,
                url: url.clone(),
                auth: None,
                conns,
                user_agent: "hydra-gui-ab".into(),
                temp_path: dir
                    .join(format!("hydra-ab-{run}.part"))
                    .to_string_lossy()
                    .into_owned(),
                final_path: final_path.to_string_lossy().into_owned(),
                held: vec![],
                expected_size: None,
                cookies: None,
                limit: None,
                adaptive: std::env::var_os("HYDRA_AB_ADAPTIVE").is_some(),
            })));
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
            loop {
                assert!(std::time::Instant::now() < deadline, "run {run} timed out");
                match rx.blocking_recv().expect("engine alive") {
                    Event::Finished { id, elapsed, size } if id == run => {
                        eprintln!(
                            "run {run}: {size} bytes in {elapsed:.2}s = {:.2} MB/s",
                            size as f64 / elapsed / 1.048576e6
                        );
                        break;
                    }
                    Event::Failed { id, error, .. } if id == run => {
                        panic!("run {run} failed: {error}")
                    }
                    _ => {}
                }
            }
            let _ = std::fs::remove_file(&final_path);
        }
    }

    /// Live network test of the full engine path (probe -> scheduler ->
    /// rename): opt-in via HYDRA_GUI_LIVE_TEST=<url>.
    #[test]
    fn live_download() {
        let Some(url) = std::env::var("HYDRA_GUI_LIVE_TEST")
            .ok()
            .filter(|s| !s.is_empty())
        else {
            return;
        };
        ensure_started();
        let mut rx = take_events().expect("events");
        let dir = std::env::temp_dir();
        let final_path = dir.join("hydra-gui-live-test.bin");
        let _ = std::fs::remove_file(&final_path);
        send(Cmd::Start(Box::new(StartSpec {
            id: 1,
            url,
            auth: None,
            conns: 8,
            user_agent: "hydra-gui-test".into(),
            temp_path: dir
                .join("hydra-gui-live-test.part")
                .to_string_lossy()
                .into_owned(),
            final_path: final_path.to_string_lossy().into_owned(),
            held: vec![],
            expected_size: None,
            cookies: None,
            limit: None,
            adaptive: false,
        })));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        let mut probed_size = None;
        loop {
            assert!(std::time::Instant::now() < deadline, "timed out");
            match rx.blocking_recv().expect("engine alive") {
                Event::Probed { size, .. } => probed_size = size,
                Event::Finished { size, .. } => {
                    let meta = std::fs::metadata(&final_path).expect("final file");
                    assert_eq!(meta.len(), size);
                    if let Some(ps) = probed_size {
                        assert_eq!(ps, size);
                    }
                    eprintln!("live download ok: {size} bytes");
                    break;
                }
                Event::Failed { error, .. } => panic!("download failed: {error}"),
                _ => {}
            }
        }
    }
}
