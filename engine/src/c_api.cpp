// extern "C" surface. Every entry point is noexcept and converts exceptions
// (in practice only std::bad_alloc from non-RT paths) into se_result codes.
// se_last_error() is set only by failing calls (like errno): success paths
// never touch the thread-local, so the RT-adjacent se_render_offline success
// path cannot trigger a lazy TLS allocation.
#include "dstudio_engine.h"

#include "engine.hpp"
#include "devices/builtin.hpp"
#include "test_hooks.hpp"

#include <cmath>
#include <cstddef>
#include <cstring>
#include <new>
#include <string>

struct se_engine {
    std::unique_ptr<dstudio::Engine> impl;
};

// ABI layout guards for the recording/device additions (64-bit targets;
// also checked from Rust in crates/dstudio-engine/src/ffi.rs).
#if UINTPTR_MAX == 0xffffffffffffffffu
static_assert(sizeof(se_track_desc_v4) == 56 && alignof(se_track_desc_v4) == 8);
static_assert(offsetof(se_track_desc_v4, arm) == 19 && offsetof(se_track_desc_v4, monitor) == 20 &&
              offsetof(se_track_desc_v4, input_mode) == 21 && offsetof(se_track_desc_v4, input_channel) == 22 &&
              offsetof(se_track_desc_v4, clips) == 24 && offsetof(se_track_desc_v4, synth) == 48);
static_assert(sizeof(se_session_desc_v4) == 16);
static_assert(sizeof(se_device_info) == 656 && offsetof(se_device_info, input_channels) == 512 &&
              offsetof(se_device_info, sample_rate_count) == 524 && offsetof(se_device_info, sample_rates) == 528);
static_assert(sizeof(se_device_config) == 32 && offsetof(se_device_config, sample_rate) == 16 &&
              offsetof(se_device_config, buffer_size) == 24 && offsetof(se_device_config, input_enabled) == 28);
static_assert(sizeof(se_device_status) == 808 && offsetof(se_device_status, running) == 768 &&
              offsetof(se_device_status, sample_rate) == 776 && offsetof(se_device_status, _pad2) == 804);
static_assert(sizeof(se_recorded_take) == 1064 && offsetof(se_recorded_take, sample_rate) == 32 &&
              offsetof(se_recorded_take, path) == 40);
static_assert(sizeof(se_live_take) == 24 && sizeof(se_record_stats) == 32);
// Plugin additions.
static_assert(sizeof(se_device_desc) == 56 && alignof(se_device_desc) == 8);
static_assert(offsetof(se_device_desc, name) == 8 && offsetof(se_device_desc, plugin_id) == 16 &&
              offsetof(se_device_desc, path) == 24 && offsetof(se_device_desc, state) == 32 &&
              offsetof(se_device_desc, state_len) == 40 && offsetof(se_device_desc, format) == 48 &&
              offsetof(se_device_desc, enabled) == 49 && offsetof(se_device_desc, _pad) == 50);
static_assert(sizeof(se_track_desc_v5) == 88 && alignof(se_track_desc_v5) == 8);
static_assert(offsetof(se_track_desc_v5, synth) == 48 && offsetof(se_track_desc_v5, devices) == 56 &&
              offsetof(se_track_desc_v5, device_count) == 64 && offsetof(se_track_desc_v5, _pad2) == 68 &&
              offsetof(se_track_desc_v5, instrument_plugin) == 72 && offsetof(se_track_desc_v5, track_name) == 80);
static_assert(sizeof(se_session_desc_v5) == 16);
static_assert(sizeof(se_plugin_message) == 264 && offsetof(se_plugin_message, message) == 8);
#endif
// Delay compensation additions (same on 32- and 64-bit targets).
static_assert(sizeof(se_pdc_report) == 16 && alignof(se_pdc_report) == 4);
static_assert(offsetof(se_pdc_report, uncapped_frames) == 4 && offsetof(se_pdc_report, track_count) == 8 &&
              offsetof(se_pdc_report, enabled) == 12 && offsetof(se_pdc_report, clamped) == 13 &&
              offsetof(se_pdc_report, _pad) == 14);
static_assert(sizeof(se_pdc_track) == 16 && alignof(se_pdc_track) == 8);
static_assert(offsetof(se_pdc_track, latency_frames) == 8 && offsetof(se_pdc_track, delay_frames) == 12);
// Automation additions.
static_assert(sizeof(se_auto_point) == 16 && alignof(se_auto_point) == 8);
static_assert(offsetof(se_auto_point, value) == 8 && offsetof(se_auto_point, curve) == 12);
#if UINTPTR_MAX == 0xffffffffffffffffu
static_assert(sizeof(se_auto_lane_desc) == 32 && alignof(se_auto_lane_desc) == 8);
static_assert(offsetof(se_auto_lane_desc, logarithmic) == 1 && offsetof(se_auto_lane_desc, synth_param) == 2 &&
              offsetof(se_auto_lane_desc, param_id) == 4 && offsetof(se_auto_lane_desc, device_id) == 8 &&
              offsetof(se_auto_lane_desc, points) == 16 && offsetof(se_auto_lane_desc, point_count) == 24 &&
              offsetof(se_auto_lane_desc, flags) == 28);
static_assert(sizeof(se_track_desc_v6) == 104 && alignof(se_track_desc_v6) == 8);
static_assert(offsetof(se_track_desc_v6, lanes) == 88 && offsetof(se_track_desc_v6, lane_count) == 96 &&
              offsetof(se_track_desc_v6, _pad3) == 100);
static_assert(sizeof(se_session_desc_v6) == 32 && offsetof(se_session_desc_v6, master_lanes) == 16 &&
              offsetof(se_session_desc_v6, master_lane_count) == 24);
static_assert(sizeof(se_clip_warp_desc) == 40 && alignof(se_clip_warp_desc) == 8);
static_assert(offsetof(se_clip_warp_desc, mode) == 8 && offsetof(se_clip_warp_desc, transpose) == 12 &&
              offsetof(se_clip_warp_desc, points) == 16 && offsetof(se_clip_warp_desc, point_count) == 24 &&
              offsetof(se_clip_warp_desc, slice_count) == 28 && offsetof(se_clip_warp_desc, slices) == 32);
static_assert(sizeof(se_session_desc_v7) == 48 && offsetof(se_session_desc_v7, warps) == 32 &&
              offsetof(se_session_desc_v7, warp_count) == 40);
static_assert(sizeof(se_clip_loop_desc) == 24 && alignof(se_clip_loop_desc) == 8 &&
              offsetof(se_clip_loop_desc, loop_end) == 16);
static_assert(sizeof(se_session_desc_v8) == 64 && offsetof(se_session_desc_v8, loops) == 48 &&
              offsetof(se_session_desc_v8, loop_count) == 56);
static_assert(sizeof(se_track_route) == 24 && alignof(se_track_route) == 8 && offsetof(se_track_route, is_return) == 16,
              "se_track_route layout");
static_assert(sizeof(se_send_desc) == 24 && offsetof(se_send_desc, gain) == 16 && offsetof(se_send_desc, pre_fader) == 20,
              "se_send_desc layout");
static_assert(sizeof(se_session_desc_v9) == 88 && offsetof(se_session_desc_v9, routes) == 64 &&
                  offsetof(se_session_desc_v9, route_count) == 72 && offsetof(se_session_desc_v9, send_count) == 76 &&
                  offsetof(se_session_desc_v9, sends) == 80,
              "se_session_desc_v9 layout");
static_assert(sizeof(se_track_strip) == 24 && alignof(se_track_strip) == 8 && offsetof(se_track_strip, trim) == 8 &&
                  offsetof(se_track_strip, width) == 12 && offsetof(se_track_strip, mono) == 16,
              "se_track_strip layout");
static_assert(sizeof(se_session_desc_v10) == 104 && offsetof(se_session_desc_v10, strips) == 88 &&
                  offsetof(se_session_desc_v10, strip_count) == 96,
              "se_session_desc_v10 layout");
static_assert(sizeof(se_clip_fade_desc) == 24 && alignof(se_clip_fade_desc) == 8, "se_clip_fade_desc layout");
static_assert(sizeof(se_session_desc_v11) == 120 && offsetof(se_session_desc_v11, fades) == 104 &&
                  offsetof(se_session_desc_v11, fade_count) == 112,
              "se_session_desc_v11 layout");
static_assert(sizeof(se_tempo_segment) == 32 && alignof(se_tempo_segment) == 8 &&
                  offsetof(se_tempo_segment, start_quarter) == 8 && offsetof(se_tempo_segment, end_bpm) == 24,
              "se_tempo_segment layout");
static_assert(sizeof(se_session_desc_v12) == 144 && offsetof(se_session_desc_v12, tempo) == 120 &&
                  offsetof(se_session_desc_v12, tempo_count) == 128 && offsetof(se_session_desc_v12, click_quarters) == 136,
              "se_session_desc_v12 layout");
#endif
static_assert(sizeof(se_warp_point) == 16 && alignof(se_warp_point) == 8 && offsetof(se_warp_point, source) == 8);
static_assert(sizeof(se_warp_slice) == 16 && offsetof(se_warp_slice, percussive) == 8);
static_assert(sizeof(se_onset) == 16 && offsetof(se_onset, strength) == 8 && offsetof(se_onset, percussive) == 12);
static_assert(sizeof(se_plugin_param_info) == 272 && alignof(se_plugin_param_info) == 4);
static_assert(offsetof(se_plugin_param_info, default_normalized) == 8 &&
              offsetof(se_plugin_param_info, automatable) == 12 && offsetof(se_plugin_param_info, name) == 16 &&
              offsetof(se_plugin_param_info, group) == 144);
static_assert(sizeof(se_plugin_param_change) == 32 && alignof(se_plugin_param_change) == 8);
static_assert(offsetof(se_plugin_param_change, frame) == 8 && offsetof(se_plugin_param_change, param_id) == 16 &&
              offsetof(se_plugin_param_change, normalized) == 20 &&
              offsetof(se_plugin_param_change, gesture_begin) == 24 &&
              offsetof(se_plugin_param_change, gesture_end) == 25);
static_assert(SE_SYNTH_P_COUNT == 20);
// v6 tracks extend v5 tracks field by field (copied with memcpy).
static_assert(offsetof(se_track_desc_v6, track_name) == offsetof(se_track_desc_v5, track_name) &&
              offsetof(se_track_desc_v6, instrument_plugin) == offsetof(se_track_desc_v5, instrument_plugin) &&
              offsetof(se_track_desc_v6, devices) == offsetof(se_track_desc_v5, devices) &&
              offsetof(se_track_desc_v6, synth) == offsetof(se_track_desc_v5, synth) &&
              offsetof(se_track_desc_v6, clips) == offsetof(se_track_desc_v5, clips) &&
              sizeof(se_track_desc_v5) == offsetof(se_track_desc_v6, lanes));
// v5 tracks extend v4 tracks field by field (same offsets for the common prefix).
static_assert(offsetof(se_track_desc_v5, track_id) == offsetof(se_track_desc_v4, track_id) &&
              offsetof(se_track_desc_v5, input_channel) == offsetof(se_track_desc_v4, input_channel) &&
              offsetof(se_track_desc_v5, clips) == offsetof(se_track_desc_v4, clips) &&
              offsetof(se_track_desc_v5, synth) == offsetof(se_track_desc_v4, synth));

namespace {

thread_local std::string t_last_error;

int32_t fail(int32_t code, std::string msg) noexcept {
    try {
        t_last_error = std::move(msg);
    } catch (...) {
    }
    return code;
}

int32_t fail_from_err(int32_t code, std::string& err) noexcept {
    if (err.empty()) err = "error " + std::to_string(code);
    return fail(code, std::move(err));
}

template <class F>
int32_t guarded(F&& f) noexcept {
    try {
        return f();
    } catch (const std::bad_alloc&) {
        return fail(SE_ERR_OUT_OF_MEMORY, "out of memory");
    } catch (const std::exception& e) {
        return fail(SE_ERR_STATE, std::string("internal error: ") + e.what());
    } catch (...) {
        return fail(SE_ERR_STATE, "internal error");
    }
}

#define SE_REQUIRE_ENGINE(e)                                                                                           \
    do {                                                                                                               \
        if ((e) == nullptr || !(e)->impl) return fail(SE_ERR_INVALID_ARG, "engine is NULL");                           \
    } while (0)

int32_t check(int32_t rc, std::string& err) noexcept { return rc == SE_OK ? SE_OK : fail_from_err(rc, err); }

} // namespace

extern "C" {

uint32_t se_abi_version(void) { return SE_ABI_VERSION; }

const char* se_last_error(void) { return t_last_error.c_str(); }

se_engine* se_engine_create(const se_config* config) {
    if (config == nullptr) {
        fail(SE_ERR_INVALID_ARG, "config is NULL");
        return nullptr;
    }
    try {
        std::string err;
        auto impl = dstudio::Engine::create(*config, err);
        if (!impl) {
            fail(SE_ERR_INVALID_ARG, err);
            return nullptr;
        }
        return new se_engine{std::move(impl)};
    } catch (const std::bad_alloc&) {
        fail(SE_ERR_OUT_OF_MEMORY, "out of memory");
    } catch (...) {
        fail(SE_ERR_STATE, "internal error creating engine");
    }
    return nullptr;
}

void se_engine_destroy(se_engine* engine) {
    try {
        delete engine;
    } catch (...) {
    }
}

int32_t se_device_start(se_engine* engine) {
    SE_REQUIRE_ENGINE(engine);
    return guarded([&] {
        std::string err;
        return check(engine->impl->device_start(err), err);
    });
}

int32_t se_device_stop(se_engine* engine) {
    SE_REQUIRE_ENGINE(engine);
    return guarded([&] { return engine->impl->device_stop(); });
}

int32_t se_device_is_running(const se_engine* engine) {
    if (engine == nullptr || !engine->impl) return 0;
    return engine->impl->device_running() ? 1 : 0;
}

double se_engine_sample_rate(const se_engine* engine) {
    if (engine == nullptr || !engine->impl) return 0.0;
    return engine->impl->sample_rate();
}

// ---- assets

int32_t se_asset_load_file(se_engine* engine, const char* utf8_path, se_asset_info* out_info) {
    SE_REQUIRE_ENGINE(engine);
    if (utf8_path == nullptr) return fail(SE_ERR_INVALID_ARG, "path is NULL");
    return guarded([&] {
        std::string err;
        return check(engine->impl->asset_load_file(utf8_path, out_info, err), err);
    });
}

int32_t se_asset_release(se_engine* engine, uint64_t asset_id) {
    SE_REQUIRE_ENGINE(engine);
    return guarded([&] {
        const int32_t rc = engine->impl->asset_release(asset_id);
        return rc == SE_OK ? SE_OK : fail(rc, "unknown asset id " + std::to_string(asset_id));
    });
}

int32_t se_asset_reverse(se_engine* engine, uint64_t asset_id, se_asset_info* out_info) {
    SE_REQUIRE_ENGINE(engine);
    return guarded([&] {
        const int32_t rc = engine->impl->asset_reverse(asset_id, out_info);
        return rc == SE_OK ? SE_OK : fail(rc, "unknown asset id " + std::to_string(asset_id));
    });
}

int32_t se_asset_get_info(const se_engine* engine, uint64_t asset_id, se_asset_info* out_info) {
    SE_REQUIRE_ENGINE(engine);
    if (out_info == nullptr) return fail(SE_ERR_INVALID_ARG, "out_info is NULL");
    return guarded([&] {
        const int32_t rc = engine->impl->asset_get_info(asset_id, out_info);
        return rc == SE_OK ? SE_OK : fail(rc, "unknown asset id " + std::to_string(asset_id));
    });
}

int32_t se_asset_compute_peaks(const se_engine* engine, uint64_t asset_id, uint64_t start_frame,
                               uint32_t frames_per_peak, float* out_minmax, size_t capacity, size_t* out_count) {
    SE_REQUIRE_ENGINE(engine);
    if (frames_per_peak == 0) return fail(SE_ERR_INVALID_ARG, "frames_per_peak must be > 0");
    if (capacity > 0 && out_minmax == nullptr) return fail(SE_ERR_INVALID_ARG, "out_minmax is NULL");
    return guarded([&] {
        const int32_t rc = engine->impl->asset_compute_peaks(asset_id, -1, start_frame, frames_per_peak, out_minmax,
                                                             capacity, out_count);
        return rc == SE_OK ? SE_OK : fail(rc, "unknown asset id " + std::to_string(asset_id));
    });
}

int32_t se_asset_compute_channel_peaks(const se_engine* engine, uint64_t asset_id, uint32_t channel,
                                       uint64_t start_frame, uint32_t frames_per_peak, float* out_minmax,
                                       size_t capacity, size_t* out_count) {
    SE_REQUIRE_ENGINE(engine);
    if (frames_per_peak == 0) return fail(SE_ERR_INVALID_ARG, "frames_per_peak must be > 0");
    if (capacity > 0 && out_minmax == nullptr) return fail(SE_ERR_INVALID_ARG, "out_minmax is NULL");
    if (channel > 1) return fail(SE_ERR_INVALID_ARG, "channel must be 0 or 1");
    return guarded([&] {
        const int32_t rc = engine->impl->asset_compute_peaks(asset_id, static_cast<int32_t>(channel), start_frame,
                                                             frames_per_peak, out_minmax, capacity, out_count);
        if (rc == SE_ERR_INVALID_ARG) return fail(rc, "asset " + std::to_string(asset_id) + " has no channel " + std::to_string(channel));
        return rc == SE_OK ? SE_OK : fail(rc, "unknown asset id " + std::to_string(asset_id));
    });
}

// ---- session

int32_t se_session_publish(se_engine* engine, const se_session_desc* session) {
    SE_REQUIRE_ENGINE(engine);
    if (session == nullptr) return fail(SE_ERR_INVALID_ARG, "session is NULL");
    return guarded([&] {
        std::string err;
        return check(engine->impl->publish(*session, err), err);
    });
}

int32_t se_session_publish_v2(se_engine* engine, const se_session_desc_v2* session) {
    SE_REQUIRE_ENGINE(engine);
    if (session == nullptr) return fail(SE_ERR_INVALID_ARG, "session is NULL");
    return guarded([&] {
        std::string err;
        return check(engine->impl->publish(*session, err), err);
    });
}

int32_t se_session_publish_v3(se_engine* engine, const se_session_desc_v3* session) {
    SE_REQUIRE_ENGINE(engine);
    if (session == nullptr) return fail(SE_ERR_INVALID_ARG, "session is NULL");
    return guarded([&] {
        std::string err;
        return check(engine->impl->publish(*session, err), err);
    });
}

void se_synth_params_default(se_synth_params* out) {
    if (out != nullptr) *out = dstudio::default_synth_params();
}

// ---- transport

static int32_t push_cmd(se_engine* engine, const dstudio::TransportCommand& cmd) {
    SE_REQUIRE_ENGINE(engine);
    const int32_t rc = engine->impl->push_command(cmd);
    return rc == SE_OK ? SE_OK : fail(rc, "transport command queue is full");
}

static int32_t push(se_engine* engine, dstudio::TransportCommand::Type type, int64_t frame) {
    dstudio::TransportCommand cmd;
    cmd.type = type;
    cmd.frame = frame;
    return push_cmd(engine, cmd);
}

int32_t se_transport_play(se_engine* engine) { return push(engine, dstudio::TransportCommand::Type::Play, 0); }
int32_t se_transport_pause(se_engine* engine) { return push(engine, dstudio::TransportCommand::Type::Pause, 0); }
int32_t se_transport_stop(se_engine* engine) { return push(engine, dstudio::TransportCommand::Type::Stop, 0); }
int32_t se_transport_seek(se_engine* engine, int64_t frame) {
    SE_REQUIRE_ENGINE(engine);
    if (frame > SE_MAX_TIMELINE_FRAME || frame < -SE_MAX_TIMELINE_FRAME)
        return fail(SE_ERR_INVALID_ARG, "seek frame out of range");
    return push(engine, dstudio::TransportCommand::Type::Seek, frame);
}

int32_t se_transport_set_tempo(se_engine* engine, double frames_per_quarter) {
    SE_REQUIRE_ENGINE(engine);
    if (!std::isfinite(frames_per_quarter) || frames_per_quarter < 1.0 || frames_per_quarter > SE_MAX_FRAMES_PER_BEAT)
        return fail(SE_ERR_INVALID_ARG, "tempo frames_per_quarter must be finite and in [1, 1e9]");
    dstudio::TransportCommand cmd;
    cmd.type = dstudio::TransportCommand::Type::SetTempo;
    cmd.frames_per_beat = frames_per_quarter;
    return push_cmd(engine, cmd);
}

int32_t se_transport_set_loop(se_engine* engine, uint8_t enabled, int64_t start_frame, int64_t end_frame) {
    SE_REQUIRE_ENGINE(engine);
    if (enabled != 0 && !(start_frame < end_frame))
        return fail(SE_ERR_INVALID_ARG, "loop start must be before loop end");
    dstudio::TransportCommand cmd;
    cmd.type = dstudio::TransportCommand::Type::SetLoop;
    cmd.enabled = enabled != 0;
    cmd.frame = start_frame;
    cmd.frame2 = end_frame;
    return push_cmd(engine, cmd);
}

int32_t se_metronome_set(se_engine* engine, const se_metronome_desc* desc) {
    SE_REQUIRE_ENGINE(engine);
    if (desc == nullptr) return fail(SE_ERR_INVALID_ARG, "desc is NULL");
    dstudio::TransportCommand cmd;
    cmd.type = dstudio::TransportCommand::Type::SetMetronome;
    cmd.enabled = desc->enabled != 0;
    if (cmd.enabled) {
        if (!std::isfinite(desc->gain) || desc->gain < 0.0f)
            return fail(SE_ERR_INVALID_ARG, "metronome gain must be finite and >= 0");
        if (!std::isfinite(desc->frames_per_beat) || desc->frames_per_beat < 1.0 ||
            desc->frames_per_beat > SE_MAX_FRAMES_PER_BEAT)
            return fail(SE_ERR_INVALID_ARG, "metronome frames_per_beat must be finite and in [1, 1e9]");
        if (desc->beats_per_bar == 0) return fail(SE_ERR_INVALID_ARG, "metronome beats_per_bar must be >= 1");
        cmd.gain = desc->gain;
        cmd.frames_per_beat = desc->frames_per_beat;
        cmd.beats_per_bar = desc->beats_per_bar;
    }
    return push_cmd(engine, cmd);
}

int32_t se_transport_get(const se_engine* engine, se_transport_info* out_info) {
    SE_REQUIRE_ENGINE(engine);
    if (out_info == nullptr) return fail(SE_ERR_INVALID_ARG, "out_info is NULL");
    engine->impl->transport_get(out_info);
    return SE_OK;
}

// ---- meters & stats

int32_t se_meters_read(se_engine* engine, se_meter* out, size_t capacity, size_t* out_count) {
    SE_REQUIRE_ENGINE(engine);
    if (capacity > 0 && out == nullptr) return fail(SE_ERR_INVALID_ARG, "out is NULL");
    return guarded([&] { return engine->impl->meters_read(out, capacity, out_count); });
}

int32_t se_meters_peek(se_engine* engine, se_meter* out, size_t capacity, size_t* out_count) {
    SE_REQUIRE_ENGINE(engine);
    if (capacity > 0 && out == nullptr) return fail(SE_ERR_INVALID_ARG, "out is NULL");
    return guarded([&] { return engine->impl->meters_peek(out, capacity, out_count); });
}

int32_t se_stats_get(const se_engine* engine, se_stats* out) {
    SE_REQUIRE_ENGINE(engine);
    if (out == nullptr) return fail(SE_ERR_INVALID_ARG, "out is NULL");
    engine->impl->stats_get(out);
    return SE_OK;
}

void se_stats_reset(se_engine* engine) {
    if (engine != nullptr && engine->impl) engine->impl->stats_reset();
}

// ---- offline

int32_t se_render_offline(se_engine* engine, float* out_interleaved, uint32_t frames) {
    SE_REQUIRE_ENGINE(engine);
    if (out_interleaved == nullptr && frames > 0) return fail(SE_ERR_INVALID_ARG, "output buffer is NULL");
    // The success path must not allocate: an empty std::string does not, and
    // guarded() is a plain try/catch.
    return guarded([&] {
        std::string err;
        return check(engine->impl->render_offline(out_interleaved, frames, err), err);
    });
}

int32_t se_export_wav(se_engine* engine, const char* utf8_path, int64_t start_frame, int64_t end_frame) {
    SE_REQUIRE_ENGINE(engine);
    if (utf8_path == nullptr) return fail(SE_ERR_INVALID_ARG, "path is NULL");
    return guarded([&] {
        std::string err;
        return check(engine->impl->export_wav(utf8_path, start_frame, end_frame, err), err);
    });
}

// ---- input, monitoring, recording, devices

int32_t se_session_publish_v4(se_engine* engine, const se_session_desc_v4* session) {
    SE_REQUIRE_ENGINE(engine);
    if (session == nullptr) return fail(SE_ERR_INVALID_ARG, "session is NULL");
    return guarded([&] {
        std::string err;
        return check(engine->impl->publish(*session, err), err);
    });
}

int32_t se_session_publish_v5(se_engine* engine, const se_session_desc_v5* session) {
    SE_REQUIRE_ENGINE(engine);
    if (session == nullptr) return fail(SE_ERR_INVALID_ARG, "session is NULL");
    return guarded([&] {
        std::string err;
        return check(engine->impl->publish(*session, err), err);
    });
}

int32_t se_session_publish_v6(se_engine* engine, const se_session_desc_v6* session) {
    SE_REQUIRE_ENGINE(engine);
    if (session == nullptr) return fail(SE_ERR_INVALID_ARG, "session is NULL");
    return guarded([&] {
        std::string err;
        return check(engine->impl->publish(*session, err), err);
    });
}

int32_t se_session_publish_v7(se_engine* engine, const se_session_desc_v7* session) {
    SE_REQUIRE_ENGINE(engine);
    if (session == nullptr) return fail(SE_ERR_INVALID_ARG, "session is NULL");
    return guarded([&] {
        std::string err;
        return check(engine->impl->publish(*session, err), err);
    });
}

int32_t se_session_publish_v8(se_engine* engine, const se_session_desc_v8* session) {
    SE_REQUIRE_ENGINE(engine);
    if (session == nullptr) return fail(SE_ERR_INVALID_ARG, "session is NULL");
    return guarded([&] {
        std::string err;
        return check(engine->impl->publish(*session, err), err);
    });
}

int32_t se_session_publish_v9(se_engine* engine, const se_session_desc_v9* session) {
    SE_REQUIRE_ENGINE(engine);
    if (session == nullptr) return fail(SE_ERR_INVALID_ARG, "session is NULL");
    return guarded([&] {
        std::string err;
        return check(engine->impl->publish(*session, err), err);
    });
}

int32_t se_session_publish_v10(se_engine* engine, const se_session_desc_v10* session) {
    SE_REQUIRE_ENGINE(engine);
    if (session == nullptr) return fail(SE_ERR_INVALID_ARG, "session is NULL");
    return guarded([&] {
        std::string err;
        return check(engine->impl->publish(*session, err), err);
    });
}
int32_t se_session_publish_v11(se_engine* engine, const se_session_desc_v11* session) {
    SE_REQUIRE_ENGINE(engine);
    if (session == nullptr) return fail(SE_ERR_INVALID_ARG, "session is NULL");
    return guarded([&] {
        std::string err;
        return check(engine->impl->publish(*session, err), err);
    });
}
int32_t se_session_publish_v12(se_engine* engine, const se_session_desc_v12* session) {
    SE_REQUIRE_ENGINE(engine);
    if (session == nullptr) return fail(SE_ERR_INVALID_ARG, "session is NULL");
    return guarded([&] {
        std::string err;
        return check(engine->impl->publish(*session, err), err);
    });
}

int32_t se_warp_renders_pending(const se_engine* engine, size_t* out_count) {
    SE_REQUIRE_ENGINE(engine);
    if (out_count == nullptr) return fail(SE_ERR_INVALID_ARG, "out_count is NULL");
    *out_count = engine->impl->warp_renders_pending();
    return SE_OK;
}

int32_t se_asset_detect_onsets(se_engine* engine, uint64_t asset_id, se_onset* out, size_t capacity,
                               size_t* out_count) {
    SE_REQUIRE_ENGINE(engine);
    if (out == nullptr && capacity > 0) return fail(SE_ERR_INVALID_ARG, "out is NULL");
    return guarded([&] {
        const int32_t rc = engine->impl->asset_detect_onsets(asset_id, out, capacity, out_count);
        return rc == SE_OK ? rc : fail(rc, "unknown asset");
    });
}

int32_t se_automation_eval(const se_auto_lane_desc* lane, const int64_t* frames, size_t count, float* out_values) {
    if (lane == nullptr || (count > 0 && (frames == nullptr || out_values == nullptr)))
        return fail(SE_ERR_INVALID_ARG, "NULL argument");
    if (const char* why = dstudio::validate_lane(*lane)) return fail(SE_ERR_INVALID_ARG, why);
    if (lane->point_count == 0) return fail(SE_ERR_INVALID_ARG, "lane has no points");
    std::vector<dstudio::AutoPoint> pts(lane->point_count);
    for (size_t i = 0; i < lane->point_count; ++i)
        pts[i] = dstudio::AutoPoint{lane->points[i].frame, lane->points[i].value, lane->points[i].curve};
    const dstudio::LaneView v{pts.data(), lane->point_count, lane->logarithmic != 0,
                              (lane->flags & SE_AUTO_FLAG_STEPPED) != 0};
    for (size_t i = 0; i < count; ++i) out_values[i] = dstudio::auto_eval(v, frames[i]);
    return SE_OK;
}

// ---- plugins

int32_t se_plugin_is_live(se_engine* engine, uint64_t device_id) {
    if (engine == nullptr || !engine->impl) return 0;
    try {
        return engine->impl->plugin_is_live(device_id) ? 1 : 0;
    } catch (...) {
        return 0;
    }
}

int32_t se_plugin_load_errors(se_engine* engine, se_plugin_message* out, size_t capacity, size_t* out_count) {
    SE_REQUIRE_ENGINE(engine);
    if (capacity > 0 && out == nullptr) return fail(SE_ERR_INVALID_ARG, "out is NULL");
    return guarded([&]() -> int32_t {
        std::vector<std::pair<uint64_t, std::string>> errs;
        engine->impl->plugin_load_errors(errs);
        for (std::size_t i = 0; i < errs.size() && i < capacity; ++i) {
            out[i] = se_plugin_message{};
            out[i].device_id = errs[i].first;
            const std::size_t n = std::min(errs[i].second.size(), sizeof out[i].message - 1);
            std::memcpy(out[i].message, errs[i].second.data(), n);
        }
        if (out_count != nullptr) *out_count = errs.size();
        return SE_OK;
    });
}

int32_t se_plugin_save_states(se_engine* engine, size_t* out_count) {
    SE_REQUIRE_ENGINE(engine);
    return guarded([&]() -> int32_t {
        const std::size_t n = engine->impl->plugin_save_states();
        if (out_count != nullptr) *out_count = n;
        return SE_OK;
    });
}

int32_t se_plugin_saved_state(se_engine* engine, size_t index, uint64_t* out_device_id, const uint8_t** out_data,
                              size_t* out_len) {
    SE_REQUIRE_ENGINE(engine);
    if (out_device_id == nullptr || out_data == nullptr || out_len == nullptr)
        return fail(SE_ERR_INVALID_ARG, "output pointer is NULL");
    return guarded([&]() -> int32_t {
        if (!engine->impl->plugin_saved_state(index, *out_device_id, *out_data, *out_len))
            return fail(SE_ERR_NOT_FOUND, "no saved state at this index");
        return SE_OK;
    });
}

int32_t se_plugin_save_state(se_engine* engine, uint64_t device_id) {
    SE_REQUIRE_ENGINE(engine);
    return guarded([&]() -> int32_t {
        std::string err;
        return check(engine->impl->plugin_save_state_of(device_id, err), err);
    });
}

#define SE_REQUIRE_MAIN_THREAD()                                                                                       \
    do {                                                                                                               \
        if (!dstudio::on_platform_main_thread()) return fail(SE_ERR_STATE, "plugin editors are main-thread only");      \
    } while (0)

int32_t se_plugin_editor_open(se_engine* engine, uint64_t device_id) {
    SE_REQUIRE_ENGINE(engine);
    SE_REQUIRE_MAIN_THREAD();
    return guarded([&] {
        std::string err;
        return check(engine->impl->plugin_editor_open(device_id, err), err);
    });
}

int32_t se_plugin_editor_close(se_engine* engine, uint64_t device_id) {
    SE_REQUIRE_ENGINE(engine);
    SE_REQUIRE_MAIN_THREAD();
    return guarded([&]() -> int32_t {
        engine->impl->plugin_editor_close(device_id);
        return SE_OK;
    });
}

int32_t se_plugin_editor_is_open(se_engine* engine, uint64_t device_id) {
    if (engine == nullptr || !engine->impl) return 0;
    try {
        return engine->impl->plugin_editor_is_open(device_id) ? 1 : 0;
    } catch (...) {
        return 0;
    }
}

int32_t se_plugin_latency(se_engine* engine, uint64_t device_id, uint32_t* out_frames) {
    SE_REQUIRE_ENGINE(engine);
    if (out_frames == nullptr) return fail(SE_ERR_INVALID_ARG, "out_frames is NULL");
    return guarded([&]() -> int32_t {
        if (!engine->impl->plugin_latency(device_id, *out_frames)) {
            *out_frames = 0;
            return fail(SE_ERR_NOT_FOUND, "no live plugin instance for this device");
        }
        return SE_OK;
    });
}

int32_t se_plugin_idle(se_engine* engine) {
    SE_REQUIRE_ENGINE(engine);
    return guarded([&]() -> int32_t {
        engine->impl->plugin_idle();
        return SE_OK;
    });
}

int32_t se_plugin_prepare(se_engine* engine, const se_device_desc* device, uint8_t instrument) {
    SE_REQUIRE_ENGINE(engine);
    if (device == nullptr) return fail(SE_ERR_INVALID_ARG, "device is NULL");
    if (device->device_id == 0 || device->plugin_id == nullptr || device->format > SE_PLUGIN_CLAP ||
        (device->state == nullptr && device->state_len > 0))
        return fail(SE_ERR_INVALID_ARG, "invalid plugin device description");
    return guarded([&] {
        std::string err;
        return check(engine->impl->plugin_prepare(*device, instrument != 0, nullptr, err), err);
    });
}

int32_t se_plugin_set_detached(se_engine* engine, uint8_t detached) {
    SE_REQUIRE_ENGINE(engine);
    return guarded([&]() -> int32_t {
        engine->impl->plugin_set_detached(detached != 0);
        return SE_OK;
    });
}

// ---- plugin parameters

namespace {
void copy_cstr(char* dst, std::size_t cap, const std::string& src) {
    if (cap == 0) return;
    std::size_t n = std::min(cap - 1, src.size());
    while (n > 0 && n < src.size() && (static_cast<unsigned char>(src[n]) & 0xC0) == 0x80) --n; // keep UTF-8 whole
    std::memcpy(dst, src.data(), n);
    dst[n] = '\0';
}
} // namespace

int32_t se_plugin_params(se_engine* engine, uint64_t device_id, se_plugin_param_info* out, size_t capacity,
                         size_t* out_count) {
    SE_REQUIRE_ENGINE(engine);
    if (capacity > 0 && out == nullptr) return fail(SE_ERR_INVALID_ARG, "out is NULL");
    return guarded([&]() -> int32_t {
        std::vector<dstudio::ParamInfo> list;
        std::string err;
        if (const int32_t rc = engine->impl->plugin_params(device_id, list, err); rc != SE_OK) return check(rc, err);
        for (std::size_t i = 0; i < list.size() && i < capacity; ++i) {
            se_plugin_param_info& o = out[i];
            o = se_plugin_param_info{};
            o.id = list[i].id;
            o.steps = list[i].steps;
            o.default_normalized = list[i].default_normalized;
            o.automatable = list[i].automatable ? 1 : 0;
            copy_cstr(o.name, sizeof o.name, list[i].name);
            copy_cstr(o.group, sizeof o.group, list[i].group);
        }
        if (out_count != nullptr) *out_count = list.size();
        return SE_OK;
    });
}

int32_t se_plugin_param_value(se_engine* engine, uint64_t device_id, uint32_t param_id, float* out_normalized) {
    SE_REQUIRE_ENGINE(engine);
    if (out_normalized == nullptr) return fail(SE_ERR_INVALID_ARG, "out_normalized is NULL");
    return guarded([&] {
        std::string err;
        return check(engine->impl->plugin_param_value(device_id, param_id, *out_normalized, err), err);
    });
}

int32_t se_plugin_param_text(se_engine* engine, uint64_t device_id, uint32_t param_id, float normalized, char* out,
                             size_t capacity) {
    SE_REQUIRE_ENGINE(engine);
    if (out == nullptr || capacity == 0) return fail(SE_ERR_INVALID_ARG, "out is NULL or capacity 0");
    if (!std::isfinite(normalized)) return fail(SE_ERR_INVALID_ARG, "normalized is not finite");
    return guarded([&] {
        std::string text, err;
        const int32_t rc = engine->impl->plugin_param_text(device_id, param_id, normalized, text, err);
        if (rc == SE_OK) copy_cstr(out, capacity, text);
        return check(rc, err);
    });
}

int32_t se_plugin_take_param_changes(se_engine* engine, se_plugin_param_change* out, size_t capacity,
                                     size_t* out_count) {
    SE_REQUIRE_ENGINE(engine);
    if (capacity > 0 && out == nullptr) return fail(SE_ERR_INVALID_ARG, "out is NULL");
    const std::size_t n = engine->impl->take_param_changes(out, capacity);
    if (out_count != nullptr) *out_count = n;
    return SE_OK;
}

// ---- built-in devices

const se_builtin_desc* se_builtin_registry(size_t* out_count) {
    std::size_t n = 0;
    const se_builtin_desc* list = dstudio::builtin_registry_c(n);
    if (out_count != nullptr) *out_count = n;
    return list;
}

int32_t se_builtin_set_param(se_engine* engine, uint64_t device_id, uint32_t index, float value) {
    SE_REQUIRE_ENGINE(engine);
    return guarded([&]() -> int32_t {
        const int32_t rc = engine->impl->builtin_set_param(device_id, index, value);
        if (rc == SE_ERR_NOT_FOUND) return fail(rc, "no live built-in device with this id");
        if (rc == SE_ERR_INVALID_ARG) return fail(rc, "parameter index out of range or value not finite");
        return rc;
    });
}

int32_t se_plugin_set_param(se_engine* engine, uint64_t device_id, uint32_t param_id, float normalized) {
    SE_REQUIRE_ENGINE(engine);
    if (!std::isfinite(normalized)) return fail(SE_ERR_INVALID_ARG, "normalized is not finite");
    return guarded([&]() -> int32_t {
        const int32_t rc = engine->impl->plugin_set_param(device_id, param_id, normalized);
        if (rc == SE_ERR_NOT_FOUND) return fail(rc, "no live plugin instance with this parameter");
        if (rc == SE_ERR_STATE) return fail(rc, "the plugin's parameter queue is full");
        return rc;
    });
}

int32_t se_mod_base(se_engine* engine, uint64_t device_id, uint32_t param_id, float* out_normalized) {
    SE_REQUIRE_ENGINE(engine);
    if (out_normalized == nullptr) return fail(SE_ERR_INVALID_ARG, "out_normalized is NULL");
    return guarded([&]() -> int32_t {
        const int32_t rc = engine->impl->mod_base(device_id, param_id, *out_normalized);
        return rc == SE_OK ? rc : fail(rc, "the parameter is not modulated");
    });
}

int32_t se_builtin_readouts(se_engine* engine, uint64_t device_id, float* out, size_t capacity, size_t* out_count) {
    SE_REQUIRE_ENGINE(engine);
    if (capacity > 0 && out == nullptr) return fail(SE_ERR_INVALID_ARG, "out is NULL");
    return guarded([&]() -> int32_t {
        std::size_t n = 0;
        const int32_t rc = engine->impl->builtin_readouts(device_id, out, capacity, n);
        if (rc != SE_OK) return fail(rc, "no live built-in device with this id");
        if (out_count != nullptr) *out_count = n;
        return SE_OK;
    });
}

int32_t se_builtin_analysis(se_engine* engine, uint64_t device_id, float* out, size_t capacity, size_t* out_count,
                            uint64_t* out_seq) {
    SE_REQUIRE_ENGINE(engine);
    if (capacity > 0 && out == nullptr) return fail(SE_ERR_INVALID_ARG, "out is NULL");
    return guarded([&]() -> int32_t {
        std::size_t n = 0;
        uint64_t seq = 0;
        const int32_t rc = engine->impl->builtin_analysis(device_id, out, capacity, n, seq);
        if (rc != SE_OK) return fail(rc, "no live built-in device with this id");
        if (out_count != nullptr) *out_count = n;
        if (out_seq != nullptr) *out_seq = seq;
        return SE_OK;
    });
}

int32_t se_builtin_set_metering(se_engine* engine, uint64_t device_id, uint8_t on) {
    SE_REQUIRE_ENGINE(engine);
    return guarded([&]() -> int32_t {
        const int32_t rc = engine->impl->builtin_set_metering(device_id, on != 0);
        return rc == SE_OK ? rc : fail(rc, "no live built-in device with this id");
    });
}

int32_t se_builtin_levels(se_engine* engine, uint64_t device_id, float* out) {
    SE_REQUIRE_ENGINE(engine);
    if (out == nullptr) return fail(SE_ERR_INVALID_ARG, "out is NULL");
    return guarded([&]() -> int32_t {
        const int32_t rc = engine->impl->builtin_levels(device_id, out);
        return rc == SE_OK ? rc : fail(rc, "no live built-in device with this id");
    });
}

int32_t se_device_set_output_metering(se_engine* engine, uint64_t device_id, uint8_t on) {
    SE_REQUIRE_ENGINE(engine);
    return guarded([&]() -> int32_t {
        const int32_t rc = engine->impl->device_set_output_metering(device_id, on != 0);
        return rc == SE_OK ? rc : fail(rc, "no live device with this id");
    });
}

int32_t se_device_output_peaks(se_engine* engine, const uint64_t* device_ids, size_t count, float* out) {
    SE_REQUIRE_ENGINE(engine);
    if (count > 0 && (device_ids == nullptr || out == nullptr)) return fail(SE_ERR_INVALID_ARG, "device_ids or out is NULL");
    return guarded([&]() -> int32_t {
        engine->impl->device_output_peaks(device_ids, count, out);
        return SE_OK;
    });
}

int32_t se_builtin_correlation(se_engine* engine, uint64_t device_id, float* out) {
    SE_REQUIRE_ENGINE(engine);
    if (out == nullptr) return fail(SE_ERR_INVALID_ARG, "out is NULL");
    return guarded([&]() -> int32_t {
        const int32_t rc = engine->impl->builtin_correlation(device_id, out);
        return rc == SE_OK ? rc : fail(rc, "no live built-in device with this id");
    });
}

int32_t se_builtin_set_spectrum(se_engine* engine, uint64_t device_id, uint8_t mode) {
    SE_REQUIRE_ENGINE(engine);
    if (mode > 2) return fail(SE_ERR_INVALID_ARG, "mode must be 0 (off), 1 (pre) or 2 (post)");
    return guarded([&]() -> int32_t {
        const int32_t rc = engine->impl->builtin_set_spectrum(device_id, mode);
        return rc == SE_OK ? rc : fail(rc, "no live built-in device with this id");
    });
}

int32_t se_builtin_spectrum(se_engine* engine, uint64_t device_id, float* out, size_t capacity, size_t* out_count,
                            uint64_t* out_seq) {
    SE_REQUIRE_ENGINE(engine);
    if ((out == nullptr && capacity > 0) || out_count == nullptr || out_seq == nullptr)
        return fail(SE_ERR_INVALID_ARG, "out, out_count or out_seq is NULL");
    return guarded([&]() -> int32_t {
        std::size_t count = 0;
        uint64_t seq = 0;
        const int32_t rc = engine->impl->builtin_spectrum(device_id, out, capacity, count, seq);
        if (rc != SE_OK) return fail(rc, "no live built-in device with this id");
        *out_count = count;
        *out_seq = seq;
        return SE_OK;
    });
}

int32_t se_builtin_note(se_engine* engine, uint64_t device_id, uint8_t pitch, uint8_t velocity) {
    SE_REQUIRE_ENGINE(engine);
    return guarded([&]() -> int32_t {
        const int32_t rc = engine->impl->builtin_note(device_id, pitch, velocity);
        if (rc == SE_ERR_NOT_FOUND) return fail(rc, "no live built-in instrument with this id");
        if (rc == SE_ERR_INVALID_ARG) return fail(rc, "pitch or velocity above 127");
        if (rc == SE_ERR_STATE) return fail(rc, "too many queued notes");
        return rc;
    });
}

// ---- plugin delay compensation

int32_t se_pdc_set_enabled(se_engine* engine, uint8_t enabled) {
    SE_REQUIRE_ENGINE(engine);
    return guarded([&]() -> int32_t {
        engine->impl->set_pdc_enabled(enabled != 0);
        return SE_OK;
    });
}

int32_t se_pdc_is_enabled(const se_engine* engine) {
    if (engine == nullptr || !engine->impl) return 0;
    return engine->impl->pdc_enabled() ? 1 : 0;
}

int32_t se_pdc_report_get(se_engine* engine, se_pdc_report* out, se_pdc_track* tracks, size_t capacity,
                          size_t* out_count) {
    SE_REQUIRE_ENGINE(engine);
    if (out == nullptr) return fail(SE_ERR_INVALID_ARG, "out is NULL");
    if (capacity > 0 && tracks == nullptr) return fail(SE_ERR_INVALID_ARG, "tracks is NULL");
    return guarded([&]() -> int32_t {
        std::vector<se_pdc_track> list;
        engine->impl->pdc_report(*out, list);
        for (std::size_t i = 0; i < list.size() && i < capacity; ++i) tracks[i] = list[i];
        if (out_count != nullptr) *out_count = list.size();
        return SE_OK;
    });
}

int32_t se_transport_audible_position(const se_engine* engine, int64_t* out_frame) {
    SE_REQUIRE_ENGINE(engine);
    if (out_frame == nullptr) return fail(SE_ERR_INVALID_ARG, "out_frame is NULL");
    *out_frame = engine->impl->audible_position();
    return SE_OK;
}

int32_t se_device_list(se_device_info* out, size_t capacity, size_t* out_count) {
    if (capacity > 0 && out == nullptr) return fail(SE_ERR_INVALID_ARG, "out is NULL");
    return guarded([&]() -> int32_t {
        std::vector<dstudio::DeviceInfo> list;
        std::string err;
        const int32_t rc = dstudio::list_audio_devices(list, err);
        if (rc != SE_OK) return fail_from_err(rc, err);
        for (std::size_t i = 0; i < list.size() && i < capacity; ++i) {
            const dstudio::DeviceInfo& d = list[i];
            se_device_info& o = out[i];
            o = se_device_info{};
            std::snprintf(o.uid, sizeof o.uid, "%s", d.uid.c_str());
            std::snprintf(o.name, sizeof o.name, "%s", d.name.c_str());
            o.input_channels = d.input_channels;
            o.output_channels = d.output_channels;
            o.is_default_input = d.is_default_input ? 1 : 0;
            o.is_default_output = d.is_default_output ? 1 : 0;
            for (double r : d.sample_rates)
                if (o.sample_rate_count < SE_DEVICE_MAX_RATES) o.sample_rates[o.sample_rate_count++] = r;
        }
        if (out_count != nullptr) *out_count = list.size();
        return SE_OK;
    });
}

int32_t se_device_configure(se_engine* engine, const se_device_config* config) {
    SE_REQUIRE_ENGINE(engine);
    if (config == nullptr) return fail(SE_ERR_INVALID_ARG, "config is NULL");
    return guarded([&] {
        dstudio::AudioDevice::Config c;
        c.output_uid = config->output_uid != nullptr ? config->output_uid : "";
        c.input_enabled = config->input_enabled != 0;
        c.input_uid = c.input_enabled && config->input_uid != nullptr ? config->input_uid : "";
        c.device_rate = config->sample_rate;
        c.buffer_size = config->buffer_size;
        std::string err;
        return check(engine->impl->device_configure(c, err), err);
    });
}

int32_t se_device_status_get(se_engine* engine, se_device_status* out) {
    SE_REQUIRE_ENGINE(engine);
    if (out == nullptr) return fail(SE_ERR_INVALID_ARG, "out is NULL");
    return guarded([&] {
        engine->impl->device_status(out);
        return int32_t{SE_OK};
    });
}

static bool preview_args_ok(float gain, float rate) {
    return std::isfinite(gain) && gain >= 0.0f && gain <= 2.0f && std::isfinite(rate) && rate >= 0.25f && rate <= 4.0f;
}

int32_t se_preview_play(se_engine* engine, uint64_t asset_id, int64_t start_frame, float gain, float rate, uint8_t solo) {
    SE_REQUIRE_ENGINE(engine);
    if (!preview_args_ok(gain, rate)) return fail(SE_ERR_INVALID_ARG, "preview gain must be 0..2 and rate 0.25..4");
    return guarded([&] {
        const int32_t rc = engine->impl->preview_play(asset_id, start_frame, gain, rate, solo != 0);
        return rc == SE_ERR_NOT_FOUND ? fail(rc, "unknown asset") : rc;
    });
}

int32_t se_preview_stop(se_engine* engine) {
    SE_REQUIRE_ENGINE(engine);
    return guarded([&] { return engine->impl->preview_stop(); });
}

int32_t se_preview_note(se_engine* engine, uint64_t device_id, uint8_t pitch, uint8_t velocity) {
    SE_REQUIRE_ENGINE(engine);
    if (pitch > 127 || velocity > 127) return fail(SE_ERR_INVALID_ARG, "pitch and velocity must be 0..127");
    return guarded([&] {
        const int32_t rc = engine->impl->preview_note(device_id, pitch, velocity);
        if (rc == SE_ERR_NOT_FOUND) return fail(rc, "unknown device, or one that plays no notes");
        return rc == SE_ERR_STATE ? fail(rc, "live note queue full") : rc;
    });
}

int32_t se_preview_set(se_engine* engine, float gain, float rate, uint8_t solo) {
    SE_REQUIRE_ENGINE(engine);
    if (!preview_args_ok(gain, rate)) return fail(SE_ERR_INVALID_ARG, "preview gain must be 0..2 and rate 0.25..4");
    return guarded([&] { return engine->impl->preview_set(gain, rate, solo != 0); });
}

int32_t se_preview_get(se_engine* engine, se_preview_info* out) {
    SE_REQUIRE_ENGINE(engine);
    if (out == nullptr) return fail(SE_ERR_INVALID_ARG, "out is NULL");
    engine->impl->preview_get(out);
    return SE_OK;
}

int32_t se_input_set_muted(se_engine* engine, uint8_t muted) {
    SE_REQUIRE_ENGINE(engine);
    engine->impl->set_input_muted(muted != 0);
    return SE_OK;
}

int32_t se_record_set_offset(se_engine* engine, int32_t frames) {
    SE_REQUIRE_ENGINE(engine);
    const double limit = engine->impl->sample_rate();
    if (static_cast<double>(frames) > limit || static_cast<double>(frames) < -limit)
        return fail(SE_ERR_INVALID_ARG, "recording offset beyond one second");
    engine->impl->set_record_offset(frames);
    return SE_OK;
}

int32_t se_record_get_offset(se_engine* engine, int32_t* out_frames) {
    SE_REQUIRE_ENGINE(engine);
    if (out_frames == nullptr) return fail(SE_ERR_INVALID_ARG, "out_frames is NULL");
    *out_frames = static_cast<int32_t>(engine->impl->record_offset());
    return SE_OK;
}

int32_t se_record_start(se_engine* engine, const char* utf8_dir) {
    SE_REQUIRE_ENGINE(engine);
    if (utf8_dir == nullptr || utf8_dir[0] == '\0') return fail(SE_ERR_INVALID_ARG, "directory is NULL or empty");
    return guarded([&] {
        std::string err;
        return check(engine->impl->record_start(utf8_dir, err), err);
    });
}

int32_t se_record_stop(se_engine* engine, size_t* out_take_count) {
    SE_REQUIRE_ENGINE(engine);
    return guarded([&] {
        std::string err;
        return check(engine->impl->record_stop(out_take_count, err), err);
    });
}

int32_t se_record_get_take(se_engine* engine, size_t index, se_recorded_take* out) {
    SE_REQUIRE_ENGINE(engine);
    if (out == nullptr) return fail(SE_ERR_INVALID_ARG, "out is NULL");
    return guarded([&] {
        const int32_t rc = engine->impl->record_get_take(index, out);
        return rc == SE_OK ? SE_OK : fail(rc, "no take " + std::to_string(index));
    });
}

int32_t se_record_state(se_engine* engine, uint8_t* out_active, se_live_take* out, size_t capacity,
                        size_t* out_count) {
    SE_REQUIRE_ENGINE(engine);
    if (capacity > 0 && out == nullptr) return fail(SE_ERR_INVALID_ARG, "out is NULL");
    return guarded([&] {
        bool active = false;
        std::vector<dstudio::LiveTakeInfo> takes;
        engine->impl->record_state(active, takes);
        if (out_active != nullptr) *out_active = active ? 1 : 0;
        for (std::size_t i = 0; i < takes.size() && i < capacity; ++i)
            out[i] = se_live_take{takes[i].track_id, takes[i].start_frame, takes[i].frames};
        if (out_count != nullptr) *out_count = takes.size();
        return int32_t{SE_OK};
    });
}

int32_t se_record_peaks(se_engine* engine, uint64_t track_id, uint32_t frames_per_peak, float* out_minmax,
                        size_t capacity, size_t* out_count) {
    SE_REQUIRE_ENGINE(engine);
    if (frames_per_peak == 0) return fail(SE_ERR_INVALID_ARG, "frames_per_peak must be > 0");
    if (capacity > 0 && out_minmax == nullptr) return fail(SE_ERR_INVALID_ARG, "out_minmax is NULL");
    return guarded([&] {
        const std::size_t n = engine->impl->record_peaks(track_id, frames_per_peak, out_minmax, capacity);
        if (out_count != nullptr) *out_count = n;
        return int32_t{SE_OK};
    });
}

int32_t se_record_stats_get(se_engine* engine, se_record_stats* out) {
    SE_REQUIRE_ENGINE(engine);
    if (out == nullptr) return fail(SE_ERR_INVALID_ARG, "out is NULL");
    return guarded([&] {
        const dstudio::RecordStats s = engine->impl->record_stats();
        *out = se_record_stats{s.overruns, s.dropped_frames, s.write_errors, s.takes};
        return int32_t{SE_OK};
    });
}

int32_t se_render_offline_with_input(se_engine* engine, const float* input_interleaved, uint32_t input_channels,
                                     float* out_interleaved, uint32_t frames) {
    SE_REQUIRE_ENGINE(engine);
    if (frames > 0 && (out_interleaved == nullptr || input_interleaved == nullptr))
        return fail(SE_ERR_INVALID_ARG, "input or output buffer is NULL");
    return guarded([&] {
        std::string err;
        return check(engine->impl->render_offline_with_input(input_interleaved, input_channels, out_interleaved,
                                                             frames, err),
                     err);
    });
}

/* ---- MIDI input ---- */

static_assert(sizeof(se_midi_input_info) == 528 && alignof(se_midi_input_info) == 8);
static_assert(sizeof(se_midi_captured) == 16 && alignof(se_midi_captured) == 8);
static_assert(sizeof(se_midi_stats) == 32);

int32_t se_midi_inputs_list(se_engine* engine, se_midi_input_info* out, size_t capacity, size_t* out_count) {
    SE_REQUIRE_ENGINE(engine);
    if (capacity > 0 && out == nullptr) return fail(SE_ERR_INVALID_ARG, "out is NULL");
    return guarded([&]() -> int32_t {
        std::vector<dstudio::MidiPortState> ports;
        engine->impl->midi().list(ports);
        for (std::size_t i = 0; i < ports.size() && i < capacity; ++i) {
            se_midi_input_info& o = out[i];
            o = se_midi_input_info{};
            std::snprintf(o.id, sizeof o.id, "%s", ports[i].info.id.c_str());
            std::snprintf(o.name, sizeof o.name, "%s", ports[i].info.name.c_str());
            o.events = ports[i].events;
            o.open = ports[i].open ? 1 : 0;
        }
        if (out_count != nullptr) *out_count = ports.size();
        return SE_OK;
    });
}

int32_t se_midi_inputs_open(se_engine* engine, const char* const* ids, size_t count) {
    SE_REQUIRE_ENGINE(engine);
    if (count > 0 && ids == nullptr) return fail(SE_ERR_INVALID_ARG, "ids is NULL");
    return guarded([&]() -> int32_t {
        std::vector<std::string> v;
        for (size_t i = 0; i < count; ++i) {
            if (ids[i] == nullptr) return fail(SE_ERR_INVALID_ARG, "an id is NULL");
            v.emplace_back(ids[i]);
        }
        std::string err;
        return check(engine->impl->midi_open(v, err), err);
    });
}

int32_t se_midi_inject(se_engine* engine, uint8_t status, uint8_t data1, uint8_t data2) {
    SE_REQUIRE_ENGINE(engine);
    if (status < 0x80 || status >= 0xF0 || data1 > 127 || data2 > 127)
        return fail(SE_ERR_INVALID_ARG, "not a MIDI channel message");
    return engine->impl->midi().inject(status, data1, data2) ? SE_OK : fail(SE_ERR_STATE, "MIDI input queue is full");
}

int32_t se_midi_set_focus(se_engine* engine, uint64_t track_id) {
    SE_REQUIRE_ENGINE(engine);
    engine->impl->midi().set_focus(track_id);
    return SE_OK;
}

int32_t se_midi_capture_enable(se_engine* engine, uint8_t enabled) {
    SE_REQUIRE_ENGINE(engine);
    engine->impl->midi().capture_enable(enabled != 0);
    return SE_OK;
}

int32_t se_midi_capture_read(se_engine* engine, se_midi_captured* out, size_t capacity, size_t* out_count) {
    SE_REQUIRE_ENGINE(engine);
    if ((capacity > 0 && out == nullptr) || out_count == nullptr) return fail(SE_ERR_INVALID_ARG, "out is NULL");
    *out_count = engine->impl->midi().capture_read(out, capacity);
    return SE_OK;
}

int32_t se_midi_capture_wrapped(se_engine* engine, uint8_t* out_wrapped) {
    SE_REQUIRE_ENGINE(engine);
    if (out_wrapped == nullptr) return fail(SE_ERR_INVALID_ARG, "out_wrapped is NULL");
    *out_wrapped = engine->impl->midi().capture_wrapped() ? 1 : 0;
    return SE_OK;
}

int32_t se_midi_stats_get(se_engine* engine, se_midi_stats* out) {
    SE_REQUIRE_ENGINE(engine);
    if (out == nullptr) return fail(SE_ERR_INVALID_ARG, "out is NULL");
    engine->impl->midi().stats(out);
    return SE_OK;
}

} // extern "C"

namespace dstudio::testing {

void set_device_factory(se_engine* e, std::function<std::unique_ptr<AudioDevice>()> factory) {
    e->impl->set_device_factory(std::move(factory));
}
void set_record_writer_stalled(se_engine* e, bool stalled) { e->impl->set_record_writer_stalled(stalled); }
void set_record_ring_frames(se_engine* e, std::size_t frames) { e->impl->set_record_ring_frames(frames); }
void set_record_max_take_bytes(se_engine* e, uint64_t bytes) { e->impl->set_record_max_take_bytes(bytes); }
uint64_t plugin_instantiations(se_engine* e) { return e->impl->plugin_instantiations(); }
void set_warp_renders_enabled(se_engine* e, bool enabled) { e->impl->set_warp_renders_enabled(enabled); }
bool plugin_set_parameter(se_engine* e, uint64_t device_id, uint32_t param_id, double value) {
    return e->impl->plugin_set_parameter(device_id, param_id, value);
}
bool plugin_simulate_gesture(se_engine* e, uint64_t device_id, uint32_t param_id, float normalized) {
    return e->impl->plugin_simulate_gesture(device_id, param_id, normalized);
}

void set_midi_backend(se_engine* e, std::unique_ptr<MidiBackend> backend) { e->impl->set_midi_backend(std::move(backend)); }

} // namespace dstudio::testing
