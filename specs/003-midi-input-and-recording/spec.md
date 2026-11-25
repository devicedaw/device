# MIDI input and recording

Status: Done · Spec: 003 · Issue: none

## Problem

Device plays instrument tracks only from notes drawn in the piano roll or imported from MIDI
files. Nobody can play a MIDI keyboard into it, hear an instrument while playing, or record a
performance. On instrument tracks the R (arm) and I (monitor) buttons only say "MIDI recording is
not implemented yet". The engine has no MIDI input at all, on any platform.

This is for anyone with a MIDI controller: the most common way to get musical ideas into a DAW is
to play them.

## Behaviour

**Connecting a keyboard.** MIDI inputs are found automatically on macOS, Windows and Linux. Every
input is on by default, including ones plugged in while the app runs (they show up within a couple
of seconds). Settings › MIDI lists the inputs under "MIDI inputs", each with a switch to turn it
off and a small activity light that blinks when it sends something. Turning an input off is an app
preference, like the audio device, not part of the project.

**Playing.** What the keyboard plays goes to every instrument track that is armed (R) or
monitoring (I), and only to those (decided in Q1): selecting a track does not make it play. Arm or
monitor several tracks to play several instruments at once. Notes go to whatever instrument the
track has: the built-in Synth, a built-in instrument such as Sampler or Drum Rack, or an instrument
plugin. Playing works while the transport is stopped. The keyboard's sustain pedal, mod wheel,
pitch bend and aftertouch go to instruments that accept them (plugins and built-in instruments);
the built-in Synth takes notes only.

Notes never hang: when a track stops receiving input (disarmed, monitor off, another track
selected, the input turned off or unplugged) while keys are held, those notes are released.

**R and I on instrument tracks.** R arms the track for recording and also lets it hear the
keyboard. I lets it hear the keyboard without arming it. Both are undoable ("Arm Track",
"Input Monitoring Change"), saved with the project, and shown in the track header, the mixer strip and the
inspector, as on audio tracks. Instrument tracks have no "In" selector: they listen to every input
that is on.

**Recording.** With at least one instrument track armed, the record button (or `R`) starts
playback and recording. While recording, each armed instrument track shows a red "Recording" clip
that grows with the playhead, with the notes drawn as they are played. Stop ends the pass, and
each armed instrument track that received notes gets a new MIDI clip:

- named after the track, starting at the bar where recording started and ending at the bar after
  the last note or the stop position, whichever is later;
- holding every note played, with its velocity, at the position it was heard (the output latency
  of the audio device is compensated). Notes still held at stop end at the stop position;
- when the loop is on and playback wrapped, the clip covers the loop and every pass adds its notes
  to it (overdub), so you can build a part up over several passes.

One recording pass is one undo step, "Record", together with any audio takes recorded in the same
pass. A track that received no notes gets no clip.

## Requirements

- R1. The engine lists the MIDI inputs of the system (stable id, display name) on macOS (Core
  MIDI), Windows (Windows multimedia MIDI) and Linux (ALSA sequencer), and opens exactly the ones
  the app asks for. A missing MIDI system (no ALSA sequencer, no Core MIDI server) means an empty
  list, not an error.
- R2. Input events reach instruments at the start of the next audio block. The audio thread never
  allocates, locks or blocks for MIDI: events cross from the MIDI thread through preallocated
  lock-free queues; a full queue drops events and counts them.
- R3. Routing follows the Behaviour rules above (armed or monitoring instrument tracks only), and works for the built-in Synth, built-in instruments and
  instrument plugins, while stopped and while playing.
- R4. Held notes are released when routing changes, when an input is closed and when the engine
  stops. No stuck notes.
- R5. `SetTrackArm` and `SetTrackMonitor` accept instrument tracks. `SetTrackInput` stays audio
  only. Projects saved before this change load unchanged (the fields already exist).
- R6. Recording captures every event delivered to an armed instrument track while the transport
  plays, with its timeline position (output latency compensated, loop wraps folded into the loop).
- R7. Stopping a recording adds one MIDI clip per armed instrument track that received notes, as
  described above, in one "Record" undo step (shared with audio takes of the same pass).
- R8. Settings › MIDI lists the inputs with an on/off switch and an activity light. The set of
  inputs turned off is an app preference; inputs default to on.
- R9. Tests can inject MIDI without hardware (an engine call), so engine, Rust and app self-tests
  cover input, routing and recording deterministically.
- R10. The cost of MIDI input on the audio thread is measured: with no MIDI input it must not move
  the existing render benchmarks; with input it is reported per block.

## Acceptance checks

- A1 (R1): engine test: listing works with no MIDI system present (CI); a manual check on a Mac
  with a USB keyboard lists and plays it (pending, task T9).
- A2 (R2, R4): engine tests in `engine/tests/test_midi_input.cpp`: injected note-on sounds in the
  next block on the built-in Synth, a built-in instrument and a test instrument plugin; the render
  path allocates nothing (`RtAllocGuard`); a full queue counts drops; disarming with a key held
  releases it.
- A3 (R3): engine tests for each routing rule (armed, monitoring, selected, none).
- A4 (R5, R7): core tests: arm/monitor on instrument tracks undo and redo; a recorded pass becomes
  one "Record" batch with the expected clip bounds and notes, including a looped pass.
- A5 (R6): engine test: captured positions while playing, latency compensation and loop folding.
- A6 (R7, R9): app self-test: arm an instrument track, inject notes during a recording pass, stop,
  the clip and its notes exist, one undo removes them.
- A7 (R8): review state screenshot of Settings › MIDI with two inputs (fake list, as the audio page
  does).
- A8 (R10): `dstudio_engine_bench` numbers before and after, and a MIDI input case.

## Out of scope

- Choosing an input device or MIDI channel per track (every instrument track hears every input
  that is on, on all channels). A per-track input selector can follow.
- MIDI output, MIDI clock and sync, MIDI learn and controller mapping.
- Recording sustain pedal, controllers, pitch bend or aftertouch into clips (clips hold notes only;
  the pedal is heard but not recorded).
- Sub-block timing from device timestamps (events are placed at block starts, so timing is
  accurate to one audio buffer, a few milliseconds at usual buffer sizes).
- Count-in, punch in/out, take lanes and comping, quantise on input.
- Virtual (on-screen or computer keyboard) MIDI input beyond what the app already has.

## Open questions

- Q1. Should the selected instrument track hear the keyboard when no track is armed or monitoring?
  Decided Nov 11: no, only armed and monitoring tracks play. The engine keeps its focus-track
  routing (`se_midi_set_focus`), but the app never sets a focus track.
