#!/usr/bin/env bash
# Tests tools/release/changelog-notes.sh against a small changelog.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

cat > "$tmp/CHANGELOG.md" <<'MD'
# Changelog

## [Unreleased]

- Not out yet.

## [0.2.0-alpha.1] - 2026-12-01

### Added

- Pre-release feature.

## [0.1.0] - 2026-11-20

- First release.

## [0.0.9]

MD

fail=0
check() { # name, expected output, version
    local got
    got=$(bash "$here/changelog-notes.sh" "$3" "$tmp/CHANGELOG.md" 2>/dev/null) || got="<failed>"
    if [ "$got" != "$2" ]; then
        echo "FAIL $1: got [$got]"
        fail=1
    fi
}
check "a dated section" "- First release." 0.1.0
check "a pre-release with subheadings" "### Added

- Pre-release feature." 0.2.0-alpha.1
check "no section" "<failed>" 0.3.0
check "an empty section" "<failed>" 0.0.9
check "a prefix of another version" "<failed>" 0.2.0
[ "$fail" = 0 ] && echo "changelog-notes: all checks passed"
exit "$fail"
