# Plan: Smart Fill

## App (crates/dstudio-app)

- `smart_fill.rs` (new): `SmartFill` (mode, drag, `FillBox`) on the controller. `smart_pointer` runs
  before the edit tools on lane presses: in the mode, a press on an empty track row starts a one-track
  range (drawn with the range overlay, outlined); the release opens the box (Record first on audio
  tracks). Menus, the description, Type, Genre, the Record switches, Create and Record.
- Create from the library: `library::create_set` for the part and genre, in the project key, at the
  range's length; the first pattern becomes one `AddMidiClip` in a "Smart Fill" batch, with the clip's
  `Composer` set to that set so the Patterns tab can swap it.
- Create with a description: an Assist-mode request through `assistant_send` that names the track, the
  range in ticks and bars, the Type and the Genre.
- Record: arm the track, optionally `SetLoop` to the range, seek to its start, start recording (the
  count-in follows the Count-in switch, which is the app's count-in setting).
- `ui/app.slint`: the pill toggles the mode; `SmartFillBox` and the Type and Genre menus at window level
  over the range; the range overlay draws an outline in the mode. `ui/studio.slint`: properties and
  callbacks. `main.rs`: Escape closes the box; review states.

## Core, engine

No change.
