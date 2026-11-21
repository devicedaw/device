# Recording offset per audio device

Status: Done · Spec: 020 · Issue: none · Roadmap: [010 › G11](../010-parity-roadmap/spec.md)

## Problem

Recorded takes are placed earlier by the latency the audio device reports (input + output, ADR 0002). Some
interfaces and drivers report it wrongly, so their takes land a few milliseconds late or early, and the error is the
same every time. The acoustic loopback test on a MacBook Air still found takes +2.7..6.2 ms late (PROGRESS.md, round
6). A musician who has measured the error has no way to correct it: the README lists the manual recording offset as
missing.

## Behaviour

- Settings › Audio has a **Recording offset** row under the plugin latency line: a value in milliseconds ("+4.5 ms",
  "0.0 ms", "−2.0 ms") between a − and a + button, with a line under it.
- The value belongs to the selected input device. Picking another input device shows that device's offset (0.0 ms for
  one never set); picking the first one again brings its value back. With no input device the row is greyed and the
  line says "Choose an input device to set its offset."
- **Positive values move takes earlier, negative later.** Takes that land late need a positive offset. The line under
  the row says so.
- + and − move the value by 0.5 ms. Clicking the value lets you type one (with or without "ms", a sign or a decimal
  comma); Enter or clicking elsewhere applies it, kept to 0.1 ms and to −100..+100 ms. Text that isn't a number leaves
  the value as it was and says so in the status line.
- The offset applies to every audio take recorded from then on, on top of the reported latency and plugin delay
  compensation. It is not undoable and not saved with a project: like the rest of Settings › Audio it is an app
  preference, saved at once with the device's preferences and restored at launch.
- While recording the value can't be changed ("Stop recording before changing the recording offset").
- Search settings finds the row by "recording offset", "latency", "calibration", "late", "early", "takes" and "timing".

## Requirements

- R1. A recording offset in ms, −100..+100 in 0.1 ms steps, default 0, kept per input device (by its device id) in
  the app preferences; devices left at 0 are not stored. Old preference files load with no offsets; a hand-edited
  value out of range is clamped.
- R2. The engine places every audio take `offset` frames earlier (negative: later): `start_frame = position − (input +
  output latency) − L − offset`, with the offset converted to engine frames at the engine rate. The samples are
  unchanged; only the placement moves.
- R3. The offset reaches the engine at launch (before the device starts), when it is edited and when the input device
  changes. A change while recording would end the takes like a change of L, so the app refuses edits while recording.
- R4. MIDI recording is not affected (see Decisions).
- R5. The C ABI change is additive: `se_record_set_offset` / `se_record_get_offset`, |frames| ≤ one second at the
  engine rate, else `SE_ERR_INVALID_ARG` and nothing changes. Nothing new runs on the audio thread except one relaxed
  atomic load per callback.
- R6. The row is in Settings search (`settings_search::ENTRIES`), with a `SearchHit` row in `settings.slint`.

## Acceptance checks

- A1 (R2, R5): engine tests `record_offset_moves_takes_by_exactly_that_many_frames` (216, −4800 and 0 frames; same
  samples; range and refused values), `record_offset_change_while_recording_starts_a_new_take`, and
  `device_recording_is_latency_compensated` (fake device: the offset adds to the 250 reported frames).
- A2 (R2, R5): Rust test `recording_offset_moves_the_take` (`crates/dstudio-engine/tests/recording.rs`): through the
  wrapper, −480 frames moves a take from 96 000 to 96 480.
- A3 (R1): prefs test `recording_offsets_are_kept_per_input_device` (per device, rounding, clamping, frames at 48 and
  44.1 kHz, JSON round trip, old files, nothing stored at 0); settings test `recording_offset_text` (display and
  parsing).
- A4 (R1, R3): self-test `recording_offset_steps`: disabled without an input, + / − / typing reach the engine in
  frames, clamping, a second device starts at 0 and the first keeps its value, back to 0 stores nothing.
- A5 (R6): `settings_search` tests (the table matches the rows; "latency" and "calib" find the row).
- A6: screenshots of `reference-settings-audio` before and after, and `review-settings-recording-offset` (+4.5 ms).

## Out of scope

- A Calibrate button that plays a click and measures the round trip: it opens the microphone and needs an acoustic
  path, so it is a feature of its own (the manual loopback test, `real_loopback.rs`, measures it today).
- An offset per output device, or per input channel.
- Moving takes that were already recorded.

## Decisions

Taken while writing (the maintainer asked for recommendations to be followed and recorded):

- **Sign:** positive moves takes earlier. The common error is takes landing late (the loopback residual is late), so
  the usual correction is a positive number, and it adds to the latency compensation the same way the reported
  latency does.
- **Range and step:** −100..+100 ms, 0.1 ms (about 5 frames at 48 kHz). Driver errors are a buffer or two at most;
  100 ms covers a 2048-frame buffer at 44.1 kHz twice over. Buttons step 0.5 ms so a typical correction is a few
  clicks; typing gives the full precision.
- **Where it is applied:** in the engine, added to the recording compensation, rather than by moving the take in the
  app afterwards. The live take drawn while recording, the finished take and the "Record" undo step then all agree,
  and the engine tests can check the frames exactly. It applies to injected input too, so tests don't need a device.
- **Audio only, not MIDI:** the offset corrects the latency the audio input device reports, which MIDI capture never
  uses (MIDI notes are placed by the output latency and L, and are accurate to one buffer). An audio device's offset
  shifting MIDI recorded from a different device would be surprising.
- **Keyed by device id**, like the device choice itself, so renaming does not lose it; the system-default input is
  not offered by the app, so there is no entry for it.
- **No Calibrate button** (see Out of scope).

## Open questions

None.
