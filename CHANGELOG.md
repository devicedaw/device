# Changelog

All notable changes to Device, by release. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/) with `0.x` while Device is pre-alpha: a minor bump (0.2.0) for new
features or anything that breaks older projects, a patch bump (0.1.1) for fixes only. Pre-releases are tagged
`-alpha.N`, `-beta.N` or `-rc.N`. How a release is made is in [docs/building.md](docs/building.md#versions-and-releases).

Add a line under **Unreleased** in the same pull request as any change a user would notice, under Added, Changed,
Fixed or Removed. Write it for people who use Device, not for developers.

## [Unreleased]

### Fixed

- Tempo-synced effects, instruments and plugins (LFO, Trance Gate, Echo, Delay, Grains and others) run at the song
  tempo in time signatures such as 6/8 and 2/2, and in exports and bounces, instead of the wrong speed.
- Exports (mixdown, stems and Live sets) line up with the timeline when a device adds latency, such as the Limiter,
  instead of starting late and cutting off the end.
- Consolidate and Bounce work while the master is muted, instead of replacing the clips with silence.
- Bounce keeps the source track's sends and group, and no longer fails because a file is missing on another track.
- Splitting a looping audio or MIDI clip keeps what it plays; splitting a very short clip no longer crashes.
- Undoing a track deletion brings the track back exactly as it was.
- No more crashes when folding a group with automation shown, or when dragging the edges of very short clips.
- Projects with damaged or hand-edited values (a time signature of 0, a missing number, an invalid note) open and play
  instead of crashing, staying silent or refusing to load. Media paths survive moving a project saved through a
  folder path with `..` or a symlink.
- Very long recordings are kept: a take that reaches the file size limit is saved up to that point, and takes over
  2 GB load again. The status line says when a take was cut short.
- A project file holding two copies of a track (for example after merging two versions) opens with each copy
  editable on its own.

## [0.1.0] - 2026-11-16

The first release of Device: a pre-alpha for trying it out and reporting what breaks. Packages for macOS (Apple
silicon and Intel), Windows and Linux are attached below. The macOS app is not notarised yet and the Windows packages
are not signed, so your system asks you to confirm the first launch.

### Added

- Timeline with audio and MIDI clips, fades, looping, a metronome, and tempo, time signature and key in the transport.
- Piano roll with a velocity lane, quantize, transpose and humanize.
- Audio recording with measured latency compensation, count-in and loop takes; MIDI keyboard input and recording.
- Mixer with sends, return tracks, group tracks, and a channel strip trim, width and mono switch.
- Built-in instruments (synth, sampler, drum rack, pad sampler) and 20 built-in effects, an LFO and Chains.
- AU, VST3 and CLAP plugin hosting with an out-of-process scanner and delay compensation.
- Automation for every device control and the channel strip.
- Export to WAV or MP3 (mix or stems, whole song or loop) and to an Ableton Live set with return tracks and sends.
- Undo and redo for every edit, and an optional assistant panel that uses your own Anthropic API key.
- The app icon on macOS, Windows and Linux.
- Browser drag and drop: while you drag a sample or MIDI clip over the arrangement, a clip preview shows where it
  will land, and it lands there (it used to land to the right of the pointer). Drop a sample on a track, below the tracks for a new audio track, or on a Sampler or
  Pad Sampler to load it. Drop an instrument on an instrument track to swap its instrument, or below the tracks
  for a new instrument track; double-clicking an instrument with no instrument track selected adds one.
- The window button shows on every built-in effect with an expanded window, the LFO included, and no longer on cards without one.
- Expanded windows for Auto Filter, Channel EQ and Character EQ, with a large display, a live analyser (Pre, Post or
  Off, and Hold view), In and Out meters and every control laid out in rows, including Character EQ's band Drive.
- Expanded windows for Tremolo, Auto-Panner, Chorus, Flanger, Phaser and Ring Mod: a large plot of the effect's
  motion or frequency response with a time or frequency axis, units and a legend, which you can drag to set its
  rate and depth (or feedback or bias), In and Out meters, and every control on one row.
- An expanded window for Character Mod: the circuit's motion in its own unit (delay, sweep, gain or pan) with the
  band it travels and a legend, the circuit and LFO controls on one card, and a rack card with Input, the circuit's
  artifacts, Depth-M and Depth-S, Seed (click for the next take), Mono and Output.
- Expanded windows for Compressor, Limiter, Gate and Glue Compressor: the last 1, 4 or 10 seconds of input, output and
  gain reduction (Hold view freezes it) under a threshold or ceiling line you can drag, a transfer curve with the live
  operating point beside the main controls, In and Out meters and every control laid out in rows.
- Expanded windows for Character Compressor (the model pills in the header, a main card that follows the model, and a
  rack card with Detector, Sidechain and Level sections) and Mastering Limiter (its style pills, a short-term loudness
  lane against a Streaming, Broadcast or Hot target with Loudness Reset, and ceiling and threshold lines you can drag).
  Mastering Limiter's ceiling reads in dBTP while True Peak is on.
- Expanded windows for Transient Shaper (a scope of the output with the gain it applied, and a picture of a model hit
  whose two handles set Attack, Sustain and their times) and Multiband Compressor (the three bands on a frequency axis
  with each band's level, gain, threshold and up threshold, crossovers you can drag, and a rack card for the selected
  band with its transfer curve).
- Expanded windows for Delay (its echo train or repeat EQ across the display, the Link, Sync, Hold view, View, Dry and
  Mode controls in the header, and a card whose band column edits the repeat-EQ band picked in its Band menu) and Reverb
  (a 1200 pt window with the tail picture across the display, a Filter range whose two handles set Low Cut and High Cut,
  and the shimmer controls lit in Shimmer mode).
- An expanded window for Echo: the machine's tape strip with its erase, record and playback heads (lit while they
  play), the tail across the display, a main card with the spacing, transport, width, mix and a Throw pill that sends
  while held, and a rack that shows the tail filters and ducking, or the machine's character behind the Character latch.
- Expanded windows for Distortion, Tape Saturation and Soft Clipper (the static transfer on a 48 dB square with the
  gain or clipping it applies lit against unity, two cycles of a full-scale sine through the device, and every control
  in one card) and Bit Crusher (one crushed cycle across the display under its held rate and number of levels).
- Expanded windows for Utility (its controls on three cards, the meters down the face) and Stereo Widener (the stereo
  field over frequency above a correlation lane that plots the input and output correlation of the last six seconds).
  The engine measures a built-in device's stereo correlation while its window is open (`se_builtin_correlation`).
- An expanded window for Tuner: the card's tuner at twice its size across the display, with its input meter.
- An expanded window for Spectrum with three views: 2D (the spectrum, its peak hold and the slope guide on a dB
  ladder), 3D (a waterfall of the last 32 to 256 frames) and Split (the spectrum over a spectrogram), with Hold view,
  Reset, History and Color in its top bar and every analyser setting in one card. Curve drawing in every expanded
  window is about three times cheaper (coordinates are written without the float formatter).
- An expanded window for Trance Gate: the step ring (step count, rate) and the pattern track on a panel down the
  left, the envelope with its draggable handles across the display, and a card with Attack, Decay, Sustain,
  Release, Swing and Mix. The ring and envelope are the card's, drawn and hit tested in the window's geometry.
- Expanded windows for the restoration devices: De-Clipper (its transfer held at the threshold on a 48 dB square
  beside two cycles of a sine through it, with the Reduction lit against unity) and De-Esser, De-Hummer and De-Noiser
  (the card's graph at full size with its handles, over a dB ladder and frequency axis, captioned and with a legend),
  each with one card of every control.
- Expanded windows for the pitch devices: Pitch Shifter (the interval field with the In and Out notes against an A4
  reference, the semitone axis, the formant strip and a legend), Grains (the buffer with Position and Spray readouts,
  a pitch axis and a time axis, Cloud / Rhythm and Freeze in the header, and two cards of controls that follow the
  mode) and Pitch Corrector (the correction trace over cent rules labelled from +100 to −100, with Follow, Key,
  Scale, Speed, Retune Speed, Humanize and Mix below).
- An expanded window for Shaper: the lane tabs over the display, the Shape, Band and Trigger strip on its caption row
  and the curve editor across the rest of it (points, bends and the lane's own pills edited in place), with the
  lane's Rate and Depth, the trigger's Threshold, Mix and Output below. The editor is the card's, drawn and hit
  tested in the window's geometry.
- An expanded window for LFO: two cycles of the wave across the window with phase quarters, the live position
  dotted and the same Rate / Depth drag as the card, captioned with the shape and the cycle length, over Shape,
  Timebase, Rate, Phase and Depth.
