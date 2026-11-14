#!/usr/bin/env bash
# Prints the plugin scanner that the dstudio-engine build script built under one Cargo
# profile directory (the newest, if there are several).
#
#   tools/release/find-scanner.sh target/release
#   tools/release/find-scanner.sh target/aarch64-apple-darwin/release
#
# The scanner sits in the build script's OUT_DIR, whose layout depends on the Cargo version:
#   build/dstudio-engine-<hash>/out/build/      (Cargo 1.99 and earlier)
#   build/dstudio-engine/<hash>/out/build/      (Cargo 1.100 and later)
# Both are searched. Fails with a message when there is none.
set -euo pipefail
profile_dir=${1:?usage: find-scanner.sh <cargo profile dir, e.g. target/release>}
name=dstudio_plugin_scanner
found=$(find "$profile_dir/build" -type f \( -name "$name" -o -name "$name.exe" \) \
    \( -path '*/build/dstudio-engine-*/out/build/*' -o -path '*/build/dstudio-engine/*/out/build/*' \) \
    -exec ls -t {} + 2>/dev/null | head -1 || true)
if [ -z "$found" ]; then
    echo "error: no $name under $profile_dir/build (looked in dstudio-engine-*/out/build and dstudio-engine/*/out/build)" >&2
    exit 1
fi
printf '%s\n' "$found"
