//! SOCKS4/4a/5 and HTTP proxy support.
//!
//! # Why three protocols and not one
//!
//! An HTTP forward proxy only helps for HTTP: for plaintext it rewrites the request
//! line to absolute form, and for TLS it opens a `CONNECT` tunnel. Both require the
//! proxy to understand HTTP. SOCKS operates a layer lower — it forwards a TCP stream
//! and does not look inside — which is why it is what Tor, ssh `-D`, and most VPN
//! clients expose, and why a downloader that only speaks HTTP proxies cannot use any
//! of them.
//!
//! # Which SOCKS variants matter
//!
//! * **SOCKS5** is the one to prefer: it carries a hostname, so DNS happens at the
//!   proxy (`socks5h` semantics), which matters when the client cannot resolve the
//!   name at all — a common case behind a restrictive network, and the case in the
//!   environment this was developed in.
//! * **SOCKS4a** also carries a hostname but has no authentication and no IPv6.
//! * **SOCKS4** requires the client to resolve the name first. Supported because old
//!   proxies still speak only this, but it will fail wherever local DNS fails, and
//!   the error says so rather than reporting a generic connection failure.
//!
//! # Deliberate omission
//!
//! Username/password authentication (RFC 1929) is implemented for SOCKS5 because
//! authenticated proxies are common. GSSAPI (the other mandatory-to-implement method)
//! is not: it needs a Kerberos stack, and a downloader that silently fell back to
//! "no authentication" when GSSAPI was demanded would look like a bug rather than an
//! unimplemented feature. The negotiation refuses explicitly instead.

use std::io;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// How to reach an origin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProxyKind {
    /// Direct connection, no proxy.
    None,
    /// HTTP forward proxy: absolute-form requests, or `CONNECT` for TLS.
    Http,
    /// SOCKS4. The client must resolve the hostname itself.
    Socks4,
    /// SOCKS4a. The proxy resolves the hostname.
    Socks4a,
    /// SOCKS5. The proxy resolves the hostname; supports authentication.
    Socks5,
}

impl ProxyKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProxyKind::None => "none",
            ProxyKind::Http => "http",
            ProxyKind::Socks4 => "socks4",
            ProxyKind::Socks4a => "socks4a",
            ProxyKind::Socks5 => "socks5",
        }
    }

    /// True when the proxy speaks SOCKS rather than HTTP.
    pub fn is_socks(&self) -> bool {
        matches!(
            self,
            ProxyKind::Socks4 | ProxyKind::Socks4a | ProxyKind::Socks5
        )
    }
}

/// A parsed proxy specification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proxy {
    pub kind: ProxyKind,
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl Proxy {
    pub fn none() -> Self {
        Self {
            kind: ProxyKind::None,
            host: String::new(),
            port: 0,
            username: None,
            password: None,
        }
    }

    /// Parse a proxy URL.
    ///
    /// Accepts `http://`, `https://` (treated as http-proxy semantics), `socks4://`,
    /// `socks4a://`, `socks5://`, and `socks5h://`, with optional `user:pass@` and an
    /// optional port. A bare `host:port` with no scheme is treated as HTTP by default.
    ///
    /// `socks5h` and `socks5` are both mapped to `Socks5`: this client always sends a
    /// hostname when it has one, because resolving locally and sending an address
    /// defeats the main reason to use a SOCKS proxy.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err("empty proxy specification".into());
        }
        let (scheme, rest) = match raw.split_once("://") {
            Some((s, r)) => (s.to_ascii_lowercase(), r),
            // No scheme: default to an HTTP proxy.
            None => ("http".to_string(), raw),
        };
        let kind = match scheme.as_str() {
            "http" | "https" => ProxyKind::Http,
            "socks4" => ProxyKind::Socks4,
            "socks4a" => ProxyKind::Socks4a,
            "socks5" | "socks5h" => ProxyKind::Socks5,
            other => {
                return Err(format!(
                "unsupported proxy scheme {other:?} (want http, socks4, socks4a, socks5, socks5h)"
            ))
            }
        };
        let rest = rest.trim_end_matches('/');
        // Credentials, if present, sit before the LAST '@' so a password containing
        // '@' still parses.
        let (creds, hostport) = match rest.rsplit_once('@') {
            Some((c, h)) => (Some(c), h),
            None => (None, rest),
        };
        let (username, password) = match creds {
            Some(c) => match c.split_once(':') {
                Some((u, p)) => (Some(u.to_string()), Some(p.to_string())),
                None => (Some(c.to_string()), None),
            },
            None => (None, None),
        };
        if hostport.is_empty() {
            return Err(format!("proxy {raw:?} has no host"));
        }
        let default_port = match kind {
            ProxyKind::Http => 8080,
            _ => 1080,
        };
        // An IPv6 literal is bracketed, so a colon inside it is not a port separator.
        let (host, port) = if let Some(end) = hostport.strip_prefix('[') {
            match end.split_once(']') {
                Some((h, tail)) => {
                    let p = tail
                        .strip_prefix(':')
                        .map(|p| p.parse::<u16>().map_err(|_| format!("bad port in {raw:?}")))
                        .transpose()?
                        .unwrap_or(default_port);
                    (h.to_string(), p)
                }
                None => return Err(format!("unterminated IPv6 literal in {raw:?}")),
            }
        } else {
            match hostport.rsplit_once(':') {
                Some((h, p)) => (
                    h.to_string(),
                    p.parse().map_err(|_| format!("bad port in {raw:?}"))?,
                ),
                None => (hostport.to_string(), default_port),
            }
        };
        if host.is_empty() {
            return Err(format!("proxy {raw:?} has no host"));
        }
        Ok(Self {
            kind,
            host,
            port,
            username,
            password,
        })
    }
}

/// Complete a SOCKS handshake on an already-connected stream, leaving it ready to
/// carry application bytes to `(dst_host, dst_port)`.
pub async fn handshake<S>(s: &mut S, proxy: &Proxy, dst_host: &str, dst_port: u16) -> io::Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    match proxy.kind {
        ProxyKind::Socks5 => socks5(s, proxy, dst_host, dst_port).await,
        ProxyKind::Socks4 | ProxyKind::Socks4a => {
            socks4(
                s,
                proxy,
                dst_host,
                dst_port,
                proxy.kind == ProxyKind::Socks4a,
            )
            .await
        }
        _ => Ok(()),
    }
}

async fn socks5<S>(s: &mut S, proxy: &Proxy, host: &str, port: u16) -> io::Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    // Greeting: offer no-auth, plus username/password when we have credentials.
    let mut greet = vec![0x05u8];
    if proxy.username.is_some() {
        greet.extend_from_slice(&[2, 0x00, 0x02]);
    } else {
        greet.extend_from_slice(&[1, 0x00]);
    }
    s.write_all(&greet).await?;

    let mut sel = [0u8; 2];
    s.read_exact(&mut sel).await?;
    if sel[0] != 0x05 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("proxy replied with SOCKS version {}, expected 5", sel[0]),
        ));
    }
    match sel[1] {
        0x00 => {}
        0x02 => {
            let (Some(u), p) = (proxy.username.as_ref(), proxy.password.clone()) else {
                return Err(io::Error::other(
                    "proxy demands username/password but none was supplied",
                ));
            };
            let pw = p.unwrap_or_default();
            if u.len() > 255 || pw.len() > 255 {
                return Err(io::Error::other(
                    "SOCKS5 username and password are limited to 255 bytes each",
                ));
            }
            let mut auth = vec![0x01u8, u.len() as u8];
            auth.extend_from_slice(u.as_bytes());
            auth.push(pw.len() as u8);
            auth.extend_from_slice(pw.as_bytes());
            s.write_all(&auth).await?;
            let mut ar = [0u8; 2];
            s.read_exact(&mut ar).await?;
            if ar[1] != 0x00 {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "SOCKS5 proxy rejected the credentials",
                ));
            }
        }
        0xFF => {
            return Err(io::Error::other(
                "SOCKS5 proxy refused every authentication method we offered",
            ))
        }
        other => {
            return Err(io::Error::other(format!(
                "SOCKS5 proxy selected authentication method {other:#04x}, which this \
                 client does not implement (GSSAPI is not supported; falling back to \
                 no-auth would look like a bug rather than a missing feature)"
            )))
        }
    }

    // CONNECT request. A domain name is sent as-is so the PROXY resolves it: that is
    // the main reason to use SOCKS at all when local DNS is unavailable.
    let mut req = vec![0x05, 0x01, 0x00];
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(v4)) => {
            req.push(0x01);
            req.extend_from_slice(&v4.octets());
        }
        Ok(std::net::IpAddr::V6(v6)) => {
            req.push(0x04);
            req.extend_from_slice(&v6.octets());
        }
        Err(_) => {
            if host.len() > 255 {
                return Err(io::Error::other(
                    "hostname exceeds the SOCKS5 limit of 255 bytes",
                ));
            }
            req.push(0x03);
            req.push(host.len() as u8);
            req.extend_from_slice(host.as_bytes());
        }
    }
    req.extend_from_slice(&port.to_be_bytes());
    s.write_all(&req).await?;

    let mut head = [0u8; 4];
    s.read_exact(&mut head).await?;
    if head[1] != 0x00 {
        return Err(socks5_error(head[1], host, port));
    }
    // Drain the bound address, whose length depends on the address type. Leaving it
    // in the stream would corrupt the first application read.
    match head[3] {
        0x01 => {
            let mut b = [0u8; 6];
            s.read_exact(&mut b).await?;
        }
        0x04 => {
            let mut b = [0u8; 18];
            s.read_exact(&mut b).await?;
        }
        0x03 => {
            let mut l = [0u8; 1];
            s.read_exact(&mut l).await?;
            let mut b = vec![0u8; l[0] as usize + 2];
            s.read_exact(&mut b).await?;
        }
        other => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("SOCKS5 reply used unknown address type {other:#04x}"),
            ))
        }
    }
    Ok(())
}

/// Map a SOCKS5 reply code to a message that says what to do about it.
fn socks5_error(code: u8, host: &str, port: u16) -> io::Error {
    let (kind, what) = match code {
        0x01 => (io::ErrorKind::Other, "general SOCKS server failure"),
        0x02 => (
            io::ErrorKind::PermissionDenied,
            "connection not allowed by the proxy's ruleset",
        ),
        0x03 => (
            io::ErrorKind::NotConnected,
            "network unreachable from the proxy",
        ),
        0x04 => (
            io::ErrorKind::NotConnected,
            "host unreachable from the proxy",
        ),
        0x05 => (
            io::ErrorKind::ConnectionRefused,
            "connection refused by the origin",
        ),
        0x06 => (io::ErrorKind::TimedOut, "TTL expired"),
        0x07 => (
            io::ErrorKind::Unsupported,
            "command not supported by the proxy",
        ),
        0x08 => (
            io::ErrorKind::Unsupported,
            "address type not supported by the proxy",
        ),
        _ => (io::ErrorKind::Other, "unknown SOCKS5 failure"),
    };
    io::Error::new(
        kind,
        format!("SOCKS5 proxy could not reach {host}:{port}: {what}"),
    )
}

async fn socks4<S>(
    s: &mut S,
    proxy: &Proxy,
    host: &str,
    port: u16,
    allow_hostname: bool,
) -> io::Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let mut req = vec![0x04u8, 0x01];
    req.extend_from_slice(&port.to_be_bytes());
    let mut trailing_host: Option<&str> = None;
    match host.parse::<std::net::Ipv4Addr>() {
        Ok(v4) => req.extend_from_slice(&v4.octets()),
        Err(_) => {
            if !allow_hostname {
                // Being explicit matters: the fix is to use socks4a or socks5, and a
                // generic "connection failed" would send the user hunting elsewhere.
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "SOCKS4 cannot carry the hostname {host:?} — it requires an IPv4 \
                         literal. Use socks4a:// or socks5:// so the proxy resolves the name."
                    ),
                ));
            }
            // SOCKS4a: an invalid 0.0.0.x address signals that a hostname follows.
            req.extend_from_slice(&[0, 0, 0, 1]);
            trailing_host = Some(host);
        }
    }
    // USERID, NUL-terminated. SOCKS4 has no password field at all.
    if let Some(u) = &proxy.username {
        req.extend_from_slice(u.as_bytes());
    }
    req.push(0);
    if let Some(h) = trailing_host {
        req.extend_from_slice(h.as_bytes());
        req.push(0);
    }
    s.write_all(&req).await?;

    let mut rep = [0u8; 8];
    s.read_exact(&mut rep).await?;
    match rep[1] {
        0x5A => Ok(()),
        0x5B => Err(io::Error::new(
            io::ErrorKind::ConnectionRefused,
            format!("SOCKS4 proxy rejected the request to {host}:{port}"),
        )),
        0x5C | 0x5D => Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "SOCKS4 proxy could not verify the client identity (identd)",
        )),
        other => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("SOCKS4 proxy returned unknown status {other:#04x}"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schemes_and_default_ports_parse() {
        let p = Proxy::parse("socks5://127.0.0.1:9050").unwrap();
        assert_eq!(p.kind, ProxyKind::Socks5);
        assert_eq!((p.host.as_str(), p.port), ("127.0.0.1", 9050));

        // Default ports differ by family: 1080 is the SOCKS convention, 8080 for HTTP.
        assert_eq!(Proxy::parse("socks5://tor.local").unwrap().port, 1080);
        assert_eq!(Proxy::parse("socks4://p.local").unwrap().port, 1080);
        assert_eq!(Proxy::parse("http://p.local").unwrap().port, 8080);

        // socks5h and socks5 are the same here: we always send a hostname.
        assert_eq!(
            Proxy::parse("socks5h://p:1080").unwrap().kind,
            ProxyKind::Socks5
        );
        assert_eq!(
            Proxy::parse("socks4a://p:1080").unwrap().kind,
            ProxyKind::Socks4a
        );
    }

    #[test]
    fn a_bare_host_port_is_an_http_proxy() {
        // Assume HTTP when no scheme is given.
        let p = Proxy::parse("proxy.corp:3128").unwrap();
        assert_eq!(p.kind, ProxyKind::Http);
        assert_eq!(p.port, 3128);
    }

    #[test]
    fn credentials_parse_including_awkward_passwords() {
        let p = Proxy::parse("socks5://alice:s3cr3t@p.local:1080").unwrap();
        assert_eq!(p.username.as_deref(), Some("alice"));
        assert_eq!(p.password.as_deref(), Some("s3cr3t"));
        assert_eq!(p.host, "p.local");

        // A password containing '@' must still parse: split on the LAST '@'.
        let q = Proxy::parse("socks5://bob:pa@ss@p.local:1080").unwrap();
        assert_eq!(q.username.as_deref(), Some("bob"));
        assert_eq!(q.password.as_deref(), Some("pa@ss"));
        assert_eq!(q.host, "p.local");

        // Username with no password is legal for SOCKS4's USERID field.
        let r = Proxy::parse("socks4://carol@p.local").unwrap();
        assert_eq!(r.username.as_deref(), Some("carol"));
        assert!(r.password.is_none());
    }

    #[test]
    fn ipv6_literals_are_not_split_at_their_colons() {
        let p = Proxy::parse("socks5://[::1]:1080").unwrap();
        assert_eq!(p.host, "::1");
        assert_eq!(p.port, 1080);
        let q = Proxy::parse("socks5://[2001:db8::1]").unwrap();
        assert_eq!(q.host, "2001:db8::1");
        assert_eq!(q.port, 1080, "no port given, so the SOCKS default applies");
    }

    #[test]
    fn bad_specifications_are_refused_with_a_reason() {
        for bad in ["", "ftp://p:1080", "socks5://", "socks5://p:notaport"] {
            let e = Proxy::parse(bad).unwrap_err();
            assert!(!e.is_empty(), "{bad} must produce an explanation");
        }
        // The unsupported-scheme message should name the alternatives.
        let e = Proxy::parse("ftp://p:1080").unwrap_err();
        assert!(
            e.contains("socks5"),
            "the error should say what IS supported: {e}"
        );
    }

    #[test]
    fn socks_and_http_are_distinguished() {
        assert!(Proxy::parse("socks5://p").unwrap().kind.is_socks());
        assert!(Proxy::parse("socks4a://p").unwrap().kind.is_socks());
        assert!(!Proxy::parse("http://p").unwrap().kind.is_socks());
        assert!(!ProxyKind::None.is_socks());
    }

    /// The SOCKS5 handshake must be byte-exact, so it is checked against a scripted
    /// peer rather than only against a live proxy.
    #[tokio::test]
    async fn socks5_handshake_is_byte_exact_for_a_hostname() {
        use tokio::io::duplex;
        let (client, mut server) = duplex(512);
        let peer = tokio::spawn(async move {
            let mut greet = [0u8; 3];
            server.read_exact(&mut greet).await.unwrap();
            assert_eq!(greet, [0x05, 0x01, 0x00], "version, 1 method, no-auth");
            server.write_all(&[0x05, 0x00]).await.unwrap();

            let mut head = [0u8; 5];
            server.read_exact(&mut head).await.unwrap();
            assert_eq!(
                &head[..4],
                &[0x05, 0x01, 0x00, 0x03],
                "CONNECT by domain name"
            );
            let n = head[4] as usize;
            let mut name = vec![0u8; n + 2];
            server.read_exact(&mut name).await.unwrap();
            assert_eq!(&name[..n], b"example.org");
            assert_eq!(&name[n..], &443u16.to_be_bytes(), "port in network order");

            // Reply with a bound IPv4 address, which the client must drain.
            server
                .write_all(&[0x05, 0x00, 0x00, 0x01, 10, 0, 0, 1, 0x1F, 0x90])
                .await
                .unwrap();
        });
        let mut c = client;
        let p = Proxy::parse("socks5://p:1080").unwrap();
        handshake(&mut c, &p, "example.org", 443).await.unwrap();
        peer.await.unwrap();
    }

    #[tokio::test]
    async fn socks5_authentication_is_negotiated_when_credentials_exist() {
        use tokio::io::duplex;
        let (client, mut server) = duplex(512);
        let peer = tokio::spawn(async move {
            let mut greet = [0u8; 4];
            server.read_exact(&mut greet).await.unwrap();
            assert_eq!(
                greet,
                [0x05, 0x02, 0x00, 0x02],
                "two methods offered: no-auth and username/password"
            );
            server.write_all(&[0x05, 0x02]).await.unwrap();
            let mut hdr = [0u8; 2];
            server.read_exact(&mut hdr).await.unwrap();
            assert_eq!(hdr[0], 0x01, "auth sub-negotiation version");
            let mut u = vec![0u8; hdr[1] as usize];
            server.read_exact(&mut u).await.unwrap();
            assert_eq!(u, b"alice");
            let mut pl = [0u8; 1];
            server.read_exact(&mut pl).await.unwrap();
            let mut pw = vec![0u8; pl[0] as usize];
            server.read_exact(&mut pw).await.unwrap();
            assert_eq!(pw, b"s3cr3t");
            server.write_all(&[0x01, 0x00]).await.unwrap();
            let mut rest = [0u8; 10];
            server.read_exact(&mut rest).await.unwrap();
            server
                .write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                .await
                .unwrap();
        });
        let mut c = client;
        let p = Proxy::parse("socks5://alice:s3cr3t@p:1080").unwrap();
        handshake(&mut c, &p, "1.2.3.4", 80).await.unwrap();
        peer.await.unwrap();
    }

    #[tokio::test]
    async fn a_refused_socks5_connect_explains_which_code_came_back() {
        use tokio::io::duplex;
        let (client, mut server) = duplex(512);
        tokio::spawn(async move {
            let mut g = [0u8; 3];
            server.read_exact(&mut g).await.unwrap();
            server.write_all(&[0x05, 0x00]).await.unwrap();
            let mut h = [0u8; 5];
            server.read_exact(&mut h).await.unwrap();
            let mut rest = vec![0u8; h[4] as usize + 2];
            server.read_exact(&mut rest).await.unwrap();
            // 0x02: not allowed by ruleset.
            server.write_all(&[0x05, 0x02, 0x00, 0x01]).await.unwrap();
        });
        let mut c = client;
        let p = Proxy::parse("socks5://p:1080").unwrap();
        let e = handshake(&mut c, &p, "blocked.example", 443)
            .await
            .unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::PermissionDenied);
        assert!(
            e.to_string().contains("ruleset"),
            "the message must say why, got: {e}"
        );
    }

    #[tokio::test]
    async fn socks4_refuses_a_hostname_and_says_what_to_use_instead() {
        use tokio::io::duplex;
        let (mut client, _server) = duplex(64);
        let p = Proxy::parse("socks4://p:1080").unwrap();
        let e = handshake(&mut client, &p, "example.org", 80)
            .await
            .unwrap_err();
        let msg = e.to_string();
        assert!(
            msg.contains("socks4a") || msg.contains("socks5"),
            "got: {msg}"
        );
    }

    #[tokio::test]
    async fn socks4a_sends_the_hostname_after_a_sentinel_address() {
        use tokio::io::duplex;
        let (client, mut server) = duplex(512);
        let peer = tokio::spawn(async move {
            let mut head = [0u8; 8];
            server.read_exact(&mut head).await.unwrap();
            assert_eq!(head[0], 0x04);
            assert_eq!(head[1], 0x01);
            assert_eq!(&head[2..4], &80u16.to_be_bytes());
            assert_eq!(
                &head[4..8],
                &[0, 0, 0, 1],
                "the invalid 0.0.0.1 address is what signals a trailing hostname"
            );
            // USERID terminator, then the hostname, then its terminator.
            let mut rest = Vec::new();
            let mut byte = [0u8; 1];
            let mut nuls = 0;
            while nuls < 2 {
                server.read_exact(&mut byte).await.unwrap();
                if byte[0] == 0 {
                    nuls += 1;
                } else {
                    rest.push(byte[0]);
                }
            }
            assert_eq!(rest, b"example.org");
            server
                .write_all(&[0x00, 0x5A, 0, 0, 0, 0, 0, 0])
                .await
                .unwrap();
        });
        let mut c = client;
        let p = Proxy::parse("socks4a://p:1080").unwrap();
        handshake(&mut c, &p, "example.org", 80).await.unwrap();
        peer.await.unwrap();
    }

    #[tokio::test]
    async fn an_unimplemented_auth_method_is_refused_not_ignored() {
        use tokio::io::duplex;
        let (client, mut server) = duplex(512);
        tokio::spawn(async move {
            let mut g = [0u8; 3];
            server.read_exact(&mut g).await.unwrap();
            // 0x01 is GSSAPI, which this client does not implement.
            server.write_all(&[0x05, 0x01]).await.unwrap();
        });
        let mut c = client;
        let p = Proxy::parse("socks5://p:1080").unwrap();
        let e = handshake(&mut c, &p, "example.org", 443).await.unwrap_err();
        assert!(
            e.to_string().contains("does not implement"),
            "silently continuing unauthenticated would look like a bug: {e}"
        );
    }
}
