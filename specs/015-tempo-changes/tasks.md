# Tasks: Tempo changes during a song

Plan: [plan.md](plan.md)

Ordered so the tree builds and the tests pass after every task. Each task says how it is verified.
Tick a task only when its verification passed.

## PR A: tempo-changes (core, ABI, engine)

- [x] T1. Core `tempo.rs`: `TempoPoint`, `TempoMap`, conversions, `bpm_at`, validation — verify:
  `cargo test -p dstudio-core tempo` (unit tests: round trip, ramp vs numeric integration)
- [x] T2. Model and conversions: `Transport::tempo`, `Project::tempo_map`, conversions through the
  map, unwarped clip lengths, `clip_source_at` / `clip_ticks_at`, warp map with tempo points and
  sixteenths in ramps — verify: `cargo test -p dstudio-core` (existing tests unchanged)
- [x] T3. Commands `SetTempoMap` and `SetTempo { at }` — verify: `tests/tempo_map.rs` commands
- [x] T4. Persistence: schema 2 only with changes, load 1 and 2, repair — verify:
  `tests/tempo_map.rs` persistence
- [x] T5. Snapshot: one map per build, `tempo` segments and `click_quarters`; bit-identical
  without a map — verify: `tests/tempo_map.rs` snapshot tests
- [x] T6. ABI: `se_tempo_segment`, `se_session_desc_v12`, `se_session_publish_v12`, Rust mirror,
  layout tests, `Engine::publish` on v12 — verify: `cargo test -p dstudio-engine abi_layout`
- [x] T7. Engine: `tempo_map.hpp`, graph tempo, `RtTransport` map and tempo per block (live and
  offline), metronome on the map — verify: `ctest` (`test_tempo.cpp`, `test_rt_safety`)
- [x] T8. End to end: the clicks land on the beat frames core computes (4/4 and 6/8) — verify:
  `cargo test -p dstudio-engine --test tempo`
- [x] T9. Benchmarks: "Tempo map" row in `bench_render.cpp`, `benches/tempo.rs` — verify: numbers
  before/after in the PR
- [x] T10. Docs: engine/README.md (formula, gap), docs/architecture.md (schema 2), README status,
  PROGRESS.md — verify: read against the change

## PR B: tempo-lane (app)

- [ ] T11. Lane: "Song › Tempo" in the master picker, drawing in BPM with fitted scale, gestures,
  ramp/hold menu, one `SetTempoMap` per gesture — verify: self-test `tempo_steps`
- [ ] T12. Ruler marks with BPM and ramp arrow; click shows the lane — verify: self-test
- [ ] T13. Transport field at the playhead (display, follow, edit via `SetTempo { at }`) —
  verify: self-test (field text at two positions)
- [ ] T14. Tempo-dependent readouts at the playhead or clip start (device panels, Echo, live
  recording clip, warp on, Smart Quantize) — verify: `cargo test -p dstudio-app`
- [ ] T15. Review state `review-tempo-lane`, screenshots before/after — verify: `--state`
- [ ] T16. Docs: README features, PROGRESS.md — verify: read against the change

## PR C: tempo-export

- [ ] T17. Exports: export length and click transient from the map (Mixdown/Stems render) —
  verify: `crates/dstudio-engine/tests/tempo.rs` export test
- [ ] T18. MIDI export tempo events (ramps as sixteenth steps) — verify: `smf` tests
- [ ] T19. Live set: tempo envelope per point (ramps with sixteenth events), warp markers at
  tempo points, `validate` checks the envelope — verify: `cargo test -p dstudio-app live_set`
- [ ] T20. Docs and close: README, PROGRESS.md, roadmap G37, spec Done — verify: `spec-verify`
