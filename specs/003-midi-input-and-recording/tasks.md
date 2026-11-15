# Tasks: MIDI input and recording

Plan: [plan.md](plan.md)

- [x] T1. Engine: `MidiInputHub`, platform backends (Core MIDI, WinMM, ALSA, stub), CMake — verify: builds on all three CI platforms
- [x] T2. Engine: routing in `render_block`, `TrackMidi`, `LiveSynth`, held-note release — verify: A2, A3
- [x] T3. Engine: capture ring, latency compensation and loop fold — verify: A5
- [x] T4. C ABI + Rust wrapper + layout tests — verify: `cargo test -p dstudio-engine`
- [x] T5. Bench: MIDI input case in `dstudio_engine_bench`, numbers before and after — verify: A8
- [x] T6. Core: arm/monitor on instrument tracks, `midi_record` — verify: A4
- [x] T7. App: R/I, recording pass, live clip, focus, Settings › MIDI, preferences — verify: A6, A7
- [x] T8. Docs: README, PROGRESS.md, architecture, engine README, building — verify: read them against the change
- [x] T9. Evidence: screenshots and bench numbers in the pull requests; manual Core MIDI check on a Mac (MacBook Air M4, USB MIDI controller, Nov 15 2026: all six checks in #30 passed) — verify: `spec-verify`
