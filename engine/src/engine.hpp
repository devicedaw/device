// Engine: owns assets, the published render graph, transport, meters, stats
// and the platform device. See engine/README.md for the threading model.
#pragma once

#include "asset.hpp"
#include "midi_input.hpp"
#include "platform/audio_device.hpp"
#include "plugins/plugin_host.hpp"
#include "processor.hpp"
#include "recorder.hpp"
#include "render_graph.hpp"
#include "dstudio_engine.h"
#include "spsc_queue.hpp"
#include "wav.hpp"

#include <array>
#include <atomic>
#include <cstdint>
#include <functional>
#include <map>
#include <memory>
#include <mutex>
#include <string>
#include <unordered_map>
#include <vector>

namespace dstudio {

// Everything that changes RT-owned playback state goes through this one
// trivially copyable command (queued while the device runs, applied inline
// otherwise; see Engine::push_command()).
struct TransportCommand {
    enum class Type : uint8_t { Play, Pause, Stop, Seek, SetLoop, SetMetronome, PreviewPlay, PreviewStop, PreviewSet };
    Type type = Type::Play;
    bool enabled = false;   // SetLoop, SetMetronome; Preview*: solo
    float gain = 0.0f;      // SetMetronome, PreviewPlay, PreviewSet
    float rate = 1.0f;      // PreviewPlay, PreviewSet
    const struct Asset* asset = nullptr; // PreviewPlay (kept alive by Engine::preview_held_)
    uint64_t seq = 0;       // PreviewPlay
    uint32_t beats_per_bar = 0; // SetMetronome
    int64_t frame = 0;      // Seek target, SetLoop start
    int64_t frame2 = 0;     // SetLoop end
    double frames_per_beat = 0.0; // SetMetronome
};

class Engine final : private AudioDevice::Callback {
public:
    static constexpr uint32_t kMaxBlockLimit = 16384;
    static constexpr std::size_t kCommandQueueSize = 256;
    static constexpr uint32_t kMaxInjectedInputChannels = 32;
    using DeviceFactory = std::function<std::unique_ptr<AudioDevice>()>;

    static std::unique_ptr<Engine> create(const se_config& config, std::string& err);
    ~Engine();
    Engine(const Engine&) = delete;
    Engine& operator=(const Engine&) = delete;

    // ---- device
    int32_t device_start(std::string& err);
    int32_t device_stop();
    bool device_running() const noexcept;
    double sample_rate() const noexcept { return sample_rate_; }
    uint32_t max_block() const noexcept { return max_block_; }
    // Validates and stores a device setup; restarts the device if running.
    int32_t device_configure(const AudioDevice::Config& setup, std::string& err);
    void device_status(se_device_status* out);
    void set_input_muted(bool muted) noexcept { input_muted_.store(muted, std::memory_order_relaxed); }
    // Test hook: replaces the platform device (takes effect when no device
    // object exists yet, i.e. before the first start/configure).
    void set_device_factory(DeviceFactory f);

    // ---- MIDI input (midi_input.hpp; se_midi_*)
    MidiInputHub& midi() noexcept { return *midi_; }
    // MidiInputHub::open_only, freeing closed ports' slots right away when
    // nothing renders.
    int32_t midi_open(const std::vector<std::string>& ids, std::string& err);
    // Test hook: a new hub over `backend` (call before any MIDI use, while
    // nothing renders).
    void set_midi_backend(std::unique_ptr<MidiBackend> backend) {
        midi_ = std::make_unique<MidiInputHub>(std::move(backend));
    }

    // ---- assets
    int32_t asset_load_file(const char* path, se_asset_info* out, std::string& err);
    int32_t asset_release(uint64_t id);
    int32_t asset_get_info(uint64_t id, se_asset_info* out) const;
    int32_t asset_reverse(uint64_t id, se_asset_info* out);
    // channel < 0: both channels mixed to mono.
    int32_t asset_compute_peaks(uint64_t id, int32_t channel, uint64_t start_frame, uint32_t frames_per_peak,
                                float* out_minmax, std::size_t capacity, std::size_t* out_count) const;
    int32_t asset_detect_onsets(uint64_t id, se_onset* out, std::size_t capacity, std::size_t* out_count) const;
    std::size_t warp_renders_pending() const { return warp_renderer_.pending(); }
    void set_warp_renders_enabled(bool enabled) { warp_renderer_.set_enabled(enabled); }
    // Registers already-decoded audio (resampling to the engine rate if needed).
    int32_t asset_add_decoded(DecodedAudio&& audio, se_asset_info* out, std::string& err);

    // ---- session
    int32_t publish(const se_session_desc& desc, std::string& err);
    int32_t publish(const se_session_desc_v2& desc, std::string& err);
    int32_t publish(const se_session_desc_v3& desc, std::string& err);
    int32_t publish(const se_session_desc_v4& desc, std::string& err);
    int32_t publish(const se_session_desc_v5& desc, std::string& err);
    int32_t publish(const se_session_desc_v6& desc, std::string& err);
    int32_t publish(const se_session_desc_v7& desc, std::string& err);
    int32_t publish(const se_session_desc_v8& desc, std::string& err);
    int32_t publish(const se_session_desc_v9& desc, std::string& err);
    int32_t publish(const se_session_desc_v10& desc, std::string& err);
    int32_t publish(const se_session_desc_v11& desc, std::string& err);

    // ---- plugins (see dstudio_engine.h; main thread where documented)
    bool plugin_is_live(uint64_t device_id);
    void plugin_load_errors(std::vector<std::pair<uint64_t, std::string>>& out);
    std::size_t plugin_save_states();
    bool plugin_saved_state(std::size_t index, uint64_t& id, const uint8_t*& data, std::size_t& len);
    // Offline renders on a worker (see "Plugins in offline renders" in the README).
    int32_t plugin_save_state_of(uint64_t device_id, std::string& err);
    int32_t plugin_prepare(const se_device_desc& d, bool instrument, const char* track_name, std::string& err);
    void plugin_set_detached(bool detached);
    int32_t plugin_editor_open(uint64_t device_id, std::string& err);
    void plugin_editor_close(uint64_t device_id);
    bool plugin_editor_is_open(uint64_t device_id);
    bool plugin_latency(uint64_t device_id, uint32_t& frames);
    void plugin_idle();

    // ---- plugin parameters (main thread; see dstudio_engine.h)
    int32_t plugin_params(uint64_t device_id, std::vector<ParamInfo>& out, std::string& err);
    int32_t plugin_param_value(uint64_t device_id, uint32_t param_id, float& out, std::string& err);
    int32_t plugin_param_text(uint64_t device_id, uint32_t param_id, float normalized, std::string& out,
                              std::string& err);
    std::size_t take_param_changes(se_plugin_param_change* out, std::size_t capacity);
    // Test hook: an editor-style gesture through the format's notifications (AU).
    bool plugin_simulate_gesture(uint64_t device_id, uint32_t param_id, float normalized);

    // ---- plugin delay compensation (docs/adr/0004)
    void set_pdc_enabled(bool enabled);
    bool pdc_enabled() const noexcept { return pdc_enabled_.load(std::memory_order_acquire); }
    void pdc_report(se_pdc_report& out, std::vector<se_pdc_track>& tracks) const;
    int64_t audible_position() const noexcept;

    // Test hooks.
    uint64_t plugin_instantiations() const noexcept { return plugins_.instantiations(); }
    bool plugin_set_parameter(uint64_t device_id, uint32_t param_id, double value);
    // Built-in devices (main thread).
    int32_t builtin_set_param(uint64_t device_id, uint32_t index, float value);
    int32_t builtin_readouts(uint64_t device_id, float* out, std::size_t capacity, std::size_t& count);
    int32_t builtin_analysis(uint64_t device_id, float* out, std::size_t capacity, std::size_t& count, uint64_t& seq);
    int32_t builtin_set_metering(uint64_t device_id, bool on);
    int32_t builtin_levels(uint64_t device_id, float* out);
    // Plugin card controls and modulation bases (main thread; see dstudio_engine.h).
    int32_t plugin_set_param(uint64_t device_id, uint32_t param_id, float normalized);
    int32_t mod_base(uint64_t device_id, uint32_t param_id, float& out);
    int32_t builtin_note(uint64_t device_id, uint8_t pitch, uint8_t velocity);
    // Frees retired graphs the audio thread can no longer reach. Non-RT.
    void collect_garbage();
    std::size_t retired_graph_count() const;

    // ---- transport
    // Validated by the caller (c_api.cpp). Applied immediately when nobody
    // owns the RT path, otherwise queued for the next block.
    int32_t push_command(const TransportCommand& cmd);
    void transport_get(se_transport_info* out) const noexcept;

    // ---- preview (browser audition) voice, see se_preview_play
    int32_t preview_play(uint64_t asset_id, int64_t start_frame, float gain, float rate, bool solo);
    int32_t preview_stop();
    int32_t preview_note(uint64_t device_id, uint8_t pitch, uint8_t velocity);
    int32_t preview_set(float gain, float rate, bool solo);
    void preview_get(se_preview_info* out) noexcept;

    // ---- meters / stats
    int32_t meters_read(se_meter* out, std::size_t capacity, std::size_t* out_count);
    int32_t meters_peek(se_meter* out, std::size_t capacity, std::size_t* out_count) const;
    void stats_get(se_stats* out) const noexcept;
    void stats_reset() noexcept;

    // ---- recording
    int32_t record_start(const char* dir, std::string& err);
    int32_t record_stop(std::size_t* out_count, std::string& err);
    int32_t record_get_take(std::size_t index, se_recorded_take* out) const;
    void record_state(bool& active, std::vector<LiveTakeInfo>& takes) const;
    std::size_t record_peaks(uint64_t track_id, uint32_t frames_per_peak, float* out, std::size_t capacity) const;
    RecordStats record_stats() const;
    // Test hooks.
    void set_record_writer_stalled(bool stalled);
    void set_record_ring_frames(std::size_t frames) { record_ring_frames_ = frames; }

    // ---- offline
    int32_t render_offline(float* out_interleaved, uint32_t frames, std::string& err);
    int32_t render_offline_with_input(const float* input, uint32_t input_channels, float* out_interleaved,
                                      uint32_t frames, std::string& err);
    int32_t export_wav(const char* path, int64_t start_frame, int64_t end_frame, std::string& err);

private:
    Engine(double sample_rate, uint32_t max_block, uint32_t channels);

    // AudioDevice::Callback (device RT thread).
    void render(float* const* out, uint32_t num_out, const float* const* in, uint32_t num_in,
                uint32_t frames) noexcept override;

    // Input of one process() call: planar device channels or an
    // interleaved injected buffer (or nothing).
    struct RtInput {
        const float* const* planar = nullptr;
        const float* interleaved = nullptr;
        uint32_t channels = 0;
    };

    // The single RT entry point used by the device callback and
    // render_offline. `sink(l, r, offset, n)` receives each rendered chunk.
    // `rec_latency` is subtracted from the start of new recorded takes.
    template <class Sink>
    void process(uint32_t frames, const RtInput& input, int64_t rec_latency, Sink&& sink) noexcept;
    int32_t render_offline_impl(const RtInput& input, float* out_interleaved, uint32_t frames, std::string& err);
    void refresh_device_latency();

    void reclaim_locked(); // requires control_mutex_
    // Delay compensation plan of `g` from the instances' cached latencies
    // (requires plugins_.mutex()).
    void plan_pdc(RenderGraph& g) const;
    // Re-plans the current graph (clone + install). Requires plugins_.mutex().
    void replan_pdc_locked();
    // Installs a copy of the current graph with the warp renders that finished
    // since (clone + install). Requires plugins_.mutex().
    void attach_warp_renders_locked();
    // Assigns synth states (reusing the previous graph's by track_id) and
    // publishes the graph to the RT side. Returns the graph's seq.
    uint64_t install_graph(std::unique_ptr<RenderGraph> built);
    // install_graph + retire every plugin instance (v1..v4 publishes).
    int32_t publish_graph(std::unique_ptr<RenderGraph> built);

    // Who currently owns the live render path (RT role). Exactly one owner
    // may touch the rt_* state and pop the command queue. kOwnerControl is a
    // short-lived claim taken by a non-RT thread to apply queued commands
    // synchronously when no device or offline render owns the path.
    enum : int { kOwnerNone = 0, kOwnerDevice = 1, kOwnerOffline = 2, kOwnerControl = 3 };

    // CAS rt_owner_ None -> `who`, waiting out transient kOwnerControl
    // claims. Returns the conflicting owner (Device/Offline) or kOwnerNone
    // on success.
    int acquire_rt_owner(int who) noexcept;
    // Store None, then apply any commands that were queued while we owned the
    // path (Dekker-style handshake with push_command, see engine.cpp).
    void release_rt_owner() noexcept;

    // ---- RT-owner-only helpers (caller holds rt_owner_)
    void apply_command(const TransportCommand& cmd) noexcept;
    void drain_commands() noexcept;
    void publish_transport() noexcept;
    uint32_t advance_peek_window(uint32_t frames, const RenderGraph* graph) noexcept;
    // r == nullptr: mono (l only).
    void mix_metronome(float* l, float* r, int64_t position, uint32_t frames) noexcept;
    void start_click(const float* buf, uint32_t offset, float* l, float* r, uint32_t frames) noexcept;
    // Adds the preview voices to l/r (silencing them first when soloed);
    // returns true when anything was added.
    bool mix_preview(float* l, float* r, uint32_t frames) noexcept;
    // Drops held preview assets the RT side can no longer reach. Requires preview_mutex_.
    void reclaim_previews_locked();

    const double sample_rate_;
    const uint32_t max_block_;
    const uint32_t channels_;

    // -- assets (non-RT)
    mutable std::mutex assets_mutex_;
    std::unordered_map<uint64_t, AssetRef> assets_;
    uint64_t next_asset_id_ = 1;

    // -- graph publication
    mutable std::mutex control_mutex_; // guards everything below up to live_graph_
    std::shared_ptr<RenderGraph> current_;                 // owner of the published graph
    std::vector<std::shared_ptr<RenderGraph>> retired_;    // waiting for the RT side to let go
    uint64_t next_seq_ = 1;
    std::atomic<RenderGraph*> live_graph_{nullptr};        // what the RT side loads
    // RT-side "hazard" marker: kIdle between blocks, kBusy while acquiring,
    // otherwise the seq of the graph in use. See reclaim_locked().
    static constexpr uint64_t kIdle = 0;
    static constexpr uint64_t kBusy = ~uint64_t{0};
    std::atomic<uint64_t> rt_active_seq_{kIdle};

    // -- device
    std::mutex device_mutex_;
    std::unique_ptr<AudioDevice> device_;
    DeviceFactory device_factory_;          // guarded by device_mutex_
    AudioDevice::Config device_setup_;      // guarded by device_mutex_ (se_device_configure)
    std::string input_error_;               // guarded by device_mutex_
    std::atomic<int64_t> device_rec_latency_{0}; // engine frames, input + output
    std::atomic<int64_t> device_out_latency_{0}; // engine frames, output side (audible position)
    std::atomic<bool> device_input_open_{false};
    std::atomic<bool> input_muted_{false};
    std::atomic<int> rt_owner_{kOwnerNone};

    // -- transport: commands in, state out
    std::mutex command_mutex_; // serialises producers (SPSC producer side)
    SpscQueue<TransportCommand, kCommandQueueSize> commands_;
    int64_t rt_position_ = 0; // RT-owned
    bool rt_playing_ = false; // RT-owned
    bool rt_loop_enabled_ = false;  // RT-owned
    int64_t rt_loop_start_ = 0;     // RT-owned
    int64_t rt_loop_end_ = 0;       // RT-owned
    // Bumped on every playback jump (seek, stop, pause, loop wrap); synths
    // fade their voices when it changes. RT-owned.
    uint64_t rt_synth_gen_ = 0;
    // Bumped on seek and stop: delay lines flush lazily when it changes.
    // RT-owned.
    uint64_t rt_pdc_gen_ = 0;
    // Tempo for plugins: the metronome grid (frames per beat), kept even
    // while the metronome is disabled; 0 = 120 BPM. RT-owned.
    double rt_tempo_fpb_ = 0.0;
    std::atomic<int64_t> pub_position_{0};
    // Timeline frames of recently rendered samples (automation under delay
    // compensation). RT-owned.
    TimeHistory rt_history_;
    std::atomic<bool> pub_playing_{false};

    // -- metronome. Click shapes are computed in the constructor; the RT
    // side keeps a few voices so a click's tail continues across blocks,
    // loop wraps and seeks.
    struct ClickVoice {
        const float* buf = nullptr;
        uint32_t pos = 0; // next sample index into buf
    };
    static constexpr std::size_t kMaxClickVoices = 8;
    std::vector<float> click_down_; // downbeat (1.2 kHz)
    std::vector<float> click_beat_; // other beats (800 Hz)
    uint32_t click_len_ = 0;
    bool rt_metro_enabled_ = false;     // RT-owned
    float rt_metro_gain_ = 0.0f;        // RT-owned
    double rt_metro_fpb_ = 0.0;         // RT-owned
    uint32_t rt_metro_bpb_ = 1;         // RT-owned
    std::array<ClickVoice, kMaxClickVoices> rt_voices_{}; // RT-owned
    std::size_t rt_voice_count_ = 0;    // RT-owned

    // -- preview voice. The non-RT side holds every asset a queued or sounding
    // voice may read (by play seq); the RT side publishes the lowest seq it may
    // still read, and everything below it is dropped (off the RT thread).
    struct PreviewVoice {
        const Asset* asset = nullptr;
        uint64_t seq = 0;
        double pos = 0.0;
        float fade = 0.0f; // 0..1 envelope
        float step = 0.0f; // per frame; < 0 fades out
    };
    static constexpr float kPreviewFadeFrames = 96.0f;
    std::mutex preview_mutex_;
    std::vector<std::pair<uint64_t, AssetRef>> preview_held_; // guarded by preview_mutex_
    uint64_t preview_seq_ = 0;                                // guarded by preview_mutex_
    std::vector<std::pair<uint64_t, uint8_t>> preview_notes_; // held (device, pitch), guarded by preview_mutex_
    std::atomic<uint32_t> pub_pv_notes_{0};
    std::atomic<uint64_t> rt_preview_floor_{0};
    PreviewVoice rt_pv_[2];             // RT-owned: [0] current, [1] fading out
    float rt_pv_gain_ = 1.0f;           // RT-owned: target
    float rt_pv_gain_cur_ = 1.0f;       // RT-owned: smoothed
    float rt_pv_rate_ = 1.0f;           // RT-owned
    bool rt_pv_solo_ = false;           // RT-owned
    uint64_t rt_pv_applied_ = 0;        // RT-owned: newest PreviewPlay seq applied
    std::atomic<uint64_t> pub_pv_asset_{0};
    std::atomic<int64_t> pub_pv_pos_{0};
    std::atomic<int64_t> pub_pv_frames_{0};
    std::atomic<bool> pub_pv_solo_{false};
    std::atomic<float> pv_peak_l_{0.0f};
    std::atomic<float> pv_peak_r_{0.0f};

    // -- meters (master; track meters live in the graph)
    std::atomic<float> master_peak_l_{0.0f};
    std::atomic<float> master_peak_r_{0.0f};
    // se_meters_peek window: kPeekBuckets buckets of peek_bucket_frames_.
    std::atomic<float> master_win_l_[kPeekBuckets] = {};
    std::atomic<float> master_win_r_[kPeekBuckets] = {};
    uint32_t peek_bucket_frames_ = 1;
    uint32_t rt_peek_slot_ = 0;   // RT-owned
    uint64_t rt_peek_fill_ = 0;   // RT-owned: frames in the current bucket

    // -- stats (written by RT only, except reset)
    std::atomic<uint64_t> stat_callbacks_{0};
    std::atomic<uint64_t> stat_overloads_{0};
    std::atomic<uint64_t> stat_last_ns_{0};
    std::atomic<uint64_t> stat_max_ns_{0};
    std::atomic<uint64_t> stat_total_ns_{0};
    std::atomic<uint64_t> stat_deadline_ns_{0};

    // -- recording. recorder_ is owned on the non-RT side (rec_mutex_); the RT
    // side sees it through rec_live_ and announces use with rec_in_use_
    // (Dekker handshake, see record_stop()).
    mutable std::mutex rec_mutex_;
    std::unique_ptr<Recorder> recorder_;
    std::vector<RecordedTakeInfo> last_takes_;
    RecordStats last_stats_;
    std::size_t record_ring_frames_ = 0; // 0 = 2 s at the engine rate
    std::atomic<Recorder*> rec_live_{nullptr};
    std::atomic<bool> rec_in_use_{false};

    // -- MIDI input (midi_input.hpp). The hub's queues are read by the RT
    // owner; its ports are opened and closed on the main thread.
    std::unique_ptr<MidiInputHub> midi_ = std::make_unique<MidiInputHub>();
    std::array<MidiEvent, MidiInputHub::kMaxBlockEvents> rt_midi_{}; // RT-owned
    bool rt_midi_wrapped_ = false; // RT-owned: a loop wrap happened since capture started
    uint32_t rt_capture_pass_ = 0; // RT-owned: the capture pass rt_midi_wrapped_ belongs to

    // -- plugins (instances shared by every graph that references them)
    PluginHost plugins_;
    // Normalized bases of modulated plugin parameters by (device, parameter),
    // shared with the graphs that modulate them (guarded by plugins_.mutex()).
    std::map<std::pair<uint64_t, uint32_t>, std::shared_ptr<std::atomic<float>>> mod_bases_;
    // -- delay compensation switch (written under plugins_.mutex()) and the
    // applied L of the current graph (for audible_position()).
    std::atomic<bool> pdc_enabled_{true};
    std::atomic<uint32_t> pdc_total_{0};

    // -- RT scratch (used by whoever holds rt_owner_)
    RenderContext live_ctx_;
    std::vector<float> in_scratch_; // kMaxInjectedInputChannels x max_block (deinterleaved injected input)
    std::vector<float> click_scratch_; // max_block, metronome before its delay line

    // -- warped clips rendered ahead of time (resolve / evict under
    // plugins_.mutex(), like every graph install). Declared last: its worker
    // stops first.
    WarpRenderer warp_renderer_;
};

} // namespace dstudio
