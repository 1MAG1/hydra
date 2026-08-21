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
#
# Runs against a Unix cc/c++ and against MSVC's cl, so the Windows leg of CI
# uses this script too rather than a second copy of it written in PowerShell.
# cl must be on PATH - a Visual Studio developer shell, or a CI step that sets
# one up.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
profile="debug"
cargo_profile=(--profile dev)
if [ "${1:-}" = "--profile" ] && [ "${2:-}" = "release" ]; then
    profile="release"
    cargo_profile=(--profile release)
fi

# The compiler for the archive is whatever built the archive, so ask rustc
# rather than uname: a msys/mingw bash on Windows still drives an MSVC toolchain.
host="$(rustc -vV | sed -n 's/^host: //p')"
case "$host" in
    *-pc-windows-msvc) toolchain="msvc" ;;
    *)                 toolchain="unix" ;;
esac

out="$root/target/ffi-c"
mkdir -p "$out"

echo "==> building the static library"
# `--print native-static-libs` is how the exact system libraries a Rust
# staticlib needs are discovered, rather than guessed per platform. Guessing is
# what makes an FFI package fail to link on somebody else's machine.
link_log="$out/native-static-libs.txt"
build_static_lib() {
    cargo rustc "${cargo_profile[@]}" -p hya-ffi --crate-type staticlib \
        -- --print native-static-libs >"$link_log" 2>&1 || {
            cat "$link_log" >&2
            exit 1
        }
}
# `.*` rather than `^note: ` because the note is a rustc diagnostic: how cargo
# decorates it is not part of any promise.
read_native_libs() {
    sed -n 's/.*native-static-libs: *//p' "$link_log" | tail -1
}

build_static_lib
native_libs="$(read_native_libs)"

if [ -z "$native_libs" ]; then
    # rustc prints the note while it links, so a build that cargo considers
    # fresh can answer with nothing at all. Make the unit stale and ask again -
    # only this crate recompiles, its dependencies stay cached.
    touch "$root/crates/hydra-ffi/src/lib.rs"
    build_static_lib
    native_libs="$(read_native_libs)"
fi

if [ -z "$native_libs" ]; then
    # Last resort, and the reason it is worth having: without -lm the link dies
    # on `exp` and `pow` from f64's methods, several hundred lines into an error
    # that says nothing about this script. A guess that is usually right beats a
    # failure nobody can read.
    case "$host" in
        *-apple-*)      native_libs="-liconv -lSystem -lc -lm" ;;
        *-linux-*)      native_libs="-lgcc_s -lutil -lrt -lpthread -lm -ldl -lc" ;;
        *-windows-msvc) native_libs="bcrypt.lib advapi32.lib kernel32.lib
                                     ntdll.lib userenv.lib ws2_32.lib
                                     dbghelp.lib" ;;
        *)              native_libs="-lm -lpthread -ldl" ;;
    esac
    echo "    warning: rustc reported no native-static-libs;" \
         "using the $host defaults" >&2
fi

if [ "$toolchain" = "msvc" ]; then
    archive="$root/target/$profile/hydra.lib"
else
    archive="$root/target/$profile/libhydra.a"
fi
[ -f "$archive" ] || { echo "error: $archive was not produced" >&2; exit 1; }

echo "    $archive"
echo "    system libraries: $native_libs"

if [ "$toolchain" = "msvc" ]; then
    # msys/mingw bash rewrites arguments that look like POSIX paths, which turns
    # `/defaultlib:libcmt` into a directory list and every MSVC flag into
    # nonsense. Switch the rewriting off and convert the paths this script owns
    # by hand; MSVC flags are spelled with `-` so they are never candidates.
    export MSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL='*'
    winpath() {
        if command -v cygpath >/dev/null 2>&1; then cygpath -w "$1"; else printf '%s' "$1"; fi
    }

    CC="${CC:-cl}"
    command -v "$CC" >/dev/null 2>&1 || {
        echo "error: $CC is not on PATH - run this from a Visual Studio" \
             "developer shell" >&2
        exit 1
    }

    # cl needs INCLUDE to compile and link.exe needs LIB to link, and it is LIB
    # that goes missing under msys bash: Windows compares environment names
    # case-insensitively while bash does not, so a pre-existing `Lib` survives
    # next to the `LIB` a developer shell exported and the child process can end
    # up reading the wrong one. Collapse the spellings to a single LIB.
    normalise_lib_env() {
        local name
        # Every case variant Windows would treat as LIB. The script's own
        # variables are named so that none of these collides with one.
        for name in Lib lib LIb LiB lIb lIB liB; do
            if [ -z "${LIB:-}" ] && [ -n "${!name:-}" ]; then
                export LIB="${!name}"
            fi
            unset "$name" || true
        done
    }

    # The static CRT is the one library that must be there: rustc built the
    # archive with +crt-static, so the link needs libcmt.lib. Looking for it is
    # also the cheapest way to tell a usable LIB from a missing one.
    crt_in_lib() {
        local dir posix
        local IFS=';'
        # shellcheck disable=SC2086
        for dir in ${LIB:-}; do
            [ -n "$dir" ] || continue
            posix="$(cygpath -u "$dir" 2>/dev/null || printf '%s' "$dir")"
            [ -f "$posix/libcmt.lib" ] && return 0
        done
        return 1
    }

    # No usable LIB: ask the Visual Studio installation itself rather than
    # giving up. This is what a developer shell does, and it is why this script
    # runs from a plain terminal too.
    import_msvc_env() {
        local vswhere vsdir vcvars arch line
        vswhere="/c/Program Files (x86)/Microsoft Visual Studio/Installer/vswhere.exe"
        [ -x "$vswhere" ] || return 1
        vsdir="$("$vswhere" -latest -products '*' -property installationPath | tr -d '\r')"
        [ -n "$vsdir" ] || return 1
        vcvars="$(cygpath -u "$vsdir" 2>/dev/null || printf '%s' "$vsdir")"
        vcvars="$vcvars/VC/Auxiliary/Build/vcvarsall.bat"
        [ -f "$vcvars" ] || return 1
        case "$host" in
            aarch64-*) arch="arm64" ;;
            *)         arch="x64" ;;
        esac
        while IFS= read -r line; do
            line="${line%$'\r'}"
            case "$line" in
                LIB=*|INCLUDE=*|LIBPATH=*) export "${line%%=*}=${line#*=}" ;;
            esac
        done < <(cmd /c "\"$(winpath "$vcvars")\" $arch >NUL && set" 2>/dev/null)
    }

    normalise_lib_env
    if ! crt_in_lib; then
        echo "==> LIB carries no static CRT; asking Visual Studio for one"
        import_msvc_env || true
        normalise_lib_env
    fi
    if ! crt_in_lib; then
        # A warning rather than an error: this is a diagnosis of a link that
        # has not been attempted yet, and being wrong about it must not stop a
        # build that would have worked. If the link does fail, LNK1104 on its
        # own says nothing, and what follows is the missing half of it.
        echo "    warning: no libcmt.lib in LIB; the link is likely to fail" >&2
        echo "    cl:  $(command -v "$CC")" >&2
        if [ -n "${LIB:-}" ]; then
            echo "    LIB:" >&2
            printf '%s\n' "$LIB" | tr ';' '\n' | sed 's/^/      /' >&2
        else
            echo "    LIB: not set at all" >&2
        fi
    fi

    # -MT, not the default -MD: .cargo/config.toml builds the Windows targets
    # with +crt-static, so rustc asks for libcmt and a C object compiled against
    # the DLL runtime would drag in a second, conflicting CRT.
    cflags=(-nologo -W3 -WX -MT -D_CRT_SECURE_NO_WARNINGS
            "-I$(winpath "$root/include")")

    for prog in abi_smoke download; do
        echo "==> compiling $prog.c"
        # -std:c11 explicitly: cl's default dialect for .c has no
        # _Static_assert, and the header asserts its whole layout with it.
        # shellcheck disable=SC2086
        "$CC" "${cflags[@]}" -std:c11 "-Fe:$(winpath "$out/$prog.exe")" \
            "-Fo:$(winpath "$out/$prog.obj")" \
            "$(winpath "$root/examples/ffi-c/$prog.c")" \
            -link "$(winpath "$archive")" $native_libs
    done

    # The header is a published artifact, so it has to survive every dialect a
    # consumer might compile it in - not just the one this project happens to
    # use. Compiled only, not linked: these assert about types.
    echo "==> checking the header across C dialects"
    # `default` is cl with no /std at all, which is the dialect a consumer's
    # existing project most likely uses and is pre-C11 - the case the header's
    # HYDRA_STATIC_ASSERT fallback exists for. Checking only c11 and c17 would
    # never compile that branch.
    for std in default c11 c17; do
        if [ "$std" = default ]; then
            "$CC" "${cflags[@]}" -c \
                "$(winpath "$root/examples/ffi-c/header_c.c")" \
                "-Fo:$(winpath "$out/header_default.obj")"
        else
            "$CC" "${cflags[@]}" "-std:$std" -c \
                "$(winpath "$root/examples/ffi-c/header_c.c")" \
                "-Fo:$(winpath "$out/header_$std.obj")"
        fi
        echo "    $std ok"
    done

    # MSVC has no -std:c++11; C++14 is its floor.
    echo "==> checking the header across C++ dialects"
    for std in c++14 c++17; do
        "${CXX:-$CC}" "${cflags[@]}" "-std:$std" -EHsc -c \
            "$(winpath "$root/examples/ffi-c/header_cxx.cpp")" \
            "-Fo:$(winpath "$out/header_$std.obj")"
        echo "    $std ok"
    done

    smoke="$out/abi_smoke.exe"
    client="$out/download.exe"
else
    CC="${CC:-cc}"

    # rustc's list says what the LINKER needs, and it is right about that. It is
    # not a list of files that exist to be found: -lc is the compiler driver's
    # own business on every Unix, and recent macOS SDKs ship no libm to find
    # because it lives inside libSystem. Passing those through unexamined turns
    # a working link into `cannot find -lc`. So ask this compiler which of them
    # it can resolve, and drop only the ones it cannot - the answer comes from
    # the toolchain doing the link rather than from a list in this script.
    usable_libs() {
        local probe="$out/probe.c" kept="" dropped=""
        printf 'int main(void) { return 0; }\n' > "$probe"
        while [ $# -gt 0 ]; do
            case "$1" in
                # `-framework Foo` is two tokens and names no file to look for.
                -framework) kept="$kept $1 ${2:-}"; shift 2; continue ;;
                -l*)
                    if "$CC" "$probe" "$1" -o "$out/probe.out" >/dev/null 2>&1; then
                        kept="$kept $1"
                    else
                        dropped="$dropped $1"
                    fi
                    ;;
                *) kept="$kept $1" ;;
            esac
            shift
        done
        rm -f "$probe" "$out/probe.out"
        [ -z "$dropped" ] || echo "    dropped (no such library for $CC):$dropped" >&2
        printf '%s' "${kept# }"
    }
    # shellcheck disable=SC2086
    link_libs="$(usable_libs $native_libs)"
    [ "$link_libs" = "$native_libs" ] || echo "    linking with: $link_libs"

    # Strict C: the header is a published artifact and must compile cleanly for
    # somebody whose build is stricter than ours.
    cflags=(-std=c11 -Wall -Wextra -Wpedantic -Werror -I "$root/include")

    for prog in abi_smoke download; do
        echo "==> compiling $prog.c"
        # shellcheck disable=SC2086
        "$CC" "${cflags[@]}" -o "$out/$prog" "$root/examples/ffi-c/$prog.c" \
            "$archive" $link_libs
    done

    # The header is a published artifact, so it has to survive every dialect a
    # consumer might compile it in - not just the one this project happens to
    # use. Compiled only, not linked: these assert about types.
    echo "==> checking the header across C dialects"
    # c99 as well as c11: under C99 the header falls back from _Static_assert
    # to a negative-length typedef, and that branch has to keep compiling.
    for std in c99 c11 c17; do
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

    smoke="$out/abi_smoke"
    client="$out/download"
fi

echo "==> running the ABI smoke test"
"$smoke"

echo
echo "built $client - try:"
echo "    $client https://example.com/file.iso ./file.iso"
