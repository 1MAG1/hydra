#!/usr/bin/env bash
# Copyright (C) 2026 Javad Rajabzadeh
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Package the Firefox add-on as an .xpi.
#
#   scripts/build-firefox-xpi.sh [--sign]
#
# An .xpi is just a zip with manifest.json at its ROOT (not inside a
# wrapper folder — Firefox rejects that). This syncs the shared sources
# from extensions/chrome, then zips extensions/firefox.
#
# --sign submits to addons.mozilla.org via web-ext and produces a SIGNED
# .xpi that installs permanently in release Firefox. It needs credentials
# from https://addons.mozilla.org/developers/addon/api/key/ in the
# environment:
#     export WEB_EXT_API_KEY=user:########:###
#     export WEB_EXT_API_SECRET=...
#
# Without signing the .xpi still works, but only via
# about:debugging -> Load Temporary Add-on (cleared on restart), or in
# Developer Edition / Nightly / ESR with xpinstall.signatures.required=false.

set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$REPO/extensions/firefox"
OUT="$REPO/target"
SIGN=0
[ "${1:-}" = "--sign" ] && SIGN=1

"$REPO/scripts/sync-extension-resources.sh" firefox

VERSION=$(python3 -c "import json;print(json.load(open('$SRC/manifest.json'))['version'])")
XPI="$OUT/hydra-firefox-$VERSION.xpi"
mkdir -p "$OUT"
rm -f "$XPI"

# -FS keeps the archive in sync, -x drops macOS cruft that trips validation.
( cd "$SRC" && zip -q -r -FS "$XPI" . \
    -x '.*' -x '__MACOSX/*' -x '*.DS_Store' -x 'README.md' )

# `unzip -Z1` prints bare entry names, so these are exact comparisons rather
# than pattern-matching against the formatted table.
ENTRIES=$(unzip -Z1 "$XPI")
for required in manifest.json background.js content.js popup.html popup.js icons/icon48.png; do
  # Whole-line match against the entry list. manifest.json in particular
  # must be a first-level entry or Firefox rejects the file outright.
  case $'\n'"$ENTRIES"$'\n' in
    *$'\n'"$required"$'\n'*) ;;
    *)
      echo "error: '$required' missing from the xpi root (did the sync step run?)" >&2
      exit 1
      ;;
  esac
done

echo "Built: ${XPI#"$REPO/"}  ($(du -h "$XPI" | cut -f1))"

if [ "$SIGN" = 1 ]; then
  if ! command -v web-ext >/dev/null 2>&1; then
    echo "error: web-ext not found — install it with: npm i -g web-ext" >&2
    exit 1
  fi
  : "${WEB_EXT_API_KEY:?set WEB_EXT_API_KEY (see the header of this script)}"
  : "${WEB_EXT_API_SECRET:?set WEB_EXT_API_SECRET}"
  # `unlisted` = self-distribution: signed for permanent install, not
  # published on addons.mozilla.org.
  web-ext sign --source-dir "$SRC" --artifacts-dir "$OUT" --channel unlisted
  echo "Signed .xpi written to ${OUT#"$REPO/"} — install it by dragging it into Firefox."
else
  echo
  echo "Install (unsigned): about:debugging#/runtime/this-firefox ->"
  echo "  Load Temporary Add-on... -> pick the .xpi above."
  echo "For a permanent install, re-run with --sign (needs AMO API keys)."
fi
