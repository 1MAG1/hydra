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

## Overview

**HYDRA** is a high-performance network file retriever designed for speed, resilience, and adaptability. It dynamically partitions downloads across multiple connections and independent mirror sources, continuously rebalancing work to maximize throughput without stalling on slow peers. It ships as both a `wget`/`curl`-compatible CLI and a cross-platform desktop download manager with browser integration.

<p align="center">
  <img src="docs/img/screenshot.png" alt="Hydra Download Manager" width="720">
</p>

## Key Features

### Engine

- **Multi-Source & Adaptive Concurrency**: Saturates high-bandwidth links by dynamically distributing byte ranges across connections and mirrors.
- **Dynamic Range Stealing**: Automatically detects laggard connections and redistributes remaining work to faster peers with zero server coordination overhead.
- **Collapse & Stall Detection**: Uses two-sided CUSUM and dual-window statistical estimators to swiftly identify degraded connections before traditional timeouts expire.
- **Protocols & Proxies**: Supports HTTP/1.1, HTTPS (via `rustls` with Mozilla roots), FTP (RFC 959 / REST), HTTP CONNECT tunneling, and SOCKS4 / SOCKS4a / SOCKS5 proxies.
- **Integrity & Offline Parity**: Per-chunk checksum manifest verification (`--emit-manifest`, `--chunk-digests`) and local Reed–Solomon erasure coding for bitrot protection.
- **Constant Memory Footprint**: Positioned writes (`pwrite`) stream data directly to disk, keeping resident memory usage flat regardless of file size.

### CLI

- **`curl` & `wget` Compatibility**: Direct drop-in support for common `wget` and `curl` command-line flags and dialect personalities.
- **Interactive Queue Manager**: Full-screen terminal UI (TUI) for managing, pausing, resuming, and monitoring queued downloads.
- **Format Sniffing & Smart Sorting**: Content-based magic byte inspection for accurate file type detection and optional category-based directory sorting (`--sort-by-type`).
- **Remote Checksum Lookup**: Inspects server-advertised digests (`Content-MD5`, `x-goog-hash`, …) and verifies downloads against a target hash.

### Desktop GUI

- **Cross-Platform Download Manager**: Native desktop app for Windows, macOS, and Linux with a persistent download list, categories, pause/resume, and per-download progress detail.
- **Browser Integration**: Extensions for Chrome/Chromium, Edge, Firefox, and Safari hand downloads captured in the browser to the app — over a local WebSocket bridge, with a native-messaging host as fallback that can also launch the app on demand.
- **Queue & Scheduler**: Queued downloads with scheduled start/stop times and retry tracking.
- **Desktop Conveniences**: System tray with light/dark-aware icons, event sounds, launch-on-startup, and localized UI.

---

## Installation

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

### From Source

Ensure you have Rust (1.80+) installed:

```bash
git clone https://github.com/ja7ad/hydra.git
cd hydra
cargo build --release
```

The compiled binary will be located at `target/release/hydra`. To build the GUI and native-messaging host as well, run `make build`.

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

## License

- The `hydra` CLI binary is licensed under the **GNU General Public License v3.0 or later** ([GPL-3.0-or-later](LICENSE)).
- The `hydra-core` and `hya-net` libraries are dual-licensed under **MIT** or **Apache-2.0** ([LICENSE-MIT](LICENSE-MIT) / [LICENSE-APACHE](LICENSE-APACHE)).

For more details, see [LICENSING.md](LICENSING.md) and [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
