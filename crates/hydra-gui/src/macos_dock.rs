// Copyright (C) 2026 Javad Rajabzadeh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Dock-icon visibility on macOS.
//!
//! "Hide Dock icon" switches the NSApplication activation policy between
//! Regular and Accessory: an accessory app keeps its windows and the
//! menu-bar tray icon but has no Dock tile, no Cmd-Tab entry, and no app
//! menu bar while frontmost. Takes effect immediately in both directions.

#![cfg(target_os = "macos")]

use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};

/// Apply the configured visibility. Main thread only (the iced update loop
/// qualifies); silently a no-op elsewhere rather than crashing in AppKit.
pub fn apply(hidden: bool) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    let policy = if hidden {
        NSApplicationActivationPolicy::Accessory
    } else {
        NSApplicationActivationPolicy::Regular
    };
    let _ = app.setActivationPolicy(policy);
}
