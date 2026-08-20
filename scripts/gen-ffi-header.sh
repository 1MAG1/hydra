#!/usr/bin/env bash
# Copyright (C) 2026 Javad Rajabzadeh
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Regenerate include/hydra.h from the Rust definitions in crates/hydra-ffi.
#
# The header is a PUBLISHED ABI ARTIFACT: it is committed, and third parties
# compile against it. It is generated rather than hand-written so it cannot
# drift from the implementation, and it is committed rather than built on
# demand so that consuming it needs no Rust toolchain.
#
#   scripts/gen-ffi-header.sh          write include/hydra.h
#   scripts/gen-ffi-header.sh --check  fail if it would change (for CI)
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out="$root/include/hydra.h"
check=0
[ "${1:-}" = "--check" ] && check=1

if ! command -v cbindgen >/dev/null 2>&1; then
    echo "error: cbindgen not found. Install it with:" >&2
    echo "    cargo install cbindgen --locked" >&2
    exit 1
fi

tmp="$(mktemp -t hydra-header.XXXXXX)"
trap 'rm -f "$tmp" "$tmp.post"' EXIT

cbindgen --quiet \
    --config "$root/crates/hydra-ffi/cbindgen.toml" \
    --crate hya-ffi \
    --output "$tmp"

# Rustdoc intra-doc links are noise in a C header: `[`hydra_job_start`]` is
# something a Rust reader clicks and a C reader trips over. They are stripped
# here rather than avoided in the source, so the Rust API documentation keeps
# its links and the header keeps its readability. Deterministic, so --check
# stays meaningful.
python3 - "$tmp" "$tmp.post" "$root/crates/hydra-ffi/Cargo.toml" <<'PY'
import os, re, sys
src, dst, manifest = sys.argv[1], sys.argv[2], sys.argv[3]
text = open(src, encoding="utf-8").read()
text = re.sub(r"\[`([^`]+)`\]", r"\1", text)
text = text.replace("crate::exports::", "").replace("crate::", "")

# cbindgen cannot emit a string constant (it refuses `Lit::Str`), so the one
# string version macro is injected here, read from Cargo.toml. That keeps the
# header's value tied to the crate's rather than to a literal someone has to
# remember to update.
version = None
for line in open(manifest, encoding="utf-8"):
    m = re.match(r'^version\s*=\s*"([^"]+)"', line)
    if m:
        version = m.group(1)
        break
if version is None:
    sys.exit("error: no version in " + manifest)

anchor = "#define HYDRA_FFI_VERSION_PATCH"
if anchor not in text:
    sys.exit("error: %r is missing from the generated header; the injection "
             "point for HYDRA_FFI_VERSION has moved" % anchor)
line = [l for l in text.split("\n") if l.startswith(anchor)][0]
text = text.replace(
    line,
    line
    + "\n\n/**\n * The library version this header was generated from.\n"
      " *\n"
      " * The LIBRARY version, not the ABI version: it moves on every release,\n"
      " * including ones that change nothing a binding can observe. Compare\n"
      " * HYDRA_FFI_ABI_VERSION to decide whether a header and a library are\n"
      " * compatible; use this to report what you linked against.\n"
      " *\n"
      " * hydra_ffi_version_string() returns the value compiled into the library.\n"
      " * If the two disagree, this header is not the one that library was built\n"
      " * from.\n"
      " */\n"
      '#define HYDRA_FFI_VERSION "%s"' % version,
    1,
)

# The layout assertions go at the FOOT of the header, inside the include guard:
# they need every type declared, and cbindgen has no hook that lands there.
# They belong in the header rather than only in our own test program, because
# then every consumer's compiler checks them — a padding rule or an enum width
# that differs on somebody else's toolchain is caught at their build rather
# than at their customer's run time.
layout = open(os.path.join(os.path.dirname(manifest), "abi-layout.h"),
              encoding="utf-8").read()
tail = "#ifdef __cplusplus\n}  // extern \"C\"\n#endif  // __cplusplus"
if tail not in text:
    sys.exit("error: the closing extern \"C\" block was not found; the "
             "injection point for the layout assertions has moved")
text = text.replace(tail, layout.rstrip("\n") + "\n\n" + tail, 1)

open(dst, "w", encoding="utf-8").write(text)
PY

if [ "$check" = 1 ]; then
    if ! diff -u "$out" "$tmp.post"; then
        echo >&2
        echo "error: include/hydra.h is out of date with crates/hydra-ffi." >&2
        echo "       Run scripts/gen-ffi-header.sh and commit the result." >&2
        exit 1
    fi
    echo "include/hydra.h is up to date"
else
    mkdir -p "$root/include"
    mv "$tmp.post" "$out"
    echo "wrote $out ($(wc -l < "$out" | tr -d ' ') lines)"
fi
