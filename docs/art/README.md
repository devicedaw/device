# README and social artwork

The banner, the close-ups, the architecture panel and the social preview at the top of the
[README](../../README.md) are HTML pages rendered to PNG. Edit a page here, then render:

```sh
npm install --no-save playwright && npx playwright install chromium   # once
node tools/art/render.mjs              # every page, into docs/assets/art/
node tools/art/render.mjs hero         # one page
```

| Page | Output | Used in |
|---|---|---|
| `hero.html` | `hero.png` | top of the README |
| `feature-*.html` | `feature-*.png` | README › A closer look |
| `architecture.html` | `architecture.png` | README › Under the hood |
| `social-preview.html` | `social-preview.png` (1280×640) | Settings › General › Social preview, uploaded by hand |

`art.css` holds the shared look: colours from the app's `crates/dstudio-app/ui/tokens.slint` and the app's
bundled Inter font. `mark.svg` is the mark: one cycle of a sine wave on the app's blue.

## The app icon

`icon/app-icon-macos.svg` (the macOS icon grid: an 824 pt tile with a soft shadow on a 1024 pt canvas) and
`icon/app-icon.svg` (Windows and Linux: the mark with a small margin) are the app icon. Render them into the files
the app and its packages carry, in `crates/dstudio-app/icons/`:

```sh
node tools/art/make-icons.mjs   # device.icns, device.ico, device-256.png, device-macos-512.png
```

`tools/macos-bundle.sh` puts `device.icns` in Device.app, `crates/dstudio-app/build.rs` builds `device.ico` into
device-studio.exe (the installer uses it too), `tools/release/package-linux.sh` installs `device-256.png` for the
desktop entry, and the window uses it as its title bar and taskbar icon.

## The app views

`shots/` holds the app views the pages frame. They show a whole song, "Night Drive", rather than a
test state: `tools/art/demo_song.py` writes it (six sections, six tracks and two returns, every sound
synthesised by the script, the same output every run), and the app's `--showcase <view>` option puts
the opened project into a view. The shots are rendered by the app itself at 2x, with the Linux menu bar
(the top 88 pixels) cropped off:

```sh
python3 tools/art/demo_song.py "/tmp/Night Drive"   # the app titles the project after the folder
SLINT_SCALE_FACTOR=2 SLINT_WGPU_CPU=1 WGPU_BACKEND=vulkan xvfb-run -a -s "-screen 0 3200x2000x24" \
  target/release/device-studio --no-audio --zoom 85 --open "/tmp/Night Drive" --timeline-zoom 52 \
  --showcase arrange --screenshot /tmp/arrange.png --exit-after-frames 40
magick /tmp/arrange.png -gravity north -chop 0x88 docs/art/shots/arrange.png
```

| Cropped shot | `--showcase` |
|---|---|
| `docs/art/shots/arrange.png` | `arrange` (the whole song, the Lead's Synth below) |
| `docs/art/shots/analyzers.png` | `analyzers` (Pitch Corrector, Tuner and Spectrum on the vocal) |
| `docs/art/shots/lfo.png` | `lfo` (an LFO on the FX track's Auto Filter) |

The other views, `pianoroll`, `mixer`, `export` and `recording`, are the ones the website's tour uses.
`--zoom 85` makes the interface scale exactly 2, so the layout matches a 1440×820 window, and
`--timeline-zoom 52` fits the 36 bars. `--exit-after-frames 40` lets the start-up status messages
clear. When a view changes visibly, render its shot again, crop it into `docs/art/shots/` the same
way and re-run `render.mjs`.

## Dither material

Public artwork sits on `textures/dither.png`, an ordered-dither texture: a bloom toward the lower right with one faint
sine cycle (the mark) running through it, quantised to four colours from the app's tokens (ground, a deep blue, the
accent and the accent's light text tone) with an 8x8 Bayer matrix. `art.css` shows it anchored to the lower right with
each texel as a crisp 3 px square (`image-rendering: pixelated`), and more quietly behind the feature and architecture
panels. App screenshots in `shots/` are never dithered.

The textures are generated, with no dependencies:

```sh
node tools/art/dither.mjs   # docs/art/textures/dither.png and dither-site.png
node tools/art/render.mjs   # then re-render the artwork
```

`dither-site.png` is the website's copy (no sine, lower strength); copy it to `assets/dither.png` in
devicedaw.github.io when it changes.
