#!/usr/bin/env bash
# Wraps the release build in "Device.app", so macOS names the app menu, About, Hide and
# Quit "Device" instead of the executable's name.
# Usage: tools/macos-bundle.sh [output dir]   (default: $CARGO_TARGET_DIR/release or target/release)
# DSTUDIO_APP_EXE and DSTUDIO_SCANNER_EXE override the executables to bundle (the release
# workflow passes universal binaries). The plugin scanner goes next to the app executable in
# Contents/MacOS, where the app looks for it first; without one, plugin scanning only works on
# the machine that built the app.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
release="${CARGO_TARGET_DIR:-$root/target}/release"
out="${1:-$release}"
exe="${DSTUDIO_APP_EXE:-$release/device-studio}"
[ -x "$exe" ] || { echo "no $exe: run cargo build --release -p dstudio-app first" >&2; exit 1; }
scanner="${DSTUDIO_SCANNER_EXE:-}"
if [ -z "$scanner" ]; then
  # build.rs builds the scanner in the engine's CMake tree under target/.../build.
  scanner="$(bash "$(dirname "$0")/release/find-scanner.sh" "$(dirname "$exe")" 2>/dev/null || true)"
fi
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)"
app="$out/Device.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
cp "$exe" "$app/Contents/MacOS/device-studio"
if [ -n "$scanner" ] && [ -x "$scanner" ]; then
  cp "$scanner" "$app/Contents/MacOS/dstudio_plugin_scanner"
else
  echo "warning: no plugin scanner found; the bundle will not scan plugins on other machines" >&2
fi
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Device</string>
    <key>CFBundleDisplayName</key><string>Device</string>
    <key>CFBundleIdentifier</key><string>com.dstudio.studio</string>
    <key>CFBundleExecutable</key><string>device-studio</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$version</string>
    <key>CFBundleVersion</key><string>$version</string>
    <key>LSMinimumSystemVersion</key><string>13.0</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSMicrophoneUsageDescription</key><string>Device records from your audio inputs.</string>
</dict>
</plist>
PLIST
echo "$app"
