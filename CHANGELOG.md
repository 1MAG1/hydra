# Changelog

All notable changes to the Hydra project are documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [Unreleased]

### Added

- **ABI Specification (`docs/ffi/ABI.md`)**: Added the canonical human-readable specification for the `libhydra` C ABI — FFI design principles, the formal ABI 1 stability policy (what is frozen, what may be appended, what forces ABI 2), the full ownership/encoding/error/threading contract, the event-queue rationale and its ordering and drop guarantees, and the compatibility-testing matrix. `include/hydra.h` now carries a concise summary and points at it.
- **Frozen ABI Baseline (`crates/hydra-ffi/abi/abi-1.manifest`)**: Added a machine-checkable record of ABI 1 — every enumerator value, field offset and width, struct size and exported symbol.
- **ABI Stability Gate (`scripts/ffi-abi-compat.sh`, `make ffi-compat`)**: Added a CI gate that derives the current layout from the header with a generated C probe and enforces the ABI 1 rules against the frozen baseline: fields may not move or change width, enumerators may not be renumbered, symbols may not disappear, and only the two size-prefixed configuration structs may grow. Additions the contract permits pass.
- **Forward-Compatibility Probe (`examples/ffi-c/compat_probe.c`)**: Added an old-header/new-library conformance program. `scripts/ffi-c-example.sh` now builds it against `include/hydra.h` as published by *every* release tag and links each against the library from the current branch, with a guard wall after each caller-allocated struct so a byte written past an older header's extent is caught at test time.
- **Wider ABI CI Matrix**: The FFI conformance job now also runs under Clang on Linux (in addition to GCC), and the stability gate runs on Linux, macOS and Windows.

### Changed

- **`libhydra` as a First-Class Product**: Documentation now draws an explicit line between `hydra` (the GPL application — CLI, GUI, host) and `libhydra` (the permissively licensed embeddable engine, with its own version, release archives and compatibility promise). Language bindings are documented as independent downstream projects that need only the published header and a release archive.

---

## [0.3.6] - 2026-08-22

### Added

- **`hydra compat-link` CLI Subcommand (`hydra-cli`)**: Added a dedicated subcommand to plan, verify, and install `wget` and `curl` dialect symlinks/shims into `$PATH` or custom directories, checking against `$PATH` shadowing so users know if another tool takes precedence. *(e.g. `hydra compat-link --dry-run`, `hydra compat-link`, `hydra compat-link --dir ~/.local/bin`)*
- **macOS Application Bundle In-Place Updates (`hydra-updater` / `hydra-gui`)**: Added full updater support for macOS `.app` bundles (`Hydra Download Manager.app`), maintaining proper bundle directory structures, re-stamping `Contents/Info.plist` bundle versions, and applying ad-hoc code signatures.
- **Elevated Self-Updates (`hydra-updater` / `hydra-gui`)**: Added native authentication prompts (`osascript` on macOS, `pkexec`/`sudo` on Linux) to allow in-place updating of system-wide / root-owned installations (e.g. in `/usr/local/bin` or `/Applications`) without requiring full re-installation.
- **Windows Apps & Features Integration (`install.ps1` / `uninstall.ps1`)**: Registered Hydra in Windows *Installed apps / Apps & features* with display icons, publisher info, and direct uninstaller registration for native Windows Settings integration.
- **Desktop & Start Menu Shortcuts (`install.ps1`)**: Added Start Menu and optional Desktop (`-Desktop`) shortcut generation with embedded high-resolution icons and application metadata.
- **Offline Windows Uninstaller Packaging**: Bundled `uninstall.ps1` into Windows release archives to allow complete offline uninstallation of files, shortcuts, PATH modifications, and registry entries.

### Fixed

- **Immediate Transport Failure Range Reclaim (`hya-net` / `hya-core`)**: Fixed long stalls on closed pooled connections, truncated responses, or refused socket requests by immediately detecting transport failures and re-assigning pending byte ranges instead of waiting out the full stall timeout.
- **Stale In-Flight Range Discard (`hya-core`)**: Added request start timestamping to prevent discarded or superseded chunks from being erroneously credited after a stall or range preemption.
- **Repeated Boolean Flags in CLI Dialects (`hydra-cli`)**: Fixed dialect canonicalizer rejecting repeated flags like `curl -s -sS` or `wget -q -q`, while correctly preserving counting flags like `-vv` for verbosity levels.
- **Sudo User Directory Resolution (`install.sh` / `uninstall.sh`)**: Fixed script installation when executed via `sudo` by resolving `$SUDO_USER` to properly place and clean desktop icons, `.desktop` files, and browser native messaging manifests in the user's home directory instead of `/root`.
- **macOS Application Unregistration & Graceful Quit (`uninstall.sh`)**: Added Launch Services unregistration (`lsregister -u`) and graceful AppleScript quit messaging before removing the `.app` bundle to prevent orphaned Spotlight entries.
- **Windows Process Locking During Upgrades (`install.ps1` / `uninstall.ps1`)**: Added graceful window close requests (`CloseMainWindow`) and process termination for running instances (`hydra-gui`, `hydra-host`, `hydra-updater`) before attempting file replacement or uninstallation.

### Changed & Refactored

- **FFI Metric Definition Clarity (`hydra-ffi` / `include/hydra.h`)**: Clarified `stall_count` documentation in `hydra_progress_t` and `hydra_metrics_t` to specify that it accounts for ranges reclaimed from both no-progress timeouts and transport/socket connection failures.
- **Update Dialog Elevation Guidance (`hydra-gui`)**: Added notices in the in-app update dialog when an update will prompt for administrator authentication, localized across 10 supported languages.

---

## [0.3.5] - 2026-08-21

### Added

- **C/C++ Foreign Function Interface (`libhydra` / `hydra-ffi`)**: Added stable C ABI bindings and header (`include/hydra.h`) to embed Hydra's engine in C, C++, Python, Swift, Go, etc. *(e.g. `hydra_engine_new()`, `hydra_engine_add_job()`, `hydra_job_start()`, `hydra_engine_poll_event()`)*
- **Self-Update Engine & CLI Command (`hydra-updater` / `hydra-cli`)**: Added `hydra update` command with automatic release discovery from GitHub releases and direct asset downloads. *(e.g. `hydra update`, `hydra update --beta`, `hydra update --download-only`, `hydra update --json`)*
- **GUI In-App Updater (`hydra-gui`)**: Added automated update checking on launch, interactive update modal dialog with rendered markdown release notes, and single-click restart finisher.
- **Beta / Pre-Release Update Channel**: Added beta channel support to opt in to pre-release builds in both CLI (`hydra update --beta`) and GUI (*Options → General → Include pre-release versions*).
- **Package Installer & Bundle Detection**: Automatically detects package-managed or read-only installations (macOS `.app` bundle, system `/usr/bin`), offering direct installer downloads (`.deb`, `.rpm`, `.dmg`, `.pkg`, `.exe`) when in-place updating is not possible.
- **Periodic Download Quotas & Rollover (`hydra-gui`)**: Added configurable data usage limits over rolling periods (hourly, daily, weekly, monthly) with auto-pausing when exceeded and auto-resuming upon quota reset. *(Configurable in Options → Connection)*
- **Live Quota Dashboard (`hydra-gui`)**: Added visual quota consumption gauges, percentage indicators, and real-time countdown timers until the next quota rollover in the Connection settings tab.
- **Homebrew Formula Support**: Added official Homebrew tap distribution for macOS and Linux CLI installations. *(e.g. `brew install ja7ad/tap/hydra`)*
- **Dynamic Speed Limit Badge & Status (`hydra-gui`)**: Added `(Limited)` indicator badge in transfer tables and progress detail views when a transfer or engine bandwidth is actively throttled.
- **Multi-language Localization**: Added localization for update workflows, package notifications, periodic quota tracking, and rate limiter status across 10 locales (`ar`, `en`, `es`, `fa`, `fr`, `ja`, `ko`, `nl`, `ru`, `zh`).
- **C/C++ Integration Examples & ABI Tests**: Added standalone C and C++ integration examples and ABI verification test suites under `examples/ffi-c/`.

### Fixed

- **Dynamic Live Speed Limiter (`hya-net` / `hydra-gui` / `hydra-ffi`)**: Fixed speed limiter freezing at transfer start; rate limit adjustments now take effect immediately mid-flight without connection teardown or accumulated sleep debt.
- **Paired Aggregate & Per-Job Rate Limiting (`hya-net`)**: Fixed multi-cap pacing using `Pace::pair` to enforce both global engine bandwidth ceilings and individual download rate caps simultaneously.
- **FTP Download Speed Limiting (`hya-net` / `hydra-cli`)**: Fixed unshaped FTP transfers by integrating `Pace` rate limiting into `FtpFetcher`. *(e.g. `hydra --limit-rate 500k ftp://example.com/file.iso`)*
- **Linux Application ID & Dock Icon (`hydra-gui`)**: Fixed missing window icon and improper taskbar grouping on GNOME/KDE/Wayland desktops by setting `application_id` to `dev.ja7ad.hydra` and installing hicolor icons.
- **Release Asset Version Resolution (`hydra-updater`)**: Fixed asset lookup failures when matching pre-release tags against clean version asset spellings (e.g. resolving `v0.3.2-rc` to base version `0.3.2`).
- **Release Notes Comment Filtering (`hydra-updater`)**: Fixed raw HTML comments and auto-generated GitHub template noise leaking into in-app release notes dialogs.
- **Windows MSVC FFI Linker Compatibility**: Fixed static CRT linkage, path spacing issues, and linker symbol collisions on MSVC by utilizing response files and `/NODEFAULTLIB` filtering.
- **C99 / Pre-C11 FFI Compatibility (`hydra-ffi`)**: Fixed header compilation errors on older C compilers by adding static assertion fallbacks and dynamic linker probes in `hydra.h`.
- **NSIS Windows Installer Version Parsing**: Fixed automated Windows installer builds by extracting clean numeric version strings directly from `Cargo.toml`.

### Changed & Refactored

- **General Settings Layout (`hydra-gui`)**: Reorganized Options → General settings to group startup behaviors together (*Launch at system startup*, *Start minimized to system tray*, *Check for updates at launch*).
- **Concurrency Warm-Up Gating (`hya-core` / `hya-net`)**: Enhanced the adaptive ramp search to gate connection scaling until existing connections complete initial TCP slow-start and achieve stable throughput delivery.
- **Pre-release Package Version Formatting**: Standardized pre-release version strings across macOS `.app` Info.plist, macOS `.pkg`, Debian `.deb`, and RedHat `.rpm` packages.
- **Workspace Dependency Stamping**: Automated workspace version inheritance and dependency stamping during crates.io publishing workflows.

---

## [0.2.3] - 2026-08-19

- Baseline release featuring core adaptive concurrency retrieval, segmented HTTP/HTTPS/FTP downloads, CLI interactive TUI manager, desktop GUI, and browser extension integrations.
