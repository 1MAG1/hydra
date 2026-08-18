#!/bin/bash
# Copyright (C) 2026 Javad Rajabzadeh
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Cross-compile the Windows binaries from macOS/Linux and pack the NSIS
# installer (scripts/windows/hydra-installer.nsi).
#
#   scripts/build-windows-installer.sh [x64|arm64]      (default: x64)
#   scripts/build-windows-installer.sh --no-build ...   (pack only)
#
# `amd64` is accepted as an alias for x64 (same x86_64-pc-windows-msvc
# target); the installer is always named -x64- for a single canonical
# artifact name.
#
# Needs: cargo-xwin, Homebrew LLVM (lld-link), makensis (brew install makensis).
#
# --cross-compiler clang (not the clang-cl default) is required: ring's
# build.rs force-switches to plain clang on windows-aarch64, which rejects
# the /imsvc flags clang-cl emits. .cargo/config.toml already pins
# +crt-static so the exes need no VC++ Redistributable on the target.
set -euo pipefail
cd "$(dirname "$0")/.."

ARCH="x64"
NO_BUILD=0
for arg in "$@"; do
  case "$arg" in
    x64|amd64) ARCH="x64" ;;
    arm64) ARCH="arm64" ;;
    --no-build) NO_BUILD=1 ;;
    *) echo "usage: $0 [x64|amd64|arm64] [--no-build]" >&2; exit 2 ;;
  esac
done

case "$ARCH" in
  arm64) TARGET="aarch64-pc-windows-msvc" ;;
  x64)   TARGET="x86_64-pc-windows-msvc" ;;
esac

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)

if [ "$NO_BUILD" = 0 ]; then
  echo "building hydra-gui + hydra-host + hydra (cli) for $TARGET..."
  PATH="/opt/homebrew/opt/llvm/bin:$PATH" \
    cargo xwin build --release --target "$TARGET" --cross-compiler clang \
      -p hya-gui -p hya-host -p hya-cli
fi

for bin in hydra-gui.exe hydra-host.exe hydra.exe; do
  [ -f "target/$TARGET/release/$bin" ] || {
    echo "missing target/$TARGET/release/$bin (build failed or --no-build without a prior build?)" >&2
    exit 1
  }
done

command -v makensis >/dev/null || {
  echo "makensis not found - install with: brew install makensis" >&2
  exit 1
}

# makensis resolves File paths against the cwd, and the .nsi is written
# relative to its own directory.
cd scripts/windows
# Without a UTF-8 locale, Unicode makensis on macOS dies with std::bad_alloc
# while writing the output (NSIS bug #1165); non-login shells (CI, editors)
# often have no LC_ALL set.
LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8 \
  makensis -DARCH="$ARCH" -DVERSION="$VERSION" hydra-installer.nsi
echo "Built: target/hydra-$VERSION-windows-$ARCH-setup.exe"
