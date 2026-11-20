# Bounce Track in Place

Status: Done · Spec: 017 · Issue: none · Roadmap: [010 › G4](../010-parity-roadmap/spec.md)

## Problem

A finished instrument part, or an audio track with heavy effects, costs CPU every time the song plays, and its sound
changes whenever a preset or plugin does. Musicians print such tracks to audio so they are fixed and cheap. Device has
Bounce to new track for one clip, but no way to print a whole track where it is: the clip menu's "Bounce track in
place" is greyed and the track menu has no such row.

## Behaviour

- Clip menu › Bounce › **Bounce track in place** (and, once the track menu is complete, the track menu row of the same
  name) prints the clip's whole track.
- Every enabled clip of the track is rendered through its devices, from the first clip's start to the last clip's end,
  plus the tail while the sound rings out (as Bounce to new track does). The render is taken before the channel strip
  (fader, pan, trim, width, mono and their lanes are not printed), because the track keeps them.
- The track becomes an audio track holding one clip, the render, from the first clip's start. It keeps its id, place,
  name, colour, fader, pan, trim, width, mono, sends, output and automation lanes. Its effects and its instrument stay
  on the track, switched off, since their sound is now in the clip. An instrument track's MIDI clips and MIDI input go.
  Disabled clips are not rendered and go too.
- The render runs in the background with the progress line, like the other clip renders; Escape or Cancel stops it and
  leaves nothing. If the track's clips change while it renders, the render is discarded with a message.
- One undo step, "Bounce Track in Place". Undo brings the track back exactly as it was (kind, clips, devices on).
- A track with no enabled clips says "Bounce: the track has no clips to bounce" and changes nothing.

## Requirements

- R1. The track becomes an audio track with one clip, the render, from its first clip's start, keeping name, colour,
  mix, sends, output and lanes; its devices and instrument stay, off.
- R2. One undo step, "Bounce Track in Place"; undo and redo are exact.
- R3. A new `ReplaceTrack` command swaps a track for a new version of itself and refuses what a track cannot be (an
  audio track with MIDI clips, a clip id in use elsewhere, a missing track, return and group tracks).
- R4. Clips that change while the render runs discard it; a cancel leaves no file, asset or undo step.
- R5. The render costs nothing on the audio thread or in playback: it runs on the existing offline render path.

## Acceptance checks

- A1 (R1, R2, R3): core tests `crates/dstudio-core/tests/bounce_in_place.rs`.
- A2 (R1, R2): self-test `bounce_in_place_steps` (instrument track to audio track, undo restores it).
- A3 (R4): covered by the shared clip render path's existing self-test checks (changed sources, cancel).
- A4 (R5): no engine change; the render time is printed by the self-test.
- A5: screenshot of the clip menu's Bounce submenu before and after (`review-clip-bounce-menu`).

## Out of scope

- The track menu row: added with the complete track menu (roadmap G3).
- Freezing (a reversible print that keeps the MIDI clips editable).

## Open questions

None. Decided while building (listed in the pull request): the bounced clip is named after the track; device lanes
are kept with their switched-off devices rather than dropped.
