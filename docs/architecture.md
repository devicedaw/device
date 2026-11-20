# Device: architecture specification

Status: milestone 1 (functional audio vertical slice). Decisions marked **ADR** have their own record in `docs/adr/`.

## Layers and ownership

```
┌──────────────────────────────────────────────────────────────────────────┐
│ UI (Rust, Slint + custom wgpu surfaces)       crates/dstudio-app           │
│  • declarative shell, panels, controls (Slint)                           │
│  • GPU timeline / waveforms / meters (wgpu)      → ADR 0001              │
│  • holds NO authoritative state; renders from Document + engine readouts │
└───────────────▲───────────────────────────────┬──────────────────────────┘
                │ revision-change notifications │ Command (validated)
┌───────────────┴───────────────────────────────▼──────────────────────────┐
│ Application core (Rust)                        crates/dstudio-core         │
│  • model: Project / Track / Clip / Asset / Transport (musical time)      │
│  • command: serialisable, validated, invertible edit commands            │
│  • history: Document = project + undo/redo + coalescing + revisions      │
│  • persist: versioned JSON project dir, external media references        │
│  • snapshot: musical → sample-frame conversion, immutable session desc   │
│  • (next) keymap with context predicates, AI tool schema, plugin catalog │
└───────────────┬──────────────────────────────────────────────────────────┘
                │ SessionSnapshot → C ABI (engine/include/dstudio_engine.h)
┌───────────────▼──────────────────────────────────────────────────────────┐
│ Audio engine (C++23)                           engine/ (static library)  │
│  • non-RT side: asset decode + resample, graph build, retire queue       │
│  • RT side: audio callback → render graph, transport, meters, stats      │
│  • platform devices: Core Audio │ WASAPI, ASIO │ PipeWire, ALSA        │
│  • plugin hosting: VST3 / CLAP (all OSes), AU (macOS), OOP scanner       │
└──────────────────────────────────────────────────────────────────────────┘
 Optional services (AI, cloud catalog) sit beside the core and talk to it only through Commands.
```

## Threading and real-time rules

| Thread | Owns | May not |
|---|---|---|
| UI / main | `Document`, UI state, event loop | block on audio, touch engine memory |
| Workers | file decode (`se_asset_load_file`), peak computation, project I/O | mutate `Document` (send results back to main) |
| Audio callback (engine) | current render graph pointer, transport position | allocate, lock, block, do I/O, log, call Rust |
| MIDI input (engine backends) | nothing: each open port pushes its messages into its own lock-free queue | block, call Rust |

Hand-off mechanisms:
1. **Session snapshots.** After every `Document` revision change, the main thread builds a
   `SessionSnapshot` (pure data, frames at engine rate) and calls `se_session_publish`. The engine
   copies it, builds an immutable render graph off the audio thread, and swaps it in with an atomic
   pointer exchange. Retired graphs are reclaimed by the non-RT side (never freed in the callback).
2. **Transport commands** go through a bounded lock-free queue drained at block start, so they're
   sample-accurate to the block boundary.
3. **Readouts** (transport position, meters, callback stats) are atomics written by the callback and
   polled by the UI each frame.
4. **MIDI input** crosses from the platform's MIDI thread to the callback through one SPSC queue per
   port; the callback routes it to instrument tracks and, while recording, writes what it delivered
   into a capture ring the main thread drains (`engine/README.md` › MIDI input).

The audio callback never sees `Project`, tempo or ticks. All musical-time maths happens in
`dstudio_core::snapshot`.

## Command system

`dstudio_core::Command` is the single mutation path for the UI, keyboard shortcuts, scripting, tests and
the AI assistant:

- **Validation** in `apply` (ranges, existence, kind compatibility, asset bounds). On error the project is
  unchanged. Batches are atomic and roll back on the first failure.
- **Undo/redo**: `apply` returns the exact inverse. Ids are assigned *before* execution, so redo
  reproduces identical ids. Ids are never recycled.
- **Coalescing**: continuous gestures (fader/pan/tempo drags) merge into one undo step within 750 ms
  per target.
- **Labels** for "Undo ⟨action⟩" menu titles.
- **Serialisation** (`{"op":"set_tempo","bpm":128}`) makes commands usable as AI tool calls and
  scripts, and replayable in tests.

## Persistence

- Project = directory `Name.dsproj/` containing `project.json`
  `{format, schema_version, saved_with, project}`. Atomic save (temp + fsync + rename).
- Media referenced externally by project-relative *and* absolute path. On load the relative path wins
  (project moved with its media), then the absolute path, and anything missing is reported.
- Schema migrations: `persist::migrate(from, value)` upgrades step by step. Newer-than-supported files
  are rejected rather than mis-read.
- **SQLite** (planned) is for the sample-library index, analysis caches and plugin scan results: data
  that is large, query-heavy and rebuildable. The project document stays JSON so it diffs well, merges
  in VCS, and is easy to inspect and repair.

## AI assistant (design)

- Tools exposed to the model = JSON schemas generated from `Command` plus read-only query tools
  (list tracks, describe selection, transport state).
- **Assist mode**: model output → `Command` (deserialised) → validation → preview/confirm policy →
  `Document::execute(cmd, Origin::Assistant)` → one undo step per assistant turn (a `Batch`).
- **Teach mode** ("Point" in the UI): the model may only return *highlight* directives
  (stable UI element ids) and explanations. Any `Command` it returns is rejected.
- Runs out of process or async. The DAW works fully offline. AI never touches engine memory.

## Cross-platform strategy

Status per platform, what CI covers and what is untested on real hardware:
`engine/README.md` › Platforms.

- Shared code (core, engine DSP and graph, UI) has no OS-specific dependencies. Platform code is
  isolated in `engine/src/platform/*` (C++) and behind `#[cfg]` in Rust (the macOS
  transparent-titlebar winit hook in `dstudio-app`, per-OS folders in `prefs.rs`/`devices.rs`,
  link libraries in `dstudio-engine/build.rs`).
- OS layer: `engine/src/platform/os.{hpp,cpp}`: UTF-8 everywhere at the interface (wide Win32
  APIs underneath: `_wfopen`, `CreateProcessW`, `GetModuleFileNameW`, `CommandLineToArgvW`),
  environment, and a `ChildProcess` abstraction (CreateProcessW / posix_spawn) used by the
  out-of-process plugin scanner and its tests.
- Audio, behind one `AudioDevice` interface:
  - macOS: Core Audio (AUHAL; private aggregate device for duplex).
  - Windows: WASAPI shared mode, event-driven, float32, converted to the engine rate by
    `AUTOCONVERTPCM`; duplex through a separate capture client and a drift-tolerant
    `FrameRing`. ASIO drivers through the Steinberg ASIO SDK (GPLv3 option): one driver for
    output and input, picked by an `asio:` device id (`windows_audio_device.cpp`).
  - Linux: PipeWire playback/capture streams (same `FrameRing` duplex). JACK is future work for
    jackd-only systems (PipeWire already serves JACK clients).
  - Real-time thread priority: Core Audio's IO thread; MMCSS "Pro Audio" on the WASAPI thread;
    PipeWire's data thread (RTKit / rt module).
- MIDI input: Core MIDI (macOS), the multimedia `midiIn*` API (Windows), the ALSA sequencer
  (Linux), behind one `MidiBackend` interface.
- Plugins: AU (macOS only), VST3 and CLAP everywhere; libraries loaded with CFBundle /
  `LoadLibraryExW` / `dlopen`, standard per-OS folders, VST3 class ids stored in the canonical
  text form on every OS. Editors: NSWindow (macOS), a top-level HWND hosting the plugin's child
  window (Windows), an X11 window the plugin embeds into (Linux; XWayland on Wayland desktops), with a
  host run loop for the plugins' fds and timers (`src/platform/run_loop.cpp`, run from `se_plugin_idle`).
- App: Slint (winit backend) + wgpu 30 build on all three; Linux needs the X11/Wayland,
  xkbcommon, fontconfig, udev, PipeWire and ALSA development packages (see the CI workflow).
- CI (`.github/workflows/ci.yml`): one job per OS, all on GitHub-hosted runners
  (`macos-latest`, `windows-latest`, `ubuntu-latest`; no audio device, so device tests skip).
  Real-hardware audio tests are run by hand on a Mac. Each job builds the
  engine with warnings as errors and runs ctest, `cargo build` / `cargo test --workspace` and the
  app self-test (`engine/README.md` › Platforms).

## Technology adjustments (documented deviations)

1. **No JUCE in milestone 1.** The engine uses direct Core Audio plus a hand-written WAV reader/writer
   and resampler: fewer dependencies, full control of RT behaviour, faster builds. JUCE modules (or
   direct SDKs: VST3 SDK, CLAP headers, AU via AudioToolbox) will be evaluated when plugin hosting
   starts. The C ABI keeps this choice invisible to Rust.
2. **Hand-written FFI** instead of bindgen, to avoid a libclang build dependency. A layout test guards
   against drift.
3. UI rendering approach: see ADR 0001 (Slint/wgpu interop spike).
