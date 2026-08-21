// Copyright (C) 2026 Javad Rajabzadeh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Application self-update: the startup version check and the "Update Now"
//! pipeline (download → checksum → extract → launch the finisher → exit).
//!
//! The heavy lifting lives in the `hya-updater` crate; this module adapts it
//! to iced — the check is a one-shot `Task::perform` future, the update run
//! is a `Task::run` stream so the dialog can draw live progress. Endpoints
//! come from `hya_updater::api_base()`, so `HYDRA_UPDATE_API` pointed at the
//! mock server (`cargo run -p hya-updater --example mock_server`) exercises
//! this whole path without touching real releases.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// What the dialog needs to know about the newer release.
#[derive(Clone, Debug)]
pub struct UpdateInfo {
    pub version: String,
    /// Release notes, GitHub-flavoured markdown.
    pub notes: String,
    /// Release web page, for "open in browser".
    pub html_url: String,
    pub asset_name: String,
    pub asset_url: String,
    pub size: u64,
    /// `SHA256SUMS.txt` asset, when the release publishes one.
    pub sums_url: Option<String>,
}

/// Progress of a running update, streamed into the dialog.
#[derive(Clone, Debug)]
pub enum UpdateEvent {
    Progress(u64, Option<u64>),
    Verifying,
    Preparing,
    /// The finisher is running; the app must now exit so it can swap files.
    ReadyToRestart,
    Cancelled,
    Failed(String),
}

fn user_agent() -> String {
    format!("hydra-gui/{}", env!("CARGO_PKG_VERSION"))
}

/// Ask the release API whether a newer GUI bundle exists for this machine.
/// `beta` (Options > General > "Download Beta channel") also considers `-rc`
/// pre-releases when one is ahead of the stable release.
///
/// `Ok(None)` covers both "up to date" and "newer release exists but has no
/// asset for this OS/arch" — the dialog can only offer what it can install.
pub async fn check(beta: bool) -> Result<Option<UpdateInfo>, String> {
    let rel = hya_updater::check_channel(&user_agent(), beta)
        .await
        .map_err(|e| e.to_string())?;
    if !hya_updater::is_newer(rel.version(), env!("CARGO_PKG_VERSION")) {
        return Ok(None);
    }
    let Some(asset) = rel.gui_asset() else {
        crate::log::warn(&format!(
            "update {} available but has no {}-{} bundle",
            rel.version(),
            hya_updater::os_tag(),
            hya_updater::arch_tag()
        ));
        return Ok(None);
    };
    Ok(Some(UpdateInfo {
        version: rel.version().to_string(),
        notes: rel.body.clone(),
        html_url: rel.html_url.clone(),
        asset_name: asset.name.clone(),
        asset_url: asset.browser_download_url.clone(),
        size: asset.size,
        sums_url: rel
            .asset("SHA256SUMS.txt")
            .map(|a| a.browser_download_url.clone()),
    }))
}

/// Run the full update as an event stream: download the archive into the OS
/// temp staging dir, verify it, extract it, put the finisher binary in a
/// path that will not be overwritten, and start it detached.
pub fn run(
    info: UpdateInfo,
    cancel: Arc<AtomicBool>,
) -> impl iced::futures::Stream<Item = UpdateEvent> {
    iced::stream::channel(64, async move |mut tx| {
        use iced::futures::SinkExt;
        match drive(info, cancel, &mut tx).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {
                let _ = tx.send(UpdateEvent::Cancelled).await;
            }
            Err(e) => {
                crate::log::error(&format!("update failed: {e}"));
                let _ = tx.send(UpdateEvent::Failed(e.to_string())).await;
            }
        }
    })
}

async fn drive(
    info: UpdateInfo,
    cancel: Arc<AtomicBool>,
    tx: &mut iced::futures::channel::mpsc::Sender<UpdateEvent>,
) -> std::io::Result<()> {
    use iced::futures::SinkExt;
    let ua = user_agent();
    let stage = hya_updater::staging_dir();
    let archive = stage.join(&info.asset_name);

    // Download. Progress goes through try_send: a full channel drops a
    // repaint, never blocks the transfer; the terminal events use send().
    {
        let mut progress_tx = tx.clone();
        let cancel = cancel.clone();
        hya_updater::http::download_to_file(&info.asset_url, &ua, &archive, move |got, total| {
            let _ = progress_tx.try_send(UpdateEvent::Progress(got, total));
            !cancel.load(Ordering::Relaxed)
        })
        .await?;
    }

    // Verify against the release's published checksums when it has any.
    if let Some(sums_url) = &info.sums_url {
        let _ = tx.send(UpdateEvent::Verifying).await;
        let sums = hya_updater::http::get_bytes(sums_url, &ua, 1024 * 1024).await?;
        let sums = String::from_utf8_lossy(&sums).into_owned();
        if let Some(want) = hya_updater::sum_for(&sums, &info.asset_name) {
            let got = hya_updater::file_sha256(&archive)?;
            if got != want {
                let _ = std::fs::remove_file(&archive);
                return Err(std::io::Error::other(
                    "checksum mismatch — the downloaded archive was discarded",
                ));
            }
        }
    }

    let _ = tx.send(UpdateEvent::Preparing).await;
    let unpack = stage.join("unpacked");
    let _ = std::fs::remove_dir_all(&unpack);
    let bundle = hya_updater::extract(&archive, &unpack)?;

    // The finisher: prefer the NEW release's copy (version-matched to what it
    // installs), fall back to the one shipped next to the running app. Either
    // way it runs from the staging dir so the swap never overwrites it.
    let updater_name = if cfg!(target_os = "windows") {
        "hydra-updater.exe"
    } else {
        "hydra-updater"
    };
    let exe = std::env::current_exe()?;
    let install_dir = exe
        .parent()
        .ok_or_else(|| std::io::Error::other("executable has no parent directory"))?
        .to_path_buf();
    let updater_src: PathBuf = [bundle.join(updater_name), install_dir.join(updater_name)]
        .into_iter()
        .find(|p| p.is_file())
        .ok_or_else(|| {
            std::io::Error::other(format!(
                "no {updater_name} in the release bundle or next to the app; \
                 the downloaded update is at {}",
                bundle.display()
            ))
        })?;
    let updater = stage.join(updater_name);
    std::fs::copy(&updater_src, &updater)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&updater, std::fs::Permissions::from_mode(0o755));
    }

    let mut cmd = std::process::Command::new(&updater);
    cmd.arg("--src-dir")
        .arg(&bundle)
        .arg("--install-dir")
        .arg(&install_dir)
        .arg("--relaunch")
        .arg(&exe);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW | DETACHED_PROCESS: no console flash, and the
        // finisher outlives this process cleanly.
        cmd.creation_flags(0x0800_0000 | 0x0000_0008);
    }
    cmd.spawn()?;
    crate::log::info(&format!(
        "update {} downloaded; finisher started, exiting to let it swap files",
        info.version
    ));
    let _ = tx.send(UpdateEvent::ReadyToRestart).await;
    Ok(())
}

/// Startup housekeeping: remove `.old` files a previous update left behind
/// (Windows cannot delete the old exe while it is still tearing down).
pub fn sweep_leftovers() {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            hya_updater::sweep_old_files(dir);
        }
    }
}
