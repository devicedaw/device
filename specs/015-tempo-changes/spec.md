# Tempo changes during a song

Status: Building · Spec: 015 · Issue: none

## Problem

A project has one tempo for its whole length. `Transport::bpm` is a single number, and every conversion from musical
time to real time (`Project::ticks_to_seconds` and the functions built on it, the snapshot, the warp maps) divides by
it. Anyone whose song speeds up into a chorus, slows into an outro, or follows a live recording that drifts must
split the song into separate projects or fake it with re-timed audio.

How tempo reaches each part today:

- **Core**: clips, notes, automation points and the loop region sit in ticks. `dstudio_core::snapshot` turns them into
  engine frames at the one tempo, so the audio thread never deals with tempo. A warped clip goes to the engine as a
  list of (timeline frame, source frame) points (`Project::warp_map`). An unwarped clip keeps its start in ticks and
  plays at its own speed, so its length in beats depends on the tempo.
- **Engine**: `se_transport_set_tempo` takes one frames-per-quarter value. Tempo-synced built-in devices and the plugin
  host's tempo and song position use it. `se_metronome_set` clicks on a fixed grid from frame 0. engine/README.md lists
  "Tempo changes are not modelled" as a known gap.
- **App**: the transport bar tempo field (`transport_fields.rs`) drags, types, steps and taps one value. Each gesture is
  one "Tempo Change" undo step (`Command::SetTempo`, 20 to 999 BPM). Count-in, recording, Smart Quantize, device-panel
  readouts and "warp on" (which sets a clip's original BPM) all read `transport.bpm`.
- **Exports**: Mixdown and Stems render through the engine at the one tempo. The Live set export writes one static
  tempo envelope and gives rendered audio two warp markers, so Live plays it unstretched at that tempo. Clip menu ›
  Export MIDI writes one tempo event at the start of the file. Device cannot import MIDI files.

## Behaviour

- **Tempo lane.** When automation lanes are shown, the master row's lane picker lists "Song › Tempo" above
  "Master › Volume". Choosing it draws the tempo as a line over the master row, labelled in BPM ("128.00 BPM"). The
  vertical scale fits the points with some headroom, and is never narrower than 40 BPM. The same gestures as other lanes
  apply: click the line to add a point, drag points (with Shift for fine steps), double-click a point to type a value,
  Delete to remove the selected points, and Draw mode to draw freehand.
- **Steps and ramps.** A new point holds its tempo until the next point. Right-click a point and choose "Ramp to Next
  Point" to make the tempo change smoothly to the next point instead. The ramp is linear in BPM over beats. Choose
  "Hold Until Next Point" to switch back.
- **Ruler.** When the project has tempo changes, the ruler shows a small mark and the BPM at each point (with an arrow
  for a ramp), even with lanes hidden. Clicking a mark shows the lanes with "Song › Tempo" chosen.
- **Transport bar.** The tempo field shows the tempo at the playhead and follows it during playback. With no tempo
  changes, dragging, typing, stepping and tapping work as today. With tempo changes, they change the point in effect at
  the playhead: the last point at or before it, or the first point before the song's first change.
- **Undo labels.** Each lane gesture is one step, "Edit Tempo Automation". Removing the lane is "Delete Tempo
  Automation", and the song then keeps the tempo it had at bar 1. The transport field stays "Tempo Change".
- **What follows the tempo.** Bars and beats stay fixed, and real time stretches around them. MIDI notes, automation
  points, the loop region and every clip start stay on their bar and beat. Warped audio clips stretch to stay locked
  to the bars. In Repitch mode their pitch changes with the tempo, and glides through a ramp. Unwarped audio clips
  start on their beat and play at their own speed, so their length on the timeline changes with the tempo under them.
  Tempo-synced devices and plugins follow the tempo at the playhead. Device panels that show tempo-dependent values
  (delay times, LFO rates in Hz, the Echo picture) use the tempo at the playhead.
- **Metronome and count-in.** The click falls on every beat of the changing tempo. A count-in plays at the tempo at
  the punch point.
- **Recording.** MIDI notes land on the beats the player heard. A recorded audio take starts on the beat where
  recording started and is an unwarped clip, as today. "Warp on" sets a clip's original BPM from the tempo at the clip's
  start.
- **Saved with the project.** The tempo points (time, BPM, hold or ramp) are saved. Projects saved before this load
  with their one tempo and no changes, and sound the same.

## Requirements

- R1. A project has a tempo map: a sorted list of points (tick ≥ 0, BPM in 20..999, hold or ramp to the next point).
  With no points it is the single `transport.bpm`. It is saved with the project, and old projects load as a single
  tempo with no changes.
- R2. Undoable commands add, move, change and remove tempo points, with the labels above. A value out of range, a
  negative tick or unsorted points are refused and change nothing. A drag is one undo step. Undo during a drag drops the
  preview and changes nothing.
- R3. All conversions between ticks, seconds and frames in core use the map. A ramp is integrated exactly, not
  sampled. Converting ticks to frames and back returns the same tick (within rounding) everywhere on the map.
- R4. The session snapshot places clips, notes, automation points, fades, clip loops and the transport loop region
  through the map. A warped clip's beats each play within 1 ms of the frame the map gives them. This also applies
  across a ramp and in every warp mode, and for reversed clips.
- R5. Unwarped clips play at their own speed whatever the tempo, start on their tick, and are drawn and trimmed with
  the length the map gives them.
- R6. The engine accepts the tempo map without allocating, locking or blocking on the audio thread. Tempo-synced
  built-in devices and the plugin host see the tempo and song position (quarter notes from bar 1) at the playhead, live
  and in offline renders. The metronome clicks on the frame core computes for each beat, within one frame.
- R7. The tempo lane, ruler marks and transport field behave as described in Behaviour.
- R8. Mixdown, Stems and render in place carry the tempo changes. The export length comes from the map.
- R9. Clip menu › Export MIDI writes a tempo event at each change inside the clip, and at its start the tempo in effect
  there. Ramps are written as steps every sixteenth note.
- R10. The Live set export writes the song tempo envelope with one event per point (ramps as linear segments in beats).
  Rendered audio clips get warp markers at the tempo points they cross, so Live plays them unstretched. `live_set::validate`
  checks the envelope.
- R11. A project with tempo changes opened in a build that predates this spec must be refused, not played at one tempo
  (principle 3).
- R12. Sessions without tempo changes render bit-identically to before, and neither the audio path nor snapshot
  building gets measurably slower for them.

## Acceptance checks

- A1 (R1, R2, R11): core tests in `crates/dstudio-core/tests/tempo_map.rs` check apply, undo and redo with labels, that
  invalid edits leave the project unchanged, and that a held drag is one step. They also save and reopen a map, load a
  0.2.0 project as a single tempo, and check that the schema version guards a project with changes.
- A2 (R3, R4, R5): core tests check the tick, frame and tick round trip over steps and ramps, and compare ramp
  conversions with numeric integration. They check that notes, automation, the loop and fades land on the map's frames,
  and that the warp map of a warped clip across a step and a ramp is within 1 ms per beat. They also check that an
  unwarped clip's frames are the same at any tempo and its tick length matches the map.
- A3 (R6): engine ctest checks clicks across a step and a ramp against frames from core, and the tempo and song
  position a synced device and a test plugin see per block. `test_rt_safety` checks that publishing and rendering a
  tempo map does not allocate.
- A4 (R7): a self-test step chooses "Song › Tempo", adds a point and a ramp, and checks the model, undo, the transport
  field text at two playhead positions and the ruler marks. Before and after screenshots of review state
  `review-tempo-lane`.
- A5 (R8, R9, R10): tests render a short project with a step and check that the export length and the position of a
  click-track transient match the map. They check the tempo events of an exported MIDI clip, and that a written Live set
  passes `validate` with a multi-point tempo envelope and matching warp markers.
- A6 (R12): engine ctest checks a bit-identical render with no map. The `dstudio_engine_bench` and snapshot-build
  numbers come before and after, plus a "Tempo map" row with 64 points.

## Out of scope

- Time-signature changes during a song. Tempo is counted in quarter notes, so the map does not depend on the meter.
  Meter changes affect bar numbering, the ruler, the grid, downbeats and Live/MIDI export separately, and need their
  own spec.
- Importing tempo changes from Live sets (`live_import.rs` keeps reading the static tempo) and MIDI file import,
  which Device does not have.
- Curved tempo ramps (only linear ramps and holds), and tempo detection from a recording to build a map.
- Assistant tools for tempo changes. `set_tempo` behaves like the transport field.
- Automation points that ramp in ticks will interpolate in frames inside a tempo ramp, as they do today. Making them
  follow the ramp exactly is not part of this work.

## Open questions

None. Agreed 2026-11-20:

1. Tempo is edited in a "Song › Tempo" lane on the master row, with marks on the ruler.
2. Steps by default; a ramp is a per-point choice, linear in BPM over beats, no curve handle.
3. The transport tempo field edits the point in effect at the playhead (as today with no changes).
4. `SCHEMA_VERSION` becomes 2, but a project is written as version 2 only when it has tempo changes;
   one without them is still written as version 1, so 0.2.x builds keep opening it. An older build
   refuses a project with tempo changes instead of playing it at one tempo (principle 3: refused,
   never misread). Loading accepts 1 and 2.
5. Core sends tempo segments (start frame, start tick, start and end BPM) with a documented formula
   that the engine evaluates, tested to agree with core.
6. The plan checks how Live interpolates tempo envelope events (facts only); if the shapes differ,
   the export adds events inside ramps so beats agree within 1 ms.

## Decided while building

- The engine segments carry the start in quarter notes (`start_quarter` = tick / 960) rather than ticks, so the
  engine needs no PPQ; the content is the same.
- `SetTempo` gained an optional position (`at`) instead of a new command: with tempo changes it sets the point in
  effect there, without them it is the old command. Scripts and the assistant that send no position edit the tempo at
  bar 1.
- A reversed warped clip on a map is mirrored in beats (its last beat plays first, on the first beat's frame), not in
  frames: mirroring frames would move the tempo changes inside the clip.
- Inside a ramp, devices and plugins get the tempo of each block's first frame; the song position they ask for is exact
  at every frame. At 256 frames and 48 kHz the tempo they see lags the ramp by at most 5 ms.
- The tempo lane keeps the lane gestures every other lane has: a double-click on a point deletes it (as on every
  lane) rather than opening a text field; a value is typed in the transport field, which edits the point in effect
  at the playhead. Drags move points in whole BPM; ⌥ drags in hundredths (and, as on every lane, without snapping
  time). The tempo lane has no power switch (the song always has a tempo) and no curve handle.
- A new point made in an empty tempo lane also puts a point at bar 1 holding the song's tempo, so a click at bar 9
  changes the tempo from bar 9 on, not for the whole song.
- The master row's pill and picker now name the master volume "Master › Volume" (it said "Track › Volume").
- A count-in follows the song's tempo map: the song plays during the count-in bars, so the click there keeps time with
  the music. That is the tempo at the punch point whenever no change falls inside the count-in bars, and before bar 1
  the first tempo holds.
