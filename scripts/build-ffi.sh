#!/usr/bin/env bash
# Copyright (C) 2026 Javad Rajabzadeh
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Build and package libhydra — the embeddable C ABI over the hydra download
# engine — for one target triple.
#
#   scripts/build-ffi.sh                              host target, release
#   scripts/build-ffi.sh --target aarch64-linux-android
#   scripts/build-ffi.sh --target x86_64-unknown-linux-musl --static-only
#   scripts/build-ffi.sh --list                       what CI builds
#
# This is the SAME script the release workflow runs. That is deliberate: an
# "official" build produced by steps that live only in a YAML file is a build
# nobody outside CI can reproduce, and the whole point of libhydra is that
# somebody else compiles it into their own application. If your platform is not
# in the release matrix, run this with your triple and you get the identical
# layout.
#
# Output, under --out (default target/ffi-dist):
#
#   libhydra-<version>-<triple>/
#       include/hydra.h              the published ABI
#       lib/libhydra.a               static library (always)
#       lib/libhydra.{so,dylib}      shared library (unless --static-only)
#       lib/pkgconfig/hydra.pc       pkg-config metadata (Unix targets)
#       docs/                        the binding guides from docs/ffi
#       examples/                    the C client and conformance program
#       native-static-libs.txt       system libraries the static archive needs
#       NOTICE.md, LICENSE-*, THIRD-PARTY-NOTICES.md
#   libhydra-<version>-<triple>.tar.gz   (.zip for *-windows-* targets)
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

# Overridable so the same script can be driven by `cross`, which supplies a
# container carrying the C toolchain for an exotic target. Everything below
# calls $CARGO rather than cargo directly for that reason.
CARGO="${CARGO:-cargo}"

# The triples the release workflow builds. Kept here rather than in the YAML so
# that `--list` and CI cannot disagree about what "supported" means.
SUPPORTED_TARGETS=(
    # Linux, glibc. The baseline for servers and desktops.
    x86_64-unknown-linux-gnu
    aarch64-unknown-linux-gnu
    armv7-unknown-linux-gnueabihf
    # Linux, musl. Static and dependency-free: containers, Alpine, embedded.
    x86_64-unknown-linux-musl
    aarch64-unknown-linux-musl
    # macOS.
    x86_64-apple-darwin
    aarch64-apple-darwin
    # Windows, MSVC ABI.
    x86_64-pc-windows-msvc
    aarch64-pc-windows-msvc
    # Android, the four ABIs Play Store apps ship.
    aarch64-linux-android
    armv7-linux-androideabi
    x86_64-linux-android
    i686-linux-android
    # iOS: device, and the two simulator architectures.
    aarch64-apple-ios
    aarch64-apple-ios-sim
    x86_64-apple-ios
)

target=""
profile="release"
out="$root/target/ffi-dist"
shared=1

while [ $# -gt 0 ]; do
    case "$1" in
        --target) target="${2:?--target needs a triple}"; shift 2 ;;
        --profile) profile="${2:?--profile needs a name}"; shift 2 ;;
        --out) out="${2:?--out needs a directory}"; shift 2 ;;
        --static-only) shared=0; shift ;;
        --list)
            printf '%s\n' "${SUPPORTED_TARGETS[@]}"
            exit 0
            ;;
        -h|--help) sed -n '2,40p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "error: unknown argument $1 (try --help)" >&2; exit 1 ;;
    esac
done

if [ -z "$target" ]; then
    target="$(rustc -vV | sed -n 's/^host: //p')"
    echo "==> no --target given; building for the host ($target)"
fi

version="$(sed -n 's/^version = "\(.*\)"/\1/p' crates/hydra-ffi/Cargo.toml | head -1)"
[ -n "$version" ] || { echo "error: cannot read hya-ffi version" >&2; exit 1; }

name="libhydra-${version}-${target}"
stage="$out/$name"
rm -rf "$stage"
mkdir -p "$stage/include" "$stage/lib" "$stage/docs" "$stage/examples"

case "$target" in
    *-windows-*) family=windows ;;
    *-apple-ios*) family=ios ;;
    *-apple-*)   family=apple ;;
    *-android*)  family=android ;;
    *)           family=unix ;;
esac

# ---------------------------------------------------------------- the header
#
# Regenerated when cbindgen is available so a local build cannot ship a header
# that has drifted; otherwise the committed one is used, which is the whole
# reason it is committed.
if command -v cbindgen >/dev/null 2>&1; then
    scripts/gen-ffi-header.sh >/dev/null
else
    echo "==> cbindgen not installed; using the committed include/hydra.h"
fi
cp include/hydra.h "$stage/include/"

# ------------------------------------------------------------------- building
#
# `cargo rustc --crate-type` rather than `cargo build`, so each library kind is
# requested explicitly. It matters for cross-compilation: building a staticlib
# never invokes a linker, so it succeeds on targets where no linker is
# configured, while a cdylib needs one. Asking for both at once would fail the
# static build for a reason that has nothing to do with it.
echo "==> building libhydra.a for $target ($profile)"
link_log="$stage/native-static-libs.txt"

build_static_lib() {
    set +e
    static_out="$("$CARGO" rustc --locked -p hya-ffi --target "$target" --profile "$profile" \
        --crate-type staticlib -- --print native-static-libs 2>&1)"
    static_rc=$?
    set -e
    if [ $static_rc -ne 0 ]; then
        echo "$static_out" >&2
        echo "error: static build failed for $target" >&2
        return $static_rc
    fi
    # `.*` rather than `^note: ` because the note is a rustc diagnostic: how
    # cargo decorates it is not part of any promise.
    printf '%s\n' "$static_out" | sed -n 's/.*native-static-libs: *//p' | tail -1
}

native_libs="$(build_static_lib)" || exit $?

if [ -z "$native_libs" ]; then
    # rustc prints the note while it links, so a build cargo considers fresh -
    # a warm CI cache, a second run in the same tree - can answer with nothing
    # at all. Make the unit stale and ask again; only this crate recompiles.
    touch "$root/crates/hydra-ffi/src/lib.rs"
    native_libs="$(build_static_lib)" || exit $?
fi

guessed=0
if [ -z "$native_libs" ]; then
    # Last resort, and the reason it is worth shipping one: an empty list in
    # this file means a consumer's link dies on `exp` and `pow` from f64's
    # methods, in an error that says nothing about libhydra. A guess that is
    # usually right beats that.
    guessed=1
    case "$target" in
        *-apple-*)      native_libs="-liconv -lSystem -lc -lm" ;;
        *-linux-*)      native_libs="-lgcc_s -lutil -lrt -lpthread -lm -ldl -lc" ;;
        *-windows-msvc) native_libs="bcrypt.lib advapi32.lib kernel32.lib ntdll.lib userenv.lib ws2_32.lib dbghelp.lib" ;;
        *-windows-gnu)  native_libs="-lkernel32 -ladvapi32 -lbcrypt -lntdll -luserenv -lws2_32 -ldbghelp" ;;
        *)              native_libs="-lm -lpthread -ldl" ;;
    esac
    echo "    warning: rustc reported no native-static-libs; recording the" \
         "$target defaults" >&2
fi

{
    echo "# System libraries required to link libhydra.a into a program."
    echo "# Target: $target"
    if [ "$guessed" = 1 ]; then
        echo "# rustc reported none at build time; these are the defaults for"
        echo "# this target. Verify with:"
        echo "#   cargo rustc -p hya-ffi --target $target --crate-type staticlib \\"
        echo "#       -- --print native-static-libs"
    else
        echo "# Reported by rustc at build time, not guessed per platform."
    fi
    echo
    echo "$native_libs"
} > "$link_log"

if [ "$shared" = 1 ]; then
    echo "==> building the shared library for $target"
    if ! "$CARGO" rustc --locked -p hya-ffi --target "$target" --profile "$profile" \
        --crate-type cdylib >/dev/null 2>&1; then
        # A missing cross-linker is the usual cause and is not fatal: the static
        # archive is the primary deliverable and is already built.
        echo "    note: no shared library produced (no linker for $target?)"
        shared=0
    fi
fi

# Cargo writes `--profile dev` output to target/<triple>/debug.
dir="$profile"
[ "$profile" = dev ] && dir=debug
bin="target/$target/$dir"

copied=0
for f in libhydra.a libhydra.so libhydra.dylib hydra.lib hydra.dll hydra.dll.lib; do
    if [ -f "$bin/$f" ]; then
        cp "$bin/$f" "$stage/lib/"
        copied=$((copied + 1))
    fi
done
[ "$copied" -gt 0 ] || { echo "error: no library artifacts in $bin" >&2; exit 1; }

# ------------------------------------------------------------------ pkg-config
#
# Unix only, and worth the twelve lines: it is how a C or C++ consumer discovers
# the include path and — the part people get wrong — the system libraries the
# static archive needs, which differ per platform and are recorded above.
if [ "$family" = unix ] || [ "$family" = apple ]; then
    mkdir -p "$stage/lib/pkgconfig"
    private="$(grep -v '^#' "$link_log" | tr -d '\n' | sed 's/^ *//')"
    cat > "$stage/lib/pkgconfig/hydra.pc" <<PC
prefix=/usr/local
exec_prefix=\${prefix}
libdir=\${exec_prefix}/lib
includedir=\${prefix}/include

Name: hydra
Description: Embeddable multi-source download engine (C ABI over hya-core and hya-net)
URL: https://github.com/ja7ad/hydra
Version: ${version}
Cflags: -I\${includedir}
Libs: -L\${libdir} -lhydra
Libs.private: ${private}
PC
fi

# ------------------------------------------------------------------- the rest
cp crates/hydra-ffi/README.md "$stage/README.md"
cp docs/ffi/*.md "$stage/docs/"
cp examples/ffi-c/*.c examples/ffi-c/*.cpp "$stage/examples/"
cp LICENSE-MIT LICENSE-APACHE THIRD-PARTY-NOTICES.md "$stage/"

# The NOTICE is not boilerplate: an Android AAR or an iOS framework built from
# this archive inherits these terms, and "which licence does the thing I am
# shipping have" is the question a downstream release engineer actually asks.
cat > "$stage/NOTICE.md" <<NOTICE
# libhydra ${version} — ${target}

An embeddable, multi-source download engine with a stable C ABI.

## Licence

libhydra is **MIT OR Apache-2.0**, at your option. See \`LICENSE-MIT\` and
\`LICENSE-APACHE\`.

This is deliberately *not* the licence of the \`hydra\` command-line tool, which
is GPL-3.0-or-later. This package contains no GPL code: it is built from
\`hya-ffi\`, \`hya-core\` and \`hya-net\` only, and those three crates are
permissively licensed precisely so that this archive can be linked into your
application. Third-party dependency terms are in \`THIRD-PARTY-NOTICES.md\`.

## Contents

- \`include/hydra.h\` — the published ABI. Generated from the Rust definitions
  and carrying static assertions that check the layout against your compiler.
- \`docs/ABI.md\` — the specification: design principles, the ABI stability
  policy, ownership, the event queue's guarantees. Read it before writing a
  binding.
- \`lib/\` — the libraries. See \`native-static-libs.txt\` for the system
  libraries the static archive must be linked against.
- \`docs/\` — the ABI specification, the per-platform integration guides, and
  the language binding guide.
- \`examples/\` — a complete C client, the ABI conformance program, and the
  forward-compatibility probe CI runs against every published header.

## ABI version

$(grep -m1 '^#define HYDRA_FFI_ABI_VERSION' include/hydra.h)

Compare it against \`hydra_ffi_abi_version()\` at startup and refuse to run on a
mismatch: a header from one ABI and a library from another disagree about the
layout of every struct.
NOTICE

# ------------------------------------------------------------------- archiving
mkdir -p "$out"
if [ "$family" = windows ]; then
    # .zip for Windows consumers, and 7z where it exists because the GitHub
    # Windows runners ship it while `zip` is not guaranteed.
    rm -f "$out/$name.zip"
    if command -v 7z >/dev/null 2>&1; then
        ( cd "$out" && 7z a -bso0 -bsp0 "$name.zip" "$name" >/dev/null )
    elif command -v zip >/dev/null 2>&1; then
        ( cd "$out" && zip -qr "$name.zip" "$name" )
    else
        echo "error: neither 7z nor zip is available to archive $name" >&2
        exit 1
    fi
    archive="$out/$name.zip"
else
    tar -czf "$out/$name.tar.gz" -C "$out" "$name"
    archive="$out/$name.tar.gz"
fi

echo
echo "==> $archive"
find "$stage" -type f | sed "s|$stage|    $name|" | sort
