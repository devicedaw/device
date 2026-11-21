# Device audio engine (`engine/`)

C++23 real-time audio engine behind the C ABI in `include/dstudio_engine.h`.
The Rust core links the static library `dstudio_engine`. The header is the
contract. `SE_ABI_VERSION` is still 1: the first milestone's structs and
signatures are unchanged, and later features are **appended** at the end of
the header (`se_meters_peek`, `se_transport_set_loop`, `se_metronome_desc` +
`se_metronome_set`, for MIDI `se_note_desc`, `se_track_desc_v2`,
`se_session_desc_v2` + `se_session_publish_v2`, and for synth parameters
`se_synth_params`, `se_track_desc_v3`, `se_session_desc_v3` +
`se_session_publish_v3`, `se_synth_params_default`, and for input and
recording `se_track_desc_v4`, `se_session_desc_v4` + `se_session_publish_v4`,
`se_device_list`, `se_device_configure`, `se_device_status_get`,
`se_input_set_muted`, `se_record_start/stop/get_take/state/peaks`,
`se_record_stats_get`, `se_render_offline_with_input`, and for plugins
`se_device_desc`, `se_track_desc_v5`, `se_session_desc_v5` +
`se_session_publish_v5`, `se_plugin_is_live`, `se_plugin_load_errors`,
`se_plugin_save_states`/`se_plugin_saved_state`, `se_plugin_editor_open/
close/is_open`, `se_plugin_latency`, `se_plugin_idle`, and for plugin delay
compensation `SE_PDC_MAX_DELAY_FRAMES`, `se_pdc_set_enabled`,
`se_pdc_is_enabled`, `se_pdc_report` + `se_pdc_track` + `se_pdc_report_get`,
`se_transport_audible_position`, and for automation `se_auto_point`,
`se_auto_lane_desc`, `se_track_desc_v6`, `se_session_desc_v6` +
`se_session_publish_v6`, `se_automation_eval`, `se_plugin_params`,
`se_plugin_param_value`, `se_plugin_param_text`, `se_plugin_param_change` +
`se_plugin_take_param_changes`, and for clip warping `se_warp_point`,
`se_warp_slice`, `se_clip_warp_desc`, `se_session_desc_v7` +
`se_session_publish_v7`, `se_warp_renders_pending`, `se_onset` +
`se_asset_detect_onsets`). Plugin hosting (AU, VST3, CLAP) is
described in [Plugins](#plugins), delay compensation in
[Plugin delay compensation](#plugin-delay-compensation), automation in
[Automation](#automation), clip warping in [Clip warping](#clip-warping), clip
looping in [Clip looping](#clip-looping), clip fades in [Clip fades](#clip-fades).

## Build, test, bench

Requirements: CMake ≥ 3.25, Ninja, a C++23 compiler (Apple clang 21, MSVC
19.4x from Visual Studio 2022 and GCC 13 have built it with warnings as
errors; see [Platforms](#platforms)). Linux also needs `pkg-config` and the
PipeWire and ALSA development headers (`libpipewire-0.3-dev`, `libasound2-dev`);
`-DDSTUDIO_ENGINE_PIPEWIRE=OFF` builds an engine with ALSA audio only. Windows: run CMake from a developer
command prompt (x64 MSVC environment) with the Ninja generator.

```sh
cd engine
cmake --preset default            # Release, Ninja, -Werror  -> engine/build
cmake --build --preset default
ctest --preset default            # every TEST_CASE is its own CTest test

cmake --preset asan               # Debug + -fsanitize=address,undefined -> engine/build-asan
cmake --build --preset asan
ctest --preset asan

./build/dstudio_engine_bench [blocks=4000] [clips_per_track=4] [features=0]  # 1 = loop + metronome on; also prints MIDI and plugin tables
./build/dstudio_scan_cache_bench [entries=2000] [runs=50]  # plugin-cache JSON parse time
./build/play_sine_or_wav [file.wav] [seconds=1.0]   # real default output device, -24 dBFS
./build/record_check                                # list devices (enumeration only, opens nothing)
DSTUDIO_TEST_REAL_INPUT=1 ./build/record_check [s=2]  # MANUAL: opens the real default input (macOS mic prompt!)
./build/plugin_editor_check [au <id> | clap <bundle> <id> | vst3 <bundle> <uid>]  # MANUAL: opens/closes an editor window
DSTUDIO_PLUGIN_PATH=build/test_plugins ./build/dstudio_plugin_scanner --scan --cache /tmp/plugins.json [--rescan]
```

The first configure fetches the CLAP headers and the VST 3 SDK (pinned tags,
network needed once; see `third_party_licenses.md`). Every build also produces
`build/dstudio_plugin_scanner` and the test bundles in `build/test_plugins/`.

CMake options: `DSTUDIO_ENGINE_BUILD_TESTS|BENCH|TOOLS` (default ON when top
level), `DSTUDIO_ENGINE_BUILD_SCANNER` (ON), `DSTUDIO_ENGINE_BUILD_TEST_PLUGINS`
(ON when top level; build.rs turns it on), `DSTUDIO_ENGINE_SANITIZE`
(ASan+UBSan), `DSTUDIO_ENGINE_WERROR`. The target `dstudio_engine_rust` builds
what the Rust crate needs (library, scanner, test plugins).

### Linking from Rust (`cmake` crate)

`crates/dstudio-engine/build.rs` builds the `dstudio_engine_rust` target and links
`<out>/build/libdstudio_engine.a`; it bakes the scanner path
(`DSTUDIO_PLUGIN_SCANNER_BUILT`) and the test plugin directory
(`DSTUDIO_TEST_PLUGIN_DIR`) into the crate, and shares one SDK download between
cargo profiles (`FETCHCONTENT_BASE_DIR=target/dstudio-third-party`). macOS
frameworks: AudioToolbox, AudioUnit, CoreAudio, CoreFoundation, CoreAudioKit,
AppKit, Foundation (+ libobjc, libc++). The generic recipe with the `cmake`
crate's `install` target (library + header only) still works:

```rust
let dst = cmake::Config::new("../../engine")
    .define("DSTUDIO_ENGINE_BUILD_TESTS", "OFF")
    .define("DSTUDIO_ENGINE_BUILD_BENCH", "OFF")
    .define("DSTUDIO_ENGINE_BUILD_TOOLS", "OFF")
    .build();
println!("cargo:rustc-link-search=native={}/lib", dst.display());
println!("cargo:rustc-link-lib=static=dstudio_engine");
// macOS
println!("cargo:rustc-link-lib=c++");
println!("cargo:rustc-link-lib=framework=AudioToolbox");
println!("cargo:rustc-link-lib=framework=CoreAudio");
println!("cargo:rustc-link-lib=framework=CoreFoundation");
```

The library is built with `-fPIC`, so it can go into a cdylib as well.
On Windows build.rs also links `ole32 shell32 avrt user32`, on Linux
`stdc++ dl` (libpipewire and libasound are loaded at run time; set
`DSTUDIO_ENGINE_PIPEWIRE=OFF` in the environment to build without PipeWire).

## Platforms

Platform code lives in `src/platform/` only; everything else is portable
C++23. `platform/os.{hpp,cpp}` is the small OS layer: UTF-8 paths
(`_wfopen`/wide APIs on Windows), environment, executable path, UTF-8 argv
and `ChildProcess` (CreateProcessW with an explicit inherited-handle list /
`posix_spawn`, stdout/stderr to files, poll/wait/kill, crash detection:
POSIX signal or Windows NTSTATUS exception code).

| | macOS (arm64) | Windows (x64, MSVC) | Linux (x64, GCC) |
|---|---|---|---|
| Audio backend | Core Audio (AUHAL; private aggregate for duplex) | WASAPI shared mode, event-driven, float32 (`wasapi_device.cpp`), or an ASIO driver (`asio_device.cpp`), picked by `windows_audio_device.cpp` | PipeWire streams (`pipewire_device.cpp`) or ALSA PCMs (`alsa_device.cpp`), picked by `linux_audio_device.cpp` |
| Duplex input | aggregate device, HAL drift compensation | separate capture client → `FrameRing` | PipeWire: capture stream → `FrameRing`; ALSA: linked capture PCM read in the same cycle |
| Plugin formats | AU, VST3, CLAP | VST3, CLAP | VST3, CLAP |
| Plugin editors | NSWindow (AU/VST3/CLAP) | top-level HWND hosting the plugin child window (VST3 `kPlatformTypeHWND`, CLAP `win32`) | X11 top-level window hosting the plugin window (VST3 `kPlatformTypeX11EmbedWindowID`, CLAP `x11`), XWayland on Wayland; see [Plugins on Windows and Linux](#plugins-on-windows-and-linux) |
| Scanner children | `posix_spawn` | `CreateProcessW` (no WER dialogs in the child) | `posix_spawn` |
| Test plugins | `.clap`/`.vst3` bundles | `.clap` DLL, `.vst3/Contents/x86_64-win/*.vst3`, + `DStudioTestGui.clap` | `.clap` .so, `.vst3/Contents/x86_64-linux/*.so` |
| Built with warnings as errors | yes (clang, `-Werror`) in CI | yes (`/W4 /WX`) in CI | yes (GCC, `-Werror`) in CI |
| ctest | all tests in CI; device tests skip on the hosted runner (no audio device), run by hand on a Mac | all tests in CI; AU tests and device tests skip (no audio endpoint) | all tests in CI, against a headless PipeWire server (null sink + loopback); AU tests skip |
| Tested on real audio hardware | yes (Core Audio output + duplex, see below) | by hand on one PC (output, duplex, buffer sizes, see [Windows audio](#windows-audio-wasapi)) | **no** (real PipeWire streams in CI, no sound card) |

**CI** (`.github/workflows/ci.yml`) runs on GitHub-hosted runners
(`macos-latest`, `windows-latest`, `ubuntu-latest`), none with an audio
device. On each system it builds the engine with warnings as errors, runs
ctest, builds and tests the Rust workspace and runs the app's
`--self-test --no-audio`. Pushes to main and the nightly run do all of it;
a pull request runs the parts its changes reach (see docs/building.md).

The app (`crates/dstudio-app`) builds on all three. The hosted jobs run it
without a GPU or audio device: on Linux under Xvfb with Mesa lavapipe (software Vulkan;
`SLINT_WGPU_CPU=1`), a `--state reference-default` screenshot and the full `--self-test
--no-audio` (125 checks pass; the 21 checks of the Audio Unit devices section
are reported as skipped off macOS), as a required step; on Windows with WARP
(DX12 software adapter) the same, also failing the job when a check fails (the
full self-test runs on main, nightly and labelled pull requests that change
the app; see docs/building.md): the screenshot renders, the self-test passes and the
exit code is checked on all three systems. On Windows the app ends its process directly
once `run` has returned (`exit_now` in `crates/dstudio-app/src/main.rs`): with a normal
exit, any run that had opened the window reported 0x87A instead of our code. The code is
set after `main` returns, while the process unloads its DLLs, with or without audio, a
screenshot or presented frames; a run that exits before the window exists returns 0.
Seen with WARP on the hosted runner; not yet checked on a real GPU.
Debug builds of the app abort at exit on every OS (pre-existing: wgpu-core's
debug-only lock tracing runs in a TLS destructor when Slint's thread-local
backend drops the wgpu queue), so CI uses a release build. Screenshots and logs are uploaded as artifacts.

**Linux runs real audio streams in CI.** The hosted runner has no sound card,
but the job starts a headless PipeWire graph (`tools/ci/headless-pipewire.sh`):
a null sink, driven by the system clock like a device, and `pw-loopback` from
its monitor to a virtual source. The device tests then open real PipeWire
streams, `device_duplex_loopback_take_lands_on_the_clicks` plays a click track
and records it back through that loopback (every click must land on its
timeline position; on PipeWire they land sample-exact), and the app runs
`--self-test-audio` with the device running.

What the hosted jobs cannot cover: the Windows runner has no audio endpoint,
so the WASAPI streaming code (open, callbacks, duplex ring in a live stream,
latency values) is compiled and its enumeration/validation "no device" paths
run, but not streamed. It was checked by hand on a Windows 10 PC with the
inbox HD Audio driver (headphones, S/PDIF, HDMI out; a 44.1 kHz microphone),
see "Windows audio: WASAPI" below; Linux has not run on a physical sound card.
Tests that need a device skip
with `SKIPPED: <reason>` (CTest shows "Skipped"; the harness exits 77 =
`SKIP_RETURN_CODE`). The duplex FIFO itself (`FrameRing`) is unit-tested on
every OS. The PDC golden hashes are compared only on Apple clang/arm64
(float results legitimately differ between compilers/architectures);
elsewhere that test checks determinism and the on/off identities.

### Windows audio: WASAPI

* **Streams.** Render: shared mode, `AUDCLNT_STREAMFLAGS_EVENTCALLBACK |
  AUTOCONVERTPCM | SRC_DEFAULT_QUALITY`, float32 at the *engine* rate and the
  engine's channel count; the Windows audio engine converts rate and channel
  layout to the endpoint mix format (the AUHAL converter's role on macOS).
  Input (only with `input_enabled`): a separate capture client converted to
  the engine rate, with the endpoint's channel count (≤ 64).
* **Duplex.** Capture packets go into `FrameRing` (`platform/frame_ring.hpp`);
  the render cycle reads exactly its frame count, so the engine sees equal
  input/output lengths. The ring primes to `target` = one capture + one
  render period, trims back to target when the fill exceeds target + slack
  (input clock fast), and zero-fills + re-primes on underrun (input clock
  slow). No resampling; the extra input latency (`target`) is reported.
  If the input cannot be opened the stream runs output-only with
  `input_error` (as on macOS).
* **Threads.** `start()` spawns one audio thread (COM MTA, MMCSS "Pro
  Audio") that opens and starts both clients, reports the result back, then
  waits on the two event handles. All COM objects of a stream live on that
  thread. `stop()` signals and joins.
* **Rates.** In shared mode an endpoint runs at the rate set in the Sound
  control panel; `device_rate` can only confirm it (else
  `SE_ERR_INVALID_ARG`).
* **Buffer.** A plain shared stream runs at the endpoint's default period
  (10 ms, 480 frames at 48 kHz) whatever buffer is asked for; `buffer_size`
  only lengthens its endpoint buffer. A `buffer_size` *below* the default
  period opens the stream through `IAudioClient3::InitializeSharedAudioStream`
  with the smallest supported period at or above it
  (`platform/wasapi_period.hpp`: multiples of the fundamental period between
  the minimum and the default), when the endpoint runs at the engine rate (that
  path does not convert rates) and offers a smaller period. Otherwise, or if
  that call fails, a plain stream. The inbox HD Audio driver offers 128..480
  frames in steps of 32; many HDMI/DisplayPort endpoints offer only 480.
  Status: endpoint rate, the period in effect as `buffer_size`, latencies in
  endpoint frames (render buffer + stream latency; capture period + stream
  latency + ring target).
* **Ids.** `uid` = the MMDevice endpoint id; render and capture endpoints are
  separate devices (a headset lists twice).
* **Checked on hardware** (Windows 10, inbox HD Audio driver, 48 kHz):
  playback on headphones, S/PDIF and HDMI with every buffer size, 0 xruns;
  output switched while playing in about 0.1 to 0.3 s; duplex recording from a
  44.1 kHz microphone (converted by Windows) with 0 overruns. Output latency
  1056 frames (22 ms) by default, 280 frames (5.8 ms) at 128.
* **Not implemented:** exclusive mode,
  endpoint change notifications (a removed device ends the stream;
  `is_running()` turns false).

### Windows audio: ASIO

`windows_audio_device.cpp` is the Windows `AudioDevice`: it sends a setup
whose output id starts with `asio:` to `asio_device.cpp` and everything else
to WASAPI (`windows_audio.hpp`, `choose_backend`, tested on every OS).

* **SDK.** The `IASIO` interface and its types come from Steinberg's ASIO SDK
  2.3.4, used under its GPLv3 option (`third_party_licenses.md`). CMake
  downloads the zip and checks its SHA-256; only the headers are used.
  `-DDSTUDIO_ENGINE_ASIO=OFF` builds without it (no ASIO devices listed).
* **Drivers.** Read from `HKLM\SOFTWARE\ASIO` (64-bit view) and loaded with
  `CoCreateInstance` on the calling thread (COM apartment-threaded, or the
  thread's existing apartment). `se_device_list` has one entry per driver
  that loads: `"<name> (ASIO)"`, id `asio:<CLSID>`, its input and output
  counts and the standard rates `canSampleRate` accepts. Listing loads each
  driver once; the result is cached for the process and a driver is never
  loaded twice while one streams.
* **Stream.** Output and input are one driver: as input, the same id or the
  default input (empty id) means that driver's inputs; an ASIO driver paired
  with a WASAPI endpoint is `SE_ERR_INVALID_ARG`. The driver runs at the
  engine rate (`setSampleRate`; ASIO does not convert, so a driver that cannot
  is refused). Buffers: the first `channels` outputs and up to 64 inputs,
  `createBuffers` with the size picked rounded to the driver's granularity
  (`asio_convert.hpp`, `choose_buffer_size`; 0 = the driver's preferred size,
  read after the rate is set). `bufferSwitch` / `bufferSwitchTimeInfo`
  convert the driver's buffers to planar float (16/24/32-bit integer, aligned
  32-bit containers, 32/64-bit float, little-endian), call the engine, convert
  back and call `outputReady` when the driver supports it. No locks or
  allocation on the driver's thread. Status: the driver's buffer size and
  `getLatencies` input and output latencies.
* **One stream per process.** The SDK's callbacks carry no context, so the
  running device is a process-wide pointer; a second ASIO start fails with
  `SE_ERR_STATE`.
* **Reset requests.** A driver's `kAsioResetRequest` or rate change (its
  control panel, for instance) ends the stream: `is_running()` turns false,
  as for a removed WASAPI device. No automatic restart.
* **Checked** with FlexASIO 1.10 (its WASAPI backend) on the PC above: every
  buffer size from 64 to 2048 plays with 0 xruns and the expected callback
  count, duplex recording at 128, 256 and 512 with 0 overruns, switching
  between ASIO and WASAPI outputs while playing, and the app starts on the
  driver from saved preferences. FlexASIO's default DirectSound backend stops
  calling back after about a second (a FlexASIO issue, dechamps/FlexASIO#176);
  use its WASAPI backend. No hardware ASIO driver has been tried yet.

### Linux audio: PipeWire and ALSA

Two backends behind one `AudioDevice` (`linux_audio_device.cpp`). The system
default output uses PipeWire when its server answers and ALSA otherwise (and
falls back to ALSA if PipeWire answers but cannot play); a chosen device
decides by its id (`alsa:...` ids are ALSA, the rest PipeWire node names); input
and output must share a backend. The choice is a pure function in
`linux_audio_select.hpp`, tested on every OS. `DSTUDIO_AUDIO_BACKEND=alsa|pipewire`
forces the default. Both libraries are loaded with dlopen, so the engine runs
on a system that lacks either.

#### ALSA

ALSA is part of the kernel, so it is there on every Linux system, and a
hardware PCM opened directly has the lowest latency possible.

* **Devices.** `alsa:default` (the default PCM) and one entry per hardware PCM
  of every card, `alsa:hw:CARD=<id>,DEV=<n>` (card ids survive reboots, card
  numbers do not), named "Card: PCM (ALSA)". Listing opens each hardware PCM
  briefly, unconfigured, for its channel count (2 when it is busy). Any other
  PCM name works as `alsa:<name>`.
* **Opening.** A hardware PCM is opened as `hw:` when it runs the engine rate
  with a float32, S32, S24, S24_3 or S16 format (`pcm_convert.hpp` converts),
  else through ALSA's plug plugin (`plughw:`). Playback asks for 2 channels
  (more device channels get silence), capture for all of the device's (up to
  64). Period = `buffer_size`, else the preferred block (at least 256); playback
  has 2 periods, capture 4.
* **Cycle.** One thread (SCHED_FIFO when allowed): output only, wait for a free
  period, render, write; duplex, read one captured period, render with it,
  write. Playback is primed with silence, so the duplex cycle never waits on
  it; input and output are linked (`snd_pcm_link`) when the driver allows, so
  they start on one sample. Xruns and suspends restart both streams from a
  primed buffer; a vanished device stops the stream (`is_running()` false).
  Reported latencies: output = playback buffer, input = one period.
* **Rates.** ALSA devices run at the engine rate (no `sample_rates` are
  listed for them); a different `sample_rate` request is refused.

#### PipeWire

PipeWire rather than JACK first: it is the default sound server of current
mainstream distributions, runs JACK clients itself (pipewire-jack), resamples
and remixes per stream, and lets a client request a small quantum
(`node.latency`). A native JACK backend is possible future work for systems
running jackd without PipeWire.

* **Streams.** A playback `pw_stream` (F32 interleaved at the engine rate,
  `PW_STREAM_FLAG_RT_PROCESS`, `target.object` = the chosen node, or the
  default sink) and, with `input_enabled`, a capture stream with the
  source's channel count feeding a `FrameRing` that the playback callback
  drains (same policy as WASAPI). Both callbacks run on PipeWire's data
  thread (real-time if RTKit/the rt module allow). `stop()` destroys the
  streams and the context, which joins that thread.
* **Ids and enumeration.** `uid` = `node.name` (stable across restarts),
  name = `node.description`, channels from `audio.channels` /
  `audio.position` (node info), defaults from the `default` metadata
  (`default.audio.sink/source`), rates from the `settings` metadata
  (`clock.rate`, `clock.allowed-rates`). One short-lived connection with two
  round trips and a 2 s watchdog; no stream is opened. No server → no
  devices; `start()` → `SE_ERR_DEVICE`.
* **One clock.** Both streams carry the same `node.group`, so PipeWire
  schedules them from one driver (and resamples the other device when input
  and output are different hardware). The ring's fill therefore cannot drift;
  a lasting change means a glitch (one stream missed a cycle), which would
  shift every later recording by a quantum. The ring re-syncs: it watches the
  end-of-cycle fill over windows of 32 cycles and skips or inserts the
  difference once a whole window is off by more than half a quantum
  (`FrameRing` re-sync, `frame_ring.hpp`). Both streams tag their ring
  access with the cycle's time (`pw_time.now`), so the fill is measured as
  if capture had already run: the two streams swapping order, even for good,
  is not mistaken for a lost cycle. `dstudio_frame_ring_bench` measures
  the ring: ~0.2 µs per 256-frame stereo cycle with or without re-sync.
* **Sources.** `Audio/Source` and `Audio/Source/Virtual` nodes (filter chains
  such as noise suppression, loopbacks) are listed as inputs.
* **Rates and buffers.** `device_rate` requests a graph rate (`node.rate`,
  must be in `clock.allowed-rates`); `buffer_size` (default: the engine
  block, ≥ 256) requests a quantum (`node.latency`). PipeWire may not honour
  either; status reports the graph rate, quantum and `pw_time.delay`.

### Plugins on Windows and Linux

* **Loading.** `LoadLibraryExW(LOAD_WITH_ALTERED_SEARCH_PATH)` / `dlopen(RTLD_NOW
  | RTLD_LOCAL)`; libraries are never unloaded (as on macOS). A CLAP is a
  single shared library; a `.vst3` bundle folder resolves to
  `Contents/<arch>-win/<name>.vst3` or `Contents/<arch>-linux/<name>.so`
  (a single-file `.vst3` is loaded as is). VST3 entry points: `InitDll`/
  `ExitDll` (optional, Windows), `ModuleEntry`/`ModuleExit` (required,
  Linux).
* **Class ids.** VST3 class ids are stored as the canonical 32-digit
  `INLINE_UID` text on every platform (on Windows the in-memory TUID is
  COM-ordered and converted), so projects and scan caches are portable.
* **Folders.** Windows: `%COMMONPROGRAMFILES%\{VST3,CLAP}`,
  `%LOCALAPPDATA%\Programs\Common\{VST3,CLAP}`, `%CLAP_PATH%`; Linux:
  `~/.vst3`, `/usr/lib/vst3`, `/usr/local/lib/vst3`, `~/.clap`,
  `/usr/lib/clap`, `/usr/local/lib/clap`, `$CLAP_PATH`; plus
  `$DSTUDIO_PLUGIN_PATH` (`;`-separated on Windows).
* **Audio Units** are macOS-only; AU tests skip elsewhere and tests that only
  need "a third plugin" use `DStudioTestLatency.clap` instead.
* **Editors.** Windows: `plugin_editor_win.cpp` (top-level window, plugin
  child HWND, `IPlugFrame::resizeView` / `request_resize`, user resizing,
  close box), covered by `DStudioTestGui.clap` +
  `plugin_editor_window_win32_open_resize_close`, which runs in
  every Windows CI job; per-monitor DPI scaling is
  not handled yet. Linux: `plugin_editor_x11.cpp` (an X11 top-level window
  with WM_DELETE_WINDOW, size hints and `_NET_WM_NAME`; the plugin embeds
  its own child window, `kPlatformTypeX11EmbedWindowID` / `CLAP_WINDOW_API_X11`).
  Linux has no system event loop for plugins, so the host runs one:
  `src/platform/run_loop.{hpp,cpp}` polls the plugins' file descriptors and
  fires their timers from `se_plugin_idle` (VST3 `Linux::IRunLoop` through
  the editor's `IPlugFrame`; CLAP `clap.posix-fd-support` and
  `clap.timer-support`). Timer resolution is the idle rate: the app idles
  every 16 ms while a plugin window is open. `dstudio_run_loop_bench` measures
  one dispatch (about 0.2 µs with 1 to 16 idle fds and timers on a 2.1 GHz
  Xeon), on the main thread only.
  `DStudioTestGui.clap` registers an fd and a timer on Linux, and
  `plugin_editor_window_x11_open_resize_close` checks both, the window size,
  the close box and re-opening; it skips without `DISPLAY` and runs under
  Xvfb in the Linux CI job. Dragonfly Room (CLAP and VST3) and an LSP CLAP
  were checked by hand with `plugin_editor_check_linux`. Configure with
  `-DDSTUDIO_ENGINE_X11=OFF` (or `DSTUDIO_ENGINE_X11=OFF` for cargo) to build
  without the libX11 headers (libX11 itself is loaded with `dlopen` when the
  first editor opens, never linked); opening an editor then returns `SE_ERR_UNSUPPORTED_FORMAT`.

## Layout

| Path | Contents |
|---|---|
| `include/dstudio_engine.h` | C ABI (v1; additions appended at the end) |
| `src/c_api.cpp` | `extern "C"` wrappers, exception barrier, thread-local `se_last_error` |
| `src/engine.{hpp,cpp}` | `Engine`: assets, graph publication/reclamation, transport, meters, stats, RT `process()` |
| `src/render_graph.{hpp,cpp}` | Immutable render graph built from a session snapshot; pan law |
| `src/processor.{hpp,cpp}` | `render_block()` (the one DSP path) and SIMD-friendly kernels |
| `src/synth.{hpp,cpp}` | Built-in polyphonic instrument (`Synth`, one per instrument track) |
| `src/spsc_queue.hpp` | Lock-free SPSC ring for transport commands |
| `src/wav.{hpp,cpp}` | Hand-written RIFF/WAVE reader, streaming float32 writer |
| `src/resampler.{hpp,cpp}` | Kaiser-windowed sinc resampler (import time) |
| `src/rt_util.hpp` | Flush-to-zero guard, clock, atomic max |
| `src/delay_line.hpp` | Delay line for plugin delay compensation (memcpy ring, lazy flush) |
| `src/automation.hpp` | Automation: lane evaluation (the core contract), ramp fill, timeline history (PDC alignment), synth parameter map |
| `src/plugins/param_bus.hpp` | Lock-free MPMC queue of plugin editor parameter changes |
| `src/platform/` | `AudioDevice` interface + backends (Core Audio, WASAPI, PipeWire), `FrameRing`, Linux plugin run loop and X11 editor windows |
| `tests/` | Self-contained harness (`test_harness.hpp`), tests, allocation-tracking `operator new` |
| `bench/bench_render.cpp` | Offline render benchmark |
| `bench/bench_scan_cache.cpp` | Plugin-cache parse benchmark (`dstudio_scan_cache_bench`) |
| `src/recorder.{hpp,cpp}` | Recording: RT capture into per-track SPSC rings, WAV writer thread, live takes/peaks |
| `src/test_hooks.hpp` | Test-only hooks (fake device factory, writer stall, ring size); not public ABI |
| `tools/play_sine_or_wav.cpp` | Manual check through the real default output device |
| `tools/record_check.cpp` | Device listing; with `DSTUDIO_TEST_REAL_INPUT=1` a 2 s real-input recording (manual only) |
| `src/plugins/plugin_instance.hpp`, `plugin_common.cpp` | `PluginInstance` interface, `PluginSpec`, `NoteFeeder` (notes → MIDI), parameter queue |
| `src/plugins/plugin_host.{hpp,cpp}` | Instance registry keyed by device id: creation, retirement, states, load errors, editors |
| `src/plugins/au_instance.cpp`, `vst3_instance.cpp`, `clap_instance.cpp` | Format hosts |
| `src/plugins/plugin_modules.{hpp,cpp}` | VST3 / CLAP bundle loading (CFBundle), shared per path |
| `src/plugins/plugin_scan.{hpp,cpp}` | Inspecting one AU / bundle (used only by the scanner process) |
| `src/platform/plugin_editor_mac.mm` | Editor windows (NSWindow; AU Cocoa view / AUGenericView, VST3 IPlugView, CLAP gui) |
| `src/platform/plugin_editor_win.cpp`, `plugin_editor_x11.cpp` | Editor windows on Windows (HWND) and Linux (X11) |
| `src/platform/run_loop.{hpp,cpp}` | Linux main-thread run loop for plugins (fds and timers, run from `se_plugin_idle`) |
| `scanner/scanner_main.cpp` | `dstudio_plugin_scanner`: out-of-process scan coordinator + per-plugin child |
| `scanner/cache_json.hpp` | The scanner's cache JSON reader (shared with the benchmark) |
| `test_plugins/` | Original test plugins: `DStudioTestGain.vst3`, `DStudioTestGain.clap`, `DStudioTestSine.clap`, `DStudioTestLatency.clap`/`.vst3`, `DStudioTestCrash.clap` |
| `tools/plugin_editor_check.mm` | Manual editor-window smoke test |
| `third_party_licenses.md` | Fetched SDKs: tags, commits, MIT texts |

## Design

### Data flow

```
Rust core (non-RT)                                  audio thread (RT)
------------------                                  -----------------
se_asset_load_file ─ decode WAV ─ resample ─► asset registry (shared_ptr<const Asset>)
se_session_publish(_v2/_v3) ─ copy + validate + build ─► RenderGraph ──atomic ptr swap──► process()
se_transport_* / set_loop / metronome_set
                   ─ mutex(producers) ─► SPSC queue ─┬─────────── drained at block start (device/offline render)
                                                     └─ applied inline when nobody owns the RT path
se_meters_read / se_meters_peek / se_stats_get ◄── atomics ◄───── written per block / segment
```

### Render graph

`se_session_publish` copies the description into an immutable `RenderGraph`
on the calling thread. Everything is resolved up front: mute/solo collapses
into one `audible` flag, track gain × pan law becomes two per-channel gains,
clips are clamped to their asset's length (clips past the asset end or with
zero length are dropped) and sorted by start, and raw sample pointers are
cached. The graph holds `shared_ptr` references to every asset it uses.
Validation errors: negative `source_offset`/`length`, non-finite gains/pan,
NULL arrays with non-zero counts → `SE_ERR_INVALID_ARG`; unknown asset id →
`SE_ERR_NOT_FOUND`. The published graph is replaced atomically: the audio
thread sees either the old or the new graph for a whole block, never a mix.

Semantics:

* **Clip**: plays asset frames `[source_offset, source_offset + length)` at
  timeline frames `[timeline_start, timeline_start + length)`, scaled by
  `gain`. Overlapping clips on a track sum. Mono assets feed both channels.
* **Pan**: constant power, `θ = (pan + 1)·π/4`, `L = cos θ`, `R = sin θ`
  (centre = −3.01 dB each side, `L² + R² = 1`, hard pans are exact 1/0).
  For stereo material it acts as a balance control (L gain on the L channel).
* **Trim, width, mono** (`se_session_publish_v10`, spec 005): after the insert chain, before pre-fader sends and
  the fader, a listed track's signal is scaled by `trim` and its side signal `(L − R)/2` by `width` around the mid
  `(L + R)/2` (`mono` = width 0). Folded at publish time into `L' = aL + bR`, `R' = bL + aR`
  (`a = trim·(1 + w)/2`, `b = trim·(1 − w)/2`); an unlisted or neutral track is not touched.
* **Mute/solo**: if any track is soloed, only soloed tracks are audible.
  Mute always wins (a soloed and muted track is silent).
* **Master**: sum of tracks (and return tracks, see
  [Return tracks and sends](#return-tracks-and-sends)) × `master_gain`. No limiter/clipper — output can
  exceed ±1.0.
* **Instrument tracks** (`se_session_publish_v2`, `instrument != 0`): the
  track's notes render through its `Synth` (mono), summed with the track's
  audio clips, then the same gain/pan/mute/solo/meters as audio. Notes are
  validated (pitch 0..127, velocity 1..127, `notes` non-NULL when
  `note_count > 0` → else `SE_ERR_INVALID_ARG`), zero-length notes dropped,
  and sorted by (start, pitch). Notes on non-instrument tracks are validated
  but ignored. `se_session_publish` builds the same graph with no
  instruments, `se_session_publish_v2` the same with default synth
  parameters (both convert to v3 internally).
* **Synth parameters** (`se_session_publish_v3`): each track may point to an
  `se_synth_params` (NULL = defaults = `SynthParams::default()` in
  dstudio-core). They are copied into the graph (`RenderGraph::synth_params`,
  reserved bytes zeroed), so they change atomically with notes and mix.
  Validation: enums in range, every float finite and inside the documented
  range (same ranges as `SynthParams::validate`) → else `SE_ERR_INVALID_ARG`.

### Threading model

| Thread | What it may do |
|---|---|
| Audio thread (Core Audio IO proc) **or** the thread inside `se_render_offline(_with_input)` | `Engine::process()` only |
| Recording writer thread (one per recording) | drains the recorder's rings into WAV files |
| Any non-RT thread | everything else in the ABI |

Exactly one "RT owner" at a time: an atomic owner flag (`rt_owner_`:
none / device / offline / control) makes `se_render_offline` fail with
`SE_ERR_STATE` while the device runs (and vice versa; a second concurrent
offline render also gets `SE_ERR_STATE`). Only the owner touches the RT
transport state and pops the command queue. The fourth value, *control*, is
a short claim a non-RT thread takes to apply transport commands inline (see
Transport); device start and offline render yield until it is released
instead of failing.
`se_render_offline` runs the identical `process()` the device callback runs
(command drain, graph acquire, chunked `render_block`, meters, stats).
`se_export_wav` uses the same `render_block()` DSP with a private transport,
scratch buffers and a `shared_ptr` to the current graph, so it never touches
live playback/meters/stats and may run while the device is playing.

Mutexes exist only on the non-RT side: `assets_mutex_` (registry),
`control_mutex_` (current/retired graphs), `command_mutex_` (serialises
transport producers onto the SPSC queue), `device_mutex_` (start/stop).

### Graph reclamation (no frees on the audio thread)

The audio thread uses a single hazard slot `rt_active_seq_`:

```
RT block start: active = BUSY;  g = live_graph (seq_cst);  active = g->seq
RT block end:   active = IDLE
```

A retired graph `R` (already swapped out) is freed only when
`active == IDLE` or (`active != BUSY` and `active > R.seq`). The RT side is
wait-free; the non-RT side checks in `se_session_publish`, `se_asset_release`
and engine destruction (`Engine::collect_garbage`). A graph that cannot be
freed yet just stays on the retired list until a later check. Graph
destruction drops asset references, so asset memory is also released only on
non-RT threads.

### Assets

`shared_ptr<const Asset>` (planar float at engine rate). The registry holds
one reference, every graph that uses the asset holds another.
`se_asset_release` removes the id from the registry (later snapshots that
reference it fail with `SE_ERR_NOT_FOUND`) but audio already published keeps
playing until the graph using it is replaced and reclaimed.

### Transport

Commands (`play`, `pause`, `stop` = pause + position 0, `seek`, and
`se_transport_set_loop` / `se_metronome_set`) all go through one 256-entry
lock-free SPSC queue (producers serialised by `command_mutex_`). Positions are
`int64` frames and may be negative (pre-roll renders silence).

* **Device running**: commands are applied, in order, at the start of the
  next callback (block boundary, not sample-accurate). A full queue returns
  `SE_ERR_STATE`. `se_transport_get` shows the state as of the last block.
* **No device running** (and no `se_render_offline` in progress): the
  command is applied synchronously on the caller's thread before the call
  returns, so `se_transport_get` reflects it immediately, without rendering.
* **During a concurrent `se_render_offline`** on another thread: queued;
  applied at that render's next block start or when it returns.

How it works and why it is race-free (`Engine::push_command`,
`acquire_rt_owner`, `release_rt_owner`):

1. A producer always pushes onto the queue first, then issues a seq_cst
   fence and tries to claim the RT path (`CAS none → control`). On success
   it drains the whole queue (so earlier queued commands keep their order),
   publishes the transport state and releases. Being the single owner, it is
   the only consumer of the SPSC queue and the only writer of the RT state.
2. If the device or an offline render owns the path the CAS fails and the
   command stays queued for that owner. If another thread holds the brief
   control claim, the producer yields and retries, so with no device and no
   offline render the command is always applied before the call returns.
3. Every owner releases with `store(none); seq_cst fence; if queue not
   empty → claim and drain` (looping). Producer (`push; fence; CAS`) and
   releaser (`store none; fence; check empty`) form a store-buffering pair:
   with both fences seq_cst, at least one of them sees the other's write, so
   either the producer's CAS succeeds or the releaser sees the command. A
   command can never be stranded in the queue with no owner left to apply
   it. `se_device_stop` releases this way after the device has stopped,
   so commands queued after the last callback are applied by the stop call.
4. All ownership transitions are seq_cst RMWs/stores on `rt_owner_`, which
   gives happens-before between successive owners for the plain RT fields
   (position, loop, metronome, click voices). Device start/stop are still
   serialised by `device_mutex_`.

Tested by `offline_transport_no_stranded_commands_under_concurrent_render`
(producer vs. offline renderer), `transport_commands_across_device_start_stop`
(producer vs. start/stop cycles on the real device; skipped without one),
under ASan/UBSan and, ad hoc, ThreadSanitizer.

### Loop

`se_transport_set_loop(enabled, start, end)` (requires `start < end` when
enabling). While playing with the loop on and the playhead before `end`,
`process()` splits its segments at `end`; when a segment reaches `end` the
playhead jumps to `start`, so the wrap is sample-accurate anywhere inside a
device/offline block, and loops shorter than a block wrap repeatedly. A
playhead before `start` plays into the loop and then wraps; a playhead at or
after `end` plays linearly. `render_block()` is stateless (clips are
addressed by timeline frame), so the segment after the wrap renders clip
offsets for `start` exactly and nothing stale carries over. (The audio jump
itself is not crossfaded.) Disabling mid-play continues linearly.

### Metronome

`se_metronome_set(&{enabled, gain, frames_per_beat, beats_per_bar})`. The
two click shapes (15 ms sine, 1.2 kHz downbeat / 800 Hz beat, 3 ms
exponential decay, 2 ms raised-cosine tail fade, first sample non-zero) are
computed once in the `Engine` constructor; setting the metronome only queues
parameters. While playing, beat `k` starts at timeline frame
`llround(k · frames_per_beat)` (beat 0 at frame 0, negative beats in
pre-roll); `k mod beats_per_bar == 0` is a downbeat. Onsets come from the
timeline beat grid of each rendered segment (so they are sample-accurate in
any block partition and follow the loop: a beat sounds whenever the playhead
crosses its frame). Up to 8 click *voices* carry tails across segments in
playback time, so a click that started in the previous block, or just before
a loop wrap or a seek, rings out instead of being cut (the oldest voice is
stolen if more overlap). Voices are cleared on pause/stop/disable. The click
is mixed into the master **after master gain and after metering**, so it is
independent of the mix level, does not show on meters, and is never in
`se_export_wav` (which calls `render_block()` directly). With plugin delay
compensation (`L > 0`) the clicks go through an `L`-frame mono delay line
first, so they are heard with the (equally delayed) music; the line rings
out on pause and is flushed on seek/stop. When the session has a tempo map
([Tempo](#tempo)), beat `k` sounds at `llround(frame_at(k · click_quarters))`
on the map instead, and `frames_per_beat` is not used.

### Built-in instrument (MIDI + `se_synth_params`)

An original subtractive voice (`src/synth.hpp`); it does not model any
commercial instrument. Every Devices-panel parameter maps onto it:

| Parameter | DSP |
|---|---|
| `oscN_wave` | Saw: polyBLEP falling ramp. Square: polyBLEP pulse, DC removed. Triangle: polyBLAMP (sine-aligned; ~13 dB less aliasing than naive). Sine: odd degree-9 polynomial (harmonics < −100 dB). |
| `oscN_shape` | Square: pulse width 50 % → 5 %. Saw ↔ triangle: linear morph toward the other shape (phases aligned so the fundamental never cancels). Sine: wavefolder `sin(π/2 · 5·shape · sin θ)` normalised to ±1 (continuous to a pure sine at 0; odd harmonics only; not band-limited). |
| `oscN_octave/semitones/fine_cents` | Offsets added to the note pitch; octave/semitone switch at the next control tick, fine is smoothed. |
| `oscN_level` | Linear mix gains (smoothed, ramped per sample). |
| `cross_fm` | Linear FM osc2 → osc1, per sample: `inc1 · (1 + 0.9 · cross_fm · osc2)`, clamped to [0.02·inc1, 0.45]; so the deviation is at most ±90 % of the carrier frequency (not through-zero; band-limiting uses the instantaneous increment). |
| `sub_level` | Sine one octave below osc1's pitch (incl. glide/LFO, without FM/unison). |
| `noise_level` | White noise (xorshift32, seeded per note from (start, pitch), so export == live). |
| `filter_mode` | TPT/ZDF state-variable filter, 12 dB/oct: LP, HP, BP (BP normalised to unity gain at the centre). Stable for any coefficient change. |
| `filter_cutoff_hz` | Cutoff at C4 when key tracking is on: `fc = cutoff · 2^(key_track·(pitch−60)/12 + 6·env_amount·fenv + lfo·lfo_to_cutoff_oct)`, clamped to 10 Hz .. 0.45 × rate. |
| `filter_resonance` | Q = 0.707 · (25/0.707)^res: 0 = Butterworth, 0.5 ≈ 4.2 (+12.5 dB), 1 = 25 (+28 dB, below self-oscillation). |
| `filter_key_track` | Octaves of cutoff per octave of pitch (uses the glided pitch, not the LFO). |
| `filter_env_amount` | ±1 → ±6 octaves × filter envelope. |
| `amp_*`, `filter_*` ADSR | Linear attack over `attack`; exponential decay (distance to sustain falls 60 dB over `decay`); sustain; exponential release (−60 dB over `release`). Amp env per sample; filter env per control tick (≤ 0.33 ms late on stage changes). Voice freed at −90 dB in release, or in decay with sustain 0. |
| `lfo_*` | Sine/triangle/square/saw (all start at 0 or +1 and rise), `rate_hz`, to pitch (cents), cutoff (octaves) and amplitude (tremolo: gain between `1 − depth` and 1). Output de-clicked by a ~1 ms one-pole. **Per voice, phase 0 at each fresh note-on** (deterministic and identical in export); a mono legato note keeps the running LFO. |
| `polyphony` | 1..16 note voices; see below. |
| `glide` | Portamento, linear in pitch, covering any interval in `glide` seconds. Works in mono **and** poly: a fresh voice glides from the previous note-on's pitch (cleared by transport jumps, so the first note after a seek/loop wrap doesn't glide). |
| `unison`, `unison_detune_cents` | 1..4 stacks of (osc1 + osc2) **inside one voice** (they do not use voice slots or count against polyphony), detuned evenly over ±detune/2, start phases spread, level normalised by 1/√n (so RMS stays about the same; mono output, no stereo spread). |
| `output` | Linear post-filter gain (smoothed). |

Fixed: per-voice gain = velocity/127 × 0.25 (`kVoiceGain`), equal
temperament, A4 = MIDI 69 = 440 Hz (C4 = 60). The defaults (saw −5 ct
level 0.6 + square +5 ct level 0.5, LP 2093 Hz at C4 tracked 1:1, amp ADSR
5 ms / 200 ms / 0.6 / 250 ms) sound close to the previous fixed patch. Both
oscillators of stack 0 start at a phase where the waveform is non-zero (osc2
half a cycle after osc1), so the note-on frame itself is non-zero.

**Control rate.** Each voice recomputes pitch increments, filter
coefficients, LFO, filter envelope, glide and the smoothed parameters every
16 samples of its own lifetime (a per-voice grid from its note-on), and
ramps its gains linearly per sample inside the tick (value = start + slope ×
position-in-tick). So the output is still bit-identical for any block
partition and does not depend on where in the timeline a note starts
(tested). Transcendental work (`exp2`, `tan`) is skipped while its inputs
are unchanged.

**Parameter changes (republish).** A new graph seq makes the RT side apply
the track's parameters (a `memcmp` skips identical ones). Continuous
parameters are approached per voice by a one-pole at control rate (time
constant 10 ms, snapped when within 1e-6), so a knob sweep published as many
snapshots does not zipper or click (tested: the largest sample step during
0.2 ↔ 2.0 output and 300 Hz ↔ 8 kHz cutoff jumps every 5 ms equals the
steady signal's own). New voices start at the target values. Discrete
parameters (waveforms, filter mode, octave/semitone, unison count, LFO wave)
switch at each voice's next control tick (a filter mode switch can click).
A lower `polyphony` releases the oldest held voices (normal release).

**Polyphony and voices.** `polyphony` note voices per track (max 16). A note
beyond the limit steals the oldest note voice (note-on order): it fades out
linearly over 5 ms in its slot while the new note takes a free slot (24
slots = 16 + 8 for fades; only if all 24 are busy is the quietest fading
voice overwritten).

**Mono (`polyphony` = 1), last-note priority.** A note-on while the voice is
*held* (notes overlap) is legato: no envelope retrigger, velocity unchanged,
the pitch glides (or jumps at the note-on frame when `glide` = 0). A
note-on while the voice is *releasing* (e.g. touching notes: the note-off
and note-on fall on the same frame) retriggers both envelopes' attacks from
their current level (no click) and takes the new velocity; the pitch still
glides from where it is. Otherwise a fresh voice starts. The voice follows
the newest note's end; there is no return to an earlier still-held note.
Simultaneous notes collapse to one voice (the highest pitch of a chord,
since notes are sorted by (start, pitch)).

**State across republishes.** `Synth` objects are allocated on the non-RT
side, preallocate every voice, and are owned by `shared_ptr`s in each graph
(`RenderGraph::synth_refs`, raw pointers in `synths` for the RT path).
`Engine::install_graph` (under `control_mutex_`) gives each instrument track
the `Synth` of the same `track_id` in the currently published graph, else a
new one. The RT side is the only thread that mutates a live `Synth`; a
`Synth` whose track disappeared is freed together with the last retired
graph that holds it, i.e. by the existing reclamation protocol, never on the
audio thread. When the RT side sees a new graph seq, held voices are
re-matched by `(start, pitch)`: a match keeps sounding and follows the note's
new `end` (if that is already behind the playhead, it releases at the next
segment start); no match → normal release. So editing elsewhere, or
lengthening/shortening a sounding note, never retriggers or cuts it.

**Events.** `Synth::render` splits each render segment at every note-on
(`start`) and note-off (`end`) frame, so both are sample-accurate and the
output is bit-identical for any block partition (tested). Note-ons are found
by binary search for `start ∈ [segment)`, so no cursor has to survive
seeks.

**Transport jumps.** `rt_synth_gen_` is bumped on seek, stop, pause and every
loop wrap; a `Synth` that sees a new generation (or a timeline position
other than where its last segment ended) fades every sounding voice over
5 ms. Notes that start exactly at the new position play; notes already in
progress there are **not** retriggered (simple, deterministic). While paused
or stopped, `render_block` still renders those fade tails (nothing else), so
pausing mid-note does not click.

**Mute.** Synths of muted / non-soloed tracks keep running (output not
mixed), so unmuting mid-note continues the note exactly as if it had never
been muted. This costs CPU for muted instrument tracks.

### Preview notes (`se_preview_note`)

The browser's preview voice (`se_preview_play`) plays an asset. `se_preview_note`
is its MIDI side: it plays a note through a track's instrument device, built-in
or plugin, for the piano roll's keys and for notes being drawn or moved. The
note goes into the device's live note queue, the same lock-free 256-event SPSC
queue that `se_builtin_note` uses. The RT side drains that queue at the start of
the next block and puts the notes at offset 0, ahead of the track's own events.
Built-in instruments drain it in `process()`, and plugin instruments in the
processor's instrument path, so the notes go through the track's chain and
mixer. The main thread keeps the held (device, pitch) pairs, so
`se_preview_stop` can release them and `se_preview_info.notes` can count them.
Nothing on the audio thread allocates or locks. `dstudio_engine_bench` has a
table for it.

### MIDI input (`se_midi_*`)

Spec: [specs/003-midi-input-and-recording](../specs/003-midi-input-and-recording/spec.md).
A MIDI keyboard plays instrument tracks live, and what was played is captured
for recording.

* **Backends** (`src/platform/midi_backend.hpp`): Core MIDI on macOS (one
  client, one MIDI 1.0 protocol input port; ids are the source's unique id),
  the multimedia `midiIn*` API on Windows (ids are product names, `#2` for
  repeats), the ALSA sequencer on Linux (one reader thread; ids are
  "client:port" names; libasound is loaded with dlopen on first use, so
  programs that never use MIDI don't load it; CMake option
  `DSTUDIO_ENGINE_ALSA`, default ON, which also builds ALSA audio),
  and a stub with no ports elsewhere. The MIDI system is first touched by
  `se_midi_inputs_list` / `se_midi_inputs_open`, so engines that never use MIDI
  never start it. Only channel messages are passed on.
* **To the audio thread** (`src/midi_input.hpp`, `MidiInputHub`): one slot per
  open port (16) plus one for `se_midi_inject`, each with a 1024-message SPSC
  queue whose single producer is that port's callback. Slots (and the capture
  ring) are allocated on the main thread when first needed, so an engine that
  never uses MIDI allocates nothing for it and its callback skips the hub. The
  RT owner drains every slot once per callback (at most 256 messages; the rest
  wait), folds channels onto channel 1, turns note-on velocity 0 into note-off,
  drops program changes and note-offs with no held note-on, and delivers the
  messages at offset 0 of the callback's first segment (timing is accurate to
  one buffer). A closed port's slot sends note-offs for the keys it still held
  before it is reused.
* **Routing** (`render_block`): if any instrument track is armed or monitoring
  (`RenderGraph::midi_explicit`), those tracks receive the input; otherwise the
  track set with `se_midi_set_focus` (the selected track). Each instrument track
  has a `TrackMidi` kept across republishes by track id (like its Synth) with
  the keys it holds: a track that stops receiving gets note-offs for them, so no
  note hangs. Instrument devices get the messages ahead of the track's own
  events (after preview notes). The built-in Synth has no live entry point, so
  a second `Synth` per track (`LiveSynth`) plays the keys through the same
  `render()` interface on its own free-running clock: a held key is a note
  that started "now" and has not ended, and a key release removes it.
* **Capture** (`se_midi_capture_enable` / `se_midi_capture_read`): while
  enabled and the transport plays, note-ons and note-offs go into a 4096-event
  ring (RT producer, main-thread consumer) with the timeline frame they were
  heard at: block position minus output latency and delay compensation
  (offline renders: no latency), folded back into the loop after a loop wrap
  (`midi_capture_frame`). The app turns them into MIDI clips.
  `se_midi_capture_wrapped` says whether the transport wrapped at the loop end
  during the latest pass, so the app never reads a seek back as a wrap.
* Tests: `tests/test_midi_input.cpp` (fake backend and injection, no hardware);
  bench: the "MIDI input" table of `dstudio_engine_bench`.

### Export

`se_export_wav` renders `[start, end)` linearly with its private transport:
it ignores the live loop and metronome (tested by
`export_ignores_loop_and_metronome`). With plugin delay compensation it
renders `L` more frames through private delay lines and drops the first `L`,
so timeline frame `t` is file frame `t - start` (`pdc_export_is_timeline_aligned`). Instrument tracks render through
private `Synth` objects created for the export, so exporting never touches
live voices (tested: live output with an export in the middle is identical
to an uninterrupted render, and the exported file matches it bit for bit).

### Blocks, meters, stats

* Scratch buffers are preallocated to `max_block_size` (1..16384). A device
  or offline request larger than that is processed in `max_block_size`
  chunks inside one `process()` call; output is bit-identical for any block
  partitioning (tested).
* Meters: master + one per track (post fader/pan), stored as atomics and
  raised with a lock-free max. `se_meters_read` returns master first, then
  tracks in session order, and **resets** each peak it reads (peak-hold since
  last read; decay/ballistics are left to the UI). Track meters belong to the
  current graph, so they restart at 0 when a new snapshot is published.
* `se_meters_peek` is the non-destructive variant (same ordering): the max
  peak over a sliding window of 5 buckets × 10 ms of *rendered* frames
  (preallocated atomic rings, master in the engine, tracks in the graph's
  `TrackMeter`). Each render segment's peak goes into the bucket it ends in;
  buckets entered are zeroed by the RT side. So values are stable however
  fast the UI polls, decay to 0 after 40–50 ms of rendered silence (paused
  output counts), and hold while nothing renders. One block longer than the
  window counts as a single bucket. Neither call disturbs the other. Both
  meter kinds exclude the metronome.
* Stats are recorded per `process()` call (device callbacks and offline
  renders alike): count, last/avg/max µs, the block deadline
  (`frames / sample_rate`) and the number of calls that exceeded it.
* FTZ/DAZ is enabled for the duration of each `process()` call.

### SIMD

The mixing kernels are plain loops over `__restrict` pointers that clang
auto-vectorises (verified with `-Rpass=loop-vectorize`: width 4, interleaved
2–4 on arm64). The max-abs peak reduction has explicit NEON and SSE2 paths,
because `std::max` semantics prevent auto-vectorisation without fast-math.

### Multithreading (future)

Not in this milestone: all tracks render on the audio thread. Tracks are
independent until the master sum and the graph is a flat read-only array, so
a worker pool can later render disjoint track ranges into per-worker scratch
buffers (allocated with the graph or the context, never on the RT thread) and
the audio thread then sums them. Buses/sends will need a precomputed
topological order in the graph; see the comment in `render_graph.hpp`.

## Real-time safety rules

Inside `Engine::process()` / `render_block()` and the device callback:

1. No heap allocation or deallocation (enforced by a test that replaces
   global `operator new/delete` and counts calls made inside an RT scope:
   zero allowed across 650 renders with seeks/pauses/stops, chunked blocks,
   32 tracks × 4 clips, and a graph swap; and across 1500 renders with loop
   wraps, metronome onsets/tails/voice stealing, while other threads call
   `se_meters_peek`/`se_meters_read` and push loop/metronome/seek commands
   that the render drains; and across 1500 renders of 8 instrument tracks ×
   200 notes with a loop, seeks and pause/play, while another thread
   republishes four different note sets as fast as it can; and across 1200
   renders of 8 instrument tracks whose `se_synth_params` (waveforms, filter
   mode, unison, polyphony incl. mono + glide, FM, LFO, ...) are republished
   every 3 blocks by the render thread and continuously by another thread);
   and across 650 renders of 8 audio tracks × (CLAP gain + VST3 gain) plus an
   instrument track (CLAP sine, 200 notes, CLAP gain insert) with a loop,
   seeks, pause/play, queued parameter changes and a republish. This
   measures **our** code around plugins with our own test plugins, which do
   not allocate; third-party plugins may allocate or lock in their process
   call, which is outside the engine's control.
   Synth parameter changes are applied on the RT side without allocation
   (plain struct copy + coefficient math). Also run ad hoc under
   ThreadSanitizer (MIDI, synth, RT, transport and concurrency tests clean).
2. No locks, no waiting inside `process()`: only atomics, the SPSC pop, and a
   CAS loop for meter max that only retries if a reader raced it. (The
   `se_render_offline` *wrapper* may yield while a non-RT thread holds the
   brief transport "control" claim; the device callback never waits.)
3. No I/O, logging, or exceptions; everything is `noexcept`.
4. Never free a graph or asset; only the non-RT side reclaims.
5. No first-touch thread-local storage (`se_last_error` is only written on
   failure paths, never by a successful `se_render_offline`).

## WAV support

Reader (hand-written RIFF parser): `WAVE_FORMAT_PCM` 8-bit unsigned and
16/24/32-bit signed, `WAVE_FORMAT_IEEE_FLOAT` 32/64-bit, and
`WAVE_FORMAT_EXTENSIBLE` around either (e.g. 24-bit in 32-bit containers).
Odd-sized chunks/padding and unknown chunks are handled. A `data` chunk that
claims more bytes than the file holds is clamped. **Mono and stereo only:
files with more than 2 channels are rejected** with
`SE_ERR_UNSUPPORTED_FORMAT` (a real downmix needs a channel-layout policy).
RF64/BW64 and compressed formats are rejected.

Writer: streaming `WAVE_FORMAT_IEEE_FLOAT` 32-bit with a `fact` chunk
(round trip is bit-exact, tested). Outputs over 4 GiB are refused.

## Resampling

Assets are converted to the engine rate on import with a Kaiser-windowed sinc
(β = 9, 32 zero crossings per side, 512-phase table with linear interpolation
between phases, cutoff 0.94 × the lower Nyquist). Measured in tests:
44.1 k → 48 k keeps a 1 kHz sine at the right frequency (±0.01 Hz) and
amplitude (±0.05 dB) with error below −66 dBFS against an ideal sine;
96 k → 48 k rejects a 30 kHz tone (alias at 18 kHz) by more than 80 dB.
Output length is `llround(frames × dst / src)`, time-aligned with no added
latency. It is offline-quality but unoptimised (scalar, about 70 taps per
output sample).

## Sample rate

`se_config.sample_rate` fixes the engine rate for the engine's lifetime
(assets are resampled to it). `0` means "use the default output device's
nominal rate" (falls back to 48 kHz). If the hardware runs at another rate,
the Core Audio output unit's built-in converter resamples.
`se_engine_sample_rate` reports the engine rate.

## Input, monitoring and recording

(ADR: `docs/adr/0002-recording.md`.)

### Devices (`se_device_list`, `se_device_configure`, `se_device_status_get`)

(Windows: WASAPI, Linux: PipeWire, see [Platforms](#platforms); the rules
below are the macOS ones, the other backends follow the same contract.)
Enumeration reads Core Audio properties only (UID, name, input/output
channel counts from the stream configuration, the standard nominal rates
inside `AvailableNominalSampleRates`, default input/output); it never opens
a device. `se_device_configure` validates first, again without opening
anything: unknown UID → `SE_ERR_NOT_FOUND`; a device without the needed
channels, an unsupported rate, a buffer outside 16..4096 or the device's
`BufferFrameSizeRange`, or an input combined with a rate other than the
engine rate → `SE_ERR_INVALID_ARG`. Stopped: the setup is stored for the
next `se_device_start`. Running: the device is stopped and restarted in
place (the engine keeps RT ownership across the restart, so no offline
render can slip in, and queued transport commands wait for the new stream).

**Input is opened lazily.** A fresh engine (and `se_device_start` without a
prior configure that enabled input) opens the output only, exactly as
before. An input is opened only after `se_device_configure` with
`input_enabled` (Rust: `DeviceConfig { input: Some(uid) }`, or
`Some(DeviceConfig::DEFAULT_INPUT)` = `""` for the system default input;
`input: None` = output only). If the output opens but the input
does not (microphone permission denied/restricted, which we read via
`AVCaptureDevice authorizationStatusForMediaType:` through the ObjC runtime
without prompting; aggregate creation failure; device gone; rate mismatch),
the device restarts **output only**, `se_device_status.input_open = 0`,
`input_error` explains, and a running reconfigure returns `SE_ERR_DEVICE`
with that message. Output keeps working.

### Duplex on macOS: private aggregate device (and why)

Input and output on different Core Audio devices (MacBook microphone +
speakers) run on independent clocks. We create a **private, non-persistent
aggregate device** (`kAudioAggregateDeviceIsPrivateKey = 1`, visible only to
this process, destroyed on stop and by the system if we crash) whose
sub-device list is [output, input], main (clock) sub-device = the output,
and **drift compensation on the input sub-device** (high quality). One
`HALOutput` AUHAL runs on it with input enabled; inside the output render
callback we pull input bus 1 with `AudioUnitRender` into preallocated
buffers, so each IO cycle delivers matching input and output frames. If
input and output are the same device, the AUHAL runs on it directly (no
aggregate).

Chosen over a separate input AUHAL + lock-free ring + adaptive resampler
because: the HAL's drift compensation is what Logic/GarageBand rely on and
is maintained by Apple; it keeps a single IO thread and a single callback,
so input and output are inherently sample-aligned per cycle (no ring fill
level to servo, no extra buffering latency, no second RT thread); and the
HAL reports the aggregate's latency and safety offsets for compensation.
Costs: aggregate creation takes up to a few hundred ms, and AUHAL does not
resample on its input side, so with an input the hardware is switched to the
engine rate (devices that cannot run at it are rejected by validation).
Output-only setups keep `DefaultOutput` (follows the system default) or a
`HALOutput` bound to the named device, with the unit's converter if the
device rate differs from the engine rate.

### Routing and monitoring (`se_track_desc_v4`)

Per track: `input_mode` (none / mono / stereo) and `input_channel`. Mono
feeds one device channel to both sides of the track, then the pan law;
stereo feeds `channel, channel+1` (pan = balance). Missing channels read as
zeros. `render_block()` looks at input only if the graph has a routed track
(`RenderGraph::uses_input`), so sessions without input cost one branch.

* `monitor` on: the input is summed with the track's clips/notes and goes
  through the track's gain, pan, mute, solo, meters and the master, **also
  while the transport is stopped**.
* **Armed, not monitored ("meter input when armed")**: the input is not
  heard, but the track meters (`se_meters_read` and `se_meters_peek`) show
  the raw input peak per side, **pre-fader**, so levels can be set before
  and during recording. (Armed and monitored: the normal post-fader meter.)
* `se_input_set_muted(1)` replaces every input channel with zeros
  (monitoring, input meters and recordings; recordings keep their timing).

### Recording

```
audio thread (process, per segment)            writer thread (~every 5 ms)
-----------------------------------            ---------------------------
rec = rec_live_ (loaded once per block)        pop markers; read each ring up
for each armed track:                            to its head; at a marker:
  new take?  -> push {start, ring index}         finish the WAV, open the next
             to the track's marker SPSC        write frames, update live takes
  ring full? -> drop block, count overrun,       and 64-frame min/max peaks
             end the take                         (mutex, never seen by RT)
  else       -> interleave into float SPSC
             ring, publish head (release)
```

`se_record_start(dir)` takes the armed tracks with an input from the
published graph (fixed for the take), allocates one ring per track (2 s at
the engine rate, power of two), starts the writer thread, publishes the
recorder to the RT side and queues Play (no-op if playing). From the next
block, every playing segment is copied raw (pre-fader, pre-mute) into the
rings. **RT argument:** the audio thread only does `memcpy`-style copies into
preallocated memory, two SPSC pushes with acquire/release atomics, and
relaxed counter increments: no allocation (tested under the allocation
tracker with 8 armed+monitored tracks, seeks, loop wraps and input mute), no
locks, no I/O, no waiting. A full ring drops that block for that track and
counts it (`se_record_stats`); the take ends and the next block that fits
starts a new take at its own timeline position, so takes stay sample-aligned
and the audio thread never blocks on the disk.

**Takes and transport.** A take is a run of contiguous playback. A seek or
stop (or any jump) ends it; the next captured block starts a new take at
the new position. Pause + play at the same position continues the take.
**Recording while looping writes linearly** (no take lanes): the loop wrap
does not end the take, the file just keeps growing, and the take is placed
from its start frame (so it extends past the loop end on the timeline).
`se_record_stop` leaves the transport as it is.

**Stop.** `rec_live_ = null` (seq_cst), then wait while the RT side's
`rec_in_use_` flag is set (it is set before loading `rec_live_` at block
start and cleared at block end: a Dekker pair, so either the block sees null
or the stopping thread waits for that block). Only the non-RT stopping
thread waits. Then the writer drains everything left (also when stalled),
patches every WAV header (RIFF/data/fact sizes) and exits. Failed writes are
counted in `write_errors` and those takes are left out of the result.

**Latency compensation.** In a duplex cycle the input sample delivered with
output frame P was captured when the listener heard frame P − (input latency
+ output latency). So `start_frame = position − (in + out)`, with each side =
`kAudioDevicePropertyLatency + kAudioDevicePropertySafetyOffset +` the first
stream's `kAudioStreamPropertyLatency` (no IO-buffer term: an acoustic loopback test on a MacBook Air showed the buffer term over-compensated by ~18.5 ms; without it recordings land +2.7..6.2 ms late, consistent with speaker→mic travel and onset detection; see crates/dstudio-engine/tests/real_loopback.rs), read from the running
(aggregate) device; for an aggregate each side is the max of what the
aggregate and the sub-device report. Converted to engine frames. Injected
input (`se_render_offline_with_input`) has device compensation 0. With
plugin delay compensation the performer hears everything `L` later, so
takes are additionally placed `L` earlier: `start_frame = position − (in +
out) − L`; a change of `L` while recording ends the takes like a seek (see
[Plugin delay compensation](#plugin-delay-compensation)). For an interface
that reports its latency wrongly, `se_record_set_offset` adds a manual
offset in engine frames (spec 020; the app's Settings › Audio › Recording
offset, kept per input device): `start_frame = position − (in + out) − L −
offset`, positive placing takes earlier. It applies to injected input too,
not to MIDI capture, and a change while recording ends the takes like a
change of `L`.

**Live view.** `se_record_state` lists every take (track, start, frames as
written by the writer, so ≤ ~5 ms behind); `se_record_peaks` gives min/max
pairs of a track's newest take at 64-frame resolution. Both take the
recorder mutex for a copy and never touch the RT side.

**Files.** `rec-<YYYYmmdd-HHMMSS>-<n>-t<track>-<take>.wav` in `dir`
(created if missing), 32-bit float at the engine rate, mono or stereo; an
existing name gets a `_2`, `_3`, ... suffix.

### Tests (no real input is ever opened)

`tests/test_recording.cpp` uses injected input or a fake `AudioDevice`
(`test_hooks.hpp`): mono/stereo routing through the pan law (bit-exact),
monitoring while stopped, monitor off, input + clip sum, armed-not-monitored
meters equal to the raw input peak with silent output, input mute, track
mute/solo; recording a ramp across odd block sizes (127/1/333/1000/64/255,
bit-exact file, exact `start_frame`, live state and peaks), two tracks on
different channels (mono + stereo), seek → new take, loop → one linear take,
pause/play continuity, error cases, writer-stall overruns (exact counts,
both takes aligned and valid after stop), zero allocations while recording
and monitoring 8 tracks, the fake device path (default start never enables
input, input failure keeps output running and reports the error, take
`start_frame` compensated by input + output latency), configure validation
against real Core Audio (no start), and a real output-only named-device
start + running reconfigure. Rust: `crates/dstudio-engine/tests/recording.rs`
(Document → snapshot → injected input → take → `load_file` → "Record" Batch →
playback renders the same samples).

### Verification status

Not verified on hardware by the engine agent, by design (it would trigger
the microphone prompt): opening a real input, the aggregate device and its
drift compensation, AUHAL input pull, and the latency values Core Audio
reports for a real duplex setup. Run `DSTUDIO_TEST_REAL_INPUT=1
./build/record_check` by hand to check level and reported latency; a
loopback measurement (play a click, record it) is the next step to confirm
the compensation is sample-accurate on real devices.

## Plugins

Audio Unit (v2 API, macOS), VST3 and CLAP hosting for track insert chains and
instrument plugins (docs/adr/0003-plugins.md). SDKs: CLAP `1.2.10` headers and
VST 3 SDK `v3.8.1_build_84` (`pluginterfaces` + `public.sdk` submodules),
both MIT, fetched by CMake at configure time and never committed
(`third_party_licenses.md`).

### ABI (`se_session_publish_v5` and `se_plugin_*`)

`se_track_desc_v5` = `se_track_desc_v4` (same offsets) + `devices` /
`device_count` (inserts in processing order), `instrument_plugin` (used when
`instrument != 0`; replaces the built-in synth) and `track_name` (editor
titles; the Rust side passes NULL because `TrackSnapshot` has no name, so
titles are just the plugin name). Each `se_device_desc` carries `device_id`
(dstudio_core `DeviceId`), `format` (`SE_PLUGIN_AU/VST3/CLAP`), `plugin_id`
(AU `"aufx lpas appl"`, VST3 class UID as 32 hex chars in TUID byte order,
CLAP id), `path` (bundle), `name`, `enabled` and the saved `state`
(pointer + length). Layouts are guarded by `-Wpadded` (no implicit padding),
`static_assert`s in `c_api.cpp` and the Rust test
`abi_layout::plugin_layout_matches_c_header`.

Other entry points: `se_plugin_is_live` (lets Rust skip base64-decoding state
for devices that already have an instance), `se_plugin_load_errors`,
`se_plugin_save_states` + `se_plugin_saved_state` (snapshot of every live
instance's state into an engine-owned buffer), `se_plugin_editor_open/close/
is_open`, `se_plugin_latency`, `se_plugin_idle`. Rust: `Engine::publish`
(now v5), `plugin_states`, `plugin_load_errors`, `open_plugin_editor`,
`close_plugin_editor`, `plugin_editor_is_open`, `plugin_latency`,
`plugin_idle` (new, optional), `scan_plugins`, `plugin_scanner_path`,
`test_plugin_dir`.

### Threads and lifetime

```
main thread (publish)                      audio thread (RT owner)
---------------------                      -----------------------
build graph (validate, fx slots)
lock plugin host
  new device id  -> create instance          render_block():
  (AU: AudioComponentInstanceNew,              source = clips + instrument plugin
   VST3: IComponent + IEditController,                  (NoteFeeder -> MIDI) | synth
         connection points, setState,                + monitored input
         setupProcessing, setActive,          -> enabled inserts, in order,
         setProcessing(true);                    process in place
   CLAP: entry->factory->create->init,       -> gain/pan, meters, master
         state load, activate)
  known id       -> reuse instance
install graph (atomic swap, seq S)
retire ids not in S: close editor,
  queue instance with S
destroy queued instances when
  rt_active_seq_ is idle or >= S
```

* **Instantiation** happens in `se_session_publish_v5` on the calling thread,
  which must be the app's main thread (VST3/CLAP main-thread APIs; AU uses the
  synchronous `AudioComponentInstanceNew`). Instances are prepared at the
  engine rate and `max_block_size`, stereo in/out; mono main buses are mapped
  (L+R)/2 in, mono out to both sides; other layouts are rejected with a load
  error. The saved state is applied once, at creation. Instances are keyed by
  `DeviceId` and reused across republishes (also when a device moves to
  another track or is reordered); the same id with a different plugin or role
  is re-created.
* **Load failures** (missing bundle/component, wrong id, init/state/setup
  failure, unsupported layout) leave a null slot: inserts pass audio through,
  an instrument plugin renders silence. The reason is kept per device
  (`se_plugin_load_errors`) and loading is not retried while the device stays
  published with the same plugin (re-adding it, e.g. undo, retries).
* **Retirement**: a device id missing from the new snapshot is retired after
  the graph swap. Its editor closes immediately; the instance is destroyed on
  the main thread (inside a later publish, `se_plugin_*` call or
  `se_plugin_idle`) once the audio thread cannot reach a graph that still
  references it, using the same hazard protocol as graph reclamation
  (`rt_active_seq_` idle or ≥ the first graph without it). Teardown follows
  the formats' rules: CLAP `stop_processing` (we take the audio-thread role
  once; it can no longer overlap with `process`) → `deactivate` → `destroy`
  → `entry->deinit` when the module's last instance goes; VST3
  `setProcessing(false)` → `setActive(false)` → disconnect → `terminate` →
  release → `bundleExit`. Engine destruction tears everything down on the
  destroying thread (destroy the engine on the main thread). Plugin
  executables are never unloaded.
* **RT path**: `render_block` calls `PluginInstance::process` with
  preallocated buffers (input copies, outputs, VST3 `IEventList` /
  `IParameterChanges` of fixed capacity, CLAP event storage, AU
  `AudioBufferList`). Tracks with plugins are processed every block, also
  while stopped (tails ring out) and when muted (state stays consistent; not
  mixed). Bypassed devices are skipped. No allocation, lock or I/O on our
  side.
* **Instrument plugins**: `NoteFeeder` turns the track's notes into
  sample-accurate note on/off (AU `MusicDeviceMIDIEvent` with sample offsets
  before `AudioUnitRender`; VST3 `kNoteOnEvent/kNoteOffEvent`; CLAP
  `clap_event_note`, or `clap_event_midi` for MIDI-only note ports). Like the
  built-in synth it re-matches sounding notes by (start, pitch) on republish
  and sends note-offs for every sounding note on seek, stop, pause and loop
  wraps (+ CC 123 for AU); notes in progress at a new position are not
  re-triggered. Up to 128 sounding notes and 512 events per segment are
  tracked.
* **Transport info**: tempo = `se_transport_set_tempo` (frames per quarter
  note, independent of the metronome's click grid and of whether it is on;
  120 BPM if never set), or with a tempo map the tempo at the block's first
  frame and the song position from the map ([Tempo](#tempo)), 4/4, playing
  flag and position: AU host callbacks
  (`beatAndTempoProc`, `transportStateProc2`), VST3 `ProcessContext`
  (tempo, project time in samples and quarter notes, playing), CLAP
  `clap_event_transport`.
* **Parameters** (no automation yet): VST3 controller edits
  (`IComponentHandler::performEdit`) are forwarded to the processor through a
  lock-free queue as `IParameterChanges` at the next block; the same queue
  backs the test hook `dstudio::testing::plugin_set_parameter` (AU
  `AudioUnitSetParameter`, CLAP `CLAP_EVENT_PARAM_VALUE`).
* **Export** (`se_export_wav`) of a session with plugins uses the live
  instances: it takes the RT ownership (fails with `SE_ERR_STATE` while the
  device runs) and pins its graph with the hazard marker; instrument plugins
  get all-notes-off before and after.

### Plugins in offline renders (private engine on a worker thread)

The app's clip renders (Consolidate, Bounce, Export audio) render on a private, device-less engine
on a worker thread, while the main thread has to stay free. Plugins want the main thread for
creation, state, latency and teardown (CLAP calls the creating thread its main thread, VST3 and AU
instantiate there, and `publish` creates), but their `process()` may run on any thread. So a
render with plugins is split:

1. **Main thread, one device per UI tick:** `se_plugin_prepare` creates the instance now, with the
   state the caller passes (the app reads the live instance's current state with
   `se_plugin_save_state`, one device per call, the same bytes a project save would store). A
   plugin that fails to load returns `SE_ERR_NOT_FOUND` with the plugin's reason, and the render
   stops before any audio or file exists. Instances are reused by the later publish when the
   device id, plugin and role (insert or instrument) match.
2. **Worker:** `se_plugin_set_detached(engine, 1)`, then publish and `se_render_offline`. A detached
   engine never instantiates a third-party plugin (a device that was not prepared reports "not
   prepared on the main thread" in `se_plugin_load_errors` and passes through), never services restarts,
   polls latency, calls `on_main_thread` or destroys instances; `se_plugin_idle` only attaches finished
   warp renders. Built-in devices have no thread affinity and are created as usual. The latency each plugin
   reported when it was prepared feeds delay compensation; the Rust side renders `L` frames more than
   asked and drops the first `L`, so the file lines up with the timeline like the audible playback.
3. **Main thread:** `se_engine_destroy` tears the instances down once the worker is done with the engine
   (Rust: the `offline::Session` travels back to the UI thread with the result and is dropped there).

Plugins run in real-time process mode, as in playback (no VST3 `kOffline` or AU offline flag): a
plugin that sounds different when it knows it is offline would make the file differ from what the user
hears. The project's tempo is set on the private engine (`se_transport_set_tempo`) so tempo-synced plugins see it.
Limits: a plugin's own restart request (CLAP `request_restart`, VST3 `restartComponent`) is ignored for
the length of a render (its latency stays as prepared); on Windows and Linux, CLAP `request_callback`
is only served by `se_plugin_idle`, which a detached engine skips (macOS dispatches it to the main queue).
Tests: `offline_*` in `tests/test_plugins.cpp` (a worker render of prepared CLAP, VST3, AU and instrument
plugins equals the main-thread render bit for bit with zero RT allocations; detached engines never
instantiate; load failures; one-instance state save; latency in the plan), benchmark
`build/dstudio_offline_plugin_bench`.

### State

`se_plugin_save_states` runs on the main thread while audio may run: AU reads
`kAudioUnitProperty_ClassInfo` and serialises it as a **binary plist**; VST3
uses `IComponent::getState` + `IEditController::getState` in an **"SVS3"
container** (`"SVS3"`, u32 version 1, u64 length + component bytes, u64
length + controller bytes; little endian) and restores with
`setState` → `setComponentState` → controller `setState`; CLAP uses the
`clap.state` stream as is. Rust base64-encodes for
`Document::sync_device_states` and decodes (only for devices that will be
instantiated, `se_plugin_is_live`) on publish.

### Editors (macOS; Windows and Linux: see [Platforms](#platforms))

`se_plugin_editor_open` (main thread only; checked with `pthread_main_np`
on macOS, tid == pid on Linux, the initial thread on Windows; else
`SE_ERR_STATE`) creates an NSWindow titled `"<plugin>: <track>"` (or the
plugin name) hosting: the AU's `kAudioUnitProperty_CocoaUI` view
(`AUCocoaUIBase`), else CoreAudioKit's `AUGenericView`; a VST3 `IPlugView`
attached to an NSView (`IPlugFrame::resizeView` and user resizing through
`checkSizeConstraint`/`onSize` when `canResize`); a CLAP `clap.gui` embedded
cocoa view (`request_resize`, `adjust_size`/`set_size`). The window's close
button tears the view down and `se_plugin_editor_is_open` returns 0. Editors
of retired instances close in the publish that retires them. Plugins without
an editor return `SE_ERR_UNSUPPORTED_FORMAT`.

### Scanning (`dstudio_plugin_scanner`)

`scan_plugins` (Rust) runs `dstudio_plugin_scanner --scan --cache <file>
[--rescan]` and reads progress lines from its stderr and a JSON report from
its stdout. The scanner is found at `$DSTUDIO_PLUGIN_SCANNER` or the path
baked in by build.rs (`<cargo out>/build/dstudio_plugin_scanner`; the app must
ship it next to its binary and set the variable, or adapt the lookup). The
coordinator enumerates AUs (macOS; types `aufx`, `aumf`, `aumu`; enumeration
loads no plugin code), VST3/CLAP bundles in `/Library/Audio/Plug-Ins/{VST3,CLAP}`,
`~/Library/Audio/Plug-Ins/{VST3,CLAP}` (Windows/Linux folders: see
[Platforms](#platforms)) and the `$DSTUDIO_PLUGIN_PATH` entries (`:`-separated,
`;` on Windows; directories searched up to 3 levels, or bundle
paths). The app's custom plugin folders (Settings › Plugins › Add folder) reach the
scanner the same way: `scan_plugins_in` puts them first in the child's
`$DSTUDIO_PLUGIN_PATH`; a folder that does not exist is skipped. Every AU and every bundle not in the cache with the same stamp (AU
version; newest mtime of the bundle, its Info.plist and its binaries in
`Contents/MacOS`, `Contents/<arch>-win` or `Contents/<arch>-linux`; a
single-file plugin's own mtime)
is inspected by its own child process (`--scan-one`, which instantiates the
plugin to read bus channels and editor support), at most 4 at a time, with a
timeout (`$DSTUDIO_PLUGIN_SCAN_TIMEOUT_MS`, default 10000). Crashes (signal;
Windows: an NTSTATUS exception exit code, with error dialogs suppressed in
the child), timeouts (SIGKILL / TerminateProcess) and failures are recorded in the cache and reported in
`failed`; they stay blocklisted until `--rescan` or a stamp change. The cache
(`{"version":1,"entries":[…]}`) is written atomically.

### Test plugins (`build/test_plugins/`, original code)

macOS: bundles. Windows/Linux: each `.clap` is a single shared library and
each `.vst3` a bundle folder with the binary in `Contents/x86_64-win/` or
`Contents/x86_64-linux/`.

* `DStudioTestGain.vst3`: processor + separate controller (connection points),
  stereo, parameter 0 "Gain" (normalized = linear 0..1, default 0.5), state =
  f32 LE.
* `DStudioTestGain.clap` (`studio.dstudio.test.gain`): same as a CLAP.
* `DStudioTestSine.clap` (`studio.dstudio.test.sine`): 16-voice instrument, a
  cosine at the equal-tempered note pitch, amplitude 0.25 × velocity, full
  amplitude on the note-on sample and silent from the note-off sample (makes
  onsets/offs exactly measurable).
* `DStudioTestLatency.clap` (`studio.dstudio.test.latency`) and
  `DStudioTestLatency.vst3` (class `5A17E5A16E0000038A1E7E576A170003`):
  stereo effect that delays its input by exactly N frames and reports
  latency N. N = state (u32 LE; VST3 component state inside the SVS3
  container), default 0. Parameter 0 "Latency" changes it at runtime through
  the format's protocol: CLAP plain value = frames → stored, `request_restart`
  → applied (buffer allocated) in `activate`, `latency.changed`; VST3
  normalized = N / 2^20 → the controller calls
  `restartComponent(kLatencyChanged)`, the processor applies the value it
  received in the next `setActive(true)` (so: set parameter → render a block
  → `se_plugin_idle`). Used as an instrument plugin it is silent but still
  reports N.
* `DStudioTestCrash.clap`: `clap_entry.init` raises SIGSEGV (Windows: writes
  through a null pointer, since `raise(SIGSEGV)` there is a clean exit).
* `DStudioTestGui.clap` (`studio.dstudio.test.gui`, **Windows only**): stereo
  pass-through with a Win32 `clap.gui` (a child `STATIC` window) that asks the
  host to resize to 400 × 250 when shown; drives the HWND editor test.

### Tests

C++ (`tests/test_plugins.cpp`, `tests/test_plugin_scan.cpp`, both presets;
off macOS the AU-only tests skip, AU halves of mixed tests are left out and
the "third plugin" is DStudioTestLatency.clap):
AULowpass attenuates 15 kHz (< 0.25×) and passes 200 Hz (±5 %); CLAP and VST3
gain halve the signal (≤ 1e-7); bypass (AU + CLAP + VST3 all disabled) is
bit-exact; chain order (CLAP gain ↔ AUDistortion) changes the output, and
CLAP→VST3 gain gives exactly a quarter; instances survive republishes,
reorders, moves between tracks and bypass/mute toggles (instantiation counter
test hook), are retired on removal and re-created on re-add or plugin change;
v5 validation; state round trips for CLAP, VST3 (parameter changed through
the queue → saved → new engine → identical output) and AU (AULowpass cutoff
→ ClassInfo plist → identical output); corrupt state → load error +
pass-through; DStudioTestSine onset exactly on the note frame, note-off exact,
pitch 440 Hz; DLSMusicDevice onset within 20 ms (measured: 7 frames above
-60 dBFS); seek
and stop send note-offs (sine: silent from the first sample; DLS: same
release as a regular note-off); instrument plugin + insert + loop wraps;
missing AU/CLAP/VST3/wrong ids/missing instrument → 5 load errors, output
bit-identical to no devices, not retried; latency query; editor calls off
the main thread rejected; export through the chain; zero allocations in the
RT path (above). Scanner: Apple AUs + all test plugins found, the crash
blocklisted while the scan completes, second run 100 % from cache, touching
a bundle re-scans only it, `--rescan` re-scans all.

Rust (`crates/dstudio-engine/tests/plugins.rs`): Document `AddDevice` (CLAP test
gain) + audio clip → publish → offline render is halved; bypass bit-exact;
model state applied at instantiation; remove + undo restores the same id and
state; `plugin_states` → `sync_device_states` → `persist::save/load` → new
engine renders identically; missing plugin → load error; `scan_plugins` finds
the test plugins and Apple AUs, blocklists the crash, second scan from cache.

### Verification status

* `tools/plugin_editor_check` was run on this Mac (GUI session): AULowpass
  (Apple's CoreAudioAUUI view) and AUDelay editors, and an installed
  third-party EQ plugin (VST3 and CLAP) editors (950 × 570) open, show a window,
  close via `performClose:` (the close button's action), via the API, and on
  retirement. Window contents were not inspected visually (no screenshot).
  Opening AU views logs Apple's own "Class RendezvousService is implemented in
  both CoreAudioKit and CoreAudioAUUI" warning (harmless, Apple-side).
* An ad-hoc probe loaded a third-party EQ plugin (VST3 and CLAP) and a third-party limiter (VST3)
  from `/Library/Audio/Plug-Ins`: they instantiate in 15-40 ms, process at
  unity with default settings, report latency (the limiter: 3115 frames) and
  round-trip their state into a new engine. Not part of the automated tests
  (third-party, may not be installed).
* Not verified: AUv3-only (out-of-process) Audio Units (synchronous
  instantiation may fail for them; they would show as load/scan errors),
  plugins that require the real `pthread_main_np` main thread for
  instantiation when the publishing thread is a different one (cargo tests
  publish from worker threads; only our test plugins and Apple AUs are used
  there), and editor resizing by the user.

## Plugin delay compensation

Design and rationale: [ADR 0004](../docs/adr/0004-delay-compensation.md).
Summary: per track `lat_i` = Σ insert latencies (**bypassed inserts
included**) + instrument plugin latency; `L = max lat_i` over all tracks
(**muted / soloed-out included**); each track is delayed by `L − lat_i`, so
the whole mix (and the metronome) is `L` late. On by default
(`se_pdc_set_enabled` / Rust `set_delay_compensation`). This formula and the
diagram below describe sessions without routing; with return tracks a return
also adds its own chain to the latency of what feeds it, see
[Return tracks and sends](#return-tracks-and-sends).

```
per track (render_block), PDC lines only where the plan needs them:
  clips ─[src line: li, instrument-plugin tracks with clips]─┐
  built-in synth / instrument plugin (latency li) ───────────┴─ sum ─┐
                                    monitoring track: [main line: L − lat_i] here
  monitored input ──────────────────────────────────────────────────┴─ insert chain ─┐
     (bypassed slot with latency x: x-frame line instead of the plugin)             │
                                    other tracks:     [main line: L − lat_i] here ──┴─ gain/pan ─ master
master ─ + [metronome line: L] (after metering, as before)
```

* **Plan.** `Engine::plan_pdc` (main thread, plugin host mutex) turns the
  instances' cached latencies into `RenderGraph::pdc` (`PdcPlan`: `L`,
  clamping, per-track `lat_i`, the delay each line needs) plus per-track
  `pdc_delay` / `pdc_src_delay` / `pdc_pre` and per-slot `pdc_bypass_delay`.
  `L = 0` or switched off → empty plan, no lines, and the render path is
  bit-identical to the engine before PDC (golden-hash test, below).
* **Re-plan.** Publish computes the plan for the new graph. A latency change
  or the switch clones the current graph with a new plan and installs it
  (same instances, synths and lines; meters keep their values). Every graph
  install (v1..v5 publishes, re-plans) runs under the plugin host mutex.
* **Latency changes.** CLAP `request_restart` / VST3
  `restartComponent(kLatencyChanged)` flag the instance. `se_plugin_idle`
  (and `se_session_publish_v5`) restarts flagged instances on the main
  thread — `PluginInstance::suspend()` (Dekker pair with `run()`: the RT side
  bypasses the instance — effects pass through, instruments are silent —
  and the main thread waits for a `process()` in flight), CLAP
  `stop_processing`/`deactivate`/`activate`, VST3 `setProcessing(false)`/
  `setActive(false)`/`setActive(true)`/`setProcessing(true)`, `resume()` —
  then re-reads every instance's latency (AU changes are picked up by this
  poll) and re-plans if any changed. **The app has to call `plugin_idle()`
  regularly on the main thread** (10-60 Hz).
* **Delay lines** (`src/delay_line.hpp`): power-of-two ring ≥ delay + max
  block, one memcpy-in / memcpy-out per channel and block. Allocated (zeroed)
  in `install_graph` on the publishing thread, carried over by track /
  device id (reused when big enough); a changed delay reinterprets the
  line's content (one-time jump). Cap `SE_PDC_MAX_DELAY_FRAMES` = 2^17
  frames per delay (~2.7 s at 48 kHz; ≤ 2 MB per stereo line); beyond it the
  delay is clamped and `clamped` reported.
* **Transport.** Seek and stop flush the lines (lazily: a generation counter
  bumped by the RT owner; a line treats older frames as silence on its next
  use). Pause rings out the last `L` frames. Loop wraps keep the lines
  running (the loop end is heard after the wrap, then its start). Plugins'
  internal buffers are not flushed on seek; their stale tail comes out
  within `lat_i` frames, before the compensated content. Idle lines only
  advance an index; muted / soloed-out tracks drop their lines' content.
* **Monitoring.** A track that monitors input applies its `L − lat_i` to the
  playback source only, before the chain; the input joins after that delay
  (heard with `lat_i` + device latency only). Toggling monitoring moves the
  line (one-time discontinuity on that track).
* **Recording / export / audible position.** Takes: `start_frame` −= `L`
  (new takes when `L` changes mid-recording). Export: `L` extra frames
  rendered, first `L` dropped. `se_transport_audible_position` = position −
  `L` − output device latency (`se_device_status.output_latency_frames`,
  0 offline / device stopped) while playing, else the position.
* **Report.** `se_pdc_report_get`: applied `L` (`total_frames`, 0 when off),
  `uncapped_frames`, `clamped`, and per track `lat_i` (also when off) and
  the applied delay. Rust: `latency_report()`, `audible_position()`,
  `delay_compensation_enabled()`, `PDC_MAX_DELAY_FRAMES`.

Tests (`tests/test_pdc.cpp`, both presets): PDC off bit-identical to the
pre-PDC library (FNV hashes of a scenario with clips, synth, instrument
plugin, CLAP/VST3 inserts, bypass, mute, monitoring + recording, metronome,
loop, seeks, pause/stop, odd blocks and export, produced with the library of
commit bf473ce in Release and ASan builds; `tests/pdc_golden_scenario.hpp`),
also with PDC on and `L = 0`; two tracks + N=1000 → one double impulse at
t + 1000, `audible_position` = position − 1000, off → two impulses 1000
apart; bypass (also toggled mid-play) keeps timing; CLAP+VST3 chain, bypassed
slot, unprocessed track and a muted latent track align (muted counts);
instrument plugin onset aligned; clips on a latent instrument track; clicks
delayed by exactly L (bit-exact vs. off); export timeline-aligned; monitored
input undelayed while its clip is delayed; take `start_frame` includes L
and an input impulse played with the heard impulse lands on its timeline
frame; L change while recording → two aligned takes; runtime latency
changes via the parameter hook (CLAP 1000→2500, VST3 0→4000, both → 0) with
re-alignment; seek/stop flush, pause tail, loop history; cap clamp + report;
zero allocations while rendering with latency changes, bypass, monitoring,
metronome, loop, seeks, pause and on/off toggles. Rust
(`crates/dstudio-engine/tests/pdc.rs`): Document `AddDevice` (DStudioTestLatency,
N = 1000 from the model state) on one of two tracks with the same clip →
aligned, report and audible position; toggle off → misaligned by N; bypass
keeps timing. Manual (not run): `real_loopback.rs`
`acoustic_loopback_latency_with_pdc` (L = 4800).

**Not verified:** acoustic alignment on a real device with plugins (the
loopback test above opens the microphone and was not run), third-party
plugins that change latency at runtime, AU latency changes (polled, but no
test AU changes latency).

## Return tracks and sends

Spec: [002](../specs/002-return-tracks-and-sends/spec.md). ABI: `se_session_publish_v9` = v8 plus
`se_track_route { track_id, output_track_id, is_return }` (one per return track and per track whose output is
not the master) and `se_send_desc { track_id, return_track_id, gain, pre_fader }`. At most `SE_MAX_RETURNS` (16)
returns. A session without routes and sends publishes exactly as v8 and renders bit-identically.

**Validation** (`apply_routing`, `SE_ERR_INVALID_ARG`): unknown track ids, a send or output to a track that is not
a return, a send from a return, a track routed twice, a return routed anywhere but the master, a send to itself,
duplicate sends to the same return, gains outside 0..4 or not finite, clips, notes, the instrument flag, input, arm
or monitor on a return, more than 16 returns.

**Buses.** Each return owns a stereo bus in `RenderContext` (`SE_MAX_RETURNS` buses and one send scratch,
preallocated). A bus is cleared lazily, the first time a block writes to it. A return's source is its bus; from
there it runs like any track: insert chain, fader, pan, meters, automation.

```
ordinary track:
  source ─ chain ─ [main line] ─ strip ─┬─ pre-fader sends ─[edge line]─▶ return bus
                                        └─ fader/pan ─┬─ post-fader sends ─[edge line]─▶ return bus
                                                      └─ output ─[edge line]─▶ master or a return bus
return track:
  bus ─ chain ─ [main line] ─ strip ─ fader/pan ─▶ master
```

**Order.** Processing order is a topological sort over routing edges (track → return) and sidechain edges.
Routing edges are fixed; on a cycle only sidechain edges are cut, as before. Every return renders after every
track that feeds it. The graph code is general (any acyclic set of buses), so group tracks and return-to-return
sends need only looser validation.

**Mute and solo** collapse at publish time into `main_on` (the track's own path to its destination) and
`send_live` (its sends feed returns):

* ordinary track: `send_live = !mute && (!any_track_solo || solo)`; `main_on` = `send_live` when its output is a
  return, else `!mute && ((!any_track_solo && !any_return_solo) || solo)`;
* return (any bus: returns and groups): with no bus soloed, `main_on = send_live = !mute`. With a bus soloed,
  `heard` is the soloed buses plus every bus they reach through unmuted buses' sends, and `feeds` every bus that
  reaches a soloed one: `main_on = !mute && heard`, `send_live = !mute && (heard || feeds)`;
* each send (`SendNode::live`): a send into a bus that is heard but neither soloed nor feeding a soloed bus runs
  only from a heard bus (spec 016).

So soloing ordinary tracks keeps the returns playing (fed by the soloed tracks only, and sending into each other),
and soloing a return plays it, still fed by every live track and by the buses feeding it, plus the returns it feeds,
which play only what it sends them. A session without sends between buses renders as before for every mute and
solo state.

**Delay compensation across buses.** ADR 0004 aligned every track at the master; with buses it aligns every input
of every bus. With `lat_t` the chain latency of track `t`, a return `r` receives at `in_r = max lat_s` over its
feeders and reaches the master at `in_r + lat_r`. `L` is the latest master arrival. Each track is delayed by its
main line to its earliest destination; every later destination gets an edge line for the difference (one per
send, one per routed output). An edge delay is non-zero only when a track's destinations receive at different
times (for example a latent track that feeds the master but not a return other tracks send to); with no plugin
latency anywhere every delay is 0 and no edge line runs. Edge
lines carry across republishes keyed by (track id, destination id). Sidechains reading a return are not
compensated (as for every sidechain).

**Cost** (`dstudio_engine_bench`, "Returns" table, 32 tracks, 64 sends into 2 returns): see the engine PR for
numbers on the CI machine class. A session without returns pays nothing (one branch per track).

**Tests:** `tests/test_routing.cpp` (levels pre/post, output to a return, mute/solo, validation, PDC alignment with
`DStudioTestLatency.clap` on a return, export = live, no allocation on the audio thread; return-into-return
sends, their solo rule and chained delay compensation);
`crates/dstudio-engine/tests/routing.rs` (end to end through the Rust wrapper).

## Automation

Decision and model: [ADR 0005](../docs/adr/0005-automation.md). The core
(`dstudio_core::automation`) owns the lanes and the evaluation contract; the
snapshot sends enabled, non-empty lanes with points converted to frames
(`TrackSnapshot::automation`, `SessionSnapshot::master_automation`), and
`Engine::publish` passes them through `se_session_publish_v6`.

### ABI (`se_session_publish_v6`)

`se_track_desc_v6` = `se_track_desc_v5` (same offsets, copied with `memcpy`
from v5) + `lanes` / `lane_count`; `se_session_desc_v6` adds `master_lanes` /
`master_lane_count` (only `SE_AUTO_VOLUME` is used there). A lane
(`se_auto_lane_desc`, 32 bytes) has `target` (`SE_AUTO_VOLUME/PAN/SYNTH/
PLUGIN/BUILTIN/MUTE/DEVICE_ON/TRIM/WIDTH/MONO`), `logarithmic`, `synth_param` (`SE_SYNTH_P_*`
= index in `SynthParam::ALL`), `device_id` + `param_id` (plugin), `flags`
(`SE_AUTO_FLAG_STEPPED`; the field was padding, so 0 keeps the old meaning)
and `points` (`se_auto_point {int64 frame, float value, float curve}`, 16
bytes). v1..v5
publishes are v6 without lanes. Validation: target/synth index in range,
frames non-decreasing, finite values and curves, curve in −1..1, synth values
in the parameter's range (else `SE_ERR_INVALID_ARG`); empty lanes are
ignored; with several lanes for one target the last wins; a plugin lane whose
device is not on the track, did not load or lacks the parameter is inactive.
Layouts: `static_assert`s in `c_api.cpp`, Rust
`abi_layout::automation_layout_matches_c_header`.
`se_automation_eval` (testing) evaluates a lane with the engine's code.

### Evaluation

`src/automation.hpp` implements `AutomationLane::value_at` over frames
literally: hold the first value at and before the first point and the last
value after the last point, the later point at a vertical step, `shape(s, c)`
= `s^(1+3c)` / `1−(1−s)^(1−3c)` (|c| < 1e-6: linear), `s` computed as
`(float)(t−a) / (float)(b−a)`, geometric `exp(lerp(log a, log b, s))` for
logarithmic lanes, all in `float` with FP contraction disabled
(`#pragma clang fp contract(off)`; Rust never fuses). The cross-language
test (`crates/dstudio-engine/tests/automation.rs`) compares 135 522 values of
200 random lanes (all target kinds, curves incl. ±1 and 1e-7, steps at the
first and inner points) with `value_at`: all are **bit-identical** (required:
1e-5 relative).

### Playback and rates

Each lane **replaces** the static value of its target while it is present
(the static gain/pan/parameter is not multiplied in); removing or disabling
the lane (a republish) restores the static value.

* **Volume / pan / master volume:** per sample. `auto_fill` evaluates the
  lane exactly at the start of runs of ≤ 32 samples that never cross a point
  and ramps the *mapped* gain linearly in between (a run ending on a point
  ends on its left limit, so vertical steps land exactly on their frame). dB
  → gain is `10^(dB/20)`, ≤ −60 dB = 0; a run that leaves or enters silence
  is evaluated per sample (the 0 → 0.001 jump at −60 dB is not smeared).
  Pan lanes produce left/right constant-power gains the same way; the track
  then mixes with `kernel_add_mul2` (per-sample gains). Error vs exact: ≤ 3e-6
  for a −60 → 0 dB ramp; max step between samples equals the exact slope.
  The meters use the block's peak gain.
* **Synth parameters:** the track's synth renders in segments ending on the
  16-frame timeline grid; at each segment start the lane values replace the
  fields of a copy of the track's `se_synth_params`. `Synth::render` now
  compares the parameters on every call (memcmp; unchanged → nothing), so a
  change bumps the parameter generation and the voices approach it through
  the existing ~10 ms one-pole smoothing at their control ticks (16 samples).
* **Plugin parameters:** before each `run()` of a slot with lanes,
  `render_block` builds sample-offset `ParamEvent`s: the value at offset 0 of
  every processed segment (so seeks / loop wraps / re-enable start right),
  every 64 timeline frames and at every point (≤ 64 points per lane and
  segment, beyond that the 64-frame grid only), unchanged values skipped,
  sorted by offset; normalized → native at publish time (VST3 0..1, CLAP
  `min + n·(max−min)` from `clap_param_info`, AU from
  `AudioUnitParameterInfo`). Formats: VST3 — points in `IParamValueQueue`s
  (queues are allocated at setup for `max_block/64 + 74` points); CLAP —
  `CLAP_EVENT_PARAM_VALUE` merged by time with the note events; AU —
  `AudioUnitScheduleParameters` immediate events at their offsets (falls back
  to `AudioUnitSetParameter` with the offset). Up to 2048 events per slot
  and segment (`PluginInstance::kMaxParamEvents`).
* **Switches and choices (stepped lanes):** a lane with
  `SE_AUTO_FLAG_STEPPED` holds each point's value up to the next point (no
  ramp, curves ignored); the core sets it for track mute, device on/off and
  built-in choice parameters, and `AutomationLane::value_at` does the same.
  `SE_AUTO_MUTE` (1 = muted) replaces the track's static mute: a per-sample
  gain of 0 or 1 on the track's signal before the fader, so its sends go
  quiet too; solo and the master are unchanged. `SE_AUTO_DEVICE_ON`
  (`device_id`, 1 = on) replaces an insert's or the instrument plugin's
  `enabled` once per block, read at the block's first frame (delay
  compensated like the slot's other lanes). An insert with an on/off lane
  always gets its bypass delay line, so switching it keeps the track aligned.
* **Channel strip:** `SE_AUTO_TRIM` (dB, clamped to ±24), `SE_AUTO_WIDTH`
  (0..2) and `SE_AUTO_MONO` (stepped, 1 = mono) replace the track's strip
  settings; while any of them is active the strip runs per sample
  (`kernel_strip_varying`: the strip matrix from per-sample trim and width,
  mono folding the width to 0), at the fader's timeline frame.
* **Stopped / paused:** lanes hold the value at the playhead (tails and
  insert chains still run).
* **Export** uses the same `render_block`, so lanes are rendered (linear
  playback, no history needed).

### Alignment with delay compensation

A lane must act on the audio it belongs to, not on whatever passes a stage
at that transport frame. With PDC active, audio reaching a stage was read at
an earlier timeline frame: the fader of every track sees audio read
`lat_i + (L − lat_i) = L` frames earlier (clamped delays: what is actually
applied); an insert sees the instrument plugin's latency plus the latencies
of every earlier slot (bypassed slots are delayed by their latency, so they
count too), plus `L − lat_i` on monitoring tracks (whose line runs before the
chain); the built-in synth and the instrument plugin see 0 (they render at
the transport frame); the master fader sees `L`. `plan_pdc` stores these as
`TrackNode::auto_delay` / `FxSlot::auto_delay` (0 when PDC is off or `L` = 0:
then nothing is compensated and lanes run on the transport frame, like the
rest of the engine).

"d frames earlier" is not `position − d` after a loop wrap or seek: for the
first d samples after a wrap the stage still receives pre-wrap audio. The RT
side therefore keeps a `TimeHistory` (64 playback segments, contiguous
playback merged into one) of which timeline frame each rendered sample was
read at; `pieces()` maps a block's output offsets, seen d samples late, back
to timeline pieces, and every lane evaluation goes through it. Seek/stop
flush the delay lines, so the samples mapped across a seek are silent
anyway. Test: a vertical volume step at 10000 with L = 1000 lands exactly at
output 11000 on the latent track and on the compensated one; with a loop
[8000, 12000) every one of 20000 samples equals `gain(value at the frame its
audio was read at)`; a gain-plugin step after a 500-frame latency plugin
lands on the delayed audio.

### Plugin parameters and editor changes

`se_plugin_params` / `_param_value` / `_param_text` (Rust `plugin_params`,
`plugin_param_value`, `plugin_param_text`; main thread, under the plugin host
mutex): VST3 `IEditController::getParameterInfo` (hidden ones skipped,
`kCanAutomate` and not read-only = automatable, group from `IUnitInfo`),
`getParamNormalized`, `getParamStringByValue`; CLAP `clap.params`
(`CLAP_PARAM_IS_HIDDEN` skipped, module = group, stepped → steps),
`get_value`, `value_to_text`; AU `kAudioUnitProperty_ParameterList` /
`ParameterInfo` (CF names, clump names), `AudioUnitGetParameter`,
`kAudioUnitProperty_ParameterStringFromValue` or the value with its unit
("11885 Hz").

`se_plugin_take_param_changes` (Rust `take_plugin_param_changes`) drains one
engine-wide **lock-free bounded MPMC queue** (`plugins/param_bus.hpp`,
Vyukov ring, 8192 records, push never blocks or allocates, overflow drops
and counts) that every format reports into from whatever thread the change
arrives on:

* **VST3:** our `IComponentHandler` — `beginEdit` (record with
  `gesture_begin`, value = `getParamNormalized`), `performEdit` (value; also
  forwarded to the processor as before), `endEdit` (`gesture_end`).
* **CLAP:** output events of `process()` (audio thread) and of
  `params.flush()`: `CLAP_EVENT_PARAM_GESTURE_BEGIN/END` (value = the last
  value seen for that parameter, an atomic per cached parameter) and
  `CLAP_EVENT_PARAM_VALUE` (normalized with the range cached at creation),
  stamped `segment position + event time`. `host.params.request_flush` sets a
  flag; `se_plugin_idle` runs pending flushes with the audio-thread role when
  it can claim the RT path (no device, no offline render running; a running
  device processes the plugin instead, which delivers the same events).
* **AU:** an `AUEventListener` on a private serial dispatch queue, registered
  for value changes and begin/end gestures of every parameter (what AU views
  send through `AUParameterSet` / `AUEventListenerNotify`). The listener
  block holds a shared state whose instance pointer is cleared under a mutex
  before the instance is disposed.

Frames are the transport position when the change was observed (block
granularity outside CLAP `process()`). Automation is never reported: VST3
automation goes to the processor only, CLAP automation is input not output,
AU automation uses `AudioUnitScheduleParameters` (no notification).

Test hooks: the test gain plugins have hidden, unlisted gesture parameters —
VST3: `setParamNormalized(1000, v)` on the controller performs
`beginEdit/performEdit/endEdit` on parameter 0; CLAP: a `PARAM_VALUE` for
1000 emits the gesture from `process()`, for 1001 from `params.flush()` after
a `request_callback` → `on_main_thread` → `request_flush` round trip.
`dstudio::testing::plugin_simulate_gesture` does an AU view's gesture on an AU.
Both gain plugins now apply their gain sample-accurately at event offsets.

### Tests

C++ (`tests/test_automation.cpp`, both presets): evaluation contract and
validation (`se_automation_eval`), publish validation; volume ramp −60 → 0 dB
over 1 s on a DC clip (max error 3.1e-6, max step 1.44e-4 = the exact slope,
odd block size) and replacement of the static gain; pan (constant power,
error 3.5e-6) and master volume with a step (exact on its frame, `master_gain`
ignored); empty lanes ignored and lane removal restoring the static gain
bit-exactly; loop wraps (every sample right after the wrap), seeks and pause;
synth cutoff sweep 150 Hz → 12 kHz brightens 0.0004 → 0.0023 → 0.026 while
the static patch stays constant; CLAP and VST3 gain lanes reproduced exactly
(0 of 10000 samples off the event-step model, steps between grid points
land on their frame), unknown parameter inactive; AULowpass cutoff sweep
(brightness 0.13 → 1.8 → 2.4); parameter queries for CLAP, VST3, AU;
editor changes for VST3, CLAP (process and flush paths) and AU with
order, flags, values and frames, and no records caused by automation; PDC
alignment (above); export with a latency plugin and a master step (max error
1.1e-5); zero allocations on the RT path with 26 tracks of dense lanes (a
point every 7 frames, curves), CLAP/VST3 gain lanes, a synth with two lanes,
a latency plugin, master lane, loop, seeks, pause and odd block sizes.

Rust (`crates/dstudio-engine/tests/automation.rs`): the cross-language
evaluation test; validation parity; a Document `SetAutomationLane` volume
lane on a DC clip follows `value_at` (worst 4.9e-6), a disabled lane renders
bit-identically to no lane, undo restores the static output bit-exactly;
master lane; a CLAP gain lane through the document and the parameter
queries.

## Clip warping

`se_session_publish_v7` adds a warp description per audio clip: a map of
(timeline frame, source frame) points and one of five modes. The header has
the exact rules; this section explains how the engine runs them.

* **Re-pitch** reads the source at the map's position with the same
  band-limited interpolator as rate-converted clips: pitch follows speed.
* **Slice** (drums) plays each transient slice at the frame where the map
  reaches it, at the transpose rate, with 2 ms fades and crossfades. No
  stretching, so hits stay sharp.
* **Stretch** and **Stretch + formant** run signalsmith-stretch (MIT, fetched
  by CMake, see `third_party_licenses.md`). We never wrote our own stretcher.
* **Auto** slices passages whose slices are flagged percussive and stretches
  the rest, crossfading over 3 ms where the choice changes.

### Real-time safety (`src/warp.cpp`)

`WarpVoice` wraps one stretcher per (track, clip slot, channel count). All
its buffers are sized at publish time for the largest block and the slowest
rate, so `process` only reads and writes reserved memory. The tests
`warp_render_path_does_not_allocate` and `warp_render_swap_does_not_allocate`
count allocations on the render path. Voices move across republishes by key,
so editing gain or another track keeps them running. A voice restarts with
`seek` (no pre-roll latency) at a clip start, after a transport jump, or when
the map moves the source under it.

### Background renders

Live stretching costs about 25–45 µs per track per block (bench below), and a
restart costs one full analysis window. So a background thread
(`WarpRenderer`) renders every stretch-mode clip ahead of time, in large
blocks, keyed by an FNV-1a hash of asset, mode, transpose, map and slices
relative to the clip start. Moving a clip or changing its gain reuses the
render. `se_plugin_idle` swaps finished renders into the live graph, and the
first block after a swap crossfades from the voice. A rendered clip then
costs a copy (about 0.3–0.5 µs per track per block), and a transport jump
costs nothing extra. Renders nobody uses are dropped after 32 newer ones.
A render is limited to 10 minutes of audio. A longer clip keeps stretching
live. `se_export_wav` uses the attached renders and private voices for the
rest.

### Onsets

`se_asset_detect_onsets` finds transients by spectral flux with adaptive
peak picking, then moves each one back to the start of its attack. A slice
is flagged percussive when its tail sits 12 dB under its head, smoothed over
neighbouring slices. The UI uses it for warp markers and the Auto mode.

### Benchmark (`build/dstudio_warp_bench`, 20 s stereo loop, map 112/120)

| Mode | µs per track per block, live | rendered | render time per track-minute |
|---|---:|---:|---:|
| unwarped 1:1 | 0.1–0.4 | – | – |
| Re-pitch | 1.9–3.8 | – | – |
| Slice | 0.9–1.8 | – | – |
| Stretch | 24–47 | 0.3–0.6 | ≈ 0.5 s |
| Stretch + formant | 24–75 | 0.3–0.6 | ≈ 0.6 s |
| Auto (3/4 percussive) | 7–19 | 0.4–0.5 | ≈ 0.3 s |

Thirty-two stretch tracks with a transport jump every 50 blocks average
11 µs per block rendered and 2.3 ms live. Onset detection takes 35–85 ms
per minute of audio. Ranges cover two runs on a MacBook Air shared with
other builds.

## Clip looping

`se_session_publish_v8` adds a loop window to any audio clip, warped or not
(`se_clip_loop_desc`: clip id, loop start and loop end in content frames).
Content frame c is what the clip plays at `timeline_start + c` without the
loop: `source_offset + c` for a 1:1 clip, the warp map at
`timeline_start + c` for a warped one. At clip-relative frame r a looped
clip plays content frame `r` while `r < loop_end`, then
`loop_start + (r - loop_end) mod (loop_end - loop_start)`, until
`timeline_start + length` (`clip_content` in render_graph.hpp).

- The process path splits a clip's run in a block at each wrap and reads each
  piece as before (`kernel_add_scaled` for 1:1 clips, `render_warped_clip`
  at the folded timeline frames for warped ones). It doesn't allocate or lock.
- A looped 1:1 clip's length is not clamped to the asset; its loop end is
  (and to the clip length). A loop the clamps leave empty is dropped.
- A warped clip's background render covers frames below the loop end only,
  so a long looped clip renders no more than its content. Live stretch voices
  restart at every wrap (the folded position jumps).
- Wraps are hard cuts with no crossfade, like the transport loop.

## Clip fades

`se_session_publish_v11` adds a fade-in and a fade-out to any audio clip
(`se_clip_fade_desc`: clip id, fade-in and fade-out in timeline frames). At
clip-relative frame r of a clip whose played range is `len` frames, the clip
is scaled by `min(1, r / fade_in)` and by `min(1, (len - r) / fade_out)`: a
linear ramp from silence (`clip_fade_gain` in render_graph.hpp). Fades follow
the timeline, so looped and warped clips fade where they play.

- Each fade is clamped to the clip's played range, and the fade-out to what
  the fade-in leaves.
- A block that touches a clip's fades renders the clip into a scratch pair
  (`RenderContext::fade_l/fade_r`) and adds it with the per-frame gain; other
  blocks, and clips without fades, take the usual path. Nothing allocates or
  locks.
- Overlapping clips on one track still sum, so a fade-out over a fade-in is a
  linear crossfade. `dstudio_clip_fade_bench` measures the cost.

## Tempo

`se_session_publish_v12` adds the song's tempo map (specs/015-tempo-changes):
a list of `se_tempo_segment {start_frame, start_quarter, start_bpm,
end_bpm}` and `click_quarters` (quarter notes per metronome beat). Clips,
notes, automation and the loop still arrive in frames, converted through the
map by the core; the engine uses the map only for what counts musical time on
the audio thread: the metronome ([Metronome](#metronome)), and the tempo
(`RtTransport::tempo_bpm`, at the first frame of each block) and song position
(`RtTransport::beats()`, at any position a device or the plugin host asks for)
that tempo-synced built-in devices and plugins see, live and in offline
renders. A session without segments behaves exactly as with v11
(`se_transport_set_tempo` and `frames_per_beat`).

The formula (`src/tempo_map.hpp`; the core's `crates/dstudio-core/src/tempo.rs`
evaluates the same in seconds). Segment `i` starts at frame `f0` and quarter
`q0` with tempo `a` BPM and runs to the next segment's `q1`, where a ramp
reaches `b` (a hold has `b = a`; the last segment holds). With `R` the sample
rate and `k = (b - a) / (q1 - q0)`, the ramp being linear in BPM over quarters:

| | quarter at frame `f` | frame of quarter `q` | tempo at `f` |
|---|---|---|---|
| hold | `q0 + (f - f0) · a / 60R` | `f0 + (q - q0) · 60R / a` | `a` |
| ramp | `q0 + (a / k) · (exp(k (f - f0) / 60R) - 1)` | `f0 + (60R / k) · ln(1 + k (q - q0) / a)` | `a · exp(k (f - f0) / 60R)` |

Before frame 0 the first segment's tempo holds (the count-in pre-roll).
Lookups are a binary search over the segments plus one `exp`/`log1p`; the
map is copied into the render graph when it is built, so the audio thread
only reads it. `test_tempo.cpp` checks the formula against numeric
integration, the clicks across a step and a ramp, the song position a synced
LFO sees per block, validation and that a session without a map renders
bit-identically; `rt_tempo_map_does_not_allocate` renders a map under the
allocation guard. `crates/dstudio-engine/tests/tempo.rs` checks the clicks
land on the frames core computes for each beat.

## Placeholders and known gaps

* **Windows / Linux audio** (`wasapi_device.cpp`, `pipewire_device.cpp`,
  `alsa_device.cpp`): WASAPI was checked by hand on one Windows sound card
  (inbox HD Audio driver); Linux has never run against real audio hardware
  (CI runners have no devices; ALSA runs there against its null PCM and
  pipewire-alsa); ASIO (`asio_device.cpp`) was checked with FlexASIO only, no
  hardware ASIO driver yet, and has no control panel button, clock source
  choice or restart after a driver reset; no WASAPI exclusive mode, no native
  JACK; device-change notifications are not handled on any platform.
* **Plugin editors:** Windows editors ignore per-monitor DPI. Linux editors
  are X11 only (Wayland desktops run them through XWayland); no HiDPI scale
  is passed to plugins, and plugin timers run at the idle rate (16 ms in the app).
* Core Audio: no handling of default-device changes, device hot-unplug or
  externally triggered rate changes while running (the stream just keeps
  whatever Core Audio does). **The duplex/aggregate input path has not been
  run against real hardware by the engine agent** (see "Input, monitoring and
  recording" > Verification).
* No janitor thread: retired graphs are reclaimed on the next publish /
  asset release / destroy. With the device running and no edits, the
  previously retired graph (and any released assets it holds) stays alive
  until the next such call.
* Plugins: no sidechains, multi-out instruments,
  host-side generic parameter UI, or MIDI CC/pitch bend to instrument
  plugins. Of VST3 `restartComponent` flags only `kLatencyChanged` is handled
  (and CLAP `request_restart`): re-activation for latency changes; layout /
  I/O changes are still ignored. Export of a session with plugins needs the device
  stopped. Windows/Linux host VST3 and CLAP only (no AU, no LV2). The scanner does not
  look inside `.component` bundles itself (AUs come from the system
  registry) and does not sandbox children beyond process isolation.
* Multithreaded track rendering, buses/sends, and
  sample-accurate transport commands while the device runs (they apply at
  block boundaries; the loop wrap itself is sample-accurate) are not
  implemented.
* Loop wraps are hard cuts (no crossfade). Within a tempo ramp, devices and
  plugins see the tempo of each block's first frame (the song position is
  exact at every frame they ask for).
* `se_meters_peek` track windows restart at 0 when a snapshot is published
  (same as `se_meters_read`).
* The resampler is scalar; large imports at a different rate take noticeably
  longer than same-rate imports.
* Output channels: only `output_channels == 2` is accepted.
* MIDI: no sustain pedal / pitch bend / CC / mod wheel, no velocity →
  filter routing, no per-note pan or stereo unison spread (the synth is
  mono). Removing a track or switching it from instrument to audio cuts its
  voices without a fade. A mid-note seek never retriggers the note (no
  "chase"). The sine wavefolder is not band-limited (aliases at high fold
  and pitch); the filter has no drive/saturation and does not
  self-oscillate. Cost is about 9 ns per voice-sample for the default patch
  and ~50 ns for a 4-voice-unison FM patch on Apple silicon (bench below);
  muted instrument tracks still cost CPU. Unison stacks are scalar (SIMD
  across the 4 stacks is the obvious next optimisation).

## Benchmark (Apple silicon, Release, 256-frame blocks @ 48 kHz, deadline 5333 µs)

| Case | 8 tracks | 32 tracks | 128 tracks |
|---|---|---|---|
| Audio, 4 clips/track (avg µs/block) | 1.0 | 3.2 | 12.3 |
| Audio, 4 clips/track, after the input/recording change (same machine, HEAD baseline in parentheses) | 1.06 (1.09) | 3.36 (3.37) | 12.6 (12.7) |
| Audio, 4 clips/track, after delay compensation (PDC on, L = 0; mean of 3 runs interleaved with the pre-PDC build in parentheses) | 1.03 (1.02) | 3.34 (3.33) | 12.53 (12.51) |
| MIDI, 8 held voices + 1 retriggered note/track, previous fixed synth | 98 | 396 | 1594 |
| MIDI, same notes, `se_synth_params` defaults | 140 | 573 | 2246 |
| MIDI, same notes, heavy patch (unison 4, cross FM, folded sine, triangle morph, sub, noise, resonant BP + filter env, LFO → pitch/cutoff/amp) | 870 | 3489 | 13782 |

Plugins (32 audio tracks × 4 clips, two test-gain inserts per track = 64
plugin calls per block; same machine and run):

| Inserts | avg µs/block | p99 |
|---|---|---|
| none (baseline) | 3.29 | 3.58 |
| DStudioTestGain CLAP + VST3 | 9.98 | 10.67 |
| 2 × DStudioTestGain CLAP | 10.03 | 10.79 |

≈ 105 ns per plugin call including our input/output copies and event/
transport setup (~0.2 % of the deadline for 64 inserts). The audio, MIDI and
input tables were unchanged by the plugin work within run-to-run noise
(e.g. audio 1.02 / 3.28 / 12.41 µs vs 1.03 / 3.34 / 12.59 µs before).

Delay compensation (32 audio tracks × 4 clips, one DStudioTestLatency insert
of N frames on track 1; same machine and run):

| N / PDC | avg µs/block | p99 |
|---|---|---|
| N = 0, off | 3.42 | 4.29 |
| N = 0, on (L = 0: no lines) | 3.43 | 4.25 |
| N = 4096, off | 3.78 | 4.58 |
| N = 4096, on (31 track lines + metronome line, 4096 frames) | 5.87 | 6.96 |

L = 4096 costs ≈ 2.1 µs/block for 31 delayed tracks (~68 ns per track per
256-frame block: one stereo memcpy in and out of the ring), 0.04 % of the
deadline. With L = 0 the plan is empty and the cost is a few branches.

Automation (32 audio tracks × 4 clips + CLAP gain + VST3 gain inserts, 4
lanes per track: volume, pan, and the gain parameter of each insert, a
curved point every 100 ms; same machine):

| Lanes | avg µs/block | p99 |
|---|---|---|
| none (same session) | 10.46 | 12.58 |
| 32 × 4 lanes | 41.91 | 50.04 |

≈ 0.24 µs per lane per 256-frame block (0.6 % of the deadline for 128
lanes): mostly the pan lane's two constant-power evaluations per ≤ 32-sample
run and the per-sample gain multiply. Without lanes nothing changed: two
interleaved runs against the pre-automation build (HEAD 4ab2922) gave audio
1.07 / 3.44 / 12.88 µs vs 1.07 / 3.40 / 12.87 µs, plugins (CLAP + VST3) 10.34
vs 10.46 µs, PDC N = 4096 off 3.89 vs 3.81 µs, MIDI default patch 8 / 32
tracks 138.7 / 562 µs vs 138.7 / 557 µs (all within run-to-run noise).

Tempo map (32 audio tracks × 4 clips + CLAP gain + VST3 gain inserts, the
metronome on; median of three runs, Apple M4): one tempo 12.70 µs/block, a
64-point map (`se_session_publish_v12`, alternating holds and ramps) 13.37
µs/block. ≈ 10 ns per song-position lookup (64 plugin calls and the click per
block), 0.01 % of the deadline. Sessions without a map take the same path as
before; interleaved runs against the build before tempo changes were within
run-to-run noise (the second run of each pair was the slower one, whichever
build it was).

Input (8 armed + monitored tracks, injected 8-channel input, no clips):
1.33 µs/block with recording off; 5.9 µs/block avg (p99 11 µs, max 32 µs)
with recording on, i.e. ~0.1 % of the deadline for 8 recorded tracks. The
recording case pauses (untimed) every 256 blocks so the writer keeps up,
because offline rendering runs ~1000x faster than real time and would
otherwise only exercise the overrun path. The extra cost is the interleaved
copy into the 8 rings while the writer thread reads them on another core.

(Default patch, 32 tracks: ≈ 1.45× the previous synth, ~11 % of the block
deadline. The heavy patch runs 8 oscillators per voice: 32 tracks × 8 voices
fit in ~65 % of the deadline; 128 such tracks do not run in real time.)
