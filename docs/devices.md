# Built-in devices

DStudio's own effects (Compressor; the dynamics family: Limiter, Gate, Glue
Compressor, Transient Shaper, Mastering Limiter; the modulation family:
Tremolo, Auto-Panner, Chorus, Flanger, Phaser, Ring Mod, Character Mod; more
follow) are
**declarative**: one definition in Rust describes a device, and the model,
undo, persistence, automation, engine and panel all derive from it. A device
is made of up to three parts:

1. a **definition** (`dstudio_core::builtin::DeviceDef`): parameters, panel layout, factory presets;
2. a **DSP kernel** (`dstudio::BuiltinProcessor` in `engine/src/devices/`);
3. optionally a **custom view** in the panel code, when the standard card body is not enough.

~~~mermaid
flowchart LR
    Def["DeviceDef<br/>crates/dstudio-core/src/builtin/&lt;id&gt;.rs"] --> Model["Device in the track chain<br/>params by id, undo, save"]
    Def --> Panel["Generic card<br/>src/device_panel.rs + ui/device_panel.slint"]
    Model --> Snap["Snapshot: values in definition order<br/>SE_PLUGIN_BUILTIN + se_builtin_state"]
    Snap --> Engine["BuiltinInstance → BuiltinProcessor<br/>engine/src/devices/&lt;id&gt;.cpp"]
    Engine -- readouts --> Panel
    Panel -- live drag values --> Engine
~~~

## How a built-in device lives in the app

- **Model.** A built-in device is an ordinary `Device` in a track's chain with
  `PluginFormat::Builtin` and `plugin.id` = the definition id. Its plain
  parameter values live in `Device::params` by parameter id; missing ids read
  as the default, unknown ids are dropped on `AddDevice`. Add / remove /
  reorder / bypass are the existing chain commands.
- **Edits.** `Command::SetDeviceParam` (validated and snapped by the
  definition; drags coalesce per parameter), `Command::SetDeviceParams`
  (preset loads, "Load Preset <name>") and `Command::SetDeviceSidechain`.
- **Automation.** `AutomationTarget::Builtin { device, device_type, param }`:
  lanes hold plain values in the parameter's unit; log parameters interpolate
  geometrically. `builtin::mod_targets` lists LFO / macro targets
  (`not_automatable` parameters are left out).
- **Engine.** The snapshot sends each built-in device with
  `format = SE_PLUGIN_BUILTIN`, `plugin_id` = the id, and an
  `se_builtin_state` blob (header + float32 values in definition order + the
  sidechain source track). Unlike plugin state it is applied on every publish.
  Lanes use `SE_AUTO_BUILTIN` with the parameter index. While a control is
  dragged the panel calls `se_builtin_set_param` (lock-free, applied at the
  next block) and commits one command on release. See the ABI block at the end
  of `engine/include/dstudio_engine.h`.
- **Panel.** `device_panel.rs` turns the definition's `Layout` into a
  `DeviceCard` (picker row, view row with slider column, dial row of knob
  clusters) and `ui/device_panel.slint` draws it. Lengths are
  100%-zoom card measurements times `device_panel::Z` (0.85, the zoom the rest
  of the UI is built at). Card width follows one rule:
  knobs × 62 + inner gaps × 10 + cluster gaps × 21 + 16, taken from the
  widest `shown_when` state so the card never resizes when a mode changes.
  The knob cells are then spread evenly across the dial row (the clusters
  only size the card); field cards centre their dial row instead.
- **Analysis frames.** Readouts are a few floats per block. A device that
  shows more (a spectrum, a pitch trace) returns `analysis_capacity()` > 0 and
  publishes whole frames through `BuiltinContext::analysis`, an
  `AnalysisChannel` (lock-free triple buffer: the audio thread writes, the
  main thread reads the newest frame). The app reads it with
  `se_builtin_analysis` / `Engine::builtin_analysis(id, &mut Vec<f32>)`, which
  returns the frame's sequence number so a poll can skip unchanged frames.
- **Expanded windows.** The window button on a card opens the device's
  expanded window (`device_window.rs`, `ui/device_window.slint`) when the
  device has one (`device_window::has_window`; Auto Filter, Channel EQ,
  Character EQ and the plotted-field family: Tremolo, Auto-Panner, Chorus,
  Flanger, Phaser, Ring Mod and Character Mod; and the dynamics family:
  Compressor, Limiter, Gate and Glue Compressor), otherwise the card itself in a window. A
  window is a face 960 pt wide (Ring Mod 860): a top bar (power, presets, the device's header pills), a
  large display (the card's canvas drawn at window size with its axis and
  legend), In / Out meters, and rows of controls laid out with the same
  gutter rule (fixed clusters, even gaps, a rule in each inner gap). A field
  window's display (`device_window_fields.rs`) asks the card's curve model in
  `device_fields.rs` for its window form (`Input::win`): a taller ladder, a
  time or frequency axis with units under the plot, and a legend naming each
  series; dragging the plot moves the card field's two parameters. Character
  Mod's picture reads in its circuit's own unit (ms delay, Hz sweep, dB gain or
  pan) with the band its series travel, and its rack card drops the artifacts
  a circuit lacks; its Seed pill plays the next take (`ACT_SEED`). A dynamics
  window's display (`device_window_dynamics.rs`) is a level history: each
  meter poll appends the device's input, output and gain reduction
  (`builtin_readouts`) to a ring of the last 10.5 s, and the display draws the
  span the header pills pick (1, 4 or 10 s, `ACT_SPAN`) as a gain-reduction lane
  over a dBFS level plot, one pixel per dB in both, with the threshold (or
  ceiling) as a dashed line you drag. Hold view freezes the history. Each
  device's first card carries a transfer square: the static curve of the
  card's law over −48..0 dB with the knee and the live operating point. The
  meters read `se_builtin_set_metering` / `se_builtin_levels`
  (`Engine::builtin_levels(id, &mut [f32; 4])`): while a window is open the
  audio thread keeps the input and output peak of each block, and the app
  takes them every frame (5 ms attack, 120 ms release, 1.5 s peak hold).
  The display's analyser reads `se_builtin_set_spectrum` / `se_builtin_spectrum`
  (`window_spectrum.hpp`): while a window is open and its view is Pre or Post,
  the audio thread measures a 128-bin log-spaced level spectrum of the
  device's input or output (4096-point FFT, a frame every ~21 ms). The window
  follows it with a 45 ms attack and 190 ms release, lets it drain at 42 dB/s
  once frames stop, and freezes it while Hold view is latched; its ruler runs
  0 to -72 dBFS over the plot. The view (Pre, Post, Off) and Hold view are
  window state, not parameters. `--state review-window-<id>` runs a test signal
  through the device for screenshots.
- **Project key.** `Transport::key` (`ProjectKey { root, minor }`, set with
  `Command::SetProjectKey`, "Key Change"). A device with `key_follow`
  (Manual / Project), `key` and `scale` parameters runs with the project key
  while it follows (`builtin::followed_key`; the snapshot sends
  `builtin::engine_device`). Its stored Key and Scale are kept, and picking
  either on the card while following writes Follow = Manual plus both values
  as one undo step.
- **Latency.** `BuiltinProcessor::latency()` is reported through
  `se_plugin_latency` and compensated like plugin latency (Pitch Corrector:
  1024 frames at 48 kHz).

## Modulation and chains

Every device gets the same modulation and container features; nothing here is
specific to one effect.

- **Sources.** An LFO (`lfo`) and a chain head (`device-rack`) are built-in
  devices with modulation outputs (`dstudio_core::modulation::outputs`: 1 and 8).
  A source owns its links in `Device::links` (`ModLink { slot, target, param,
  min, max }`); `param` is a built-in parameter id or a plugin parameter id
  (`ModParam`). A parameter takes at most three sources
  (`modulation::link_refusal` lists every rule). Edits are
  `AddModLink` / `RemoveModLink` / `SetModLink` (amount drags coalesce).
- **Maths** (in the target parameter's normalized 0..1 domain). LFO:
  `n = base + depth * (min + (max - min) * wave)`, wave 0..1, several LFOs add.
  Macro: `n = min + (max - min) * macro` (sets the value; min above max inverts).
  The result is clamped to 0..1.
- **Chains.** Members follow their head with `Device::chain = head`. The head's
  power switch bypasses every member (`modulation::effectively_enabled`, used by
  the snapshot); macros only reach members. Removing a head removes its members;
  moving it moves them. `collapsed` hides the members, `folded` folds any card.
- **Engine.** A source's state is an `se_builtin_state_v2` blob (the same
  header the samplers use): values, then `se_mod_route` records built by
  `modulation::routes` after the (empty) sample and slice tables. Every 32 frames
  (`SE_MOD_GRID_FRAMES`) the engine runs each enabled source, applies its routes
  to built-in parameters (as parameter events) or plugin parameters (as plugin
  events, the base restored when the last route goes), with no allocation on the
  audio thread. Plugin card knobs set values with `se_plugin_set_param`; a
  modulated parameter keeps that as its base (`se_mod_base`).
- **UI** (`src/device_mod.rs`). Right-click any control (built-in or plugin card)
  for Automate, Modulators (New LFO, the track's LFOs, Remove) and Macros inside a
  chain. Modulated knobs draw the reach of their sources and a dot at the live
  value. The LFO and chain head have custom bodies (`View::Custom("lfo" |
  "chain")`); their Map pill arms the source so the next control clicked becomes a
  target. Card headers: name = effect picker (swap in place), fold, device window
  (plugins), remove.
- **Plugin card.** A third-party plugin shows the parameters added with "+ Add"
  (`Device::card_params`, `SetCardParams`) plus any a source or automation moves,
  nine per page.

## Instruments and sample slots

The Sampler, Drum Rack and Pad Sampler added two general features to the
system. Any built-in device can use them.

- **Built-in instruments.** A definition with `kind: PluginKind::Instrument`
  goes in the track's instrument slot (`Command::SetInstrumentPlugin`, on
  instrument tracks only), not in the insert chain. The engine kernel sets
  `BuiltinDescriptor::instrument`. It then gets the track's notes as
  `BuiltinContext::events` and renders into silence. `se_builtin_note`
  plays a note on a live instrument right away, for pad and preview clicks.
  Velocity 0 releases the note.
- **Sample slots.** `DeviceDef::samples` is the number of audio files the
  device can hold. Each file is a `DeviceSample` in `Device::samples`: a
  slot, a project asset and optional slices (start, gain, pan, pitch).
  `Command::SetDeviceSample` loads, replaces or clears one slot, and slice
  edits use it too. Each of these is one undo step. `RemoveAsset` is refused
  while a device still plays the asset. The snapshot maps the sample assets to
  engine assets (`SessionSnapshot::device_assets`). The engine then gets the
  v2 state (`se_builtin_state_v2`): the v1 header, the values, then one
  `se_builtin_sample` per slot, followed by the slices. The kernel reads the
  samples as `BuiltinContext::samples`, which are bound on the control thread
  and never copied on the audio thread.
- **Shared sampler engine.** `engine/src/devices/sampler_engine.{hpp,cpp}`
  has the voice and voice pool, the sinc interpolator (band-limited when
  pitched up), envelopes, the filter, the LFO and the granular warp. All
  three samplers use it.
- **Panel.** `View::Custom("sampler" | "pad-sampler" | "drum-rack")` uses
  `src/sampler_panel.rs`. It has the waveform well with its flags, slices and
  playheads, the tabs with a picture canvas, and the pad grid. Files are loaded
  from the file picker or by dropping them on the window
  (`ImportTarget::DeviceSample`).

## Adding a device

1. **Definition.** Add `crates/dstudio-core/src/builtin/<id>.rs` with a
   `pub static <ID>: DeviceDef` and list it in `builtin::all()`.
   - Parameter order is the engine order. Never reorder or remove parameters
     of a shipped device; append new ones (old projects keep loading because
     values are stored by id).
   - Ids, ranges, defaults, units, tapers and the panel arrangement are
     defined here. Write descriptions in your own words. Never copy code,
     CSS, images, audio or preset files from any other product.
   - `Layout`: `picker` (segmented `Menu`s, `Segments` with their own
     labels, `Dropdown` pills for long lists, `ModeDropdown` (a type menu
     whose last entry switches a mode parameter, e.g. Auto Filter's DJ),
     toggles, `Latch` on/off pills, `Sidechain`, pills;
     `Gap` or the sidechain picker pushes what follows to the right),
     `view` (`DynamicsStrip`, `Field`, `Eq` or `Custom`), `sliders`,
     `clusters` of knobs (`Swap` holds alternatives in one cell, such as
     Rate / Rate Sync), `faces` (drawings inside a knob), `shown_when`
     (mode-dependent controls: hidden parameters get no control and no
     automation entry), `fixed_width` (card width in pt; 0 derives it from
     the dial row and view).
   - A card narrower than 240 pt hides the preset label and arrows and opens
     the preset menu from a chevron after the name. `Control::Toggle` on an
     Off/On choice draws a latch capsule in the dial row.
   - `View::Field` is a plotted well (motion / response / sweep
     fields): name a curve model in `crates/dstudio-app/src/device_fields.rs`
     (traces, fill or band, ticks, caption value, live overlay), the caption,
     the parameters dragged on x and y, and the field's width; the card is
     at least field + sliders + padding wide. Field cards centre their dial
     row and space its knobs evenly. The live overlay follows the kernel's
     first readout as an LFO phase (0..1) while the transport plays.
   - Parameter extras: `preset_locked()` (presets never load or save it,
     e.g. a random seed), `reset_on_load()`, `Format::Freq` (generic Hz
     text), `Format::SignedPercent`, `Format::DbCut` (a positive stored cut
     shown as −x.x dB), `Format::MsAuto`, `Format::Rate` (LFO rates,
     `1.00 Hz`), `Format::SignedBareZero(n)` (explicit sign, bare zero) and
     `Format::Degrees`. Tempo-synced LFOs use the `DIVISIONS` /
     `MOD_NOTES` tables and `synced_hz`.
   - Linked writes: when one edit must move other parameters in the same
     undo step (Character Mod's circuit switch resets untouched artifacts),
     add an arm to `builtin::linked_writes`; the panel then commits one
     labelled `Command::Batch`.
   - Dynamics additions: `Toggle` latches (a latch in a knob cell in the dial
     row; a right-aligned "<label> <choice>" pill in the picker row), detent
     ticks on discrete knobs with more than two choices (drawn like the
     Behavior detents), transfer faces
     (`SoftKneeCurve`, `BrickWall`, `Expander`, `BusVca`, `PushWall`), and an
     empty picker moves the view row up.
   - Views: `View::Strip(Strip::new("<line param>").range(top_db, floor_db).second("<param>"))`
     is the level / gain-reduction strip with a draggable dashed line, an
     optional solid second line (Alt+drag moves both by the same dB, one undo
     step) and its own plot range; `DynamicsStrip { threshold }` is shorthand
     for the 0..-36 dB strip. `View::Custom("gain-scope")` is the transient
     shaper's scrolling waveform + applied-gain scope.
   - Choice overlays: `ParamDef::overlays` lists, per choice, other
     parameters that choice writes (the mastering limiter's styles); they go
     through `linked_writes` like any other linked write. Also `max_text` (the
     top of the range reads e.g. "Full"), `Format::Ms(decimals)`, and
     `DeviceDef::sidechain_enable` (the parameter that follows the sidechain
     picker).
   - `View::Strip(Strip::new("threshold").range(top, floor))` is the
     dynamics strip with its own dB range. Value arcs and slider fills start
     at 0 when the range spans it (−12..24 dB starts at 0 dB). Cards without
     a field spread their knob cells evenly across the card.
   - `Format::Custom(fn)` gives device-specific text; parsing still reads
     the unit ("1.2 s" → 1200 ms, "2k" → 2000 Hz).
   - **Behaviour** (optional, `builtin::behavior`): a `Behavior` with
     state-dependent detents (drags snap within 0.025 of the range, arrow
     keys step between them, ticks drawn on the ring), value text, latches
     (a knob that becomes a two-state pill in some modes) and the curve of a
     `Face::Transfer` knob. Start from `Behavior::NONE`.
2. **DSP kernel.** Add `engine/src/devices/<id>.cpp`: a `BuiltinProcessor`
   (`prepare` may allocate; `reset` / `process` are real-time: no
   allocation, locks, I/O or exceptions) and a `BuiltinDescriptor` whose
   parameter ids are the definition's ids **in the same order**. Register it
   in `kBuiltinRegistry` (`engine/src/devices/builtin.cpp`), give the
   descriptor its `defaults` (checked against the definition by the Rust
   registry test through `se_builtin_desc::param_defaults`) and add the
   file to `engine/CMakeLists.txt`. `params` are plain values, constant
   within one `process` call (the instance splits blocks at automation
   events); smoothing audible steps is the kernel's job (`dsp::Smoother`).
   `engine/src/devices/dsp.hpp` also has `LfoClock` (free or locked to
   the song position through `BuiltinContext::offset`), `DelayLine`
   (Hermite) and `Oversampler2x` (report its latency from `latency()`).
   Readouts (meters, gain reduction) are written per block; give
   `idle_readouts` for the values before the first block. When automation
   splits a block, the engine keeps each readout's segment value farthest
   from its idle value, so readouts must move away from idle as they grow
   (a peak meter rises from -120 dB, gain reduction falls from 0 dB).
   A device has at most `kMaxReadouts` (64) readouts. Spectral helpers are in
   `engine/src/devices/spectral.hpp` (`RealFft`, and `SpectrumTap`: 48 log
   bands, 20 Hz–20 kHz, a full-scale sine reads 0 dB); `dsp.hpp` has the
   state-variable filter (`SvfCoeffs`, including `bandpass`). Report latency for
   PDC: `latency_for(params)` (the default returns the fixed `latency`), so a
   kernel whose delay depends on a setting (the mastering limiter's
   oversampling) reports the delay of the values it will run with. Shared
   dynamics building blocks (biquads, delay lines, sliding minimum, look-ahead
   gain, polyphase oversampler, 4x true-peak detector) are in
   `dsp_dynamics.hpp`.
3. **EQ view.** `View::Eq(&EqView)` (`builtin/eq.rs`) describes an EQ card:
   its bands (parameter ids per band), the shapes and their picker order,
   pass slopes, Output, Bank and Auto Gain parameters, the dB window and
   the node behaviour. `device_canvas.rs` draws it (band tabs, On latch,
   Bank menu, response field with nodes and Q wings, type glyphs, value
   cells, Output slider) and runs its gestures. The panel curve and the
   engine use the same design (`eq::sections`, `engine/src/devices/eq_sections.hpp`);
   `engine_contract_points` (Rust) and `eq_design_matches_the_panel_model`
   (C++) pin the same points, so change both together.
4. **Custom view (optional).** For bodies the template cannot express (EQ
   curve, shaper curve, trance-gate grid), use `View::Custom("<name>")`, add
   an arm in `Controller::builtin_card` that fills the card's view data, and a
   Slint component in `ui/device_panel.slint` chosen by that data. Reuse
   `ParamDrag` / `ParamDragX`, `KnobCell` and `SliderCol` for the gestures. Custom bodies
   return a `PanelCanvas` from `Controller::custom_body` (`device_views.rs`):
   paths, texts, grip pills and linear drag zones. When handles need
   non-linear mapping or hit-testing (several handles on one well, a live
   spectrum), set `pointer: true` and handle `Studio.panel-graph` in Rust,
   as the restoration devices do (`crates/dstudio-app/src/panel_graph.rs`: hit
   zones, spectrum ballistics from the readouts, one undo step per gesture).
   The analyzers (`src/analyzer_views.rs`) are `custom_body` arms too: they
   poll analysis frames (`Controller::panel_poll_views`), draw gradient
   fills with `PanelPath.fill2` (applied only when it differs from `fill`),
   set a knob cell width (`CustomBody.cell`) and take their card width from
   `Layout::fixed_width`.
   View-local state (a selected band, meter ballistics) goes through
   `Studio.panel-view(device, action, value)`; `src/panel_multiband.rs` is
   an example with its own Slint component (band selector, threshold / floor
   and crossover handles, dial row following the selected band).
   The EQ and Auto Filter canvases (`device_canvas.rs`) use the same flag and
   add dots, anchored grips, double-click (phase 6), wheel and keys
   (`panel-graph-wheel`, `panel-graph-key`).
   Full editors with their own gestures and menus (step grids, curve
   editors) add an arm to `custom_card` in
   `crates/dstudio-app/src/device_editors.rs` and set `interactive: true`:
   `DeviceCanvas` then sends every press, drag, release, hover,
   double-click, right-click and wheel over the canvas to `panel_editor`,
   with window coordinates for menus. A gesture sends live values to the
   engine and commits one undo step on release; Escape restores the values
   from before it. Set `Layout::fixed_width` for a card that the dial row
   does not size. The Trance Gate and the Shaper are the editor examples.
   - A parameter that only the stored value should drive (the Shaper's
     Pitch switch, which sets its latency) returns false from
     `BuiltinProcessor::automatable`; the engine then leaves automation
     lanes on it inactive.
5. **Tests.**
   - Engine: `engine/tests/devices/device_harness.hpp` gives `DeviceRig`
     (one instance, `set(id, value)`, `run` / `run_mono`, `readouts`),
     `DeviceSession` (a published engine session with tracks, devices and
     lanes) and test signals (`impulse`, `tone`, `noise`). Write
     `engine/tests/devices/test_<id>.cpp` for the device's behaviour.
     `every_builtin_is_rt_safe_and_robust` runs every registered device
     under the allocation guard with hostile input and extreme values
     automatically.
   - Rust: `engine_registry_matches_core_definitions`
     (`crates/dstudio-engine/tests/builtin.rs`) fails when the definition and
     the kernel disagree on parameter order or defaults. Add core tests for formatting and
     parsing of any new display format.
6. **Benchmark.** Add a row for the device to `engine/bench/bench_devices.cpp`
   (`dstudio_device_bench`, Release build) and put the numbers in the PR.
7. **Render.** `--state reference-<id>` shows any built-in device at its
   defaults. Render it with `--offscreen --screenshot <png>` (a hidden
   window, so it does not take over the screen) and check the
   result. Add the device to the app self-test (the dynamics,
   modulation and restoration sections in `src/selftest.rs` add each device
   through Add device › Built-in).
   `--state review-analyzers` feeds a sung-like line through Pitch
   Corrector, Tuner and Spectrum so their views show live data.

## Commands

~~~sh
cargo test --workspace                                   # core, engine wrapper, app unit tests
cd engine && cmake --build build && ./build/dstudio_engine_tests
./build/dstudio_device_bench                               # per-block cost, param updates, 32-track session
cargo run -p dstudio-app -- --state reference-limiter --offscreen --screenshot /tmp/limiter.png
cargo run -p dstudio-app -- --state reference-compressor --offscreen --screenshot /tmp/compressor.png
cargo run -p dstudio-app -- --state reference-lfo --offscreen --screenshot /tmp/lfo.png      # also reference-chain
cargo run -p dstudio-app -- --self-test                    # includes the built-in device section
~~~
