# Changelog

All notable changes to the Hydra project are documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [0.3.12] - 2026-08-24

### Added

- **HTML Redirect & Meta Refresh Resolution (`hya-net`, `hydra-cli`, `hydra-gui`, `hydra-ffi`)**: Added automated HTML redirector page and `<meta http-equiv="refresh">` / JavaScript redirection detection (`hya_net::redirect`, `Probe::maybe_redirector`). CLI automatically follows redirectors to adopt target destination file names unless overridden by `-O`/`--output-dir`. GUI integrates asynchronous redirect probing in the File Info dialog and Batch download window to show resolved file names, and FFI drivers resolve HTML redirects within hop limits.
- **Resilient Probe Fallback & Observable Single-Stream Fetching (`hya-net`, `hydra-cli`, `hydra-gui`, `hydra-ffi`)**: Added `probe_resilient` to seamlessly fallback to a 1-byte ranged GET (`Range: bytes=0-0`) when origins refuse HTTP HEAD requests or drop connections without `Content-Length`. Added `fetch_streaming_observed` for single-stream fallback downloads supporting atomic write progress tracking, real-time download rate calculations, rate limiter pacing (`Pace`), and responsive cancellation/pause handling.
- **Adaptive Concurrency Throttling on 429/503 Limits (`hya-net`)**: Implemented dynamic concurrency backoff upon encountering HTTP 429 (Too Many Requests) or 503 (Service Unavailable) status codes. Retains active streaming connections while lowering concurrency ceilings down to the streaming floor during refusal bursts, periodically probing upward after sustained progress to safely restore concurrency.
- **Official Arch Linux AUR Packaging (`packaging/aur`, `packaging/aur-bin`, CI)**: Added official Arch Linux User Repository (AUR) package specifications for both source (`hydra-download-manager`) and pre-built binary (`hydra-download-manager-bin`) distributions, complete with automated `.SRCINFO` generation, release publishing workflows, and documentation.
- **German Localization (`hydra-gui`)**: Added full German (`de`) language translation catalog across all GUI menus, settings tabs, dialogs, and notifications.
- **Connection Limit Status Indicator (`hydra-gui`)**: Added dedicated "Waiting (connection limit)..." connection state status when connections are throttled or held back by concurrency limits or server rate limiting.

### Fixed

- **GUI Rendering & Resource Usage Optimization (`hydra-gui`)**: Virtualized the download table viewport to render only visible rows plus overscan, drastically reducing memory usage and CPU rendering time on large queues. Replaced per-row linear searches with `HashSet<DlId>` lookups for selection styling.
- **Mouse Movement Overhead & Selection Scroll Stability (`hydra-gui`)**: Replaced high-frequency mouse motion event redraws with zero-sized cursor probing and cadence-sampled drag ticks (`Message::DragTick`). Stabilized the root widget tree with an unconditional overlay stack to prevent scroll position resets when clicking or dragging rows.
- **macOS Window Color Space Pinning (`hydra-gui`)**: Pinned AppKit window color space to the softbuffer drawing surface on macOS (`macos_surface`), eliminating full-window ColorSync conversions on Retina/P3 displays and cutting main-thread redraw CPU usage by ~35%.
- **GUI Menu Bar Hover Tracking (`hydra-gui`)**: Fixed menu bar behavior on Windows and Linux so hovering across menu headers (`File`, `Edit`, `View`, etc.) automatically switches the active dropdown menu once a menu is open.
- **Empty File Downloads with `Content-Length: 0` (`hya-net`, `hydra-cli`)**: Fixed zero-byte download handling by accurately distinguishing explicit `Content-Length: 0` headers from missing length metadata.
- **Launchpad PPA Upload & Release Automation (`.github/workflows/release.yml`)**: Fixed PPA upload loop indentation, repaired FTP upload errors, and removed duplicate workflow steps.
- **Release Notes Deduplication (`.github/workflows/release.yml`, `.github/release.yml`)**: Integrated GitHub API release notes generation into release workflows to prevent duplicate release note bodies.

### Changed & Refactored

- **Korean & Multi-Language Localization**: Updated Korean (`ko`) translation strings and added new status strings across all 11 supported locales (`ar`, `de`, `en`, `es`, `fa`, `fr`, `ja`, `ko`, `nl`, `ru`, `zh`).
- **CI Security & Documentation Assets**: Added explicit top-level workflow permissions in CI workflows, optimized documentation and extension images via lossless compression, and updated web documentation SEO and sitemap metadata.

---

## [0.3.11] - 2026-08-23

### Added

- **Microsoft Edge Add-ons Store Publication (`hydra-gui`, `README.md`)**: Added direct store link to the official Microsoft Edge Add-ons listing (`microsoftedge.microsoft.com/addons/detail/hydra-download-manager-in/obemipfpeenmhkdpkobdkeedhdakaoai`) in *Options → Extensions*, with dedicated Microsoft Edge brand artwork and separated rows for other Chromium-based browsers (Brave, Vivaldi, Opera, Arc, Chromium).
- **GitHub Issue Templates (`.github/ISSUE_TEMPLATE`)**: Added standardized issue form templates for bug reports and feature requests with platform/component selectors, reproduction guidance, and security disclosure links.

### Fixed

- **UI Scaling by Font Size Setting (`hydra-gui`)**: Fixed font size setting by scaling the entire GUI window interface proportionately (`theme::ui_scale`), keeping layouts, buttons, and dialogs balanced rather than resizing text within fixed-size containers.
- **macOS Native Menu State Synchronization (`hydra-gui`)**: Added in-place menu synchronization (`macos_menu::sync`) to prevent radio and checkbox menu items (Dark Mode, Speed Limiter, Font Size, Language) from becoming desynchronized or dual-ticked on click.
- **Release Notes Deduplication (`hydra-updater`)**: Fixed duplicate release note sections appearing in in-app update dialogs when release workflows or notes generators run multiple times over the same tag.
- **Launchpad PPA Build Toolchain & Vendoring (`packaging/debian`, CI)**: Fixed Launchpad PPA source builds on Ubuntu 24.04 (noble) and 22.04 (jammy) by targeting versioned `rustc-1.91` / `cargo-1.91` packages, configuring `CARGO_HOME` for sbuild sandboxes, and clearing per-crate `.cargo-checksum.json` paths during packaging.
- **Release CI GPG Key Validation (`.github/workflows/release.yml`)**: Added fail-fast validation in release automation to verify secret signing keys are present in `LAUNCHPAD_GPG_KEY` before attempting PPA uploads.

### Changed & Refactored

- **Korean Localization**: Updated and completed Korean (`ko`) translation strings for dialogs, settings, updater prompts, and extension options.
- **GUI Asset Organization (`hydra-gui`)**: Reorganized built-in localization catalogues into `crates/hydra-gui/assets/locale/` alongside visual brand assets.
- **Release Automation Prerelease Filtering (`.github/workflows/release.yml`)**: Prevented automated publishing of pre-release builds to Homebrew tap, Fedora Copr, and Launchpad PPA repositories.

---

## [0.3.10] - 2026-08-23

### Added

- **Firefox Add-ons Store Publication (`hydra-gui`, `extensions/firefox`)**: Added direct store link to the official Firefox Add-ons listing (`addons.mozilla.org/en-US/firefox/addon/hdm-integration/`) in *Options → Extensions*, replacing manual installation guides with one-click store installation.
- **Multi-Language Localization**: Updated Firefox extension translation strings across all 10 supported locales (`ar`, `en`, `es`, `fa`, `fr`, `ja`, `ko`, `nl`, `ru`, `zh`).

---

## [0.3.9] - 2026-08-23

### Added

- **Linux StatusNotifierItem (SNI) Tray Integration (`hydra-gui`)**: Added native Linux D-Bus `StatusNotifierItem` (SNI) system tray support (`ksni` / `zbus`) with automatic desktop environment detection and runtime fallback between D-Bus SNI and X11/muda. Delivers seamless tray icon support across modern Wayland and X11 desktop environments (GNOME via AppIndicator, KDE Plasma, COSMIC, Sway, Hyprland). Added selectable tray backend options (*Auto*, *StatusNotifierItem (D-Bus)*, *Muda (X11/Legacy)*) under *Options → General*.
- **Linux Taskbar & Launcher Progress (`hydra-gui`)**: Added Unity and KDE launcher D-Bus progress bar and urgent transfer count badges via `com.canonical.Unity.LauncherEntry`, displaying aggregate download progress and active transfer counts directly on the Linux dock and taskbar launcher.
- **In-App Browser Extension Manager (`hydra-gui`)**: Added a dedicated **Browser Extensions** tab in the Options window (*Options → Extensions*) and an **Extensions** action button on the main toolbar. Provides interactive extension installation guides for Chrome, Firefox, Edge, and Brave with status badges, installation directories, packed files (`.zip`/`.xpi`), and one-click actions to open extension folders or browser extension management pages (`chrome://extensions`, `about:debugging`, `edge://extensions`, `brave://extensions`).
- **Official Debian / Ubuntu Launchpad PPA Packaging (`packaging/debian`, CI)**: Added official Debian packaging infrastructure (`control`, `rules`, `copyright`, `postinst`, `postrm`, native messaging host manifests, `.desktop` autostart) with automated Source Package (DSC) generation and Launchpad PPA release triggers (`ppa:ja7ad/hydra`).
- **Official Fedora Copr Packaging (`packaging/rpm`, CI)**: Added Fedora RPM package specifications (`packaging/rpm/hydra.spec`) and automated Copr repository build and release triggers (`copr enable ja7ad/hydra`).
- **Post-Install Extension Setup Guidance (`install.sh` / `install.ps1`)**: Added interactive post-installation setup instructions in both Unix shell and Windows PowerShell installation scripts, outlining browser extension loading steps for Chrome, Edge, Brave, and Firefox.
- **Interactive Web Changelog & Documentation Portal (`docs/`)**: Added a searchable and filterable web changelog portal (`docs/changelog.html`) with category tagging, release links, and improved landing page download modals and engine architecture cards.
- **Multi-Language Localization**: Added localized strings for the new Extensions settings tab, toolbar extension buttons, and Linux tray backend options across all 10 supported locales (`ar`, `en`, `es`, `fa`, `fr`, `ja`, `ko`, `nl`, `ru`, `zh`).

### Fixed

- **Debian Packaging Native Messaging Manifest Generation**: Replaced inline heredoc JSON manifest creation in `debian/rules` with standalone validated JSON manifest files for Chrome/Chromium and Mozilla Firefox.
- **Debian Changelog Formatting in Launchpad Release Automation**: Fixed Debian release versioning and distribution targeting (`noble`, `jammy`) in automated Launchpad source package generation.

### Changed & Refactored

- **Linux Package Dependencies**: Updated Debian and RPM package specifications to recommend `gnome-shell-extension-appindicator` instead of legacy shared library appindicator dependencies.
- **GitHub Release Categorization**: Added the `enhancement` label to the Features category in `.github/release.yml`.

---

## [0.3.7] - 2026-08-22

### Added

- **Browser Extension Bundling & Packaging (`scripts/build-extensions.sh`, packaging targets)**: Added a unified extension build system (`make extensions`) producing both packed (`.zip` for Chrome/Chromium, `.xpi` for Firefox) and unpacked directory trees with path-tailored `INSTALL.txt` guides. Bundled extensions across all packaging targets: Windows setup (`%LOCALAPPDATA%\Programs\Hydra\extensions`), macOS `.app` bundle (`Contents/Resources/extensions`), macOS `.pkg` (`/Library/Application Support/Hydra/extensions`), Linux `.deb`/`.rpm` (`/usr/share/hydra-download-manager/extensions`), and release archives (`<prefix>/share/hydra/extensions`).
- **CRX Signing & Enterprise Policy Packaging**: Added Chromium `.crx` generation and signing (`--crx-key`, `HYDRA_CRX_KEY`, `--crx`) with pinned extension ID (`jpnonmbbkjdpeebdhkjoliklfhkdcomj`) for enterprise policy deployments (`ExtensionSettings` / `ExtensionInstallForcelist`).
- **Rubber-Band Drag Selection (`hydra-gui`)**: Added interactive marquee drag selection to the download table, allowing users to click and drag across rows to select multiple downloads simultaneously, with accompanying theme and bounding-box styling.
- **ABI Specification (`docs/ffi/ABI.md`)**: Added the canonical human-readable specification for the `libhydra` C ABI — FFI design principles, the formal ABI 1 stability policy (what is frozen, what may be appended, what forces ABI 2), the full ownership/encoding/error/threading contract, the event-queue rationale and its ordering and drop guarantees, and the compatibility-testing matrix. `include/hydra.h` now carries a concise summary and points at it.
- **Frozen ABI Baseline (`crates/hydra-ffi/abi/abi-1.manifest`)**: Added a machine-checkable record of ABI 1 — every enumerator value, field offset and width, struct size and exported symbol.
- **ABI Stability Gate (`scripts/ffi-abi-compat.sh`, `make ffi-compat`)**: Added a CI gate that derives the current layout from the header with a generated C probe and enforces the ABI 1 rules against the frozen baseline: fields may not move or change width, enumerators may not be renumbered, symbols may not disappear, and only the two size-prefixed configuration structs may grow. Additions the contract permits pass.
- **Forward-Compatibility Probe (`examples/ffi-c/compat_probe.c`)**: Added an old-header/new-library conformance program. `scripts/ffi-c-example.sh` now builds it against `include/hydra.h` as published by *every* release tag and links each against the library from the current branch, with a guard wall after each caller-allocated struct so a byte written past an older header's extent is caught at test time.
- **Wider ABI CI Matrix**: The FFI conformance job now also runs under Clang on Linux (in addition to GCC), and the stability gate runs on Linux, macOS and Windows.

### Fixed

- **PowerShell Execution Policy in Windows Installer (`install.ps1`)**: Fixed native messaging host registration failure on Windows machines with Restricted PowerShell execution policies by invoking `install-native-host.ps1` in its own PowerShell process with `-ExecutionPolicy Bypass` and non-fatal warning recovery.
- **MSVC Guard Index Cast & MSYS2 Legacy Header Checkout (`scripts/ffi-c-example.sh` / `compat_probe.c`)**: Fixed unsigned pointer cast in compatibility probe for MSVC and resolved legacy release header checkouts when running under MSYS2 environments.

### Changed & Refactored

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
