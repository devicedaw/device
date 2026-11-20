# Plan: Windows: ASIO drivers

Spec: [spec.md](spec.md)

## Approach

A second Windows backend behind the same `AudioDevice` interface, chosen per configuration the way
Linux picks between PipeWire and ALSA:

- `windows_audio_device.cpp` owns `create_platform_audio_device()` and the device list. It sends a
  configuration to ASIO when the output id starts with `asio:` and to WASAPI otherwise
  (`windows_audio.hpp`, `choose_backend`, plain string logic tested on every OS).
- `asio_device.cpp` finds drivers in `HKLM\SOFTWARE\ASIO`, loads one with `CoCreateInstance`, and
  drives it through the SDK's `IASIO` interface: rate, buffer size, `createBuffers`, `start`. The
  driver's `bufferSwitch` converts its buffers to planar float (`asio_convert.hpp`), calls the
  engine, converts back and calls `outputReady`.
- Only the SDK's headers are used (`asio.h`, `asiosys.h`, `iasiodrv.h`); the SDK's own driver-list
  and global wrapper sources are not, so there is no global driver state beyond the one running
  stream that the context-free callbacks need.

Alternatives: writing the `IASIO` declarations ourselves would avoid the SDK but copies its
interface by hand with no gain now that the SDK is GPLv3; a third-party wrapper (PortAudio, RtAudio)
would bring a second audio abstraction into the engine.

## Changes by layer

- **Engine**: `windows_audio.hpp`, `windows_audio_device.cpp`, `asio_device.cpp`,
  `asio_convert.hpp`; `wasapi_device.cpp` exports its device, list and default rate under
  `windows_audio::` instead of the platform functions. CMake fetches the SDK zip (URL + SHA-256) on
  Windows, behind `DSTUDIO_ENGINE_ASIO` (default ON). Nothing new on the C ABI: ASIO devices are
  ordinary entries in `se_device_list` with an `asio:` id.
- **App**: none needed; ASIO drivers appear in the existing Output and Input lists.
- **Docs**: README (status, platform table), engine/README.md (platforms, Windows audio, gaps),
  third_party_licenses.md, docs/architecture.md, docs/building.md, docs/adr/0003-plugins.md,
  PROGRESS.md.

## Principles check

- Real-time safety: `bufferSwitch` only converts samples and calls the engine; buffers are
  allocated before `start`. No locks, no allocation, no logging on the driver thread.
- Cross-platform: the new logic that can be tested without a driver (backend choice, conversion,
  buffer size) is in headers built and tested on every OS.
- Original work: the backend is written against the SDK headers; no SDK sources are copied.
- Licence: SDK used under GPLv3, compatible with AGPL-3.0-or-later (GPLv3 section 13); fetched,
  never vendored.

## Tests and evidence

- Tests: `test_asio_convert.cpp`, `test_windows_audio.cpp`.
- Hardware: FlexASIO on the Windows test PC, numbers in the pull request.
- Benchmarks: the render path is unchanged; conversion is a per-sample copy.
- Screenshots: Settings › Audio listing the ASIO driver.

## Risks

- Drivers differ widely. FlexASIO is the only one tested; the backend follows the SDK's documented
  calling order so other drivers should behave, and failures surface as clear errors.
- Probing a driver for the device list loads it; probes are cached for the process.
