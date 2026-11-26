# Warp marker editing

Status: Done · Spec: 027 · Issue: none

## Problem

Warp markers in the audio clip editor were hard to use. A single click in the handle band added a
marker, and a single click on a marker removed it, so markers came and went by accident. Markers
were drawn as flags in the band's upper row, next to the clip's start and end flags, while the pins
under the clip's handles looked like markers but did nothing. Transients were lines through the
waveform lane, and a press on one placed a marker even without a drag.

## Behaviour

The handle band under the loop bar has two rows. The upper row holds the clip's start and end flags.
The lower row holds the warp markers and the transients.

- A warp marker is a pin (rounded top, pointed bottom) in the clip colour. A warped clip without
  markers shows two, at its start and its end; the first edit makes them part of the clip.
- Transients are short grey ticks at the bottom of the lower row.
- Hovering a marker lights it and drops a thin line through the lane.
- Double-click in the lower row adds a marker there, on a transient when one is under the pointer.
  Undo step: "Add warp marker" (or "Add warp marker at transient").
- Drag a marker to move it. It stops at its neighbours. Undo step: "Move warp marker".
  Shift-drag slides the audio under the marker instead.
- Press a transient and drag: a marker is placed on the transient and follows the drag, one undo
  step "Add warp marker at transient". Cmd (Ctrl) also pins the transients either side. Alt ignores
  transients.
- Double-click a marker, or right-click it, to remove it. Undo step: "Remove warp marker".
- A single click on a marker, on a transient or on an empty spot of the row changes nothing.
- In the lane (warp on) the transients also show as faint lines: drag one to move the transient,
  double-click one to remove it, double-click elsewhere to add one ("Move transient", "Remove
  transient", "Add transient"). These corrections feed marker snapping and Drums/Auto slicing.
- Hovering a transient tick shows a grey ghost marker where a press would place one.
- Right-click in the lane or on an empty spot of the marker row (warp on) opens a menu with
  Insert Transient (enabled away from a transient) and Delete Transient (enabled on one).
- A press in the ruler, the upper row or an empty spot of the lane moves the playhead.
- With warp off the markers stay visible, dimmed, and cannot be edited.

## Requirements

- R1. Markers are drawn as pins in the lower row; transients as ticks in the same row; no pins
  under the clip handles apart from markers.
- R9. Lane transient editing (move, add, remove) keeps working with warp on.
- R10. A ghost marker shows over a hovered transient; right-click offers Insert/Delete Transient.
- R2. A warped clip without markers shows markers at its start and end.
- R3. Double-click adds a marker (snapping to a transient within reach, unless Alt).
- R4. Drag moves a marker, clamped between its neighbours; Shift slides the audio.
- R5. Press-drag on a transient places a marker there and drags it, one undo step; a click alone
  does nothing.
- R6. Double-click or right-click on a marker removes it; a single click does not.
- R7. Hover lights the marker under the pointer and shows its line through the lane.
- R8. Warp off: markers dimmed, no marker edits.

## Acceptance checks

- A1 (R2, R3, R6, R7): self-test steps in the warp section: two markers shown before any edit, a
  click adds nothing, a double-click adds, hover lights, a click keeps, double-click and right-click
  remove.
- A2 (R4, R5): self-test Shift-drag, Cmd-drag on a transient and press-drag on a transient steps;
  unit test `a_dragged_marker_stops_at_its_neighbours`.
- A3 (R2): unit test `a_clip_without_markers_is_anchored_at_its_window_start_and_end`.
- A5 (R10): self-test steps for the ghost and both menu items.
- A4 (R1, R7, R8): `review-warp`, `review-warp-hover` and `review-warp-off` screenshots.

## Out of scope

- Warp markers on the timeline's clips.

## Open questions

None.
