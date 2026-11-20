# Default instrument for new instrument tracks

Status: Done · Spec: 013 · Issue: none · Roadmap: G8 ([010](../010-parity-roadmap/spec.md))

## Problem

Settings › MIDI shows a "Default instrument" dropdown that always reads "Synth" and does nothing
when clicked. Every new instrument track gets the built-in Synth. Someone who starts most tracks
with the Sampler, the Drum Rack or a favourite instrument plugin has to replace the instrument by
hand after each ⇧⌘T, through the browser.

## Behaviour

- Settings › MIDI › Default instrument opens a list: the built-in instruments (Synth, Sampler,
  Pad Sampler, Drum Rack), then a divider, then the instrument plugins the last scan found, sorted
  by name, each with its format (VST3, CLAP, AU). A check marks the current choice. Picking one
  closes the list and saves it in the app preferences at once. It is an app setting, not a project
  setting, so it is not undoable and not saved with projects.
- A new empty instrument track then plays the chosen instrument: ⇧⌘T, Create › Instrument Track,
  the add-track column and menus in the timeline and the mixer. The new track is named after the
  instrument ("Sampler", "Drum Rack", the plugin's name); with Synth it keeps the name
  "Instrument", as today. Adding the track stays one undo step, "Add Instrument Track".
- The new track is selected and the Devices tab shows its instrument, as when an instrument is
  added from the browser. No plugin window opens on its own (the auto-show switches in Settings ›
  General are for effects).
- If the chosen plugin is no longer installed (not in the last scan), the dropdown shows its name
  with a "Missing" badge, new tracks get the Synth, and the status bar says why. The choice is kept,
  so the plugin comes back as the default when it is installed again and rescanned.
- Default is Synth. Preferences saved before this setting load as Synth.

## Requirements

- R1. The dropdown must list Synth, Sampler, Pad Sampler and Drum Rack, then every instrument
  plugin of the last scan, and mark the current choice.
- R2. Picking an entry must save it in the preferences immediately; it must survive a restart.
- R3. Every path that creates an empty instrument track (⇧⌘T, Create › Instrument Track, the
  timeline and mixer add-track entries) must give the new track the chosen instrument, in one undo
  step labelled "Add Instrument Track". Undo must remove the track and its instrument together.
- R4. With Synth chosen, a new track must be identical to one made today (same name, same
  instrument, same saved project data).
- R5. A chosen plugin missing from the last scan must fall back to Synth for new tracks, show a
  status message, keep the choice, and show "Missing" in the dropdown.
- R6. Preferences without the setting, or with an unknown built-in id, must load as Synth.
- R7. A MIDI clip dragged from the Files browser onto empty space must make its new track with the
  default instrument too ("Add MIDI Clip", one undo step). Paths that already choose an instrument
  (dragging an instrument from the browser, Send to Sampler, Live set import, the assistant) must
  keep their current behaviour.

## Acceptance checks

- A1 (R2, R6): prefs unit test: round trip of each kind of choice; old preferences and an unknown
  id load as Synth.
- A2 (R3, R4): controller unit tests: with Synth, the new track equals today's (name, `None`
  instrument plugin, serialised form); with Sampler, the track is named "Sampler" and holds a
  Sampler; one undo removes it.
- A3 (R5): controller unit test with a plugin choice absent from the scan list: Synth track,
  status text set, preference unchanged.
- A4 (R1, R2, R3): self-test section: open Settings › MIDI, open the dropdown, check the built-in
  entries, pick Drum Rack, close Settings, press ⇧⌘T, check the new track holds a Drum Rack; set
  Synth back.
- A5 (R1, R5): review-state screenshots of the open dropdown and of a missing plugin
  (`review-default-instrument`).
- A6 (R7): `files::tests::a_dropped_midi_clip_gets_the_default_instrument`; the existing tests for
  browser drops and Send to Sampler pass unchanged.

## Out of scope

- A default preset for the chosen instrument (it starts from its own defaults).
- A default effect chain for new audio or instrument tracks.
- A per-project default.

## Open questions

None. Agreed 2026-11-20: tracks are named after the instrument, plugins are listed, a missing
plugin falls back to Synth and the choice is kept.
