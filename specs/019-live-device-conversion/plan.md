# Plan: Convert effects for Live sets

## App (crates/dstudio-app)

- `live_devices.rs` (new): `LiveDevice` (Live's element name, the name for the notes, its elements in
  Live's order, the clamped settings) and `convert(&Device)`. Compressor → Compressor2, Utility →
  StereoGain, Channel EQ → Eq8. Every element Live saves for the device is written; the ones Device
  has no setting for keep a new device's value. Units: dB thresholds and Utility gain to linear
  gain, percent to 0..1, Device's Utility channel order to Live's, Channel EQ types and slopes to
  EQ Eight modes. A Compressor with a sidechain has no Live device.
- `live_set.rs`: `SetTrack::devices`; the track's inner device chain writes them with the shared
  device head and the existing float, on/off and value element writers, plus groups.
- `dialogs.rs`: `ExportUi::convert_fx` and the Effects menu. `live_chain(track)` decides per track
  (R2). Tracks that convert are rendered by a dry pass (`make_dry` on the stem snapshot: no devices,
  unity gain, centre pan); their Live track keeps volume, pan and unbaked sends and gets the devices.
  The notes list converted tracks, tracks rendered instead with the reason, and clamped settings.
- `ui/dialogs.slint`, `ui/studio.slint`: the Effects pill shows the choice and opens a two-row
  ChoiceList.

## Core, engine

No change. Old projects and the file format are untouched.

## Risks

- Live upgrades a set saved for an older schema when it opens it. The set declares Live 11.3; the
  device values follow the units Live saves. A4 checks this in Live 12.
