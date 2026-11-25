# Song-ready roadmap: what comes after parity

Status: Agreed · Spec: 025 · Issue: none

## Problem

The parity roadmap ([010](../010-parity-roadmap/spec.md)) is done apart from G27, which another contributor
owns. Device now has the whole song-making toolset: timeline, piano roll, step sequencer, recording with takes,
warp, groups, returns, automation, plugins with delay compensation, and export to WAV, MP3, stems and Live sets.

What still stops a musician from trusting it with a real song is not a missing screen. It is losing work (no
prompt before quitting, recordings left in the temp folder, no autosave), finishing a song (no effects on the
master bus, export that clips without a warning), and moving a project between machines (no relink). This list
was written on 2026-11-25 against `main` at `db8d8b4` (0.3.1 plus #164 to #168), from an audit of the code,
the CI history and the website. It replaces 010 as the list contributors and agents work from.

## How to use this list

- Work top to bottom. S0 protects people's work, S1 finishes a song, S2 keeps the codebase fast and safe to
  change.
- Each item says what is wrong today (with where the code is), and when it is done. Items marked **spec** get
  their own `specs/NNN-slug/` first, as [specs/README.md](../README.md) says.
- Built-in device DSP and presets are not on this list: another contributor owns them. Items that touch the
  device-hosting interface (S1.3) are agreed with them first.
- Every visible change needs before and after screenshots, every broken behaviour a few small tests, and
  anything on the audio path a benchmark.
- Tick an item in the pull request that finishes it (`- [x]`, with the PR number).

## S0: never lose work

- [ ] **S0.1 Ask before quitting, closing or opening.** Only New Project and the Live set import asked about
  unsaved changes. Quit, the window's close button, File › Open, Open Recent and opening a project from the
  browser threw them away (`main.rs`, `controller.rs::open_recent`, `files.rs::files_activate`), and a change
  made in a plugin's own window never marked the project as edited. Done when every one of those asks first
  (Save / Don't Save / Cancel), with self-test steps for the close button and Open.
- [ ] **S0.2 Recordings made before the first save move into the project.** Takes and rendered clips go to
  `<temp>/Device Studio/` until the project is saved, and Save left them there (`recording.rs`,
  `clip_render.rs`). Done when Save moves them into `<project>/media/`, the undo history follows, and a unit
  test covers takes, renders and name clashes.
- [x] **S0.3 Autosave and crash recovery** ([spec 026](../026-autosave/spec.md), #176). Nothing is written
  between manual saves, and a crash (a plugin crash takes the whole app down) loses everything since. Done when a
  backup of an edited project is written every 2 minutes off the audio thread, atomically, in the app's data
  folder; the next launch after a crash opens it as unsaved changes (Save keeps them, Don't Save declines); a take
  that was being recorded is recovered with a valid WAV header; and tests cover writing, restoring and declining.
- [ ] **S0.4 Relink missing media.** Opening a project whose files moved only says "N media file(s) missing"
  in the status bar (`controller.rs`, `open`). Done when a dialog lists the missing files, Locate… searches a
  chosen folder for the rest by name and length, and the result is one undo step.
- [ ] **S0.5 Collect All and Save.** Imported audio is referenced where it lies. Done when File › Collect All
  and Save copies every outside file into `<project>/media/` and repoints the project.

## S1: finish a song

- [ ] **S1.1 Safer export** (spec). The WAV writer clamps at ±1 with no warning (`wavgen.rs`); there is no
  dither, no 32-bit float, no peak or loudness readout, and the tail is a fixed 2 s. Done when the export
  dialog offers 32-bit float, TPDF dither for 16-bit, warns when the mix clipped (with the peak), reports
  integrated loudness, and the tail runs until the reverbs and delays have died away (with a cap).
- [ ] **S1.2 CPU load and dropout meter.** The engine counts callbacks over their deadline
  (`se_stats.xruns_or_overloads`) but nothing shows it. Done when the transport shows DSP load and flags
  dropouts, with a benchmark showing the meter costs nothing on the audio thread.
- [ ] **S1.3 Effects on the master bus** (spec, routing and engine ABI; agree the device-hosting interface
  with the devices owner). The master has gain, mute and automation only (`model.rs`). Done when built-in
  effects and plugins can be inserted on the master, saved with the project (older projects still open),
  shown in the mixer and device panel, and covered by a benchmark.
- [ ] **S1.4 MIDI file import.** Device writes `.mid` files (`midi_export.rs`) but cannot read them. Done
  when dropping or importing a format 0 or 1 file makes one MIDI clip per track at the project's tempo, with
  tests on small fixtures.
- [ ] **S1.5 Record MIDI controllers** (spec). Sustain, mod wheel and pitch bend are dropped when recording,
  and the piano roll has no controller lanes. Done when they are recorded, drawn and edited in lanes, played
  back to instruments and plugins, and exported to MIDI files.
- [ ] **S1.6 Punch in and out.** Done when a punch range (from the loop brace) limits what a pass records.
- [ ] **S1.7 Crossfades and click-free clip edges.** Done when audio clip edges get a short automatic declick
  and overlapping clips on one track can crossfade.
- [ ] **S1.8 Smaller workflow gaps.** Project templates (File › Save as Template), loops following the song's
  tempo on import, metronome volume and modes, and send-level automation.

## S2: keep it fast and safe to change

- [ ] **S2.1 Compatibility files from every release.** The load tests only change files the current build just
  wrote (`tests/core.rs`). Done when projects saved by 0.1, 0.2 and 0.3 are stored as fixtures that every
  build must open, and a test fails when a built-in parameter id changes.
- [ ] **S2.2 Plugin crash containment and crash logs** (spec). The scanner is out of process, processing is
  not; native crash reports carry only the signal. Done when a plugin crash names the plugin, the next launch
  offers to start with it switched off, a log file sits next to crash reports, and release symbols are kept.
- [ ] **S2.3 App performance benchmarks.** Nothing measures frame time on a large project, open time, startup
  or memory, and the app benches never run in CI. Done when they run and gate in the Performance workflow.
- [ ] **S2.4 Sanitizers in CI.** The ASan/UBSan build exists but no job runs it. Done when a weekly (and
  label-triggered) job runs the engine tests under ASan+UBSan, then TSan.
- [ ] **S2.5 Fuzz the parsers.** WAV, FLAC, MP3, Ogg, `.als` and project JSON are untrusted input. Done when
  each has a fuzz target and a short fuzz run is part of the weekly security job.
- [ ] **S2.6 Undo for every command.** Done when one test applies, undoes and redoes every `Command` variant and
  checks the project comes back the same.
- [ ] **S2.7 Same lint gates on every system, plus cargo-deny.** Clippy runs on Linux only and Rust warnings do
  not fail macOS and Windows builds; a failed weekly audit tells nobody.
- [ ] **S2.8 Split the two largest files.** `main.rs::run()` (about 3,200 lines) and `selftest.rs` (about 19,600
  lines) cause most merge conflicts. Done when both are split by area with no change in behaviour.
- [ ] **S2.9 Reach.** Sign the Windows builds (Azure Trusted Signing) and check the oldest Linux the release
  binary starts on.

## Out of scope

- Effect and instrument DSP, new built-in devices and presets (another contributor).
- More AI features, more pixel parity with the reference design, more Live device conversions, sample rates
  other than 48 kHz, until S0 and S1 are done.

## Open questions

None: the maintainer agreed this order on 2026-11-25.
