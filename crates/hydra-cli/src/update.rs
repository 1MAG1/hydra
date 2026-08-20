// Copyright (C) 2026 Javad Rajabzadeh
// SPDX-License-Identifier: GPL-3.0-or-later

//! `hydra update`: report whether a newer release exists, with its notes and
//! the download link for this OS/arch. Deliberately report-only — the CLI is
//! frequently installed by package managers or scripts that own the binary,
//! so replacing itself behind their back would be wrong. The GUI has the
//! self-updating flow.
//!
//! The endpoint honours `HYDRA_UPDATE_API`, so the whole command is testable
//! against `cargo run -p hya-updater --example mock_server`.

use std::process::ExitCode;

pub async fn run(json: bool) -> ExitCode {
    let current = env!("CARGO_PKG_VERSION");
    let ua = format!("hydra-cli/{current}");
    let rel = match hya_updater::check_latest(&ua).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("hydra: update check failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    let latest = rel.version().to_string();
    let newer = hya_updater::is_newer(&latest, current);
    let asset_name = hya_updater::cli_asset_name(&latest);
    let asset = rel.asset(&asset_name);

    if json {
        let out = serde_json::json!({
            "current": current,
            "latest": latest,
            "update_available": newer,
            "release_page": rel.html_url,
            "published_at": rel.published_at,
            "asset": asset.map(|a| serde_json::json!({
                "name": a.name,
                "url": a.browser_download_url,
                "size": a.size,
            })),
            "notes": rel.body,
        });
        println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
        return ExitCode::SUCCESS;
    }

    if !newer {
        println!("hydra {current} is up to date (latest release: {latest}).");
        return ExitCode::SUCCESS;
    }

    println!("A new version of hydra is available: {latest} (you have {current})");
    if !rel.published_at.is_empty() {
        // `2026-08-19T00:00:00Z` — the date part reads fine on its own.
        let date = rel.published_at.split('T').next().unwrap_or("");
        println!("Published: {date}");
    }
    if !rel.body.trim().is_empty() {
        println!();
        println!("Release notes:");
        for line in rel.body.lines() {
            println!("  {line}");
        }
    }
    println!();
    if !rel.html_url.is_empty() {
        println!("Release page: {}", rel.html_url);
    }
    match asset {
        Some(a) => println!(
            "Download ({}-{}): {}",
            hya_updater::os_tag(),
            hya_updater::arch_tag(),
            a.browser_download_url
        ),
        None => println!(
            "No prebuilt archive for {}-{} in this release; see the release page.",
            hya_updater::os_tag(),
            hya_updater::arch_tag()
        ),
    }
    ExitCode::SUCCESS
}
