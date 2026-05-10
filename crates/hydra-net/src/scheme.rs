//! The protocol seam: what a scheme must provide for the scheduler to drive it.
//!
//! # Why `Connector` was not already this
//!
//! `Connector` abstracts a byte STREAM — TCP, TLS, a SOCKS-relayed socket, an in-process
//! duplex pipe. Every implementation answers the same question: "give me bytes to and from
//! this endpoint." That is the right seam for *transport* and the wrong one for *protocol*,
//! because HTTP and FTP do not differ in how bytes move; they differ in how you ask for a
//! byte range, and in what it costs to stop asking.
//!
//! So this trait sits one level up. A scheme answers two questions:
//!
//! * `probe` — how large is the object, does it support ranged reads, and is there a
//!   validator that proves two sources serve the same bytes?
//! * `fetch_range` — deliver `[lo, hi)` into a sink, and stop when told to.
//!
//! # The property that actually matters
//!
//! The scheduler's central result is that a byte range is not a commitment: an HTTP range
//! request has a server-side start (`Range: bytes=lo-hi`) and a *client-side* end, so a
//! laggard's range can be shrunk by simply not reading the rest — no message to the server,
//! no round trip, no wasted work beyond bytes already in flight. Preemption is free, which
//! is why the makespan excess is independent of object size.
//!
//! Not every protocol has that property, and a scheme layer that hid the difference would
//! be actively misleading — it would let the scheduler make reassignment decisions priced
//! for HTTP against a protocol where they cost a round trip each. So the cost is part of
//! the interface: [`Capabilities::preempt_cost`] states what stopping early costs, and the
//! scheduler's deadband can be scaled by it rather than discovering the cost as mysterious
//! wasted time.
//!
//! FTP is the case that motivates this. `REST <offset>` sets where a transfer starts and
//! there is no way to say where it ends, so shrinking a range means aborting the data
//! connection: `ABOR` on the control channel, drain the reply, and open a fresh data
//! connection (a new `PASV`) for the next range. That is two round trips where HTTP needs
//! zero. See `ftp.rs` for the measurement.

use crate::{Connector, SparseSink};
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;

/// What a scheme can do, and what its operations cost.
#[derive(Clone, Debug, PartialEq)]
pub struct Capabilities {
    /// Ranged reads at all. Without this the object can only be streamed whole, so
    /// multi-source assembly is impossible regardless of how many mirrors exist.
    pub ranged: bool,
    /// A range can be ended by the CLIENT without telling the server.
    ///
    /// True for HTTP (`Range: bytes=lo-hi` names the end; the client may also just stop
    /// reading). False for FTP, where `REST` gives a start and only `ABOR` ends a transfer.
    pub client_bounded_ranges: bool,
    /// Round trips required to stop a transfer early and be ready for the next one.
    ///
    /// Zero for HTTP. The scheduler multiplies its repair deadband by this, so a protocol
    /// where preemption is expensive reassigns less eagerly instead of thrashing.
    pub preempt_cost_rtt: f64,
    /// A validator (ETag, or a modification time plus size) that can prove two sources
    /// serve identical bytes. Without one, cross-mirror assembly is unsound.
    pub has_validators: bool,
}

impl Capabilities {
    /// HTTP/1.1 with byte ranges: everything the scheduler was designed around.
    pub fn http() -> Self {
        Self {
            ranged: true,
            client_bounded_ranges: true,
            preempt_cost_rtt: 0.0,
            has_validators: true,
        }
    }

    /// SFTP (SSH file transfer): the protocol that would suit this scheduler BEST.
    ///
    /// Not implemented here — see the note below on why — but the capability entry is worth
    /// stating, because SFTP is the opposite of FTP on the axis that matters. Its read
    /// operation is `SSH_FXP_READ(handle, offset, length)`: both ends of the range are named
    /// by the CLIENT, on every request, and requests are pipelined over one connection with
    /// independent ids. Shrinking a laggard's work means simply not issuing the remaining
    /// reads, which costs nothing and needs no new connection — the same property HTTP has,
    /// and arguably cleaner, since there is no header round trip per range either.
    ///
    /// The validator story is weaker than HTTP's but real: `SSH_FXP_FSTAT` gives size and
    /// mtime, and the OpenSSH `check-file` extension can return per-block hashes, which is
    /// an actual content digest rather than an opaque tag. Marked `false` here because the
    /// extension is not universal and size+mtime alone cannot prove two hosts serve
    /// identical bytes.
    pub fn sftp() -> Self {
        Self {
            ranged: true,
            client_bounded_ranges: true,
            preempt_cost_rtt: 0.0,
            has_validators: false,
        }
    }

    /// FTP: ranged reads via `REST`, but only server-bounded, and `ABOR` to stop.
    ///
    /// `preempt_cost_rtt` is 2.0 — one `ABOR` plus reply, and one `PASV` plus reply for the
    /// replacement data connection. Measured in `ftp::tests`, not assumed.
    pub fn ftp() -> Self {
        Self {
            ranged: true,
            client_bounded_ranges: false,
            preempt_cost_rtt: 2.0,
            has_validators: false,
        }
    }
}

/// What a probe learned about an object.
#[derive(Clone, Debug, Default)]
pub struct SchemeProbe {
    pub size: u64,
    pub ranged: bool,
    /// Strong-enough identity for cross-source assembly, when the scheme has one.
    pub validator: Option<String>,
    /// Weak validators must not be used to prove two sources agree: the specification
    /// permits them to compare equal across representations that are merely equivalent.
    pub weak_validator: bool,
    pub content_type: Option<String>,
    /// Raw protocol exchange, for `--server-response`.
    pub raw: String,
}

/// A protocol that can be driven by the scheduler.
///
/// Object-safe on purpose: the engine holds `Arc<dyn Fetcher>` chosen by URL scheme, so
/// adding a protocol does not touch the scheduler or the CLI.
pub trait Fetcher: Send + Sync {
    /// Scheme name, for diagnostics (`http`, `https`, `ftp`).
    fn scheme(&self) -> &'static str;

    /// What this protocol can do, and what preemption costs on it.
    fn capabilities(&self) -> Capabilities;

    fn probe<'a>(
        &'a self,
        t: &'a Endpoint,
    ) -> Pin<Box<dyn Future<Output = io::Result<SchemeProbe>> + Send + 'a>>;

    /// Deliver `[lo, hi)` into `sink` at absolute offsets.
    ///
    /// The implementation must write bytes at their true file offsets and must not report
    /// success without delivering the whole range — a short read reported as success is how
    /// a truncating server produces a corrupt file that passes every length check.
    fn fetch_range<'a>(
        &'a self,
        t: &'a Endpoint,
        lo: u64,
        hi: u64,
        sink: Arc<SparseSink>,
    ) -> Pin<Box<dyn Future<Output = io::Result<()>> + Send + 'a>>;
}

/// Where an object lives, independent of protocol.
///
/// Carries credentials because FTP's authentication *is* part of the URL
/// (`ftp://user:pass@host/path`), unlike HTTP where it is a header. They are held here and
/// never included in `Debug` output — see the manual implementation below.
#[derive(Clone)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
    pub path: String,
    /// Set when the connection must be tunnelled or routed; interpreted by the transport.
    pub origin: Option<(String, u16)>,
    pub tls: bool,
    pub user: Option<String>,
    pub pass: Option<String>,
    pub extra_headers: Vec<String>,
    pub agent: Option<String>,
}

impl std::fmt::Debug for Endpoint {
    /// Never print credentials.
    ///
    /// A `Debug` derive would put the password in every log line, panic message, and error
    /// report that formats an Endpoint. This is the whole reason the derive is not used.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Endpoint")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("path", &self.path)
            .field("tls", &self.tls)
            .field("user", &self.user.as_deref().map(|_| "<set>"))
            .field("pass", &self.pass.as_deref().map(|_| "<redacted>"))
            .finish()
    }
}

impl Endpoint {
    pub fn new(host: &str, port: u16, path: &str) -> Self {
        Self {
            host: host.to_string(),
            port,
            path: path.to_string(),
            origin: None,
            tls: false,
            user: None,
            pass: None,
            extra_headers: Vec::new(),
            agent: None,
        }
    }

    pub fn with_credentials(mut self, user: Option<&str>, pass: Option<&str>) -> Self {
        self.user = user.map(|s| s.to_string());
        self.pass = pass.map(|s| s.to_string());
        self
    }

    /// The login to use, defaulting to anonymous access.
    ///
    /// RFC 1635: anonymous FTP takes the literal user `anonymous` with an email address as
    /// the password. A contact address is not invented here — a generic placeholder is
    /// sent, because fabricating a user's email into network traffic is worse than being
    /// slightly impolite.
    pub fn ftp_login(&self) -> (String, String) {
        match (&self.user, &self.pass) {
            (Some(u), Some(p)) => (u.clone(), p.clone()),
            (Some(u), None) => (u.clone(), String::new()),
            (None, _) => ("anonymous".into(), "anonymous@example.invalid".into()),
        }
    }

    /// True when explicit credentials were supplied.
    pub fn has_credentials(&self) -> bool {
        self.user.is_some()
    }
}

/// Pick a fetcher for a URL scheme.
///
/// Returns `None` for a scheme this build cannot serve, so the caller can refuse with a
/// reason rather than silently downgrading — a client that quietly fetches a different
/// thing than it was asked for is worse than one that says no.
pub fn for_scheme<C: Connector>(scheme: &str, conn: Arc<C>) -> Option<Arc<dyn Fetcher>> {
    match scheme {
        "http" | "https" => Some(Arc::new(crate::http_scheme::HttpFetcher::new(conn))),
        "ftp" => Some(Arc::new(crate::ftp::FtpFetcher::new(conn))),
        // sftp:// and scp:// are recognised by name so the refusal can be specific. See
        // `unsupported_reason`.
        _ => None,
    }
}

/// Why a scheme is not available, for an error message that helps.
///
/// A bare "unsupported scheme" tells a user nothing about whether to wait for it, use
/// another tool, or fix their URL. These strings distinguish "this protocol is a poor fit"
/// from "this protocol fits well and simply is not built yet".
pub fn unsupported_reason(scheme: &str) -> &'static str {
    match scheme {
        "sftp" | "ssh" => {
            "sftp is not implemented yet. It is the best fit of any protocol for this \
             scheduler — SSH_FXP_READ names both ends of every range and requests pipeline \
             over one connection, so preemption is free exactly as it is for HTTP. What it \
             needs is an SSH transport (key and agent authentication, host-key \
             verification), which is a substantial dependency rather than a protocol \
             problem. Use `scp`/`sftp` or `rsync` for now."
        }
        "scp" => {
            "scp is not implemented, and would not help: the protocol streams a whole file \
             with no offsets, so there are no ranges to schedule. sftp is the one to want, \
             and it is not built yet either."
        }
        "file" => {
            "file:// is not fetched: there is no transfer to schedule, and reading a local \
             path through a download manager only risks writing over it. Use cp."
        }
        "gopher" | "gemini" => "that protocol is not supported.",
        _ => "unknown scheme.",
    }
}

/// Every scheme this build can fetch, for `--help` and for error messages.
pub fn supported() -> &'static [&'static str] {
    &["http", "https", "ftp"]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_preemption_is_free_and_ftp_preemption_is_not() {
        // This is the difference the whole abstraction exists to preserve. If a future
        // scheme layer "simplified" these to a single capability set, the scheduler would
        // price FTP reassignments as free and thrash.
        assert_eq!(Capabilities::http().preempt_cost_rtt, 0.0);
        assert!(Capabilities::http().client_bounded_ranges);
        assert!(Capabilities::ftp().preempt_cost_rtt > 0.0);
        assert!(!Capabilities::ftp().client_bounded_ranges);
    }

    #[test]
    fn ftp_has_no_validator_so_cross_mirror_assembly_is_unsound() {
        // FTP offers MDTM and SIZE, which together are a weak identity at best: two
        // mirrors can agree on both and still serve different builds. Claiming a validator
        // here would let the engine assemble one file from two versions of an object.
        assert!(!Capabilities::ftp().has_validators);
        assert!(Capabilities::http().has_validators);
    }

    #[test]
    fn credentials_never_appear_in_debug_output() {
        let e = Endpoint::new("ftp.example.org", 21, "/pub/f.bin")
            .with_credentials(Some("alice"), Some("s3cret"));
        let shown = format!("{e:?}");
        assert!(
            !shown.contains("s3cret") && !shown.contains("alice"),
            "a Debug derive would leak credentials into every log line: {shown}"
        );
        assert!(shown.contains("redacted"));
    }

    #[test]
    fn anonymous_is_the_default_login_and_no_real_address_is_invented() {
        let e = Endpoint::new("ftp.example.org", 21, "/pub/f.bin");
        let (u, p) = e.ftp_login();
        assert_eq!(u, "anonymous");
        // Fabricating the user's real address into network traffic would be worse than
        // being impolite, so the placeholder is deliberately non-routable.
        assert!(
            p.ends_with(".invalid"),
            "must not invent a real address: {p}"
        );
        assert!(!e.has_credentials());

        let with = e.with_credentials(Some("alice"), Some("pw"));
        assert_eq!(with.ftp_login(), ("alice".into(), "pw".into()));
        assert!(with.has_credentials());
    }

    #[test]
    fn an_unknown_scheme_is_refused_rather_than_guessed() {
        let conn = Arc::new(crate::TcpConnector);
        assert!(for_scheme("gopher", conn.clone()).is_none());
        assert!(for_scheme("file", conn.clone()).is_none());
        assert!(for_scheme("ftp", conn).is_some());
        assert!(supported().contains(&"ftp"));
    }

    #[test]
    fn sftp_would_have_free_preemption_unlike_ftp() {
        // Recording the reason SFTP is the protocol to want next: SSH_FXP_READ names both
        // ends of the range, so a laggard's remaining reads are simply not issued.
        let s = Capabilities::sftp();
        assert!(s.client_bounded_ranges);
        assert_eq!(s.preempt_cost_rtt, 0.0);
        assert_eq!(s.preempt_cost_rtt, Capabilities::http().preempt_cost_rtt);
        assert!(Capabilities::ftp().preempt_cost_rtt > s.preempt_cost_rtt);
        // But no universal content validator, so cross-host assembly stays refused.
        assert!(!s.has_validators);
    }

    #[test]
    fn refusals_explain_themselves_per_scheme() {
        // A bare "unsupported scheme" does not tell a user whether to wait for it, use
        // another tool, or fix the URL.
        assert!(unsupported_reason("sftp").contains("not implemented yet"));
        assert!(
            unsupported_reason("scp").contains("no offsets"),
            "scp's problem is structural, not effort"
        );
        assert!(unsupported_reason("file").contains("cp"));
        for s in ["sftp", "scp", "file"] {
            assert!(
                unsupported_reason(s).len() > 40,
                "{s} deserves an actionable reason"
            );
            assert!(!supported().contains(&s));
        }
    }
}
