// The block renderer shared by the device callback, se_render_offline and
// se_export_wav. Real-time safe: no allocation, no locks, no I/O.
#pragma once

#include "devices/builtin.hpp"
#include "midi_input.hpp"
#include "plugins/plugin_instance.hpp"
#include "render_graph.hpp"

#include <cstdint>
#include <memory>
#include <vector>

namespace dstudio {

// Scratch buffers, preallocated to max_block frames. One context per thread
// that renders (the live/RT context, or an export job's private context).
class RenderContext {
public:
    explicit RenderContext(uint32_t max_block);

    uint32_t max_block() const noexcept { return max_block_; }
    float* master_l() noexcept { return master_l_.data(); }
    float* master_r() noexcept { return master_r_.data(); }
    float* track_l() noexcept { return track_l_.data(); }
    float* track_r() noexcept { return track_r_.data(); }
    float* synth() noexcept { return synth_.data(); } // mono instrument scratch
    float* live_synth() noexcept { return live_.data(); } // mono live-input synth scratch
    // MIDI input events of one track (kMidiScratch).
    static constexpr uint32_t kMidiScratch = MidiInputHub::kMaxBlockEvents + 128;
    MidiEvent* midi_scratch() noexcept { return midi_.data(); }
    float* plugin_l() noexcept { return plugin_l_.data(); } // instrument plugin output
    float* plugin_r() noexcept { return plugin_r_.data(); }
    const float* zeros() const noexcept { return zeros_.data(); } // max_block zeros (missing input channels)
    // Automation scratch: per-sample gains and lane values, plugin
    // parameter events, the automated synth parameters.
    float* gain_l() noexcept { return gain_l_.data(); }
    float* gain_r() noexcept { return gain_r_.data(); }
    float* auto_a() noexcept { return auto_a_.data(); }
    float* auto_b() noexcept { return auto_b_.data(); }
    ParamEvent* param_events() noexcept { return param_events_.data(); }
    se_synth_params& synth_params() noexcept { return synth_params_; }
    // Modulation scratch: per modulated parameter of one device, an offset
    // and an absolute value per grid point; and the built-in bindings.
    static constexpr uint32_t kMaxModTargets = 128;
    uint32_t mod_points() const noexcept { return mod_points_; }
    float* mod_off(uint32_t t) noexcept { return mod_.data() + (2 * std::size_t{t}) * mod_points_; }
    float* mod_abs(uint32_t t) noexcept { return mod_.data() + (2 * std::size_t{t} + 1) * mod_points_; }
    BuiltinMod* mod_binds() noexcept { return mod_binds_.data(); }
    // Sidechain taps (SE_BUILTIN_MAX_SIDECHAINS stereo buffers).
    float* sc_l(uint32_t tap) noexcept { return sc_.data() + (2 * std::size_t{tap}) * max_block_; }
    float* sc_r(uint32_t tap) noexcept { return sc_.data() + (2 * std::size_t{tap} + 1) * max_block_; }
    // Return track buses (SE_MAX_RETURNS stereo buffers) and whether each
    // received anything in the current block; a stereo scratch for sends.
    // Left uninitialised (a bus is cleared by its first writer in a block,
    // the send scratch is written before it is read), so an engine without
    // returns never touches them.
    float* bus_l(uint32_t bus) noexcept { return bus_.get() + (2 * std::size_t{bus}) * max_block_; }
    float* bus_r(uint32_t bus) noexcept { return bus_.get() + (2 * std::size_t{bus} + 1) * max_block_; }
    bool& bus_used(uint32_t bus) noexcept { return bus_used_[bus]; }
    float* send_l() noexcept { return send_.get(); }
    float* send_r() noexcept { return send_.get() + max_block_; }
    // A fading clip's own output (se_session_publish_v11), before its fade gain.
    float* fade_l() noexcept { return fade_l_.data(); }
    float* fade_r() noexcept { return fade_r_.data(); }

private:
    uint32_t max_block_;
    std::vector<float> track_l_, track_r_, master_l_, master_r_, synth_, zeros_, plugin_l_, plugin_r_;
    std::vector<float> gain_l_, gain_r_, auto_a_, auto_b_;
    std::vector<ParamEvent> param_events_;
    std::vector<float> sc_;
    std::unique_ptr<float[]> bus_;
    bool bus_used_[SE_MAX_RETURNS] = {};
    std::unique_ptr<float[]> send_;
    uint32_t mod_points_;
    std::vector<float> mod_;
    std::vector<BuiltinMod> mod_binds_;
    std::vector<float> live_;
    std::vector<float> fade_l_, fade_r_;
    std::vector<MidiEvent> midi_;
    se_synth_params synth_params_{};
};

// Instrument inputs of render_block(). `synths` is parallel to
// graph->tracks (null entries, or a null array, render no notes). The live
// path passes graph->synths (state shared across republishes); export passes
// its own private Synth objects. `transport_gen` changes whenever playback
// jumps (seek / stop / pause / loop wrap) so the synths fade their voices.
// `transport` non-null enables plugin processing (insert chains and
// instrument plugins) for this call; its position must equal `position`.
// Only the RT owner may pass it (plugin instances are shared live state).
// `lines` (non-null only when graph->pdc.need is non-empty): delay lines in
// the PdcPlan layout (graph->lines live, private copies for export);
// `pdc_gen` changes whenever they must be flushed (seek / stop).
// `history` (live path): where recently rendered samples were read on the
// timeline, so delay-compensated stages evaluate automation at the frame
// their audio came from across loop wraps; null = linear playback (export).
struct InstrumentInputs {
    Synth* const* synths = nullptr;
    uint64_t transport_gen = 0;
    const RtTransport* transport = nullptr;
    DelayLine* const* lines = nullptr;
    uint64_t pdc_gen = 0;
    const TimeHistory* history = nullptr;
    // Stretch voices of warped clips, parallel to graph->warp_voice_keys
    // (graph->warp_voices live, private ones for export); null = warped clips
    // in the stretch modes fall back to varispeed.
    WarpVoice* const* warp_voices = nullptr;
    // MIDI input (live path only; see MidiBlock and graph->midi_tracks).
    const MidiBlock* midi = nullptr;
};

// Device input for one render_block() call: `count` channel pointers, each
// valid for the block's frames (count 0 = no input). Channels a track asks
// for beyond `count` read as silence.
struct InputView {
    const float* const* channels = nullptr;
    uint32_t count = 0;
};

struct BlockPeaks {
    float l = 0.0f;
    float r = 0.0f;
};

// Renders `frames` (<= ctx.max_block()) frames starting at timeline frame
// `position` into ctx.master_l()/master_r() (overwritten). `graph` may be
// null (silence). When `playing` is false the output is silence.
// If `update_meters` is set, per-track meters in the graph are raised, and
// so is bucket `peek_slot` (< kPeekBuckets) of each track's peek window.
// Returns the master output peaks of the block (zero unless update_meters).
// Instrument tracks: notes are rendered through `inst.synths` and summed with
// the track's clips before gain/pan. Synths run even for muted tracks (so an
// unmuted note continues where it would be) but are only mixed if audible.
// While not playing, only voice tails (fade-outs after a pause/stop) render.
// Input (`input`, used only if graph->uses_input): a track whose input is
// set hears it when `monitor` is on (summed with clips/notes, then the
// track's gain/pan/mute/solo; also while not playing). Mono input feeds both
// sides. An armed track that is not monitoring raises its meters to the raw
// input peak instead (pre-fader; it is not heard).
// Plugins (inst.transport set): an instrument plugin replaces the track's
// synth (fed with the track's notes as MIDI); then the track's insert chain
// (enabled slots with a loaded instance, in order) processes the summed
// source in place before gain/pan/meters. Tracks with plugins are processed
// every call, also when stopped or muted (tails, consistent state), but are
// only mixed when audible.
// Delay compensation (inst.lines set, docs/adr/0004): the clips of a track
// with an instrument plugin are delayed by its latency (pdc_src_delay); the
// track is delayed by pdc_delay after its chain, or, when it monitors input
// (pdc_pre), only its playback source before the input is added; bypassed
// slots delay by their plugin's latency. Lines with pending output keep the
// track rendering while stopped (tails); muted tracks drop their lines.
// Automation (graph->lanes, docs/adr/0005): volume/pan lanes replace the
// static fader values with per-sample gains (ramps of <= kAutoRampFrames,
// exact at points); synth lanes replace synth parameters every 16 timeline
// frames; plugin lanes become sample-offset parameter events; the master
// volume lane replaces master_gain. Each stage evaluates its lanes at the
// timeline frame its audio was read at (delay compensation, inst.history).
BlockPeaks render_block(const RenderGraph* graph, int64_t position, bool playing, uint32_t frames,
                        RenderContext& ctx, bool update_meters, uint32_t peek_slot = 0,
                        const InstrumentInputs& inst = {}, const InputView& input = {}) noexcept;

// ---- kernels (exposed for tests/bench) -------------------------------------
// Plain loops over restrict pointers so clang/gcc auto-vectorise them
// (NEON on arm64, SSE/AVX on x86-64).
void kernel_add_scaled(float* __restrict dst, const float* __restrict src, float gain, uint32_t n) noexcept;
void kernel_add_scaled2(float* __restrict dst_l, float* __restrict dst_r, const float* __restrict src_l,
                        const float* __restrict src_r, float gain_l, float gain_r, uint32_t n) noexcept;
void kernel_scale(float* __restrict buf, float gain, uint32_t n) noexcept;
// In place: l' = a l + b r, r' = b l + a r (the channel-strip matrix).
void kernel_strip(float* __restrict l, float* __restrict r, float a, float b, uint32_t n) noexcept;
// kernel_strip with a per-sample linear trim and width (0..2; 0 = mono).
void kernel_strip_varying(float* __restrict l, float* __restrict r, const float* __restrict trim,
                          const float* __restrict width, uint32_t n) noexcept;
// dst_l += src_l * gain_l[i], dst_r += src_r * gain_r[i]
void kernel_add_mul2(float* __restrict dst_l, float* __restrict dst_r, const float* __restrict src_l,
                     const float* __restrict src_r, const float* __restrict gain_l, const float* __restrict gain_r,
                     uint32_t n) noexcept;
void kernel_mul(float* __restrict buf, const float* __restrict gain, uint32_t n) noexcept;
float kernel_peak(const float* __restrict buf, uint32_t n) noexcept;
void meter_output(PluginInstance& p, const float* l, const float* r, uint32_t frames) noexcept;

} // namespace dstudio
