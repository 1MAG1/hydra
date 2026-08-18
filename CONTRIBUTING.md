# Contributing to HYDRA

Thanks for your interest in improving HYDRA! Contributions of all kinds are welcome — bug reports, feature requests, documentation fixes, and code.

## Table of Contents

- [Project Layout](#project-layout)
- [Prerequisites](#prerequisites)
- [Building](#building)
- [Before You Submit](#before-you-submit)
- [Testing](#testing)
- [Commit Messages](#commit-messages)
- [Pull Request Process](#pull-request-process)
- [Reporting Bugs](#reporting-bugs)
- [Licensing of Contributions](#licensing-of-contributions)

## Project Layout

HYDRA is a Cargo workspace with five crates:

| Crate directory | Published as | What it is |
|---|---|---|
| `crates/hydra-core` | [`hya-core`](https://crates.io/crates/hya-core) | I/O-free download scheduler: range partitioning, range stealing, collapse/stall detection |
| `crates/hydra-net` | `hya-net` | Transport layer: HTTP/1.1, HTTPS (`rustls`), FTP, HTTP CONNECT, SOCKS proxies |
| `crates/hydra-cli` | `hydra` binary | The CLI, `wget`/`curl` compatibility dialects, and the interactive TUI |
| `crates/hydra-gui` | — | Cross-platform desktop download manager (iced) |
| `crates/hydra-host` | — | Native-messaging host bridging browser extensions to the app |

Browser extensions live in `extensions/` (`chrome/`, `firefox/`, `safari/`), packaging and release tooling in `scripts/` and the `Makefile`, and design docs in `docs/`.

A good rule of thumb for where a change belongs: scheduling logic goes in `hydra-core` (it must stay free of I/O), protocol/transport work in `hydra-net`, and user-facing behavior in `hydra-cli` or `hydra-gui`.

## Prerequisites

- **Rust 1.80+** (the workspace pins the `stable` channel via `rust-toolchain.toml`, which also pulls in `rustfmt` and `clippy` automatically).
- **Linux only** — GUI/system dependencies used by CI and the desktop build:

  ```bash
  sudo apt-get install -y pkg-config libasound2-dev libx11-dev libxrandr-dev libxcb1-dev libxkbcommon-dev
  ```

- **macOS / Windows** — no extra system dependencies for a standard build.

## Building

```bash
# CLI only
cargo build --release          # binary at target/release/hydra

# Everything (CLI + GUI + native-messaging host)
make build
```

Other useful Makefile targets: `make cli`, `make gui`, `make host`, and platform packaging targets (`make dmg` on macOS, `make deb` / `make rpm` on Linux, `make windows`).

## Before You Submit

CI enforces all of the following, so running them locally saves a round trip:

```bash
# 1. Formatting (CI fails on any diff)
cargo fmt --all -- --check

# 2. Lints (warnings are errors in CI)
cargo clippy --all-targets --all-features -- -D warnings

# 3. Tests, debug and release (CI runs both)
cargo test --all-targets --all-features
cargo test --release --all-targets --all-features
```

## Testing

- Put unit tests next to the code they cover; integration tests go in each crate's `tests/` directory.
- New engine behavior in `hydra-core` should come with tests — the scheduler is deliberately I/O-free precisely so it can be tested deterministically without a network.
- Coverage is tracked on [Codecov](https://codecov.io/gh/ja7ad/hydra). A PR doesn't need to hit a specific number, but changes that add meaningful logic without any tests will usually get pushback.
- To reproduce the coverage report locally:

  ```bash
  cargo llvm-cov --workspace --all-features --ignore-filename-regex '(/examples/|/benches/)'
  ```

## Commit Messages

HYDRA follows [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>(<optional scope>): <short summary>
```

Types in use: `feat`, `fix`, `docs`, `chore`, `refactor`, `test`, `perf`, `ci`. Scope is the crate or area touched, e.g.:

```
feat(cli): add --sort-by-type category sorting
fix(net): handle FTP REST beyond 2 GiB
chore(extensions): bump browser extension manifests to 0.2.2
```

Keep the summary imperative and under ~72 characters. Branch names follow the same spirit: `feat/short-description`, `fix/short-description`.

## Pull Request Process

1. Fork the repository and create a branch off `main`.
2. Make your change, including tests and doc updates where relevant.
3. Run the [pre-submit checks](#before-you-submit) locally.
4. Open a PR against `main` describing **what** changed and **why**. Link the related issue if one exists.
5. CI (formatting, clippy, tests, coverage) must pass before review/merge.

For large features or anything that changes engine behavior (scheduling, stall detection, integrity), please open an issue first to discuss the design — it avoids wasted work on both sides.

## Reporting Bugs

Open an issue at [github.com/ja7ad/hydra/issues](https://github.com/ja7ad/hydra/issues) with:

- HYDRA version (`hydra --version`), OS, and how it was installed (installer script, package, source).
- The exact command line (redact URLs/credentials as needed) and what happened vs. what you expected.
- For engine issues: whether the server supports ranges, number of connections/mirrors, and any proxy in the path.

For security vulnerabilities, please **do not** open a public issue — report them privately via [GitHub security advisories](https://github.com/ja7ad/hydra/security/advisories/new).

## Licensing of Contributions

HYDRA uses a split licensing model (see [LICENSING.md](LICENSING.md)):

- Contributions to `crates/hydra-core` and `crates/hydra-net` are accepted under **MIT OR Apache-2.0** (dual license).
- Contributions to `crates/hydra-cli` (and the rest of the workspace) are accepted under **GPL-3.0-or-later**.

By submitting a pull request, you agree that your contribution is licensed under the license(s) of the crate(s) it modifies. Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion shall be licensed as above, without any additional terms or conditions (per Apache-2.0 §5 for the dual-licensed crates).
