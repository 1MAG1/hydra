#!/usr/bin/env bash
# Copyright (C) 2026 Javad Rajabzadeh
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Package the Firefox add-on as an .xpi.
#
#   scripts/build-firefox-xpi.sh [--sign]
#
# Kept as the Firefox-only entry point; the work happens in
# scripts/build-extensions.sh, which builds the same .xpi alongside the
# Chromium .zip and writes the install instructions. Use that one to build
# both, or to pack into a packaging staging directory (--out).
#
# --sign submits to addons.mozilla.org via web-ext and produces a SIGNED
# .xpi that installs permanently in release Firefox. It needs credentials
# from https://addons.mozilla.org/developers/addon/api/key/ in the
# environment:
#     export WEB_EXT_API_KEY=user:########:###
#     export WEB_EXT_API_SECRET=...
#
# Without signing the .xpi still installs, but only as a temporary add-on
# (about:debugging), or permanently in Developer Edition / Nightly / ESR with
# xpinstall.signatures.required=false — see the printed instructions.

set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
exec "$REPO/scripts/build-extensions.sh" firefox "$@"
