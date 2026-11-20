# Parity roadmap: what Device still needs

Status: Draft · Spec: 010 · Issue: none

## Problem

Device is meant to give a musician everything a complete, modern song-making app gives them, with the same
workflow, the same screens and the same feel, down to spacing and wording. Most of that is built: the timeline,
piano roll, step sequencer, mixer, returns and sends, groups, automation, recording, takes, sections and
markers, every built-in effect with its expanded window, device chains, the assistant, export and Live set
import and export. What is left is scattered: a few menus that are shorter than they should be, controls that
are drawn but do nothing (marked `PLACEHOLDER` in `crates/dstudio-app/ui/*.slint`), settings that are greyed out,
and factory content that is thin.

This document is the single list of those gaps, in the order they should be built, so that any contributor or
agent can pick the next item without re-auditing the app. It was written on 2026-11-20 against `main` at
`c66fcf2` (0.2.0 plus #119 to #121), by walking every screen, menu and settings page and comparing it with the
target behaviour.

## How to use this list

- Work top to bottom. P0 items block making a song from start to finish; P1 items are visible differences in
  everyday use; P2 items are polish and content; P3 items are larger features that need a decision first.
- Each item names the area, what is there today, what it must do, and where the code lives. Items marked
  **[devices]** touch built-in device internals or presets, which another contributor owns: coordinate before
  building them.
- Non-trivial items (anything marked **spec**) get their own `specs/NNN-slug/` before code, as
  [specs/README.md](../README.md) says. Small fixes go straight to a pull request.
- Every visible change needs before and after screenshots (see [docs/pr-media.md](../../docs/pr-media.md)),
  every broken behaviour a few small tests, and anything that may cost CPU a benchmark.
- When an item is done, tick it here in the same pull request (`- [x]`) and add the PR number. When a new gap
  is found, add it in the right tier.

## P0: core workflow

- [x] **G1. Edit tools: Selector, Marquee and Scissors** (spec, [012](../012-edit-tools/spec.md), #125). The tool button in the timeline header
  (`ui/app.slint`, the cursor icon with a chevron next to the zoom sliders) is drawn but does nothing, and the
  `tool.scissors` key (T) in `dstudio-core/src/keymap.rs` has no handler. It must open a three-row menu
  (Selector, Marquee tool, Scissors, each with its icon and key) and switch the timeline's pointer mode:
  Selector is today's behaviour; Marquee drags a time range across tracks (the range then works with Cut,
  Copy, Paste, Delete, Duplicate and Split, and with ⌘L to loop it); Scissors splits the clip under the pointer
  at the snapped position on click (one undo step, "Split Clip"). The header icon shows the active tool.
  T toggles Scissors with the previous tool; Escape leaves the tool as it is. (The clip editor header has no tool
  button, in Device or the target, so there is nothing to add there.)
- [x] **G2. Zoom sliders in the timeline header** (#126). The vertical (track height) and horizontal (time) sliders
  next to the tool button are painted rectangles (`ui/app.slint`, "vertical zoom (placeholder)"). Both must drag:
  horizontal zoom follows and sets the real zoom level (`src/zoom.rs`), vertical sets every track's height
  (same steps as ⌥-scroll). Double-click resets. The clip editor's pair works the same way.
- [ ] **G3. Track header menu, complete** (`src/track_menu.rs`). Today: Rename, Duplicate track, Duplicate track
  with clips, Color, Group or Ungroup, Delete track. It must be, in order, with separators between groups:
  Rename… / Duplicate track (⌘D) / Duplicate with clips (⇧⌘D) / Bounce track in place / Show automation (Hide
  automation when shown) / Group track (⌘G) / Delete track (Del) / Audio track (⌘T) / Instrument track (⇧⌘T) /
  Sampler track / Drum track / Return track / and an inline colour grid at the bottom (see G5). The add-track
  rows insert the new track below the clicked one. Keys shown in the menu must be the ones that work.
- [ ] **G4. Bounce track in place** (spec). Greyed in the clip Bounce submenu (`src/clipedit.rs`, `BOUNCE`) and
  missing from the track menu. Renders the whole track through its devices to audio, replaces its clips with
  the rendered audio, and turns an instrument track into an audio track, keeping name, colour, sends, output
  and automation of volume and pan; the instrument and effects are kept but switched off so the change can be
  undone and redone as one step ("Bounce Track in Place").
- [ ] **G5. Track colours: a full palette shown inline.** The track menu has a 13-colour submenu. It must show a
  grid of 40 colours (13 per row, 3 full rows and one) directly at the bottom of the menu, with a check on the
  current one, and new tracks cycle through the same palette. Saved projects keep their colour index, and old
  indexes map to the same colour.
- [ ] **G6. Empty lane menu** (`open_lane_menu` in `src/clipedit.rs`). Right-clicking empty space in a lane
  only offers Insert MIDI clip and Paste. It must start with "Open <instrument name>" on instrument tracks
  (opens the Devices view on that instrument), then "Insert empty MIDI clip (⇧⌘M)" and Paste when there is
  something to paste, then the whole track menu from G3.
- [ ] **G7. Add track menu** (`open_add_track_menu` in `src/routing.rs`, timeline and mixer). Today: Audio,
  Instrument, Return, Group. It must list Audio track (⌘T), Instrument track (⇧⌘T), Sampler track, Drum track,
  Return track, with the keys shown. Sampler track and Drum track are instrument tracks that start with the
  Sampler or the Drum Rack instead of the default instrument. Groups are made from selected tracks with ⌘G or
  the track menu (see open question Q1 about the Group track row).
- [x] **G8. Default instrument setting works** (#128, spec 013). Settings › MIDI › Default instrument is a placeholder (new
  instrument tracks always get the Synth; `ui/settings.slint`, MidiPage). It must list the built-in instruments
  and installed instrument plugins, be saved as a preference, and be what Instrument track (⇧⌘T), the add-track
  menu and dropped MIDI files create.
- [ ] **G9. Note audition in the piano roll.** Clicking, drawing or dragging a note (pitch change) and clicking a
  key on the keyboard must play the note through the track's instrument for its length (capped), without
  recording anything. The README status table lists this as missing.
- [ ] **G10. Rename the project from the title bar.** Clicking the project name in the window title turns it
  into a text field (Return saves, Escape cancels). For a saved project it renames the project file and its
  folder on disk; for an unsaved one it sets the name used by the first save. Undo is not needed (it is a file
  operation), but a failed rename must say why and leave the name as it was.
- [ ] **G11. Recording offset per audio device.** Settings › Audio needs a manual recording offset (ms, per input
  device, saved with the device's preferences) for interfaces that report latency wrongly. The README lists it
  as missing.

## P1: everyday UI and behaviour

- [ ] **G12. Help on every setting.** Each settings row has a small (i) button whose hover shows one sentence
  explaining the setting ("Restore the last session on launch.", "Plugin loaded on new instrument tracks.",
  "Folders scanned for VST3 and AU plugins.", …). Device's settings pages have none. Add the icon and tooltips
  to General, Audio, MIDI, Plugins and Privacy.
- [x] **G13. Settings search** (spec 014). The search box at the top of the settings sidebar is drawn but does nothing
  (`ui/settings.slint`, "PLACEHOLDER: no settings search yet"). Typing must filter the sidebar to pages with
  matching rows, and highlight the matching rows on the open page. Escape clears.
- [ ] **G14. Settings › Privacy page.** Today "Not available yet". It must hold "Send crash reports" (on by
  default; sends a crash report through the same service as Report a bug, with nothing personal) and "Usage
  tracking" (off; Device collects nothing, so the row explains that and stays off), each with help text.
- [ ] **G15. Settings › Downloads page.** Today "Not available yet". It must list optional downloadable content
  (the factory sound library for the multi-sample instrument in G27, when it exists) with its version and size,
  and a Download / Re-download button with progress. Until there is downloadable content, the page says so.
- [ ] **G16. Real update check and an "Update available" button.** Options › Check for Updates… only says
  "There's no update server yet". It must compare the running version with the latest GitHub release (once at
  launch, quietly when off-line, and from the menu), and when a newer one exists show a blue "Update available"
  pill in the title bar left of Report a bug. The pill and the dialog show the new version and open its
  download page.
- [ ] **G17. Ask from a clip (⌘K).** The clip menu's Ask row is disabled (`src/clipedit.rs`, "PLACEHOLDER entry
  (disabled): Ask"), and `assistant.ask` (⌘K) is not handled. It must open the assistant panel with the clip
  set as its context chip and the cursor in the composer.
- [ ] **G18. Assistant: Getting started tour.** On first launch the assistant panel shows a "Getting started"
  card ("1 of 4", progress bar, Continue, collapse and close) that walks a new user through four steps with a
  coach mark: a highlight ring around the area and a popover with the step's title, one line of help, Back,
  Next and close. Device's steps: find a sound in Files and preview it, drag it to the timeline, add an
  instrument track and draw notes, play and export. The card remembers its step; closing it hides it for good
  (a preference), and it can be restarted from the assistant's menu.
- [ ] **G19. Assistant: voice input.** The mic button and the General › Voice rows (Talk key, Hold talk key to
  record, Auto-send, Language) are placeholders, and Settings › Audio has no "Assistant voice input" device row
  (Same as input device by default). Spec needed: a local or provider speech-to-text that respects privacy.
- [ ] **G20. Assistant: "Let the assistant listen to audio".** Greyed today. When on, a turn may include a short
  analysis of the selected clip or the mix (loudness, spectrum summary, key and tempo guess) so answers can refer
  to the sound. Off by default if it sends anything off the machine.
- [ ] **G21. Assistant: full-height layout.** The header's panel button is a placeholder ("Full height (not
  implemented)"). It must toggle the panel between the timeline row only and the full window height (over the
  bottom panel).
- [ ] **G22. Assistant: works for normal users and knows the devices.** The key can only come from the
  `ANTHROPIC_API_KEY` environment variable, which a Finder-launched app never sees; it has never been run
  against the live API; and the model is not told about device chains, so it cannot add effects or change their
  parameters, sends, automation, the Sampler or the Drum Rack, or create audio clips. Add a key field (stored
  in the system keychain), describe chains and devices in the state and tool schema, test against the live API
  once, and update the README status row.
- [ ] **G23. Smart Fill** (spec). The Smart Fill pill in the timeline header is a placeholder. It toggles a mode
  in which selecting an empty time range on a track offers generated material that fits the surrounding clips
  (pattern for drum tracks, notes for instrument tracks, from Device's own pattern library in
  `dstudio-core/src/library.rs`). Needs a decision on scope (Q2).
- [ ] **G24. Export: convert effects for Live sets.** In File › Export › Project, "Convert effects to Live
  devices" is a placeholder; effects are always rendered. Map the built-in effects that have a close Live
  device (EQ, compressor, delay, reverb, utility…) to that device with matching settings, and render the rest.

- [x] **G36. Sends between return tracks** (#132, spec 016). A return sends into the returns after it, from the
  mixer's Sends section and its Sends tab, with one solo rule for every bus and the sends kept in the Live set.

### Visual differences in the main window

Found by putting Device's review states (`--state reference-default`, `reference-mixer`, `reference-pianoroll`)
next to the target on the same window size.

- [ ] **G30. I (input monitoring) on audio track headers.** Audio track headers show M S R; only instrument
  headers have I (`ui/track_header.slint`, `if root.t.instrument`). Every recordable track shows M S R I in the
  header, as the channel strip already does.
- [ ] **G31. A second "Add track" row below the last track.** The track list has "+ Add track" only at the
  top. There is a second one in the empty space below the last track (above Master), opening the same menu
  (G7), and adding the track at the end.
- [ ] **G32. Default track colours.** A new project's first track is pink and the second orange. The first
  track (audio) should be teal and the second (instrument) orange, and new tracks continue through the palette
  from there (ties in with G5).
- [ ] **G33. Piano roll loop bar in the clip's colour.** The loop bar above the piano roll is a pale pink on an
  orange instrument clip. It must be the track colour at full strength, like the clip header in the timeline.
- [ ] **G34. Shape tab available on an empty MIDI clip.** In the clip sidebar the Shape tab is greyed until the
  clip has notes; it should always be selectable for a MIDI clip (the rack just has nothing to shape yet).
- [ ] **G35. Instruments in the Devices browser open into their presets.** Devices › Instruments lists the
  built-in instruments as plain rows under "Built-in". Each instrument with factory presets is a folder
  (with the preset count on the right) that opens into category folders (with counts) and then presets;
  clicking a preset previews it, dragging or double-clicking adds the instrument with that preset. Depends on
  G25 for the Synth's content.

## P2: content and polish

- [ ] **G25. Synth factory library** [devices]. The Synth has 12 named sounds and numbered placeholders
  (`dstudio-core/src/builtin/synth_v2/mod.rs`, `PRESETS`). The browser's Devices › Instruments › Synth folder
  must hold about 100 original sounds in categories (Bass, Keys, Pluck, Lead, Pad, Brass, and the rest), each
  folder with its count, and the device header's preset arrows step through them.
- [ ] **G26. Factory chains** (`dstudio-core/src/chains.rs`). 27 chains today. Grow to about 85 original
  chains across the eight folders (Vocals, Drums, Bass, Guitar, Keys & Synths, Pads & Textures, FX & Glitch,
  Mix & Master), and rename the "FX" folder to "FX & Glitch". Keep saved user chains loading.
- [ ] **G27. Multi-sample preset instrument** [devices] (spec). A fifth built-in instrument that plays a
  library of a couple of hundred multi-sampled sounds (keys, strings, brass, guitars, vocals, textures), with
  its own preset folder in the browser and its sound library as optional downloadable content (G15). All
  samples must be original or properly licensed for redistribution.
- [ ] **G28. Device-chain rows in Settings › Shortcuts.** The Shortcuts page lists every timeline and editor
  key, but not the device-chain ones (move, delete, duplicate, fold, toggle a device). Add them, bindable like
  the others.
- [ ] **G29. Windows: interface labels render as empty boxes** (issue #115). The last report is that preferring
  Direct3D 12 (#117) likely fixes it; confirm on the Windows test PC and close the issue, or fix the font
  loading if it still happens.

## P3: needs a decision first

- **Online sample catalogue.** The browser's Search and Library sections, the Link button in the preview bar,
  and Fits / Similar / Variations on audio clips depend on an online sample catalogue that Device does not
  have. They stay as they are (empty sections and disabled controls) unless Device gets its own catalogue.
- **Accounts, sample licensing and AI usage units.** Settings › Account, Usage and the "License samples when
  you use them" switch only make sense with an online service. Out of scope; hide or keep greyed (Q3).

## Requirements

- R1. Every item above is either built (ticked, with its PR), moved to P3 with a reason, or dropped with a
  reason. No item is ticked without screenshots and tests as the project's PR rules require.
- R2. Built items must not slow the timeline, the audio thread or exports measurably (benchmarks where the
  item touches them: G1, G2, G4, G9, G23).
- R3. This file stays the one place the remaining gaps are listed; the README status table links here.

## Acceptance checks

- A1 (R1): every P0 to P2 line is `- [x]` with a PR number, or moved or dropped with a line saying why.
- A2 (R2): `dstudio_engine_bench` and the app's render bench before and after G1, G2, G4, G9 and G23.
- A3 (R3): the README Status section links to this spec.

## Out of scope

- Device internals of the existing built-in effects and instruments (their DSP and cards), which their owner
  maintains; only the items marked [devices] above.
- Platform work that is not about matching behaviour: notarisation, signed Windows packages, ASIO and
  exclusive mode on Windows, testing ALSA on a physical Linux sound card.

## Open questions

- Q1. Device's add-track menu has a "Group track" row that the target does not; groups are made from a
  selection. Keep it as an extra, or remove it for parity?
- Q2. Smart Fill (G23): build it on Device's own pattern library, wait for the assistant work (G22), or drop?
- Q3. Account, Usage and sample licensing: hide those settings pages and rows, or keep them greyed?
