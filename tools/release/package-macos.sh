#!/usr/bin/env bash
# Builds the universal (Apple silicon + Intel) "Device.app" for macOS, unsigned apart
# from the linker's ad hoc signature. tools/release/make-dmg.sh signs it and wraps it in a .dmg.
#
# Usage: tools/release/package-macos.sh <out dir>   (writes "<out dir>/Device.app")
#   Expects release builds of dstudio-app for both architectures:
#     cargo build --release --locked -p dstudio-app --target aarch64-apple-darwin
#     cargo build --release --locked -p dstudio-app --target x86_64-apple-darwin
#   (target dir: $CARGO_TARGET_DIR, default target/).
#   Or, with the two builds made elsewhere (the release workflow builds each architecture on
#   its own runner), set DSTUDIO_MACOS_ARM_DIR and DSTUDIO_MACOS_X86_DIR to folders that each
#   hold that architecture's device-studio and dstudio_plugin_scanner.
set -euo pipefail
out="${1:?usage: package-macos.sh <out dir>}"
root="$(cd "$(dirname "$0")/../.." && pwd)"
target="${CARGO_TARGET_DIR:-$root/target}"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$out"

scanner_of() {  # the scanner build.rs built for one target triple
  bash "$root/tools/release/find-scanner.sh" "$target/$1/release" 2>/dev/null || true
}
if [ -n "${DSTUDIO_MACOS_ARM_DIR:-}" ] || [ -n "${DSTUDIO_MACOS_X86_DIR:-}" ]; then
  arm_app="${DSTUDIO_MACOS_ARM_DIR:?set both DSTUDIO_MACOS_ARM_DIR and DSTUDIO_MACOS_X86_DIR}/device-studio"
  x86_app="${DSTUDIO_MACOS_X86_DIR:?set both DSTUDIO_MACOS_ARM_DIR and DSTUDIO_MACOS_X86_DIR}/device-studio"
  arm_scanner="$DSTUDIO_MACOS_ARM_DIR/dstudio_plugin_scanner"
  x86_scanner="$DSTUDIO_MACOS_X86_DIR/dstudio_plugin_scanner"
  [ -f "$arm_scanner" ] || arm_scanner=""
  [ -f "$x86_scanner" ] || x86_scanner=""
else
  arm_app="$target/aarch64-apple-darwin/release/device-studio"
  x86_app="$target/x86_64-apple-darwin/release/device-studio"
  arm_scanner="$(scanner_of aarch64-apple-darwin)"
  x86_scanner="$(scanner_of x86_64-apple-darwin)"
fi
if [ -z "$arm_scanner" ] || [ -z "$x86_scanner" ]; then
  echo "plugin scanner not built for both architectures" >&2
  exit 1
fi

lipo -create -output "$work/device-studio" "$arm_app" "$x86_app"
lipo -create -output "$work/dstudio_plugin_scanner" "$arm_scanner" "$x86_scanner"
chmod +x "$work/device-studio" "$work/dstudio_plugin_scanner"
lipo -info "$work/device-studio" "$work/dstudio_plugin_scanner"

rm -rf "$out/Device.app"
DSTUDIO_APP_EXE="$work/device-studio" DSTUDIO_SCANNER_EXE="$work/dstudio_plugin_scanner" \
  bash "$root/tools/macos-bundle.sh" "$out"
