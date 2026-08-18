// Copyright (C) 2026 Javad Rajabzadeh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Chrome/Firefox native-messaging host for Hydra.
//!
//! The browser spawns this binary and speaks the native-messaging framing on
//! stdio: 4-byte little-endian length, then a JSON document, each way. Every
//! request is forwarded to the running hydra-gui over the loopback socket it
//! publishes in `<app_dir>/ipc.json` (adding the secret token from that
//! file — the browser never sees it), and the GUI's reply is framed back.
//!
//! When the GUI is not running the host launches it minimized and waits for
//! the socket to appear, so clicking a download in the browser "just works"
//! exactly like monitor.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn app_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("hydra")
    }
    #[cfg(not(target_os = "windows"))]
    {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
            .join(".config")
            .join("hydra")
    }
}

/// (port, token) from ipc.json, if the file exists and parses.
fn read_ipc() -> Option<(u16, String)> {
    let text = std::fs::read_to_string(app_dir().join("ipc.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let port = v.get("port")?.as_u64()? as u16;
    let token = v.get("token")?.as_str()?.to_string();
    Some((port, token))
}

/// Try to connect to the GUI right now. The file may be stale from a
/// previous run, so a parse success still has to survive the connect.
fn connect_once() -> Option<(TcpStream, String)> {
    let (port, token) = read_ipc()?;
    let stream = TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_millis(600),
    )
    .ok()?;
    stream.set_nodelay(true).ok();
    Some((stream, token))
}

/// Detached spawn of a GUI binary; true when the process started.
fn spawn_direct(program: std::ffi::OsString) -> bool {
    std::process::Command::new(program)
        .arg("--minimized")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .is_ok()
}

/// Launch hydra-gui minimized: the capture dialog is the only surface that
/// should appear. On macOS the app bundle comes first — it carries the TCC
/// identity the user granted folder access to; a raw sibling binary would
/// hit EACCES on ~/Downloads. `open -ga` exits non-zero when the app is not
/// installed, so its exit status (not spawn success) is the real signal.
fn launch_gui() {
    if let Some(p) = std::env::var_os("HYDRA_GUI_BIN") {
        if spawn_direct(p) {
            return;
        }
    }
    #[cfg(target_os = "macos")]
    {
        let ok = std::process::Command::new("open")
            .args(["-ga", "Hydra Download Manager", "--args", "--minimized"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            return;
        }
    }
    // Dev layout: hydra-host sits next to hydra-gui in target/release.
    if let Ok(me) = std::env::current_exe() {
        if let Some(dir) = me.parent() {
            let sibling = dir.join(if cfg!(windows) {
                "hydra-gui.exe"
            } else {
                "hydra-gui"
            });
            if sibling.exists() && spawn_direct(sibling.into()) {
                return;
            }
        }
    }
    spawn_direct("hydra-gui".into());
}

/// Connect, launching the GUI and polling if needed.
fn connect(launch: bool) -> Option<(TcpStream, String)> {
    if let Some(c) = connect_once() {
        return Some(c);
    }
    if !launch {
        return None;
    }
    launch_gui();
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(300));
        if let Some(c) = connect_once() {
            return Some(c);
        }
    }
    None
}

/// One native-messaging frame from the browser. None on clean EOF.
fn read_frame(stdin: &mut impl Read) -> Option<Vec<u8>> {
    let mut len = [0u8; 4];
    stdin.read_exact(&mut len).ok()?;
    let len = u32::from_le_bytes(len) as usize;
    // Chrome caps extension->host messages well below this; anything larger
    // is framing corruption, and exiting lets the browser respawn us.
    if len == 0 || len > 64 * 1024 * 1024 {
        return None;
    }
    let mut buf = vec![0u8; len];
    stdin.read_exact(&mut buf).ok()?;
    Some(buf)
}

fn write_frame(stdout: &mut impl Write, payload: &[u8]) {
    let _ = stdout.write_all(&(payload.len() as u32).to_le_bytes());
    let _ = stdout.write_all(payload);
    let _ = stdout.flush();
}

fn error_reply(msg: &str) -> Vec<u8> {
    format!("{{\"ok\":false,\"error\":\"{msg}\"}}").into_bytes()
}

/// Send one request over an established GUI connection, returning the reply
/// line. Any IO failure returns None so the caller can reconnect once.
fn round_trip(conn: &mut (TcpStream, String), req: &mut serde_json::Value) -> Option<String> {
    req["token"] = serde_json::Value::String(conn.1.clone());
    let mut line = serde_json::to_string(req).ok()?;
    line.push('\n');
    conn.0.write_all(line.as_bytes()).ok()?;
    let mut reader = BufReader::new(conn.0.try_clone().ok()?);
    let mut reply = String::new();
    reader.read_line(&mut reply).ok()?;
    if reply.trim().is_empty() {
        return None;
    }
    Some(reply)
}

fn main() {
    let mut stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout().lock();
    // One GUI connection kept across frames: connectNative ports send many
    // requests through a single host process.
    let mut conn: Option<(TcpStream, String)> = None;

    while let Some(frame) = read_frame(&mut stdin) {
        let mut req: serde_json::Value = match serde_json::from_slice(&frame) {
            Ok(serde_json::Value::Object(o)) => serde_json::Value::Object(o),
            _ => {
                write_frame(&mut stdout, &error_reply("bad json"));
                continue;
            }
        };
        // Pings probe state; they must not boot the app. Everything else
        // (a capture the browser already cancelled!) must reach a GUI.
        let launch = req.get("type").and_then(|t| t.as_str()) != Some("ping");

        let mut reply = None;
        for attempt in 0..2 {
            if conn.is_none() {
                conn = connect(launch && attempt == 0);
            }
            let Some(c) = conn.as_mut() else { break };
            match round_trip(c, &mut req) {
                Some(r) => {
                    reply = Some(r);
                    break;
                }
                // Stale connection (GUI restarted): drop and retry fresh.
                None => conn = None,
            }
        }
        match reply {
            Some(r) => write_frame(&mut stdout, r.trim().as_bytes()),
            None => write_frame(&mut stdout, &error_reply("hydra is not running")),
        }
    }
}
