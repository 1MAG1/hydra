// Copyright (C) 2026 Javad Rajabzadeh
// SPDX-License-Identifier: MIT OR Apache-2.0
//
// This library is intentionally permissive, not GPL, even though the `hydra`
// binary that ships it is GPL-3.0-or-later: Rust links statically, so copyleft
// here would propagate to every downstream crate. See LICENSING.md.

//! HTTP/1.1 range transport driving the `hydra-core` scheduler.
//!
//! Deliberately minimal: a hand-rolled HTTP/1.1 client over `tokio::net::TcpStream`,
//! because the point of this crate is to show that the scheduler core needs
//! nothing from the transport but *bytes arrived* and *when*. The same core runs
//! under the discrete-event simulator with no changes.
//!
//! Two properties the harness measures:
//!
//! * **Memory is independent of object size.** Bytes are written to their file
//!   offset as they arrive (positioned write, `pwrite`), never buffered whole and
//!   never reassembled. Resident memory is `O(connections × buffer)`.
//! * **The scheduler is I/O-free.** Everything in `hydra-core` is driven by
//!   `on_bytes` / `tick`; this crate contains all the syscalls.
//!
//! # Where SIMD is and is not used
//!
//! Three byte-parallel paths matter here, and each gets its vectorization from a
//! different place — deliberately, because hand-written intrinsics are a
//! maintenance and correctness cost that has to be paid for by a measurement:
//!
//! * **Byte search** (`find_crlf`, `find_crlf2`, [`framebuf::FrameBuf`]) uses
//!   `memchr`, which does runtime feature detection and dispatch: AVX2 or SSE2
//!   on x86-64, NEON on aarch64, scalar elsewhere. One binary is correct and
//!   fast on every target, with no `unsafe` in this crate.
//! * **Hex encoding** ([`digest::to_lower_hex`]) is a table lookup over a
//!   preallocated buffer, shaped so LLVM autovectorizes it for whatever
//!   `target-cpu` the build selects. Measured 11.8x over the `write!` form it
//!   replaced, which is all the gain intrinsics could have bought.
//! * **Hashing and erasure coding** are delegated: `sha2` dispatches to SHA-NI
//!   on x86-64 and the ARMv8 SHA-2 extensions via `cpufeatures`, `blake3` to
//!   AVX2/AVX-512/NEON, and `reed-solomon-simd` to its own kernels. These are
//!   the genuinely compute-bound operations and they are already
//!   hardware-accelerated by crates that test those paths on real silicon.
//!
//! Vectorized paths are covered by differential tests against scalar references,
//! maintaining safety and high performance across architectures.

pub mod digest;
pub mod framebuf;
pub mod ftp;
pub mod ftp_origin;
pub mod http_scheme;
pub mod manifest;
pub mod origin;
pub mod parity;
pub mod polite;
pub mod scheme;
pub mod socks;
pub mod stream_digest;
pub mod tls;

/// The identity sent when the user supplies no `--user-agent`.
///
/// One definition for the whole workspace: the CLI's flag default and the
/// queue manager reference it too, so a version bump cannot leave the paths
/// disagreeing about who they say they are.
pub const DEFAULT_USER_AGENT: &str = "hydra/0.1";

use std::future::Future;
use std::io;
use std::pin::Pin;
use tokio::net::TcpStream;

/// Per-connection read buffer. The only memory that scales with concurrency.
pub const READ_BUF: usize = 64 * 1024;

#[derive(Debug)]
pub struct Arrival {
    pub conn: usize,
    /// Absolute file offset these bytes landed at. The scheduler credits by
    /// offset, not by cursor, so a response still draining from a superseded
    /// range cannot advance the cursor of the range the connection holds now.
    pub off: u64,
    pub bytes: u64,
    pub at: f64,
    pub dt: f64,
}

#[derive(Clone, Debug)]
pub struct Target {
    /// Host to CONNECT the socket to. For a direct fetch this is the origin; for
    /// a proxied fetch it is the proxy.
    pub host: String,
    pub port: u16,
    pub path: String,
    /// Connect with TLS.
    pub tls: bool,
    /// Extra request headers, verbatim `Name: value` lines (curl -H).
    pub headers: Vec<String>,
    /// `User-Agent` to send.
    pub agent: Option<String>,
    /// Origin authority (`host` or `host:port`) when the request must be sent in
    /// absolute form through a forward proxy. `None` = origin-form request to a
    /// directly-connected origin.
    ///
    /// RFC 9112 §3.2.2: a client sending to a proxy MUST send the target URI in
    /// absolute form. Carrying it here keeps the proxy decision entirely inside
    /// the transport, so the scheduler core is unchanged.
    pub origin: Option<String>,
}

impl Target {
    /// Direct origin-form target.
    pub fn direct(host: &str, port: u16, path: &str) -> Self {
        Self {
            host: host.into(),
            port,
            path: path.into(),
            origin: None,
            tls: false,
            headers: Vec::new(),
            agent: None,
        }
    }

    /// Direct target over TLS.
    pub fn direct_tls(host: &str, port: u16, path: &str) -> Self {
        Self {
            tls: true,
            ..Self::direct(host, port, path)
        }
    }

    /// Name to present in SNI and to validate the certificate against.
    ///
    /// This is the ORIGIN authority, never the socket peer: through a proxy the
    /// socket connects to the proxy while the certificate belongs to the origin.
    pub fn tls_server_name(&self) -> &str {
        match &self.origin {
            Some(o) => o.split(':').next().unwrap_or(o),
            None => &self.host,
        }
    }

    /// The ORIGIN host and port, regardless of how the connection is routed.
    ///
    /// A SOCKS proxy needs this: the TCP connection goes to the proxy, but the
    /// handshake must name the origin. `host`/`port` may hold the HTTP proxy's
    /// address, so they cannot be used directly.
    pub fn origin_endpoint(&self) -> (String, u16) {
        match &self.origin {
            Some(o) => match o.rsplit_once(':') {
                Some((h, p)) => (
                    h.to_string(),
                    p.parse().unwrap_or(if self.tls { 443 } else { 80 }),
                ),
                None => (o.clone(), if self.tls { 443 } else { 80 }),
            },
            None => (self.host.clone(), self.port),
        }
    }

    /// Authority for a proxy `CONNECT`: the origin host and port.
    pub fn proxy_authority(&self) -> &str {
        self.origin.as_deref().unwrap_or(&self.host)
    }

    /// Attach extra request headers and a `User-Agent`, as the CLI flags request.
    pub fn with_headers(mut self, headers: Vec<String>, agent: Option<String>) -> Self {
        self.headers = headers;
        self.agent = agent;
        self
    }

    fn user_agent(&self) -> &str {
        self.agent.as_deref().unwrap_or(DEFAULT_USER_AGENT)
    }

    fn extra_headers(&self) -> &[String] {
        &self.headers
    }

    /// Absolute-form target routed through a forward proxy.
    pub fn via_proxy(proxy_host: &str, proxy_port: u16, origin_host: &str, path: &str) -> Self {
        Self {
            host: proxy_host.into(),
            port: proxy_port,
            path: path.into(),
            origin: Some(origin_host.into()),
            tls: false,
            headers: Vec::new(),
            agent: None,
        }
    }

    /// The request-target for the start line, and the `Host` header value.
    fn request_target(&self) -> (String, &str) {
        match &self.origin {
            Some(o) => (format!("http://{}{}", o, self.path), o.as_str()),
            None => (self.path.clone(), self.host.as_str()),
        }
    }
}

/// Anything that can open a byte stream to a target.
///
/// This exists because the transport must be swappable: `TcpConnector` for real
/// networks, `DuplexConnector` (in `origin`) for hermetic tests. The scheduler
/// core sees neither -- it only ever receives `on_bytes`.
pub trait Connector: Send + Sync + 'static {
    type Stream: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send;
    fn connect<'a>(
        &'a self,
        t: &'a Target,
    ) -> Pin<Box<dyn Future<Output = io::Result<Self::Stream>> + Send + 'a>>;
}

/// Real TCP.
pub struct TcpConnector;

impl Connector for TcpConnector {
    type Stream = TcpStream;
    fn connect<'a>(
        &'a self,
        t: &'a Target,
    ) -> Pin<Box<dyn Future<Output = io::Result<TcpStream>> + Send + 'a>> {
        Box::pin(async move { TcpStream::connect((t.host.as_str(), t.port)).await })
    }
}

pub mod http;
pub mod sink;
pub mod transfer;

pub use http::{
    fetch_range_retry, fetch_small, fetch_streaming, header_lookup, probe, probe_size_via_range,
    probe_via_get, Probe,
};
pub use sink::SparseSink;
pub use transfer::{
    run_transfer, run_transfer_into, run_transfer_observed, run_transfer_paced, run_transfer_tick,
};

pub use socks::{Proxy, ProxyKind};
pub use tls::{connect_family, IpFamily, MaybeTls, TlsCapableConnector};
