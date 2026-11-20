# Plan: Recording offset per audio device

Spec: [spec.md](spec.md)

## Approach

The engine already subtracts a compensation from every new take's start (`Recorder::capture(..., latency, ...)`,
fed with the device latency plus L). The offset is one more term: an atomic in `Engine`, read once per callback in
`process()` and added to that compensation. Because `Recorder` already starts a new take when the compensation
changes, a change while recording behaves like a change of L with no new code.

The app stores milliseconds per input device id in `AudioPrefs` and converts to engine frames at the engine rate
whenever it hands the value over.

Rejected: shifting the take in the app when it lands (the live take would be drawn in one place and land in another;
the engine tests could not check it); storing frames (they change meaning with the sample rate).

## Changes by layer

- **Engine**: `Engine::record_offset_` (atomic, engine frames); `process()` adds it to the capture compensation.
  C ABI: `se_record_set_offset(engine, int32 frames)` (|frames| ≤ engine rate) and `se_record_get_offset`. Header
  comment and engine/README.md › Latency compensation.
- **Rust wrapper**: `ffi.rs` declarations; `Engine::set_recording_offset` / `recording_offset`.
- **App**: `AudioPrefs::recording_offsets` (id -> ms) with `recording_offset_ms`, `set_recording_offset_ms`,
  `recording_offset_frames`; `apply_startup_prefs` and `apply_audio_prefs` hand it to the engine; the Settings ›
  Audio row (`settings.slint`, `Studio` properties and two callbacks); `settings_search::ENTRIES`; review state
  `review-settings-recording-offset`; self-test `recording_offset_steps`.
- **Core**: none. No project or schema change.
- **Docs**: README (Recording row of the status table, Settings), engine/README.md, ADR 0002 implementation notes,
  PROGRESS.md, roadmap G11.

## Principles check

- Real-time safety: one relaxed atomic load per callback; no allocation, lock or call out.
- One mutation path: not project state (an app preference, like the device choice), so no `Command`.
- Projects keep loading: no project change; preference files without the field load with no offsets.
- C ABI: two appended functions, plain types; no struct changes, `SE_ABI_VERSION` stays.
- Cross-platform: no platform code.
- Evidence: engine, Rust and app tests, self-test, screenshots. Performance: the render path gains one atomic load and
  an add per callback, below what the benchmarks resolve; the Callgrind gate in CI covers it.

## Risks

- The value applied in frames depends on the engine rate, which is fixed when the engine is created, so the frames
  never go stale.
- A TextInput's binding breaks once typed in: the field puts the shown value back after each commit and follows
  changes made with the buttons.
