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
