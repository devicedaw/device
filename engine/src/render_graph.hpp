// Immutable render graph built from a session snapshot on a non-RT thread.
//
// Everything the audio thread needs is resolved at build time: mute/solo is
// collapsed into `audible`, pan law and track gain are folded into two
// per-channel gains, clips are clamped to their asset and sorted by start,
// and raw sample pointers are cached. The graph also owns AssetRefs so the
// sample memory outlives every block that can still read it.
//
// Concurrency model: a graph is never mutated after publication except for
// its meter atomics. Retired graphs are destroyed on non-RT threads only.
//
// Future work (multithreaded processing): TrackNodes are independent of each
// other until the master sum, so a worker pool can render disjoint track
// ranges into per-worker scratch buffers and the RT thread then sums them.
// The graph is already a flat, read-only array, which is what a lock-free
// work-stealing scheduler wants. Return tracks and sends (se_session_publish_v9)
// make it a DAG: `order` is a topological order (sources before the returns
// and sidechain readers that depend on them).
#pragma once

#include "asset.hpp"
#include "automation.hpp"
#include "dstudio_engine.h"
#include "synth.hpp"
#include "tempo_map.hpp"
#include "warp.hpp"

#include <atomic>
#include <cstdint>
#include <functional>
#include <memory>
#include <string>
#include <vector>

namespace dstudio {

struct TrackMidi; // midi_input.hpp

class PluginInstance;
class DelayLine;

// One insert of a track's device chain. `inst` is null while unresolved
// (between build_render_graph and Engine::publish) or when the plugin failed
// to load (pass-through). Instances are owned by the PluginHost.
struct FxSlot {
    PluginInstance* inst = nullptr;
    uint64_t device_id = 0;
    bool enabled = true;
    // SE_AUTO_DEVICE_ON lane of this device (RenderGraph::lanes), -1 = none:
    // replaces `enabled`, evaluated per block.
    int32_t on_lane = -1;
    // Delay compensation: while bypassed, the slot delays by the plugin's
    // latency instead (0 = nothing to do). See RenderGraph::pdc.
    uint32_t pdc_bypass_delay = 0;
    // Automation lanes of this device (RenderGraph::lanes), and how late
    // the audio entering the slot is relative to the timeline (delay
    // compensation; lanes are evaluated that much earlier).
    uint32_t lane_begin = 0;
    uint32_t lane_count = 0;
    uint32_t auto_delay = 0;
    // Built-in devices with a sidechain: the source track id (0 = none) and
    // its tap (RenderContext::sc_l/sc_r, -1 = silence). sc_wanted: a source
    // was named, so a rejected route reads silence instead of no sidechain.
    uint64_t sc_source = 0;
    int32_t sc_tap = -1;
    bool sc_wanted = false;
    // Modulation (RenderGraph::mod_targets): the modulated parameters of
    // this device; mod_source: the device is a source (LFO, chain head).
    uint32_t mod_begin = 0;
    uint32_t mod_count = 0;
    bool mod_source = false;
};

// One modulation route into a ModTarget (se_mod_route): the source slot
// (index into RenderGraph::fx), its output and the link range.
struct ModRoute {
    uint32_t src = 0;
    uint8_t slot = 0;
    bool absolute = false;
    float min = 0.0f;
    float max = 0.0f;
};

// A modulated parameter of one device with its routes (contiguous in
// RenderGraph::mod_routes). Plugin targets are resolved on the main thread
// (Engine::publish): native range, automation lane and the engine-owned base.
struct ModTarget {
    uint32_t param_id = 0;
    bool plugin = false;
    bool log = false;
    float p_min = 0.0f, p_max = 1.0f, p_skew = 1.0f, p_step = 0.0f;
    uint32_t route_begin = 0;
    uint32_t route_count = 0;
    bool active = true;
    double native_min = 0.0;
    double native_max = 1.0;
    int32_t lane = -1;                     // plugin targets: the parameter's automation lane
    std::atomic<float>* base = nullptr;    // plugin targets: normalized base (Engine::mod_bases_)
};

// One automation lane (se_auto_lane_desc) with its points in
// RenderGraph::auto_points. Plugin lanes are resolved against the instance
// on the main thread (Engine::publish): native range, or inactive.
struct AutoLane {
    uint32_t point_begin = 0;
    uint32_t point_count = 0; // > 0
    uint8_t target = 0;       // SE_AUTO_*
    bool log = false;
    uint16_t synth_param = 0;
    uint32_t param_id = 0;
    uint64_t device_id = 0;
    bool step = false;         // SE_AUTO_FLAG_STEPPED: hold each point's value
    bool active = true;        // plugin lanes: the parameter exists
    double native_min = 0.0;   // plugin lanes: normalized -> native (VST3 0..1, CLAP/AU plain range)
    double native_max = 1.0;
};

struct ClipNode {
    const float* src_l = nullptr; // asset channel 0
    const float* src_r = nullptr; // asset channel 1, or channel 0 for mono assets
    int64_t timeline_start = 0;   // first frame on the timeline
    int64_t timeline_end = 0;     // one past the last frame (clamped to asset end)
    int64_t source_offset = 0;    // asset frame played at timeline_start
    float gain = 1.0f;
    int64_t source_frames = 0;    // asset length (warped clips read anywhere in it)
    int32_t warp = -1;            // index into RenderGraph::warps; -1 = plays 1:1
    // Loop (se_session_publish_v8), in content frames from timeline_start;
    // loop_end == 0: plays once.
    int64_t loop_start = 0;
    int64_t loop_end = 0;
    // Fades (se_session_publish_v11), in timeline frames from timeline_start
    // and back from timeline_end; 0 = none. fade_in + fade_out <= the clip.
    int64_t fade_in = 0;
    int64_t fade_out = 0;
};

// A clip's fade gain at clip-relative frame r (see se_session_publish_v11).
inline float clip_fade_gain(const ClipNode& c, int64_t r) noexcept {
    float g = 1.0f;
    if (r < c.fade_in) g = static_cast<float>(r) / static_cast<float>(c.fade_in);
    const int64_t left = c.timeline_end - c.timeline_start - r;
    if (left < c.fade_out) g *= static_cast<float>(left) / static_cast<float>(c.fade_out);
    return g;
}

// Whether a clip's fades touch the timeline frames [from, to).
inline bool clip_fades_in(const ClipNode& c, int64_t from, int64_t to) noexcept {
    return (c.fade_in > 0 && from < c.timeline_start + c.fade_in) ||
           (c.fade_out > 0 && to > c.timeline_end - c.fade_out);
}

// Content frame a clip plays at clip-relative frame r (see se_session_publish_v8).
inline int64_t clip_content(const ClipNode& c, int64_t r) noexcept {
    if (c.loop_end <= 0 || r < c.loop_end) return r;
    return c.loop_start + (r - c.loop_end) % (c.loop_end - c.loop_start);
}

struct TrackNode {
    uint64_t track_id = 0;
    float gain_l = 0.0f; // track gain * constant-power pan (left)
    float gain_r = 0.0f; // track gain * constant-power pan (right)
    float gain = 1.0f;   // static linear gain and pan (automation replaces one or both)
    float pan = 0.0f;
    // Automation (RenderGraph::lanes): -1 = none.
    int32_t vol_lane = -1;
    int32_t pan_lane = -1;
    int32_t mute_lane = -1;     // SE_AUTO_MUTE (replaces the static mute)
    int32_t trim_lane = -1;     // SE_AUTO_TRIM / WIDTH / MONO (replace the strip settings)
    int32_t width_lane = -1;
    int32_t mono_lane = -1;
    int32_t iplug_on_lane = -1; // SE_AUTO_DEVICE_ON of the instrument plugin
    // Any of mute_lane .. iplug_on_lane is set: render_block() only looks at
    // them behind this one flag, so tracks without them pay a single branch.
    bool switch_lanes = false;
    uint32_t synth_lane_begin = 0;
    uint32_t synth_lane_count = 0;
    uint32_t iplug_lane_begin = 0; // instrument plugin parameters
    uint32_t iplug_lane_count = 0;
    // Delay compensation: how late the audio at the fader is relative to the
    // timeline (lat_i + pdc_delay while compensation is active, else 0).
    uint32_t auto_delay = 0;
    bool audible = false;
    bool instrument = false; // render notes through the track's Synth
    uint32_t clip_begin = 0; // index into RenderGraph::clips
    uint32_t clip_count = 0;
    uint32_t note_begin = 0; // index into RenderGraph::notes (instrument tracks only)
    uint32_t note_count = 0; // sorted by start, then pitch
    // Input (se_session_publish_v4). input_mode: SE_INPUT_NONE/MONO/STEREO.
    bool arm = false;
    bool monitor = false;
    uint8_t input_mode = 0;
    uint16_t input_channel = 0;
    // Plugins (se_session_publish_v5). fx_begin/fx_count index RenderGraph::fx.
    uint32_t fx_begin = 0;
    uint32_t fx_count = 0;
    bool has_instrument_plugin = false;          // instrument plugin replaces the synth
    uint64_t instrument_device_id = 0;           // its device id (0 = none)
    bool instrument_plugin_enabled = true;       // false: bypassed (silent)
    PluginInstance* instrument_plugin = nullptr; // null: not loaded (silent)
    // Plugin delay compensation (RenderGraph::pdc; all 0 when inactive).
    uint32_t pdc_delay = 0;     // L - lat_i, applied post-chain (or pre-chain, see pdc_pre)
    uint32_t pdc_src_delay = 0; // instrument plugin latency, applied to the clips of that track
    bool pdc_pre = false;       // monitoring input: delay only the playback source, before the chain
    // Sidechain tap fed with this track's post-fader output (-1 = none).
    int32_t sc_tap = -1;
    // Routing (se_session_publish_v9). `audible`: the track is heard on its
    // output or through its sends; main_on: its output is live; send_live:
    // its sends are live. A return reads bus `bus` (RenderContext::bus_l/r);
    // out_bus: the bus the output plays into (-1 = master). Sends:
    // RenderGraph::sends[send_begin, send_begin + send_count). `plain`: no
    // sends, not a return and output on the master (the render loop then
    // skips the routing work).
    bool is_return = false;
    bool plain = true;
    bool main_on = false;
    bool send_live = false;
    int32_t bus = -1;
    int32_t out_bus = -1;
    uint32_t send_begin = 0;
    uint32_t send_count = 0;
    // Channel strip (se_session_publish_v10): trim, width and mono folded
    // into a 2x2 matrix applied after the insert chain, before pre-fader
    // sends and the fader: L' = a L + b R, R' = b L + a R. strip_on: the
    // matrix is not the identity.
    bool strip_on = false;
    float strip_a = 1.0f;
    float strip_b = 0.0f;
    // The static settings (linear trim, width, mono), for strip lanes that
    // replace some of them.
    float strip_trim = 1.0f;
    float strip_width = 1.0f;
    bool strip_mono = false;
    // Delay compensation: extra delay on the output edge (after the fader)
    // when its destination receives later than the track's earliest one.
    uint32_t out_delay = 0;
    // Modulation: source slots (RenderGraph::mod_sources, fx indices) and
    // the instrument plugin's modulated parameters (RenderGraph::mod_targets).
    uint32_t msrc_begin = 0;
    uint32_t msrc_count = 0;
    uint32_t imod_begin = 0;
    uint32_t imod_count = 0;
};

// One send of a track into a return's bus (se_send_desc).
struct SendNode {
    uint64_t return_id = 0;
    int32_t bus = 0;
    float gain = 1.0f;
    bool pre_fader = false;
    // With a bus soloed, a send into a bus that is only heard downstream of
    // the soloed one runs only from a bus on that path (spec 016); otherwise
    // every send of a track with send_live runs.
    bool live = true;
    uint32_t edge_delay = 0; // delay compensation after the tap (see TrackNode::out_delay)
};

// Plugin delay compensation plan of a graph (docs/adr/0004). Computed on the
// main thread from the plugins' reported latencies; immutable afterwards.
inline constexpr uint32_t kPdcMaxDelay = SE_PDC_MAX_DELAY_FRAMES;
struct PdcPlan {
    bool enabled = false;      // the global switch when the plan was made
    bool clamped = false;      // some delay exceeded kPdcMaxDelay and was clamped
    uint32_t total = 0;        // applied L (0 when disabled), also the metronome delay
    uint32_t uncapped = 0;     // L before clamping
    std::vector<uint32_t> track_latency; // lat_i per track (reported even when disabled)
    // Delay line layout (RenderGraph::lines): [main per track][source per
    // track][bypass per fx slot][metronome][edge per send][output edge per
    // track]. need[i] = frames line i must delay (0 = no line). Empty when
    // no delay is needed.
    std::vector<uint32_t> need;
    std::size_t src_index(std::size_t track_count, std::size_t t) const noexcept { return track_count + t; }
    std::size_t slot_index(std::size_t track_count, std::size_t f) const noexcept { return 2 * track_count + f; }
    std::size_t metro_index(std::size_t track_count, std::size_t fx_count) const noexcept {
        return 2 * track_count + fx_count;
    }
    std::size_t send_index(std::size_t track_count, std::size_t fx_count, std::size_t s) const noexcept {
        return 2 * track_count + fx_count + 1 + s;
    }
    std::size_t out_index(std::size_t track_count, std::size_t fx_count, std::size_t send_count,
                          std::size_t t) const noexcept {
        return 2 * track_count + fx_count + 1 + send_count + t;
    }
};

// Number of buckets in the se_meters_peek sliding window (each bucket covers
// ~10 ms of rendered frames, see Engine::advance_peek_window()).
inline constexpr uint32_t kPeekBuckets = 5;

struct TrackMeter {
    uint64_t track_id = 0;
    std::atomic<float> peak_l{0.0f}; // peak since last se_meters_read (reset by the reader)
    std::atomic<float> peak_r{0.0f};
    std::atomic<float> win_l[kPeekBuckets] = {}; // se_meters_peek ring (zeroed by the RT side)
    std::atomic<float> win_r[kPeekBuckets] = {};
};

struct RenderGraph {
    uint64_t seq = 0; // publication sequence number, assigned when published
    double sample_rate = 48000.0; // the engine's, set when published
    float master_gain = 1.0f;
    // Some track routes device input (input set and armed or monitoring):
    // render_block() only looks at the input when this is set.
    bool uses_input = false;
    std::vector<TrackNode> tracks;
    std::vector<ClipNode> clips;
    std::vector<NoteEvent> notes;
    std::vector<FxSlot> fx; // insert chains, track after track
    std::vector<SendNode> sends; // per track, contiguous (TrackNode::send_begin)
    uint32_t bus_count = 0;      // return tracks (buses in use, <= SE_MAX_RETURNS)
    // Track processing order (sidechain sources first; identity without
    // sidechains) and the number of sidechain taps in use.
    std::vector<uint32_t> order;
    uint32_t sc_taps = 0;
    // Automation: lanes (per track: [volume][pan][synth...][instrument
    // plugin...][slot 0...][slot 1...]...), their points, master volume.
    std::vector<AutoLane> lanes;
    std::vector<AutoPoint> auto_points;
    // Modulation (se_mod_route): targets per device, their routes, sources
    // per track, and keep-alive references to the plugin bases.
    std::vector<ModTarget> mod_targets;
    std::vector<ModRoute> mod_routes;
    std::vector<uint32_t> mod_sources;
    std::vector<std::shared_ptr<std::atomic<float>>> mod_base_refs;
    // Plugin parameters that lost their last route in this publish: their
    // base, sent once at offset 0 of the graph's first block (the previous
    // graph may still have moved them after the main thread's last look).
    struct ModRestore {
        uint64_t device_id = 0;
        uint32_t param_id = 0;
        double value = 0.0;
    };
    std::vector<ModRestore> mod_restores;
    mutable std::atomic<bool> restores_done{false}; // set by the audio thread
    int32_t master_vol_lane = -1;
    bool has_plugin_lanes = false;
    bool has_plugins = false; // any insert or instrument plugin
    // Synth parameters per track, parallel to `tracks` (defaults where the
    // description had none, unused on non-instrument tracks).
    std::vector<se_synth_params> synth_params;
    std::vector<AssetRef> assets; // keep-alive references
    // Per-track synth state for live playback, parallel to `tracks` (null for
    // non-instrument tracks). Assigned by Engine::publish: a track keeps the
    // Synth of the same track_id in the previously published graph, so voices
    // survive republishes. Owned jointly by every graph that uses it; freed
    // with the last such graph, i.e. only on non-RT threads.
    std::vector<std::shared_ptr<Synth>> synth_refs;
    std::vector<Synth*> synths; // raw pointers for the RT path (same order)
    // Live MIDI input state per instrument track, parallel to `tracks` (null
    // elsewhere), carried across republishes by track_id like synth_refs
    // (Engine::install_graph). See midi_input.hpp.
    std::vector<std::shared_ptr<TrackMidi>> midi_refs;
    std::vector<TrackMidi*> midi_tracks;
    // True when some instrument track is armed or monitoring: MIDI input goes
    // to those tracks only (else to the focus track).
    bool midi_explicit = false;
    std::unique_ptr<TrackMeter[]> meters; // one per track, same order
    std::size_t meter_count = 0;
    // Delay compensation: the plan, and the live delay lines it needs (same
    // layout as PdcPlan::need, null where no line is needed; empty when
    // the plan needs none). Lines are mutated only by the RT owner and are
    // carried over to the next graph by key (track / device id), like synths.
    PdcPlan pdc;
    std::vector<std::shared_ptr<DelayLine>> line_refs;
    std::vector<DelayLine*> lines;
    // Warped clips (se_session_publish_v7): per clip its map segments and
    // slices; the stretch voice slots, and the voices for live playback
    // (same order as warp_voice_keys, assigned by Engine::install_graph and
    // kept across republishes like synths; null until then).
    std::vector<WarpClip> warps;
    std::vector<WarpSegment> warp_segments;
    std::vector<WarpSlice> warp_slices;
    std::vector<WarpVoiceKey> warp_voice_keys;
    std::vector<std::shared_ptr<WarpVoice>> warp_voice_refs;
    std::vector<WarpVoice*> warp_voices;
    // Tempo map (se_session_publish_v12): empty when the song has one tempo
    // (devices then see se_transport_set_tempo). click_quarters: quarter notes
    // per metronome beat on the map.
    std::vector<se_tempo_segment> tempo;
    double click_quarters = 1.0;
    TempoMap tempo_map() const noexcept { return TempoMap(tempo.data(), tempo.size(), sample_rate); }
};

// Lane view for evaluation.
inline LaneView lane_view(const RenderGraph& g, const AutoLane& l) noexcept {
    return LaneView{g.auto_points.data() + l.point_begin, l.point_count, l.log, l.step};
}

// Validates one lane description (also used by se_automation_eval).
const char* validate_lane(const se_auto_lane_desc& d) noexcept;

// Copy of `g` for a new plan (same instances, synths and lines; fresh meters
// holding the current peak values). Non-RT.
std::unique_ptr<RenderGraph> clone_render_graph(const RenderGraph& g);

// Constant-power pan law: theta = (pan + 1) * pi / 4, L = cos, R = sin.
// Centre is -3 dB on both sides (L^2 + R^2 = 1 for any pan). Applied as a
// balance control to stereo sources (left gain on the left channel).
void constant_power_pan(float pan, float& left, float& right) noexcept;

// The channel-strip matrix (see TrackNode::strip_a): trim (linear), width
// (0..2) and mono folded into L' = a L + b R, R' = b L + a R.
void strip_matrix(float trim, float width, bool mono, float& a, float& b) noexcept;

// Validates and copies the description. Returns an se_result code. The v1..v8
// entry points convert to v6 (v1..v5: no automation lanes) (v1: no instrument, no notes; v1/v2: default
// synth parameters; v1..v3: no arm/monitor/input; v1..v4: no devices).
// Synth states are not assigned here (see RenderGraph::synth_refs); `synths`
// is sized and null. Plugin slots are created with null instances (the
// engine resolves them in the same track/device order).
int32_t build_render_graph(const se_session_desc_v12& desc, const AssetLookup& lookup,
                           std::unique_ptr<RenderGraph>& out, std::string& err);
int32_t build_render_graph(const se_session_desc_v11& desc, const AssetLookup& lookup,
                           std::unique_ptr<RenderGraph>& out, std::string& err);
int32_t build_render_graph(const se_session_desc_v10& desc, const AssetLookup& lookup,
                           std::unique_ptr<RenderGraph>& out, std::string& err);
int32_t build_render_graph(const se_session_desc_v9& desc, const AssetLookup& lookup,
                           std::unique_ptr<RenderGraph>& out, std::string& err);
int32_t build_render_graph(const se_session_desc_v8& desc, const AssetLookup& lookup,
                           std::unique_ptr<RenderGraph>& out, std::string& err);
int32_t build_render_graph(const se_session_desc_v7& desc, const AssetLookup& lookup,
                           std::unique_ptr<RenderGraph>& out, std::string& err);
int32_t build_render_graph(const se_session_desc_v6& desc, const AssetLookup& lookup,
                           std::unique_ptr<RenderGraph>& out, std::string& err);
int32_t build_render_graph(const se_session_desc_v5& desc, const AssetLookup& lookup,
                           std::unique_ptr<RenderGraph>& out, std::string& err);
int32_t build_render_graph(const se_session_desc_v4& desc, const AssetLookup& lookup,
                           std::unique_ptr<RenderGraph>& out, std::string& err);
int32_t build_render_graph(const se_session_desc_v3& desc, const AssetLookup& lookup,
                           std::unique_ptr<RenderGraph>& out, std::string& err);
int32_t build_render_graph(const se_session_desc_v2& desc, const AssetLookup& lookup,
                           std::unique_ptr<RenderGraph>& out, std::string& err);
int32_t build_render_graph(const se_session_desc& desc, const AssetLookup& lookup,
                           std::unique_ptr<RenderGraph>& out, std::string& err);

} // namespace dstudio
