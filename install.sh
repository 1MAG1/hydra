#!/usr/bin/env bash
# Copyright (C) 2026 Javad Rajabzadeh
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Install the latest Hydra release on Linux or macOS.
#
#   curl -fsSL https://raw.githubusercontent.com/ja7ad/hydra/main/install.sh | bash
#   ... | bash -s -- --cli            # CLI only (default installs the GUI bundle)
#   ... | bash -s -- --version v0.2.0 # pin a release instead of latest
#   ... | bash -s -- --prefix ~/.local
#
# Default install is the GUI bundle: hydra, hydra-gui, hydra-host into
# <prefix>/bin, browser extensions + native-host installer into
# <prefix>/share/hydra. On Linux this uses the release tarball (no deb/rpm
# needed). --cli installs only the hydra binary.

set -euo pipefail

REPO="ja7ad/hydra"
MODE="gui"
VERSION=""
PREFIX=""

usage() {
  cat >&2 <<EOF
Usage: install.sh [--cli] [--version vX.Y.Z] [--prefix DIR]

  --cli            install only the hydra CLI binary
  --version TAG    install a specific release tag (default: latest)
  --prefix DIR     install root (default: /usr/local, falling back to ~/.local)
EOF
  exit 2
}

while [ $# -gt 0 ]; do
  case "$1" in
    --cli) MODE="cli" ;;
    --gui) MODE="gui" ;;
    --version) VERSION="$2"; shift ;;
    --prefix) PREFIX="$2"; shift ;;
    -h|--help) usage ;;
    *) echo "unknown option: $1" >&2; usage ;;
  esac
  shift
done

case "$(uname -s)" in
  Linux)  OS="linux" ;;
  Darwin) OS="macos" ;;
  *) echo "error: unsupported OS $(uname -s) (use install.ps1 on Windows)" >&2; exit 1 ;;
esac

case "$(uname -m)" in
  x86_64|amd64)  ARCH="amd64" ;;
  aarch64|arm64) ARCH="arm64" ;;
  *) echo "error: unsupported architecture $(uname -m)" >&2; exit 1 ;;
esac

fetch() { # fetch URL [OUTFILE] — curl with a wget fallback
  if command -v curl >/dev/null 2>&1; then
    if [ $# -eq 2 ]; then curl -fSL --proto '=https' -o "$2" "$1"; else curl -fsSL --proto '=https' "$1"; fi
  elif command -v wget >/dev/null 2>&1; then
    if [ $# -eq 2 ]; then wget -qO "$2" "$1"; else wget -qO- "$1"; fi
  else
    echo "error: need curl or wget" >&2; exit 1
  fi
}

if [ -z "$VERSION" ]; then
  VERSION=$(fetch "https://api.github.com/repos/${REPO}/releases/latest" \
    | grep -m1 '"tag_name"' | cut -d'"' -f4)
  [ -n "$VERSION" ] || { echo "error: could not resolve the latest release tag" >&2; exit 1; }
fi
VER="${VERSION#v}"

if [ "$MODE" = cli ]; then
  NAME="hydra-cli-${VER}-${OS}-${ARCH}"
else
  NAME="hydra-${VER}-${OS}-${ARCH}"
fi
URL="https://github.com/${REPO}/releases/download/${VERSION}/${NAME}.tar.gz"

# Prefix: /usr/local when writable (or sudo is available), else ~/.local.
SUDO=""
if [ -z "$PREFIX" ]; then
  if [ -w /usr/local/bin ] 2>/dev/null || [ -w /usr/local ]; then
    PREFIX="/usr/local"
  elif command -v sudo >/dev/null 2>&1; then
    PREFIX="/usr/local"; SUDO="sudo"
  else
    PREFIX="$HOME/.local"
  fi
elif [ ! -w "$PREFIX" ] && [ -e "$PREFIX" ] && command -v sudo >/dev/null 2>&1; then
  SUDO="sudo"
fi
BIN_DIR="$PREFIX/bin"
SHARE_DIR="$PREFIX/share/hydra"

echo "hydra ${VERSION} (${MODE}) -> ${PREFIX}  [${OS}/${ARCH}]"

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

echo "downloading ${URL}"
fetch "$URL" "$TMP/${NAME}.tar.gz"
tar -xzf "$TMP/${NAME}.tar.gz" -C "$TMP"
SRC="$TMP/$NAME"

$SUDO mkdir -p "$BIN_DIR"
$SUDO install -m 755 "$SRC/hydra" "$BIN_DIR/hydra"
echo "installed $BIN_DIR/hydra"

if [ "$MODE" = gui ]; then
  $SUDO install -m 755 "$SRC/hydra-gui" "$BIN_DIR/hydra-gui"
  $SUDO install -m 755 "$SRC/hydra-host" "$BIN_DIR/hydra-host"
  echo "installed $BIN_DIR/hydra-gui"
  echo "installed $BIN_DIR/hydra-host"

  # Extensions + native-host installer keep the bundle layout (the script
  # resolves the bundle root as the parent of its own directory).
  $SUDO rm -rf "$SHARE_DIR"
  $SUDO mkdir -p "$SHARE_DIR"
  $SUDO cp -R "$SRC/extensions" "$SRC/scripts" "$SHARE_DIR/"
  echo "installed $SHARE_DIR (browser extensions + native-host installer)"

  if [ "$OS" = linux ]; then
    # Desktop launcher (user-level; the tarball ships no packaging metadata).
    APPS_DIR="$HOME/.local/share/applications"
    mkdir -p "$APPS_DIR"
    cat > "$APPS_DIR/hydra.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Hydra Download Manager
Exec=$BIN_DIR/hydra-gui
Terminal=false
Categories=Network;FileTransfer;
StartupWMClass=hydra-gui
EOF
    echo "installed $APPS_DIR/hydra.desktop"
  fi

  # Register the native-messaging host for the current user. Manifests land
  # in \$HOME, so this deliberately runs without sudo.
  if command -v python3 >/dev/null 2>&1; then
    bash "$SHARE_DIR/scripts/install-native-host.sh" --no-build --host-bin "$BIN_DIR/hydra-host" \
      || echo "warning: native-messaging host registration failed; rerun: $SHARE_DIR/scripts/install-native-host.sh --no-build --host-bin $BIN_DIR/hydra-host" >&2
  else
    echo "note: python3 not found; to enable browser integration run:" >&2
    echo "  $SHARE_DIR/scripts/install-native-host.sh --no-build --host-bin $BIN_DIR/hydra-host" >&2
  fi
fi

case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) echo "note: $BIN_DIR is not on your PATH" >&2 ;;
esac

echo "done."
