//! TLS transport for HTTPS connections using rustls.
//!
//! Provides TLS encryption and verification with root certificate support,
//! session resumption caching, and SNI configuration.
//!
//! Certificate verification can be disabled, because self-signed certificates on
//! internal mirrors are a real situation. It is not quiet about it: the flag warns
//! on every use. A downloader that silently accepted any certificate would let an
//! interception replace the object with something else and still report a
//! successful verified transfer.

use crate::{Connector, Target};
use rustls::ClientConfig;
use rustls_pki_types::ServerName;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector as RustlsConnector;

/// A stream that is either plaintext or TLS.
///
/// An enum rather than a boxed trait object: the transport does millions of small
/// reads on this, and a vtable dispatch per read is a measurable cost for no gain
/// when there are exactly two variants.
pub enum MaybeTls {
    Plain(TcpStream),
    Tls(Box<tokio_rustls::client::TlsStream<TcpStream>>),
}

impl tokio::io::AsyncRead for MaybeTls {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        match self.get_mut() {
            MaybeTls::Plain(s) => Pin::new(s).poll_read(cx, buf),
            MaybeTls::Tls(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
        }
    }
}

impl tokio::io::AsyncWrite for MaybeTls {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        match self.get_mut() {
            MaybeTls::Plain(s) => Pin::new(s).poll_write(cx, buf),
            MaybeTls::Tls(s) => Pin::new(s.as_mut()).poll_write(cx, buf),
        }
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        match self.get_mut() {
            MaybeTls::Plain(s) => Pin::new(s).poll_flush(cx),
            MaybeTls::Tls(s) => Pin::new(s.as_mut()).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        match self.get_mut() {
            MaybeTls::Plain(s) => Pin::new(s).poll_shutdown(cx),
            MaybeTls::Tls(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
        }
    }
}

/// Open a CONNECT tunnel through a forward proxy.
///
/// Required before a TLS handshake on a proxied target: cleartext absolute-form
/// requests work because the proxy parses the request line, and an encrypted
/// request line cannot be parsed. A 2xx means the tunnel is open and the socket is
/// now end-to-end with the origin.
async fn connect_tunnel(sock: &mut TcpStream, authority: &str) -> io::Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let req = format!(
        "CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\nProxy-Connection: keep-alive\r\n\r\n"
    );
    sock.write_all(req.as_bytes()).await?;
    let mut buf = Vec::with_capacity(512);
    let mut byte = [0u8; 1];
    // Read only to the end of the header block: everything after it belongs to the
    // TLS handshake and must be left in the socket, not consumed here.
    while !buf.ends_with(b"\r\n\r\n") {
        let n = sock.read(&mut byte).await?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "proxy closed the connection during CONNECT",
            ));
        }
        buf.push(byte[0]);
        if buf.len() > 8192 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "proxy CONNECT response header exceeded 8 KiB",
            ));
        }
    }
    let head = String::from_utf8_lossy(&buf);
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse::<u16>().ok())
        .unwrap_or(0);
    if (200..300).contains(&status) {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "proxy refused CONNECT to {authority}: status {status}"
        )))
    }
}

/// Verifier that accepts any certificate. Only reachable via `--insecure`.
///
/// Kept in one clearly-named place so it cannot be enabled by accident: an
/// accidental blanket-accept turns a verified download into an unverified one
/// while still reporting success.
#[derive(Debug)]
struct AcceptAnyCert(Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for AcceptAnyCert {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls_pki_types::CertificateDer<'_>,
        _intermediates: &[rustls_pki_types::CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: rustls_pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls_pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls_pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// Which IP version to connect over: `-4`, `-6`, or whatever resolves.
///
/// A property of the connection rather than of the request, so it lives on the
/// connector beside the proxy. The flags were parsed and forwarded through the
/// wget/curl compat layer but never reached any socket: `-6` against an
/// IPv4-only host connected happily over IPv4 and reported success.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IpFamily {
    /// No preference: use whatever the resolver returns, in its own order.
    #[default]
    Any,
    /// `-4` / `--ipv4`: refuse anything but an A record.
    V4,
    /// `-6` / `--ipv6`: refuse anything but an AAAA record.
    V6,
}

impl IpFamily {
    /// Pick the family from the two flags. Both set is treated as no preference:
    /// the CLI rejects that combination before it reaches here.
    pub fn from_flags(v4: bool, v6: bool) -> Self {
        match (v4, v6) {
            (true, false) => Self::V4,
            (false, true) => Self::V6,
            _ => Self::Any,
        }
    }

    pub fn matches(&self, addr: &std::net::SocketAddr) -> bool {
        match self {
            Self::Any => true,
            Self::V4 => addr.is_ipv4(),
            Self::V6 => addr.is_ipv6(),
        }
    }

    /// How the flag is spelled, for error messages.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Any => "any",
            Self::V4 => "IPv4",
            Self::V6 => "IPv6",
        }
    }
}

/// Connect to `host:port`, honouring an address-family restriction.
///
/// Resolution is explicit rather than left to `TcpStream::connect((host, port))`
/// because that helper hides the candidate list: it tries every address the
/// resolver returned, of either family, and there is no way to tell it not to.
/// Filtering here is what makes `-4`/`-6` mean anything.
///
/// A host with no address of the requested family is an ERROR, not a silent
/// fallback to the other one — the whole point of the flag is to refuse.
pub async fn connect_family(host: &str, port: u16, family: IpFamily) -> io::Result<TcpStream> {
    use tokio::net::lookup_host;
    if family == IpFamily::Any {
        return TcpStream::connect((host, port)).await;
    }
    let addrs: Vec<std::net::SocketAddr> = lookup_host((host, port))
        .await?
        .filter(|a| family.matches(a))
        .collect();
    if addrs.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            format!(
                "{host} has no {} address (requested by -{})",
                family.as_str(),
                if family == IpFamily::V4 { '4' } else { '6' }
            ),
        ));
    }
    // Try each candidate: the first may be unreachable even when it resolves.
    let mut last: Option<io::Error> = None;
    for a in addrs {
        match TcpStream::connect(a).await {
            Ok(s) => return Ok(s),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| {
        io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            format!("cannot reach {host}"),
        )
    }))
}

/// Connector that speaks plaintext or TLS depending on the target.
#[derive(Clone)]
pub struct TlsCapableConnector {
    config: Arc<ClientConfig>,
    /// How to reach the origin. A SOCKS proxy is handled here rather than in the
    /// target, because it is a property of the connection, not of the request: SOCKS
    /// forwards a TCP stream and never sees the HTTP.
    proxy: crate::socks::Proxy,
    /// `-4` / `-6`, applied to every socket this connector opens.
    family: IpFamily,
}

impl TlsCapableConnector {
    /// Verifying client, trusting the bundled Mozilla root set.
    ///
    /// The bundled roots rather than the platform store: they make behaviour
    /// identical across the three platforms CI builds for, and a mirror set that
    /// works on one developer's machine and not another's is a bug that costs more
    /// than the flexibility is worth.
    pub fn new() -> io::Result<Self> {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let mut config = ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        // Session resumption lowers the handshake cost of the SECOND and later
        // connections to a host, which matters here because a multi-source
        // transfer opens several per source.
        config.resumption = rustls::client::Resumption::in_memory_sessions(64);
        Ok(Self {
            config: Arc::new(config),
            proxy: crate::socks::Proxy::none(),
            family: IpFamily::Any,
        })
    }

    /// Route every connection through a SOCKS proxy.
    ///
    /// This belongs on the connector rather than on a target because SOCKS operates
    /// below HTTP: it forwards a TCP stream and never parses the request, so the choice
    /// affects how the socket is opened, not what is written to it. An HTTP proxy is the
    /// opposite — it rewrites the request line — and lives on the target.
    pub fn with_socks(mut self, proxy: crate::socks::Proxy) -> Self {
        self.proxy = proxy;
        self
    }

    /// Restrict every connection to one IP version (`-4` / `-6`).
    pub fn with_family(mut self, family: IpFamily) -> Self {
        self.family = family;
        self
    }

    /// Client that accepts any certificate. `--insecure` only.
    pub fn insecure() -> io::Result<Self> {
        let provider = rustls::crypto::ring::default_provider();
        let verifier = Arc::new(AcceptAnyCert(Arc::new(provider.clone())));
        let mut config = ClientConfig::builder_with_provider(Arc::new(provider))
            .with_safe_default_protocol_versions()
            .map_err(|e| io::Error::other(format!("tls config: {e}")))?
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_no_client_auth();
        config.resumption = rustls::client::Resumption::in_memory_sessions(64);
        Ok(Self {
            config: Arc::new(config),
            proxy: crate::socks::Proxy::none(),
            family: IpFamily::Any,
        })
    }

    /// Pick the verifying or accept-anything client.
    pub fn with_insecure(insecure: bool) -> io::Result<Self> {
        if insecure {
            eprintln!(
                "hydra: warning: --insecure disables certificate verification. \
                 The bytes you receive are not authenticated: an interception can \
                 substitute a different object and this transfer will still report \
                 success."
            );
            Self::insecure()
        } else {
            Self::new()
        }
    }
}

impl Connector for TlsCapableConnector {
    type Stream = MaybeTls;

    fn connect<'a>(
        &'a self,
        t: &'a Target,
    ) -> Pin<Box<dyn Future<Output = io::Result<Self::Stream>> + Send + 'a>> {
        let config = self.config.clone();
        let socks = self.proxy.clone();
        let family = self.family;
        Box::pin(async move {
            // With a SOCKS proxy the TCP connection goes to the PROXY, and the
            // handshake asks it to reach the origin. The origin name is sent as a
            // name, not an address, so the proxy resolves it — which is the whole
            // point when local DNS cannot.
            let mut tcp = if socks.kind.is_socks() {
                // The family restriction applies to the hop this process actually
                // opens, which is the one to the PROXY. Beyond it the proxy
                // resolves the origin itself and this client has no say — saying
                // otherwise would be a promise we cannot keep.
                let mut sock = connect_family(&socks.host, socks.port, family).await?;
                let _ = sock.set_nodelay(true);
                let (dst_host, dst_port) = t.origin_endpoint();
                crate::socks::handshake(&mut sock, &socks, &dst_host, dst_port).await?;
                sock
            } else {
                let sock = connect_family(&t.host, t.port, family).await?;
                let _ = sock.set_nodelay(true);
                sock
            };
            if !t.tls {
                return Ok(MaybeTls::Plain(tcp));
            }
            // Through a proxy, TLS needs a CONNECT tunnel first: absolute-form
            // requests only work in cleartext, because the proxy has to read the
            // request line, and it cannot read an encrypted one.
            // A CONNECT tunnel is only for an HTTP proxy. Through SOCKS the stream is
            // already end-to-end with the origin, so a CONNECT would be sent TO the
            // origin as a bogus request.
            if t.origin.is_some() && !socks.kind.is_socks() {
                connect_tunnel(&mut tcp, t.proxy_authority()).await?;
            }
            // SNI must be the ORIGIN name, not the socket peer: when routing
            // through a proxy the socket connects to the proxy but the certificate
            // being validated belongs to the origin.
            let sni_host = t.tls_server_name();
            let server_name = ServerName::try_from(sni_host.to_string()).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("bad TLS name: {sni_host}"),
                )
            })?;
            let stream = RustlsConnector::from(config)
                .connect(server_name, tcp)
                .await?;
            Ok(MaybeTls::Tls(Box::new(stream)))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_verifying_client_builds_with_a_nonempty_root_set() {
        let c = TlsCapableConnector::new().expect("verifying client must build");
        // A client with no roots would fail every handshake at runtime rather than
        // at construction, which is the worst place to find out.
        assert!(
            !webpki_roots::TLS_SERVER_ROOTS.is_empty(),
            "the bundled root set must not be empty"
        );
        drop(c);
    }

    #[test]
    fn an_insecure_client_builds() {
        assert!(TlsCapableConnector::insecure().is_ok());
    }

    #[test]
    fn sni_uses_the_origin_name_not_the_socket_peer() {
        // Through a proxy the socket goes to the proxy, but the certificate being
        // validated belongs to the origin. Using the peer name here would either
        // fail every proxied handshake or, worse, validate the wrong identity.
        let direct = Target::direct_tls("example.org", 443, "/f");
        assert_eq!(direct.tls_server_name(), "example.org");
        let proxied = Target::via_proxy("proxy.local", 3128, "example.org:443", "/f");
        assert_eq!(
            proxied.tls_server_name(),
            "example.org",
            "SNI must name the origin, never the proxy"
        );
    }

    #[test]
    fn proxy_authority_is_the_origin_with_its_port() {
        let t = Target::via_proxy("proxy.local", 3128, "example.org:443", "/f");
        assert_eq!(
            t.proxy_authority(),
            "example.org:443",
            "CONNECT must name the ORIGIN and its port, not the proxy"
        );
    }

    #[test]
    fn tls_targets_are_marked_and_plaintext_ones_are_not() {
        assert!(Target::direct_tls("x.org", 443, "/f").tls);
        assert!(!Target::direct("x.org", 80, "/f").tls);
    }

    /// `-4` and `-6` must actually filter candidate addresses.
    ///
    /// Regression test: both flags were declared on the CLI and forwarded through
    /// the wget/curl compat layer, but no DNS or socket code ever read them —
    /// `-6` against an IPv4-only host connected over IPv4 and reported success.
    /// The filter is what makes the flag mean something, and refusing is the
    /// correct outcome when the requested family is absent: falling back to the
    /// other one is precisely what the user asked not to happen.
    #[test]
    fn an_address_family_restriction_selects_and_refuses() {
        use std::net::SocketAddr;
        let v4: SocketAddr = "93.184.216.34:443".parse().unwrap();
        let v6: SocketAddr = "[2606:2800:220:1:248:1893:25c8:1946]:443".parse().unwrap();

        assert!(IpFamily::Any.matches(&v4) && IpFamily::Any.matches(&v6));
        assert!(IpFamily::V4.matches(&v4) && !IpFamily::V4.matches(&v6));
        assert!(IpFamily::V6.matches(&v6) && !IpFamily::V6.matches(&v4));

        // A dual-stack resolution filtered by each flag keeps only its own family.
        let both = [v4, v6];
        assert_eq!(
            both.iter().filter(|a| IpFamily::V4.matches(a)).count(),
            1,
            "-4 must keep exactly the A record"
        );
        assert_eq!(
            both.iter().filter(|a| IpFamily::V6.matches(a)).count(),
            1,
            "-6 must keep exactly the AAAA record"
        );
        // An IPv4-only host under `-6` leaves nothing: the connect must fail
        // rather than quietly fall back to the address the user excluded.
        assert_eq!(
            [v4].iter().filter(|a| IpFamily::V6.matches(a)).count(),
            0,
            "-6 against an IPv4-only host must have no candidate left"
        );
    }

    /// The flags map to a family, and asking for neither restricts nothing.
    #[test]
    fn ip_family_from_flags_reads_the_pair() {
        assert_eq!(IpFamily::from_flags(false, false), IpFamily::Any);
        assert_eq!(IpFamily::from_flags(true, false), IpFamily::V4);
        assert_eq!(IpFamily::from_flags(false, true), IpFamily::V6);
        // Both is rejected by the CLI (`conflicts_with`); if one ever reaches
        // here it must not silently become a restriction the user did not choose.
        assert_eq!(IpFamily::from_flags(true, true), IpFamily::Any);
    }

    /// A family with no matching address must report which host and which flag.
    #[tokio::test]
    async fn connecting_with_no_address_of_the_requested_family_says_so() {
        // A name that resolves to a v4 loopback only. No connection is attempted
        // against a listener: resolution fails the family filter first.
        let e = connect_family("127.0.0.1", 9, IpFamily::V6)
            .await
            .expect_err("an IPv4 literal must not satisfy -6");
        assert_eq!(e.kind(), std::io::ErrorKind::AddrNotAvailable);
        let msg = e.to_string();
        assert!(
            msg.contains("127.0.0.1") && msg.contains("IPv6"),
            "the error must name the host and the family it lacks, got: {msg}"
        );
    }
}
