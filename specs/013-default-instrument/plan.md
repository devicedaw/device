# Plan: Default instrument for new instrument tracks

Spec: [spec.md](spec.md)

App only. No core, engine, ABI or project-format change.

## App

- `src/prefs.rs`: `MidiPrefs::default_instrument: Option<InstrumentChoice>`, where
  `InstrumentChoice` is `Builtin { id }` (a built-in device id) or `Plugin { plugin: PluginRef }`.
  `None` is Synth. The field reads leniently: a value it can't parse loads as `None`, so a bad
  entry never throws away the rest of the preferences (R6).
- `src/default_instrument.rs` (new): the dropdown entries (Synth, Sampler, Pad Sampler, Drum Rack,
  then the catalog's instrument plugins by name) and `Controller::resolve_default_instrument`,
  which turns the choice into the track name and the instrument device, or Synth with a reason
  when the plugin is missing or the built-in id is unknown.
- `Controller::add_instrument_track` (all empty-track paths go through it, R3): one
  `Command::Batch` "Add Instrument Track" of `AddTrack` and, unless Synth, `SetInstrumentPlugin`.
  Synth issues exactly today's `AddTrack` inside the batch (R4).
- `src/settings.rs`: `Field::DefaultInstrument` (5), `Choice::Instrument` and `Choice::Header`
  (a non-clickable "Plugins" row, drawn as a hint row). Picking saves the preferences at once.
- `ui/settings.slint`: the MIDI page dropdown uses field 5, shows the choice's name and a "Missing"
  badge; the popup is placed under it. `ChoiceList` scrolls when the list is taller than the room
  below it (a long plugin list).

## Principles check

- No audio-thread change. The track and its instrument are added by `Command`s, one undo step.
- Preferences only gain an optional field; old files load (R6). Project files are unchanged.

## Risks

- A long plugin list: handled by capping the popup height and scrolling.
