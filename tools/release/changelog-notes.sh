#!/usr/bin/env bash
# Prints the CHANGELOG.md section for one version, for the release notes.
#
#   tools/release/changelog-notes.sh 0.2.0 [CHANGELOG.md]
#
# The section is everything between its heading `## [0.2.0]` (a date may follow) and the next
# `## ` heading, with leading and trailing blank lines trimmed. Fails when the version has no
# section or the section is empty, so a release can't go out without notes.
# Test it with tools/release/test-changelog-notes.sh.
set -euo pipefail

version=${1:?usage: changelog-notes.sh VERSION [CHANGELOG.md]}
file=${2:-CHANGELOG.md}
[ -f "$file" ] || { echo "error: $file not found" >&2; exit 1; }

notes=$(awk -v v="$version" '
    /^## / {
        if (found) exit
        heading = $0
        sub(/^## +/, "", heading)
        if (index(heading, "[" v "]") == 1) { found = 1; next }
    }
    found { print }
' "$file" | sed -e '/./,$!d' | sed -e ':a' -e '/^\n*$/{$d;N;ba' -e '}')

if [ -z "${notes//[[:space:]]/}" ]; then
    echo "error: $file has no notes for $version (add a '## [$version] - YYYY-MM-DD' section)" >&2
    exit 1
fi
printf '%s\n' "$notes"
