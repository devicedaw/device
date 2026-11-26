---
name: dstudio-ui
description: Work on Device's interface - Slint components in crates/dstudio-app/ui, the controller and panels in crates/dstudio-app/src, wgpu surfaces, design tokens, review states rendered with --state/--screenshot, and the app self-test. Use for any visible change or UI behaviour change.
---

# The app and its interface

The app (`crates/dstudio-app`, binary `device-studio`) is a Slint shell with custom wgpu surfaces
for the timeline, waveforms and meters (ADR `docs/adr/0001-ui-rendering.md`). It holds no
authoritative state: it renders from the `Document` and the engine's readouts, and every edit goes
through `Controller::execute` with a `Command` (`dstudio-command`).

## Where things are

| What | Where |
|---|---|
| Slint components | `crates/dstudio-app/ui/*.slint` (`app.slint` is the root, `widgets.slint` the shared controls) |
| Colours, sizes, type | `crates/dstudio-app/ui/tokens.slint`; use tokens, never literal colours |
| State and edits | `crates/dstudio-app/src/controller.rs` |
| Panels | one Rust module per area in `crates/dstudio-app/src/` (`timeline.rs`, `mixer.rs`, `pianoroll.rs`, `device_panel.rs`, ...) |
| GPU drawing | `crates/dstudio-app/src/gpu/` |
| Command-line options and review states | `crates/dstudio-app/src/main.rs` (run with `--help`) |
| Self-test | `crates/dstudio-app/src/selftest.rs` |

Built-in device cards have their own rules: `dstudio-builtin-device` and `docs/devices.md`.

## Gestures

- A drag sends live values straight to the engine (or the view) and commits **one** command on
  release, so undo takes the whole gesture back. Escape during a gesture restores the values from
  before it.
- Right-click menus, keyboard shortcuts and menu-bar items reach the same commands as the mouse.
- The UI thread never blocks: file I/O, decoding and analysis run on workers and send results back.

## Review states and screenshots

`--state <name>` opens the app in a prepared state; `--screenshot <png>` saves a frame, and
`--offscreen` renders in a hidden window so it doesn't take over the screen.
`--exit-after-frames N` quits after N frames. `reference-<device-id>` shows any built-in device at
its defaults; the other states are listed in the `--state` match in `main.rs`.

When a change is visible, add or reuse a state that shows it, so before and after can be rendered
identically on `origin/main` and on the branch:

```sh
cargo run --release -p dstudio-app -- --no-audio --state <state> --offscreen --screenshot /tmp/after.png --exit-after-frames 5
```

Same state, same window size, same zoom for both shots. CI renders on Linux (Xvfb with lavapipe)
and Windows (WARP), so a state must not depend on a real audio device or the local library.

## Self-test

`--self-test --no-audio` drives the real interface with synthetic pointer and keyboard events
through Slint's input path and checks the resulting `Document` and engine state. It prints
`self-test: P / N checks passed`; CI requires P = N on every platform. Click targets come from live
anchors the panels report, not fixed coordinates, because window sizes differ between machines and
CI.

A feature with a UI flow gets a short self-test section: add a `<area>_steps()` function next to the
others and list it in `SECTIONS` (selftest.rs) where it runs, with the part of its neighbours. CI runs
the Windows self-test in parts (`--self-test-part 1..3`: the shared setup, then one contiguous run of
`SECTIONS`), so a section that starts a part sets up the state it needs itself. A step that holds a
button across timer ticks can lose the press to the system on a slow machine; press and move in one
step and record what the checks need. Check results in the `Document`, not in
pixels. A UI bug fix gets a step that would have failed before the fix.

## Performance

Frame time matters (the timeline and meters redraw every frame). `--frame-stats` prints frame
timings, `--render-bench N` renders N frames offscreen, and `--drag-bench` measures drags. Use them
before and after a change to anything that draws every frame (`dstudio-bench`).

## Docs

A visible change to what the README screenshots show means re-rendering the artwork in `docs/assets/art/`
(`docs/art/README.md` has the states and the commands).
