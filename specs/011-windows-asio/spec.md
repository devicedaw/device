# Windows: ASIO drivers

Status: Done · Spec: 011 · Issue: none

## Problem

On Windows, Device plays and records only through WASAPI shared mode. Most audio interfaces made
for music ship an ASIO driver, and musicians expect to pick it: it talks to the interface without
the Windows audio engine in between, so it reaches lower latencies and exposes every input and
output of the interface. Device has no ASIO backend. It was left out because the ASIO SDK was
proprietary; since October 2025 Steinberg also offers the SDK under GPLv3, which can be combined
with Device's AGPL-3.0-or-later.

## Behaviour

- Settings › Audio lists every installed ASIO driver beside the Windows devices, as
  "<driver> (ASIO)", in both the Output and Input lists.
- Picking an ASIO driver as output plays through it. Its inputs are the recording inputs: as input,
  pick the same driver (picking "None" plays only). Track inputs list the driver's input channels.
- The Buffer setting sets the driver's buffer, rounded to a size the driver supports. Latencies
  come from the driver, so recorded takes line up as with any other device.
- A driver that cannot run at the session rate (48 kHz) is refused with a message saying so: ASIO
  does not convert rates.
- Picking an ASIO driver with a Windows device on the other side (for example ASIO output and the
  built-in microphone) is refused with a message.
- If the driver asks to be reset (its control panel changed the rate or buffer), playback stops as
  when a device is unplugged; picking the device again restarts it.

## Requirements

- R1. Every installed ASIO driver that loads must be listed with its input and output channel
  counts and the standard rates it accepts.
- R2. An ASIO output must play the engine's output with no dropouts at the driver's buffer sizes.
- R3. An ASIO input on the same driver must record, with the driver's latencies reported for
  compensation.
- R4. The driver's sample formats (16/24/32-bit integer, 32/64-bit float, aligned 32-bit containers)
  must be converted exactly; unsupported formats must fail clearly, not play noise.
- R5. The buffer size must follow the driver's granularity (powers of two, fixed steps or fixed).
- R6. Mixing an ASIO driver with a WASAPI endpoint must be refused, without opening anything.
- R7. The SDK must be fetched at a pinned version with a checksum, never committed, and the build
  must work with ASIO switched off (`DSTUDIO_ENGINE_ASIO=OFF`).

## Acceptance checks

- A1 (R4, R5): `test_asio_convert` (every format round-trips, integer clipping and byte layout,
  buffer size choice). Runs on every OS.
- A2 (R6): `test_windows_audio` (backend choice for ASIO and endpoint pairs). Runs on every OS.
- A3 (R1, R2, R3): on a Windows PC with FlexASIO: the driver is listed; playback at several buffer
  sizes with 0 xruns; a duplex recording with 0 overruns and the latencies reported.
- A4 (R7): CI builds the Windows engine with ASIO; the configure step checks the SDK's SHA-256.

## Out of scope

- The driver's control panel button, clock source choice and channel names in the UI.
- Restarting automatically after a driver reset request.
- WASAPI exclusive mode.
- More than the first two outputs (the engine is stereo).

## Open questions

None.
