# ALSA audio on Linux: plan

## Engine (engine/src/platform)

- `linux_audio_device.cpp` provides the platform functions (`create_platform_audio_device`,
  `list_audio_devices`, `query_default_output_sample_rate`) and a device that dispatches each
  start to PipeWire or ALSA. The choice is a pure function in `linux_audio_select.hpp` (tested on
  every OS): an `alsa:` id means ALSA, another id PipeWire, the default follows an ALSA input,
  then `DSTUDIO_AUDIO_BACKEND`, then whether a PipeWire server answers.
- `pipewire_device.cpp` loads libpipewire with dlopen (30 functions in one table, as the ALSA
  MIDI input does for libasound); the rest of its API is header-inline.
- `alsa_device.cpp`: libasound through dlopen; enumeration walks cards and PCM devices; one
  SCHED_FIFO thread per stream: wait for a free period, render, write; duplex reads a captured
  period first. Playback is primed with silence; xruns and suspends restart both streams from a
  primed buffer; a vanished device stops the stream.
- `pcm_convert.hpp`: planar float to and from interleaved float32, S32, S24, S24_3, S16.

## Build, CI, release

- CMake: PipeWire headers only (no link); `DSTUDIO_ENGINE_ALSA` (was `DSTUDIO_ENGINE_ALSA_MIDI`)
  covers ALSA audio and MIDI. `build.rs` no longer links libpipewire.
- CI: an extra step runs the device tests with `DSTUDIO_AUDIO_BACKEND=alsa`, including the duplex
  loopback through pipewire-alsa. The release script checks the binary links neither library.

## Principles check

RT safety: the ALSA cycle does no allocation (buffers sized at start), no locks and no logging;
ALSA's read/write/wait calls are the device I/O the thread exists for. No C ABI or project format
change.

## Risks

- Real hardware has only been exercised through CI's virtual devices and ALSA's null PCM; the
  hardware path (hw: open, format probing, linking) needs a check on a physical card.
