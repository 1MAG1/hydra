//! An in-process FTP server, for exercising the FTP client end to end.
//!
//! The sandbox this was developed in refuses outbound connections to port 21 (a port
//! policy, not a domain one — no grant can lift it), and refuses `bind()` on any address,
//! so neither a live server nor a loopback one is available. The HTTP side already solved
//! this: origins run over `tokio::io::duplex` behind the `Connector` trait, which is the
//! same async byte stream a socket provides. This does the same for FTP.
//!
//! What that buys is worth being precise about. It exercises the real client code path —
//! reply parsing including the multi-line form, `PASV` and the separate data connection,
//! `REST` offsets, `TYPE I`, `ABOR`, authentication, and the short-transfer check. It does
//! NOT measure network latency, and the control-channel round trips it counts are logical
//! ones. So the preemption cost is reported as a round-trip COUNT, which is a protocol
//! property and transfers to a real network, rather than as a duration, which would not.
//!
//! The server implements exactly the subset the client uses, and refuses the rest with 502
//! rather than pretending — a mock that accepts everything tests nothing.

use crate::{Connector, Target};
use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

/// Knobs for making the server behave badly on purpose.
#[derive(Clone)]
pub struct FtpControl {
    /// Refuse the login, to test that credentials are handled and errors are legible.
    pub reject_login: Arc<Mutex<bool>>,
    /// Credentials the server will accept. `None` means anonymous access.
    pub require: Arc<Mutex<Option<(String, String)>>>,
    /// Refuse `REST`, so the client must fall back rather than assume ranges work.
    pub no_rest: Arc<Mutex<bool>>,
    /// Refuse `SIZE`, which RFC 959 does not define (it is an RFC 3659 extension).
    pub no_size: Arc<Mutex<bool>>,
    /// Send fewer bytes than asked and close, to prove a short transfer is not success.
    pub truncate_after: Arc<Mutex<Option<u64>>>,
    /// Greet with a multi-line banner, the form that breaks naive reply parsers.
    pub multiline_banner: Arc<Mutex<bool>>,
    /// Control-channel commands received, by verb.
    pub commands: Arc<Mutex<Vec<String>>>,
    /// Data connections opened. One per range is the cost FTP cannot avoid.
    pub data_connections: Arc<AtomicU64>,
}

impl Default for FtpControl {
    fn default() -> Self {
        Self {
            reject_login: Arc::new(Mutex::new(false)),
            require: Arc::new(Mutex::new(None)),
            no_rest: Arc::new(Mutex::new(false)),
            no_size: Arc::new(Mutex::new(false)),
            truncate_after: Arc::new(Mutex::new(None)),
            multiline_banner: Arc::new(Mutex::new(false)),
            commands: Arc::new(Mutex::new(Vec::new())),
            data_connections: Arc::new(AtomicU64::new(0)),
        }
    }
}

impl FtpControl {
    /// Verbs seen on the control channel, in order.
    pub fn verbs(&self) -> Vec<String> {
        self.commands.lock().unwrap().clone()
    }

    /// How many times a given verb was issued.
    pub fn count(&self, verb: &str) -> usize {
        self.commands
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.as_str() == verb)
            .count()
    }
}

/// Deterministic object content, so a fetched range can be checked byte for byte.
pub fn byte_at(off: u64) -> u8 {
    (off.wrapping_mul(2_654_435_761) >> 13) as u8
}

/// A set of in-process FTP servers, addressed by port.
pub struct FtpOriginSet {
    control_port: u16,
    size: u64,
    ctl: FtpControl,
    /// Data connections awaiting a RETR, keyed by the port handed out in the PASV reply.
    ///
    /// The start offset is delivered when RETR arrives, not when PASV is answered: `REST`
    /// applies to the NEXT transfer, and a client may legitimately send `PASV` first. An
    /// earlier version of this harness captured the offset at PASV time and so served every
    /// range from zero — which looked exactly like a client bug.
    pending: Arc<Mutex<HashMap<u16, tokio::sync::oneshot::Sender<u64>>>>,
    /// The most recently advertised passive port, which the next RETR belongs to.
    last_pasv: Arc<Mutex<Option<u16>>>,
    next_data_port: Arc<AtomicU64>,
}

impl FtpOriginSet {
    /// A server on `port` offering an object of `size` bytes.
    pub fn new(port: u16, size: u64) -> (Arc<Self>, FtpControl) {
        let ctl = FtpControl::default();
        let s = Arc::new(Self {
            control_port: port,
            size,
            ctl: ctl.clone(),
            pending: Arc::new(Mutex::new(HashMap::new())),
            last_pasv: Arc::new(Mutex::new(None)),
            next_data_port: Arc::new(AtomicU64::new(40000)),
        });
        (s, ctl)
    }

    fn serve_control(self: Arc<Self>, mut sock: DuplexStream) {
        tokio::spawn(async move {
            let banner = if *self.ctl.multiline_banner.lock().unwrap() {
                "220-Welcome to the test archive\r\n220-Be excellent to each other\r\n220 Ready.\r\n"
            } else {
                "220 Ready.\r\n"
            };
            if sock.write_all(banner.as_bytes()).await.is_err() {
                return;
            }
            let mut rest: u64 = 0;
            let mut logged_in = false;
            let mut pending_user: Option<String> = None;
            let mut buf = String::new();
            let mut chunk = [0u8; 1024];
            loop {
                let n = match sock.read(&mut chunk).await {
                    Ok(0) | Err(_) => return,
                    Ok(n) => n,
                };
                buf.push_str(&String::from_utf8_lossy(&chunk[..n]));
                while let Some(i) = buf.find("\r\n") {
                    let line = buf[..i].to_string();
                    buf.drain(..i + 2);
                    let mut it = line.splitn(2, ' ');
                    let verb = it.next().unwrap_or("").to_ascii_uppercase();
                    let arg = it.next().unwrap_or("").to_string();
                    self.ctl.commands.lock().unwrap().push(verb.clone());

                    let reply: String = match verb.as_str() {
                        "USER" => {
                            pending_user = Some(arg.clone());
                            let need = self.ctl.require.lock().unwrap().clone();
                            match need {
                                // A server wanting a password says 331, and the client must
                                // follow with PASS.
                                Some(_) => "331 Password required.\r\n".into(),
                                None => {
                                    logged_in = true;
                                    "230 Logged in anonymously.\r\n".into()
                                }
                            }
                        }
                        "PASS" => {
                            let need = self.ctl.require.lock().unwrap().clone();
                            let ok = !*self.ctl.reject_login.lock().unwrap()
                                && match (&need, &pending_user) {
                                    (Some((u, p)), Some(gu)) => gu == u && &arg == p,
                                    _ => true,
                                };
                            if ok {
                                logged_in = true;
                                "230 Logged in.\r\n".into()
                            } else {
                                "530 Login incorrect.\r\n".into()
                            }
                        }
                        _ if !logged_in => "530 Log in first.\r\n".into(),
                        "TYPE" => {
                            if arg.eq_ignore_ascii_case("I") {
                                "200 Type set to I.\r\n".into()
                            } else {
                                // ASCII mode would rewrite line endings and corrupt binary
                                // objects, so the client must ask for I and this asserts it.
                                "504 Only TYPE I is supported here.\r\n".into()
                            }
                        }
                        "SIZE" => {
                            if *self.ctl.no_size.lock().unwrap() {
                                "502 SIZE not implemented.\r\n".into()
                            } else {
                                format!("213 {}\r\n", self.size)
                            }
                        }
                        "MDTM" => "213 20260815120000\r\n".into(),
                        "REST" => {
                            if *self.ctl.no_rest.lock().unwrap() {
                                "502 REST not implemented.\r\n".into()
                            } else {
                                rest = arg.trim().parse().unwrap_or(0);
                                format!("350 Restarting at {rest}.\r\n")
                            }
                        }
                        "PASV" => {
                            let dp = self.next_data_port.fetch_add(1, Ordering::Relaxed) as u16;
                            *self.last_pasv.lock().unwrap() = Some(dp);
                            let (p1, p2) = (dp / 256, dp % 256);
                            format!("227 Entering Passive Mode (127,0,0,1,{p1},{p2}).\r\n")
                        }
                        "RETR" => {
                            // Release the waiting data connection with the offset that is
                            // current NOW. This is the point at which REST takes effect.
                            let port = *self.last_pasv.lock().unwrap();
                            if let Some(p) = port {
                                if let Some(tx) = self.pending.lock().unwrap().remove(&p) {
                                    let _ = tx.send(rest);
                                }
                            }
                            "150 Opening BINARY mode data connection.\r\n226 Transfer complete.\r\n"
                                .into()
                        }
                        "ABOR" => {
                            // Two replies is the normal case for an in-progress transfer,
                            // and it is exactly the cost the capability declares.
                            "426 Transfer aborted.\r\n226 Closing data connection.\r\n".into()
                        }
                        "QUIT" => {
                            let _ = sock.write_all(b"221 Bye.\r\n").await;
                            return;
                        }
                        // A mock that accepts everything tests nothing.
                        _ => format!("502 {verb} not implemented.\r\n"),
                    };
                    if sock.write_all(reply.as_bytes()).await.is_err() {
                        return;
                    }
                }
            }
        });
    }

    fn serve_data(
        self: Arc<Self>,
        mut sock: DuplexStream,
        start_rx: tokio::sync::oneshot::Receiver<u64>,
    ) {
        let size = self.size;
        let trunc = *self.ctl.truncate_after.lock().unwrap();
        self.ctl.data_connections.fetch_add(1, Ordering::Relaxed);
        tokio::spawn(async move {
            // Nothing is sent until RETR arrives, which is also when the start offset is
            // known. A server that streamed at connect time would serve the wrong bytes for
            // any client that sends PASV before REST.
            let Ok(start) = start_rx.await else { return };
            // FTP has no way to say where a transfer ends, so the server streams to
            // end-of-file from `start`. Enforcing the upper bound is the client's job, and
            // that asymmetry is exactly why preemption costs a round trip here.
            let mut off = start;
            let limit = trunc.map(|t| (start + t).min(size)).unwrap_or(size);
            let mut buf = vec![0u8; 32 * 1024];
            while off < limit {
                let n = ((limit - off) as usize).min(buf.len());
                for (i, b) in buf[..n].iter_mut().enumerate() {
                    *b = byte_at(off + i as u64);
                }
                if sock.write_all(&buf[..n]).await.is_err() {
                    return;
                }
                off += n as u64;
            }
        });
    }
}

impl Connector for FtpOriginSet {
    type Stream = DuplexStream;

    fn connect<'a>(
        &'a self,
        t: &'a Target,
    ) -> Pin<Box<dyn Future<Output = io::Result<DuplexStream>> + Send + 'a>> {
        // `self` is behind an Arc in practice; clone the pieces the tasks need.
        let is_control = t.port == self.control_port;
        let port = t.port;
        Box::pin(async move {
            let (client, server) = tokio::io::duplex(64 * 1024);
            // Reconstruct an Arc to hand to the spawned task. Safe because every caller
            // holds the set in an Arc for the duration of the test.
            let me: Arc<FtpOriginSet> = Arc::new(FtpOriginSet {
                control_port: self.control_port,
                size: self.size,
                ctl: self.ctl.clone(),
                pending: self.pending.clone(),
                last_pasv: self.last_pasv.clone(),
                next_data_port: self.next_data_port.clone(),
            });
            if is_control {
                me.serve_control(server);
            } else {
                let (tx, rx) = tokio::sync::oneshot::channel();
                self.pending.lock().unwrap().insert(port, tx);
                me.serve_data(server, rx);
            }
            Ok(client)
        })
    }
}
