# Smart Fill

Status: Done · Spec: 021 · Issue: none · Roadmap: [010 › G23](../010-parity-roadmap/spec.md)

## Problem

The timeline header's Smart Fill pill does nothing. Musicians often know where a part should go before
they know what it is: they want to mark a gap in the arrangement and have Device fill it, or record into
exactly that gap.

## Behaviour

- **Smart Fill** in the timeline header switches a mode on and off; the pill is lit while it is on.
- In the mode, dragging over empty space on a track's lane marks that time range on that track and opens
  the Smart Fill box under it. Escape, × or a click outside closes it and leaves the mode on.
- The box shows the track (its icon and name) and two tabs, **Create** and **Record**.
- **Create** (instrument tracks):
  - a "Describe a musical part…" field and a **Create** button;
  - **Type**: Any, Chords, Bass, Lead, Arp; **Genre**: a searchable list of the pattern library's genres.
  - With the field empty, Create fills the range at once with a MIDI clip from Device's own pattern
    library for the Type and Genre (Any picks Chords), in the project key; the clip's Patterns tab lists
    the rest of the set. One undo step, "Smart Fill".
  - With a description, Create asks the assistant (Assist mode) for that part, naming the track, the
    range, the Type and the Genre; the part arrives as the assistant's one undo step. Without an API key
    the assistant panel opens on its key card.
- **Record**: **Typing** (musical typing on or off), **Count-in** and **Loop** switches and a red
  **Record** button. Record sets the range as the punch range (looped when Loop is on), arms the track
  and starts recording with or without the count-in.
- On an audio track the box opens on Record; Create is shown but greyed (there is no audio library).

## Requirements

- R1. The mode, the range and the box are view state; only Create and Record change the project.
- R2. A Create from the library is one `Command::Batch` labelled "Smart Fill" adding one MIDI clip that
  spans the range exactly, with its `Composer` set so the Patterns tab can swap the pattern.
- R3. A Create with a description goes through the assistant like a typed request, so its edits are
  validated and undoable the same way.
- R4. Record uses the existing count-in, loop and recording paths; nothing new runs on the audio thread.
- R5. Old projects are unaffected (no file format change).

## Acceptance checks

- A1. Core/unit: the library fill for each Type spans the range and stays inside it.
- A2. Self-test: Smart Fill on, drag a range on the instrument track, the box opens with the track's name;
  Create with an empty field adds one clip over the range in one undo step labelled "Smart Fill"; undo
  removes it; Escape closes the box.
- A3. Review states for the box (Create, Type menu, Record).

## Out of scope

Audio generation, filling several tracks at once.

## Decided questions

- Q1. Audio tracks: Record only (Create greyed), as there is no sample library to fill from.
- Q2. Type Any with an empty field picks Chords; with a description the assistant decides.
- Q3. The pill is lit while the mode is on, so it is clear why a drag on an empty lane opens the box.
- Q4. Record starts recording from the range's start; it does not stop by itself at the range's end
  (Device has no punch-out yet). With Loop on, the range becomes the loop, so the passes become takes.
