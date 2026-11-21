# ADR 0002: Audio input, monitoring and recording

Status: accepted (2026-11-08). Implemented in the engine (real-input path not yet verified on hardware).

## Context
The app needs to arm audio tracks (R), choose a per-track input ("None", "Input 1 (Mono)", ...),
monitor input (I), record from the transport, and configure output/input device, sample rate and
buffer size in Settings › Audio. Our engine was output-only.

## Decision
- **Model (dstudio-core):** `Track.arm`, `Track.monitor`, `Track.input: Option<InputSource { channel, stereo }>`,
  with `SetTrackArm`, `SetTrackMonitor` and `SetTrackInput` (audio tracks only). The snapshot carries them to the
  engine. A finished take becomes an undoable `Batch` "Record": `AddAsset` + `AddClip` on the armed track at
  the take's latency-compensated start.
- **Device setup is an app preference, not project state:** output/input device, sample rate, buffer size and
  input mute.
- **Engine API (Rust contract in `crates/dstudio-engine/src/lib.rs`):** `list_devices`, `configure_device`,
  `device_status`, `set_input_muted`, `start_recording(dir)`, `stop_recording() -> Vec<RecordedTake>`,
  `recording_state`, `recording_peaks`, and `render_offline_with_input` for deterministic tests.
- **Real-time path:** the input callback writes into preallocated per-track ring buffers. A non-RT writer
  thread drains them into 32-bit float WAV files. The audio thread never touches the disk, and a ring
  overrun is counted and reported, never blocking.
- **macOS duplex:** built-in mic and speakers are separate Core Audio devices with independent clocks. The
  engine must either create a private (non-persistent) aggregate device with drift correction, or run a
  separate input unit with an adaptive-rate ring buffer. Documented in engine/README.md.
- **Latency compensation:** takes are shifted earlier by input latency + output latency + safety offsets as
  reported by the device.
- **Microphone permission:** opening an input device on macOS can show the system microphone-permission
  prompt for the host application. Automated tests use injected input only. A real-input check runs
  only with `DSTUDIO_TEST_REAL_INPUT=1`, after the user agrees.

## Implementation notes (engine, 2026-11-08)
Details in engine/README.md, "Input, monitoring and recording".
- **ABI:** additive, `SE_ABI_VERSION` stays 1: `se_track_desc_v4` / `se_session_publish_v4` (arm, monitor,
  `input_mode` none/mono/stereo, `input_channel`; same 56-byte size as v3, no implicit padding), plus
  `se_device_list`, `se_device_configure`, `se_device_status_get`, `se_input_set_muted`,
  `se_record_start/stop/get_take/state/peaks`, `se_record_stats_get`, `se_render_offline_with_input`.
- **Duplex:** private aggregate device (output = clock, drift compensation on the input sub-device),
  one HALOutput AUHAL pulling input inside the output callback. With an input the hardware runs at the
  engine rate (AUHAL does not resample input); other rates are rejected at configure time.
- **Lazy input:** nothing opens an input unless `configure_device` names one. In the Rust contract,
  `DeviceConfig.input = None` means **no input** (as the app's preferences use it),
  `Some(uid)` that device, `Some(DeviceConfig::DEFAULT_INPUT)` (`""`) the system default input.
  The engine sample rate is fixed at engine creation; `configure_device` never changes it.
- **Monitoring semantics:** monitored input is summed with the track's clips/notes and goes through the
  track chain, also while stopped. Armed and not monitored, the input is not heard, but the track meter
  shows the raw (pre-fader) input level.
- **Takes:** a seek/stop ends a take and starts a new one at the new position (several takes per
  track per recording are possible); a loop wrap does not (looping writes linearly, no take lanes).
  An overrun drops the block and also starts a new take, so takes stay sample-aligned.
- **Latency:** `start_frame = position − (input + output latency)`, each side = device latency +
  safety offset + stream latency (no IO-buffer term: an acoustic loopback test on a MacBook Air showed the buffer term over-compensated by ~18.5 ms; without it recordings land +2.7..6.2 ms late, consistent with speaker→mic travel and onset detection; see crates/dstudio-engine/tests/real_loopback.rs); 0 for injected input. `DeviceStatus` reports the two
  sides separately.
- **Recording offset (spec 020, 2026-11-21):** a manual correction for interfaces that report their latency wrongly,
  `se_record_set_offset` (engine frames, positive = earlier), subtracted from `start_frame` with the latency and L.
  Kept in the app preferences per input device id (ms, −100..+100), not in projects. Audio takes only.
- **Additive Rust API:** `Engine::recording_stats()` (overruns, dropped frames, write errors, takes),
  `Engine::input_error()`, `DeviceConfig::DEFAULT_INPUT`.
- **Not verified on hardware:** the real-input path (prompt) — `DSTUDIO_TEST_REAL_INPUT=1 record_check`.
