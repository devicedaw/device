# ADR 0003: Plugin hosting (Audio Unit, VST3, CLAP)

Status: accepted (2026-11-08). Implemented in the engine (see "Implementation notes").

## Context
"Add device" lists the built-ins, Audio Units, VST3, CLAP and Chains. We host AU (macOS), VST3
and CLAP. The development Mac has Apple's built-in AUs plus third-party commercial plugins (AU, VST3
and CLAP) installed; the automated tests only rely on Apple's AUs and our own test plugins.

## Licensing
- VST 3 SDK: MIT since VST 3.8 (Steinberg, 2025-10-29). The ASIO SDK is separate (GPLv3/proprietary) and not used here.
- CLAP: MIT (free-audio/clap).
- Audio Units: Apple system frameworks.
SDKs are fetched at a pinned tag at configure time (CMake FetchContent) and never vendored into git.

## Decision
- **Model (dstudio-core `device.rs`):** `Track.devices` (insert effects, in order) and `Track.instrument_plugin`
  (instrument tracks; replaces the built-in synth). `Device { id, plugin: PluginRef, enabled, state (base64) }`.
  Commands: `AddDevice`, `RemoveDevice`, `MoveDevice`, `SetDeviceEnabled`, `SetInstrumentPlugin`.
- **Instance lifetime:** the engine keeps plugin instances keyed by `DeviceId` across republishes. `state` is
  applied only at instantiation (load, undo of a removal). Before saving, the app pulls states with
  `Engine::plugin_states()` into the model via `Document::sync_device_states` (no undo step).
- **Scanning** runs out of process (a scanner executable per plugin bundle with a timeout). Crashing or hanging
  plugins are blocklisted, and results are cached as JSON in the app's support directory.
- **Real-time:** instantiation, state load/save and editor handling happen on non-RT threads (main thread where
  the format requires it). The audio thread only calls the plugin's process function with preallocated
  buffers and parameter/event queues. Bypassed devices are skipped. A plugin that failed to load is a
  silent pass-through, reported via `plugin_load_errors`.
- **Editors** are native windows owned by the engine's platform layer (NSWindow + NSView on macOS), opened
  and closed on the main thread.
- **Testing:** small test plugins are built with the SDKs (VST3 gain, CLAP gain, CLAP sine instrument). AU is
  tested with Apple's AULowpass/AUDelay and DLSMusicDevice.
- **Not in v1:** plugin delay compensation (latency is reported only), sidechains, multi-out instruments,
  parameter automation and a host-side generic parameter UI. (Delay compensation was added later: ADR 0004.)

## Implementation notes (engine, 2026-11-08)
Details in engine/README.md "Plugins". Where the implementation refines or deviates from the decision above:
- **SDK pins:** CLAP `1.2.10`, VST 3 SDK `v3.8.1_build_84` (only the `pluginterfaces` and `public.sdk`
  submodules). Fetched into the build tree (`engine/build*/_deps`, `target/dstudio-third-party` for cargo).
- **ABI:** `se_session_publish_v5` (per-track `devices`, `instrument_plugin`, `track_name`) plus
  `se_plugin_*` functions; `SE_ABI_VERSION` stays 1 (append-only).
- **Scanning orchestration lives in the scanner executable,** not in Rust: `dstudio_plugin_scanner --scan`
  is a coordinator that enumerates, consults/writes the JSON cache, and spawns itself once per AU / bundle
  (`--scan-one`) with a timeout. Rust's `scan_plugins` only runs the coordinator and parses its report. This
  keeps the C++ scan test self-contained and still keeps all plugin code out of the app process.
- **"Main thread"** for instantiation is the publishing thread (the app's main thread by contract); CLAP's
  thread-check reports it as main. Editors additionally require the real Cocoa main thread.
- **Retired instances** are destroyed on the main thread at the next publish / `se_plugin_*` call /
  `se_plugin_idle` once the audio thread can no longer reach them (no janitor thread).
- **Editor titles** are just the plugin name for now: `TrackSnapshot` carries no track name (the ABI has a
  `track_name` field ready for it).
- **Tempo for plugins** comes from `se_transport_set_tempo` (frames per quarter note, separate from the
  metronome's click grid, which is one time-signature denominator note), 120 BPM until one is set.
- **Export** with plugins uses the live instances and therefore needs the audio device stopped.
- **Failed instrument plugins** render silence (they do not fall back to the built-in synth).
