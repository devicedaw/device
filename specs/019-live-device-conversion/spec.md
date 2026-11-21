# Convert effects for Live sets

Status: Done · Spec: 019 · Issue: none · Roadmap: [010 › G24](../010-parity-roadmap/spec.md)

## What

Export › Project writes a Live set. Today every track with clips is rendered after its effects,
so the set sounds like the project but its effects can't be changed in Live. The dialog's
Effects pill ("Render") is a placeholder.

With Effects set to "Convert to Live devices", a track whose effects all have a close Live device keeps them as Live
devices: the track is rendered before its effects (the clips, and on an instrument track its
instrument), and the set puts the matching Live devices, with matching settings, on that track,
with the track's volume and pan on Live's mixer. Any other track is rendered after its effects as
before. Render stays the default; the choice lasts for the session, like the dialog's other settings.

## Why

People move a song to Live to keep working on it. Effects they can still adjust are worth more
than a printed sound, as long as the set still sounds like the project.

## Requirements

- R1. Export › Project's Effects pill opens a menu: Render (the default) and Convert to Live devices.
- R2. A track converts when every insert effect on it is one of: Compressor, Utility, Channel EQ;
  each is on (a bypassed one is left out); none is modulated, automated or
  in a chain; and the track's volume, pan and mute have no automation. Otherwise it renders as
  before. Returns always render as before.
- R3. A converted track's audio is rendered without its effects, at unity volume and centre pan;
  Live's mixer carries its volume and pan.
- R4. Each effect becomes the Live device that does the same job, in the same order, with every
  setting Live has mapped to Live's units (dB to linear gain where Live stores gain, percent to
  0..1). A setting outside Live's range is clamped, and the export notes list it.
- R5. The set still passes `live_set::validate`, and Live 12 opens it with the devices in place
  and the settings shown as mapped.
- R6. The export notes list which tracks kept Live devices and which were rendered, with the reason.

## Acceptance checks

- A1. Unit tests: each mapping (values and units) and the per-track decision (R2).
- A2. A written set with converted devices passes `validate`.
- A3. Self-test: export with Convert to Live devices on a project with a Compressor on a track: the
  set has a Live Compressor with the mapped threshold on that track, passes its check, and the notes
  list it.
- A4. Manual: the set opens in Live 12 with the devices and settings in place. Checked in Live 12.4:
  Compressor (threshold, ratio, attack, release, output, knee, dry/wet, detector), Utility (gain,
  width, balance) and an EQ Eight bell band all show the mapped values. The Compressor's sidechain
  filter is written as Live 11's `SideChainEq` group, which the set's schema expects.

## Out of scope

Limiter, Gate, Delay, Reverb and the other effects (rendered as before; each can be added to the table later),
device automation in Live, chains as Live racks, sidechains.
