# Automate any device control, mute and device on/off

Status: Done · Spec: 007 · Issue: none

## Problem

Only some controls could be automated from where they sit. Built-in devices had "Automate" on knobs, but the Synth's
right-click menus only offered modulation, choice switches and menus had nothing, and the channel strip had no menu at
all. A track could not automate its mute, and a device could not be switched on and off over time. Lane labels showed
the Synth's parameters as bare numbers (and plugin-like values as percentages), so a cutoff lane did not read in Hz
the way the knob does.

## Behaviour

- Right-clicking any device control (built-in devices, plugin cards, every Synth control: knobs, faders, pills,
  switches, the mod matrix) opens a menu that starts with "Automate <parameter>". The Synth's other rows (reset,
  modulation) stay below it.
- Right-clicking a device's power switch opens "Automate On".
- Right-clicking the channel volume, pan or M (track header, inspector, mixer strip), or the inspector's trim, width
  or mono, opens "Automate <name>". The master's fader offers "Automate Volume".
- Choosing an Automate row shows the automation lanes (if hidden) and switches the track's lane to that parameter.
- The lane picker lists Track › Volume, Pan, Mute, Trim, Width, Mono, and under each device its automatable
  parameters followed by "On".
- Lane labels and a hovered or dragged point's readout use the parameter's own text: Hz/kHz, dB, ms, ct, %, choice
  names, Muted/Unmuted, On/Off.
- Mute, Mono, On and choice lanes are steps: a point's value holds until the next point. Points dragged on them snap to the
  allowed values.
- While a Mute or On lane is active, the M button and the power switch show the lane's state at the playhead.
  Clicking them explains that the parameter is automated; with Automation Arm on during playback the click writes one
  step at the playhead instead (one undo step, "Edit Mute Automation" or "Edit On Automation").

## Requirements

- R1. Every automatable device parameter must be reachable with right-click › Automate on its control.
- R2. A track's mute and each device's on/off state must be automatable, with step (hold) evaluation that matches
  bit for bit between core and engine.
- R3. Lane labels must show the parameter's own unit and text.
- R4. The channel strip's volume, pan, mute, trim, width and mono must be automatable and offer Automate on
  right-click.
- R5. The audio path must not get measurably slower for sessions without the new lanes.

## Acceptance checks

- A1 (R1, R4): app test `right_click_menus_open_the_parameters_lane` (device knob, power switch, strip volume, pan
  and mute, master).
- A2 (R2, R4): engine tests `automation_stepped_lanes_hold_each_value`, `automation_mute_lane_replaces_the_static_mute`,
  `automation_device_on_lane_switches_an_insert_per_block`, `automation_strip_lanes_set_trim_width_and_mono`; core `switch_lanes_hold_each_value`,
  `switch_targets_round_trip_through_json`, `switch_lanes_validate_their_device_and_reach_the_snapshot`; app
  `switches_follow_an_active_lane`.
- A3 (R3): app tests `synth_lanes_read_in_the_parameters_own_units`, `device_lanes_read_like_their_controls`;
  before and after screenshots of a Synth cutoff lane.
- A4 (R5): `dstudio_engine_bench` Automation table, before and after, plus the new "32 x 7 lanes" row.

## Out of scope

- Modulation and macro rows on channel strip controls.
- A track header context menu ("Show automation"); the lanes button already does this.

## Open questions

None.
