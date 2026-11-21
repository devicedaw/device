# Tasks: Update check and the "Update available" pill

Plan: [plan.md](plan.md)

- [x] T1. `src/updates.rs`: versions, release parsing, notes summary, request, 12-hour reuse, with unit tests —
  verify: `cargo test -p dstudio-app updates`
- [x] T2. Controller state, job result, menu action, view model; launch check in `main.rs` with the self-test
  override — verify: build
- [x] T3. Settings › General switch and preference, search entry — verify: `general_settings_steps`,
  `settings_search` tests
- [x] T4. Slint: header pill, dialog states, settings row — verify: self-test `updates_steps`, `menu_bar_steps`
- [x] T5. Review states `review-update-available`, `review-update-dialog` — verify: screenshots
- [x] T6. Docs: README, docs/building.md, CHANGELOG, PROGRESS.md, roadmap G16 — verify: read them against the change
