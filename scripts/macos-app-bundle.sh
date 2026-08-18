#!/bin/bash
# Builds "Hydra Download Manager.app" so macOS surfaces (Dock, Login Items,
# menu bar) show the product name instead of the binary name `hydra-gui`.
#
#   scripts/macos-app-bundle.sh [--install]
#
# --install also copies the result over /Applications/Hydra Download
# Manager.app. Without it the build only lands in target/release, and an
# installed copy keeps running the OLD code — which is invisible from the
# outside, because hydra-host launches the /Applications bundle and the
# single-instance guard makes any newly built binary hand over and exit.
set -euo pipefail
cd "$(dirname "$0")/.."

INSTALL=0
[ "${1:-}" = "--install" ] && INSTALL=1

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)

cargo build --release -p hya-gui -p hya-host -p hya-cli

# With CARGO_BUILD_TARGET set (CI cross-arch builds), cargo emits into
# target/<triple>/release instead of target/release.
BIN="target/${CARGO_BUILD_TARGET:+$CARGO_BUILD_TARGET/}release"
APP="$BIN/Hydra Download Manager.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN/hydra-gui" "$APP/Contents/MacOS/Hydra Download Manager"
# The CLI and the native-messaging host travel inside the bundle, so a
# drag-installed app is complete: browser manifests can point at
# Contents/MacOS/hydra-host, and the CLI can be symlinked onto PATH.
cp "$BIN/hydra-host" "$APP/Contents/MacOS/hydra-host"
cp "$BIN/hydra" "$APP/Contents/MacOS/hydra"

# App icon from docs/logo.png
ICONSET=$(mktemp -d)/hydra.iconset
mkdir -p "$ICONSET"
for size in 16 32 64 128 256 512; do
  sips -z $size $size docs/logo.png --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
  sips -z $((size*2)) $((size*2)) docs/logo.png --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/hydra.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Hydra Download Manager</string>
    <key>CFBundleDisplayName</key><string>Hydra Download Manager</string>
    <key>CFBundleExecutable</key><string>Hydra Download Manager</string>
    <key>CFBundleIdentifier</key><string>io.github.ja7ad.hydra</string>
    <key>CFBundleVersion</key><string>${VERSION}</string>
    <key>CFBundleShortVersionString</key><string>${VERSION}</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleIconFile</key><string>hydra</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>LSMinimumSystemVersion</key><string>11.0</string>
</dict>
</plist>
PLIST
# Ad-hoc signature: unsigned binaries get silently denied by TCC instead of
# prompting for Downloads-folder access.
codesign --force --deep -s - "$APP"
echo "Built: $APP"

INSTALLED="/Applications/Hydra Download Manager.app"
if [ "$INSTALL" = 1 ]; then
  if pgrep -f "$INSTALLED/Contents/MacOS" >/dev/null 2>&1; then
    echo "Quitting the running app so the replacement takes effect..."
    osascript -e 'quit app "Hydra Download Manager"' 2>/dev/null || true
    sleep 2
    pkill -f "$INSTALLED/Contents/MacOS" 2>/dev/null || true
  fi
  # ditto preserves the bundle structure and the ad-hoc signature.
  rm -rf "$INSTALLED"
  ditto "$APP" "$INSTALLED"
  echo "Installed: $INSTALLED"
  echo "Relaunch it (or let the browser extension start it on the next capture)."
elif [ -d "$INSTALLED" ]; then
  echo
  echo "NOTE: $INSTALLED exists and still holds the PREVIOUS build."
  echo "      That copy is what hydra-host launches. Re-run with --install"
  echo "      to update it, or the app you use keeps running old code."
fi
