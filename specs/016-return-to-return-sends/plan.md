# Plan: Sends from one return track into another

Spec: [spec.md](spec.md)

## Approach

The engine already renders sends between buses (a group is published as a return), refuses loops,
orders the buses and carries delay compensation down a chain. So the work is: let the core accept a
send from a return into a later return, give the engine one solo rule for every bus, show the sends
on return strips, and write them into the Live set.

"Later returns only" is one rule, `Project::send_allowed(from, to)`, used by the core's validation,
by load repair and by the app's `can_send`. Because every send points down the return list, a loop
can't be made, and the only edit that could break the rule is a move: `MoveTrack` checks the order
after the move and refuses with the send's names.

The solo rule is computed when the render graph is built (`apply_routing`), never in the callback.
Each `SendNode` gets a `live` flag next to the track's `send_live`; the callback checks both. With
no bus soloed nothing changes (every send stays live), so sessions without bus-to-bus sends render
bit for bit as today in every mute and solo state.

Rejected: any other return as a target with loops refused (Q1: controls would be disabled for
reasons the user can't see); renumbering returns automatically on a refused move (surprising);
a per-send solo flag in the C ABI (the engine has everything it needs to compute it).

## Changes by layer

- **Core** (`crates/dstudio-core`):
  - `model.rs`: `Project::send_allowed(from, to)` (ordinary or group track into any return; a return
    into a return after it); `Project::return_move_conflict(track, index)` returning the first return send that
    a move would turn upward (for `MoveTrack`); `sanitize_routing` keeps a return's sends into later returns (deduped,
    levels clamped) instead of clearing them.
  - `command.rs`: `validate_routing` takes the position the track has (or will have) in the list and
    accepts a return's sends into later returns; `add_track` passes the insert index. `MoveTrack` of a
    return refuses a move that leaves a send pointing up: "Return B sends into Return C: remove that
    send to move it here". `RemoveTrack` already removes sends into a removed track in one batch.
  - Snapshot: unchanged (it already carries every track's sends; returns are `is_return`).
  - No schema bump: return sends live in the existing `sends` list (`SCHEMA_VERSION` stays 1).
- **Engine** (`engine/src/render_graph.cpp`, `render_graph.hpp`, `processor.cpp`):
  - `SendNode::live` (default true). `apply_routing`: when a bus is soloed, `heard` = soloed buses
    and everything reachable from them through unmuted bus sends; `feeds` = buses that reach a soloed
    bus. A bus plays on the master when heard; its sends run when it is heard or feeds; a send into a
    bus that is heard but neither soloed nor feeding one is live only from a heard bus. With no bus
    soloed, returns stay solo-safe and every send between buses runs (as today).
  - `processor.cpp::play`: skip a send when `!send.live` (its edge line discards, as for `send_live`).
  - Nothing new in the callback beyond one bool test per send.
- **C ABI**: unchanged.
- **App** (`crates/dstudio-app`):
  - `routing.rs`: `can_send` uses `send_allowed`; `send_rows` of a return lists later returns only,
    with `row` = the target's return index, so mixer rows line up across strips; `sends_add_return`
    and `sel_can_send` accept returns. A `send_slot_count` helper feeds the strip ranges.
  - `controller.rs`: strips' `send_count` from the helper; `push_routing` includes return strips.
  - Return rows are pinned above the master row and can't be dragged in the timeline today, so a
    return moves only through `MoveTrack` (scripts, the assistant); its refusal reaches the status
    line through `Controller::execute` ("Edit rejected: ... Return A sends into Return B: remove
    that send to move it here").
  - `ui/mixer.slint`: return strips get the Sends row and section (the "Return" caption goes).
  - `ui/app.slint`: Sends tab of a return: knobs for later returns; no "Return A" placeholder; the
    last return shows a one-line hint.
  - `dialogs.rs`: `stem_snapshot` of a return mutes every bus that neither is it nor feeds it, so its
    render includes upstream returns and no downstream ones; `write_project` treats a return fed by a
    rendered or fed return as fed, writes Live return sends into later unrendered returns, lets a
    rendered return's audio track carry its sends (pre-fader divided by the volume), and lists return
    sends, clamped and rescaled sends in the notes.
  - `live_set.rs`: `SetReturn::sends`; return sends with a level are written active; `validate`
    counts them (`SetSummary::return_sends`).
  - Review states `review-return-sends` (mixer open, Sends expanded, A sending into B, A selected)
    and `review-return-sends-strip` (Return A's Sends tab); self-test `return_send_steps`, and the
    Project export self-test section gains a third return and sends between returns.
- **Docs**: README › Features (mixer / returns), PROGRESS.md entry, `docs/` mentions of "returns
  have no sends", roadmap G36.

## Principles check

1. Real-time safety: the solo sets are computed while building the graph; the callback reads one
   more precomputed bool. Covered by a new RT-guarded chained-returns render test.
2. One mutation path: every send edit is the existing `SetSends` / `MoveTrack` / `RemoveTrack`;
   the refused move goes through `MoveTrack`'s validation.
3. Projects keep loading: no schema change; load repair only drops sends that break the rule.
   Older builds drop return sends on load (their `sanitize_routing` clears them); noted in the
   release notes line in PROGRESS.md.
4. ABI: unchanged.
5. Cross-platform: plain C++ and Rust; engine stays warning-free.
6. Original work: yes.
7. Evidence: before/after screenshots of `review-return-sends`, `bench_render` Returns rows.
8. Docs: listed above.
9. Naming: no new bare `device` identifiers.

## Tests and evidence

- Core (`crates/dstudio-core/tests/return_sends.rs`): accept A→B; refuse self, earlier, group,
  ordinary; remove B and undo; labels and coalescing; moves allowed and refused; old project, round
  trip, hand-edited loop / upward send repaired.
- Engine (`engine/tests/test_routing.cpp`): A→B pre and post; muted A; the three solo cases with
  A→B and with a group feeding a return; export equals live with an RT allocation guard; PDC with
  latent plugins on A and B.
- App unit tests: `send_rows` of returns, `stem_snapshot` of a chained return, Live set XML
  (`live_set.rs`); the written set from a real export is checked in the self-test's Project export.
- Self-test: Sends tab knob, mixer row and tag menu, last return empty, refused move status.
- Benchmarks: `bench_render` Returns section gets "+ A into B" (routing 3) against routing 1.
- Screenshots: `review-return-sends` and `review-return-sends-strip` on the branch, and on
  origin/main with only those two review states patched in (main refuses the A into B send).

## Risks

- The solo rule changes what a group's send does with a return soloed (the fix). Existing tests
  keep passing; new tests pin each case.
- `MoveTrack` inside a group move batch: group moves only move ordinary tracks, so returns are never
  touched.
- A Live set with active return sends has not been opened in Live here (A8 manual check, as spec 006).
