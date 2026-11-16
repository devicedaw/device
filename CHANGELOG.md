# Changelog

All notable changes to Device, by release. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/) with `0.x` while Device is pre-alpha: a minor bump (0.2.0) for new
features or anything that breaks older projects, a patch bump (0.1.1) for fixes only. Pre-releases are tagged
`-alpha.N`, `-beta.N` or `-rc.N`. How a release is made is in [docs/building.md](docs/building.md#versions-and-releases).

Add a line under **Unreleased** in the same pull request as any change a user would notice, under Added, Changed,
Fixed or Removed. Write it for people who use Device, not for developers.

## [Unreleased]

Nothing has been released yet. The first release, 0.1.0, will cover what is on `main` today:

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
