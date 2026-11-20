# Plan: Tempo changes during a song

Spec: [spec.md](spec.md)

## Approach

The tempo map lives in the core as a list of points on the transport. One type, `TempoMap`, turns
the points into segments (start tick, start second, start and end BPM) and does every conversion
between ticks, seconds and frames with the closed-form formulas below. With no points the map is
the old single tempo, and every conversion takes the exact arithmetic it takes today, so projects
without tempo changes produce bit-identical snapshots (R12).

The engine keeps getting positions in frames: clips, notes, automation and the loop still arrive
converted. What it gains is the map itself, as segments in the session snapshot
(`se_session_publish_v12`), for the three things that need musical time on the audio thread: the
metronome, the tempo and song position tempo-synced devices and plugins see, and nothing else.
Both sides evaluate the same documented formula, and tests check they agree.

The UI edits the map in a "Song › Tempo" lane on the master row, reusing the automation lane
gestures: the lane code works on a view of the map as an automation lane and commits one
`SetTempoMap` per gesture.

Alternatives rejected:

- **Tempo as an ordinary master automation lane** (`AutomationTarget::Tempo` in
  `master_automation`). It would get the lane UI for free, but tempo is not a parameter: it changes
  every conversion in the project, it needs exact integration rather than per-sample
  interpolation, holds and ramps per point rather than a curve, and the transport field and
  undo labels behave differently. A dedicated model keeps those rules in one place; the UI adapts
  it to the lane code.
- **Sampling the map into a frames-per-quarter curve for the engine.** Simpler for the engine but
  never exact, and the metronome would drift from core's beat frames. Segments plus a formula are
  exact and small (one per point).
- **Moving the tempo into the transport queue** (a new `se_transport_set_tempo_map`). The map has
  to change atomically with the clip positions computed from it, so it travels in the session
  snapshot.

### The formula (the contract)

A segment starts at frame `f0` and quarter `q0` with tempo `a` BPM, ends at the next segment's
quarter `q1` with tempo `b` (`b = a` for a hold; the last segment always holds). `R` is the sample
rate. With `k = (b - a) / (q1 - q0)` (BPM per quarter, a ramp linear in BPM over beats):

- hold: `q(f) = q0 + (f - f0) · a / (60 R)`, `f(q) = f0 + (q - q0) · 60 R / a`, tempo `a`;
- ramp: `f(q) = f0 + (60 R / k) · ln(1 + k (q - q0) / a)`,
  `q(f) = q0 + (a / k) · (exp(k (f - f0) / (60 R)) - 1)`, tempo `a · exp(k (f - f0) / (60 R))`.

Before the first segment (negative positions, the count-in pre-roll) the first segment's tempo
holds. Core evaluates the same with seconds instead of frames (`R = 1`) and ticks
(`q = tick / 960`).

## Changes by layer

- **Core** (`crates/dstudio-core`):
  - `tempo.rs` (new): `TempoPoint { time, bpm, ramp }`, `TempoMap` (segments; `ticks_to_seconds`,
    `seconds_to_ticks`, `ticks_to_frames`, `frames_to_ticks` on `f64` ticks too, `bpm_at`,
    `validate`), `TempoSegment` for the snapshot. Constants `MIN_BPM`/`MAX_BPM` stay in
    `command.rs`.
  - `model.rs`: `Transport::tempo: Vec<TempoPoint>` (`serde(default, skip_serializing_if =
    "Vec::is_empty")`). Invariant: with points, `transport.bpm` is the first point's BPM (the tempo
    at bar 1). `Project::tempo_map()`, and the existing `ticks_to_seconds`, `seconds_to_ticks`,
    `ticks_to_frames`, `frames_to_ticks` delegate to it; `bpm_at(tick)`; `content_length_ticks` of
    an unwarped clip measures its real-time length from the clip's start through the map.
  - `warp.rs`: `clip_source_at`, `clip_ticks_at` (unwarped clips through the map) and `warp_map`
    (with changes: points at each source marker, each tempo point inside the clip, and every
    sixteenth inside a ramp, so each beat is exact and in-between error stays far below 1 ms).
  - `command.rs`: `SetTempoMap { points, bpm }` replaces the whole map (labels "Edit Tempo
    Automation", "Delete Tempo Automation"; validation: tick ≥ 0, strictly increasing, BPM in
    20..=999; `bpm` restores the single tempo when the inverse removes the map, so the inverse is
    exact). `SetTempo { bpm, at }` gains an optional tick: with changes it sets the point in effect
    at `at` (the last point at or before it, else the first), label "Tempo Change", coalescing per
    `at`. Without changes it is today's command.
  - `persist.rs`: `SCHEMA_VERSION = 2`; `save` writes 2 only when the map has points, else 1;
    `load` accepts 1 and 2 (no migration step needed: version 1 has no map). `repair` drops
    non-finite points, clamps BPM and ticks, sorts, removes duplicate ticks and restores the
    invariant.
  - `snapshot.rs`: builds one `TempoMap` and uses it for every conversion; `SessionSnapshot`
    gains `tempo: Vec<TempoSegment>` (empty without changes) and `click_quarters` (quarters per
    metronome beat, `4 / denominator`).
  - `midi_record.rs` already converts through `frames_to_ticks`, so recorded notes land on the
    beats the player heard.
- **C ABI**: `se_tempo_segment { start_frame, start_quarter, start_bpm, end_bpm }` (doubles, 32
  bytes) and `se_session_desc_v12 { v11, tempo, tempo_count, _pad, click_quarters }` (144 bytes),
  `se_session_publish_v12`. Validation: segment 0 at frame 0 and quarter 0, frames and quarters
  strictly increasing, BPM finite in [1, 10 000], the last segment holds, `click_quarters` finite
  and > 0, at most 65 536 segments. Rust mirror and layout tests in `ffi.rs`; `Engine::publish`
  always uses v12 (count 0 without changes).
- **Engine** (`engine/`):
  - `src/tempo_map.hpp` (new): `TempoMap` over a span of segments with `quarter_at(frame)`,
    `frame_at(quarter)`, `bpm_at(frame)`; `noexcept`, no allocation, binary search.
  - `RenderGraph::tempo` (a vector filled at build time, off the audio thread) and
    `click_quarters`.
  - `RtTransport` gets a `const TempoMap* map`; `beats()` uses it when set; the engine sets
    `tempo_bpm` to the tempo at the block's first frame. Live and offline renders fill it from the
    current graph. With no map nothing changes.
  - Metronome: with a map, beat `k` sounds at `llround(frame_at(k · click_quarters))`.
  - `engine/README.md`: the formula, and the "tempo changes are not modelled" gap removed.
- **App** (`crates/dstudio-app`), PR B:
  - Master row lane picker lists "Song › Tempo" first; the lane draws the map in BPM (scale fitted
    to the points with headroom, at least 40 BPM wide), click, drag (Shift fine), double-click to
    type, Delete, Draw mode; right-click menu "Ramp to Next Point" / "Hold Until Next Point". The
    lane code gets an adapter: `AutomationTarget::Tempo` (app-side only, refused by
    `SetAutomationLane`) whose lane view is built from the map and whose commit becomes one
    `SetTempoMap`. Creating the first point also adds a point at bar 1 holding the current tempo,
    so a click at bar 9 changes the tempo from bar 9 only.
  - Ruler marks with BPM (an arrow for a ramp) whenever the map has points; clicking one shows the
    lanes with "Song › Tempo".
  - Transport field shows `bpm_at(playhead)`, follows it in playback, and edits through
    `SetTempo { at: playhead }`.
  - Readouts that use the tempo (device panels, Echo picture, recording live clip, "warp on",
    Smart Quantize) take it at the playhead or the clip start.
  - Review state `review-tempo-lane`; self-test `tempo_steps`.
- **Exports**, PR C: Mixdown, Stems and render in place render the published snapshot, so they
  carry the map from PR A; the export length already comes from `ticks_to_seconds`. MIDI export
  writes a tempo event per change (ramps as sixteenth-note steps). The Live set writes one tempo
  envelope event per point (ramps as linear segments plus extra events every sixteenth inside them,
  since no Live-saved fixture is in the repository to confirm how Live interpolates; the extra
  events keep beats within 1 ms either way), and rendered audio clips get warp markers at each
  tempo point they cross; `live_set::validate` checks the envelope.
- **Docs**: README (features, status), PROGRESS.md, `docs/architecture.md` › Persistence (schema
  2), `engine/README.md` (formula, gap removed), `specs/010-parity-roadmap/spec.md` (G37).

## Delivery

Three stacked pull requests, each complete on its own:

1. **tempo-changes**: core model, commands, persistence, conversions, snapshot, ABI v12, engine,
   tests and benchmarks. Projects with tempo changes (from a file or a script) play correctly.
2. **tempo-lane**: the lane, ruler marks, transport field, readouts, review state, self-test.
3. **tempo-export**: MIDI export, Live set export, export tests; marks the spec Done.

## Principles check

1. Real-time safety: the map is copied into the render graph at build time; the audio thread only
   reads it (binary search, `exp`, `log1p`). `test_rt_safety` renders a session with a map under
   the allocation guard.
2. One mutation path: `SetTempoMap` and `SetTempo` are commands with exact inverses; the lane and
   the field commit through `Controller::execute`.
3. Projects keep loading: version 1 files load unchanged; files with changes are version 2, which
   older builds refuse (`TooNew`); files without changes stay version 1 so 0.2.x opens them.
4. The C ABI is the only boundary: one new struct and one new publish function, appended; layout
   tests on both sides.
5. Cross-platform: plain C++23 and Rust, no platform code.
6. Original work: the lane reuses Device's own automation code; facts only about the Live set
   format.
7. Evidence: engine bench before/after, core snapshot bench with a 64-point map, screenshots of
   `review-tempo-lane`.
8. Docs move with the code (above).
9. Naming: `tempo`, `TempoMap`, `se_tempo_segment`; no bare `device`.

## Tests and evidence

- Core `crates/dstudio-core/tests/tempo_map.rs`: commands (apply, undo, redo, labels, invalid
  edits, coalescing of field edits), persistence (round trip, version 1 vs 2, a 0.2.0 project,
  version guard), conversions (round trip, ramp vs numeric integration), snapshot (notes,
  automation, loop, fades, warped clip within 1 ms per beat across a step and a ramp, reversed,
  every warp mode; unwarped clip frames independent of the map), bit-identical snapshot without a
  map.
- Engine `engine/tests/test_tempo.cpp`: clicks across a step and a ramp at the frames the
  formula gives, the tempo and song position a built-in device and the test plugin see per block,
  a bit-identical render without a map, validation; `test_rt_safety` with a map.
- End to end `crates/dstudio-engine/tests/tempo.rs`: the click frames core computes match the
  engine's, and an offline render with a step puts a click transient where the map says.
- Benchmarks: `dstudio_engine_bench` before/after plus a "Tempo map" row (64 points);
  `cargo bench -p dstudio-core --bench tempo` (snapshot build with no map and with 64 points).
- Screenshots: `review-tempo-lane` before/after (PR B).

## Risks

- Bit-identity without a map: every conversion keeps today's expression when the map is empty;
  a core test compares snapshots built with and without the new code path, an engine test renders
  with v11 and v12.
- Warp map accuracy inside ramps: checked per beat in every mode and reversed.
- Rounding drift between core (seconds) and engine (frames): the metronome test allows one frame.
