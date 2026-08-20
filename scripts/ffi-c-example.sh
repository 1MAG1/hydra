#!/usr/bin/env bash
# Copyright (C) 2026 Javad Rajabzadeh
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Build libhydra and the C programs in examples/ffi-c against it, then run the
# ABI smoke test.
#
# This is the check that Rust tests cannot perform. A Rust test of Rust
# functions proves the logic works; only a C compiler and a linker prove that
# the committed header parses, that the struct layouts agree, and that every
# symbol the header promises is actually in the archive.
#
#   scripts/ffi-c-example.sh              build and run the smoke test
#   scripts/ffi-c-example.sh --profile release   optimised build
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
profile="debug"
cargo_profile=(--profile dev)
if [ "${1:-}" = "--profile" ] && [ "${2:-}" = "release" ]; then
    profile="release"
    cargo_profile=(--profile release)
fi

CC="${CC:-cc}"
out="$root/target/ffi-c"
mkdir -p "$out"

echo "==> building the static library"
# `--print native-static-libs` is how the exact system libraries a Rust
# staticlib needs are discovered, rather than guessed per platform. Guessing is
# what makes an FFI package fail to link on somebody else's machine.
link_log="$out/native-static-libs.txt"
cargo rustc "${cargo_profile[@]}" -p hya-ffi --crate-type staticlib \
    -- --print native-static-libs 2>&1 | tee "$link_log" >/dev/null || {
        cat "$link_log" >&2
        exit 1
    }

lib="$root/target/$profile/libhydra.a"
[ -f "$lib" ] || { echo "error: $lib was not produced" >&2; exit 1; }

native_libs="$(sed -n 's/^note: native-static-libs: *//p' "$link_log" | tail -1)"
echo "    $lib"
echo "    system libraries: ${native_libs:-none reported}"

# Strict C: the header is a published artifact and must compile cleanly for
# somebody whose build is stricter than ours.
cflags=(-std=c11 -Wall -Wextra -Wpedantic -Werror -I "$root/include")

for prog in abi_smoke download; do
    echo "==> compiling $prog.c"
    # shellcheck disable=SC2086
    "$CC" "${cflags[@]}" -o "$out/$prog" "$root/examples/ffi-c/$prog.c" \
        "$lib" $native_libs
done

# The header is a published artifact, so it has to survive every dialect a
# consumer might compile it in - not just the one this project happens to use.
# Compiled only, not linked: these assert about types.
echo "==> checking the header across C dialects"
for std in c11 c17; do
    "$CC" "-std=$std" -Wall -Wextra -Wpedantic -Werror -I "$root/include" \
        -c "$root/examples/ffi-c/header_c.c" -o "$out/header_$std.o"
    echo "    $std ok"
done

if command -v "${CXX:-c++}" >/dev/null 2>&1; then
    echo "==> checking the header across C++ dialects"
    for std in c++11 c++17; do
        "${CXX:-c++}" "-std=$std" -Wall -Wextra -Wpedantic -Werror \
            -I "$root/include" -c "$root/examples/ffi-c/header_cxx.cpp" \
            -o "$out/header_$std.o"
        echo "    $std ok"
    done
else
    echo "==> skipping the C++ header check (no C++ compiler)"
fi

echo "==> running the ABI smoke test"
"$out/abi_smoke"

echo
echo "built $out/download - try:"
echo "    $out/download https://example.com/file.iso ./file.iso"
