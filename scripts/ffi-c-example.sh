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
    # What this does NOT fix is a space inside an argument - see the response
    # files further down, which is where that is dealt with.
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

    # LIB, split once: the Windows spelling is what the linker is given, the
    # POSIX spelling is what this script looks things up with.
    split_lib() {
        local rest="${LIB:-}" dir
        lib_dirs_win=()
        lib_dirs_posix=()
        while [ -n "$rest" ]; do
            case "$rest" in
                *\;*) dir="${rest%%;*}"; rest="${rest#*;}" ;;
                *)    dir="$rest";       rest="" ;;
            esac
            # A trailing backslash immediately before the closing quote of a
            # quoted argument escapes that quote, and the rest of the linker's
            # command line then means something else entirely. Drop it - the
            # directory is the same one either way.
            while [ -n "$dir" ] && [ "${dir%\\}" != "$dir" ]; do
                dir="${dir%\\}"
            done
            [ -n "$dir" ] || continue
            lib_dirs_win+=("$dir")
            lib_dirs_posix+=("$(cygpath -u "$dir" 2>/dev/null || printf '%s' "$dir")")
        done
    }

    # Where a library actually is, answered by this shell rather than by the
    # linker's own search.
    find_in_lib() {
        local name="$1" dir
        for dir in ${lib_dirs_posix[@]+"${lib_dirs_posix[@]}"}; do
            if [ -f "$dir/$name" ]; then
                winpath "$dir/$name"
                return 0
            fi
        done
        return 1
    }

    # The same lookup, answering with a path this shell can read rather than
    # one the linker can.
    find_in_lib_posix() {
        local name="$1" dir
        for dir in ${lib_dirs_posix[@]+"${lib_dirs_posix[@]}"}; do
            if [ -f "$dir/$name" ]; then
                printf '%s' "$dir/$name"
                return 0
            fi
        done
        return 1
    }

    # What a failed link could not tell us: where the linker came from, which
    # directories it was given, whether the static CRT is in any of them, what
    # that file looks like from here, and - the one answer no guess replaces -
    # which paths the linker itself searched.
    report_link_failure() {
        local dir crt rsp="${1:-}"
        echo "--- link failed; the state it ran in ---" >&2
        echo "  cl:   $(command -v "$CC")" >&2
        echo "  link: $LINK_EXE" >&2
        echo "  LIB directories, and whether each holds libcmt.lib:" >&2
        for dir in ${lib_dirs_posix[@]+"${lib_dirs_posix[@]}"}; do
            if [ -f "$dir/libcmt.lib" ]; then
                echo "    yes  $dir" >&2
            elif [ -d "$dir" ]; then
                echo "    no   $dir" >&2
            else
                echo "    ??   $dir  (bash sees no such directory)" >&2
            fi
        done
        [ -n "${LIB:-}" ] || echo "    LIB is not set at all" >&2

        if crt="$(find_in_lib_posix libcmt.lib)"; then
            echo "  libcmt.lib as this shell sees it:" >&2
            ls -l "$crt" >&2 || true
            # An MS archive begins with `!<arch>`. Anything else means the file
            # is a placeholder rather than a library, which would explain a
            # linker that cannot open what bash can see.
            echo "  its first bytes:" >&2
            head -c 16 "$crt" | od -c | head -2 >&2 || true
        fi

        if [ -n "$rsp" ]; then
            echo "  what the linker searched, from the linker (-verbose:lib):" >&2
            cp "$rsp" "$rsp.verbose"
            printf -- '/VERBOSE:LIB\n' >> "$rsp.verbose"
            "$LINK_EXE" "@$(winpath "$rsp.verbose")" 2>&1 |
                head -40 | sed 's/^/    /' >&2 || true
        fi
    }

    normalise_lib_env
    split_lib
    if ! find_in_lib libcmt.lib >/dev/null; then
        # No static CRT in LIB: ask the Visual Studio installation itself rather
        # than giving up. This is what a developer shell does, and it is why
        # this script runs from a plain terminal too.
        echo "==> LIB carries no static CRT; asking Visual Studio for one"
        import_msvc_env || true
        normalise_lib_env
        split_lib
    fi

    # rustc built the archive with +crt-static, so these four are what the link
    # needs from the CRT. Each is named by its full path, and each is then
    # struck off the default-library list.
    #
    # That second half is the fix for this platform, and /VERBOSE:LIB is what
    # showed why. The linker opens these libraries happily when they are named
    # in full - it says so - and then fails on a *by-name* request for
    # `libcmt.lib` coming from the /DEFAULTLIB:LIBCMT directive that every
    # /MT object carries. The library it cannot find is one it already has
    # open. /NODEFAULTLIB drops the request: the symbols come from the copy on
    # the command line, which is the same file.
    crt_libs=()
    crt_nodefault=()
    for name in libcmt libvcruntime libucrt oldnames; do
        if found="$(find_in_lib "$name.lib")"; then
            crt_libs+=("$found")
            # Both spellings: the directive in the object says LIBCMT and the
            # file is libcmt.lib, and an unmatched /NODEFAULTLIB costs nothing.
            crt_nodefault+=("/NODEFAULTLIB:$name.lib" "/NODEFAULTLIB:$name")
        fi
    done
    if [ ${#crt_libs[@]} -eq 0 ]; then
        echo "    warning: none of the CRT libraries are in LIB;" \
             "the link is likely to fail" >&2
        report_link_failure
    fi

    # And invoke the linker ourselves rather than through cl, so that the
    # argument list this script builds is the argument list link.exe sees.
    # link.exe sits next to cl.exe; `command -v link` would find GNU coreutils'
    # link first on a bash PATH.
    LINK_EXE="$(dirname "$(command -v "$CC")")/link.exe"
    [ -x "$LINK_EXE" ] || LINK_EXE="link"

    # -MT, not the default -MD: .cargo/config.toml builds the Windows targets
    # with +crt-static, so rustc asks for libcmt and a C object compiled against
    # the DLL runtime would drag in a second, conflicting CRT.
    cflags=(/nologo /W3 /WX /MT /D_CRT_SECURE_NO_WARNINGS)
    include_dir="$(winpath "$root/include")"

    # Every MSVC tool is driven through a response file, and that is the whole
    # Windows story of this script. An argument containing a space does not
    # survive the msys-to-Windows boundary: `C:\Program Files\...\libcmt.lib`
    # arrives at the linker in pieces, which is why it reported a file bash
    # could see perfectly well as impossible to open. A response file reduces
    # the command line to one token with no space in it, and the quoting inside
    # the file is the tool's own to parse.
    cl_rsp_head() {
        printf -- '%s\n' "${cflags[@]}"
        printf -- '/I"%s"\n' "$include_dir"
    }

    for prog in abi_smoke download; do
        echo "==> compiling $prog.c"
        rsp="$out/$prog.cl.rsp"
        {
            cl_rsp_head
            # -std:c11 explicitly: cl's default dialect for .c has no
            # _Static_assert, and the header asserts its whole layout with it.
            printf -- '/std:c11\n/c\n'
            printf -- '/Fo:"%s"\n' "$(winpath "$out/$prog.obj")"
            printf '"%s"\n' "$(winpath "$root/examples/ffi-c/$prog.c")"
        } > "$rsp"
        "$CC" "@$(winpath "$rsp")"

        echo "==> linking $prog.exe"
        rsp="$out/$prog.link.rsp"
        {
            printf -- '/nologo\n'
            printf -- '/OUT:"%s"\n' "$(winpath "$out/$prog.exe")"
            printf '"%s"\n' "$(winpath "$out/$prog.obj")"
            printf '"%s"\n' "$(winpath "$archive")"
            for dir in ${lib_dirs_win[@]+"${lib_dirs_win[@]}"}; do
                printf -- '/LIBPATH:"%s"\n' "$dir"
            done
            for name in ${crt_libs[@]+"${crt_libs[@]}"}; do
                printf '"%s"\n' "$name"
            done
            printf '%s\n' ${crt_nodefault[@]+"${crt_nodefault[@]}"}
            # shellcheck disable=SC2086
            printf '%s\n' $native_libs
        } > "$rsp"
        # Printed because every Windows failure here has come down to what the
        # linker was handed, which no error message from it ever says.
        sed 's/^/      /' "$rsp"
        "$LINK_EXE" "@$(winpath "$rsp")" || { report_link_failure "$rsp"; exit 1; }
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
        rsp="$out/header_$std.rsp"
        {
            cl_rsp_head
            [ "$std" = default ] || printf -- '/std:%s\n' "$std"
            printf -- '/c\n'
            printf -- '/Fo:"%s"\n' "$(winpath "$out/header_$std.obj")"
            printf '"%s"\n' "$(winpath "$root/examples/ffi-c/header_c.c")"
        } > "$rsp"
        "$CC" "@$(winpath "$rsp")"
        echo "    $std ok"
    done

    # MSVC has no -std:c++11; C++14 is its floor.
    echo "==> checking the header across C++ dialects"
    for std in c++14 c++17; do
        rsp="$out/header_$std.rsp"
        {
            cl_rsp_head
            printf -- '/std:%s\n/EHsc\n/c\n' "$std"
            printf -- '/Fo:"%s"\n' "$(winpath "$out/header_$std.obj")"
            printf '"%s"\n' "$(winpath "$root/examples/ffi-c/header_cxx.cpp")"
        } > "$rsp"
        "${CXX:-$CC}" "@$(winpath "$rsp")"
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
