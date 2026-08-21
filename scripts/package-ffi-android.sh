#!/usr/bin/env bash
# Copyright (C) 2026 Javad Rajabzadeh
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Build libhydra for Android and lay it out the way an Android project consumes
# native code.
#
#   scripts/package-ffi-android.sh
#   scripts/package-ffi-android.sh --abis arm64-v8a,x86_64
#   scripts/package-ffi-android.sh --api 24 --out DIR
#
# Requires the NDK. The script finds it in this order, and says which it used:
#   $ANDROID_NDK_HOME, $ANDROID_NDK_ROOT, $ANDROID_NDK_LATEST_HOME
#   $ANDROID_HOME/ndk/<newest>
#
# `cargo-ndk` is used when present. That is not laziness: it sets the clang
# wrappers, `AR`, the API-level suffix and — the part hand-rolled setups
# famously get wrong — the `libunwind` link path that NDK r23 and later require.
# When it is absent the environment is configured directly, which works but is
# the path more likely to break on a future NDK layout.
#
# Output:
#   libhydra-<version>-android/
#       include/hydra.h
#       jniLibs/<abi>/libhydra.so     drop into src/main/jniLibs
#       static/<abi>/libhydra.a       for an NDK CMake/ndk-build target
#       docs/, examples/, NOTICE.md, LICENSE-*
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

out="$root/target/ffi-dist"
profile="release"
# 21 is Android 5.0. Below that the NDK's own toolchain support is gone, and
# above it excludes devices for no gain — the engine uses nothing newer.
api=21
abis="arm64-v8a,armeabi-v7a,x86_64,x86"

while [ $# -gt 0 ]; do
    case "$1" in
        --out) out="${2:?--out needs a directory}"; shift 2 ;;
        --profile) profile="${2:?--profile needs a name}"; shift 2 ;;
        --api) api="${2:?--api needs a level}"; shift 2 ;;
        --abis) abis="${2:?--abis needs a list}"; shift 2 ;;
        -h|--help) sed -n '2,30p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "error: unknown argument $1" >&2; exit 1 ;;
    esac
done

# ------------------------------------------------------------------- the NDK
ndk="${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-${ANDROID_NDK_LATEST_HOME:-}}}"
if [ -z "$ndk" ] && [ -n "${ANDROID_HOME:-}" ] && [ -d "$ANDROID_HOME/ndk" ]; then
    ndk="$ANDROID_HOME/ndk/$(ls -1 "$ANDROID_HOME/ndk" | sort -V | tail -1)"
fi
if [ -z "$ndk" ] || [ ! -d "$ndk" ]; then
    cat >&2 <<'MSG'
error: no Android NDK found.

       Set ANDROID_NDK_HOME to an NDK installation, or install one through
       Android Studio (SDK Manager -> SDK Tools -> NDK). On a CI runner the
       variable is usually already set as ANDROID_NDK_LATEST_HOME.
MSG
    exit 1
fi
echo "==> NDK: $ndk"

case "$(uname -s)" in
    Darwin) host_tag=darwin-x86_64 ;;
    Linux)  host_tag=linux-x86_64 ;;
    *)      echo "error: unsupported build host $(uname -s)" >&2; exit 1 ;;
esac
ndk_bin="$ndk/toolchains/llvm/prebuilt/$host_tag/bin"
[ -d "$ndk_bin" ] || { echo "error: no toolchain at $ndk_bin" >&2; exit 1; }

# ABI -> (rust target, clang triple). The clang triple differs from the Rust
# one for 32-bit ARM, which is the classic source of a "linker not found" that
# looks like a broken NDK and is not.
abi_target() {
    case "$1" in
        arm64-v8a)   echo "aarch64-linux-android aarch64-linux-android" ;;
        armeabi-v7a) echo "armv7-linux-androideabi armv7a-linux-androideabi" ;;
        x86_64)      echo "x86_64-linux-android x86_64-linux-android" ;;
        x86)         echo "i686-linux-android i686-linux-android" ;;
        *) echo "error: unknown ABI $1" >&2; exit 1 ;;
    esac
}

version="$(sed -n 's/^version = "\(.*\)"/\1/p' crates/hydra-ffi/Cargo.toml | head -1)"
name="libhydra-${version}-android"
stage="$out/$name"
rm -rf "$stage"
mkdir -p "$stage/include" "$stage/docs" "$stage/examples"

if command -v cbindgen >/dev/null 2>&1; then
    scripts/gen-ffi-header.sh >/dev/null
fi
cp include/hydra.h "$stage/include/"

use_cargo_ndk=0
command -v cargo-ndk >/dev/null 2>&1 && use_cargo_ndk=1
[ "$use_cargo_ndk" = 1 ] && echo "==> using cargo-ndk" \
                         || echo "==> cargo-ndk not found; configuring the toolchain directly"

dir="$profile"
[ "$profile" = dev ] && dir=debug

IFS=',' read -r -a abi_list <<< "$abis"
for abi in "${abi_list[@]}"; do
    read -r target clang_triple <<< "$(abi_target "$abi")"
    echo "==> $abi ($target, API $api)"
    rustup target add "$target" >/dev/null 2>&1 || true

    if [ "$use_cargo_ndk" = 1 ]; then
        cargo ndk --target "$abi" --platform "$api" -- \
            rustc --locked -p hya-ffi --profile "$profile" --crate-type staticlib >/dev/null
        cargo ndk --target "$abi" --platform "$api" -- \
            rustc --locked -p hya-ffi --profile "$profile" --crate-type cdylib >/dev/null
    else
        cc="$ndk_bin/${clang_triple}${api}-clang"
        [ -x "$cc" ] || { echo "error: no compiler at $cc" >&2; exit 1; }
        # Rust spells the env-var suffix in upper snake case.
        up="$(echo "$target" | tr 'a-z-' 'A-Z_')"
        export "CC_${target}=$cc"
        export "AR_${target}=$ndk_bin/llvm-ar"
        export "CARGO_TARGET_${up}_LINKER=$cc"
        export "CARGO_TARGET_${up}_AR=$ndk_bin/llvm-ar"
        cargo rustc --locked -p hya-ffi --target "$target" --profile "$profile" \
            --crate-type staticlib >/dev/null
        cargo rustc --locked -p hya-ffi --target "$target" --profile "$profile" \
            --crate-type cdylib >/dev/null
    fi

    mkdir -p "$stage/jniLibs/$abi" "$stage/static/$abi"
    cp "target/$target/$dir/libhydra.so" "$stage/jniLibs/$abi/"
    cp "target/$target/$dir/libhydra.a" "$stage/static/$abi/"
    # Stripping is worth doing here rather than leaving to the app's build:
    # the debug info in a release .so is several times the code, it ships to
    # every device, and nothing on the device can use it.
    "$ndk_bin/llvm-strip" --strip-unneeded "$stage/jniLibs/$abi/libhydra.so" 2>/dev/null || true
    ls -lh "$stage/jniLibs/$abi/libhydra.so" | awk '{print "    " $9 "  " $5}'
done

# A CMake package config, so an app with native code can `find_package(hydra)`
# and link the static library from its own CMakeLists rather than copying paths
# around by hand.
mkdir -p "$stage/cmake"
cat > "$stage/cmake/hydra-config.cmake" <<'CMAKE'
# Use from an Android (or any NDK) CMake project:
#
#   list(APPEND CMAKE_PREFIX_PATH "${CMAKE_SOURCE_DIR}/libhydra/cmake")
#   find_package(hydra REQUIRED)
#   target_link_libraries(my_native_lib PRIVATE hydra::hydra)
#
# Resolves the right ABI slice from ANDROID_ABI, so one line covers all four.
get_filename_component(_hydra_root "${CMAKE_CURRENT_LIST_DIR}/.." ABSOLUTE)

if(ANDROID_ABI)
    set(_hydra_lib "${_hydra_root}/static/${ANDROID_ABI}/libhydra.a")
else()
    message(FATAL_ERROR "hydra: ANDROID_ABI is not set; this package config is for NDK builds")
endif()

if(NOT EXISTS "${_hydra_lib}")
    message(FATAL_ERROR "hydra: no library for ABI ${ANDROID_ABI} at ${_hydra_lib}")
endif()

add_library(hydra::hydra STATIC IMPORTED)
set_target_properties(hydra::hydra PROPERTIES
    IMPORTED_LOCATION "${_hydra_lib}"
    INTERFACE_INCLUDE_DIRECTORIES "${_hydra_root}/include")
CMAKE

cp crates/hydra-ffi/README.md "$stage/README.md"
cp docs/ffi/*.md "$stage/docs/"
cp examples/ffi-c/*.c examples/ffi-c/*.cpp "$stage/examples/"
cp LICENSE-MIT LICENSE-APACHE THIRD-PARTY-NOTICES.md "$stage/"

cat > "$stage/NOTICE.md" <<NOTICE
# libhydra ${version} — Android

Built against NDK API level ${api} for: ${abis}

## Layout

- \`jniLibs/<abi>/libhydra.so\` — copy into \`src/main/jniLibs/\` and load with
  \`System.loadLibrary("hydra")\`, or point \`sourceSets\` at this directory.
- \`static/<abi>/libhydra.a\` — for a native target of your own that links the
  engine directly; see \`cmake/hydra-config.cmake\`.
- \`include/hydra.h\` — the published ABI.

## Licence

**MIT OR Apache-2.0**. No GPL code is included: this is built from
\`hya-ffi\`, \`hya-core\` and \`hya-net\` only. Third-party dependency terms are
in \`THIRD-PARTY-NOTICES.md\`, and they must be reproduced in your app's
open-source-licences screen.

## Before you ship

hydra is the download engine, not the lifecycle owner. Android decides when your
process may run; a foreground service, a user-initiated data transfer job or
WorkManager must own execution, and the engine simply works while it is allowed
to. See \`docs/android.md\`.
NOTICE

mkdir -p "$out"
( cd "$out" && rm -f "$name.zip" && zip -qry "$name.zip" "$name" )
echo
echo "==> $out/$name.zip"
