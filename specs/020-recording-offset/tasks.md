# Tasks: Recording offset per audio device

Plan: [plan.md](plan.md)

- [x] T1. Engine: the offset in `process()`, `se_record_set_offset` / `se_record_get_offset`, engine tests — verify: `ctest --preset default -R record_offset`
- [x] T2. Rust wrapper and end-to-end test — verify: `cargo test -p dstudio-engine --test recording`
- [x] T3. Preferences per input device, applied at launch and on device change — verify: `cargo test -p dstudio-app prefs`
- [x] T4. Settings › Audio row, search entry, review state — verify: `settings_search` tests, `review-settings-recording-offset`
- [x] T5. Self-test `recording_offset_steps` — verify: `--self-test --no-audio`
- [x] T6. Docs: README, engine/README.md, ADR 0002, PROGRESS.md, roadmap G11 — verify: read against the change
- [x] T7. Evidence: screenshots before and after — verify: `spec-verify`
