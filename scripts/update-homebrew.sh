#!/usr/bin/env bash
# Copyright (C) 2026 Javad Rajabzadeh
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Update Formula/hydra.rb and Casks/hydra.rb for a release.
#
#   scripts/update-homebrew.sh [vX.Y.Z] [DIST_DIR]
#
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

VERSION="${1:-}"
if [ -z "$VERSION" ]; then
  VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
fi
VERSION="${VERSION#v}"
TAG="v${VERSION}"
DIST="${2:-dist}"
TAP_DIR="${3:-${HOMEBREW_TAP_DIR:-../homebrew-tap}}"

echo "Updating Homebrew tap at ${TAP_DIR} for version ${VERSION} (tag ${TAG})..."

mkdir -p "${TAP_DIR}/Formula" "${TAP_DIR}/Casks"

# 1. Formula sha256 (source tarball)
SOURCE_URL="https://github.com/ja7ad/hydra/archive/refs/tags/${TAG}.tar.gz"
echo "Fetching source tarball SHA256..."
FORMULA_SHA=$(curl -fsSL "$SOURCE_URL" | sha256sum | awk '{print $1}') || FORMULA_SHA=""
if [ -z "$FORMULA_SHA" ]; then
  echo "warning: could not fetch remote tarball; computing from git archive" >&2
  TMP_TAR=$(mktemp)
  git archive --format=tar.gz --prefix="hydra-${VERSION}/" "$TAG" -o "$TMP_TAR"
  FORMULA_SHA=$(sha256sum "$TMP_TAR" | awk '{print $1}')
  rm -f "$TMP_TAR"
fi

# 2. Cask sha256s (DMGs)
DMG_ARM_SHA=""
DMG_INTEL_SHA=""

if [ -f "$DIST/SHA256SUMS.txt" ]; then
  DMG_ARM_SHA=$(grep "Hydra-${VERSION}-arm64.dmg" "$DIST/SHA256SUMS.txt" | awk '{print $1}' || true)
  DMG_INTEL_SHA=$(grep "Hydra-${VERSION}-x86_64.dmg" "$DIST/SHA256SUMS.txt" | awk '{print $1}' || true)
fi

if [ -z "$DMG_ARM_SHA" ] && [ -f "$DIST/Hydra-${VERSION}-arm64.dmg" ]; then
  DMG_ARM_SHA=$(sha256sum "$DIST/Hydra-${VERSION}-arm64.dmg" | awk '{print $1}')
fi

if [ -z "$DMG_INTEL_SHA" ] && [ -f "$DIST/Hydra-${VERSION}-x86_64.dmg" ]; then
  DMG_INTEL_SHA=$(sha256sum "$DIST/Hydra-${VERSION}-x86_64.dmg" | awk '{print $1}')
fi

# Update ${TAP_DIR}/Formula/hydra.rb
cat > "${TAP_DIR}/Formula/hydra.rb" <<EOF
class Hydra < Formula
  desc "Fast, resilient, multi-source file retriever and download engine"
  homepage "https://github.com/ja7ad/hydra"
  url "https://github.com/ja7ad/hydra/archive/refs/tags/${TAG}.tar.gz"
  sha256 "${FORMULA_SHA}"
  license "GPL-3.0-or-later"
  head "https://github.com/ja7ad/hydra.git", branch: "main"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args(path: "crates/hydra-cli")

    man1.install Dir["docs/man/*.1"] if Dir.exist?("docs/man")

    generate_completions_from_executable(bin/"hydra", "completions")
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/hydra --version")
  end
end
EOF

# Update ${TAP_DIR}/Casks/hydra.rb if DMG SHA256s are available
if [ -n "$DMG_ARM_SHA" ] && [ -n "$DMG_INTEL_SHA" ]; then
cat > "${TAP_DIR}/Casks/hydra.rb" <<EOF
cask "hydra" do
  arch arm: "arm64", intel: "x86_64"

  version "${VERSION}"
  sha256 arm:   "${DMG_ARM_SHA}",
         intel: "${DMG_INTEL_SHA}"

  url "https://github.com/ja7ad/hydra/releases/download/v#{version}/Hydra-#{version}-#{arch}.dmg"
  name "Hydra"
  desc "Multi-source file retriever and download manager"
  homepage "https://github.com/ja7ad/hydra"

  livecheck do
    url :url
    strategy :github_latest
  end

  auto_updates true
  depends_on macos: :big_sur

  app "Hydra Download Manager.app"
  binary "#{appdir}/Hydra Download Manager.app/Contents/MacOS/hydra"
  manpage "#{appdir}/Hydra Download Manager.app/Contents/Resources/man/man1/hydra.1"

  zap trash: [
    "~/.config/hydra",
    "~/Library/Application Support/Hydra",
    "~/Library/Preferences/io.github.ja7ad.hydra.plist",
    "~/Library/Saved Application State/io.github.ja7ad.hydra.savedState",
  ]
end
EOF
fi

echo "Tap at ${TAP_DIR} updated successfully."
