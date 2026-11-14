#!/usr/bin/env bash
# Tests tools/release/find-scanner.sh against both Cargo build-directory layouts.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
fail=0
expect() { # name, expected path ("" = failure), profile dir
    local got
    got=$(bash "$here/find-scanner.sh" "$3" 2>/dev/null) || got=""
    if [ "$got" != "$2" ]; then
        echo "FAIL $1: got [$got], want [$2]"
        fail=1
    fi
}
put() { mkdir -p "$(dirname "$1")" && : > "$1"; }

old="$tmp/old/release/build/dstudio-engine-0123abcd/out/build/dstudio_plugin_scanner"
put "$old"
expect "Cargo 1.99 layout" "$old" "$tmp/old/release"

new="$tmp/new/release/build/dstudio-engine/0123abcd/out/build/dstudio_plugin_scanner"
put "$new"
expect "Cargo 1.100 layout" "$new" "$tmp/new/release"

win="$tmp/win/release/build/dstudio-engine/0123abcd/out/build/dstudio_plugin_scanner.exe"
put "$win"
expect "Windows name" "$win" "$tmp/win/release"

put "$tmp/two/release/build/dstudio-engine/aaaa/out/build/dstudio_plugin_scanner"
touch -d '2026-01-01' "$tmp/two/release/build/dstudio-engine/aaaa/out/build/dstudio_plugin_scanner"
newest="$tmp/two/release/build/dstudio-engine/bbbb/out/build/dstudio_plugin_scanner"
put "$newest"
expect "newest of two" "$newest" "$tmp/two/release"

put "$tmp/other/release/build/some-other-crate/0123/out/build/dstudio_plugin_scanner"
expect "another crate's file" "" "$tmp/other/release"
expect "no build dir" "" "$tmp/missing/release"

[ "$fail" = 0 ] && echo "find-scanner: all checks passed"
exit "$fail"
