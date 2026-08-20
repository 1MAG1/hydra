<p align="center">
  <img src="docs/logo.png" alt="HYDRA Logo" width="180">
</p>

<h1 align="center">HYDRA</h1>

<p align="center">
  <strong>A fast, resilient, multi-source file retriever and download engine.</strong>
</p>

<p align="center">
  <a href="https://crates.io/crates/hya-core"><img src="https://img.shields.io/crates/v/hya-core.svg?style=flat-square" alt="crates.io"></a>
  <a href="https://docs.rs/hya-core"><img src="https://img.shields.io/docsrs/hya-core?style=flat-square" alt="docs.rs"></a>
  <a href="https://codecov.io/gh/ja7ad/hydra"><img src="https://codecov.io/gh/ja7ad/hydra/graph/badge.svg?token=bQuMUwagma" alt="Coverage"/></a>
  <a href="https://github.com/ja7ad/hydra/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/ja7ad/hydra/ci.yml?branch=main&label=CI&style=flat-square" alt="CI Status"></a>
  <a href="LICENSING.md"><img src="https://img.shields.io/badge/license-GPL--3.0--or--later%20%7C%20MIT%2FApache--2.0-blue?style=flat-square" alt="License"></a>
  <img src="https://img.shields.io/badge/rust-2021%20edition-orange?style=flat-square" alt="Rust Edition">
</p>

---

## Contents

- [Overview](#overview)
- [Key Features](#key-features)
  - [Engine](#engine)
  - [CLI](#cli)
  - [Desktop GUI](#desktop-gui)
- [Installation](#installation)
  - [Homebrew (macOS / Linux)](#homebrew-macos--linux)
  - [Quick Install (prebuilt binaries)](#quick-install-prebuilt-binaries)
  - [From Source](#from-source)
- [Uninstall](#uninstall)
  - [Quick Uninstall (prebuilt installs)](#quick-uninstall-prebuilt-installs)
- [Usage](#usage)
  - [Basic Download](#basic-download)
  - [Multi-Connection & Mirror Sources](#multi-connection--mirror-sources)
  - [CLI Compatibility (`wget` / `curl` Mode)](#cli-compatibility-wget--curl-mode)
  - [Interactive Queue Manager (TUI)](#interactive-queue-manager-tui)
  - [Remote Checksum Lookup & Verification](#remote-checksum-lookup--verification)
- [Embedding HYDRA — `libhydra`](#embedding-hydra--libhydra)
  - [Platform guides](#platform-guides)
- [Contributing](#contributing)
- [License](#license)

---

## Overview

**HYDRA** is a high-performance network file retriever designed for speed, resilience, and adaptability. It dynamically partitions downloads across multiple connections and independent mirror sources, continuously rebalancing work to maximize throughput without stalling on slow peers. It ships as both a `wget`/`curl`-compatible CLI and a cross-platform desktop download manager with browser integration.

<p align="center">
  <img src="docs/img/screenshot.png" alt="Hydra Download Manager" width="720">
</p>

## Key Features

<table>
<tr><td valign="top" width="33%">

### Engine

- **Adaptive Concurrency** — splits files across connections and mirrors, rebalancing live
- **Range Stealing** — reassigns work from slow peers to fast ones automatically
- **Stall Detection** — statistical estimators catch degraded connections early
- **Broad Protocol Support** — HTTP(S), FTP, CONNECT tunneling, SOCKS4/4a/5
- **Integrity Checks** — checksum manifests plus Reed–Solomon bitrot protection
- **Flat Memory Use** — direct positioned writes keep RAM usage constant

</td><td valign="top" width="33%">

### CLI

- **`wget` / `curl` Compatible** — drop-in flag and dialect support
- **Interactive TUI** — manage, pause, resume, and monitor queued downloads
- **Smart File Sorting** — content-based type detection and auto-sort
- **Remote Checksum Lookup** — verify server-advertised digests before or after download

</td><td valign="top" width="33%">

### Desktop GUI

- **Cross-Platform App** — Windows, macOS, and Linux with categories and progress detail
- **Browser Integration** — Chrome, Edge, Firefox, and Safari extensions hand off downloads
- **Queue & Scheduler** — scheduled start/stop times with retry tracking
- **Desktop Niceties** — tray icon, sounds, launch-on-startup, localized UI

</td></tr>
</table>

---

## Installation

### Homebrew (macOS / Linux)

**CLI**:

```bash
brew install ja7ad/tap/hydra
```

**macOS Desktop App (GUI)**:

```bash
brew install --cask ja7ad/tap/hydra
```

### Quick Install (prebuilt binaries)

**macOS / Linux** — installs the GUI bundle (GUI + CLI + browser extensions) by default:

```bash
curl -fsSL https://raw.githubusercontent.com/ja7ad/hydra/main/install.sh | bash
```

CLI only:

```bash
curl -fsSL https://raw.githubusercontent.com/ja7ad/hydra/main/install.sh | bash -s -- --cli
```

**Windows (PowerShell)** — installs the GUI bundle by default:

```powershell
irm https://raw.githubusercontent.com/ja7ad/hydra/main/install.ps1 | iex
```

CLI only:

```powershell
& ([scriptblock]::Create((irm https://raw.githubusercontent.com/ja7ad/hydra/main/install.ps1))) -Cli
```

The scripts detect your OS and architecture (amd64/arm64), fetch the matching archive from the [latest GitHub release](https://github.com/ja7ad/hydra/releases/latest), and install it — on Linux and macOS to `/usr/local` (falling back to `~/.local`; override with `--prefix DIR`), on Windows to `%LOCALAPPDATA%\Programs\Hydra`. GUI installs also register the browser native-messaging host. Pin a release with `--version vX.Y.Z` / `-Version vX.Y.Z`, or download the archives yourself from the [releases page](https://github.com/ja7ad/hydra/releases).

**Beta channel** — `--beta` (`-Beta` on Windows) installs the newest `-rc` pre-release when it is ahead of the latest stable release; otherwise it installs the stable release:

```bash
curl -fsSL https://raw.githubusercontent.com/ja7ad/hydra/main/install.sh | bash -s -- --beta
```

```powershell
& ([scriptblock]::Create((irm https://raw.githubusercontent.com/ja7ad/hydra/main/install.ps1))) -Beta
```

The GUI's in-app updater follows the same rule: enable **Options → General → Download Beta channel** and update checks will also offer release candidates while one is ahead of stable.

**macOS notes**: since the app isn't notarized yet, Gatekeeper may block it — see the [macOS Permissions Guide](https://github.com/ja7ad/hydra/wiki/macOS-Permissions-Guide-for-Hydra) for granting the required permissions. If you installed via the `.dmg` and macOS refuses to open the app ("damaged" or "unidentified developer"), clear the quarantine attribute:

```bash
xattr -cr /Applications/Hydra\ Download\ Manager.app
```

### From Source

Ensure you have Rust (1.80+) installed:

```bash
git clone https://github.com/ja7ad/hydra.git
cd hydra
cargo build --release
```

The compiled binary will be located at `target/release/hydra`. To build the GUI and native-messaging host as well, run `make build`.

---

## Uninstall

### Quick Uninstall (prebuilt installs)

**macOS / Linux**:

```bash
curl -fsSL https://raw.githubusercontent.com/ja7ad/hydra/main/uninstall.sh | bash
```

To also delete config, state, and logs:

```bash
curl -fsSL https://raw.githubusercontent.com/ja7ad/hydra/main/uninstall.sh | bash -s -- --purge
```

**Windows (PowerShell)**:

```powershell
irm https://raw.githubusercontent.com/ja7ad/hydra/main/uninstall.ps1 | iex
```

To also delete config and state:

```powershell
& ([scriptblock]::Create((irm https://raw.githubusercontent.com/ja7ad/hydra/main/uninstall.ps1))) -Purge
```

**Homebrew**:

```bash
# Uninstall CLI
brew uninstall hydra

# Uninstall macOS Desktop App (and zap settings)
brew uninstall --cask --zap hydra
```

The scripts remove binaries, extensions, manifests, desktop shortcuts, and startup entries. On macOS, they also remove the bundled app and package receipts. Config and state are kept by default (`~/.config/hydra` on Linux/macOS, `%APPDATA%\hydra` on Windows); use `--purge` / `-Purge` to delete them.

---

## Usage

### Basic Download

```bash
# Retrieve a file with automatic concurrency discovery
hydra https://example.com/archive.tar.gz

# Specify output destination
hydra https://example.com/archive.tar.gz -o output.tar.gz
```

### Multi-Connection & Mirror Sources

```bash
# Explicit connection count (e.g., 8 connections)
hydra -x 8 https://example.com/largefile.iso

# Fetch across multiple mirror origins serving identical files
hydra https://mirror1.example.org/file.iso https://mirror2.example.org/file.iso
```

### CLI Compatibility (`wget` / `curl` Mode)

HYDRA can seamlessly emulate `wget` or `curl` flags:

```bash
# wget dialect
hydra --compat=wget -c -O myfile.zip https://example.com/file.zip

# curl dialect
hydra --compat=curl -C - -o myfile.zip https://example.com/file.zip
```

### Interactive Queue Manager (TUI)

```bash
# Launch interactive terminal UI
hydra interactive

# Add multiple downloads into the queue
hydra interactive https://example.com/file1.iso https://example.com/file2.zip
```

### Remote Checksum Lookup & Verification

```bash
# Check remote advertised checksums without downloading the object
hydra checksum https://example.com/release.tar.gz

# Download with target hash verification
hydra --checksum sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855 https://example.com/file.tar.gz
```

---

## Embedding HYDRA — `libhydra`

The engine is not only a CLI. `hya-ffi` exposes `hya-core` and `hya-net`
through a stable **C ABI**, so a desktop application, an Android app, an iOS
app, or a program in Go, Swift, Kotlin, Dart, C# or Python can run the same
download engine without taking the CLI or the GUI with it.

```bash
make ffi        # libhydra.a, libhydra.so/.dylib, and include/hydra.h
make ffi-test   # the ABI suite plus a C conformance program
```

```c
#include "hydra.h"

hydra_engine_config_t cfg;
HYDRA_ENGINE_CONFIG_INIT(&cfg);
cfg.state_path = "hydra-state.json";     /* jobs survive a process restart */

hydra_engine_t *engine = hydra_engine_create(&cfg);

const char *urls[] = { "https://example.com/big.iso" };
hydra_job_config_t job;
HYDRA_JOB_CONFIG_INIT(&job);
job.urls = urls; job.url_count = 1; job.output_path = "big.iso";

hydra_job_id_t id;
hydra_job_create(engine, &job, &id);
hydra_job_start(engine, id);
```

Job identity is a durable `uint64_t` rather than a pointer, so it survives an
app restart, a UI rebuild or a killed Android service; the event queue is the
asynchronous interface, so it becomes a Go channel, a Kotlin `Flow`, a Swift
`AsyncStream` or a Dart `Stream`; and file bytes never cross the boundary, so
resident memory stays independent of object size.

Every release publishes a prebuilt archive — static library, shared library,
header, pkg-config metadata and these guides — for Linux (glibc and musl),
macOS, Windows, Android and iOS. Any other target builds from source with
`scripts/build-ffi.sh --target <triple>`, the same script CI runs.

### Platform guides

| | |
|---|---|
| [Getting started](docs/ffi/README.md) | The contract, the archive layout, sixty seconds of C |
| [Linux](docs/ffi/linux.md) | glibc vs musl, pkg-config, CMake, containers, systemd |
| [macOS](docs/ffi/macos.md) | universal binaries, Xcode, App Sandbox, notarisation |
| [Windows](docs/ffi/windows.md) | MSVC, the static CRT, `hydra.lib` vs `hydra.dll` |
| [Android](docs/ffi/android.md) | jniLibs, JNI, CMake, `Flow`, background execution |
| [iOS](docs/ffi/ios.md) | `Hydra.xcframework`, SwiftPM, `AsyncStream`, app lifecycle |
| [Any other platform](docs/ffi/other-platforms.md) | building for a triple outside the release matrix |
| [Language bindings](docs/ffi/bindings.md) | Go, Python, C#, Dart, C++, Zig, and writing your own |

See also [`crates/hydra-ffi/README.md`](crates/hydra-ffi/README.md) for the full
contract, [`include/hydra.h`](include/hydra.h) for the published ABI, and
[`examples/ffi-c/download.c`](examples/ffi-c/download.c) for a complete C
client with mirrors, pause and resume.

---

## Contributing

Contributions are welcome! Please read the [Contributing Guide](CONTRIBUTING.md) for the project layout, build instructions, pre-submit checks (`fmt`, `clippy`, tests), commit conventions, and how licensing applies to each crate. In short:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

Bug reports and feature requests go to the [issue tracker](https://github.com/ja7ad/hydra/issues); security vulnerabilities should be reported privately via [GitHub security advisories](https://github.com/ja7ad/hydra/security/advisories/new).

---

## License

- The `hydra` CLI binary is licensed under the **GNU General Public License v3.0 or later** ([GPL-3.0-or-later](LICENSE)).
- The `hydra-core`, `hya-net` and `hya-ffi` libraries are dual-licensed under **MIT** or **Apache-2.0** ([LICENSE-MIT](LICENSE-MIT) / [LICENSE-APACHE](LICENSE-APACHE)).

For more details, see [LICENSING.md](LICENSING.md) and [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
