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

- A note played from the piano roll's keyboard no longer keeps sounding when the window loses the mouse while the key
  is held (another window or a system dialog coming up, for example).

## [0.4.0] - 2026-11-25

### Added

- Talk to the assistant. Click the mic in the assistant panel, or press Control-Option (Ctrl-Alt on Windows and
  Linux), and speak: your words are turned into text on this computer and never sent anywhere for recognition. The
  speech model (82 MB) is downloaded once from Settings › Downloads.
- The assistant can listen. Turn on Settings › General › Let the assistant listen to audio and it measures the
  selected clip, track or the whole mix (loudness, peaks, tone, key and tempo) before it answers. Only those numbers
  are sent; the audio stays on your computer.
- A Getting started tour in the assistant panel walks you through making a first song in four steps.
- Autosave. While a project has unsaved changes, Device keeps a backup of it every 2 minutes. If Device or a plugin
  crashes, the next launch reopens your changes as unsaved: save to keep them. A take that was being recorded keeps
  everything recorded up to the crash.

### Changed

- Quitting, closing the window, File › Open, Open Recent and opening a project from the browser now ask whether to
  save unsaved changes first. A change made in a plugin's own window counts as an unsaved change.
- Recordings and renders made before a project's first save are moved into the project's media folder when you save
  it, instead of staying in a temporary folder the system may clear.

### Fixed

- File › Show Project in Finder did nothing on Windows and Linux. It now opens Explorer or the file manager, and the
  menus name the right one.
- Opening a project whose audio files moved now lists them in a Missing media dialog. Locate… finds them in a folder
  you pick (by name and length) and relinks them in one undo step.
- Voice input no longer risks crashing on older computers: the speech recogniser is built for a common processor
  baseline instead of the build machine's.
- Opening badly formed FLAC, MP3, WAV and Ogg files is safer (updated decoders).

## [0.3.1] - 2026-11-21

### Fixed

- AI apps connected to Device work more reliably. Asking for the built-in effects and instruments returns a short list
  first and the full settings only for the ones the app asks about, instead of one very large answer, and notes the
  app adds without a number are no longer refused as duplicates.

## [0.3.0] - 2026-11-21

### Added

- Tempo changes. A song can change tempo, in steps or smooth ramps: pick Song › Tempo in the master track's lane
  picker to draw them, the ruler marks each change, and the transport shows the tempo at the playhead. Tempo changes
  carry into MIDI file and Live set exports.
- Use your own AI app with Device. Turn on Settings › General › Let AI apps control Device and press Copy setup, and
  an AI app on this computer that supports MCP can read the open project and make edits, each one undoable. Device
  asks once per app before it may make changes.
- 85 ready-made chains (up from 27), ten or eleven in each folder. The FX folder is now called FX & Glitch.
- Timeline edit tools: the tool button in the timeline header switches between Selector, Marquee (select a time
  range across tracks) and Scissors (click to split). T toggles Scissors.
- Smart Fill: with the Smart Fill pill on, drag over empty space on a track to fill that range with a part from the
  pattern library, or record into it.
- Bounce Track in Place (clip menu › Bounce) renders a whole track through its effects into one audio clip.
- The Synth has 100 factory sounds in ten categories, including synth drums. Devices › Instruments opens the Synth
  into its categories, a click plays a sound in the preview bar, and dropping a sound adds the Synth with it.
- Effects in a chain have level meters between them and a + in front of each one to insert a device at that spot.
  Click an effect's header to select it (Shift or ⌘ for more): ⌘D duplicates, ⌘G groups into a new chain and ⌘⇧G
  unpacks a chain.
- Return tracks can send into later return tracks.
- Windows: ASIO drivers are listed in Settings › Audio, marked "(ASIO)", and play and record at the driver's own
  buffer sizes.
- Settings › Audio › Recording offset: nudge recordings from each input earlier or later, in milliseconds, if takes
  land slightly off the beat.
- Settings: search at the top of the sidebar finds any setting, every setting has an (i) with a one-line
  explanation, Settings › MIDI › Default instrument picks what new instrument tracks play, and Settings › Privacy
  has crash reports (on by default, nothing personal sent) and confirms that Device collects no usage data.
- Rename a project by clicking its name in the title bar; its folder and file are renamed to match.
- The assistant can keep your Anthropic API key in the system keychain, work with built-in effects and their
  settings, be asked about a clip from its menu or with ⌘K, and stretch to the full window height.
- The track header menu has Bounce, Automation, Copy and Paste effects and a grid of track colours; right-clicking an
  empty lane offers the track's actions.
- Exported Live sets can keep Compressor, Utility and Channel EQ as Live's own devices instead of rendering them
  (Export › Project › Effects › Convert to Live devices).
- **Report a bug** and **Share your feedback** (top right) now open a form that sends your report straight to the
  developer. Your email is optional and never shown publicly, and system info is only included if you leave it on.
- Device tells you when a new version is out. It checks once at launch and from Options › Check for Updates…, and a
  blue **Update available** pill at the top right shows the new version and what changed, with a link to download
  it. Only the app's version is sent; Settings › General › Check for updates at launch turns the check off.

### Changed

- The browser's empty Search and Library tabs are gone, and the toolbar button that shows it is now called Browser.
- Clicking a note in the piano roll plays it for its length (up to a second).
- Audio track headers have the input monitoring button, new tracks start on the default colours, and the piano
  roll's loop bar takes the clip's colour.
- Settings › Downloads says there is nothing to download yet instead of "Not available yet".

### Fixed

- Windows: the Buffer setting now works below 10 ms, so 64 to 256 samples give low latency instead of about 22 ms.
- Knobs in expanded device windows keep dragging instead of stopping after the first move.
- The timeline header's zoom sliders change the track height and zoom.
- The assistant no longer throws away good replies that name a colour or a setting in an unexpected way.

## [0.2.0] - 2026-11-18

### Added

- Linux: Device now plays and records without PipeWire. When no sound server runs, it goes straight through ALSA,
  which every Linux system has, and it starts even when PipeWire isn't installed. Settings › Audio also lists each
  sound card's ALSA devices, marked "(ALSA)", next to the PipeWire ones, for the lowest latency.
- Device chains can be saved and reused. The chain's name menu has Save, Save as…, Rename…, Unpack chain (keeps the
  effects, drops the chain) and Delete saved chain, and Chains › swaps the chain for another one.
- 27 ready-made chains in eight folders (Vocals, Drums, Bass, Guitar, Keys & Synths, Pads & Textures, FX and
  Mix & Master), each with its main controls on macros. Find them, and your saved chains, under Browser › Devices ›
  Chains and Add device › Chains.
- Markers (the labelled ranges on the ruler) have a full right-click menu: Cut, Copy, Paste, Color, and Insert ›
  Marker before / Marker after. ⌘X, ⌘C and ⌘V work on the selected marker. New markers are named "Marker 1",
  "Marker 2" and so on.
- Settings › General › Projects folder: choose where your projects are saved. Save starts there, and the browser
  lists the projects in it.
- Settings › General › Sections move clips: when on, moving a marker moves the clips and automation in it,
  resizing, inserting or deleting a marker moves everything after it, and duplicating or pasting a marker copies its
  clips too. Each edit is one undo step.

### Changed

- Projects has its own tab at the top of the browser, next to Devices and Files, with its own search. The Files tab
  now only shows your folders.

### Fixed

- Windows: Device now draws with Direct3D 12, and only falls back to Vulkan on a PC without it. This is aimed at a
  report of every label and the timeline showing blank on one PC, with only shapes and icons drawn.
- Importing MP3, FLAC and Ogg Vorbis files uses newer decoders with fixes for damaged or unusual files: a crash when
  seeking some FLAC files, MP3 tag-reading errors, and several overflow and memory fixes for Ogg Vorbis.
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
