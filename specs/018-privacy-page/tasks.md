# Tasks: Settings › Privacy

Plan: [plan.md](plan.md)

- [x] T1. Feedback service accepts `crash` — verify: `node --test services/feedback/test/*.test.mjs`
- [x] T2. Preferences field and `crash.rs` (record, take, scrub, payload) with unit tests — verify: `cargo test -p dstudio-app crash`
- [x] T3. Panic hook and Unix signal handlers, installed only with the preferences; send at launch — verify: `DSTUDIO_CRASH_TEST=panic` and `segv`, then a launch with `DSTUDIO_FEEDBACK_URL` at a local server
- [x] T4. Hide Account, Usage and sample licensing; seven pages; search indices — verify: `cargo test -p dstudio-app settings_search`
- [x] T5. Privacy page, switch, help, search entries — verify: self-test section `privacy_steps`
- [x] T6. Review state `review-settings-privacy` — verify: screenshot
- [x] T7. Docs: README, PROGRESS.md, services/feedback/README.md, roadmap G14 — verify: read them against the change
