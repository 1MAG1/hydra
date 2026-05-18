//! FTP (RFC 959, plus `SIZE`/`REST` from RFC 3659).
//!
//! # Why FTP is the interesting case, not just another protocol
//!
//! The scheduler's result rests on preemption being free. An HTTP range request names both
//! ends — `Range: bytes=100-199` — and the far end is enforced by the CLIENT: to shrink a
//! laggard's range you stop reading and close the connection. No message to the server, no
//! round trip, and nothing wasted but bytes already in flight. That is why the makespan
//! excess is independent of object size.
//!
//! FTP has ranged reads but not that property. `REST <offset>` says where a transfer
//! STARTS; there is no command that says where it ends. A `RETR` runs to end-of-file, so
//! shrinking a range means aborting the transfer:
//!
//! ```text
//!   -> ABOR                      (control channel)
//!   <- 426 transfer aborted      (on the data connection's behalf)
//!   <- 226 closing data connection
//!   -> PASV                      (a new data connection for the next range)
//!   <- 227 entering passive mode (h1,h2,h3,h4,p1,p2)
//! ```
//!
//! Two round trips on the control channel where HTTP needs zero. That is not a detail to
//! paper over in an abstraction layer: a scheduler that prices FTP reassignment as free
//! will reassign constantly and spend the entire benefit on control traffic. So the cost is
//! declared in [`Capabilities::preempt_cost_rtt`] and the engine scales its repair deadband
//! by it — FTP reassigns less eagerly, which is the correct response to expensive
//! preemption, not a limitation of the implementation.
//!
//! # What FTP cannot offer at all
//!
//! No validator. `SIZE` plus `MDTM` is a weak identity: two mirrors can agree on both and
//! still serve different builds — which was already observed on HTTP mirrors in this
//! project, with matching sizes and differing content. Cross-mirror assembly is therefore
//! refused for FTP rather than attempted, because a file spliced from two versions passes
//! every length check and is silently wrong.
//!
//! # Authentication
//!
//! FTP's credentials are part of the URL (`ftp://user:pass@host/path`), unlike HTTP where
//! they are a header. That makes them easy to leak: they must not reach a log line, a
//! `Debug` format, or a progress display. [`Endpoint`]'s `Debug` is hand-written to redact
//! them, and the raw exchange this module records replaces the `PASS` argument.

use crate::scheme::{Capabilities, Endpoint, Fetcher, SchemeProbe};
use crate::{Connector, SparseSink};
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// FTP over a `Connector`-provided stream.
pub struct FtpFetcher<C: Connector> {
    conn: Arc<C>,
    /// Control-channel round trips spent on preemption, for measurement.
    pub abort_rtts: AtomicU64,
}

impl<C: Connector> FtpFetcher<C> {
    pub fn new(conn: Arc<C>) -> Self {
        Self {
            conn,
            abort_rtts: AtomicU64::new(0),
        }
    }
}

/// One line of an FTP reply, parsed.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub code: u16,
    pub text: String,
}

impl Reply {
    /// 1xx-3xx are progress or success; 4xx-5xx are failure.
    pub fn is_ok(&self) -> bool {
        (100..400).contains(&self.code)
    }
}

/// Parse an FTP reply, handling the multi-line form.
///
/// A multi-line reply opens `220-first line` and closes with the SAME code followed by a
/// space. Treating the first line as the whole reply is the classic FTP client bug: the
/// remaining lines are then read as the response to the NEXT command, and every subsequent
/// exchange is off by one.
pub fn parse_reply(buf: &str) -> Option<(Reply, usize)> {
    let first_end = buf.find("\r\n")?;
    let first = &buf[..first_end];
    if first.len() < 4 {
        return None;
    }
    let code: u16 = first.get(..3)?.parse().ok()?;
    let sep = first.as_bytes()[3];
    if sep == b' ' {
        return Some((
            Reply {
                code,
                text: first[4..].to_string(),
            },
            first_end + 2,
        ));
    }
    if sep != b'-' {
        return None;
    }
    // Multi-line: scan for a line beginning `<code> `.
    let terminator = format!("{code} ");
    let mut pos = first_end + 2;
    let mut text = first[4..].to_string();
    while pos < buf.len() {
        let end = pos + buf[pos..].find("\r\n")?;
        let line = &buf[pos..end];
        if line.starts_with(&terminator) {
            text.push(' ');
            text.push_str(&line[4..]);
            return Some((Reply { code, text }, end + 2));
        }
        text.push(' ');
        text.push_str(line.trim());
        pos = end + 2;
    }
    None
}

/// Parse the host and port out of a `227 Entering Passive Mode (h1,h2,h3,h4,p1,p2)` reply.
///
/// The port is `p1 * 256 + p2`. Servers vary the surrounding punctuation freely, so the
/// digits are extracted rather than the format matched.
pub fn parse_pasv(text: &str) -> Option<(String, u16)> {
    let open = text.find('(')?;
    let close = text[open..].find(')')? + open;
    let nums: Vec<u16> = text[open + 1..close]
        .split(',')
        .map(|s| s.trim().parse().ok())
        .collect::<Option<Vec<u16>>>()?;
    if nums.len() != 6 {
        return None;
    }
    let host = format!("{}.{}.{}.{}", nums[0], nums[1], nums[2], nums[3]);
    let port = nums[4].checked_mul(256)?.checked_add(nums[5])?;
    Some((host, port))
}

/// Parse a `213 <size>` reply.
pub fn parse_size(text: &str) -> Option<u64> {
    text.split_whitespace().next()?.parse().ok()
}

/// A control-channel session.
struct Control<S> {
    stream: S,
    buf: String,
    /// The exchange so far, with credentials redacted.
    log: String,
}

impl<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> Control<S> {
    async fn read_reply(&mut self) -> io::Result<Reply> {
        loop {
            if let Some((r, used)) = parse_reply(&self.buf) {
                self.buf.drain(..used);
                self.log.push_str(&format!("< {} {}\r\n", r.code, r.text));
                return Ok(r);
            }
            let mut chunk = [0u8; 4096];
            let n = self.stream.read(&mut chunk).await?;
            if n == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "control connection closed mid-reply",
                ));
            }
            self.buf.push_str(&String::from_utf8_lossy(&chunk[..n]));
        }
    }

    async fn send(&mut self, line: &str) -> io::Result<()> {
        // Redact the password before it can reach the log that --server-response prints.
        let shown = if line.starts_with("PASS ") {
            "PASS <redacted>"
        } else {
            line
        };
        self.log.push_str(&format!("> {shown}\r\n"));
        self.stream
            .write_all(format!("{line}\r\n").as_bytes())
            .await
    }

    async fn cmd(&mut self, line: &str) -> io::Result<Reply> {
        self.send(line).await?;
        self.read_reply().await
    }

    /// Send a command and require success, with the server's own text on failure.
    async fn expect(&mut self, line: &str) -> io::Result<Reply> {
        let r = self.cmd(line).await?;
        if !r.is_ok() {
            return Err(io::Error::other(format!(
                "FTP {} rejected: {} {}",
                line.split_whitespace().next().unwrap_or(line),
                r.code,
                r.text
            )));
        }
        Ok(r)
    }
}

/// Open a control connection, greet, log in, and switch to binary mode.
async fn login<C: Connector>(conn: &Arc<C>, t: &Endpoint) -> io::Result<Control<C::Stream>> {
    let target = crate::Target {
        host: t.host.clone(),
        port: t.port,
        path: t.path.clone(),
        origin: t.origin.clone().map(|(h, p)| format!("{h}:{p}")),
        tls: false,
        headers: Vec::new(),
        agent: None,
    };
    let stream = conn.connect(&target).await?;
    let mut c = Control {
        stream,
        buf: String::new(),
        log: String::new(),
    };
    let greeting = c.read_reply().await?;
    if !greeting.is_ok() {
        return Err(io::Error::other(format!(
            "FTP server refused the connection: {} {}",
            greeting.code, greeting.text
        )));
    }
    let (user, pass) = t.ftp_login();
    let r = c.cmd(&format!("USER {user}")).await?;
    // 230 = logged in without a password; 331 = password wanted.
    if r.code == 331 {
        let r2 = c.cmd(&format!("PASS {pass}")).await?;
        if !r2.is_ok() {
            // Do not echo the credential back in the error.
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("FTP login failed for user {user}: {} {}", r2.code, r2.text),
            ));
        }
    } else if !r.is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("FTP USER rejected: {} {}", r.code, r.text),
        ));
    }
    // Binary mode is not optional: ASCII mode rewrites line endings, which corrupts every
    // non-text object and makes SIZE disagree with the bytes delivered.
    c.expect("TYPE I").await?;
    Ok(c)
}

impl<C: Connector> Fetcher for FtpFetcher<C> {
    fn scheme(&self) -> &'static str {
        "ftp"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::ftp()
    }

    fn probe<'a>(
        &'a self,
        t: &'a Endpoint,
    ) -> Pin<Box<dyn Future<Output = io::Result<SchemeProbe>> + Send + 'a>> {
        Box::pin(async move {
            let mut c = login(&self.conn, t).await?;
            let size = match c.cmd(&format!("SIZE {}", t.path)).await {
                Ok(r) if r.code == 213 => parse_size(&r.text).unwrap_or(0),
                // A server without SIZE (it is an RFC 3659 extension) leaves the size
                // unknown, which the engine handles by streaming rather than failing.
                _ => 0,
            };
            // REST support decides whether ranged reads are possible at all. Asking is
            // cheaper and more honest than assuming: 350 means it will resume.
            let ranged = matches!(c.cmd("REST 1").await, Ok(r) if r.code == 350);
            if ranged {
                // Cancel the pending restart so it cannot leak into the next transfer.
                let _ = c.cmd("REST 0").await;
            }
            let mdtm = c
                .cmd(&format!("MDTM {}", t.path))
                .await
                .ok()
                .filter(|r| r.code == 213)
                .map(|r| r.text.trim().to_string());
            let _ = c.cmd("QUIT").await;
            Ok(SchemeProbe {
                size,
                ranged,
                // SIZE+MDTM is recorded for display but flagged weak: two mirrors can
                // agree on both and serve different builds, so this must never be used to
                // justify assembling one file from several FTP sources.
                validator: mdtm.map(|m| format!("MDTM {m}")),
                weak_validator: true,
                content_type: None,
                raw: c.log,
            })
        })
    }

    fn fetch_range<'a>(
        &'a self,
        t: &'a Endpoint,
        lo: u64,
        hi: u64,
        sink: Arc<SparseSink>,
    ) -> Pin<Box<dyn Future<Output = io::Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let mut c = login(&self.conn, t).await?;
            let pasv = c.expect("PASV").await?;
            let (dh, dp) = parse_pasv(&pasv.text).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("could not parse PASV reply: {}", pasv.text),
                )
            })?;
            // Restart at the range's start. This is the half of a range FTP can express.
            if lo > 0 {
                let r = c.cmd(&format!("REST {lo}")).await?;
                if r.code != 350 {
                    return Err(io::Error::other(format!(
                        "FTP REST {lo} refused ({} {}); this server cannot serve ranges",
                        r.code, r.text
                    )));
                }
            }
            let dtarget = crate::Target {
                host: dh,
                port: dp,
                path: String::new(),
                origin: None,
                tls: false,
                headers: Vec::new(),
                agent: None,
            };
            let mut data = self.conn.connect(&dtarget).await?;
            c.send(&format!("RETR {}", t.path)).await?;
            let r = c.read_reply().await?;
            if !r.is_ok() {
                return Err(io::Error::other(format!(
                    "FTP RETR refused: {} {}",
                    r.code, r.text
                )));
            }

            // Read exactly the range. The server will keep sending to end-of-file, so the
            // upper bound is enforced here — and stopping early is what costs the ABOR.
            let want = hi.saturating_sub(lo);
            let mut off = lo;
            let mut buf = vec![0u8; 64 * 1024];
            while off < hi {
                let room = ((hi - off) as usize).min(buf.len());
                let n = match data.read(&mut buf[..room]).await {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
                    Err(e) => return Err(e),
                };
                sink.write_at(off, &buf[..n])?;
                off += n as u64;
            }
            let got = off - lo;
            if got < want {
                // A short transfer reported as success is how a truncating server produces
                // a corrupt file that passes every length check.
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    format!("FTP delivered {got} of {want} requested bytes"),
                ));
            }

            // The range is satisfied but the server is still sending: this is the
            // preemption cost the abstraction exists to expose. ABOR, drain the replies,
            // and the data connection is finished.
            drop(data);
            let _ = c.send("ABOR").await;
            self.abort_rtts.fetch_add(1, Ordering::Relaxed);
            // 426 (aborted) then 226 (closing), or just 226 if the transfer had finished.
            if let Ok(first) = c.read_reply().await {
                if first.code == 426 {
                    let _ = c.read_reply().await;
                    self.abort_rtts.fetch_add(1, Ordering::Relaxed);
                }
            }
            let _ = c.cmd("QUIT").await;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_multi_line_reply_is_consumed_whole() {
        // The classic FTP client bug: treat the first line as the whole reply, and every
        // subsequent exchange is off by one because the remaining lines are read as the
        // next command's response.
        let raw = "220-Welcome to the archive\r\n220-Rules apply\r\n220 Ready.\r\nNEXT";
        let (r, used) = parse_reply(raw).expect("a multi-line reply must parse");
        assert_eq!(r.code, 220);
        assert!(
            r.text.contains("Ready"),
            "the final line belongs to the reply"
        );
        assert!(
            r.text.contains("Rules"),
            "intermediate lines are part of it too"
        );
        assert_eq!(&raw[used..], "NEXT", "exactly the reply is consumed");
    }

    #[test]
    fn a_single_line_reply_stops_at_its_own_terminator() {
        let raw = "213 1048576\r\n213 later\r\n";
        let (r, used) = parse_reply(raw).unwrap();
        assert_eq!(r.code, 213);
        assert_eq!(r.text, "1048576");
        assert_eq!(&raw[used..], "213 later\r\n");
    }

    #[test]
    fn a_code_hyphen_line_that_never_closes_is_not_a_reply() {
        // Returning the partial text would make the client act on a reply the server has
        // not finished sending.
        assert!(parse_reply("220-opening\r\n220-still going\r\n").is_none());
    }

    #[test]
    fn pasv_port_arithmetic_is_p1_times_256_plus_p2() {
        let (h, p) = parse_pasv("Entering Passive Mode (192,168,0,7,195,80)").unwrap();
        assert_eq!(h, "192.168.0.7");
        assert_eq!(p, 195 * 256 + 80, "50000; a wrong formula connects nowhere");
        // Servers punctuate freely, so the digits are what matters.
        let (h2, p2) = parse_pasv("227 PASV ok (10,0,0,1,4,1).").unwrap();
        assert_eq!((h2.as_str(), p2), ("10.0.0.1", 1025));
        assert!(parse_pasv("227 no tuple here").is_none());
        // A 6-tuple whose port overflows must be rejected, not wrapped.
        assert!(parse_pasv("(1,2,3,4,999,999)").is_none());
    }

    #[test]
    fn size_reply_parses_and_junk_does_not() {
        assert_eq!(parse_size("1048576"), Some(1048576));
        assert_eq!(parse_size("2957812 bytes"), Some(2957812));
        assert_eq!(parse_size("unknown"), None);
    }

    #[test]
    fn ftp_declares_preemption_as_expensive_and_offers_no_validator() {
        // These two facts drive engine behaviour: the deadband is scaled by the cost, and
        // cross-mirror assembly is refused for want of a validator.
        let caps = Capabilities::ftp();
        assert_eq!(caps.preempt_cost_rtt, 2.0, "ABOR+reply, then PASV+reply");
        assert!(
            !caps.client_bounded_ranges,
            "REST gives a start, never an end"
        );
        assert!(!caps.has_validators);
        assert!(
            caps.ranged,
            "REST does give ranged reads, just not free preemption"
        );
    }
}
