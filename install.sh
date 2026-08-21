#!/usr/bin/env bash
# Copyright (C) 2026 Javad Rajabzadeh
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Install the latest Hydra release on Linux or macOS.
#
#   curl -fsSL https://raw.githubusercontent.com/ja7ad/hydra/main/install.sh | bash
#   ... | bash -s -- --cli            # CLI only (default installs the GUI bundle)
#   ... | bash -s -- --version vx.x.x # pin a release instead of latest
#   ... | bash -s -- --beta           # newest -rc pre-release when ahead of latest
#   ... | bash -s -- --prefix ~/.local
#
# Default install is the GUI bundle: hydra, hydra-gui, hydra-host into
# <prefix>/bin, browser extensions + native-host installer into
# <prefix>/share/hydra. On Linux this uses the release tarball (no deb/rpm
# needed). --cli installs only the hydra binary.
#
# Homebrew users on macOS/Linux can also run:
#   brew install ja7ad/tap/hydra         # CLI
#   brew install --cask ja7ad/tap/hydra  # macOS Desktop GUI bundle

set -euo pipefail

REPO="ja7ad/hydra"
MODE="gui"
VERSION=""
PREFIX=""
BETA=0

usage() {
  cat >&2 <<EOF
Usage: install.sh [--cli] [--version vX.Y.Z] [--beta] [--prefix DIR]

  --cli            install only the hydra CLI binary
  --version TAG    install a specific release tag (default: latest)
  --beta           install the newest -rc pre-release when it is ahead of the
                   latest stable release (otherwise the stable release)
  --prefix DIR     install root (default: /usr/local, falling back to ~/.local)
EOF
  exit 2
}

while [ $# -gt 0 ]; do
  case "$1" in
    --cli) MODE="cli" ;;
    --gui) MODE="gui" ;;
    --version) VERSION="$2"; shift ;;
    --beta) BETA=1 ;;
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

ver_gt() { # ver_gt A B — true when A's numeric core is ahead of B's
  awk -v a="${1#v}" -v b="${2#v}" 'BEGIN{
    sub(/[-+].*/, "", a); sub(/[-+].*/, "", b)
    n = split(a, x, "."); m = split(b, y, ".")
    for (i = 1; i <= 3; i++) {
      ai = (i <= n ? x[i] : 0) + 0; bi = (i <= m ? y[i] : 0) + 0
      if (ai > bi) exit 0
      if (ai < bi) exit 1
    }
    exit 1
  }'
}

if [ -z "$VERSION" ]; then
  # Capture the JSON before parsing: grep -m1 on a live curl pipe closes it
  # early, which makes curl fail with exit 56 under pipefail.
  RELEASE_JSON=$(fetch "https://api.github.com/repos/${REPO}/releases/latest") || RELEASE_JSON=""
  VERSION=$(printf '%s\n' "$RELEASE_JSON" | grep -m1 '"tag_name"' | cut -d'"' -f4 || true)
  [ -n "$VERSION" ] || { echo "error: could not resolve the latest release tag" >&2; exit 1; }
  if [ "$BETA" = 1 ]; then
    # The newest -rc pre-release wins only while its version is ahead of the
    # stable release; once stable catches up, --beta installs stable.
    LIST_JSON=$(fetch "https://api.github.com/repos/${REPO}/releases?per_page=30") || LIST_JSON=""
    RC_TAG=$(printf '%s\n' "$LIST_JSON" | grep -o '"tag_name": *"[^"]*"' | cut -d'"' -f4 | grep -m1 -- '-rc' || true)
    if [ -n "$RC_TAG" ] && ver_gt "$RC_TAG" "$VERSION"; then
      VERSION="$RC_TAG"
    fi
  fi
fi
VER="${VERSION#v}"
# Assets are named from the workspace manifest version, which may stay on the
# plain release version while the tag carries a pre-release suffix: the
# v0.3.2-rc release ships hydra-0.3.2-* files. Try the tag spelling first, then
# the tag without its suffix.
CORE="${VER%%-*}"
if [ "$MODE" = cli ]; then
  ASSET_PREFIX="hydra-cli"
else
  ASSET_PREFIX="hydra"
fi
if [ "$CORE" != "$VER" ]; then
  CANDIDATES="$VER $CORE"
else
  CANDIDATES="$VER"
fi
DL_BASE="https://github.com/${REPO}/releases/download/${VERSION}"

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

NAME=""
for CAND in $CANDIDATES; do
  TRY="${ASSET_PREFIX}-${CAND}-${OS}-${ARCH}"
  echo "downloading ${DL_BASE}/${TRY}.tar.gz"
  # Missing the first spelling is expected on a pre-release tag, so only the
  # last candidate's failure is worth reporting.
  if [ "$CAND" = "$CORE" ]; then
    if fetch "${DL_BASE}/${TRY}.tar.gz" "$TMP/${TRY}.tar.gz"; then NAME="$TRY"; break; fi
  elif fetch "${DL_BASE}/${TRY}.tar.gz" "$TMP/${TRY}.tar.gz" 2>/dev/null; then
    NAME="$TRY"; break
  fi
  rm -f "$TMP/${TRY}.tar.gz"
done
[ -n "$NAME" ] || { echo "error: no ${MODE} archive for ${OS}/${ARCH} in release ${VERSION}" >&2; exit 1; }
tar -xzf "$TMP/${NAME}.tar.gz" -C "$TMP"
SRC="$TMP/$NAME"

$SUDO mkdir -p "$BIN_DIR"
$SUDO install -m 755 "$SRC/hydra" "$BIN_DIR/hydra"
echo "installed $BIN_DIR/hydra"

# Man pages (releases >= 0.2.2 ship them in the tarball as man/*.1).
if [ -d "$SRC/man" ]; then
  MAN_DIR="$PREFIX/share/man/man1"
  $SUDO mkdir -p "$MAN_DIR"
  for m in "$SRC"/man/*.1; do
    $SUDO install -m 644 "$m" "$MAN_DIR/$(basename "$m")"
  done
  echo "installed man pages into $MAN_DIR (try: man hydra)"
fi

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
    # Logo into the per-user hicolor theme, so Icon=hydra below resolves.
    # Without it the launcher, the dock and the switcher all draw the
    # generic fallback icon.
    if [ -f "$SRC/logo.png" ]; then
      ICON_DIR="$HOME/.local/share/icons/hicolor/256x256/apps"
      mkdir -p "$ICON_DIR"
      install -m 644 "$SRC/logo.png" "$ICON_DIR/hydra.png"
      echo "installed $ICON_DIR/hydra.png"
    fi

    # Desktop launcher (user-level; the tarball ships no packaging metadata).
    # The basename must stay "hydra": hydra-gui sets that as its window app id
    # (Wayland app_id / X11 WM_CLASS), which is how the shell matches a running
    # window back to this entry to pick up Icon=. StartupWMClass repeats it for
    # desktops that only consult that key.
    APPS_DIR="$HOME/.local/share/applications"
    mkdir -p "$APPS_DIR"
    cat > "$APPS_DIR/hydra.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Hydra Download Manager
GenericName=Download Manager
Comment=Multi-connection download accelerator
Exec=$BIN_DIR/hydra-gui
Icon=hydra
Terminal=false
Categories=Network;FileTransfer;
StartupWMClass=hydra
EOF
    echo "installed $APPS_DIR/hydra.desktop"

    command -v update-desktop-database >/dev/null 2>&1 &&
      update-desktop-database -q "$APPS_DIR" || true
    command -v gtk-update-icon-cache >/dev/null 2>&1 &&
      gtk-update-icon-cache -qt "$HOME/.local/share/icons/hicolor" || true
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
