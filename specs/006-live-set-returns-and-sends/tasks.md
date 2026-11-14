# Tasks: Return tracks and sends in the Live set export

Plan: [plan.md](plan.md)

- [x] T1. Set writer: returns, sends, Sends Only, pre/post list, structure check — verify: `cargo test -p dstudio-app live_set`
- [x] T2. Exporter: send levels, return renders, notes; `stem_snapshot` for returns — verify: `cargo test -p dstudio-app dialogs`
- [x] T3. Self-test: Project export with returns and sends — verify: self-test
- [x] T4. Docs: README, PROGRESS.md — verify: read them against the change
- [x] T5. Open an exported set in Live 11.3 or later and confirm the returns, sends and Sends Only routing (needs a machine with Live) — checked in Live 12 on Nov 15 2026: returns, sends, Sends Only and the rendered return load; samples needed the "Ableton Project Info" folder (fixed)
