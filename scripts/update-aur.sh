#!/usr/bin/env bash
# Copyright (C) 2026 Javad Rajabzadeh
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Update PKGBUILD, generate .SRCINFO via makepkg, and publish to AUR.
#
#   scripts/update-aur.sh [vX.Y.Z] [DIST_DIR]
#
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

VERSION="${1:-}"
if [ -z "$VERSION" ]; then
  VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
fi
VERSION="${VERSION#v}"
DIST="${2:-dist}"

echo "Updating AUR packages for version ${VERSION}..."

WORKDIR=$(mktemp -d)
trap 'rm -rf "$WORKDIR"' EXIT

generate_srcinfo() {
  local dir="$1"
  if command -v makepkg >/dev/null 2>&1; then
    (cd "$dir" && makepkg --printsrcinfo > .SRCINFO)
  elif command -v docker >/dev/null 2>&1; then
    docker run --rm -i archlinux:base bash -c "
      useradd -m builder
      cat > /home/builder/PKGBUILD
      chown builder:builder /home/builder/PKGBUILD
      su builder -c 'cd /home/builder && makepkg --printsrcinfo'
    " < "$dir/PKGBUILD" > "$dir/.SRCINFO"
  else
    echo "error: neither makepkg nor docker found to generate .SRCINFO" >&2
    exit 1
  fi
}

update_pkg() {
  local pkg="$1"
  local template_dir="$2"
  local aur_repo="ssh://aur@aur.archlinux.org/${pkg}.git"
  local clone_dir="$WORKDIR/$pkg"

  echo "==> Updating $pkg..."
  git clone "$aur_repo" "$clone_dir"

  # Copy template PKGBUILD
  cp "$template_dir/PKGBUILD" "$clone_dir/PKGBUILD"

  # Update version in PKGBUILD dynamically from release version
  perl -pi -e "s/^pkgver=.*/pkgver=${VERSION}/" "$clone_dir/PKGBUILD"
  perl -pi -e "s/^pkgrel=.*/pkgrel=1/" "$clone_dir/PKGBUILD"

  # For the binary package, inject SHA256 sums from dist/SHA256SUMS.txt if available
  if [ "$pkg" = "hydra-download-manager-bin" ]; then
    local amd64_sha=""
    local arm64_sha=""

    if [ -f "$DIST/SHA256SUMS.txt" ]; then
      amd64_sha=$(grep "hydra-${VERSION}-linux-amd64.tar.gz" "$DIST/SHA256SUMS.txt" | awk '{print $1}' || true)
      arm64_sha=$(grep "hydra-${VERSION}-linux-arm64.tar.gz" "$DIST/SHA256SUMS.txt" | awk '{print $1}' || true)
    fi

    if [ -n "$amd64_sha" ]; then
      echo "    Setting x86_64 SHA256: $amd64_sha"
      perl -pi -e "s/^sha256sums_x86_64=.*/sha256sums_x86_64=('${amd64_sha}')/" "$clone_dir/PKGBUILD"
    fi
    if [ -n "$arm64_sha" ]; then
      echo "    Setting aarch64 SHA256: $arm64_sha"
      perl -pi -e "s/^sha256sums_aarch64=.*/sha256sums_aarch64=('${arm64_sha}')/" "$clone_dir/PKGBUILD"
    fi
  fi

  # Generate .SRCINFO with makepkg
  generate_srcinfo "$clone_dir"

  cd "$clone_dir"
  git config user.name "github-actions[bot]"
  git config user.email "github-actions[bot]@users.noreply.github.com"
  git add PKGBUILD .SRCINFO

  if ! git diff --staged --quiet; then
    git commit -m "chore(release): bump to v${VERSION}"
    git push origin master
    echo "==> Pushed update for $pkg"
  else
    echo "==> No changes for $pkg"
  fi
  cd "$REPO_ROOT"
}

update_pkg "hydra-download-manager" "$REPO_ROOT/packaging/aur"
update_pkg "hydra-download-manager-bin" "$REPO_ROOT/packaging/aur-bin"

echo "AUR updates completed."
