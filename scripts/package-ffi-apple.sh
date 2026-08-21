#!/usr/bin/env bash
# Copyright (C) 2026 Javad Rajabzadeh
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Build libhydra for Apple platforms and assemble Hydra.xcframework.
#
#   scripts/package-ffi-apple.sh                 iOS device + simulator + macOS
#   scripts/package-ffi-apple.sh --no-macos      iOS only
#   scripts/package-ffi-apple.sh --out DIR
#
# Why an XCFramework rather than a pile of .a files: it is the only artifact
# Xcode can consume that carries several architectures for several *platforms*
# at once. A universal binary cannot, because iOS-device arm64 and
# iOS-simulator arm64 are the same architecture for different platforms — `lipo`
# refuses to put them in one file, and that refusal is why the XCFramework
# format exists.
#
# Static libraries, not dynamic. Apple's guidance for embedded third-party code
# is static linking, an app that dynamically links a third-party library pays a
# launch-time cost for it, and the download engine has no reason to be
# separately replaceable at run time.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

out="$root/target/ffi-dist"
with_macos=1
with_ios=1
profile="release"

while [ $# -gt 0 ]; do
    case "$1" in
        --out) out="${2:?--out needs a directory}"; shift 2 ;;
        --profile) profile="${2:?--profile needs a name}"; shift 2 ;;
        --no-macos) with_macos=0; shift ;;
        # macOS only, for somebody who wants a universal desktop library and no
        # mobile slices — and the only path testable without full Xcode.
        --no-ios) with_ios=0; shift ;;
        -h|--help) sed -n '2,25p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "error: unknown argument $1" >&2; exit 1 ;;
    esac
done

[ "$(uname -s)" = Darwin ] || { echo "error: this must run on macOS" >&2; exit 1; }

[ "$with_ios" = 1 ] || [ "$with_macos" = 1 ] || {
    echo "error: --no-ios and --no-macos leaves nothing to build" >&2; exit 1; }

# Fail here, with the fix, rather than three minutes later inside a cc-rs error
# about `xcrun --show-sdk-path`. The Command Line Tools alone carry no iOS SDK.
if [ "$with_ios" = 1 ] && ! xcrun --sdk iphoneos --show-sdk-path >/dev/null 2>&1; then
    cat >&2 <<'MSG'
error: no iOS SDK. Building for iOS needs full Xcode, not just the Command
       Line Tools. Install Xcode from the App Store, then point the toolchain
       at it:

           sudo xcode-select -s /Applications/Xcode.app/Contents/Developer
           xcodebuild -runFirstLaunch

       Both commands need your password, so run them yourself.
MSG
    exit 1
fi

version="$(sed -n 's/^version = "\(.*\)"/\1/p' crates/hydra-ffi/Cargo.toml | head -1)"
name="libhydra-${version}-apple"
stage="$out/$name"
work="$out/.apple-work"
rm -rf "$stage" "$work"
mkdir -p "$stage" "$work"

if command -v cbindgen >/dev/null 2>&1; then
    scripts/gen-ffi-header.sh >/dev/null
fi

# The headers directory every slice shares. The module map is what lets Swift
# write `import Hydra` instead of a bridging header, and what makes the
# framework usable from a Swift package rather than only from a mixed target.
headers="$work/Headers"
mkdir -p "$headers"
cp include/hydra.h "$headers/"
cat > "$headers/module.modulemap" <<'MAP'
// Clang module for libhydra.
//
// `export *` re-exports what hydra.h itself includes (stdint.h, stddef.h), so a
// Swift or C consumer that imports Hydra gets the fixed-width integer types the
// ABI is written in without importing them separately.
module Hydra {
    header "hydra.h"
    export *
}
MAP

ios_targets=(aarch64-apple-ios)
sim_targets=(aarch64-apple-ios-sim x86_64-apple-ios)
mac_targets=(aarch64-apple-darwin x86_64-apple-darwin)

# Prints ONLY the artifact path on stdout; progress goes to stderr, because the
# caller reads this through a command substitution and a stray progress line
# would become part of the filename.
build() {
    local target="$1"
    echo "==> building libhydra.a for $target" >&2
    rustup target add "$target" >/dev/null 2>&1 || true
    # Static only: see the note at the top about why nothing here is dynamic.
    if ! cargo rustc --locked -p hya-ffi --target "$target" --profile "$profile" \
        --crate-type staticlib >&2; then
        echo "error: build failed for $target" >&2
        exit 1
    fi
    local dir="$profile"
    [ "$profile" = dev ] && dir=debug
    echo "target/$target/$dir/libhydra.a"
}

# `lipo -create` for slices that share a platform, which is legal, and never
# across platforms, which is not.
fuse() {
    local dest="$1"; shift
    mkdir -p "$(dirname "$dest")"
    if [ $# -eq 1 ]; then
        cp "$1" "$dest"
    else
        lipo -create "$@" -output "$dest"
    fi
    echo "    $(lipo -info "$dest")" >&2
}

xcargs=()
if [ "$with_ios" = 1 ]; then
    device_lib="$work/ios-device/libhydra.a"
    libs=()
    for t in "${ios_targets[@]}"; do libs+=("$(build "$t")"); done
    fuse "$device_lib" "${libs[@]}"

    sim_lib="$work/ios-simulator/libhydra.a"
    libs=()
    for t in "${sim_targets[@]}"; do libs+=("$(build "$t")"); done
    fuse "$sim_lib" "${libs[@]}"

    xcargs+=(-library "$device_lib" -headers "$headers"
             -library "$sim_lib" -headers "$headers")
fi

if [ "$with_macos" = 1 ]; then
    mac_lib="$work/macos/libhydra.a"
    libs=()
    for t in "${mac_targets[@]}"; do libs+=("$(build "$t")"); done
    fuse "$mac_lib" "${libs[@]}"
    xcargs+=(-library "$mac_lib" -headers "$headers")
fi

echo "==> assembling Hydra.xcframework"
framework="$stage/Hydra.xcframework"
rm -rf "$framework"
if xcodebuild -create-xcframework "${xcargs[@]}" -output "$framework" >/dev/null 2>&1; then
    echo "    $framework"
else
    # Command Line Tools alone cannot create an XCFramework; full Xcode can.
    # Rather than fail the build, ship the slices and the module map so the
    # archive is still usable, and say plainly what is missing.
    echo "    note: xcodebuild could not create the XCFramework." >&2
    echo "          Full Xcode is required (xcode-select -p currently reports" >&2
    echo "          $(xcode-select -p)). Shipping the individual slices instead." >&2
    rm -rf "$framework"
    mkdir -p "$stage/include"
    if [ "$with_ios" = 1 ]; then
        mkdir -p "$stage/lib/ios-arm64" "$stage/lib/ios-simulator"
        cp "$device_lib" "$stage/lib/ios-arm64/"
        cp "$sim_lib" "$stage/lib/ios-simulator/"
    fi
    [ "$with_macos" = 1 ] && { mkdir -p "$stage/lib/macos"; cp "$mac_lib" "$stage/lib/macos/"; }
    cp "$headers/hydra.h" "$headers/module.modulemap" "$stage/include/"
fi

# A Package.swift so the archive can be consumed as a binary Swift package —
# which is how a Swift application actually wants to depend on this.
cat > "$stage/Package.swift" <<'SWIFT'
// swift-tools-version:5.7
//
// A binary target, not a source one: the engine is Rust, already compiled for
// every Apple slice in Hydra.xcframework. Point `path` at the framework beside
// this file, or replace it with `url`/`checksum` to consume a released archive.
import PackageDescription

let package = Package(
    name: "Hydra",
    platforms: [.iOS(.v13), .macOS(.v11)],
    products: [
        .library(name: "Hydra", targets: ["Hydra"]),
    ],
    targets: [
        .binaryTarget(name: "Hydra", path: "Hydra.xcframework"),
    ]
)
SWIFT

cp crates/hydra-ffi/README.md "$stage/README.md"
mkdir -p "$stage/docs"
cp docs/ffi/*.md "$stage/docs/"
cp LICENSE-MIT LICENSE-APACHE THIRD-PARTY-NOTICES.md "$stage/"
cat > "$stage/NOTICE.md" <<NOTICE
# libhydra ${version} — Apple platforms

\`Hydra.xcframework\` contains static libraries for:

$([ "$with_ios" = 1 ] && printf -- '- iOS device (arm64)\n- iOS simulator (arm64, x86_64)\n')$([ "$with_macos" = 1 ] && echo "- macOS (arm64, x86_64)")

Licence: **MIT OR Apache-2.0**. This package contains no GPL code — see
\`NOTICE\` in the platform-neutral archive and \`docs/ios.md\` for integration.

Add it to an Xcode target under *Frameworks, Libraries, and Embedded Content*
with **Do Not Embed** (it is static), or depend on the \`Package.swift\` beside
it as a binary Swift package.
NOTICE

mkdir -p "$out"
( cd "$out" && rm -f "$name.zip" && zip -qry "$name.zip" "$name" )
echo
echo "==> $out/$name.zip"
find "$stage" -maxdepth 2 | sed "s|$stage|    $name|" | sort | head -30
