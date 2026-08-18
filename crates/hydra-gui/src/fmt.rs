// Copyright (C) 2026 Javad Rajabzadeh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Human formatting: `121.66 MB`, `1.027 MB/sec`,
//! `3 min 32 sec`, `Aug 17 15:48:32 2026`.

use chrono::{DateTime, Local, TimeZone};

/// `121.66 MB` (two decimals — the download-list spelling).
pub fn size2(bytes: u64) -> String {
    size_n(bytes, 2)
}

/// `121.665 MB` (three decimals — the progress-dialog spelling).
pub fn size3(bytes: u64) -> String {
    size_n(bytes, 3)
}

fn size_n(bytes: u64, prec: usize) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.prec$} GB", b / GB)
    } else if b >= MB {
        format!("{:.prec$} MB", b / MB)
    } else if b >= KB {
        format!("{:.prec$} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

/// `1.027 MB/sec`, `200.666 KB/sec`.
pub fn rate(bytes_per_sec: f64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    if bytes_per_sec >= MB {
        format!("{:.3} MB/sec", bytes_per_sec / MB)
    } else if bytes_per_sec >= 1.0 {
        format!("{:.3} KB/sec", bytes_per_sec / KB)
    } else {
        String::new()
    }
}

/// `3 min 32 sec`, `1 hr 12 min`, `45 sec`.
pub fn eta(secs: u64) -> String {
    if secs >= 3600 {
        format!("{} hr {} min", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{} min {} sec", secs / 60, secs % 60)
    } else {
        format!("{secs} sec")
    }
}

/// `5.80%` — the Status column while a transfer runs.
pub fn pct(done: u64, size: u64) -> String {
    if size == 0 {
        return String::new();
    }
    format!("{:.2}%", done as f64 * 100.0 / size as f64)
}

/// `Aug 17 15:48:32 2026` — the Last Try Date column.
pub fn date(unix: i64) -> String {
    match Local.timestamp_opt(unix, 0) {
        chrono::LocalResult::Single(t) => t.format("%b %d %H:%M:%S %Y").to_string(),
        _ => String::new(),
    }
}

/// Current unix time, seconds.
pub fn now_unix() -> i64 {
    Local::now().timestamp()
}

/// `15:48` for scheduler time fields.
pub fn hhmm(t: &DateTime<Local>) -> String {
    t.format("%H:%M").to_string()
}
