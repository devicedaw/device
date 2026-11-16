# ALSA audio on Linux

Status: Done · Spec: 008 · Issue: none

## Problem

On Linux, audio went through PipeWire only, and the app linked libpipewire directly. On a
distribution that does not ship or run PipeWire, the app either did not start at all (missing
library) or started with no audio devices. ALSA is part of the kernel, so it is the one audio
API every desktop Linux system has, and opening a hardware device directly through it gives the
lowest latency with nothing to configure.

## Behaviour

- The app starts on any Linux system, with or without PipeWire installed.
- Settings › Audio › Output device and Input device list the PipeWire devices (when its server
  runs) and the ALSA ones, which end in "(ALSA)": "ALSA default" and every hardware PCM of every
  sound card ("HDA Intel PCH: ALC892 Analog (ALSA)").
- "System default" plays through PipeWire when its server runs and through ALSA otherwise. If
  PipeWire runs but cannot play, the default falls back to ALSA.
- Buffer size sets the ALSA period; the status shows the period actually in effect.
- Picking a hardware device that PipeWire (or another program) holds open shows that it is busy.
- Picking an ALSA input with a PipeWire output, or the other way round, is refused with a message
  saying to pick both from the same audio system.

## Requirements

- R1. The app must not link libpipewire or libasound; both are loaded at run time.
- R2. With no PipeWire server, the system default output must be ALSA's default PCM.
- R3. ALSA hardware PCMs must be listed with stable ids (card id, not card number) and opened
  directly when they can run the project rate in a float, 32-, 24- or 16-bit format, else through
  ALSA's plug plugin.
- R4. The ALSA backend must honour the buffer size (period) and support duplex (input and output
  in one callback, linked on one card) with recordings landing on the timeline like PipeWire's.
- R5. The audio thread must stay RT safe: no allocation, locks or logging in the ALSA cycle.
- R6. Input and output must use one backend.
- R7. `DSTUDIO_AUDIO_BACKEND=alsa|pipewire` must force the backend of the system default.

## Acceptance checks

- A1 (R1): the release packaging script fails if the binary lists libpipewire or libasound as
  NEEDED; `readelf -d device-studio` shows neither.
- A2 (R2, R6, R7): `linux_audio_default_output_prefers_pipewire_then_alsa` and
  `linux_audio_input_and_output_must_share_a_backend` (engine tests, every OS).
- A3 (R3): `linux_audio_alsa_ids_map_to_pcm_names`, `pcm_convert_round_trips_every_format`.
- A4 (R4): `alsa_device_runs_the_engine_with_the_chosen_buffer_size`,
  `alsa_device_duplex_opens_the_input` (ALSA's null PCM, no hardware), and the duplex loopback
  test through ALSA PCMs routed into the CI's headless PipeWire graph: every click lands at its
  timeline position.
- A5 (R5): `dstudio_pcm_convert_bench` (the only new per-period work besides the ALSA calls):
  about 1 to 4 µs per 256-frame period on a hosted Linux runner.

## Out of scope

- A native JACK backend.
- Sample rates other than the project rate on ALSA devices (ALSA's plug plugin resamples when a
  device cannot run it).
- An "audio system" chooser in Settings: both backends' devices share one list.

## Open questions

None.
