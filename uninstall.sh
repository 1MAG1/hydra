#!/usr/bin/env bash
# Copyright (C) 2026 Javad Rajabzadeh
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Uninstall Hydra from Linux or macOS.
#
#   curl -fsSL https://raw.githubusercontent.com/ja7ad/hydra/main/uninstall.sh | bash
#   ... | bash -s -- --purge          # also delete config/state (~/.config/hydra)
#   ... | bash -s -- --prefix ~/.local
#
# Removes everything install.sh created: hydra, hydra-gui, hydra-host and
# hydra-updater from <prefix>/bin (binaries or symlinks into the app bundle),
# <prefix>/share/hydra, the desktop launcher and icons, the autostart login
# item, and the native-messaging manifests. Also removes the macOS
# "Hydra Download Manager.app" from /Applications and ~/Applications. Config
# and state in ~/.config/hydra are kept unless --purge is given.

set -euo pipefail

PREFIX=""
PURGE=0
REMOVED=0

usage() {
  cat >&2 <<EOF
Usage: uninstall.sh [--purge] [--prefix DIR]

  --purge          also delete config/state/logs (~/.config/hydra)
  --prefix DIR     uninstall from DIR only (default: /usr/local and ~/.local)
EOF
  exit 2
}

while [ $# -gt 0 ]; do
  case "$1" in
    --purge) PURGE=1 ;;
    --prefix) PREFIX="$2"; shift ;;
    -h|--help) usage ;;
    *) echo "unknown option: $1" >&2; usage ;;
  esac
  shift
done

case "$(uname -s)" in
  Linux)  OS="linux" ;;
  Darwin) OS="macos" ;;
  *) echo "error: unsupported OS $(uname -s) (use uninstall.ps1 on Windows)" >&2; exit 1 ;;
esac

# rm_path PATH — remove a file/dir, with sudo when it is not ours to delete.
rm_path() {
  local p="$1" SUDO=""
  [ -e "$p" ] || [ -L "$p" ] || return 0
  if [ ! -w "$(dirname "$p")" ] && command -v sudo >/dev/null 2>&1; then SUDO="sudo"; fi
  $SUDO rm -rf "$p"
  echo "removed $p"
  REMOVED=1
}

# Stop background instances so binaries and manifests are not in use.
for p in hydra-gui hydra-host; do pkill -x "$p" 2>/dev/null || true; done

# deb/rpm installs are owned by the package manager; don't reach into /usr.
if [ "$OS" = linux ]; then
  if command -v dpkg >/dev/null 2>&1 && dpkg -s hydra-download-manager >/dev/null 2>&1; then
    echo "note: hydra was installed as a package; also run: sudo apt remove hydra-download-manager" >&2
  elif command -v rpm >/dev/null 2>&1 && rpm -q hydra-download-manager >/dev/null 2>&1; then
    echo "note: hydra was installed as a package; also run: sudo dnf remove hydra-download-manager" >&2
  fi
fi

# Homebrew installs are managed by brew.
if command -v brew >/dev/null 2>&1; then
  if brew list --formula hydra >/dev/null 2>&1 || brew list --formula ja7ad/tap/hydra >/dev/null 2>&1; then
    echo "note: hydra CLI was installed via Homebrew; to remove run: brew uninstall hydra" >&2
  fi
  if brew list --cask hydra >/dev/null 2>&1 || brew list --cask ja7ad/tap/hydra >/dev/null 2>&1; then
    echo "note: hydra app was installed via Homebrew Cask; to remove run: brew uninstall --cask hydra" >&2
  fi
fi

# Man pages, by exact name: a glob like hydra*.1 could hit the unrelated
# THC hydra(1) if it shares the prefix.
MAN_PAGES=(hydra.1 hydra-interactive.1 hydra-checksum.1 hydra-parity.1
           hydra-formats.1 hydra-bench.1 hydra-completions.1)

# Binaries + share dir + man pages from the tarball install.
if [ -n "$PREFIX" ]; then PREFIXES=("$PREFIX"); else PREFIXES=(/usr/local "$HOME/.local"); fi
for prefix in "${PREFIXES[@]}"; do
  rm_path "$prefix/bin/hydra"
  rm_path "$prefix/bin/hydra-gui"
  rm_path "$prefix/bin/hydra-host"
  rm_path "$prefix/bin/hydra-updater"
  rm_path "$prefix/share/hydra"
  # System-wide desktop integration, when install.sh had write access to the
  # prefix (the per-user copies are handled further down).
  if [ "$OS" = linux ]; then
    rm_path "$prefix/share/applications/hydra.desktop"
    for size in 16 24 32 48 64 128 256 512; do
      rm_path "$prefix/share/icons/hicolor/${size}x${size}/apps/hydra.png"
    done
  fi
  for m in "${MAN_PAGES[@]}"; do
    rm_path "$prefix/share/man/man1/$m"
    rm_path "$prefix/share/man/man1/$m.gz"
  done
done

if [ "$OS" = macos ]; then
  rm_path "/Applications/Hydra Download Manager.app"
  # install.sh --app-dir, and its fallback when /Applications is not writable.
  rm_path "$HOME/Applications/Hydra Download Manager.app"
  # .pkg installs: bundled extensions + the installer receipt.
  rm_path "/Library/Application Support/Hydra"
  if pkgutil --pkg-info io.github.ja7ad.hydra >/dev/null 2>&1; then
    if command -v sudo >/dev/null 2>&1; then
      sudo pkgutil --forget io.github.ja7ad.hydra >/dev/null 2>&1 || true
      echo "removed pkg receipt io.github.ja7ad.hydra"
    fi
  fi
  # Login item (the "Launch on startup" toggle writes this LaunchAgent).
  launchctl bootout "gui/$(id -u)/io.github.ja7ad.hydra" 2>/dev/null || true
  rm_path "$HOME/Library/LaunchAgents/io.github.ja7ad.hydra.plist"
else
  rm_path "$HOME/.local/share/applications/hydra.desktop"
  rm_path "$HOME/.config/autostart/hydra.desktop"
  # install.sh renders the logo at every hicolor size it can.
  for size in 16 24 32 48 64 128 256 512; do
    rm_path "$HOME/.local/share/icons/hicolor/${size}x${size}/apps/hydra.png"
  done
fi

# Native-messaging manifests (per-user; mirrors install-native-host.sh).
if [ "$OS" = macos ]; then
  MANIFEST_DIRS=(
    "$HOME/Library/Application Support/Google/Chrome/NativeMessagingHosts"
    "$HOME/Library/Application Support/Chromium/NativeMessagingHosts"
    "$HOME/Library/Application Support/Microsoft Edge/NativeMessagingHosts"
    "$HOME/Library/Application Support/BraveSoftware/Brave-Browser/NativeMessagingHosts"
    "$HOME/Library/Application Support/Vivaldi/NativeMessagingHosts"
    "$HOME/Library/Application Support/Arc/User Data/NativeMessagingHosts"
    "$HOME/Library/Application Support/Mozilla/NativeMessagingHosts"
    "$HOME/Library/Application Support/LibreWolf/NativeMessagingHosts"
  )
else
  MANIFEST_DIRS=(
    "$HOME/.config/google-chrome/NativeMessagingHosts"
    "$HOME/.config/chromium/NativeMessagingHosts"
    "$HOME/.config/microsoft-edge/NativeMessagingHosts"
    "$HOME/.config/BraveSoftware/Brave-Browser/NativeMessagingHosts"
    "$HOME/.config/vivaldi/NativeMessagingHosts"
    "$HOME/.mozilla/native-messaging-hosts"
    "$HOME/.librewolf/native-messaging-hosts"
  )
fi
for d in "${MANIFEST_DIRS[@]}"; do
  rm_path "$d/com.hydra.host.json"
done

if [ "$PURGE" = 1 ]; then
  rm_path "$HOME/.config/hydra"
else
  [ -d "$HOME/.config/hydra" ] \
    && echo "kept $HOME/.config/hydra (config/state/logs); rerun with --purge to delete it"
fi

if [ "$REMOVED" = 1 ]; then
  echo "done."
else
  echo "nothing to remove; hydra does not appear to be installed."
fi
