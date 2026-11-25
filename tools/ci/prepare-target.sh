#!/usr/bin/env bash
# Prepares the cargo target dir restored from the CI cache for this job's build (CI only):
#
#   tools/ci/prepare-target.sh debug|release
#
# $CARGO_TARGET_DIR must be set. What it does, and why:
#
# 1. Keeps only this job's profile. The tests job builds debug, the app job release; a target
#    dir restored from the older shared cache holds both, and saving the other profile again
#    would only grow the cache.
#
# 2. Makes every tracked source newer than the restored build. Cargo (path crates, build
#    scripts' rerun-if-changed) and Ninja decide what to rebuild by modification time.
#    Checkout writes every file before the restore, so the sources are normally newer anyway,
#    but a main run that saves its cache between this job's checkout and its restore would
#    make its outputs look newer than the checkout, and cargo would reuse them for different
#    sources. Touching after the restore rules that out: every workspace crate and the
#    engine are always rebuilt from this commit's sources (C++ objects come back from ccache
#    when their preprocessed source, flags and compiler match).
#
# 3. Removes CMake caches that were made in another checkout folder (a renamed repository
#    moves the workspace), which CMake refuses to reuse. Not on Windows, whose paths CMake
#    spells differently from Git Bash.
#
# 4. Zeroes the ccache statistics (they are stored in the restored cache), so the job's
#    `ccache --show-stats` counts this run's compiles only.
#
# 5. Removes the engine build script's output (`cargo clean -p dstudio-engine`), so its CMake
#    build tree is configured from scratch. The rebuild itself does not depend on this (step 2
#    already reruns build.rs), but a restored CMakeCache.txt pins the compiler's full path,
#    which a runner image update (a new Xcode or MSVC version) removes, and keeps cache
#    variables that build.rs no longer passes. The fresh configure costs about 15 to 30
#    seconds; the compiles are ccache hits. The speech recogniser's build output
#    (whisper-rs-sys) goes too: its build script does not rerun when the GGML_ settings in
#    .cargo/config.toml change, so a restored build would keep the old ones.
set -euo pipefail

profile=${1:?usage: prepare-target.sh debug|release}
target=${CARGO_TARGET_DIR:?CARGO_TARGET_DIR is not set}
case "$profile" in
    debug) other=release ;;
    release) other=debug ;;
    *) echo "unknown profile $profile" >&2; exit 2 ;;
esac
mkdir -p "$target"

# 1.
if [ -d "$target/$other" ]; then
    echo "Removing the restored $other build ($target/$other)"
    rm -rf "${target:?}/$other"
fi

# 2.
git ls-files -z | xargs -0 touch --
echo "Touched $(git ls-files | wc -l | tr -d ' ') tracked files"

# 3.
if [ "${RUNNER_OS:-}" != "Windows" ]; then
    resolved_target=$(cd "$target" && pwd -P)
    dirs=("$resolved_target")
    if [ -n "${DSTUDIO_DEPS_DIR:-}" ] && [ -d "$DSTUDIO_DEPS_DIR" ]; then
        dirs+=("$(cd "$DSTUDIO_DEPS_DIR" && pwd -P)")
    fi
    while IFS= read -r cache; do
        [ -f "$cache" ] || continue
        here=$(cd "$(dirname "$cache")" && pwd -P)
        made=$(sed -n 's/^CMAKE_CACHEFILE_DIR:INTERNAL=//p' "$cache")
        if [ -d "$made" ]; then made=$(cd "$made" && pwd -P); fi
        [ "$made" = "$here" ] && continue
        case "$here" in
            "$resolved_target"/dstudio-third-party/*) stale="$resolved_target/dstudio-third-party" ;;
            "${dirs[1]:-/nonexistent}"/*) stale="${dirs[1]}" ;;
            *) stale="$here" ;;
        esac
        echo "Removing $stale (its CMakeCache.txt was made in $made)"
        rm -rf "$stale"
    done < <(find "${dirs[@]}" -name CMakeCache.txt 2>/dev/null)
fi

# 4.
if command -v ccache > /dev/null; then
    ccache --zero-stats > /dev/null
fi

# 5. (No empty-array expansion: macOS runs bash 3.2, where that fails under `set -u`.)
if [ "$profile" = release ]; then
    cargo clean -p dstudio-engine -p whisper-rs-sys --locked --release
else
    cargo clean -p dstudio-engine -p whisper-rs-sys --locked
fi
