# Plan: Windows: small audio buffers

Spec: [spec.md](spec.md)

## Approach

In `Stream::open` (`engine/src/platform/wasapi_device.cpp`), when a buffer size is set and the
endpoint's mix rate equals the engine rate, activate an `IAudioClient3`, ask
`GetSharedModeEnginePeriod` for the float32 format we already build, choose the period with a pure
helper (`platform/wasapi_period.hpp`), and call `InitializeSharedAudioStream`. On success that
client replaces the plain one and the period in effect (`GetCurrentSharedModeEnginePeriod`, since
the engine period is shared by every stream on the endpoint) becomes the stream's period. On any
failure the untouched plain client is initialised as before.

Alternatives: exclusive mode gives the lowest latency but takes the device away from every other
application and needs format negotiation; ASIO needs a driver this hardware does not have and an
SDK under its own licence. Both stay out of scope.

## Changes by layer

- **Engine**: `wasapi_period.hpp` (new, platform-free), `wasapi_device.cpp` (`open_small_period`,
  period used for status, ring target and latencies). Nothing new runs on the audio thread: the
  stream is opened on the audio thread before it starts, as before.
- **App**: `settings.rs` `buffer_note`, appended to the Settings › Audio note.
- **Docs**: README status, engine/README.md Windows audio section and platform table,
  docs/building.md.

## Principles check

- Real-time safety: unchanged; the render loop and its buffers are the same.
- Cross-platform: the helper and its test build on every OS; the WASAPI code is Windows-only.
- Evidence: hardware numbers in the pull request; no benchmark covers device open, and the render
  path is unchanged.

## Tests and evidence

- Tests: `engine/tests/test_wasapi_period.cpp`; `buffer_note_only_when_the_device_differs` in
  `settings.rs`.
- Benchmarks: no render-path change.
- Screenshots: the note is plain text under the existing controls; hardware measurements instead.

## Risks

- A driver that accepts `InitializeSharedAudioStream` but glitches at small periods: the user picks
  a larger buffer or Device default, which use the plain path as before.
