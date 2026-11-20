# PROGRESS: Device

Living handoff document for future agent sessions. Read this first, then `docs/architecture.md`.

**Keep the README current.** `README.md` is the public front page. Any change that adds, finishes or removes a
feature, device, platform or build step, or visibly changes the screens it shows, updates the README (and
`docs/building.md`, re-rendered `docs/assets/art/` artwork) in the same PR. The checklist is in README.md ›
Development › Keeping this README up to date.

## Current milestone

**Phase 1 (inspection + architecture) and the first slice of Phase 3 (functional audio).**

## Repository map

| Path | What | Owner role |
|---|---|---|
| `engine/` | C++23 real-time engine, C ABI `engine/include/dstudio_engine.h` | Audio engineering |
| `crates/dstudio-core` | Model, commands, undo/redo, persistence, snapshots | App architecture |
| `crates/dstudio-engine` | Safe Rust FFI wrapper + end-to-end tests | App architecture |
| `crates/dstudio-app` | Desktop app (Slint shell + wgpu surfaces) | UI engineering |
| `docs/` | Architecture spec, ADRs | Lead |

## How to build and test

```bash
export PATH=$HOME/.cargo/bin:$PATH
cmake --preset default -S engine && cmake --build engine/build && ctest --test-dir engine/build   # engine
cargo test --workspace                                                                          # core + FFI + e2e
```

Windows: from an x64 MSVC developer prompt with Ninja; Linux: needs `pkg-config` and
`libpipewire-0.3-dev` (plus the X11/Wayland/xkbcommon/fontconfig/udev dev packages for the app;
see `.github/workflows/ci.yml`). Platform status: `engine/README.md` › Platforms.

## Status log

### 2026-11-08: session 1

**Implemented and verified**
- Engine (C++23): 34/34 ctest (Release and ASan/UBSan), zero allocations on the audio path, Core Audio playback
  (manual tool), bench: 128 tracks × 4 clips = 12.4 µs per 256-frame block (≈430× real-time, one thread, M4).
- Core (Rust): model, validated invertible commands, undo/redo with coalescing, persistence, snapshots,
  assistant layer (Assist/Teach), context-aware keymap: 28 tests.
- FFI + end-to-end: 6 tests (ABI layout, multitrack summing with sample-accurate placement, mute/solo/gain/pan,
  seek/stop/trim, peaks, export).
- UI rendering decision: Slint 1.18.1 + wgpu 30.0.1, offscreen texture via `Image::try_from` (ADR 0001):
  ≈0.15 ms CPU/frame for 8,192 clips at a locked 60 fps.

- App (`crates/dstudio-app`, binary `device-studio`): app shell + wgpu timeline with real waveforms,
  wired to core/engine (transport, mixer controls, import, clip move/trim, undo/redo, save/open).
  `--self-test` 21/21;
  60 fps with ≈0.06 ms of our render CPU per frame.
  Run: `cargo run --release -p dstudio-app -- --demo`.

**Known gaps / next steps**
1. Note preview/audition, velocity lane, clip loop, synth bypass + modulation/macros/effects; SIMD unison;
   splitters, track rename/resize, native menu bar.
   Engine: input/recording, loop crossfade, tempo map for the metronome.
2. Engine: device selection, input/recording, worker-pool parallel track rendering, WASAPI/ASIO and PipeWire/JACK backends.
3. MIDI: model + piano roll + built-in instrument; plugin hosting (VST3/CLAP/AU) with an out-of-process scanner.
4. Assistant: provider adapter (outside core), assistant panel, highlight overlay.
5. Original branding and icons before any public release; the Söhne display face must not ship.

### 2026-11-08: session 1, round 2 (engine fixes + fonts)
- Engine ABI v1 additions (append-only): `se_meters_peek` (non-destructive ~50 ms window), `se_transport_set_loop`
  (sample-accurate wrap), `se_metronome_set` (+ `se_metronome_desc`); transport commands now apply immediately
  when no device runs (race-free ownership hand-off; see engine/README.md). 46/46 ctest in default and ASan;
  bench unchanged (128 tracks ≈12.4 µs/block).
- App: Inter 4.1 bundled (OFL, `crates/dstudio-app/ui/fonts/`), sizes ×0.91 after a sweep.
  Loop (⌘L, button, `SetLoop` undoable) and metronome wired; offline-seek workaround removed; meters use peek.
  `--self-test` 26/26 offline and with Core Audio.

### 2026-11-08: session 1, round 3 (mixer + assistant panels)
- Mixer (header toggle / ⇧⌘X): per-track strips with real fader, pan, stereo `peek_meters`, M/S, peak hold;
  Master strip; add-track column.
- Assistant panel (header toggle / ⇧⌘A): Assist/Teach over `dstudio_core::assistant`, one undo step per turn,
  highlight overlay (`src/highlight.rs`), Anthropic Messages API provider (`src/assistant_provider.rs`, key from
  `ANTHROPIC_API_KEY` only, model `DSTUDIO_ASSISTANT_MODEL`, default `claude-opus-5-5`). **No live API call has been
  exercised yet** (no key in the dev environment); the pipeline is covered by the labelled TEST provider
  `--assistant-fixture`.
- Self-test 42/42 offline and with Core Audio; workspace tests 49 pass.

### 2026-11-08: session 1, round 4 (MIDI + piano roll)
- Core: MIDI clips/notes, commands (AddMidiClip/Remove/Move/Resize/SetMidiNotes), seeded note transforms, note
  snapshots in engine frames. Engine ABI additions `se_session_publish_v2` (+ note/track v2 structs); original
  16-voice polyBLEP synth with sample-accurate events, republish-safe voices: 60/60 ctest (default + ASan),
  pitch within ±0.5 Hz. Bench: 32 MIDI tracks × 8 voices ≈ 396 µs per 256-frame block (unoptimised).
- App: MIDI clips on the timeline, piano roll (wgpu) with editing, toolbar transforms, footer, context menu.
  Self-test 60/60 offline and with Core Audio, incl. a check that notes drawn in the piano roll sound on the
  instrument track.
- Requested core API: RenameClip command, clip loop fields; engine: note audition call.

### 2026-11-08: session 1, round 5 (synth Devices panel)
- Core: `SynthParams` contract + 6 original factory presets, `Command::SetSynthParams`, `Document::seal_coalescing`.
- Engine: `se_session_publish_v3` with `se_synth_params`; every parameter drives the DSP (polyBLEP/BLAMP oscillators,
  shape morph/fold, cross-FM, sub/noise, TPT SVF LP/HP/BP, ADSRs, LFO, mono legato/glide, unison) with 10 ms
  smoothing; 76/76 ctest (default + ASan). Bench 32 tracks: default patch 573 µs/block, heavy patch 3489 µs (65 % of
  deadline).
- App: Synth device card, preset browser, 6 real pages, live displays derived from
  the engine formulas; self-test 76/76 offline and with Core Audio, incl. a rendered-audio check that a panel cutoff
  change reaches the engine.

### 2026-11-08: session 1, round 6 (recording + input)
- Core: Track arm/monitor/input + commands; ADR 0002.
- Engine: device enumeration/config (private aggregate device for separate mic/speaker clocks), per-track input
  routing + monitoring, disk recording via SPSC rings + writer thread, latency-compensated takes, input-injected
  offline render. 94/94 ctest (default + ASan). Input opens ONLY when a device is configured by name/default.
- App: R/I buttons, In popup, record transport, live recording clip, Settings › Audio panel with saved preferences
  (`~/Library/Application Support/Device Studio/preferences.json`). Self-test 99/99 both modes (recording checks via
  injected input). The session rate is fixed at 48 kHz; the Audio page says so when a different device rate is picked.
- Real microphone verified (2026-11-08, with the user's permission): MacBook Air mic via the private aggregate
  device, 3 s take, 0 overruns, real room signal. An acoustic loopback test (metronome → speakers → mic,
  `DSTUDIO_TEST_REAL_INPUT=1 cargo test -p dstudio-engine --test real_loopback -- --ignored --nocapture`) found the
  latency compensation 18.5 ms too early (an extra IO buffer per side); fixed, residual now +2.7..6.2 ms late.
  Next: per-device manual recording offset (calibration) in Settings › Audio.

### 2026-11-08: session 1, round 7 (plugin hosting)
- Core: device chains (`Track.devices`, `Track.instrument_plugin`), commands, base64 plugin state,
  `Document::sync_device_states`; ADR 0003.
- Engine: AU/VST3 (SDK v3.8.1, MIT)/CLAP (1.2.10, MIT) hosting, publish v5, out-of-process scanner with cache and
  blocklist, sample-accurate instrument plugins, state save/restore, native editor windows; own test plugins
  (VST3/CLAP gain, CLAP sine, crashing CLAP). 111/111 ctest (default + ASan). Scan on the dev Mac: 68 plugins.
- App: browser Devices tab from the real catalog, device cards (power, editor, remove, reorder), Add device
  menus, Inst selector, Settings › Plugins with rescan, plugin state synced on save. Self-test 120/120 both modes.
- Screenshot states only show Apple's built-in AUs; never commit names of the user's installed plugins.

### 2026-11-08: session 1, round 8 (plugin delay compensation)
- Engine (ADR 0004): per-track alignment delays, whole-mix delay L, bypass keeps timing, monitored input skips the
  compensation delay, metronome/export/recording compensated, runtime latency changes via plugin_idle, 2^17-frame
  cap, global toggle (off = bit-identical to before). 125/125 ctest (default + ASan); bench L=0 unchanged.
- App: playhead and Bar/Time readouts show the audible position; Settings › Audio has a compensation switch
  (preference `pdc_off`) and a plugin-latency readout. Self-test 125/125 both modes, incl. an offline check that a
  4800-frame plugin puts the readout 100 ms behind the transport.
- Not run: acoustic loopback with plugins (`acoustic_loopback_latency_with_pdc`, opens the mic).

### 2026-11-08: session 1, round 9 (automation)
- Core: automation lanes (Volume dB, Pan, 20 synth params, plugin params; master volume), curve contract,
  `SetAutomationLane`; ADR 0005.
- Engine: publish v6 with lanes; evaluation bit-identical to core (135k random samples); volume/pan per ≤32
  samples (per sample near silence), synth at 16-frame control rate, plugin params as sample-offset events
  (VST3/CLAP/AU); PDC-aligned per stage incl. loop wraps; editor gesture capture for all formats. 139/139 ctest
  (default + ASan). Bench: 32 tracks × 4 lanes ≈ 0.6 % of the deadline.
- App: lanes toggle, header automation row with power + parameter picker, gold overlay lanes with labels,
  point/curve/marquee/freehand editing, Automation Arm touch-write, controls follow automation. Self-test 146/146
  offline, 145/145 (+1 offline-only skip) with Core Audio.

### 2026-11-08: session 1, round 10 (Windows and Linux port, branch `port/windows-linux`)
- CI green on macos-15, windows-2025 (MSVC /W4 /WX) and ubuntu-24.04 (GCC -Werror): engine build,
  145 ctest (139 + 5 FrameRing + 1 Win32 editor test; AU and device tests skip off macOS with a
  reason, CTest "Skipped"), `cargo build/test --workspace` (120 tests) on all three.
- Engine: `platform/os.{hpp,cpp}` (UTF-8 paths/_wfopen, env, ChildProcess over
  CreateProcessW/posix_spawn), scanner ported (per-OS folders, NTSTATUS crash detection), VST3/CLAP
  loading via LoadLibraryExW/dlopen with the VST3 bundle layout, canonical cross-platform VST3 class
  id strings, test plugins built as Windows/Linux modules.
- Audio: WASAPI shared/event/float32 backend (separate capture client + drift-tolerant
  `FrameRing`, MMCSS) and PipeWire backend (playback/capture streams). **Neither has run on real
  audio hardware** (CI runners have none); ASIO is future work (licence).
- Editors: Windows HWND host and Linux X11 host (both tested in CI with `DStudioTestGui.clap`; Linux
  under Xvfb, with the host run loop for plugin fds and timers).
- App: builds on all three; macOS-only winit hook stays cfg-gated; per-OS plugin folders. Off macOS
  the self-test reports its Audio Unit devices section as skipped.
- macOS-visible changes: two hardware-/compiler-dependent tests relaxed (PDC golden hashes only on
  Apple clang/arm64; output latency plausibility instead of `>= 512`), ABI header comment fixed
  (no buffer term in latency), `on_platform_main_thread` unchanged on macOS.
- CI now runs on the self-hosted Mac mini only (as on main, 30a5ccb); the hosted Windows/Linux matrix
  was dropped. The Windows/Linux results above are from hosted run 42464468427 (`6cecd4f`); later
  changes are not compiled on those systems until a self-hosted Windows/Linux runner is added.

### 2026-11-08: Windows and Linux back in CI

- CI is a three-job matrix again: macOS stays on the self-hosted Mac mini, Windows and Linux run on
  GitHub-hosted `windows-latest` / `ubuntu-latest` (engine `-Werror`/`/W4 /WX`, ctest, Rust
  workspace build + tests, app self-test). All three pass.
- Fix found by the Linux job: an import into the "selected track" was resolved when the import
  finished, so when imports finished out of order (the demo's second, new-track import first) the
  file landed on the newly selected track. The target track is now resolved when the import starts.
- Windows app self-test: 125/125 pass; the process still exits 0x87A afterwards (step stays
  informational, as before).

### 2026-11-10: step sequencer, part 1: pattern clips in core and engine (SAIN-37)

- Core (`steps.rs`): a pattern clip is a MIDI clip with a step grid (`MidiClip.pattern`): rate 1/1 to 1/32T,
  8 to 64 steps, swing, and Velocity, Offset, Repeat and Tie per cell. Step edits bake the notes
  (`Command::SetStepPattern`); piano-roll edits read only the changed rows back into the grid; `None` converts
  the clip to plain MIDI (its repeats become notes). The snapshot repeats the pattern over the clip. Old project
  files load unchanged.
- Engine: patterns play as ordinary notes; a new ctest checks that dense repeats republished while playing
  don't allocate, and bench_render has a Pattern table. `cargo bench -p dstudio-core --bench steps` times bake,
  read-back and snapshot builds.

### 2026-11-10: minimap pointer hide and Devices minimap (SAIN-33)

- Header minimap drags (window and zoom grabs) hide the pointer and put it back on the grabbed spot on release
  (`src/pointer.rs`: NSCursor + winit warp; self-test records the requests).
- Devices minimap at the right of the Clip / Devices strip (`src/devminimap.rs`, `DevMinimap` in `ui/app.slint`):
  tiles with glyphs, chain frames, window, jump / drag / wheel, Esc restores. Not done: the hover name.

### 2026-11-10: grid size and snap, clip-editor rulers, minimap pictures and motion (SAIN-33)

- Timeline toolbar snap control (`src/snap.rs`): the readout toggles snapping, the chevron opens Off / Auto / 1 Bar ...
  1/32 / triplet sizes. Auto follows the ruler step for the zoom (two beats at the default zoom), a
  fixed size overrides it when finer (doubled to 6 px), Cmd at the press drags off the grid.
- Piano roll and audio clip editor rulers and grids use the same `RulerSteps` rules (bar labels by power-of-two
  steps, ticks, 16 / 8 px sub and beat lines).
- Devices minimap tiles show pictures we draw from each device's own layout (`Art`), and fade in (300 ms) and glide
  (spring) when the chain changes; the strip redraws only while they run. `--drag-bench devmm`
  benchmarks it; `--state review-snap-menu` shows the menu.

### 2026-11-10: step sequencer, gaps closed (SAIN-37)

- Paging: the grid shows 16 or 32 steps at a time (by width), with page tabs and a Follow
  playhead button at the right of the toolbar. A MIDI clip longer than 64 steps now converts with its own step
  count (up to `STEP_LIMIT`, 4096) instead of being refused; Length and Double still stop at 64.
- While the transport plays inside the clip, the playing step's column has a white ring and the cells it hits
  flash (230 ms, Steps lane).
- Toolbar tooltips (Double, Clear, the lane tabs, Follow playhead) with a 1 s hover delay; Clear is
  disabled when the pattern has no cells.
- Value lanes: empty steps dark, Velocity filled from the bottom on a lighter base.
### 2026-11-10: pattern library, Genre list and Shape tab (SAIN-37, PR B)

- Genre now lists Any and 21 genres. The ten new ones come after the old eleven, because saved
  clips store the index. Each new genre has its own feel (scale, grid, swing, density, octave, progressions) and
  its own drum grooves, including two new grooves, Dembow and Warehouse pulse. Drum swing now follows the
  genre's feel, which gives the same result as before for the old genres. The Genre menu shows 12 rows and
  scrolls to the active one.
- Shape tab (`dstudio_core::shape`, `composer.rs`): Octave, Voices, Density, Length, Velocity, Nudge, Repeat and
  Arp reshape what the clip plays. The unshaped notes stay in `Composer::base`. Each part, and the kept
  notes, keep their own Shape. A slider drag is one undo step ("Shape Change"). If you edit notes after they
  were shaped, they play as edited under a neutral Shape. The other rack rows show disabled with
  tips.
- Review states: `--state review-patterns-3` (Shape on a lead) and `review-patterns-4` (the Genre menu).
- Next: Shape option A, the full rack with drawn curves and the remaining transforms.

### 2026-11-10: Shape rack, full (SAIN-37, PR C)

- `dstudio_core::shape` now has all 16 controls: Octave, Voices, Density, Length, Velocity, Vary, Nudge, Repeat,
  Air, Sub, Spread, Tension, Inversion, Arp, and the arp's Pattern and Octaves. Each one has a base value and a
  drawn curve (one offset per beat). Shapes saved before this change load unchanged.
- `shape_rack.rs` holds the rack rows (click a label to choose the curve to edit). The piano roll toolbar has
  Shape (draw), the Pencil, Steps and Ramp brushes, and Reset. The curves are drawn into the piano roll's wgpu
  draw list. A stroke or a slider drag is one undo step.
- Review states: `review-patterns-5` (Velocity and Density curves) and `review-patterns-6` (Air, a Tension
  curve, and the arp).

### 2026-11-11: Clip renders through plugins (CHR-38)
- Consolidate's sibling renders (Bounce to new track, Export audio) now render tracks with VST3, AU and CLAP plugins
  and instrument plugins. The UI thread prepares the instances in the private engine, one per tick, from the live
  instances' current state; the worker renders; the engine is dropped on the UI thread. New engine entry points
  `se_plugin_prepare`, `se_plugin_save_state`, `se_plugin_set_detached` (engine/README.md "Plugins in offline renders").
- The render honours plugin delay compensation (renders L more frames, drops the first L) and the project tempo.
- Self-test section "clip render plugin", ctest `offline_*`, `tests/clip_render.rs`, `dstudio_offline_plugin_bench`.

### 2026-11-09: Renamed to Device Studio (user-facing)

- Window title, binary (`device-studio`), built-in device vendor, preferences folder
  (`Device Studio` / `device-studio`), recording temp folder, CLAP host name, Core Audio aggregate and
  PipeWire stream names, assistant prompt and README/docs now say Device Studio.
- Projects save as `Name.dsproj/` with format `device-studio-project`, by default in `~/Documents/Device Studio`.
  (Opening projects from before the rename and moving the old support and projects folders over were supported
  at first, then removed on 2026-11-11: open old projects in an earlier build and use Save As with a `.dsproj`
  name, and move any projects kept in the old projects folder into `~/Documents/Device Studio` or point
  `DSTUDIO_PROJECTS_DIR` at it.)
  The macOS bundle id is `com.devicestudio.app` (microphone permission is asked again once).
- Internal names use `dstudio` (`dstudio-*` crates, `dstudio::` namespace, `dstudio_engine.h`,
  `DSTUDIO_*` CMake options and env vars, `DStudioTest*` test plugins, `studio.dstudio.test.*` ids).
  Not `device`: it already names the effects/instruments concept. The GitHub repo was `chrislaupama/device`
  (transferred to `devicedaw/device` on 2026-11-12).

### 2026-11-10: Linux audio runs in CI; Linux plugin windows; Windows CI GPU memory

- Linux CI starts a headless PipeWire graph (`tools/ci/headless-pipewire.sh`: null sink + `pw-loopback`
  to a virtual source). The device tests now open real PipeWire streams instead of skipping, and the
  new opt-in `device_duplex_loopback_take_lands_on_the_clicks` records a click track back through the
  loopback: every click lands sample-exact on its timeline position. The app also runs
  `--self-test-audio` there (265/265).
- Fix found by the loopback test: when one PipeWire stream missed a cycle (an xrun), the duplex input
  ring kept the extra cycle forever, so every later take landed one quantum late. Both streams now
  share a `node.group` (one clock), and `FrameRing` re-syncs after a window of 32 cycles that is off
  by more than half a quantum (`dstudio_frame_ring_bench`: no measurable cost). WASAPI keeps the old
  policy (separate clocks; re-sync off).
- Linux plugin editor windows (X11): `plugin_editor_x11.cpp` plus a main-thread run loop
  (`run_loop.cpp`) dispatched from `se_plugin_idle`, serving VST3 `Linux::IRunLoop` and CLAP
  `posix-fd-support` / `timer-support`. `DStudioTestGui.clap` is cross-platform now; the Xvfb test checks
  size, timer and fd callbacks, close box, re-open and retire. Dragonfly Room (CLAP, VST3) and an LSP
  CLAP open and run by hand. The app's "Open device window" button is enabled on Linux.
- Windows CI self-test crashed out of GPU memory (WARP) partway through: the runner's window is
  occluded, so presenting never blocks and frames queued faster than WARP drew them. The app now keeps
  at most 2 frames in flight (`gpu::Renderer::end_frame`, after each Slint frame); the self-test
  prints how many frames had to wait. No cost when the GPU keeps up (0 waits on Linux, frame times
  unchanged).
- Still open: WASAPI has never run against an audio device (the hosted Windows runner has none);
  Linux editors pass no HiDPI scale to plugins.

### 2026-11-10: spec-driven workflow and agent skills

- `specs/` holds specs for non-trivial work (`specs/README.md`: write, plan, build, verify, with a stop for agreement
  after the spec and after the plan), templates in `specs/_template/`, and `specs/principles.md`. Spec 001 is the
  worked example.
- `AGENTS.md` is the entry point for coding agents; codebase skills live in `.agents/skills/` (commands, C ABI and
  audio thread, built-in devices, UI, benchmarks, pull requests, plus the spec phases and bug fixing).
  `python3 tools/agents/check_skills.py` fails when a skill names a file that no longer exists.

### 2026-11-11: return tracks and sends, part 1: core (spec 002)

- Spec `specs/002-return-tracks-and-sends/` (spec, plan, tasks). Return tracks are `TrackKind::Return` in
  `Project::tracks`, always after the ordinary tracks (at most 16); `Track::sends` (`TrackSend`: return, level 0..+6 dB,
  pre/post fader) and `Track::output` (a return, or the master). Commands `SetSends` (level drags coalesce per track)
  and `SetTrackOutput`; removing a return removes the sends to it and resets outputs to it in one undo step. Clips,
  input, arm, monitor and instrument plugins are refused on returns. Old projects load unchanged; damaged routing is
  repaired on load. The snapshot carries the routing; single-track renders stay dry.
- Not yet: the engine ignores the routing (returns publish as empty tracks) and the app has no UI for it. Next: engine
  (spec 002 T4-T8), then the UI (T9).

### 2026-11-11: transport bar tempo, time signature and key

- The transport bar's BPM, TAP, Key and Sig fields were display-only; they now edit the project
  (`src/transport_fields.rs`, `ui/transport.slint`). Tempo: drag right or up (1 BPM per point), double-click to type
  (two decimals kept, shown when not whole), or tap (the average of up to eight taps; a pause over 2 s or a sudden
  change of pace starts over). A drag or a tap series is one "Tempo Change" undo step whatever its length. Sig opens a
  menu of twelve common signatures; Key opens a picker (Major / Minor, twelve roots, No key) that sets
  `Command::SetProjectKey`, which the composer, Shape and key-following devices already read.
- A change of pace while tapping starts a new undo step. Keyboard: in the open Sig menu Up/Down and Enter or Space;
  in the open key picker the arrows move over the roots, Enter or Space picks, M switches Major / Minor, Delete or
  Backspace is No key (not while Settings or a dialog is open). Both openers and every choice have accessible default
  actions. Opening or replacing the project drops a tempo gesture in progress.
- Shift+T taps the tempo (`transport.tap_tempo`). The tempo field's accessible default action opens the typed editor
  and its increment / decrement step one BPM. A typed tempo takes an optional "bpm" suffix and refuses other text;
  typing or dragging the tempo ends a tap series.
- Self-test section `transport_fields_steps`; review states `review-transport-sig` and `review-transport-key`.

### 2026-11-11: return tracks and sends, part 2: engine (spec 002)

- `se_session_publish_v9` (v8 + `se_track_route`, `se_send_desc`, `SE_MAX_RETURNS` 16); `Engine::publish` in Rust
  now publishes v9. Each return owns a preallocated stereo bus; sends tap pre-fader (after the chain) or post-fader,
  outputs replace the master path. Order is a topological sort over routing and sidechain edges (only sidechain
  edges are cut on a cycle). Mute/solo: muted tracks send nothing, soloing tracks keeps returns playing, soloing a
  return plays only soloed returns.
- Delay compensation aligns every input of every bus (ADR 0004 note): main line to the earliest destination, edge
  lines for later ones. Without latent returns no edge line exists; sessions without routing render bit-identically.
- Tests: `engine/tests/test_routing.cpp` (9), `crates/dstudio-engine/tests/routing.rs` (2). Bench: "Returns" table in
  `dstudio_engine_bench`.
- Next: the UI (spec 002 T9): Returns pill and rows, return strips, Sends sections, Out menu.

### 2026-11-11: track rename

- Double-click a track's name in its header, in the Devices tab's inspector or in the Clip tab's title to rename it
  (`TrackName` in `ui/track_header.slint`, `Controller::rename_track`): Enter or clicking away commits, Escape
  cancels, surrounding spaces are dropped and an empty name keeps the old one. `Command::RenameTrack` already existed.
- The field commits to the track it opened on, also when the selection moves first, and a press in the lanes or a
  clip editor commits it (`Controller::press_epoch` moves the focus back to the shortcuts). The name's accessible
  default action opens the field, and ⌘R (`track.rename`, a track selected and no clip) opens the selected track's
  header field, scrolled into view (a row taller than the lanes is centred).
- The open field mirrors its text to `Controller::rename_draft`; when the bottom panel holding it goes away (tab
  switch, mixer, selection cleared) `push_models` commits the draft, since the field never sees its focus loss, and
  gives the keyboard back to the shortcuts. A field records its draft as soon as it opens, so an untouched one counts.
- Rename and draft calls carry the `doc_epoch` the field opened in (`Studio.doc-epoch`): when another project
  replaces the open one (Open, Live Set import) the field closes and a late commit from it renames nothing.
- Undo of a rename is `Command::RestoreTrackName`, which puts the old name back exactly (an imported name over the
  128-character limit included). It is never deserialized, so the assistant cannot send one.
- Self-test section `track_rename_steps` drives the inspector name with real double-clicks and keys.

### 2026-11-11: piano roll velocity lane

- The velocity button in the piano-roll toolbar opens a 68-unit lane under the note grid (the grid gives up that
  height). Each note has a stem from the lane floor to its velocity (1 to 127). Dragging a stem moves the velocity
  of that note, or of the whole selection when the note is selected; a press on empty lane paints the stems the
  pointer sweeps across (the selection only, when there is one). Each gesture is one "Note Velocity" undo step.
- Code: `pianoroll.rs` (`VEL_H`, `vel_y`/`vel_at`, `PrDrag::Velocity`, `pr_vel_pointer`, `paint_velocity`, the
  `vel_lane` draw). Review state `review-pr-velocity`; self-test section "velocity"; unit test
  `velocity_lane_maps_and_paints`.

### 2026-11-11: MIDI input, engine side (spec 003)

- Spec `specs/003-midi-input-and-recording/` (spec, plan, tasks). This first part is the engine and the C ABI; the
  app side (R/I on instrument tracks, Settings › MIDI inputs, recorded MIDI clips) follows.
- Engine: MIDI input backends (Core MIDI, Windows `midiIn*`, ALSA sequencer, stub) behind `MidiBackend`;
  `MidiInputHub` (per-port SPSC queues, normalisation, held-note release on close); routing in `render_block` to
  armed/monitoring instrument tracks, else the focus track; `LiveSynth` plays the built-in Synth live; capture ring
  with latency compensation and loop folding. `se_midi_*` in the header, `Engine::midi_inputs` and friends in Rust.
- Core: `SetTrackArm` / `SetTrackMonitor` accept instrument tracks.
- Tests: `engine/tests/test_midi_input.cpp`, `crates/dstudio-engine/tests/midi_input.rs`; bench: "MIDI input"
  table in `dstudio_engine_bench`.
- Not verified on hardware yet: Core MIDI with a real keyboard (Mac mini), WinMM (CI build only).

### 2026-11-11: MIDI recording, app side (spec 003)

- Core: `midi_record` turns a pass's captured note events into one `AddMidiClip` per armed instrument track (clip
  from the start bar to the bar after the last note or stop; looped passes cover the loop and overdub).
- App (`midi_in.rs`, `recording.rs`): R and I work on instrument tracks, and only armed or monitoring instrument
  tracks play the keyboard (selecting a track does not); record starts MIDI capture (MIDI-only passes start the
  transport themselves) and stop adds the clips to the pass's single "Record" undo step, after any audio takes; the
  live "Recording" clip draws the notes as they are played. Settings › MIDI lists the inputs (activity light, on/off switch, preference
  `midi.disabled_inputs`); ports are rescanned every 2 s (every 100 ms while that page shows). Self-test and
  screenshot runs never open real ports.
- Review states: `--state review-midi-recording`, `--state review-settings-midi` (fake input list).
- Self-test: instrument R/I steps and a recorded MIDI pass (inject, live clip, stop, clip and notes, undo).

### 2026-11-11: timeline shortcuts

- Default keymap actions that resolved but did nothing are wired: ⇧⌘T `track.add_instrument` (new Instrument track,
  selected), ⌘R `clip.reverse` (MIDI notes mirrored in the clip as one "Reverse Clip" step; with the clip's loop on,
  the part before the loop and the loop itself are each mirrored in place, so every note that played still plays; audio clips toggle
  `ClipWarp::reverse`; step patterns are left alone with a status message), ⌘L `transport.loop_selection` (loop exactly
  the selected clip or section, turned on; with nothing selected ⌘L still toggles the loop), ⌘D `section.duplicate`.
  The key context now reports `section_selected`, so the section bindings resolve at all.
- Still unwired: `tool.scissors` (T) and the range actions (`range.duplicate`, `clip.split_at_range`): there is no
  scissors tool or range selection yet.
- Self-test section "shortcuts" (real key presses); unit test `reversing_mirrors_notes_in_the_clip`.

### 2026-11-11: master mute and bottom panel collapse

- `Project::master_mute` (saved only when on) and `Command::SetMasterMute` ("Master Mute Change"). The snapshot sends a
  muted master as gain 0 with no master volume lane, so automation cannot bring it back; the fader keeps its value.
  The mixer's master M toggles it, and the timeline's master row dims and reads "Master (muted)".
- The collapse button in the bottom panel header (Clip/Devices and the mixer) shuts the panel to its tab row and reopens
  it at the height it had (`Controller::toggle_bottom_collapse`, src/splitters.rs). Like the splitter it is a view
  change saved with the project, not an undo step; a panel shut by dragging reopens at its default height.

### 2026-11-11: return tracks and sends, part 3: app (spec 002)

- `crates/dstudio-app/src/routing.rs`: the Returns pill (view state, saved as `hide_returns` in the preferences),
  the Add track menu (Audio, Instrument, Return track; the timeline and mixer buttons open it, Cmd+T still adds an
  audio track), the add-send menu (existing returns, then "New return track"), the Out menu, send edits.
- Timeline: return rows are pinned above the master row (`View::pinned_from` / `pinned_h`; the pinned band is cut
  off in a very short timeline, leaving half a track row of scrolling lanes). They have no R button and no expanded
  automation rows, and take no clips.
- Mixer: return strips after the track strips with a divider; the Sends chevron expands every strip's Sends section
  (one slider per send). Channel strip: the Sends tab (knob, Pre switch, remove), Out lists Master and the returns,
  In reads "Return" on a return track. `Studio.strip-tab` makes the strip tab reachable from review states.
- Send lists scroll with the wheel when they outgrow their room (the Sends tab, and the mixer's Sends section, which
  takes at most the room under the insert pill). A held send knob is one undo step however long it pauses
  (`Document::hold_coalescing`, sealed on release). Pinned return automation rows stop at the master row.
- Stems are dry: a stem pass clears every track's sends and output (`dialogs::stem_snapshot`), so a track routed
  into a return still renders. Return tracks are renamed like any track (double-click the name).
- Review states `review-returns` and `review-returns-strip`; self-test section `returns_steps`.

### 2026-11-11: export at 44.1 or 48 kHz

- The Export dialog's 44.1 kHz / 48 kHz switch now sets the rate of the Mixdown and Stems files; before, it showed a
  placeholder and every file had the engine's rate. The default is the rate nearest the engine's (48 kHz at 47 kHz
  and up). The size estimate follows the picked rate. Project exports stay at the engine rate, which the Live set
  records.
- `src/resample.rs`: a polyphase Kaiser-windowed sinc converter (128 taps, about 85 dB stopband, cutoff at 93 % of the
  lower Nyquist frequency) for rate pairs whose ratio reduces to at most 2048 output steps (44.1 / 48 kHz and their
  multiples). The render happens at the engine rate and is converted once per file.
- Tests: `resample::tests` (pitch and level both ways, 18 kHz passes, 23 kHz stopped going to 44.1 kHz, channels
  stay apart, odd ratios refused) and `the_export_rate_follows_the_engine_until_one_is_picked`. The self-test exports
  the mix again at the other rate and checks its rate, length in seconds and level against the first file. Review
  state `review-export-44k`.

### 2026-11-11: export the loop region

- The Export dialog's Range pill is now a menu: Full project (bar 1 to the end of the last clip, plus the 2 s tail, as
  before) or Loop region. The loop region renders from its start for exactly its length, with no tail, so the file
  loops seamlessly; it can be exported whether or not looping is on. The bars readout ("Bars 3–4") and the size
  estimate follow the range, and Stems use it too. A project without a loop region falls back to the full project.
- Escape or a click outside closes the menu without closing the dialog.
- Tests: `the_range_text_counts_the_bars_the_loop_touches` and
  `the_loop_range_renders_the_loop_exactly_and_falls_back_without_one`. The self-test sets a loop over bars 2-3, picks
  Loop region in the menu, checks the readout and estimate, exports it and checks the file's length and sound. Review
  state `review-export-range`.

### 2026-11-12: Device Studio GitHub organization

- Transferred the existing public repository to `devicedaw/device`, preserving its history, branches and issues.
- Updated the README badges and clone command, private security-report links, release-provenance examples and
  pull request media guidance to the organization URL. The old repository URL redirects to the new location.
- The project website is https://devicedaw.github.io/. Repository ownership and documentation links changed;
  application code and CI behavior are unchanged.

### 2026-11-12: channel strip trim, width and mono (spec 005)

- The channel strip's Trim, Width and Mono controls work (they were placeholders). Trim is ±24 dB in 0.1 dB steps,
  width 0-200 % (100 % unchanged), and Mono folds the track to mono whatever the width. Double-click resets a knob; a
  held knob drag is one undo step ("Trim Change", "Width Change"; the switch is "Mono Change"). Return tracks have
  them too. Saved per track as `trim` (dB), `width` and `mono`, left out of the file when neutral.
- Core: `Track::trim_db` / `width` / `mono`, `Command::SetTrackTrim` / `SetTrackWidth` / `SetTrackMono`.
  Bounce-in-place and material renders are taken before them (the track keeps its settings); stems keep them.
- Engine: `se_session_publish_v10` adds `se_track_strip` entries (only for tracks that are not neutral). Trim, width
  and mono fold into one 2x2 matrix per track at publish time, applied in place after the insert chain and before
  pre-fader sends, the fader and pan (`kernel_strip`). A neutral track is untouched, bit for bit.
- Tests: `engine/tests/test_strip.cpp`, `crates/dstudio-core/tests/strip.rs`, self-test section `strip_steps`.
  Bench: `dstudio_engine_bench` "Channel strip" table. Review state `review-strip-trim`.

### 2026-11-12: custom plugin folders

- Settings › Plugins › Add folder works (it was a placeholder): it opens a folder picker, keeps the folder in the
  preferences (`plugin_folders`) and checks for new plugins, so what the folder holds shows up. Custom folders are
  listed after the standard ones with a "Custom" badge and a remove button; the standard ones can't be removed. A
  folder that is gone shows the "Missing" badge and is skipped by the scan. "Check for new plugins" and "Rescan all
  plugins" include them, and removing one drops the plugins only it held. A folder already scanned, or whose path the
  scanner can't take (a ':' on macOS and Linux), is refused with a status message. Folder changes during a scan queue
  one more check.
- Engine: `dstudio_engine::scan_plugins_in` passes extra folders to the scanner process first in its
  `$DSTUDIO_PLUGIN_PATH` (no scanner change).
- Tests: `scanner_searches_custom_folders` (a copy of the test gain CLAP in a temp folder is found, a missing folder
  is skipped, and the plugin goes when the folder does), `custom_plugin_folders_are_added_once_and_never_shadow_a_standard_one`,
  the preferences round trip, and self-test section `plugin_folder_steps`. Review state `review-plugin-folders`.

### 2026-11-12: Device public name

- Public organization/profile, website, current docs, README hero and sharing artwork use **Device**.
- Desktop windows, menus, update text, export creator and audio/MIDI/plugin host display names use Device.
- macOS bundles are `Device.app`; release titles, download archives, Windows installer/shortcuts and
  Linux launcher labels use Device. Windows upgrades remove legacy default shortcuts.
- Existing `.dsproj` format, preferences/cache/project/scratch paths, executable, crate names,
  platform identities and installer AppId remain stable; see `docs/building.md` › Compatibility names.
- Historical entries retain the name used at the time. See spec 004 for requirements and verification.

### 2026-11-12: return tracks and sends in the Live set export (spec 006)

- Export › Project now writes return tracks and sends into the Live set (they were dropped). Each return is a Live
  return track with its volume, pan and on/off state; every track has one send per return at its level. A track whose
  Out is a return is "Sends Only" with a full send. Tracks are still rendered dry.
- A return with devices that a playing track feeds is also rendered (wet) onto "<return> (rendered)", and its Live
  return is written turned off so nothing sounds twice. `stem_snapshot` given a return solos it for that pass.
- Approximations, each listed per track in the notes file: sends above 0 dB are set to 0 dB (Live's maximum), and a
  pre-fader send on a rendered track becomes post-fader with its level divided by the track volume.
- `live_set::validate` checks one send per return on every track, levels in range, returns last and the pre/post
  list. Not yet opened in Live itself (spec 006, T5).

### 2026-11-12: making MIDI clips from the timeline

- Double-click an empty spot on an instrument track's lane: a one-bar MIDI clip in that bar (on the beat while snapping
  is off or bypassed), moved past a clip it would start inside and cut short rather than running into the next one.
  It is selected and opens in the piano roll, so notes can be drawn straight away. A double-click on a clip opens it
  in the Clip tab. Double-clicks in the lanes are timed by the app (500 ms from the first click's release), not by
  Slint, because the first click often selects a track and rebuilding the panels for it can outlast Slint's window.
- Right-click on an empty spot: Insert MIDI clip (instrument tracks) and Paste (when the copied clip fits the track),
  both acting at that spot.
- ⌘⇧M now opens the new clip in the piano roll (the Clip tab used to say "No clip selected").
- New Create menu in the menu bar: Audio Track (⌘T), Instrument Track (⇧⌘T), MIDI Clip (⇧⌘M).
- Tests: `double_click_on_an_empty_instrument_lane_makes_a_clip_ready_for_notes`,
  `a_new_clip_fits_between_its_neighbours`, `the_empty_lane_menu_inserts_and_pastes_where_it_was_opened`.
- From a hand walkthrough on a Mac (new project, clips, notes, play, sections, save and reopen):
  - The piano roll showed only the first of several notes added one after another until the panel was switched
    away and back. The bottom panel's rounded corners make Slint keep it as a cached picture (a layer) that it redraws
    only when a property it read changes, and drawing new notes into our own texture changed none. The renderer now
    hands its texture to Slint again whenever the picture changed (a new `Image`, which Slint never treats as equal),
    and skips drawing when nothing changed; the timeline gets the same. Self-test section `note_redraw_steps` checks
    the screen itself (a snapshot through Slint's caches): before the fix only the first of three notes showed.
  - ⇧⌘M and Create › MIDI Clip with the playhead inside a clip stacked a second clip exactly on top of it. The new
    clip now goes after the clips at the playhead and stops at the next one (`insert_at_the_playhead_never_stacks_clips`).
  - A double-click that makes a clip or renames a section no longer moves the playhead (the first click's seek is
    taken back), and renaming a section keeps the clip open in the Clip tab.
  - Saving names the project after its folder (the title and `project.json` used to stay "Untitled"), and the window
    title says "Edited" while there are unsaved changes.

### 2026-11-12: return tracks and sends read as one pair

- A return track is the shared effect track ("Return A"); a send is how much of one track goes into it. Every
  ordinary track now has a send slot for every return, so the two always appear together: the channel strip's Sends
  tab shows one knob per return (two to a row, like the Input tab) with the return's name, its dB value and a
  Post / Pre chip, and the mixer's Sends section one row per return on every strip. A slot with nothing sent is
  silent and dim; moving it stores a post-fader send ("Send Level Change"), double-clicking sets 0 dB.
- The "+" beside Sends (Sends tab and mixer strips) adds the next lettered return ("Add Return Track") and keeps the
  track selected; the add-send menu is gone. With no returns, the Sends tab shows a dim "Return A" placeholder that
  creates the return and a 0 dB send, and the Timeline's Returns pill adds Return A.
- Mixer: a send row's letter tag opens its options: Post-fader, Pre-fader, Reset to 0 dB, Remove send.
- Out lists Master only (send to a return to share an effect); a track an older project routed into a return lists
  that return too, so it can be moved back. Spec 002 is updated to match.
- Tests: `every_return_is_a_send_slot_on_every_track`, `moving_a_silent_send_turns_it_on`,
  `returns_pill_and_out_menu`, and the self-test's returns section rewritten for the new flow.
- Not yet: sends from one return track into another (needs engine routing and solo rules), and return rows in the
  timeline without a pan knob.

### 2026-11-12: automate any device control, mute and device on/off

- Every device control's right-click menu starts with "Automate <parameter>": built-in device knobs, buttons, choice
  switches and menus, plugin cards, and every Synth control (knobs, faders, pills, switches, mod matrix). A device's
  power switch offers "Automate On"; the channel's volume, pan and M (track header, inspector, mixer) and the
  inspector's trim, width and mono offer "Automate <name>". Choosing one shows the lanes with that parameter.
- New lane targets: Track › Mute, Trim, Width, Mono and <device> › On (listed last under each device in the lane
  picker). Mute, Mono and On are stepped: a value holds until the next point. Built-in choice parameters are stepped
  too. The M button, the inspector's trim, width and mono, and power switches follow an active lane; clicking a switch
  while armed and playing writes one step at the playhead. Bounce to new track now carries the strip's trim, width and
  mono (and their lanes) over to the new track, since the render is taken before them.
- Lane labels read in the parameter's own units, taken from the device's own text (Hz, dB, ms, ct, choice names,
  On/Off, the Synth's legacy parameters in Hz, ct, dB, ms and %), and a hovered or dragged point shows its value.
- Engine: `se_auto_lane_desc.flags` (was padding) with `SE_AUTO_FLAG_STEPPED`; `SE_AUTO_MUTE` (per-sample gain before
  the fader, replaces the static mute), `SE_AUTO_DEVICE_ON` (per block, replaces `enabled`, with a bypass delay line
  so switching keeps delay compensation) and `SE_AUTO_TRIM/WIDTH/MONO` (per sample, replace the strip settings).
  Tests: four engine tests in `test_automation.cpp`, core `switch_lanes_*`, app `automation::tests` (units, picker,
  right-click menus, switches and strip controls following lanes).

### 2026-11-13: Fine Signal public artwork

Applied the selected fine blue dot material to the public README, feature and sharing artwork.
The source texture and exact generation prompt live in `docs/art/textures/`; the HTML renderer
uses it at lower strength behind feature and architecture cards. App source, screenshots,
installation identities and settings are unchanged. The website uses compressed versions of
the same material in its own repository.

### 2026-11-14: export to MP3

- The Export dialog's Format segments now pick WAV or MP3 for Mixdown and Stems. With MP3 the second segments pick the
  bitrate (192 or 320 kbps, 320 by default) in place of the bit depth; the sample rate and range choices work as for
  WAV, the Name field ends in ".mp3", and the size estimate follows the bitrate. A Project export (Live set) stays WAV.
- `src/mp3.rs`: constant-bitrate joint stereo through LAME (`mp3lame-encoder` 0.3.0, LAME built from source by
  `mp3lame-sys`, LGPL), quality setting 2, with LAME's Info tag in the first frame so decoders trim the encoder delay
  and padding and the file has exactly the rendered length.
- Tests: `mp3::tests` encodes one second of a sine at both bitrates and reads it back through the engine's MP3
  decoder (same frame count, peak within 0.05 of the source, size within four MP3 frames of the estimate).

### 2026-11-14: Resample in the audio clip editor

- The Resample button in the audio clip editor's title row (a placeholder until now) renders the open clip through its
  track's devices, as Bounce to new track (⌘B) does: rendering onto the same track would run the devices twice, so the
  printed clip lands on a new "<track> (Bounce)" track below, at the same time and with the track's fader and pan, and
  the source clip is disabled. The source file stays on disk, the render covers the whole clip (not only the visible
  range) and runs in the background, and the result is one undo step.
- `Controller::resample_open_clip` (src/clip_render.rs). Self-test: the audio clip editor section presses Resample,
  checks the new track, the disabled source and the undo label, then undoes it in one step.


### 2026-11-15: dithered public artwork

The fine blue dot material behind the README, feature and sharing artwork is replaced by an ordered-dither
material: `tools/art/dither.mjs` (no dependencies) computes a bloom toward the lower right with one faint sine cycle,
quantises it to four colours from the app's tokens with an 8x8 Bayer matrix and writes small palette PNGs to
`docs/art/textures/`. `art.css` shows each texel as a crisp 3 px square (`image-rendering: pixelated`), so the pattern
survives GitHub scaling the images down. The generated bitmap and its prompt are gone. The website uses
`dither-site.png` from the same script and draws its sine waves dithered too. App views are unchanged.

### 2026-11-20: ASIO on Windows

Windows gets a second audio backend: installed ASIO drivers are listed in Settings › Audio as "<driver> (ASIO)"
and play and record through the driver, at the driver's buffer sizes and latencies (spec 011). The Steinberg ASIO SDK
(GPLv3 since 2025-10) is downloaded by CMake and checked by SHA-256; only its headers are used.
`windows_audio_device.cpp` picks WASAPI or ASIO per setup (`asio:` ids), `asio_device.cpp` drives the driver,
`asio_convert.hpp` converts its sample formats. Checked with FlexASIO on the Windows test PC (its WASAPI backend; the
default DirectSound backend stalls after a second, a FlexASIO issue). No hardware ASIO driver tried yet.

### 2026-11-20: edit tools

The timeline header's tool button opens its menu: Selector, Marquee tool and Scissors (spec 012). T toggles Scissors
with the tool used before. The Marquee tool drags a time range across tracks (snapped, drawn over their lanes);
Delete, ⌘X, ⌘C, ⌘V, ⌘D, ⌘E and ⌘L act on it, each one undo step. Range edits live in `dstudio_core::range`: they split
clips at the edges on a copy of the project first, so the batch carries every id and redo is exact, and clips that
can't be split (warped, reversed, patterns) stay whole. Scissors click splits the clip under the pointer at the snapped
position. App side: `src/tools.rs`; review states `review-tool-menu` and `review-marquee-range`; self-test section
`tool_steps`; core tests `tests/range.rs`.

### 2026-11-20: default instrument for new instrument tracks (spec 013)

- Settings › MIDI › Default instrument works (it was a placeholder that always read "Synth"). Its menu lists Synth,
  Sampler, Pad Sampler and Drum Rack, then the instrument plugins of the last scan under "Plugins" (by name, with
  their format); a long list scrolls. The choice is an app preference (`midi.default_instrument`), saved at once.
- Every empty-track path (⇧⌘T, Create › Instrument Track, the add-track entries in the timeline and the mixer) goes
  through `Controller::add_instrument_track` (now in `src/default_instrument.rs`): one "Add Instrument Track" step
  that adds the track named after the instrument and, unless Synth, sets its instrument. Synth makes exactly the
  track it made before. A MIDI clip dragged from the Files browser onto empty space makes its track the same way.
- A chosen plugin the last scan didn't find shows "Missing" in the dropdown; new tracks play the Synth with a status
  message, and the choice is kept for when the plugin is back. A preferences entry that doesn't parse loads as Synth
  without losing the rest of the file.
- Tests: `default_instrument::tests` (Synth unchanged, built-in named and undone in one step, missing and found
  plugin), `files::tests::a_dropped_midi_clip_gets_the_default_instrument`, `prefs::tests::a_default_instrument_round_trips_and_bad_entries_load_as_the_synth`, self-test section
  `default_instrument_steps`. Review states `review-default-instrument` and `review-default-instrument-missing`.

### 2026-11-20: Search settings (spec 014)

- The Search settings field at the top of the Settings sidebar works (it was drawn only). Typing filters the sidebar to
  the pages with a match, packed together, and highlights the matching rows on the open page; when the open page has no
  match the first matching page opens. A setting matches by its label or a few keywords ("latency" finds Sample rate,
  Buffer size and Plugin delay compensation). Nothing matching shows "No settings match your search". Escape clears
  the text (and closes Settings when it is empty), ⌘F puts the cursor in the field, and the text clears on close.
- `src/settings_search.rs` holds the table of settings (page, label, keywords). Each highlightable row in
  `ui/settings.slint` is a `SearchHit`, and a test compares the two so they can't drift apart.
- Fixed on the way: closing Settings with the cursor in one of its search fields (this one or Shortcuts' own) left the
  keyboard with the field, so no shortcut worked until something else took the focus. The key handler now takes the
  focus back when Settings closes.
- Tests: `settings_search::tests`, self-test section `settings_search_steps` (incl. ⇧⌘T working after closing with the
  cursor in the field). Review states `review-settings-search` and `review-settings-search-none`.

### 2026-11-20: timeline zoom sliders

The two sliders in the timeline header, painted until now, work: the horizontal one follows and sets the zoom (log
scale from half the default zoom to the closest), the vertical one sets every shown track's height (0.75 to 6 times
the default row), both as view changes, not undo steps. Double-click resets either; the clip editor's pair gains the
same double-click. `ZoomSlider` moved to `ui/widgets.slint` for both headers; mapping in `src/fit.rs`. Self-test
section `zoom_slider_steps`.

### 2026-11-20: sends between return tracks

A return track sends into the returns after it (spec 016): a delay return into a reverb, a reverb into a shimmer.
Signal always flows down the return list, so sends can't loop; a return move that would turn a send upward is
refused and the status line names the send. The mixer's Sends section shows on return strips (a row for each later
return, on that return's line) and a selected return's Sends tab has a knob for each later return, with the same
Post / Pre, dB readout, tag menu and undo as ordinary sends. Return sends are saved in the track's existing `sends`
list (`SCHEMA_VERSION` stays 1); a hand-edited upward or looping send is dropped on load, and builds before this one
drop return-to-return sends when they open a project.
- Engine: one solo rule for returns and groups. A soloed bus is heard with the buses it feeds (which take only what
  comes down that path); buses feeding it keep sending without being heard. Before, soloing a return silenced a
  group's send into it. `SendNode::live` is computed when the graph is built; with no bus soloed nothing changes.
- Live set export: return sends into later returns are written as active Live return sends, a rendered return's
  audio track carries its sends, no send goes into a rendered return, and a rendered return's audio includes the
  returns feeding it. The notes list them.
- Tests: `tests/return_sends.rs` (core), five `routing_*` engine cases (pre/post, solo with returns and a group,
  chained delay compensation, RT-guarded export parity), app unit tests, self-test `return_send_steps` and the
  Project export section. `bench_render` gets a "+ A into B" Returns row. Review states `review-return-sends` and
  `review-return-sends-strip`.
### 2026-11-21: assistant key and devices

The assistant no longer needs `ANTHROPIC_API_KEY` (which a Finder-launched app never sees): with no key, its panel
asks for one and keeps it in the system keychain (`src/api_key.rs`, the `keyring` crate: Keychain, Credential Manager,
Secret Service over zbus); "Remove API key" under the greeting deletes it. The variable still wins, for development.
The self-test uses an in-memory store and never touches the keychain. The model now sees each track's chain
(`devices`, `instrument`, with only the parameters changed from their defaults), sends, output, lanes and the
project's audio files, and can `add_builtin_device` (effects into the chain, instruments onto the track) and
`load_sample` besides any edit command (device parameters, sends, automation, audio clips). The built-in device
catalogue (44 devices, about 2000 parameters) goes into the system prompt as a cached block instead of every turn's
state. Still to do: one run against the live API.

