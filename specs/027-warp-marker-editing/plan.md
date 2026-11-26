# Plan: warp marker editing

All changes are in the app (`crates/dstudio-app`). The model (`ClipWarp`, `Command::SetClipWarp`)
does not change, so projects and undo are unaffected.

- `warpedit.rs`: `MARKER_ROW` is the band's lower row. `anchored` pins the window's start and end
  when a clip has no markers; `ac_marker_beats` shows those (and shows markers with warp off, mapped
  1:1). `ac_pointer` gains hover (3), leave (4) and right-press (5) kinds and the gesture rules from
  the spec. A press on a transient becomes a marker drag on the first move.
- `pianoroll.rs`: draw transient ticks and marker pins in the lower row, the hovered pin lighter with
  a line through the lane, dimmed pins with warp off; drop the decorative handle pins. The lane's transient
  lines stay (they are dragged to correct a transient). `PrState::ac_hover` keeps the hovered marker.
- `pianoroll.slint`: send hover, leave and right-press to `pr-pointer` while the audio clip editor
  is open.
- Review states `review-warp-hover` and `review-warp-off` for the screenshots.

Principles: every edit is still one `SetClipWarp` command with a named undo step; nothing touches the
engine or the audio thread. Hover only redraws when the hovered marker changes.

Risk: one more pointer event per mouse move over the clip editor. It is a hit test over the
markers shown (a handful) and returns without a redraw when nothing changes.
