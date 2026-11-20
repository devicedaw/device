# Plan: Bounce Track in Place

Spec: [spec.md](spec.md)

## Approach

Reuse the clip render pipeline (`src/clip_render.rs`): the span is the whole track (`render_plan::track_span`),
rendered in `RenderMode::Bounce` (devices on, neutral fader and pan, channel lanes off) on the worker with a private
offline engine. The landing builds one batch: `AddAsset` and a new `ReplaceTrack` with the bounced track
(`render_plan::bounced_in_place`).

A track that changes kind needs a command: removing and re-adding it would also drop references other tracks hold to
it, so `ReplaceTrack` swaps it in place, with the same id and index, and returns the old track as its inverse. It is
validated as an added track would be, against the project without the old version, so the new version may reuse its
ids.

Rejected: `RemoveTrack` + `AddTrack` (loses references, moves ids); a `SetTrackKind` command (the clips and devices
change too, so it would be a replace in all but name).

## Changes by layer

- **Core**: `Command::ReplaceTrack` (label "Replace Track", used inside the batch labelled "Bounce Track in Place");
  `render_plan::track_span` and `render_plan::bounced_in_place`. No model or schema change.
- **App**: `ClipOp::BounceInPlace`, `Controller::bounce_track_in_place`, the landing (with the check that the track's
  clips did not change), the clip menu's Bounce submenu entry enabled. Self-test `bounce_in_place_steps`.
- **Docs**: README › Printing clips, PROGRESS.md, roadmap G4.

## Principles check

- Real-time safety: nothing new on the audio thread; the render is offline on a worker.
- One mutation path: one `Command::Batch` through `Controller::execute`.
- Projects keep loading: no saved shape changes; `ReplaceTrack` only appears in history and scripts.
- Evidence: tests, screenshot, render time.

## Tests and evidence

- Tests: `tests/bounce_in_place.rs` (span and bounced track, exact undo/redo and JSON round trip, refusals); self-test.
- Benchmarks: the engine is unchanged; the self-test prints the render time.
- Screenshots: `review-clip-bounce-menu` before and after.

## Risks

- A long track renders a long file: the render is in the background with progress and can be cancelled.
