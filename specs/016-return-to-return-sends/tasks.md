# Tasks: Sends from one return track into another

Plan: [plan.md](plan.md)

Ordered so the tree builds and the tests pass after every task. Each task says how it is verified.
Tick a task only when its verification passed.

- [x] T1. Core: `send_allowed`, `validate_routing` by position, `MoveTrack` refusal, load repair — verify: `cargo test -p dstudio-core --test return_sends`
- [x] T2. Engine: one solo rule for every bus (`SendNode::live`, `apply_routing`, `play`) — verify: `ctest --preset default` (new `routing_return_into_return_*` cases, existing routing and PDC tests unchanged)
- [x] T3. Engine: chained-return PDC and RT-guarded export parity tests — verify: `ctest --preset default`
- [x] T4. Bench: `bench_render` Returns row "+ A into B" — verify: `./build/dstudio_engine_bench` prints it
- [ ] T5. App: `can_send`, `send_rows`, strip ranges, "+" and Sends tab for returns; mixer and Sends tab Slint — verify: `cargo test -p dstudio-app routing`
- [ ] T6. App: refused return move shows the status line — verify: self-test step
- [ ] T7. App: `stem_snapshot` of a return keeps upstream buses; Live set return sends, rendered returns, notes — verify: `cargo test -p dstudio-app live_set dialogs`
- [ ] T8. Review state `review-return-sends` and self-test `return_send_steps` — verify: `--self-test --no-audio` P = N
- [ ] T9. Docs: README, PROGRESS.md, docs/, roadmap G36 — verify: read them against the change
- [ ] T10. Evidence: screenshots and benchmark numbers for the pull request — verify: `spec-verify`
