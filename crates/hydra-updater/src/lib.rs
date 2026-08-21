// Copyright (C) 2026 Javad Rajabzadeh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Hydra self-update: release check, asset download, archive extraction and
//! the swap-in-place step.
//!
//! One library serves three callers:
//! - **hydra-gui** — startup check, the update dialog's download-with-progress,
//!   extraction, and launching the finisher binary.
//! - **hydra** (CLI) — `hydra update`, which only reports the newer version
//!   and the download link.
//! - **hydra-updater** (this crate's own bin) — the finisher that runs after
//!   the app exits and copies the new files into place.
//!
//! The release endpoint defaults to the real GitHub API but is overridable via
//! the `HYDRA_UPDATE_API` environment variable, which is how the whole flow is
//! exercised against a local mock server (see `examples/mock_server.rs` and
//! `tests/mock_flow.rs`) before it is pointed at production releases.

use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;

pub mod http;

/// GitHub repository the updater checks, `owner/name`.
pub const REPO: &str = "ja7ad/hydra";

/// Real release API. Override with `HYDRA_UPDATE_API=http://127.0.0.1:8642`
/// to point every updater consumer (GUI, CLI, tests) at a mock server.
pub const DEFAULT_API_BASE: &str = "https://api.github.com";

/// Environment variable that overrides [`DEFAULT_API_BASE`].
pub const API_ENV: &str = "HYDRA_UPDATE_API";

/// The API base currently in effect (env override or the GitHub default).
pub fn api_base() -> String {
    match std::env::var(API_ENV) {
        Ok(v) if !v.trim().is_empty() => v.trim().trim_end_matches('/').to_string(),
        _ => DEFAULT_API_BASE.to_string(),
    }
}

// -------------------------------------------------------------- release data

/// One downloadable file attached to a release.
#[derive(Clone, Debug, Deserialize)]
pub struct ReleaseAsset {
    pub name: String,
    pub browser_download_url: String,
    #[serde(default)]
    pub size: u64,
}

/// The subset of the GitHub release object the updater needs.
#[derive(Clone, Debug, Deserialize)]
pub struct Release {
    /// `v0.2.4` — the leading `v` is the tag convention, not the version.
    pub tag_name: String,
    /// GitHub's pre-release flag; `-rc` tags are published with it set.
    #[serde(default)]
    pub prerelease: bool,
    /// Human release title (`hydra 0.2.4`).
    #[serde(default)]
    pub name: String,
    /// Release notes, GitHub-flavoured markdown.
    #[serde(default)]
    pub body: String,
    /// Web page of the release, for "open in browser" links.
    #[serde(default)]
    pub html_url: String,
    #[serde(default)]
    pub published_at: String,
    #[serde(default)]
    pub assets: Vec<ReleaseAsset>,
}

impl Release {
    /// Version without the tag's `v` prefix.
    pub fn version(&self) -> &str {
        self.tag_name.trim_start_matches('v')
    }

    pub fn asset(&self, name: &str) -> Option<&ReleaseAsset> {
        self.assets.iter().find(|a| a.name == name)
    }

    /// The GUI bundle for this machine, `None` when the release ships none.
    pub fn gui_asset(&self) -> Option<&ReleaseAsset> {
        self.asset_for(gui_asset_name)
    }

    /// The standalone CLI archive for this machine.
    pub fn cli_asset(&self) -> Option<&ReleaseAsset> {
        self.asset_for(cli_asset_name)
    }

    /// Asset lookup across both spellings of the release version: the tag's
    /// first, then the tag without its pre-release suffix. The pipeline
    /// names assets from the workspace manifest version, which may stay on
    /// the plain version while a candidate is cut from a `v0.3.2-rc` tag
    /// (.github/workflows/release.yml accepts either spelling) — that
    /// release ships `hydra-0.3.2-…` assets, which the tag version alone
    /// never matches.
    fn asset_for(&self, name_of: fn(&str) -> String) -> Option<&ReleaseAsset> {
        let v = self.version();
        if let Some(a) = self.asset(&name_of(v)) {
            return Some(a);
        }
        let core = version_core(v);
        (core != v).then(|| self.asset(&name_of(core))).flatten()
    }
}

/// `latest` is strictly newer than `current`, comparing dotted numeric parts
/// (`v` prefixes and any `+build` suffix are ignored). On an equal core a
/// stable release outranks any `-rc` build and rc builds order by their
/// number: `0.3.0 > 0.3.0-rc2 > 0.3.0-rc1`. Unparsable versions compare as
/// not-newer — an updater must never "upgrade" onto a version it cannot
/// read.
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

/// `((maj, min, pat), pre_rank)` — `pre_rank` is `u64::MAX` for a stable
/// version and the suffix's number for a pre-release (`-rc2` → 2, bare
/// `-rc` → 0), so tuple order IS release order.
fn parse_version(v: &str) -> Option<((u64, u64, u64), u64)> {
    let v = v.trim().trim_start_matches('v');
    let v = v.split('+').next().unwrap_or("");
    let (core, pre) = match v.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (v, None),
    };
    let mut it = core.split('.');
    let maj = it.next()?.parse().ok()?;
    let min = it.next().unwrap_or("0").parse().ok()?;
    let pat = it.next().unwrap_or("0").parse().ok()?;
    let rank = match pre {
        None => u64::MAX,
        Some(p) => {
            let digits: String = p.chars().filter(|c| c.is_ascii_digit()).collect();
            digits.parse().unwrap_or(0)
        }
    };
    Some(((maj, min, pat), rank))
}

// ----------------------------------------------------------------- platform

/// OS tag as the release pipeline spells it (`linux` | `macos` | `windows`).
pub fn os_tag() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

/// Architecture tag as the release pipeline spells it (`amd64` | `arm64`).
pub fn arch_tag() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "amd64"
    }
}

/// Archive format per platform: the pipeline zips Windows, tars the rest.
pub fn archive_ext() -> &'static str {
    if cfg!(target_os = "windows") {
        "zip"
    } else {
        "tar.gz"
    }
}

/// The version without its pre-release or build suffix (`0.3.2-rc` →
/// `0.3.2`), the spelling the release pipeline names assets with whenever
/// the workspace manifest stays on the plain version.
pub fn version_core(version: &str) -> &str {
    let v = version.trim().trim_start_matches('v');
    v.split(['-', '+']).next().unwrap_or(v)
}

/// Release asset holding the GUI bundle for this machine:
/// `hydra-0.2.4-macos-arm64.tar.gz`.
pub fn gui_asset_name(version: &str) -> String {
    format!(
        "hydra-{version}-{}-{}.{}",
        os_tag(),
        arch_tag(),
        archive_ext()
    )
}

/// Release asset holding the standalone CLI for this machine:
/// `hydra-cli-0.2.4-macos-arm64.tar.gz`.
pub fn cli_asset_name(version: &str) -> String {
    format!(
        "hydra-cli-{version}-{}-{}.{}",
        os_tag(),
        arch_tag(),
        archive_ext()
    )
}

// -------------------------------------------------------------------- checks

/// Fetch the latest release from the effective API base ([`api_base`]).
pub async fn check_latest(user_agent: &str) -> io::Result<Release> {
    check_latest_at(&api_base(), REPO, user_agent).await
}

/// Fetch the latest release from an explicit API base — the mock-test entry
/// point ([`check_latest`] is this with the env-resolved base).
pub async fn check_latest_at(base: &str, repo: &str, user_agent: &str) -> io::Result<Release> {
    let url = format!("{base}/repos/{repo}/releases/latest");
    // Release JSON with notes and a dozen assets runs tens of KB; 4 MB is a
    // refusal threshold, not a size expectation.
    let body = http::get_bytes(&url, user_agent, 4 * 1024 * 1024).await?;
    serde_json::from_slice(&body)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("release JSON: {e}")))
}

/// Fetch the release the given channel should offer.
///
/// Stable (`beta == false`) is exactly [`check_latest`]: `/releases/latest`,
/// where GitHub never lists pre-releases. The beta channel additionally
/// scans the release list for the newest `-rc` pre-release and offers it
/// only while it is ahead of the stable release — once stable catches up
/// (`0.3.0` after `0.3.0-rc2`), beta serves the stable release again.
pub async fn check_channel(user_agent: &str, beta: bool) -> io::Result<Release> {
    check_channel_at(&api_base(), REPO, user_agent, beta).await
}

/// [`check_channel`] against an explicit API base — the mock-test entry
/// point.
pub async fn check_channel_at(
    base: &str,
    repo: &str,
    user_agent: &str,
    beta: bool,
) -> io::Result<Release> {
    let latest = check_latest_at(base, repo, user_agent).await;
    if !beta {
        return latest;
    }
    let url = format!("{base}/repos/{repo}/releases?per_page=30");
    // A failed or unparsable list degrades beta to stable behaviour rather
    // than blocking updates: the pre-release is an extra offer, not a
    // dependency.
    let pre = match http::get_bytes(&url, user_agent, 8 * 1024 * 1024).await {
        Ok(body) => serde_json::from_slice::<Vec<Release>>(&body)
            .ok()
            .and_then(newest_prerelease),
        Err(_) => None,
    };
    match (latest, pre) {
        (Ok(stable), Some(pre)) if is_newer(pre.version(), stable.version()) => Ok(pre),
        (Ok(stable), _) => Ok(stable),
        // A repo whose only releases are pre-releases has no `latest`
        // (GitHub 404s); the rc is still a valid beta offer.
        (Err(_), Some(pre)) => Ok(pre),
        (Err(e), None) => Err(e),
    }
}

/// The highest-versioned pre-release in a release list. The GitHub flag is
/// authoritative; a `-rc` tag counts too, so a release someone forgot to
/// mark pre-release still reaches the beta channel.
fn newest_prerelease(list: Vec<Release>) -> Option<Release> {
    list.into_iter()
        .filter(|r| r.prerelease || r.version().contains("-rc"))
        .fold(None::<Release>, |best, r| match best {
            Some(b) if !is_newer(r.version(), b.version()) => Some(b),
            _ => Some(r),
        })
}

// ----------------------------------------------------------------- checksums

/// Extract the sha256 hex digest for `asset_name` from a `SHA256SUMS.txt`
/// body (`<hex>  <name>` lines, `sha256sum` format).
pub fn sum_for(sums: &str, asset_name: &str) -> Option<String> {
    for line in sums.lines() {
        let mut it = line.split_whitespace();
        let (Some(hex), Some(name)) = (it.next(), it.next()) else {
            continue;
        };
        // `sha256sum` writes `<hex> *<name>` in binary mode.
        if name.trim_start_matches('*') == asset_name && hex.len() == 64 {
            return Some(hex.to_ascii_lowercase());
        }
    }
    None
}

/// sha256 of a file, lowercase hex.
pub fn file_sha256(path: &Path) -> io::Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    let out = h.finalize();
    let mut hex = String::with_capacity(64);
    for b in out {
        hex.push_str(&format!("{b:02x}"));
    }
    Ok(hex)
}

// ---------------------------------------------------------------- extraction

/// Unpack a release archive (`.tar.gz` or `.zip`, decided by the file name)
/// into `dest` and return the bundle root: the single top-level directory
/// when there is exactly one (the pipeline's layout), `dest` itself otherwise.
pub fn extract(archive: &Path, dest: &Path) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dest)?;
    let name = archive.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if name.ends_with(".zip") {
        let f = std::fs::File::open(archive)?;
        let mut z = zip::ZipArchive::new(f).map_err(io::Error::other)?;
        // `extract` sanitises entry names (no absolute paths, no `..`).
        z.extract(dest).map_err(io::Error::other)?;
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        let f = std::fs::File::open(archive)?;
        let gz = flate2::read::GzDecoder::new(f);
        let mut t = tar::Archive::new(gz);
        // tar::Archive::unpack refuses entries that escape `dest`.
        t.unpack(dest)?;
    } else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unrecognised archive format: {name}"),
        ));
    }
    // The pipeline stages everything under one `hydra-<ver>-<os>-<arch>/`
    // directory; when that is the whole archive, that directory is the root.
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dest)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    if entries.len() == 1 && entries[0].is_dir() {
        return Ok(entries.remove(0));
    }
    Ok(dest.to_path_buf())
}

// -------------------------------------------------------------------- apply

/// What the swap step did, for the log.
#[derive(Debug, Default)]
pub struct ApplyReport {
    pub replaced: Vec<String>,
    pub skipped: Vec<String>,
}

/// Copy the new bundle's files over the installed ones.
///
/// Only files that ALREADY EXIST in `install_dir` are replaced: a plain
/// unpacked-archive install gets every binary refreshed, while a macOS
/// `.app` (whose `Contents/MacOS/` holds just the executables) or a partial
/// install never gains stray files it did not have. Everything else in the
/// bundle (extensions/, scripts/, licences) is left alone and reported in
/// `skipped`.
///
/// Each replacement retries with backoff for up to ~20 s per file: on
/// Windows the old process's executable stays locked until it has fully
/// exited, and the retry loop IS the "wait for exit" mechanism — no PID
/// polling required. Locked-but-replaceable executables are handled with the
/// classic rename dance: the running file may not be overwritten, but it may
/// be renamed away, and a fresh copy takes its name.
pub fn apply(src_root: &Path, install_dir: &Path) -> io::Result<ApplyReport> {
    let mut report = ApplyReport::default();
    for entry in std::fs::read_dir(src_root)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            report
                .skipped
                .push(entry.file_name().to_string_lossy().into_owned());
            continue;
        }
        let name = entry.file_name();
        let dest = install_dir.join(&name);
        if !dest.exists() {
            report.skipped.push(name.to_string_lossy().into_owned());
            continue;
        }
        replace_file(&path, &dest)?;
        report.replaced.push(name.to_string_lossy().into_owned());
    }
    Ok(report)
}

/// Replace `dest` with `src`, retrying while `dest` is still locked by the
/// exiting process.
fn replace_file(src: &Path, dest: &Path) -> io::Result<()> {
    let mut last = None;
    for attempt in 0..40u32 {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        match try_replace(src, dest) {
            Ok(()) => return Ok(()),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| io::Error::other("replace failed")))
}

fn try_replace(src: &Path, dest: &Path) -> io::Result<()> {
    // Move the old file aside first: overwriting a running executable fails
    // on Windows, but renaming it succeeds — and on Unix the rename keeps a
    // rollback copy an interrupted update can be recovered from by hand.
    let old = dest.with_extension(match dest.extension().and_then(|e| e.to_str()) {
        Some(ext) => format!("{ext}.old"),
        None => "old".to_string(),
    });
    let _ = std::fs::remove_file(&old);
    std::fs::rename(dest, &old)?;
    if let Err(e) = std::fs::copy(src, dest) {
        // Roll the rename back so a failed copy leaves the install runnable.
        let _ = std::fs::rename(&old, dest);
        return Err(e);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(&old) {
            let _ = std::fs::set_permissions(dest, meta.permissions());
        } else {
            let _ = std::fs::set_permissions(dest, std::fs::Permissions::from_mode(0o755));
        }
    }
    // Best effort: on Windows the old exe may stay locked until process
    // teardown completes; the leftover `.old` is harmless and replaced on
    // the next update.
    let _ = std::fs::remove_file(&old);
    Ok(())
}

/// Remove `.old` leftovers a previous update could not delete (Windows keeps
/// the old exe locked until teardown finishes). Called on the next startup.
pub fn sweep_old_files(install_dir: &Path) {
    if let Ok(rd) = std::fs::read_dir(install_dir) {
        for entry in rd.flatten() {
            let name = entry.file_name();
            if name.to_string_lossy().ends_with(".old") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

/// Directory downloads and extractions are staged in:
/// `<os temp>/hydra-update`.
pub fn staging_dir() -> PathBuf {
    std::env::temp_dir().join("hydra-update")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_compare() {
        assert!(is_newer("v0.2.4", "0.2.3"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(is_newer("0.2.10", "0.2.9"));
        assert!(!is_newer("0.2.3", "0.2.3"));
        assert!(!is_newer("v0.2.2", "0.2.3"));
        assert!(is_newer("0.3.0-rc1", "0.2.9"));
        // On an equal core, stable outranks rc and rc numbers order.
        assert!(is_newer("0.3.0", "0.3.0-rc2"));
        assert!(!is_newer("0.3.0-rc1", "0.3.0"));
        assert!(is_newer("0.3.0-rc2", "0.3.0-rc1"));
        assert!(is_newer("v0.3.0-rc1", "0.3.0-rc"));
        assert!(!is_newer("0.3.0-rc", "0.3.0-rc"));
        // Unparsable input must never look like an upgrade.
        assert!(!is_newer("nightly", "0.2.3"));
        assert!(!is_newer("0.3.0", "unknown"));
    }

    fn rel(tag: &str, prerelease: bool) -> Release {
        Release {
            tag_name: tag.into(),
            prerelease,
            name: String::new(),
            body: String::new(),
            html_url: String::new(),
            published_at: String::new(),
            assets: vec![],
        }
    }

    #[test]
    fn beta_channel_picks_the_newest_prerelease() {
        // Flagged pre-releases and unflagged `-rc` tags both qualify; the
        // highest version wins regardless of list order.
        let list = vec![
            rel("v0.2.4", false),
            rel("v0.3.0-rc1", true),
            rel("v0.3.0-rc2", false), // forgot the flag; the tag still counts
            rel("v0.2.3", false),
        ];
        assert_eq!(
            newest_prerelease(list).map(|r| r.tag_name),
            Some("v0.3.0-rc2".into())
        );
        assert!(newest_prerelease(vec![rel("v0.2.4", false)]).is_none());
    }

    #[test]
    fn asset_names_follow_the_release_pipeline() {
        let gui = gui_asset_name("0.2.4");
        let cli = cli_asset_name("0.2.4");
        assert!(gui.starts_with("hydra-0.2.4-"));
        assert!(cli.starts_with("hydra-cli-0.2.4-"));
        for n in [&gui, &cli] {
            assert!(n.contains(os_tag()) && n.contains(arch_tag()));
            assert!(n.ends_with(archive_ext()));
        }
    }

    #[test]
    fn rc_tag_finds_assets_named_from_the_manifest_version() {
        // The real pipeline: tag `v0.3.2-rc`, manifest still `0.3.2`, so
        // every asset is spelled without the suffix. Missing it here is
        // what made the beta channel report "you are on the latest
        // version" while an rc was published.
        let mut r = rel("v0.3.2-rc", true);
        for name in [gui_asset_name("0.3.2"), cli_asset_name("0.3.2")] {
            r.assets.push(ReleaseAsset {
                name,
                browser_download_url: String::new(),
                size: 0,
            });
        }
        assert_eq!(
            r.gui_asset().map(|a| a.name.as_str()),
            Some(gui_asset_name("0.3.2").as_str())
        );
        assert_eq!(
            r.cli_asset().map(|a| a.name.as_str()),
            Some(cli_asset_name("0.3.2").as_str())
        );

        // A manifest that does carry the suffix keeps working, and a
        // release without an asset for this machine still resolves to None.
        let mut suffixed = rel("v0.3.2-rc", true);
        suffixed.assets.push(ReleaseAsset {
            name: gui_asset_name("0.3.2-rc"),
            browser_download_url: String::new(),
            size: 0,
        });
        assert_eq!(
            suffixed.gui_asset().map(|a| a.name.as_str()),
            Some(gui_asset_name("0.3.2-rc").as_str())
        );
        assert!(suffixed.cli_asset().is_none());
        assert!(rel("v0.3.2", false).gui_asset().is_none());
    }

    #[test]
    fn version_core_strips_suffixes() {
        assert_eq!(version_core("v0.3.2-rc"), "0.3.2");
        assert_eq!(version_core("0.3.2-rc.2"), "0.3.2");
        assert_eq!(version_core("0.3.2+build7"), "0.3.2");
        assert_eq!(version_core("0.3.2"), "0.3.2");
    }

    #[test]
    fn sums_parsing() {
        let sums = "abc\n\
            0123456789012345678901234567890123456789012345678901234567890123  hydra-0.2.4-linux-amd64.tar.gz\n\
            ABCDEF6789012345678901234567890123456789012345678901234567890123 *hydra-0.2.4-windows-amd64.zip\n";
        assert_eq!(
            sum_for(sums, "hydra-0.2.4-linux-amd64.tar.gz").as_deref(),
            Some("0123456789012345678901234567890123456789012345678901234567890123")
        );
        // Binary-mode `*` prefix and uppercase hex both normalise.
        assert_eq!(
            sum_for(sums, "hydra-0.2.4-windows-amd64.zip").as_deref(),
            Some("abcdef6789012345678901234567890123456789012345678901234567890123")
        );
        assert_eq!(sum_for(sums, "missing.tar.gz"), None);
    }

    #[test]
    fn release_json_parses() {
        let json = "{
            \"tag_name\": \"v0.2.4\",
            \"name\": \"hydra 0.2.4\",
            \"body\": \"## What's Changed\\n- faster\",
            \"html_url\": \"https://github.com/ja7ad/hydra/releases/tag/v0.2.4\",
            \"published_at\": \"2026-08-01T00:00:00Z\",
            \"assets\": [
                {\"name\": \"hydra-0.2.4-macos-arm64.tar.gz\",
                 \"browser_download_url\": \"https://example.com/a.tar.gz\",
                 \"size\": 123}
            ]
        }";
        let r: Release = serde_json::from_str(json).unwrap();
        assert_eq!(r.version(), "0.2.4");
        assert_eq!(r.assets.len(), 1);
        assert!(r.asset("hydra-0.2.4-macos-arm64.tar.gz").is_some());
    }
}
