#!/usr/bin/env bash
# Packages Device for Linux: a .tar.gz with the app, the plugin scanner (the app looks
# for it next to itself), a desktop entry and an install script.
#
# Usage: tools/release/package-linux.sh <version> <arch> <out dir>
#   after: cargo build --release --locked -p dstudio-app   (target dir: $CARGO_TARGET_DIR or target/)
# Runtime needs: a Vulkan or OpenGL driver and X11 or Wayland. Audio uses PipeWire when its
# server runs and ALSA (part of every Linux system) otherwise; both libraries are loaded at run
# time, so the package works without PipeWire.
set -euo pipefail
version="${1:?usage: package-linux.sh <version> <arch> <out dir>}"
arch="${2:?usage: package-linux.sh <version> <arch> <out dir>}"
out="${3:?usage: package-linux.sh <version> <arch> <out dir>}"
root="$(cd "$(dirname "$0")/../.." && pwd)"
release="${CARGO_TARGET_DIR:-$root/target}/release"
name="Device-$version-linux-$arch"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
stage="$work/$name"
mkdir -p "$stage/bin" "$stage/share/applications" "$stage/share/icons/hicolor/256x256/apps" "$out"

install -m 755 "$release/device-studio" "$stage/bin/device-studio"
# The audio libraries must be loaded at run time, never linked: a hard link to
# libpipewire would stop the app from starting on systems without PipeWire.
if readelf -d "$stage/bin/device-studio" | grep -E 'NEEDED.*(libpipewire|libasound)'; then
  echo "device-studio links an audio library directly; it must be loaded with dlopen" >&2
  exit 1
fi
scanner="$(bash "$root/tools/release/find-scanner.sh" "$release")"
install -m 755 "$scanner" "$stage/bin/dstudio_plugin_scanner"
strip --strip-debug "$stage/bin/device-studio" "$stage/bin/dstudio_plugin_scanner"
install -m 644 "$root/crates/dstudio-app/icons/device-256.png" "$stage/share/icons/hicolor/256x256/apps/device-studio.png"
for doc in LICENSE README.md; do
  [ -f "$root/$doc" ] && install -m 644 "$root/$doc" "$stage/$doc"
done

cat > "$stage/share/applications/device-studio.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Device
Comment=Make music: arrange, record and mix
Exec=device-studio
Icon=device-studio
Terminal=false
Categories=AudioVideo;Audio;Midi;Music;
DESKTOP

cat > "$stage/install.sh" <<'INSTALL'
#!/usr/bin/env sh
# Installs Device for the current user (default prefix ~/.local) or, as root, for
# everyone (PREFIX=/usr/local). Usage: ./install.sh [prefix]
set -eu
here="$(cd "$(dirname "$0")" && pwd)"
if [ "$(id -u)" = 0 ]; then default=/usr/local; else default="$HOME/.local"; fi
prefix="${1:-${PREFIX:-$default}}"
mkdir -p "$prefix/lib/device-studio" "$prefix/bin" "$prefix/share/applications" "$prefix/share/icons/hicolor/256x256/apps"
cp "$here/bin/device-studio" "$here/bin/dstudio_plugin_scanner" "$prefix/lib/device-studio/"
ln -sf "$prefix/lib/device-studio/device-studio" "$prefix/bin/device-studio"
cp "$here/share/icons/hicolor/256x256/apps/device-studio.png" "$prefix/share/icons/hicolor/256x256/apps/"
sed "s#^Exec=.*#Exec=$prefix/lib/device-studio/device-studio#" \
  "$here/share/applications/device-studio.desktop" > "$prefix/share/applications/device-studio.desktop"
echo "Installed to $prefix (run: device-studio)"
INSTALL
chmod 755 "$stage/install.sh"

tar -C "$work" -czf "$out/$name.tar.gz" "$name"
echo "$out/$name.tar.gz"
