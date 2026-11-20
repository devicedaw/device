# Tasks: Windows: ASIO drivers

Plan: [plan.md](plan.md)

- [x] T1. `windows_audio.hpp` (`choose_backend`) and `test_windows_audio.cpp`. Verify: ctest.
- [x] T2. `asio_convert.hpp` (sample formats, buffer size) and `test_asio_convert.cpp`. Verify: ctest.
- [x] T3. WASAPI exports under `windows_audio::`; `windows_audio_device.cpp` dispatcher. Verify: WASAPI
  playback and switching unchanged on the test PC.
- [x] T4. CMake: SDK fetch (URL + SHA-256), `DSTUDIO_ENGINE_ASIO`, sources and tests. Verify: Windows
  build with warnings as errors.
- [x] T5. `asio_device.cpp`. Verify: FlexASIO listed; playback at 64..2048 with 0 xruns; duplex
  recording with 0 overruns; switching between ASIO and WASAPI while playing; the app starts on the
  driver from saved preferences.
- [x] T6. Docs: README, engine/README.md, third_party_licenses.md, docs/architecture.md,
  docs/building.md, docs/adr/0003-plugins.md, PROGRESS.md. Verify: read against the change.
- [x] T7. Evidence for the pull request. Verify: `spec-verify`.
