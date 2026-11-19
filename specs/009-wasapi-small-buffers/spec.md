# Windows: small audio buffers

Status: Done · Spec: 009 · Issue: none

## Problem

On Windows the Buffer setting in Settings › Audio does nothing below 10 ms. A plain WASAPI shared
stream always runs at the endpoint's default period (480 frames at 48 kHz), so picking 64, 128 or
256 samples still gives about 22 ms of output latency, while Settings shows the number picked.
Found by running the WASAPI backend on a real sound card for the first time (Windows 10, inbox HD
Audio driver), where the endpoints offer periods down to 128 frames through `IAudioClient3`.

## Behaviour

- Picking a buffer below 10 ms in Settings › Audio runs the device at that size when the sound
  card supports it, or at the closest size above it that it supports. Playback, recording and
  monitoring keep working.
- When the device ends up at a different size than the one picked, Settings › Audio says so under
  the controls ("The device runs at 128 samples (2.7 ms), the closest buffer it supports.").
- Device default and buffers of 10 ms or more behave as before.

## Requirements

- R1. On Windows, a buffer size below the endpoint's default period must open the output with the
  smallest supported shared-mode period at or above it, when the endpoint runs at the engine rate
  and offers a smaller period.
- R2. Any other case, or a failure of the low-latency call, must fall back to the plain stream, so
  no setup that worked before stops working.
- R3. The device status must report the period in effect as the buffer size, and the latencies
  must follow it.
- R4. Settings › Audio must say when the running device uses a different buffer than the one picked.

## Acceptance checks

- A1 (R1, R2): `test_wasapi_period` pins the period choice: rounding to the fundamental period,
  clamping to the minimum, and falling back for 0, for sizes at or above the default and for
  endpoints without smaller periods. Runs on every OS.
- A2 (R1, R3): on a real sound card, buffer 128 runs at 128 frames with 0 xruns, and output
  latency drops from 1056 to 280 frames; an HDMI endpoint without small periods keeps 480.
- A3 (R2): duplex recording with a 44.1 kHz microphone and a 48 kHz output at buffer 128 records
  10 s with 0 overruns.
- A4 (R4): a unit test of the note text, shown only when the device differs from the pick.

## Out of scope

- WASAPI exclusive mode and ASIO.
- Small periods for an input whose rate differs from the engine's (that path relies on Windows
  rate conversion, which `IAudioClient3` streams do not do).
- Changing the list of buffer sizes offered in Settings.

## Open questions

None.
