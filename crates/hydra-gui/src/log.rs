// Copyright (C) 2026 Javad Rajabzadeh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Leveled session log in `<app_dir>/logs/gui.log`.
//!
//! `[2026-08-18 06:36:12] [INFO ] start #3 https://…` — level filter comes
//! from `log_level` in config.toml (`debug`/`info`/`warn`/`error`, default
//! `info`) or the `HYDRA_LOG` environment variable, which wins. Failure to
//! log must never fail the operation being logged.

use std::io::Write;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd)]
pub enum Level {
    Debug = 0,
    Info = 1,
    Warn = 2,
    Error = 3,
}

static FILTER: AtomicU8 = AtomicU8::new(Level::Info as u8);

pub fn set_level(name: &str) {
    let lvl = match name.to_ascii_lowercase().as_str() {
        "debug" | "trace" | "verbose" => Level::Debug,
        "warn" | "warning" => Level::Warn,
        "error" => Level::Error,
        _ => Level::Info,
    };
    FILTER.store(lvl as u8, Ordering::Relaxed);
}

/// Apply config level, then let HYDRA_LOG override it for one-off debugging.
pub fn init(config_level: Option<&str>) {
    if let Some(l) = config_level {
        set_level(l);
    }
    if let Ok(env) = std::env::var("HYDRA_LOG") {
        set_level(&env);
    }
}

fn write(level: Level, tag: &str, line: &str) {
    if (level as u8) < FILTER.load(Ordering::Relaxed) {
        return;
    }
    let dir = crate::model::app_dir().join("logs");
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("gui.log"))
    {
        let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
        // One formatted string, one write_all: `writeln!` with arguments
        // emits a write per fragment, and the extbus socket threads log
        // concurrently with the UI thread — that interleaves mid-line.
        let record = format!("[{ts}] [{tag}] {line}\n");
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = LOCK.lock();
        let _ = f.write_all(record.as_bytes());
    }
}

pub fn debug(line: &str) {
    write(Level::Debug, "DEBUG", line);
}

pub fn info(line: &str) {
    write(Level::Info, "INFO ", line);
}

pub fn warn(line: &str) {
    write(Level::Warn, "WARN ", line);
}

pub fn error(line: &str) {
    write(Level::Error, "ERROR", line);
}

/// Back-compat alias for the original single-level call sites.
pub fn log(line: &str) {
    info(line);
}
