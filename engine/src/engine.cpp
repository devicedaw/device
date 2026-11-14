#include "engine.hpp"

#include "delay_line.hpp"
#include "devices/builtin.hpp"
#include "resampler.hpp"
#include "rt_util.hpp"
#include "wav.hpp"

#include <algorithm>
#include <cmath>
#include <cstring>
#include <limits>
#include <chrono>
#include <map>
#include <numbers>
#include <thread>
#include <tuple>

namespace dstudio {

namespace {

// ~15 ms exponentially decaying sine click. Phase starts half a sample in so
// the very first sample is non-zero (the onset frame is the beat frame); a
// short raised-cosine fade makes the last sample exactly reach zero.
std::vector<float> make_click(double sample_rate, double freq, std::size_t len) {
    std::vector<float> v(len);
    const double tau = 0.003 * sample_rate; // decay time constant (frames)
    const std::size_t fade = std::max<std::size_t>(1, static_cast<std::size_t>(0.002 * sample_rate));
    for (std::size_t i = 0; i < len; ++i) {
        const double t = static_cast<double>(i);
        double env = std::exp(-t / tau);
        const std::size_t left = len - 1 - i;
        if (left < fade) env *= 0.5 - 0.5 * std::cos(std::numbers::pi * static_cast<double>(left) / static_cast<double>(fade));
        v[i] = static_cast<float>(std::sin(2.0 * std::numbers::pi * freq * (t + 0.5) / sample_rate) * env);
    }
    return v;
}

int64_t floor_mod(int64_t a, int64_t m) noexcept {
    const int64_t r = a % m;
    return r < 0 ? r + m : r;
}

} // namespace

// ============================================================ lifecycle

Engine::Engine(double sample_rate, uint32_t max_block, uint32_t channels)
    : sample_rate_(sample_rate), max_block_(max_block), channels_(channels), plugins_(sample_rate, max_block),
      live_ctx_(max_block), warp_renderer_(sample_rate, max_block) {
    // Preallocated here so neither se_metronome_set nor the callback allocates.
    const auto len = static_cast<std::size_t>(std::llround(0.015 * sample_rate));
    click_down_ = make_click(sample_rate, 1200.0, len);
    click_beat_ = make_click(sample_rate, 800.0, len);
    click_len_ = static_cast<uint32_t>(len);
    peek_bucket_frames_ = std::max<uint32_t>(1, static_cast<uint32_t>(std::llround(0.010 * sample_rate)));
    in_scratch_.assign(static_cast<std::size_t>(kMaxInjectedInputChannels) * max_block, 0.0f);
    click_scratch_.assign(max_block, 0.0f);
    device_factory_ = [] { return create_platform_audio_device(); };
    plugins_.bus().set_position_source(&pub_position_);
    plugins_.set_asset_lookup([this](uint64_t id) -> AssetRef {
        std::lock_guard lock(assets_mutex_);
        auto it = assets_.find(id);
        return it == assets_.end() ? nullptr : it->second;
    });
}

std::unique_ptr<Engine> Engine::create(const se_config& config, std::string& err) {
    if (config.abi_version != SE_ABI_VERSION) {
        err = "ABI version mismatch: header " + std::to_string(SE_ABI_VERSION) + ", caller " +
              std::to_string(config.abi_version);
        return nullptr;
    }
    double rate = config.sample_rate;
    if (rate <= 0.0) {
        // 0 = "use the device's rate". Fixed for the engine's lifetime because
        // assets are resampled to it on import.
        rate = query_default_output_sample_rate();
        if (rate <= 0.0) rate = 48000.0;
    }
    if (!std::isfinite(rate) || rate < 8000.0 || rate > 768000.0) {
        err = "unsupported sample rate";
        return nullptr;
    }
    if (config.max_block_size == 0 || config.max_block_size > kMaxBlockLimit) {
        err = "max_block_size must be in 1.." + std::to_string(kMaxBlockLimit);
        return nullptr;
    }
    if (config.output_channels != 2) {
        err = "only output_channels == 2 is supported in this milestone";
        return nullptr;
    }
    return std::unique_ptr<Engine>(new Engine(rate, config.max_block_size, config.output_channels));
}

Engine::~Engine() {
    device_stop();
    {
        // Nothing renders any more; finalize a recording still in progress.
        std::lock_guard lock(rec_mutex_);
        rec_live_.store(nullptr, std::memory_order_seq_cst);
        if (recorder_) recorder_->finish(last_takes_);
        recorder_.reset();
    }
    {
        // Nothing renders any more: destroy plugin instances (and close their
        // editors) here, on the destroying thread (the app's main thread).
        std::lock_guard lock(plugins_.mutex());
        plugins_.shutdown_all();
    }
    // Nothing can be on the RT path any more (contract: no calls race with
    // destroy). Members are destroyed normally on this (non-RT) thread.
}

// ============================================================ device

int32_t Engine::device_start(std::string& err) {
    std::lock_guard lock(device_mutex_);
    const int conflict = acquire_rt_owner(kOwnerDevice);
    if (conflict != kOwnerNone) {
        err = conflict == kOwnerDevice ? "device already running" : "offline render in progress";
        return SE_ERR_STATE;
    }
    if (!device_) device_ = device_factory_();
    AudioDevice::Config cfg = device_setup_; // defaults: default output, no input
    cfg.sample_rate = sample_rate_;
    cfg.channels = channels_;
    cfg.preferred_block = max_block_;
    const int32_t rc = device_->start(cfg, this, err);
    if (rc != SE_OK) {
        device_input_open_.store(false);
        release_rt_owner();
        return rc;
    }
    refresh_device_latency();
    input_error_ = device_->status().input_error;
    return SE_OK;
}

// Requires device_mutex_ and a started device.
void Engine::refresh_device_latency() {
    const AudioDevice::Status st = device_->status();
    const double ratio = st.device_rate > 0.0 ? sample_rate_ / st.device_rate : 1.0;
    const int64_t lat =
        st.input_open ? std::llround(static_cast<double>(st.input_latency + st.output_latency) * ratio) : 0;
    device_rec_latency_.store(lat, std::memory_order_relaxed);
    device_out_latency_.store(std::llround(static_cast<double>(st.output_latency) * ratio), std::memory_order_relaxed);
    device_input_open_.store(st.input_open, std::memory_order_release);
}

int32_t Engine::device_configure(const AudioDevice::Config& setup, std::string& err) {
    std::lock_guard lock(device_mutex_);
    if (!device_) device_ = device_factory_();
    AudioDevice::Config cfg = setup;
    cfg.sample_rate = sample_rate_;
    cfg.channels = channels_;
    cfg.preferred_block = max_block_;
    int32_t rc = device_->validate(cfg, err); // opens nothing
    if (rc != SE_OK) return rc;
    device_setup_ = cfg;
    input_error_.clear();
    if (rt_owner_.load() != kOwnerDevice) return SE_OK; // applied by the next se_device_start
    // Restart in place: we keep the RT ownership across stop/start, so no
    // offline render can sneak in; queued commands wait for the new stream.
    device_->stop();
    rc = device_->start(cfg, this, err);
    if (rc != SE_OK) {
        device_input_open_.store(false);
        release_rt_owner();
        return rc;
    }
    refresh_device_latency();
    const AudioDevice::Status st = device_->status();
    input_error_ = st.input_error;
    if (cfg.input_enabled && !st.input_open) {
        err = "output restarted, but the input could not be opened: " + st.input_error;
        return SE_ERR_DEVICE;
    }
    return SE_OK;
}

namespace {
void copy_str(char* dst, std::size_t cap, const std::string& src) {
    const std::size_t n = std::min(cap - 1, src.size());
    std::memcpy(dst, src.data(), n);
    dst[n] = '\0';
}
} // namespace

void Engine::device_status(se_device_status* out) {
    std::lock_guard lock(device_mutex_);
    *out = se_device_status{};
    out->sample_rate = sample_rate_;
    copy_str(out->input_error, sizeof out->input_error, input_error_);
    if (!device_ || rt_owner_.load() != kOwnerDevice) return;
    const AudioDevice::Status st = device_->status();
    const double ratio = st.device_rate > 0.0 ? sample_rate_ / st.device_rate : 1.0;
    out->running = 1;
    out->input_open = st.input_open ? 1 : 0;
    copy_str(out->output_name, sizeof out->output_name, st.output_name);
    if (st.input_open) copy_str(out->input_name, sizeof out->input_name, st.input_name);
    if (st.device_rate > 0.0) out->sample_rate = st.device_rate;
    out->buffer_size = st.buffer_size;
    out->input_channels = st.input_channels;
    out->output_channels = st.output_channels;
    out->input_latency_frames = static_cast<uint32_t>(std::llround(st.input_latency * ratio));
    out->output_latency_frames = static_cast<uint32_t>(std::llround(st.output_latency * ratio));
}

void Engine::set_device_factory(DeviceFactory f) {
    std::lock_guard lock(device_mutex_);
    device_factory_ = std::move(f);
    if (rt_owner_.load() != kOwnerDevice) device_.reset();
}

int32_t Engine::device_stop() {
    std::lock_guard lock(device_mutex_);
    if (rt_owner_.load() != kOwnerDevice) return SE_OK;
    if (device_) device_->stop(); // returns once the callback can no longer run
    device_input_open_.store(false);
    device_out_latency_.store(0, std::memory_order_relaxed);
    // Commands queued but not yet consumed by a callback are applied here.
    release_rt_owner();
    return SE_OK;
}

bool Engine::device_running() const noexcept {
    return rt_owner_.load(std::memory_order_acquire) == kOwnerDevice;
}

// ============================================================ assets

int32_t Engine::asset_add_decoded(DecodedAudio&& audio, se_asset_info* out, std::string& err) {
    if (audio.channels < 1 || audio.channels > 2) {
        err = "asset must be mono or stereo";
        return SE_ERR_UNSUPPORTED_FORMAT;
    }
    auto asset = std::make_shared<Asset>();
    asset->channels = audio.channels;
    asset->source_sample_rate = audio.sample_rate;
    if (audio.sample_rate == sample_rate_) {
        for (uint32_t c = 0; c < audio.channels; ++c) asset->planar[c] = std::move(audio.planar[c]);
        asset->frames = audio.frames;
    } else {
        const SincResampler rs(audio.sample_rate, sample_rate_);
        for (uint32_t c = 0; c < audio.channels; ++c) {
            rs.process(audio.planar[c].data(), audio.frames, asset->planar[c]);
            audio.planar[c] = {};
        }
        asset->frames = asset->planar[0].size();
    }

    std::lock_guard lock(assets_mutex_);
    asset->id = next_asset_id_++;
    if (out != nullptr) {
        out->asset_id = asset->id;
        out->frames = asset->frames;
        out->channels = asset->channels;
        out->source_sample_rate = asset->source_sample_rate;
    }
    assets_.emplace(asset->id, std::move(asset));
    return SE_OK;
}

int32_t Engine::asset_load_file(const char* path, se_asset_info* out, std::string& err) {
    DecodedAudio audio;
    const int32_t rc = decode_audio_file(path, audio, err);
    if (rc != SE_OK) return rc;
    return asset_add_decoded(std::move(audio), out, err);
}

int32_t Engine::asset_release(uint64_t id) {
    AssetRef victim; // dropped outside the lock
    {
        std::lock_guard lock(assets_mutex_);
        auto it = assets_.find(id);
        if (it == assets_.end()) return SE_ERR_NOT_FOUND;
        victim = std::move(it->second);
        assets_.erase(it);
    }
    // Published/retired graphs may still hold the asset; it is freed when the
    // last of them is reclaimed (always on a non-RT thread).
    collect_garbage();
    return SE_OK;
}

int32_t Engine::asset_reverse(uint64_t id, se_asset_info* out) {
    AssetRef src;
    {
        std::lock_guard lock(assets_mutex_);
        auto it = assets_.find(id);
        if (it == assets_.end()) return SE_ERR_NOT_FOUND;
        src = it->second;
    }
    auto asset = std::make_shared<Asset>();
    asset->channels = src->channels;
    asset->frames = src->frames;
    asset->source_sample_rate = src->source_sample_rate;
    for (uint32_t c = 0; c < src->channels; ++c) asset->planar[c].assign(src->planar[c].rbegin(), src->planar[c].rend());
    std::lock_guard lock(assets_mutex_);
    asset->id = next_asset_id_++;
    if (out != nullptr) {
        out->asset_id = asset->id;
        out->frames = asset->frames;
        out->channels = asset->channels;
        out->source_sample_rate = asset->source_sample_rate;
    }
    assets_.emplace(asset->id, std::move(asset));
    return SE_OK;
}

int32_t Engine::asset_get_info(uint64_t id, se_asset_info* out) const {
    std::lock_guard lock(assets_mutex_);
    auto it = assets_.find(id);
    if (it == assets_.end()) return SE_ERR_NOT_FOUND;
    const Asset& a = *it->second;
    out->asset_id = a.id;
    out->frames = a.frames;
    out->channels = a.channels;
    out->source_sample_rate = a.source_sample_rate;
    return SE_OK;
}

int32_t Engine::asset_compute_peaks(uint64_t id, int32_t channel, uint64_t start_frame, uint32_t frames_per_peak,
                                    float* out_minmax, std::size_t capacity, std::size_t* out_count) const {
    AssetRef asset;
    {
        std::lock_guard lock(assets_mutex_);
        auto it = assets_.find(id);
        if (it == assets_.end()) return SE_ERR_NOT_FOUND;
        asset = it->second;
    }
    if (channel >= 0 && static_cast<uint32_t>(channel) >= asset->channels) return SE_ERR_INVALID_ARG;
    std::size_t count = 0;
    const float* l = asset->channel(channel < 0 ? 0 : static_cast<uint32_t>(channel));
    const float* r = asset->channel(1);
    const bool stereo = channel < 0 && asset->channels == 2;
    for (uint64_t f = start_frame; count < capacity && f < asset->frames; f += frames_per_peak) {
        const uint64_t end = std::min<uint64_t>(asset->frames, f + frames_per_peak);
        float lo = std::numeric_limits<float>::infinity();
        float hi = -std::numeric_limits<float>::infinity();
        for (uint64_t i = f; i < end; ++i) {
            const float v = stereo ? 0.5f * (l[i] + r[i]) : l[i];
            lo = std::min(lo, v);
            hi = std::max(hi, v);
        }
        out_minmax[2 * count] = lo;
        out_minmax[2 * count + 1] = hi;
        ++count;
    }
    if (out_count != nullptr) *out_count = count;
    return SE_OK;
}

int32_t Engine::asset_detect_onsets(uint64_t id, se_onset* out, std::size_t capacity, std::size_t* out_count) const {
    AssetRef asset;
    {
        std::lock_guard lock(assets_mutex_);
        auto it = assets_.find(id);
        if (it == assets_.end()) return SE_ERR_NOT_FOUND;
        asset = it->second;
    }
    std::vector<se_onset> found;
    detect_onsets(*asset, sample_rate_, found);
    if (out != nullptr) std::copy_n(found.begin(), std::min(capacity, found.size()), out);
    if (out_count != nullptr) *out_count = found.size();
    return SE_OK;
}

// ============================================================ session

int32_t Engine::publish(const se_session_desc& desc, std::string& err) {
    // Build entirely off the RT thread. Asset lookups take assets_mutex_.
    std::unique_ptr<RenderGraph> built;
    const AssetLookup lookup = [this](uint64_t id) -> AssetRef {
        std::lock_guard lock(assets_mutex_);
        auto it = assets_.find(id);
        return it == assets_.end() ? nullptr : it->second;
    };
    const int32_t rc = build_render_graph(desc, lookup, built, err);
    if (rc != SE_OK) return rc;
    return publish_graph(std::move(built));
}

int32_t Engine::publish(const se_session_desc_v2& desc, std::string& err) {
    std::unique_ptr<RenderGraph> built;
    const AssetLookup lookup = [this](uint64_t id) -> AssetRef {
        std::lock_guard lock(assets_mutex_);
        auto it = assets_.find(id);
        return it == assets_.end() ? nullptr : it->second;
    };
    const int32_t rc = build_render_graph(desc, lookup, built, err);
    if (rc != SE_OK) return rc;
    return publish_graph(std::move(built));
}

int32_t Engine::publish(const se_session_desc_v3& desc, std::string& err) {
    std::unique_ptr<RenderGraph> built;
    const AssetLookup lookup = [this](uint64_t id) -> AssetRef {
        std::lock_guard lock(assets_mutex_);
        auto it = assets_.find(id);
        return it == assets_.end() ? nullptr : it->second;
    };
    const int32_t rc = build_render_graph(desc, lookup, built, err);
    if (rc != SE_OK) return rc;
    return publish_graph(std::move(built));
}

int32_t Engine::publish(const se_session_desc_v4& desc, std::string& err) {
    std::unique_ptr<RenderGraph> built;
    const AssetLookup lookup = [this](uint64_t id) -> AssetRef {
        std::lock_guard lock(assets_mutex_);
        auto it = assets_.find(id);
        return it == assets_.end() ? nullptr : it->second;
    };
    const int32_t rc = build_render_graph(desc, lookup, built, err);
    if (rc != SE_OK) return rc;
    return publish_graph(std::move(built));
}

// Synth states outlive graph swaps: an instrument track takes over the Synth
// of the same track_id in the currently published graph (the RT side keeps
// mutating that object; we only copy the reference). New instrument tracks
// get a fresh Synth, allocated here. A Synth whose track disappeared stays
// owned by the retired graph(s) and is freed when they are reclaimed, which
// happens only on non-RT threads once the RT side can no longer reach them.
int32_t Engine::publish(const se_session_desc_v5& desc, std::string& err) {
    if (desc.track_count > 0 && desc.tracks == nullptr) {
        err = "session has track_count > 0 but tracks == NULL";
        return SE_ERR_INVALID_ARG;
    }
    std::vector<se_track_desc_v6> tracks(desc.track_count);
    for (uint32_t t = 0; t < desc.track_count; ++t) {
        tracks[t] = se_track_desc_v6{};
        std::memcpy(&tracks[t], &desc.tracks[t], sizeof(se_track_desc_v5)); // v6 extends v5 field by field
    }
    se_session_desc_v6 d6{};
    d6.tracks = tracks.data();
    d6.track_count = desc.track_count;
    d6.master_gain = desc.master_gain;
    return publish(d6, err);
}

int32_t Engine::publish(const se_session_desc_v6& desc, std::string& err) {
    se_session_desc_v7 d7{};
    d7.v6 = desc;
    return publish(d7, err);
}

int32_t Engine::publish(const se_session_desc_v7& desc, std::string& err) {
    se_session_desc_v8 d8{};
    d8.v7 = desc;
    return publish(d8, err);
}

int32_t Engine::publish(const se_session_desc_v8& desc8, std::string& err) {
    se_session_desc_v9 d9{};
    d9.v8 = desc8;
    return publish(d9, err);
}

int32_t Engine::publish(const se_session_desc_v9& desc9, std::string& err) {
    se_session_desc_v10 d10{};
    d10.v9 = desc9;
    return publish(d10, err);
}

int32_t Engine::publish(const se_session_desc_v10& desc10, std::string& err) {
    se_session_desc_v11 d11{};
    d11.v10 = desc10;
    return publish(d11, err);
}

int32_t Engine::publish(const se_session_desc_v11& desc11, std::string& err) {
    const se_session_desc_v10& desc10 = desc11.v10;
    const se_session_desc_v9& desc9 = desc10.v9;
    const se_session_desc_v8& desc8 = desc9.v8;
    const se_session_desc_v6& desc = desc8.v7.v6;
    std::unique_ptr<RenderGraph> built;
    const AssetLookup lookup = [this](uint64_t id) -> AssetRef {
        std::lock_guard lock(assets_mutex_);
        auto it = assets_.find(id);
        return it == assets_.end() ? nullptr : it->second;
    };
    const int32_t rc = build_render_graph(desc11, lookup, built, err);
    if (rc != SE_OK) return rc;
    built->sample_rate = sample_rate_;
    warp_renderer_.resolve(*built);
    // Resolve plugin instances (instantiating new device ids on this, the
    // main, thread), in the order build_render_graph created the slots.
    std::lock_guard plock(plugins_.mutex());
    plugins_.begin_publish();
    for (uint32_t t = 0; t < desc.track_count; ++t) {
        const se_track_desc_v6& td = desc.tracks[t];
        TrackNode& node = built->tracks[t];
        for (uint32_t i = 0; i < td.device_count; ++i)
            built->fx[node.fx_begin + i].inst = plugins_.use(td.devices[i], false, td.track_name);
        if (node.has_instrument_plugin)
            node.instrument_plugin = plugins_.use(*td.instrument_plugin, true, td.track_name);
    }
    // Plugin automation lanes: the parameter's native range, or inactive
    // (device not loaded, unknown parameter).
    if (built->has_plugin_lanes) {
        const auto resolve = [&](PluginInstance* inst, uint32_t begin, uint32_t count) {
            for (uint32_t i = begin; i < begin + count; ++i) {
                AutoLane& lane = built->lanes[i];
                BuiltinInstance* const b = inst != nullptr ? inst->as_builtin() : nullptr;
                if (lane.target == SE_AUTO_BUILTIN || b != nullptr) // built-in lanes only on built-ins, and vice versa
                    lane.active = b != nullptr && lane.target == SE_AUTO_BUILTIN && b->param_automatable(lane.param_id);
                else
                    lane.active = inst != nullptr && inst->param_range(lane.param_id, lane.native_min, lane.native_max);
            }
        };
        for (TrackNode& node : built->tracks) {
            if (node.iplug_lane_count > 0) resolve(node.instrument_plugin, node.iplug_lane_begin, node.iplug_lane_count);
            for (uint32_t i = 0; i < node.fx_count; ++i) {
                FxSlot& slot = built->fx[node.fx_begin + i];
                if (slot.lane_count > 0) resolve(slot.inst, slot.lane_begin, slot.lane_count);
            }
        }
    }
    // Modulation targets: built-in parameters exist on built-ins; plugin
    // parameters get their native range, lane and base (kept across publishes).
    {
        std::map<std::pair<uint64_t, uint32_t>, std::shared_ptr<std::atomic<float>>> bases;
        const auto resolve_mods = [&](PluginInstance* inst, uint64_t device_id, uint32_t begin, uint32_t count,
                                      uint32_t lane_begin, uint32_t lane_count) {
            BuiltinInstance* const b = inst != nullptr ? inst->as_builtin() : nullptr;
            for (uint32_t i = begin; i < begin + count; ++i) {
                ModTarget& t = built->mod_targets[i];
                if (!t.plugin) {
                    t.active = b != nullptr && t.param_id < b->param_count();
                    continue;
                }
                t.active = inst != nullptr && b == nullptr && inst->param_range(t.param_id, t.native_min, t.native_max);
                if (!t.active) continue;
                for (uint32_t l = lane_begin; l < lane_begin + lane_count; ++l) {
                    const AutoLane& lane = built->lanes[l];
                    if (lane.active && lane.target == SE_AUTO_PLUGIN && lane.param_id == t.param_id) t.lane = static_cast<int32_t>(l);
                }
                const auto key = std::make_pair(device_id, t.param_id);
                std::shared_ptr<std::atomic<float>> base;
                if (auto it = mod_bases_.find(key); it != mod_bases_.end()) {
                    base = it->second;
                } else if (auto jt = bases.find(key); jt != bases.end()) {
                    base = jt->second;
                } else {
                    float v = 0.0f;
                    inst->param_value(t.param_id, v);
                    base = std::make_shared<std::atomic<float>>(std::clamp(std::isfinite(v) ? v : 0.0f, 0.0f, 1.0f));
                }
                bases[key] = base;
                t.base = base.get();
                built->mod_base_refs.push_back(base);
            }
        };
        for (uint32_t t = 0; t < desc.track_count; ++t) {
            TrackNode& node = built->tracks[t];
            if (node.imod_count > 0)
                resolve_mods(node.instrument_plugin, desc.tracks[t].instrument_plugin->device_id, node.imod_begin,
                             node.imod_count, node.iplug_lane_begin, node.iplug_lane_count);
            for (uint32_t i = 0; i < node.fx_count; ++i) {
                FxSlot& slot = built->fx[node.fx_begin + i];
                if (slot.mod_count > 0)
                    resolve_mods(slot.inst, slot.device_id, slot.mod_begin, slot.mod_count, slot.lane_begin, slot.lane_count);
            }
        }
        // Parameters that are no longer modulated go back to their base, on
        // the audio thread once the new graph runs.
        for (const auto& [key, base] : mod_bases_) {
            if (bases.count(key) != 0) continue;
            PluginInstance* const inst = plugins_.instance(key.first);
            double lo = 0.0, hi = 1.0;
            if (inst != nullptr && inst->as_builtin() == nullptr && inst->param_range(key.second, lo, hi))
                built->mod_restores.push_back(
                    {key.first, key.second, lo + static_cast<double>(base->load(std::memory_order_relaxed)) * (hi - lo)});
        }
        mod_bases_ = std::move(bases);
    }
    // Delay compensation: pending restarts and fresh latencies (main thread),
    // then the plan for the new graph.
    plugins_.service_latencies();
    plan_pdc(*built);
    const uint64_t seq = install_graph(std::move(built));
    plugins_.end_publish(seq);
    plugins_.collect(rt_active_seq_.load(std::memory_order_seq_cst));
    return SE_OK;
}

// v1..v4: no devices, so every plugin instance is retired.
int32_t Engine::publish_graph(std::unique_ptr<RenderGraph> built) {
    std::lock_guard plock(plugins_.mutex()); // every graph install happens under it (see replan_pdc_locked)
    plan_pdc(*built);
    const uint64_t seq = install_graph(std::move(built));
    plugins_.begin_publish();
    plugins_.end_publish(seq);
    plugins_.collect(rt_active_seq_.load(std::memory_order_seq_cst));
    return SE_OK;
}

uint64_t Engine::install_graph(std::unique_ptr<RenderGraph> built) {
    std::shared_ptr<RenderGraph> next(std::move(built));
    std::lock_guard lock(control_mutex_);
    std::unordered_map<uint64_t, std::shared_ptr<Synth>> previous;
    if (current_)
        for (std::size_t i = 0; i < current_->tracks.size(); ++i)
            if (current_->synth_refs[i]) previous.emplace(current_->tracks[i].track_id, current_->synth_refs[i]);
    for (std::size_t i = 0; i < next->tracks.size(); ++i) {
        if (!next->tracks[i].instrument) continue;
        std::shared_ptr<Synth> synth;
        if (auto it = previous.find(next->tracks[i].track_id); it != previous.end()) {
            synth = std::move(it->second);
            previous.erase(it); // a duplicated track_id must not share one Synth
        } else {
            synth = std::make_shared<Synth>(sample_rate_);
        }
        next->synths[i] = synth.get();
        next->synth_refs[i] = std::move(synth);
    }

    // MIDI input state: kept by track_id like the synths, unless the track
    // switched between the built-in Synth and an instrument device (then a
    // fresh state: the old one may still be in use by the RT side).
    {
        std::unordered_map<uint64_t, std::shared_ptr<TrackMidi>> previous_midi;
        if (current_ && current_->midi_refs.size() == current_->tracks.size())
            for (std::size_t i = 0; i < current_->tracks.size(); ++i)
                if (current_->midi_refs[i]) previous_midi.emplace(current_->tracks[i].track_id, current_->midi_refs[i]);
        next->midi_refs.resize(next->tracks.size());
        next->midi_tracks.assign(next->tracks.size(), nullptr);
        for (std::size_t i = 0; i < next->tracks.size(); ++i) {
            if (!next->tracks[i].instrument) continue;
            const bool wants_live = !next->tracks[i].has_instrument_plugin;
            std::shared_ptr<TrackMidi> m;
            if (auto it = previous_midi.find(next->tracks[i].track_id);
                it != previous_midi.end() && (it->second->live != nullptr) == wants_live) {
                m = std::move(it->second);
                previous_midi.erase(it);
            } else {
                m = std::make_shared<TrackMidi>();
                if (wants_live) m->live = std::make_unique<LiveSynth>(sample_rate_);
            }
            next->midi_tracks[i] = m.get();
            next->midi_refs[i] = std::move(m);
        }
    }

    // Warp voices: keep the current graph's voice for the same slot (track,
    // slot, channels), so a clip keeps stretching across republishes.
    {
        std::map<std::tuple<uint64_t, uint32_t, uint32_t>, std::shared_ptr<WarpVoice>> previous_voices;
        if (current_)
            for (std::size_t i = 0; i < current_->warp_voice_keys.size(); ++i) {
                const WarpVoiceKey& k = current_->warp_voice_keys[i];
                if (current_->warp_voice_refs[i]) previous_voices.emplace(std::make_tuple(k.track_id, k.slot, k.channels), current_->warp_voice_refs[i]);
            }
        for (std::size_t i = 0; i < next->warp_voice_keys.size(); ++i) {
            const WarpVoiceKey& k = next->warp_voice_keys[i];
            std::shared_ptr<WarpVoice> voice;
            if (auto it = previous_voices.find(std::make_tuple(k.track_id, k.slot, k.channels)); it != previous_voices.end()) {
                voice = std::move(it->second);
                previous_voices.erase(it);
            } else {
                voice = std::make_shared<WarpVoice>(sample_rate_, k.channels, max_block_);
            }
            next->warp_voices[i] = voice.get();
            next->warp_voice_refs[i] = std::move(voice);
        }
    }

    // Delay lines: keep the current graph's line for the same track / device
    // (so delays continue across republishes) when it is big enough;
    // allocate the others here, never on the RT side.
    next->line_refs.clear();
    next->lines.clear();
    if (!next->pdc.need.empty()) {
        // kind (0 main, 1 source, 2 bypass, 3 metronome, 4 send, 5 output edge), ids (sends and
        // output edges also by destination, so a line never plays into a new destination)
        using Key = std::tuple<int, uint64_t, uint64_t>;
        const auto key_of = [](const RenderGraph& g, std::size_t i) -> Key {
            const std::size_t t = g.tracks.size();
            const std::size_t f = g.fx.size();
            if (i < t) return {0, g.tracks[i].track_id, 0};
            if (i < 2 * t) return {1, g.tracks[i - t].track_id, 0};
            if (i < 2 * t + f) return {2, g.fx[i - 2 * t].device_id, 0};
            if (i == 2 * t + f) return {3, 0, 0};
            const std::size_t s = i - (2 * t + f + 1);
            if (s < g.sends.size()) {
                // the sending track: the send's position in its track's range
                std::size_t from = 0;
                while (from + 1 < t && g.tracks[from].send_begin + g.tracks[from].send_count <= s) ++from;
                return {4, g.tracks[from].track_id, g.sends[s].return_id};
            }
            // the output's destination: the return whose bus it plays into (0: the master)
            const TrackNode& from = g.tracks[s - g.sends.size()];
            uint64_t to = 0;
            for (const TrackNode& r : g.tracks)
                if (from.out_bus >= 0 && r.is_return && r.bus == from.out_bus) to = r.track_id;
            return {5, from.track_id, to};
        };
        std::map<Key, std::shared_ptr<DelayLine>> previous_lines;
        if (current_)
            for (std::size_t i = 0; i < current_->line_refs.size(); ++i)
                if (current_->line_refs[i]) previous_lines.emplace(key_of(*current_, i), current_->line_refs[i]);
        const std::size_t metro = next->pdc.metro_index(next->tracks.size(), next->fx.size());
        next->line_refs.resize(next->pdc.need.size());
        next->lines.assign(next->pdc.need.size(), nullptr);
        for (std::size_t i = 0; i < next->pdc.need.size(); ++i) {
            if (next->pdc.need[i] == 0) continue;
            const uint32_t channels = i == metro ? 1 : 2;
            const uint32_t cap = DelayLine::capacity_for(next->pdc.need[i], max_block_);
            std::shared_ptr<DelayLine> line;
            if (auto it = previous_lines.find(key_of(*next, i));
                it != previous_lines.end() && it->second->capacity() >= cap && it->second->channels() == channels) {
                line = std::move(it->second);
                previous_lines.erase(it);
            } else {
                line = std::make_shared<DelayLine>(cap, channels);
            }
            next->lines[i] = line.get();
            next->line_refs[i] = std::move(line);
        }
    }
    pdc_total_.store(next->pdc.total, std::memory_order_relaxed);

    const uint64_t seq = next_seq_++;
    next->seq = seq;
    live_graph_.store(next.get(), std::memory_order_seq_cst);
    if (current_) retired_.push_back(std::move(current_));
    current_ = std::move(next);
    reclaim_locked();
    warp_renderer_.evict(*current_);
    return seq;
}

// Reclamation protocol (single RT reader, wait-free on the RT side):
//   RT block start:  rt_active_seq_ = kBusy;  g = live_graph_;  rt_active_seq_ = g->seq
//   RT block end:    rt_active_seq_ = kIdle
// A retired graph R (already swapped out of live_graph_) may be freed when
//   active == kIdle            -> RT holds nothing; the next load sees the new graph
//   active != kBusy && active > R.seq -> RT holds a newer graph and can never
//                                 reach R again (it only ever loads live_graph_)
// All five operations are seq_cst, so if RT loaded R before the swap, its
// kBusy store precedes our read of rt_active_seq_ and we observe kBusy, R.seq
// (or older), or kIdle only after RT finished with R.
void Engine::reclaim_locked() {
    if (retired_.empty()) return;
    const uint64_t active = rt_active_seq_.load(std::memory_order_seq_cst);
    if (active == kBusy) return;
    std::erase_if(retired_, [active](const std::shared_ptr<RenderGraph>& g) {
        return active == kIdle || active > g->seq;
    });
}

void Engine::collect_garbage() {
    std::lock_guard lock(control_mutex_);
    reclaim_locked();
}

std::size_t Engine::retired_graph_count() const {
    std::lock_guard lock(control_mutex_);
    return retired_.size();
}

// ============================================================ transport

// ---- RT ownership and synchronous (offline) command application
//
// rt_owner_ says who may touch rt_* state and pop the queue: the device
// callback (kOwnerDevice), an se_render_offline call (kOwnerOffline), or a
// non-RT thread applying commands inline (kOwnerControl). Ownership changes
// only through seq_cst RMW/stores, which also order the plain rt_* fields
// between successive owners.
//
// Every command is first pushed onto the SPSC queue, then the producer tries
// to claim the path (None -> Control). If it succeeds it drains the queue
// itself (so the command applies before push_command returns) and releases.
// If the device or an offline render owns the path, the command stays queued
// and that owner applies it at its next block start or when it releases.
//
// Lost-wakeup freedom: an owner releasing the path does
//     owner.store(None); fence; if (!queue.empty()) claim + drain
// and a producer does
//     queue.push(cmd);   fence; claim (CAS None -> Control) + drain
// Both fences are seq_cst, so (store-buffering argument) either the
// producer's CAS observes None and drains, or the releaser's empty() check
// observes the push and drains. A command can therefore never be stranded in
// the queue with nobody owning the path. kOwnerControl is held only for the
// few loads/stores of a drain; other claimants yield until it is gone.
int32_t Engine::midi_open(const std::vector<std::string>& ids, std::string& err) {
    // Closing slots are freed by whoever renders; with nobody rendering, take
    // the RT role briefly and free them here.
    return midi_->open_only(ids, err, [this] {
        if (acquire_rt_owner(kOwnerControl) != kOwnerNone) return false;
        midi_->reclaim_closing();
        release_rt_owner();
        return true;
    });
}

int Engine::acquire_rt_owner(int who) noexcept {
    for (;;) {
        int expected = kOwnerNone;
        if (rt_owner_.compare_exchange_strong(expected, who, std::memory_order_seq_cst)) return kOwnerNone;
        if (expected != kOwnerControl) return expected;
        std::this_thread::yield();
    }
}

void Engine::release_rt_owner() noexcept {
    for (;;) {
        rt_owner_.store(kOwnerNone, std::memory_order_seq_cst);
        std::atomic_thread_fence(std::memory_order_seq_cst);
        if (commands_.empty()) return;
        int expected = kOwnerNone;
        // If somebody else claimed the path in the meantime, they drain.
        if (!rt_owner_.compare_exchange_strong(expected, kOwnerControl, std::memory_order_seq_cst)) return;
        drain_commands();
        publish_transport();
    }
}

int32_t Engine::push_command(const TransportCommand& cmd) {
    std::lock_guard lock(command_mutex_); // producers stay serialised (SPSC)
    if (!commands_.push(cmd)) return SE_ERR_STATE;
    std::atomic_thread_fence(std::memory_order_seq_cst);
    if (acquire_rt_owner(kOwnerControl) == kOwnerNone) {
        drain_commands();
        publish_transport();
        release_rt_owner();
    }
    return SE_OK;
}

void Engine::apply_command(const TransportCommand& cmd) noexcept {
    switch (cmd.type) {
    case TransportCommand::Type::Play: rt_playing_ = true; break;
    case TransportCommand::Type::Pause:
        rt_playing_ = false;
        rt_voice_count_ = 0;
        ++rt_synth_gen_;
        break;
    case TransportCommand::Type::Stop:
        rt_playing_ = false;
        rt_position_ = 0;
        rt_voice_count_ = 0;
        ++rt_synth_gen_;
        ++rt_pdc_gen_; // delay lines flush (pause lets them ring out)
        break;
    case TransportCommand::Type::Seek: // click tails ring on; synth voices fade
        rt_position_ = cmd.frame;
        ++rt_synth_gen_;
        ++rt_pdc_gen_;
        break;
    case TransportCommand::Type::SetLoop:
        rt_loop_enabled_ = cmd.enabled;
        if (cmd.enabled) {
            rt_loop_start_ = cmd.frame;
            rt_loop_end_ = cmd.frame2;
        }
        break;
    case TransportCommand::Type::PreviewPlay:
        if (rt_pv_[0].asset != nullptr) {
            rt_pv_[1] = rt_pv_[0]; // the old voice fades out (a fading one is cut)
            rt_pv_[1].step = -1.0f / kPreviewFadeFrames;
        }
        rt_pv_[0] = PreviewVoice{cmd.asset, cmd.seq, static_cast<double>(cmd.frame), 0.0f, 1.0f / kPreviewFadeFrames};
        rt_pv_applied_ = cmd.seq;
        rt_pv_gain_ = cmd.gain;
        rt_pv_rate_ = cmd.rate;
        rt_pv_solo_ = cmd.enabled;
        break;
    case TransportCommand::Type::PreviewStop:
        if (rt_pv_[0].asset != nullptr) {
            rt_pv_[1] = rt_pv_[0];
            rt_pv_[1].step = -1.0f / kPreviewFadeFrames;
            rt_pv_[0] = PreviewVoice{};
        }
        break;
    case TransportCommand::Type::PreviewSet:
        rt_pv_gain_ = cmd.gain;
        rt_pv_rate_ = cmd.rate;
        rt_pv_solo_ = cmd.enabled;
        break;
    case TransportCommand::Type::SetMetronome:
        if (std::isfinite(cmd.frames_per_beat) && cmd.frames_per_beat >= 1.0) rt_tempo_fpb_ = cmd.frames_per_beat;
        rt_metro_enabled_ = cmd.enabled;
        if (cmd.enabled) {
            rt_metro_gain_ = cmd.gain;
            rt_metro_fpb_ = cmd.frames_per_beat;
            rt_metro_bpb_ = cmd.beats_per_bar;
        } else {
            rt_voice_count_ = 0;
        }
        break;
    }
}

void Engine::drain_commands() noexcept {
    TransportCommand cmd;
    while (commands_.pop(cmd)) apply_command(cmd);
}

void Engine::publish_transport() noexcept {
    pub_position_.store(rt_position_, std::memory_order_release);
    pub_playing_.store(rt_playing_, std::memory_order_release);
}

void Engine::transport_get(se_transport_info* out) const noexcept {
    *out = se_transport_info{};
    out->position = pub_position_.load(std::memory_order_acquire);
    out->playing = pub_playing_.load(std::memory_order_acquire) ? 1 : 0;
}

// ============================================================ meters / stats

int32_t Engine::meters_read(se_meter* out, std::size_t capacity, std::size_t* out_count) {
    std::size_t n = 0;
    if (capacity > 0) {
        out[0].track_id = 0;
        out[0].peak_l = master_peak_l_.exchange(0.0f, std::memory_order_relaxed);
        out[0].peak_r = master_peak_r_.exchange(0.0f, std::memory_order_relaxed);
        n = 1;
    }
    std::lock_guard lock(control_mutex_); // keeps current_ alive while reading
    if (current_) {
        for (std::size_t i = 0; i < current_->meter_count && n < capacity; ++i, ++n) {
            TrackMeter& m = current_->meters[i];
            out[n].track_id = m.track_id;
            out[n].peak_l = m.peak_l.exchange(0.0f, std::memory_order_relaxed);
            out[n].peak_r = m.peak_r.exchange(0.0f, std::memory_order_relaxed);
        }
    }
    if (out_count != nullptr) *out_count = n;
    return SE_OK;
}

int32_t Engine::meters_peek(se_meter* out, std::size_t capacity, std::size_t* out_count) const {
    auto window_max = [](const std::atomic<float>* ring) {
        float m = 0.0f;
        for (uint32_t i = 0; i < kPeekBuckets; ++i) m = std::max(m, ring[i].load(std::memory_order_relaxed));
        return m;
    };
    std::size_t n = 0;
    if (capacity > 0) {
        out[0].track_id = 0;
        out[0].peak_l = window_max(master_win_l_);
        out[0].peak_r = window_max(master_win_r_);
        n = 1;
    }
    std::lock_guard lock(control_mutex_); // keeps current_ alive while reading
    if (current_) {
        for (std::size_t i = 0; i < current_->meter_count && n < capacity; ++i, ++n) {
            const TrackMeter& m = current_->meters[i];
            out[n].track_id = m.track_id;
            out[n].peak_l = window_max(m.win_l);
            out[n].peak_r = window_max(m.win_r);
        }
    }
    if (out_count != nullptr) *out_count = n;
    return SE_OK;
}

void Engine::stats_get(se_stats* out) const noexcept {
    const uint64_t calls = stat_callbacks_.load(std::memory_order_relaxed);
    out->callbacks = calls;
    out->xruns_or_overloads = stat_overloads_.load(std::memory_order_relaxed);
    out->last_callback_us = static_cast<double>(stat_last_ns_.load(std::memory_order_relaxed)) / 1000.0;
    out->max_callback_us = static_cast<double>(stat_max_ns_.load(std::memory_order_relaxed)) / 1000.0;
    out->avg_callback_us =
        calls ? static_cast<double>(stat_total_ns_.load(std::memory_order_relaxed)) / 1000.0 / static_cast<double>(calls)
              : 0.0;
    out->block_deadline_us = static_cast<double>(stat_deadline_ns_.load(std::memory_order_relaxed)) / 1000.0;
}

void Engine::stats_reset() noexcept {
    stat_callbacks_.store(0, std::memory_order_relaxed);
    stat_overloads_.store(0, std::memory_order_relaxed);
    stat_last_ns_.store(0, std::memory_order_relaxed);
    stat_max_ns_.store(0, std::memory_order_relaxed);
    stat_total_ns_.store(0, std::memory_order_relaxed);
    stat_deadline_ns_.store(0, std::memory_order_relaxed);
}

// ============================================================ RT process path

template <class Sink>
void Engine::process(uint32_t frames, const RtInput& input, int64_t rec_latency, Sink&& sink) noexcept {
    const uint64_t t0 = now_ns();
    ScopedFlushDenormals ftz;

    // 1. Transport commands, applied at block start.
    drain_commands();
    if (!rt_playing_ || !rt_metro_enabled_) rt_voice_count_ = 0;

    // Recording session for this block (see record_stop() for the handshake).
    rec_in_use_.store(true, std::memory_order_seq_cst);
    Recorder* const rec = rec_live_.load(std::memory_order_seq_cst);
    const bool muted = input_muted_.load(std::memory_order_relaxed);
    const uint32_t in_count =
        std::min(input.channels, input.planar != nullptr ? kMaxDeviceInputChannels : kMaxInjectedInputChannels);
    const float* in_ptrs[kMaxDeviceInputChannels];

    // MIDI input: drained once per callback, delivered in its first segment.
    MidiInputHub& midi = *midi_;
    // The capture pass, sampled once: a new pass starts with no loop wrap seen.
    const uint32_t pass_now = midi.capture_pass();
    if (pass_now != rt_capture_pass_) {
        rt_capture_pass_ = pass_now;
        rt_midi_wrapped_ = false;
    }
    const uint32_t midi_count = midi.drain(rt_midi_.data(), MidiInputHub::kMaxBlockEvents);

    // 2. Acquire the published graph (see reclaim_locked()).
    rt_active_seq_.store(kBusy, std::memory_order_seq_cst);
    const RenderGraph* graph = live_graph_.load(std::memory_order_seq_cst);
    if (graph != nullptr) rt_active_seq_.store(graph->seq, std::memory_order_seq_cst);

    // Recording: what was played is captured at the position the performer
    // heard, i.e. output latency (device; none offline) plus delay
    // compensation earlier, folded back into the loop after a wrap.
    const uint32_t capture_pass = midi_count > 0 && rt_playing_ ? pass_now : 0;
    if (capture_pass != 0) {
        const int64_t lat = (rt_owner_.load(std::memory_order_relaxed) == kOwnerOffline
                                 ? 0
                                 : device_out_latency_.load(std::memory_order_relaxed)) +
                            (graph != nullptr && !graph->lines.empty() ? graph->pdc.total : 0);
        const int64_t frame =
            midi_capture_frame(rt_position_, lat, rt_midi_wrapped_, rt_loop_enabled_, rt_loop_start_, rt_loop_end_);
        for (uint32_t i = 0; i < midi_count; ++i) midi.capture(rt_midi_[i], frame, capture_pass);
    }

    // 3. Render in segments of at most max_block frames, additionally split
    //    at the loop end so the wrap is sample-accurate. render_block() is
    //    stateless (clips are addressed by timeline frame), so the segment
    //    after a wrap renders the loop start with correct clip offsets.
    float peak_l = 0.0f, peak_r = 0.0f;
    for (uint32_t done = 0; done < frames;) {
        uint32_t n = std::min(frames - done, max_block_);
        const bool looping = rt_playing_ && rt_loop_enabled_ && rt_position_ < rt_loop_end_;
        if (looping) n = static_cast<uint32_t>(std::min<int64_t>(n, rt_loop_end_ - rt_position_));
        const uint32_t slot = advance_peek_window(n, graph);
        // Input channels for this segment: device buffers (offset), injected
        // input deinterleaved into scratch, or zeros when input is muted.
        for (uint32_t c = 0; c < in_count; ++c) {
            if (muted) {
                in_ptrs[c] = live_ctx_.zeros();
            } else if (input.planar != nullptr) {
                in_ptrs[c] = input.planar[c] + done;
            } else {
                float* d = in_scratch_.data() + static_cast<std::size_t>(c) * max_block_;
                const float* s = input.interleaved + static_cast<std::size_t>(done) * input.channels + c;
                for (uint32_t i = 0; i < n; ++i) d[i] = s[static_cast<std::size_t>(i) * input.channels];
                in_ptrs[c] = d;
            }
        }
        const InputView iv{in_ptrs, in_count};
        RtTransport transport;
        transport.position = rt_position_;
        transport.playing = rt_playing_;
        transport.sample_rate = sample_rate_;
        transport.tempo_bpm = rt_tempo_fpb_ > 0.0 ? 60.0 * sample_rate_ / rt_tempo_fpb_ : 120.0;
        DelayLine* const* const lines = graph != nullptr && !graph->lines.empty() ? graph->lines.data() : nullptr;
        const MidiBlock midi_block{done == 0 ? rt_midi_.data() : nullptr, done == 0 ? midi_count : 0u, midi.focus()};
        const InstrumentInputs inst{graph != nullptr ? graph->synths.data() : nullptr,
                                    rt_synth_gen_,
                                    &transport,
                                    lines,
                                    rt_pdc_gen_,
                                    &rt_history_,
                                    graph != nullptr && !graph->warp_voices.empty() ? graph->warp_voices.data() : nullptr,
                                    &midi_block};
        BlockPeaks p = render_block(graph, rt_position_, rt_playing_, n, live_ctx_, true, slot, inst, iv);
        if (mix_preview(live_ctx_.master_l(), live_ctx_.master_r(), n)) {
            p.l = kernel_peak(live_ctx_.master_l(), n);
            p.r = kernel_peak(live_ctx_.master_r(), n);
        }
        rt_history_.append(rt_position_, n, rt_playing_);
        // The performer hears the mix L (delay compensation) + output latency
        // late, so takes are placed that much earlier too.
        const int64_t pdc_total = lines != nullptr ? graph->pdc.total : 0;
        if (rec != nullptr && rt_playing_)
            rec->capture(in_ptrs, in_count, live_ctx_.zeros(), rt_position_, rec_latency + pdc_total, n);
        peak_l = std::max(peak_l, p.l);
        peak_r = std::max(peak_r, p.r);
        atomic_store_max(master_win_l_[slot], p.l);
        atomic_store_max(master_win_r_[slot], p.r);
        // Metronome: after master gain and after metering, never in export.
        // With delay compensation it is delayed by L like the music.
        if (pdc_total == 0) {
            if (rt_playing_ && rt_metro_enabled_)
                mix_metronome(live_ctx_.master_l(), live_ctx_.master_r(), rt_position_, n);
        } else {
            DelayLine* const ml = lines[graph->pdc.metro_index(graph->tracks.size(), graph->fx.size())];
            const auto md = static_cast<uint32_t>(pdc_total);
            const bool clicking = rt_playing_ && rt_metro_enabled_;
            if (clicking || !ml->idle(md, rt_pdc_gen_)) {
                float* const c = click_scratch_.data();
                std::memset(c, 0, sizeof(float) * n);
                if (clicking) mix_metronome(c, nullptr, rt_position_, n);
                ml->process_mono(c, n, md, rt_pdc_gen_);
                kernel_add_scaled(live_ctx_.master_l(), c, 1.0f, n);
                kernel_add_scaled(live_ctx_.master_r(), c, 1.0f, n);
            } else {
                ml->discard(n, rt_pdc_gen_);
            }
        }
        sink(live_ctx_.master_l(), live_ctx_.master_r(), done, n);
        if (rt_playing_) {
            rt_position_ += n;
            if (looping && rt_position_ == rt_loop_end_) {
                rt_position_ = rt_loop_start_;
                if (rec != nullptr) rec->on_loop_wrap(rt_loop_start_); // takes continue linearly
                rt_midi_wrapped_ = true;
                if (rt_capture_pass_ != 0) midi.capture_note_wrap(rt_capture_pass_);
                ++rt_synth_gen_; // sounding notes fade; notes at loop start retrigger
            }
        }
        done += n;
    }

    // 4. Release the graph, publish state.
    {
        const PreviewVoice& v = rt_pv_[0];
        pub_pv_asset_.store(v.asset != nullptr ? v.asset->id : 0, std::memory_order_relaxed);
        pub_pv_pos_.store(static_cast<int64_t>(v.pos), std::memory_order_relaxed);
        pub_pv_frames_.store(v.asset != nullptr ? static_cast<int64_t>(v.asset->frames) : 0, std::memory_order_relaxed);
        pub_pv_solo_.store(rt_pv_solo_, std::memory_order_relaxed);
        uint64_t floor = rt_pv_applied_ + 1;
        for (const PreviewVoice& w : rt_pv_)
            if (w.asset != nullptr) floor = std::min(floor, w.seq);
        rt_preview_floor_.store(floor, std::memory_order_release);
    }
    rt_active_seq_.store(kIdle, std::memory_order_seq_cst);
    rec_in_use_.store(false, std::memory_order_release);
    publish_transport();
    atomic_store_max(master_peak_l_, peak_l);
    atomic_store_max(master_peak_r_, peak_r);

    // 5. Timing stats.
    const uint64_t elapsed = now_ns() - t0;
    const auto deadline = static_cast<uint64_t>(static_cast<double>(frames) * 1e9 / sample_rate_);
    stat_callbacks_.fetch_add(1, std::memory_order_relaxed);
    stat_last_ns_.store(elapsed, std::memory_order_relaxed);
    stat_total_ns_.fetch_add(elapsed, std::memory_order_relaxed);
    stat_deadline_ns_.store(deadline, std::memory_order_relaxed);
    if (elapsed > stat_max_ns_.load(std::memory_order_relaxed)) stat_max_ns_.store(elapsed, std::memory_order_relaxed);
    if (elapsed > deadline) stat_overloads_.fetch_add(1, std::memory_order_relaxed);
}

// Moves the se_meters_peek window forward by `frames` rendered frames and
// returns the bucket the segment's peak belongs to (the one it ends in).
// Buckets entered are zeroed for the master and the live graph's tracks.
uint32_t Engine::advance_peek_window(uint32_t frames, const RenderGraph* graph) noexcept {
    const uint64_t total = rt_peek_fill_ + frames;
    uint64_t steps = total / peek_bucket_frames_;
    rt_peek_fill_ = total % peek_bucket_frames_;
    if (steps > kPeekBuckets) steps = kPeekBuckets;
    for (uint64_t s = 0; s < steps; ++s) {
        rt_peek_slot_ = (rt_peek_slot_ + 1) % kPeekBuckets;
        const uint32_t k = rt_peek_slot_;
        master_win_l_[k].store(0.0f, std::memory_order_relaxed);
        master_win_r_[k].store(0.0f, std::memory_order_relaxed);
        if (graph != nullptr)
            for (std::size_t t = 0; t < graph->meter_count; ++t) {
                graph->meters[t].win_l[k].store(0.0f, std::memory_order_relaxed);
                graph->meters[t].win_r[k].store(0.0f, std::memory_order_relaxed);
            }
    }
    return rt_peek_slot_;
}

// Mixes `buf` from its start into l/r at `offset`; keeps a voice for the
// tail if it does not fit in this segment (stealing the oldest if full).
void Engine::start_click(const float* buf, uint32_t offset, float* l, float* r, uint32_t frames) noexcept {
    const uint32_t m = std::min(click_len_, frames - offset);
    kernel_add_scaled(l + offset, buf, rt_metro_gain_, m);
    if (r != nullptr) kernel_add_scaled(r + offset, buf, rt_metro_gain_, m);
    if (m >= click_len_) return;
    std::size_t slot = rt_voice_count_;
    if (slot == kMaxClickVoices) {
        slot = 0;
        for (std::size_t i = 1; i < rt_voice_count_; ++i)
            if (rt_voices_[i].pos > rt_voices_[slot].pos) slot = i;
    } else {
        ++rt_voice_count_;
    }
    rt_voices_[slot] = {buf, m};
}

// Adds metronome clicks for timeline frames [position, position + frames).
// Voices carry tails across segment boundaries in playback time (so a click
// rings out over a block edge or a loop wrap); onsets are taken from the
// timeline beat grid, so only beats the playhead actually crosses sound.
void Engine::mix_metronome(float* l, float* r, int64_t position, uint32_t frames) noexcept {
    const float g = rt_metro_gain_;
    for (std::size_t i = 0; i < rt_voice_count_;) {
        ClickVoice& v = rt_voices_[i];
        const uint32_t m = std::min(click_len_ - v.pos, frames);
        kernel_add_scaled(l, v.buf + v.pos, g, m);
        if (r != nullptr) kernel_add_scaled(r, v.buf + v.pos, g, m);
        v.pos += m;
        if (v.pos >= click_len_)
            rt_voices_[i] = rt_voices_[--rt_voice_count_];
        else
            ++i;
    }
    const double fpb = rt_metro_fpb_;
    const auto beat_frame = [fpb](int64_t k) { return std::llround(static_cast<double>(k) * fpb); };
    const int64_t end = position + frames;
    int64_t k = static_cast<int64_t>(std::floor(static_cast<double>(position) / fpb)) - 1;
    while (beat_frame(k) < position) ++k;
    for (int64_t f = beat_frame(k); f < end; f = beat_frame(++k)) {
        const bool downbeat = floor_mod(k, rt_metro_bpb_) == 0;
        start_click(downbeat ? click_down_.data() : click_beat_.data(), static_cast<uint32_t>(f - position), l, r,
                    frames);
    }
}

void Engine::render(float* const* channels, uint32_t num_channels, const float* const* in, uint32_t num_in,
                    uint32_t frames) noexcept {
    const RtInput input{in, nullptr, in != nullptr ? num_in : 0};
    process(frames, input, device_rec_latency_.load(std::memory_order_relaxed),
            [&](const float* l, const float* r, uint32_t offset, uint32_t n) {
        if (num_channels == 0) return;
        if (num_channels == 1) {
            float* dst = channels[0] + offset;
            for (uint32_t i = 0; i < n; ++i) dst[i] = 0.5f * (l[i] + r[i]);
            return;
        }
        std::memcpy(channels[0] + offset, l, sizeof(float) * n);
        std::memcpy(channels[1] + offset, r, sizeof(float) * n);
        for (uint32_t c = 2; c < num_channels; ++c) std::memset(channels[c] + offset, 0, sizeof(float) * n);
    });
}

int32_t Engine::render_offline(float* out_interleaved, uint32_t frames, std::string& err) {
    return render_offline_impl(RtInput{}, out_interleaved, frames, err);
}

int32_t Engine::render_offline_with_input(const float* input, uint32_t input_channels, float* out_interleaved,
                                          uint32_t frames, std::string& err) {
    if (input_channels == 0 || input_channels > kMaxInjectedInputChannels) {
        err = "input_channels must be 1.." + std::to_string(kMaxInjectedInputChannels);
        return SE_ERR_INVALID_ARG;
    }
    return render_offline_impl(RtInput{nullptr, input, input_channels}, out_interleaved, frames, err);
}

int32_t Engine::render_offline_impl(const RtInput& input, float* out_interleaved, uint32_t frames, std::string& err) {
    if (frames == 0) return SE_OK;
    const int conflict = acquire_rt_owner(kOwnerOffline);
    if (conflict != kOwnerNone) {
        err = conflict == kOwnerDevice ? "cannot render offline while the device is running"
                                       : "another offline render is in progress";
        return SE_ERR_STATE;
    }
    // Injected input is not delayed by a device, so takes get 0 compensation.
    process(frames, input, 0, [out_interleaved](const float* l, const float* r, uint32_t offset, uint32_t n) {
        float* dst = out_interleaved + static_cast<std::size_t>(offset) * 2;
        for (uint32_t i = 0; i < n; ++i) {
            dst[2 * i] = l[i];
            dst[2 * i + 1] = r[i];
        }
    });
    release_rt_owner(); // also applies commands queued during the render
    return SE_OK;
}

// ============================================================ preview

bool Engine::mix_preview(float* l, float* r, uint32_t frames) noexcept {
    if (rt_pv_[0].asset == nullptr && rt_pv_[1].asset == nullptr) {
        rt_pv_gain_cur_ = rt_pv_gain_;
        return false;
    }
    if (rt_pv_solo_ && rt_pv_[0].asset != nullptr) {
        std::memset(l, 0, sizeof(float) * frames);
        std::memset(r, 0, sizeof(float) * frames);
    }
    const float g0 = rt_pv_gain_cur_;
    const float dg = (rt_pv_gain_ - g0) / static_cast<float>(frames);
    const double rate = static_cast<double>(rt_pv_rate_);
    float pk_l = 0.0f, pk_r = 0.0f;
    for (PreviewVoice& v : rt_pv_) {
        if (v.asset == nullptr) continue;
        const Asset& a = *v.asset;
        const float* sl = a.channel(0);
        const float* sr = a.channel(a.channels > 1 ? 1 : 0);
        const double last = a.frames > 0 ? static_cast<double>(a.frames - 1) : 0.0;
        for (uint32_t i = 0; i < frames; ++i) {
            if (v.pos >= last) {
                v.asset = nullptr;
                break;
            }
            const auto k = static_cast<uint64_t>(v.pos);
            const float t = static_cast<float>(v.pos - static_cast<double>(k));
            const float g = (g0 + dg * static_cast<float>(i)) * v.fade;
            const float yl = g * (sl[k] + t * (sl[k + 1] - sl[k]));
            const float yr = g * (sr[k] + t * (sr[k + 1] - sr[k]));
            l[i] += yl;
            r[i] += yr;
            pk_l = std::max(pk_l, std::fabs(yl));
            pk_r = std::max(pk_r, std::fabs(yr));
            v.pos += rate;
            v.fade = std::min(1.0f, v.fade + v.step);
            if (v.fade <= 0.0f) {
                v.asset = nullptr;
                break;
            }
        }
    }
    rt_pv_gain_cur_ = rt_pv_gain_;
    atomic_store_max(pv_peak_l_, pk_l);
    atomic_store_max(pv_peak_r_, pk_r);
    return true;
}

void Engine::reclaim_previews_locked() {
    const uint64_t floor = rt_preview_floor_.load(std::memory_order_acquire);
    std::erase_if(preview_held_, [floor](const auto& h) { return h.first < floor; });
}

int32_t Engine::preview_play(uint64_t asset_id, int64_t start_frame, float gain, float rate, bool solo) {
    AssetRef asset;
    {
        std::lock_guard lock(assets_mutex_);
        auto it = assets_.find(asset_id);
        if (it == assets_.end()) return SE_ERR_NOT_FOUND;
        asset = it->second;
    }
    std::lock_guard lock(preview_mutex_);
    reclaim_previews_locked();
    TransportCommand cmd;
    cmd.type = TransportCommand::Type::PreviewPlay;
    cmd.asset = asset.get();
    cmd.seq = ++preview_seq_;
    cmd.frame = std::clamp<int64_t>(start_frame, 0, static_cast<int64_t>(asset->frames));
    cmd.gain = gain;
    cmd.rate = rate;
    cmd.enabled = solo;
    preview_held_.emplace_back(cmd.seq, std::move(asset));
    return push_command(cmd);
}

int32_t Engine::preview_stop() {
    std::lock_guard lock(preview_mutex_);
    reclaim_previews_locked();
    if (!preview_notes_.empty()) {
        std::lock_guard pl(plugins_.mutex());
        for (const auto& [device, pitch] : preview_notes_)
            if (PluginInstance* inst = plugins_.instance(device)) inst->push_note(pitch, 0);
        preview_notes_.clear();
        pub_pv_notes_.store(0, std::memory_order_relaxed);
    }
    TransportCommand cmd;
    cmd.type = TransportCommand::Type::PreviewStop;
    return push_command(cmd);
}

int32_t Engine::preview_set(float gain, float rate, bool solo) {
    TransportCommand cmd;
    cmd.type = TransportCommand::Type::PreviewSet;
    cmd.gain = gain;
    cmd.rate = rate;
    cmd.enabled = solo;
    return push_command(cmd);
}

int32_t Engine::preview_note(uint64_t device_id, uint8_t pitch, uint8_t velocity) {
    if (pitch > 127 || velocity > 127) return SE_ERR_INVALID_ARG;
    std::lock_guard lock(preview_mutex_);
    {
        std::lock_guard pl(plugins_.mutex());
        PluginInstance* inst = plugins_.instance(device_id);
        if (inst == nullptr || !inst->plays_notes()) return SE_ERR_NOT_FOUND;
        if (!inst->push_note(pitch, velocity)) return SE_ERR_STATE;
    }
    const std::pair<uint64_t, uint8_t> key{device_id, pitch};
    const auto held = std::find(preview_notes_.begin(), preview_notes_.end(), key);
    if (velocity > 0 && held == preview_notes_.end()) preview_notes_.push_back(key);
    if (velocity == 0 && held != preview_notes_.end()) preview_notes_.erase(held);
    pub_pv_notes_.store(static_cast<uint32_t>(preview_notes_.size()), std::memory_order_relaxed);
    return SE_OK;
}

void Engine::preview_get(se_preview_info* out) noexcept {
    *out = se_preview_info{};
    out->asset_id = pub_pv_asset_.load(std::memory_order_relaxed);
    out->playing = out->asset_id != 0 ? 1 : 0;
    out->position = pub_pv_pos_.load(std::memory_order_relaxed);
    out->frames = pub_pv_frames_.load(std::memory_order_relaxed);
    out->notes = static_cast<uint8_t>(std::min<uint32_t>(pub_pv_notes_.load(std::memory_order_relaxed), 255));
    out->peak_l = pv_peak_l_.exchange(0.0f, std::memory_order_relaxed);
    out->peak_r = pv_peak_r_.exchange(0.0f, std::memory_order_relaxed);
    if (std::unique_lock lock(preview_mutex_, std::try_to_lock); lock.owns_lock()) reclaim_previews_locked();
}

// ============================================================ recording

int32_t Engine::record_start(const char* dir, std::string& err) {
    std::lock_guard lock(rec_mutex_);
    if (recorder_) {
        err = "already recording";
        return SE_ERR_STATE;
    }
    std::vector<RecordTrackSpec> specs;
    {
        std::lock_guard g(control_mutex_);
        if (current_)
            for (const TrackNode& t : current_->tracks)
                if (t.arm && t.input_mode != SE_INPUT_NONE)
                    specs.push_back({t.track_id, t.input_channel, t.input_mode == SE_INPUT_STEREO ? 2u : 1u});
    }
    if (specs.empty()) {
        err = "no armed track has an input";
        return SE_ERR_STATE;
    }
    if (device_running() && !device_input_open_.load(std::memory_order_acquire)) {
        err = "the audio device has no input open (configure an input device first)";
        return SE_ERR_STATE;
    }
    if (sample_rate_ != std::floor(sample_rate_)) {
        err = "engine sample rate is not representable in a WAV header";
        return SE_ERR_UNSUPPORTED_FORMAT;
    }
    const std::size_t ring = record_ring_frames_ != 0 ? record_ring_frames_
                                                      : static_cast<std::size_t>(2.0 * sample_rate_);
    auto rec = Recorder::create(dir, std::move(specs), sample_rate_, ring, err);
    if (!rec) return SE_ERR_IO;
    last_takes_.clear();
    last_stats_ = {};
    recorder_ = std::move(rec);
    rec_live_.store(recorder_.get(), std::memory_order_seq_cst); // the next block captures
    TransportCommand play;
    play.type = TransportCommand::Type::Play; // no-op if already playing
    push_command(play);
    return SE_OK;
}

// Stop handshake with process(): we clear rec_live_ and then wait until the
// RT side is not inside a block that loaded the old pointer. Both sides use
// seq_cst (RT: in_use = true; load ptr / us: store null; load in_use), so
// either the RT block sees null or we see in_use == true and wait for it.
// Only this non-RT thread waits; the audio thread never does.
int32_t Engine::record_stop(std::size_t* out_count, std::string& err) {
    std::lock_guard lock(rec_mutex_);
    if (!recorder_) {
        err = "not recording";
        return SE_ERR_STATE;
    }
    rec_live_.store(nullptr, std::memory_order_seq_cst);
    while (rec_in_use_.load(std::memory_order_seq_cst)) std::this_thread::sleep_for(std::chrono::microseconds(200));
    recorder_->finish(last_takes_);
    last_stats_ = recorder_->stats();
    recorder_.reset();
    if (out_count != nullptr) *out_count = last_takes_.size();
    return SE_OK;
}

int32_t Engine::record_get_take(std::size_t index, se_recorded_take* out) const {
    std::lock_guard lock(rec_mutex_);
    if (index >= last_takes_.size()) return SE_ERR_NOT_FOUND;
    const RecordedTakeInfo& t = last_takes_[index];
    *out = se_recorded_take{};
    out->track_id = t.track_id;
    out->start_frame = t.start_frame;
    out->frames = t.frames;
    out->channels = t.channels;
    out->sample_rate = t.sample_rate;
    copy_str(out->path, sizeof out->path, t.path);
    return SE_OK;
}

void Engine::record_state(bool& active, std::vector<LiveTakeInfo>& takes) const {
    std::lock_guard lock(rec_mutex_);
    active = recorder_ != nullptr;
    takes.clear();
    if (recorder_) recorder_->live_takes(takes);
}

std::size_t Engine::record_peaks(uint64_t track_id, uint32_t frames_per_peak, float* out, std::size_t capacity) const {
    std::lock_guard lock(rec_mutex_);
    return recorder_ ? recorder_->peaks(track_id, frames_per_peak, out, capacity) : 0;
}

RecordStats Engine::record_stats() const {
    std::lock_guard lock(rec_mutex_);
    return recorder_ ? recorder_->stats() : last_stats_;
}

void Engine::set_record_writer_stalled(bool stalled) {
    std::lock_guard lock(rec_mutex_);
    if (recorder_) recorder_->set_writer_stalled(stalled);
}

// Export renders with a private context and transport so it never disturbs
// live playback, meters or stats. It shares render_block() with the RT path.
// Plugin instances are live, shared state: a session with plugins is
// exported with the RT ownership held (device stopped, no offline render),
// using the live instances; the export marks itself as the RT reader of the
// graph so retired instances it uses are not destroyed under it.
int32_t Engine::export_wav(const char* path, int64_t start_frame, int64_t end_frame, std::string& err) {
    if (end_frame <= start_frame) {
        err = "export range is empty";
        return SE_ERR_INVALID_ARG;
    }
    std::shared_ptr<RenderGraph> graph;
    {
        std::lock_guard lock(control_mutex_);
        graph = current_; // keeps the graph (and its assets) alive for the export
    }
    if (sample_rate_ != std::floor(sample_rate_) || sample_rate_ > 4294967295.0) {
        err = "engine sample rate is not representable in a WAV header";
        return SE_ERR_UNSUPPORTED_FORMAT;
    }

    const bool plugins = graph && graph->has_plugins;
    if (plugins) {
        const int conflict = acquire_rt_owner(kOwnerOffline);
        if (conflict != kOwnerNone) {
            err = conflict == kOwnerDevice ? "stop the audio device to export a session with plugins"
                                           : "an offline render is in progress";
            return SE_ERR_STATE;
        }
        std::lock_guard lock(control_mutex_);
        graph = current_; // the newest graph; pinned for the RT protocol below
        rt_active_seq_.store(graph->seq, std::memory_order_seq_cst);
        ++rt_synth_gen_; // instrument plugins: all notes off before and after
    }
    struct Release {
        Engine* e;
        bool on;
        ~Release() {
            if (!on) return;
            ++e->rt_synth_gen_;
            e->rt_active_seq_.store(kIdle, std::memory_order_seq_cst);
            e->release_rt_owner();
        }
    } release{this, plugins};

    WavFloatWriter writer;
    int32_t rc = writer.open(path, 2, static_cast<uint32_t>(sample_rate_), err);
    if (rc != SE_OK) return rc;

    RenderContext ctx(max_block_);
    std::vector<float> interleaved(static_cast<std::size_t>(max_block_) * 2);
    // Private synth state: export never touches the live voices.
    std::vector<std::unique_ptr<Synth>> synths;
    std::vector<Synth*> synth_ptrs;
    if (graph) {
        synth_ptrs.assign(graph->tracks.size(), nullptr);
        for (std::size_t i = 0; i < graph->tracks.size(); ++i)
            if (graph->tracks[i].instrument) {
                synths.push_back(std::make_unique<Synth>(sample_rate_));
                synth_ptrs[i] = synths.back().get();
            }
    }
    // Delay compensation: private delay lines (live ones stay untouched); the
    // mix comes out L late, so render L more frames and drop the first L:
    // timeline frame t lands on file frame t - start_frame.
    std::vector<std::unique_ptr<DelayLine>> own_lines;
    std::vector<DelayLine*> line_ptrs;
    uint32_t pdc_total = 0;
    if (graph && !graph->pdc.need.empty()) {
        pdc_total = graph->pdc.total;
        const std::size_t metro = graph->pdc.metro_index(graph->tracks.size(), graph->fx.size());
        line_ptrs.assign(graph->pdc.need.size(), nullptr);
        for (std::size_t i = 0; i < graph->pdc.need.size(); ++i) {
            if (graph->pdc.need[i] == 0 || i == metro) continue;
            own_lines.push_back(std::make_unique<DelayLine>(
                DelayLine::capacity_for(graph->pdc.need[i], max_block_), 2));
            line_ptrs[i] = own_lines.back().get();
        }
    }
    RtTransport transport;
    transport.sample_rate = sample_rate_;
    transport.playing = true;
    transport.tempo_bpm = rt_tempo_fpb_ > 0.0 ? 60.0 * sample_rate_ / rt_tempo_fpb_ : 120.0;
    // Warped clips: private stretch voices (the live ones keep their state).
    std::vector<std::unique_ptr<WarpVoice>> own_voices;
    std::vector<WarpVoice*> voice_ptrs;
    if (graph)
        for (const WarpVoiceKey& k : graph->warp_voice_keys) {
            own_voices.push_back(std::make_unique<WarpVoice>(sample_rate_, k.channels, max_block_));
            voice_ptrs.push_back(own_voices.back().get());
        }
    InstrumentInputs inst{synth_ptrs.empty() ? nullptr : synth_ptrs.data(), plugins ? rt_synth_gen_ : 0,
                          plugins ? &transport : nullptr, line_ptrs.empty() ? nullptr : line_ptrs.data(), 0};
    inst.warp_voices = voice_ptrs.empty() ? nullptr : voice_ptrs.data();
    ScopedFlushDenormals ftz;
    const int64_t render_end = end_frame + pdc_total;
    const int64_t first_kept = start_frame + pdc_total;
    for (int64_t pos = start_frame; pos < render_end;) {
        const auto n = static_cast<uint32_t>(std::min<int64_t>(render_end - pos, max_block_));
        transport.position = pos;
        render_block(graph.get(), pos, true, n, ctx, false, 0, inst);
        const auto skip = static_cast<uint32_t>(std::clamp<int64_t>(first_kept - pos, 0, n));
        const float* l = ctx.master_l() + skip;
        const float* r = ctx.master_r() + skip;
        const uint32_t keep = n - skip;
        for (uint32_t i = 0; i < keep; ++i) {
            interleaved[2 * i] = l[i];
            interleaved[2 * i + 1] = r[i];
        }
        if (keep > 0) {
            rc = writer.write(interleaved.data(), keep, err);
            if (rc != SE_OK) return rc;
        }
        pos += n;
    }
    return writer.finish(err);
}

// ============================================================ plugins

bool Engine::plugin_is_live(uint64_t device_id) {
    std::lock_guard lock(plugins_.mutex());
    return plugins_.is_live(device_id);
}

void Engine::plugin_load_errors(std::vector<std::pair<uint64_t, std::string>>& out) {
    std::lock_guard lock(plugins_.mutex());
    plugins_.load_errors(out);
}

std::size_t Engine::plugin_save_states() {
    std::lock_guard lock(plugins_.mutex());
    plugins_.collect(rt_active_seq_.load(std::memory_order_seq_cst));
    return plugins_.save_states();
}

bool Engine::plugin_saved_state(std::size_t index, uint64_t& id, const uint8_t*& data, std::size_t& len) {
    std::lock_guard lock(plugins_.mutex());
    return plugins_.saved_state(index, id, data, len);
}

int32_t Engine::plugin_save_state_of(uint64_t device_id, std::string& err) {
    std::lock_guard lock(plugins_.mutex());
    return plugins_.save_state_of(device_id, err);
}

int32_t Engine::plugin_prepare(const se_device_desc& d, bool instrument, const char* track_name, std::string& err) {
    std::lock_guard lock(plugins_.mutex());
    if (plugins_.detached()) {
        err = "the engine is detached from the main thread";
        return SE_ERR_STATE;
    }
    PluginInstance* inst = plugins_.prepare(d, instrument, track_name, err);
    if (inst == nullptr) {
        if (err.empty()) err = "could not load the plugin";
        return SE_ERR_NOT_FOUND;
    }
    return SE_OK;
}

void Engine::plugin_set_detached(bool detached) {
    std::lock_guard lock(plugins_.mutex());
    plugins_.set_detached(detached);
}

int32_t Engine::plugin_editor_open(uint64_t device_id, std::string& err) {
    std::lock_guard lock(plugins_.mutex());
    plugins_.collect(rt_active_seq_.load(std::memory_order_seq_cst));
    return plugins_.editor_open(device_id, err);
}

void Engine::plugin_editor_close(uint64_t device_id) {
    std::lock_guard lock(plugins_.mutex());
    plugins_.editor_close(device_id);
}

bool Engine::plugin_editor_is_open(uint64_t device_id) {
    std::lock_guard lock(plugins_.mutex());
    return plugins_.editor_is_open(device_id);
}

bool Engine::plugin_latency(uint64_t device_id, uint32_t& frames) {
    std::lock_guard lock(plugins_.mutex());
    return plugins_.latency(device_id, frames);
}

void Engine::plugin_idle() {
    std::lock_guard lock(plugins_.mutex());
    plugins_.idle(rt_active_seq_.load(std::memory_order_seq_cst));
    if (plugins_.service_latencies()) replan_pdc_locked();
    if (warp_renderer_.take_finished()) attach_warp_renders_locked();
    // CLAP parameter flushes requested while nothing processes: run them
    // here with the RT path held (a running device or offline render
    // processes the plugin instead, which delivers the same events).
    if (plugins_.flush_pending() && acquire_rt_owner(kOwnerControl) == kOwnerNone) {
        plugins_.flush_params();
        release_rt_owner();
    }
}

int32_t Engine::plugin_params(uint64_t device_id, std::vector<ParamInfo>& out, std::string& err) {
    std::lock_guard lock(plugins_.mutex());
    PluginInstance* inst = plugins_.instance(device_id);
    if (inst == nullptr) {
        err = "no live plugin instance for this device";
        return SE_ERR_NOT_FOUND;
    }
    inst->params(out);
    return SE_OK;
}

int32_t Engine::plugin_param_value(uint64_t device_id, uint32_t param_id, float& out, std::string& err) {
    std::lock_guard lock(plugins_.mutex());
    PluginInstance* inst = plugins_.instance(device_id);
    if (inst == nullptr || !inst->param_value(param_id, out)) {
        err = inst == nullptr ? "no live plugin instance for this device" : "unknown parameter";
        return SE_ERR_NOT_FOUND;
    }
    return SE_OK;
}

int32_t Engine::plugin_param_text(uint64_t device_id, uint32_t param_id, float normalized, std::string& out,
                                  std::string& err) {
    std::lock_guard lock(plugins_.mutex());
    PluginInstance* inst = plugins_.instance(device_id);
    if (inst == nullptr || !inst->param_text(param_id, normalized, out)) {
        err = inst == nullptr ? "no live plugin instance for this device" : "unknown parameter";
        return SE_ERR_NOT_FOUND;
    }
    return SE_OK;
}

std::size_t Engine::take_param_changes(se_plugin_param_change* out, std::size_t capacity) {
    std::size_t n = 0;
    ParamChangeRecord r;
    while (n < capacity && plugins_.bus().pop(r)) {
        se_plugin_param_change& c = out[n++];
        c = se_plugin_param_change{};
        c.device_id = r.device_id;
        c.frame = r.frame;
        c.param_id = r.param_id;
        c.normalized = r.normalized;
        c.gesture_begin = r.gesture_begin ? 1 : 0;
        c.gesture_end = r.gesture_end ? 1 : 0;
    }
    return n;
}

bool Engine::plugin_simulate_gesture(uint64_t device_id, uint32_t param_id, float normalized) {
    std::lock_guard lock(plugins_.mutex());
    PluginInstance* inst = plugins_.instance(device_id);
    return inst != nullptr && inst->simulate_editor_gesture(param_id, normalized);
}

// ============================================================ delay compensation

void Engine::plan_pdc(RenderGraph& g) const {
    PdcPlan p;
    p.enabled = pdc_enabled_.load(std::memory_order_acquire);
    const std::size_t track_count = g.tracks.size();
    const std::size_t fx_count = g.fx.size();
    const auto lat_of = [](const PluginInstance* inst) -> uint64_t {
        return inst != nullptr ? inst->cached_latency() : 0;
    };
    // lat_i: every insert (bypassed ones too) + the instrument plugin.
    p.track_latency.assign(track_count, 0);
    for (std::size_t t = 0; t < track_count; ++t) {
        TrackNode& node = g.tracks[t];
        node.pdc_delay = node.pdc_src_delay = node.out_delay = 0;
        node.pdc_pre = false;
        uint64_t lat = node.has_instrument_plugin ? lat_of(node.instrument_plugin) : 0;
        for (uint32_t i = 0; i < node.fx_count; ++i) lat += lat_of(g.fx[node.fx_begin + i].inst);
        p.track_latency[t] = static_cast<uint32_t>(std::min<uint64_t>(lat, UINT32_MAX));
    }
    // A track's chain latency (capped to 32 bits, far beyond any real chain).
    const auto lat = [&](std::size_t t) -> uint64_t { return p.track_latency[t]; };
    // Routing (spec 002): a return receives at in_r, the latest chain latency
    // of the tracks feeding it, and reaches the master at in_r + lat_r. The
    // master's L is the latest arrival. Muted tracks count, so mute and solo
    // never shift timing.
    // A return that sends into another return (a group's sends) feeds it at
    // its own arrival plus its chain, so buses are visited in processing order.
    std::vector<uint64_t> bus_in(g.bus_count, 0);
    const auto input_of = [&](const TrackNode& node) -> uint64_t {
        return node.bus >= 0 ? bus_in[static_cast<std::size_t>(node.bus)] : 0;
    };
    const bool ordered = g.order.size() == track_count;
    for (std::size_t k = 0; k < track_count; ++k) {
        const std::size_t t = ordered ? g.order[k] : k;
        const TrackNode& node = g.tracks[t];
        const uint64_t ready = input_of(node) + lat(t);
        if (node.out_bus >= 0) bus_in[static_cast<std::size_t>(node.out_bus)] = std::max(bus_in[static_cast<std::size_t>(node.out_bus)], ready);
        for (uint32_t s = node.send_begin; s < node.send_begin + node.send_count; ++s) {
            const auto b = static_cast<std::size_t>(g.sends[s].bus);
            bus_in[b] = std::max(bus_in[b], ready);
        }
    }
    uint64_t total = 0;
    for (std::size_t t = 0; t < track_count; ++t)
        if (g.tracks[t].out_bus < 0) total = std::max(total, input_of(g.tracks[t]) + lat(t));
    for (FxSlot& slot : g.fx) slot.pdc_bypass_delay = slot.auto_delay = 0;
    for (SendNode& send : g.sends) send.edge_delay = 0;
    for (TrackNode& node : g.tracks) node.auto_delay = 0;
    p.uncapped = static_cast<uint32_t>(std::min<uint64_t>(total, UINT32_MAX));
    if (!p.enabled || total == 0) {
        g.pdc = std::move(p);
        return;
    }
    p.clamped = total > kPdcMaxDelay;
    const auto cap = [&](uint64_t v) {
        if (v > kPdcMaxDelay) p.clamped = true;
        return static_cast<uint32_t>(std::min<uint64_t>(v, kPdcMaxDelay));
    };
    p.total = cap(total);
    const std::size_t send_count = g.sends.size();
    p.need.assign(p.out_index(track_count, fx_count, send_count, track_count), 0);
    // When a destination is reached: L for the master, in_r for a return.
    const auto arrival = [&](int32_t bus) -> uint64_t {
        return bus < 0 ? p.total : std::min<uint64_t>(bus_in[static_cast<std::size_t>(bus)], p.total);
    };
    for (std::size_t t = 0; t < track_count; ++t) {
        TrackNode& node = g.tracks[t];
        // Aligned for the earliest destination after the chain; later
        // destinations get an edge delay after the fader (or the tap).
        const uint64_t in = input_of(node);
        const uint64_t at = std::min<uint64_t>(in + lat(t), p.total); // when the chain's output is ready
        uint64_t first = arrival(node.out_bus);
        for (uint32_t s = node.send_begin; s < node.send_begin + node.send_count; ++s)
            first = std::min(first, arrival(g.sends[s].bus));
        first = std::max(first, at);
        node.pdc_delay = cap(first - at);
        node.out_delay = cap(arrival(node.out_bus) - std::min(arrival(node.out_bus), first));
        p.need[p.out_index(track_count, fx_count, send_count, t)] = node.out_delay;
        for (uint32_t s = node.send_begin; s < node.send_begin + node.send_count; ++s) {
            SendNode& send = g.sends[s];
            send.edge_delay = cap(arrival(send.bus) - std::min(arrival(send.bus), first));
            p.need[p.send_index(track_count, fx_count, s)] = send.edge_delay;
        }
        node.pdc_pre = node.monitor && node.input_mode != SE_INPUT_NONE;
        p.need[t] = node.pdc_delay;
        const uint64_t li = node.has_instrument_plugin ? lat_of(node.instrument_plugin) : 0;
        if (li > 0 && node.clip_count > 0) {
            node.pdc_src_delay = cap(li);
            p.need[p.src_index(track_count, t)] = node.pdc_src_delay;
        }
        for (uint32_t i = 0; i < node.fx_count; ++i) {
            FxSlot& slot = g.fx[node.fx_begin + i];
            const uint64_t sl = lat_of(slot.inst);
            if (sl == 0 || (slot.enabled && slot.on_lane < 0)) continue; // only (possibly) bypassed slots need a line
            slot.pdc_bypass_delay = cap(sl);
            p.need[p.slot_index(track_count, node.fx_begin + i)] = slot.pdc_bypass_delay;
        }
        // Automation (docs/adr/0005): how late the audio reaching each stage
        // is relative to the timeline frame it was read at. Inserts see the
        // instrument plugin's latency plus every earlier slot's (bypassed
        // ones are delayed by the same amount), plus the track delay when it
        // runs before the chain (monitoring); the fader sees lat_i + delay.
        uint64_t before = (node.pdc_pre ? node.pdc_delay : 0) + li + in;
        for (uint32_t i = 0; i < node.fx_count; ++i) {
            FxSlot& slot = g.fx[node.fx_begin + i];
            slot.auto_delay = cap(before);
            before += lat_of(slot.inst);
        }
        node.auto_delay = cap(at + node.pdc_delay);
    }
    p.need[p.metro_index(track_count, fx_count)] = p.total;
    g.pdc = std::move(p);
}

void Engine::replan_pdc_locked() {
    std::shared_ptr<RenderGraph> current;
    {
        std::lock_guard lock(control_mutex_);
        current = current_;
    }
    if (!current) return;
    auto next = clone_render_graph(*current);
    plan_pdc(*next);
    install_graph(std::move(next));
}

void Engine::attach_warp_renders_locked() {
    std::shared_ptr<RenderGraph> current;
    {
        std::lock_guard lock(control_mutex_);
        current = current_;
    }
    if (!current) return;
    auto next = clone_render_graph(*current);
    warp_renderer_.resolve(*next);
    install_graph(std::move(next));
}

void Engine::set_pdc_enabled(bool enabled) {
    std::lock_guard lock(plugins_.mutex());
    if (pdc_enabled_.exchange(enabled, std::memory_order_acq_rel) == enabled) return;
    replan_pdc_locked();
}

void Engine::pdc_report(se_pdc_report& out, std::vector<se_pdc_track>& tracks) const {
    out = se_pdc_report{};
    tracks.clear();
    std::lock_guard lock(control_mutex_);
    out.enabled = pdc_enabled() ? 1 : 0;
    if (!current_) return;
    const PdcPlan& p = current_->pdc;
    out.enabled = p.enabled ? 1 : 0;
    out.total_frames = p.total;
    out.uncapped_frames = p.uncapped;
    out.clamped = p.clamped ? 1 : 0;
    out.track_count = static_cast<uint32_t>(current_->tracks.size());
    for (std::size_t t = 0; t < current_->tracks.size(); ++t)
        tracks.push_back(se_pdc_track{current_->tracks[t].track_id, p.track_latency[t], current_->tracks[t].pdc_delay});
}

int64_t Engine::audible_position() const noexcept {
    const int64_t pos = pub_position_.load(std::memory_order_acquire);
    if (!pub_playing_.load(std::memory_order_acquire)) return pos;
    return pos - static_cast<int64_t>(pdc_total_.load(std::memory_order_relaxed)) -
           device_out_latency_.load(std::memory_order_relaxed);
}

int32_t Engine::builtin_set_param(uint64_t device_id, uint32_t index, float value) {
    std::lock_guard lock(plugins_.mutex());
    PluginInstance* inst = plugins_.instance(device_id);
    BuiltinInstance* b = inst != nullptr ? inst->as_builtin() : nullptr;
    if (b == nullptr) return SE_ERR_NOT_FOUND;
    if (index >= b->param_count() || !std::isfinite(value)) return SE_ERR_INVALID_ARG;
    b->set_value(index, value);
    return SE_OK;
}

int32_t Engine::builtin_note(uint64_t device_id, uint8_t pitch, uint8_t velocity) {
    if (pitch > 127 || velocity > 127) return SE_ERR_INVALID_ARG;
    std::lock_guard lock(plugins_.mutex());
    PluginInstance* inst = plugins_.instance(device_id);
    BuiltinInstance* b = inst != nullptr ? inst->as_builtin() : nullptr;
    if (b == nullptr || !b->instrument()) return SE_ERR_NOT_FOUND;
    return b->push_note(pitch, velocity) ? SE_OK : SE_ERR_STATE;
}

int32_t Engine::builtin_readouts(uint64_t device_id, float* out, std::size_t capacity, std::size_t& count) {
    std::lock_guard lock(plugins_.mutex());
    PluginInstance* inst = plugins_.instance(device_id);
    BuiltinInstance* b = inst != nullptr ? inst->as_builtin() : nullptr;
    if (b == nullptr) return SE_ERR_NOT_FOUND;
    count = b->readouts(out, static_cast<uint32_t>(std::min<std::size_t>(capacity, UINT32_MAX)));
    return SE_OK;
}

int32_t Engine::builtin_analysis(uint64_t device_id, float* out, std::size_t capacity, std::size_t& count,
                                 uint64_t& seq) {
    std::lock_guard lock(plugins_.mutex());
    PluginInstance* inst = plugins_.instance(device_id);
    BuiltinInstance* b = inst != nullptr ? inst->as_builtin() : nullptr;
    if (b == nullptr) return SE_ERR_NOT_FOUND;
    count = b->analysis(out, static_cast<uint32_t>(std::min<std::size_t>(capacity, UINT32_MAX)), seq);
    return SE_OK;
}

int32_t Engine::plugin_set_param(uint64_t device_id, uint32_t param_id, float normalized) {
    if (!std::isfinite(normalized)) return SE_ERR_INVALID_ARG;
    std::lock_guard lock(plugins_.mutex());
    PluginInstance* inst = plugins_.instance(device_id);
    double lo = 0.0, hi = 1.0;
    if (inst == nullptr || inst->as_builtin() != nullptr || !inst->param_range(param_id, lo, hi)) return SE_ERR_NOT_FOUND;
    const float n = std::clamp(normalized, 0.0f, 1.0f);
    if (auto it = mod_bases_.find({device_id, param_id}); it != mod_bases_.end()) {
        it->second->store(n, std::memory_order_relaxed);
        return SE_OK;
    }
    return inst->set_parameter(param_id, lo + static_cast<double>(n) * (hi - lo)) ? SE_OK : SE_ERR_STATE;
}

int32_t Engine::mod_base(uint64_t device_id, uint32_t param_id, float& out) {
    std::lock_guard lock(plugins_.mutex());
    auto it = mod_bases_.find({device_id, param_id});
    if (it == mod_bases_.end()) return SE_ERR_NOT_FOUND;
    out = it->second->load(std::memory_order_relaxed);
    return SE_OK;
}

bool Engine::plugin_set_parameter(uint64_t device_id, uint32_t param_id, double value) {
    std::lock_guard lock(plugins_.mutex());
    PluginInstance* inst = plugins_.instance(device_id);
    return inst != nullptr && inst->set_parameter(param_id, value);
}

} // namespace dstudio
