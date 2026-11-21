# Edit tools: Selector, Marquee and Scissors

Status: Done · Spec: 012 · Issue: none · Roadmap: [010 › G1](../010-parity-roadmap/spec.md)

## Problem

The timeline header has a tool button (a pointer icon with a chevron, left of the zoom sliders), but
it is only a picture: clicking it does nothing. The T key is bound to `tool.scissors` and has no
handler. So there is no way to cut a clip at the pointer, and no way to select a stretch of time
across several tracks and act on it (delete a bar from every track, duplicate a section, loop it).

## Behaviour

- Clicking the tool button opens a menu with three rows, each with its icon: **Selector** (a
  pointer), **Marquee tool** (a cross) and **Scissors**. The active tool has a check on the right.
  Rows highlight on hover. Picking a row switches the tool and closes the menu; clicking outside
  closes it. The header button shows the active tool's icon.
- **T** toggles Scissors: from any other tool it switches to Scissors, and from Scissors back to the
  tool used before. Escape does not change the tool (it keeps closing menus and cancelling
  gestures, as today).
- **Selector** is today's behaviour, unchanged.
- **Marquee tool**: pressing anywhere in the lanes (on a clip or on empty space) and dragging selects
  a time range across the tracks the drag covers. Both edges snap to the grid when snap is on. The
  range is drawn as a light fill over those tracks' lanes, and the playhead moves to its start.
  A click without a drag clears the range and moves the playhead, as a click on an empty lane does.
  With a range selected:
  - Delete (or Backspace) removes everything inside the range on those tracks: clips crossing an
    edge are split there and the inside part removed. One undo step, "Delete Range".
  - ⌘X does the same after copying the inside to the clipboard ("Cut Range"). ⌘C copies it.
  - ⌘V pastes the clipboard at the playhead (snapped to the beat), keeping the copied range's layout
    across tracks: what started at the range's start lands at the playhead. One undo step.
  - ⌘D copies the inside right after the range (at its end) and moves the range there, so repeated
    ⌘D keeps extending the pattern. "Duplicate Range".
  - ⌘E splits every clip that crosses either edge, leaving the range's content as separate clips.
    "Split at Range".
  - ⌘L sets the loop to the range and turns looping on ("Set Loop").
  - Switching tools or pressing Escape clears the range.
- **Scissors**: hovering a clip shows a thin vertical line at the snapped position; clicking splits
  the clip under the pointer there, and the right part becomes the selected clip. One undo step,
  "Split Clip". Clicking empty space does nothing. Clips that can't be split (warped or reversed
  audio, pattern clips, a cut at the clip's edge) say why in the status bar, as ⌘E does.
- The tool is view state: it is not saved with the project and every launch starts with Selector.

## Requirements

- R1. The header tool button opens a three-row menu (Selector, Marquee tool, Scissors, each with an
  icon) with a check on the active tool; picking a row sets the tool; the header shows its icon.
- R2. T toggles between Scissors and the previous tool. Escape leaves the tool as it is.
- R3. A Scissors click on a clip splits it at the snapped pointer position in one undo step,
  "Split Clip", and selects the right part; refused splits leave the project unchanged.
- R4. A Marquee drag selects a snapped time range over the tracks it covers, drawn over their lanes.
- R5. Delete, Cut, Copy, Paste, Duplicate, Split and ⌘L act on the range as described, each as one
  undo step with the label given, and undo restores the project exactly.
- R6. Clips that can't be split at a range edge (warped or reversed audio, pattern clips) are left
  whole: a range operation never half-applies.
- R7. Selector behaviour and existing shortcuts are unchanged when no range is selected.
- R8. Drawing the tool state costs nothing measurable per frame.

## Acceptance checks

- A1 (R5, R6): core tests in `crates/dstudio-core/tests/range.rs`: delete, duplicate and split of a
  range over an audio and a MIDI track, each undone to the exact project; a warped clip is kept
  whole; the labels.
- A2 (R1, R2, R3, R4, R5): app self-test steps: open the tool menu and pick Scissors, T back to
  Selector, a Scissors click splits a clip (one undo step), a Marquee drag over two tracks followed
  by Delete removes the inside, ⌘Z restores it.
- A3 (R1, R4): screenshots of `review-tool-menu` and `review-marquee-range`.
- A4 (R7): the existing self-test passes unchanged.
- A5 (R8): `--render-bench` before and after.

## Out of scope

- A tool button in the clip editor header: the clip editor has none (its header holds the draw and
  velocity buttons and the zoom sliders), so the roadmap's note about it is corrected.
- Range selection inside automation lanes, and dragging a range's content to move it.
- A dedicated key for the Marquee tool; it is picked from the menu.

## Open questions

None. Decided while building (listed in the pull request): Escape leaves the tool as it is and T
toggles Scissors with the previous tool, matching the target; pasting a range anchors at the range's
start rather than the first clip.
