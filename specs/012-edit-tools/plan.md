# Plan: Edit tools

Spec: [spec.md](spec.md)

## Approach

The tool and the Marquee range are view state in the app, like the snap setting. Range edits are built in the core
from existing commands, as one `Command::Batch` each, so no new command variant, schema change or snapshot change is
needed. The one subtlety is ids: a range delete has to split clips at both edges and then remove the parts in between,
whose ids only exist after the splits. `dstudio_core::range` therefore runs the splits on a copy of the project first,
with explicit ids, and reads the inside parts off that copy; the batch it returns names every id, so execution, undo
and redo are exact. A split that fails on the copy (warped or reversed audio, a pattern clip) is dropped, and the clip
then counts as outside the range, so an edit never half-applies.

Rejected: a new `DeleteRange` command (more code to validate and invert, and the batch of splits and removals already
is its exact inverse); letting the batch allocate ids (redo would depend on `next_id`).

## Changes by layer

- **Core** (`crates/dstudio-core`): `range.rs` with `TimeRange`, `Piece`, `copy`, `delete`, `delete_labelled`,
  `split`, `duplicate` and `add_pieces`. Labels: "Delete Range", "Cut Range", "Split at Range", "Duplicate Range".
- **App** (`crates/dstudio-app`): `src/tools.rs` (tool state, menu, T toggle, Marquee drag, Scissors click and hover
  line, range key actions, `push_tools`). `on_lane_pointer` offers presses to `tool_pointer` first. Key actions
  `tool.scissors`, `tool.select_*`, `range.*` and `clip.split_at_range` in `run_action`; `range_selected` in the key
  context; Delete, ⌘X, ⌘C and ⌘L bindings for a range in `app_keymap` (⌘E and ⌘D were already in the core preset).
  Slint: the header button, `ToolMenu`, the range fill and the cut line in `ui/app.slint`; the scissors icon in
  `ui/widgets.slint`. Settings › Shortcuts lists the tool and range rows. Review states `review-tool-menu` and
  `review-marquee-range`; self-test `tool_steps`.
- **Docs**: README › Features › Timeline, PROGRESS.md, roadmap G1.

## Principles check

- One mutation path: every range edit and the Scissors cut are `Command`s through `Controller::execute`; the tool and
  range are view state and never touch the project.
- Projects keep loading: nothing saved changes.
- Real-time safety, the ABI: untouched.
- Original work: the scissors icon is drawn for Device.
- Evidence: core tests, self-test steps, screenshots, `--render-bench`.

## Tests and evidence

- Tests: `crates/dstudio-core/tests/range.rs` (four tests); self-test `tool_steps`.
- Benchmarks: `--render-bench 300` on `reference-default` and `review-marquee-range`, before and after.
- Screenshots: `reference-default` before; `review-tool-menu` and `review-marquee-range` after.

## Risks

- Clips that overlap after a range duplicate: the timeline already allows overlapping clips (⌘D on a clip does the
  same), so this is consistent.
- Cloning the project for each range edit: a discrete edit, measured in microseconds for normal projects.
