# Return tracks and sends in the Live set export

Status: Done (T5 checked in Live 12) · Spec: 006 · Issue: #31

## Problem

Export › Project writes an Ableton Live set, but it drops return tracks and sends: every track is rendered dry, as if
its Out went to the master, and the notes file says so. A project that leans on a shared reverb or delay return opens
in Live sounding different, and its routing has to be rebuilt by hand.

## Behaviour

- Every return track becomes a Live return track, in order, with its name, colour, volume, pan and on/off state.
- Every track keeps its sends: one Live send per return, at the same level. Tracks are still rendered dry, so the
  returns are fed inside Live and the sends stay adjustable there.
- Live can't load Device Studio's devices. A return **with devices** that a playing track feeds is therefore also
  rendered as audio (its wet sound, after its devices, volume and pan) onto an audio track named
  "<return> (rendered)". Its Live return track is written turned off, so nothing sounds twice. The notes say how to
  swap in an effect of your own (put it on the return, turn the return on, delete the rendered track).
- A track whose Out is a return is set to "Sends Only" in Live with a full (0 dB) send to that return.
- Differences Live forces, each listed in the notes file with the tracks concerned:
  - Live sends stop at 0 dB. Sends above that (up to +6 dB here, or an Out to a return plus a send to it) are set to
    0 dB.
  - A rendered track already carries its volume and pan, and sits at 0 dB in Live. A post-fader send keeps its level.
    A pre-fader send becomes a post-fader send with its level divided by the track's volume; it follows the track's
    pan, which a pre-fader send here does not.
- A project without return tracks exports exactly as before.

## Requirements

- R1. The set holds one `ReturnTrack` per return, after the ordinary tracks, and one send per return on every
  ordinary and return track; the set-level pre/post list has one entry per return.
- R2. Send levels, "Sends Only" outputs and return volume, pan and on/off follow Behaviour.
- R3. A fed return with devices is rendered through the same pass as the track renders: that return alone, fed by
  the sends and outputs of every unmuted track.
- R4. The structure check run before the file is written covers the new elements (send count per track, levels in
  Live's range, returns last, pre/post entries) and rejects sets that break them.
- R5. The notes file describes what was carried over and every approximation made.

## Acceptance checks

- A1. Unit tests: the written XML for a set with two returns (levels, clamping, inactive sends on returns, Sends
  Only, pre/post list), a set without returns, and broken sets failing the check.
- A2. Unit test: the render snapshot for a return solos it, mutes the other returns and keeps the mix's sends.
- A3. Self-test: Export › Project with two returns (one with a device), sends and an Out to a return: the set passes
  the check with 2 return tracks and 3 sends, the wet render exists, the notes name each approximation.
- A4. The exported set opens in Live 11.3 or later with the returns and sends in place (manual check).

## Out of scope

- Importing return tracks and sends from a Live set.
- Automation of sends, and carrying Device Studio devices over as Live devices.
- The channel strip's trim, width and mono (they are part of the dry render already).
