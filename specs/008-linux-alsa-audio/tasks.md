# ALSA audio on Linux: tasks

- [x] Backend choice and ALSA id helpers, with tests (linux_audio_select.hpp).
- [x] Load libpipewire at run time; stop linking it (CMake, build.rs).
- [x] ALSA backend: enumeration, open with plug fallback, output and duplex cycle, xrun restart.
- [x] Sample conversion with tests and a benchmark (pcm_convert.hpp, dstudio_pcm_convert_bench).
- [x] Dispatcher with PipeWire-to-ALSA fallback for the system default.
- [x] Engine tests on ALSA's null PCM; CI loopback through pipewire-alsa.
- [x] Release check that no audio library is linked.
- [x] Docs: building.md, engine README, README.
