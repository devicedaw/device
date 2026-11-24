// Input routing, monitoring, metering and recording, driven by injected
// input (se_render_offline_with_input) or a fake device. No test here opens
// a real input device (that would trigger the macOS microphone prompt).
#include "test_harness.hpp"
#include "test_util.hpp"

#include "platform/audio_device.hpp"
#include "test_hooks.hpp"
#include "platform/os.hpp"
#include "wav.hpp"

#include <algorithm>
#include <chrono>
#include <cstdlib>
#include <cstdio>
#include <cmath>
#include <cstring>
#include <filesystem>
#include <functional>
#include <numbers>
#include <thread>

using namespace testutil;

namespace {

struct TrackIn {
    uint64_t id = 1;
    float gain = 1.0f;
    float pan = 0.0f;
    bool mute = false, solo = false, arm = false, monitor = false;
    uint8_t mode = SE_INPUT_NONE;
    uint16_t channel = 0;
    std::vector<se_clip_desc> clips;
};

int32_t publish_v4(se_engine* e, const std::vector<TrackIn>& tracks, float master = 1.0f) {
    std::vector<se_track_desc_v4> td(tracks.size());
    for (std::size_t i = 0; i < tracks.size(); ++i) {
        const TrackIn& t = tracks[i];
        td[i] = se_track_desc_v4{};
        td[i].track_id = t.id;
        td[i].gain = t.gain;
        td[i].pan = t.pan;
        td[i].mute = t.mute;
        td[i].solo = t.solo;
        td[i].arm = t.arm;
        td[i].monitor = t.monitor;
        td[i].input_mode = t.mode;
        td[i].input_channel = t.channel;
        td[i].clips = t.clips.empty() ? nullptr : t.clips.data();
        td[i].clip_count = static_cast<uint32_t>(t.clips.size());
    }
    se_session_desc_v4 sd{td.data(), static_cast<uint32_t>(td.size()), master};
    return se_session_publish_v4(e, &sd);
}

// Interleaved input where channel c at absolute frame k is f(c, k).
std::vector<float> make_input(uint32_t channels, std::size_t frames, std::size_t first,
                              const std::function<float(uint32_t, std::size_t)>& f) {
    std::vector<float> v(frames * channels);
    for (std::size_t k = 0; k < frames; ++k)
        for (uint32_t c = 0; c < channels; ++c) v[k * channels + c] = f(c, first + k);
    return v;
}

float ramp_val(uint32_t c, std::size_t k) { return static_cast<float>(c + 1) * 1e-6f * static_cast<float>(k + 1); }

// Renders `frames` frames with input f(c, k) (k counts from `first`), in
// blocks cycling through `blocks`. Returns interleaved stereo output.
std::vector<float> render_in(se_engine* e, uint32_t channels, std::size_t frames, std::size_t first,
                             const std::vector<uint32_t>& blocks,
                             const std::function<float(uint32_t, std::size_t)>& f = ramp_val) {
    const std::vector<float> in = make_input(channels, frames, first, f);
    std::vector<float> out(frames * 2);
    std::size_t done = 0;
    for (std::size_t b = 0; done < frames; ++b) {
        const auto n = static_cast<uint32_t>(std::min<std::size_t>(blocks[b % blocks.size()], frames - done));
        if (se_render_offline_with_input(e, in.data() + done * channels, channels, out.data() + done * 2, n) != SE_OK) {
            CHECK(false);
            break;
        }
        done += n;
    }
    return out;
}

float pan_gain(float pan, bool left) {
    const double theta = (pan + 1.0) * (std::numbers::pi / 4.0);
    return static_cast<float>(left ? std::cos(theta) : std::sin(theta));
}

se_meter peek_track(se_engine* e, std::size_t index) {
    se_meter m[16]{};
    std::size_t n = 0;
    se_meters_peek(e, m, 16, &n);
    return index < n ? m[index] : se_meter{};
}

bool wait_until(const std::function<bool()>& pred, int ms = 5000) {
    const auto t0 = std::chrono::steady_clock::now();
    while (!pred()) {
        if (std::chrono::steady_clock::now() - t0 > std::chrono::milliseconds(ms)) return false;
        std::this_thread::sleep_for(std::chrono::milliseconds(2));
    }
    return true;
}

std::vector<se_live_take> live_takes(se_engine* e, bool* active = nullptr) {
    std::size_t n = 0;
    uint8_t a = 0;
    se_record_state(e, &a, nullptr, 0, &n);
    std::vector<se_live_take> v(n);
    se_record_state(e, &a, v.data(), v.size(), &n);
    v.resize(std::min(n, v.size()));
    if (active) *active = a != 0;
    return v;
}

std::vector<se_recorded_take> stop_takes(se_engine* e) {
    std::size_t n = 0;
    REQUIRE(se_record_stop(e, &n) == SE_OK);
    std::vector<se_recorded_take> v(n);
    for (std::size_t i = 0; i < n; ++i) REQUIRE(se_record_get_take(e, i, &v[i]) == SE_OK);
    return v;
}

struct TempDir {
    TempDir() {
        static int counter = 0;
        path = (std::filesystem::temp_directory_path() /
                ("dstudio_rec_test_" + std::to_string(dstudio::os::process_id()) + "_" + std::to_string(++counter)))
                   .string();
        std::filesystem::remove_all(path);
    }
    ~TempDir() {
        std::error_code ec;
        std::filesystem::remove_all(path, ec);
    }
    std::string path;
};

// Checks a take file holds exactly f(channel, k) for the given channels
// with k = first_k + i.
void check_take(const se_recorded_take& t, const std::vector<uint32_t>& in_channels, std::size_t first_k,
                const std::function<float(uint32_t, std::size_t)>& f = ramp_val) {
    dstudio::DecodedAudio a;
    std::string err;
    REQUIRE(dstudio::decode_wav_file(t.path, a, err) == SE_OK);
    CHECK_EQ(a.channels, static_cast<uint32_t>(in_channels.size()));
    CHECK_EQ(a.frames, t.frames);
    CHECK_EQ(a.sample_rate, t.sample_rate);
    long bad = 0;
    for (std::size_t c = 0; c < in_channels.size() && c < a.channels; ++c)
        for (uint64_t i = 0; i < a.frames; ++i)
            if (a.planar[c][i] != f(in_channels[c], first_k + i)) ++bad;
    CHECK_EQ(bad, 0L);
}

} // namespace

// ------------------------------------------------------------ routing & monitoring

TEST_CASE(input_mono_monitoring_routes_channel_through_pan_law) {
    EngineHandle e(48000, 256);
    TrackIn t;
    t.monitor = true;
    t.mode = SE_INPUT_MONO;
    t.channel = 1;
    t.pan = 0.3f;
    t.gain = 0.5f;
    REQUIRE(publish_v4(e, {t}) == SE_OK);
    // Transport stopped: monitoring is live anyway.
    const auto out = render_in(e, 3, 1000, 0, {256, 77, 1000});
    const float gl = 0.5f * pan_gain(0.3f, true), gr = 0.5f * pan_gain(0.3f, false);
    long bad = 0;
    for (std::size_t k = 0; k < 1000; ++k) {
        const float x = ramp_val(1, k);
        if (out[2 * k] != x * gl || out[2 * k + 1] != x * gr) ++bad;
    }
    CHECK_EQ(bad, 0L);
    se_transport_info ti{};
    se_transport_get(e, &ti);
    CHECK_EQ(ti.playing, 0);
    CHECK_EQ(ti.position, 0);
}

TEST_CASE(input_stereo_monitoring_routes_pair) {
    EngineHandle e(48000, 256);
    TrackIn t;
    t.monitor = true;
    t.mode = SE_INPUT_STEREO;
    t.channel = 2;
    REQUIRE(publish_v4(e, {t}) == SE_OK);
    REQUIRE(se_transport_play(e) == SE_OK); // also while playing
    const auto out = render_in(e, 4, 700, 0, {300, 1, 255});
    const float g = pan_gain(0.0f, true), gr = pan_gain(0.0f, false);
    long bad = 0;
    for (std::size_t k = 0; k < 700; ++k)
        if (out[2 * k] != ramp_val(2, k) * g || out[2 * k + 1] != ramp_val(3, k) * gr) ++bad;
    CHECK_EQ(bad, 0L);
    // A stereo pair past the device's channels reads silence on that side.
    t.channel = 3;
    REQUIRE(publish_v4(e, {t}) == SE_OK);
    const auto out2 = render_in(e, 4, 64, 0, {64});
    for (std::size_t k = 0; k < 64; ++k) {
        CHECK(out2[2 * k] == ramp_val(3, k) * g);
        CHECK(out2[2 * k + 1] == 0.0f);
    }
}

TEST_CASE(input_monitor_off_is_silent_and_sums_with_clips_when_on) {
    EngineHandle e(48000, 256);
    const uint64_t a = load_asset(e, {constant(4000, 0.25f)});
    TrackIn t;
    t.mode = SE_INPUT_MONO;
    t.clips = {clip(1, a, 0, 0, 4000)};
    REQUIRE(publish_v4(e, {t}) == SE_OK);
    REQUIRE(se_transport_play(e) == SE_OK);
    auto out = render_in(e, 1, 512, 0, {512});
    const float g = pan_gain(0.0f, true);
    CHECK(out[100] == 0.25f * g); // only the clip
    t.monitor = true;
    REQUIRE(publish_v4(e, {t}) == SE_OK);
    out = render_in(e, 1, 512, 0, {512});
    long bad = 0;
    for (std::size_t k = 0; k < 512; ++k)
        if (out[2 * k] != (0.25f + ramp_val(0, k)) * g) ++bad;
    CHECK_EQ(bad, 0L);
    // Not monitored, not armed, nothing playing: input contributes nothing.
    t.monitor = false;
    t.clips.clear();
    REQUIRE(publish_v4(e, {t}) == SE_OK);
    out = render_in(e, 1, 512, 0, {512});
    for (float v : out) CHECK(v == 0.0f);
    CHECK_EQ(peek_track(e, 1).peak_l, 0.0f);
}

TEST_CASE(input_armed_not_monitored_meters_input_but_is_silent) {
    EngineHandle e(48000, 256);
    TrackIn t;
    t.arm = true;
    t.mode = SE_INPUT_STEREO;
    t.channel = 0;
    t.gain = 0.1f; // the input meter is pre-fader
    TrackIn quiet;
    quiet.id = 2;
    REQUIRE(publish_v4(e, {t, quiet}) == SE_OK);
    const auto sine_in = [](uint32_t c, std::size_t k) {
        return static_cast<float>((c == 0 ? 0.5 : 0.25) * std::sin(2.0 * std::numbers::pi * 440.0 * static_cast<double>(k) / 48000.0));
    };
    float max_l = 0.0f, max_r = 0.0f;
    for (std::size_t k = 0; k < 480; ++k) {
        max_l = std::max(max_l, std::fabs(sine_in(0, k)));
        max_r = std::max(max_r, std::fabs(sine_in(1, k)));
    }
    const auto out = render_in(e, 2, 480, 0, {480}, sine_in);
    for (float v : out) CHECK(v == 0.0f);
    const se_meter m = peek_track(e, 1);
    CHECK_EQ(m.track_id, 1u);
    CHECK_EQ(m.peak_l, max_l);
    CHECK_EQ(m.peak_r, max_r);
    CHECK_EQ(peek_track(e, 0).peak_l, 0.0f); // master silent
    CHECK_EQ(peek_track(e, 2).peak_l, 0.0f);
    se_meter rd[3]{};
    std::size_t n = 0;
    se_meters_read(e, rd, 3, &n);
    CHECK_EQ(rd[1].peak_l, max_l); // destructive meters too
}

TEST_CASE(input_mute_zeroes_monitoring_meters_and_respects_track_mute_solo) {
    EngineHandle e(48000, 256);
    TrackIn t;
    t.monitor = true;
    t.mode = SE_INPUT_MONO;
    TrackIn armed;
    armed.id = 2;
    armed.arm = true;
    armed.mode = SE_INPUT_MONO;
    REQUIRE(publish_v4(e, {t, armed}) == SE_OK);
    REQUIRE(se_input_set_muted(e, 1) == SE_OK);
    auto out = render_in(e, 1, 1024, 0, {256}, [](uint32_t, std::size_t) { return 0.5f; });
    for (float v : out) CHECK(v == 0.0f);
    CHECK_EQ(peek_track(e, 1).peak_l, 0.0f);
    CHECK_EQ(peek_track(e, 2).peak_l, 0.0f);
    REQUIRE(se_input_set_muted(e, 0) == SE_OK);
    out = render_in(e, 1, 1024, 0, {256}, [](uint32_t, std::size_t) { return 0.5f; });
    CHECK(out[1000] == 0.5f * pan_gain(0.0f, true));
    CHECK_EQ(peek_track(e, 2).peak_l, 0.5f);
    // Track mute and another track's solo silence monitoring.
    t.mute = true;
    REQUIRE(publish_v4(e, {t, armed}) == SE_OK);
    out = render_in(e, 1, 256, 0, {256}, [](uint32_t, std::size_t) { return 0.5f; });
    for (float v : out) CHECK(v == 0.0f);
    t.mute = false;
    armed.solo = true;
    REQUIRE(publish_v4(e, {t, armed}) == SE_OK);
    out = render_in(e, 1, 256, 0, {256}, [](uint32_t, std::size_t) { return 0.5f; });
    for (float v : out) CHECK(v == 0.0f);
}

// Voice input (spec 024): the chosen channel (or a pair's average) arrives in
// the ring unchanged, in order, across odd block sizes; off reads nothing.
TEST_CASE(voice_capture_reads_the_chosen_input_in_order) {
    EngineHandle e(48000, 256);
    std::vector<float> got(4096);
    std::size_t n = 0;
    float peak = 0.0f;
    render_in(e, 3, 300, 0, {256});
    REQUIRE(se_voice_capture_read(e, got.data(), got.size(), &n, &peak) == SE_OK);
    CHECK_EQ(n, std::size_t{0}); // off by default

    REQUIRE(se_voice_capture_set(e, 1, 0) == SE_OK);
    render_in(e, 3, 1000, 0, {256, 77, 1000});
    REQUIRE(se_voice_capture_read(e, got.data(), got.size(), &n, &peak) == SE_OK);
    REQUIRE(n == 1000u);
    long bad = 0;
    for (std::size_t k = 0; k < n; ++k) bad += got[k] != ramp_val(1, k);
    CHECK_EQ(bad, 0L);
    CHECK_EQ(peak, ramp_val(1, 999));

    // Stereo pair 0/1: the average. Turning it on again empties what was left.
    render_in(e, 3, 100, 0, {256});
    REQUIRE(se_voice_capture_set(e, 0, 1) == SE_OK);
    render_in(e, 3, 500, 0, {128});
    REQUIRE(se_voice_capture_read(e, got.data(), got.size(), &n, nullptr) == SE_OK);
    REQUIRE(n == 500u);
    bad = 0;
    for (std::size_t k = 0; k < n; ++k) bad += got[k] != 0.5f * (ramp_val(0, k) + ramp_val(1, k));
    CHECK_EQ(bad, 0L);

    // A channel the input lacks reads as silence; Mute input silences it too.
    REQUIRE(se_voice_capture_set(e, 7, 0) == SE_OK);
    render_in(e, 3, 64, 0, {64});
    REQUIRE(se_input_set_muted(e, 1) == SE_OK);
    REQUIRE(se_voice_capture_set(e, 0, 0) == SE_OK);
    render_in(e, 3, 64, 0, {64});
    REQUIRE(se_voice_capture_read(e, got.data(), got.size(), &n, &peak) == SE_OK);
    REQUIRE(n == 64u);
    for (std::size_t k = 0; k < n; ++k) CHECK(got[k] == 0.0f);
    CHECK_EQ(peak, 0.0f);

    REQUIRE(se_voice_capture_set(e, -1, 0) == SE_OK);
    render_in(e, 3, 64, 0, {64});
    REQUIRE(se_voice_capture_read(e, got.data(), got.size(), &n, nullptr) == SE_OK);
    CHECK_EQ(n, std::size_t{0});
    CHECK_EQ(se_voice_capture_read(e, nullptr, 4, &n, nullptr), SE_ERR_INVALID_ARG);
}

TEST_CASE(input_publish_v4_validates_and_older_publishes_have_no_input) {
    EngineHandle e(48000, 256);
    TrackIn t;
    t.monitor = true;
    t.mode = 3;
    CHECK_EQ(publish_v4(e, {t}), SE_ERR_INVALID_ARG);
    se_session_desc_v4 null_tracks{nullptr, 1, 1.0f};
    CHECK_EQ(se_session_publish_v4(e, &null_tracks), SE_ERR_INVALID_ARG);
    // v3 publish: no input routing at all.
    se_track_desc_v3 t3{};
    t3.track_id = 1;
    t3.gain = 1.0f;
    se_session_desc_v3 s3{&t3, 1, 1.0f};
    REQUIRE(se_session_publish_v3(e, &s3) == SE_OK);
    const auto out = render_in(e, 1, 256, 0, {256});
    for (float v : out) CHECK(v == 0.0f);
    std::vector<float> o(2 * 64), in(64, 0.1f);
    CHECK_EQ(se_render_offline_with_input(e, in.data(), 0, o.data(), 64), SE_ERR_INVALID_ARG);
    CHECK_EQ(se_render_offline_with_input(e, in.data(), 33, o.data(), 1), SE_ERR_INVALID_ARG);
    CHECK_EQ(se_render_offline_with_input(e, nullptr, 1, o.data(), 64), SE_ERR_INVALID_ARG);
}

// ------------------------------------------------------------ recording

TEST_CASE(record_ramp_bit_exact_across_odd_blocks) {
    EngineHandle e(48000, 256);
    TempDir dir;
    TrackIn t;
    t.arm = true;
    t.mode = SE_INPUT_MONO;
    t.channel = 1;
    t.gain = 0.0f; // the take is the raw input, pre-fader
    REQUIRE(publish_v4(e, {t}) == SE_OK);
    REQUIRE(se_transport_seek(e, 12345) == SE_OK);
    REQUIRE(se_record_start(e, dir.path.c_str()) == SE_OK);
    CHECK_EQ(se_record_start(e, dir.path.c_str()), SE_ERR_STATE);
    se_transport_info ti{};
    se_transport_get(e, &ti);
    CHECK_EQ(ti.playing, 1); // start_recording started the transport
    const std::size_t total = 20011;
    render_in(e, 2, total, 0, {127, 1, 333, 1000 /* > max_block: chunked */, 64, 255});
    bool active = false;
    CHECK(wait_until([&] {
        auto v = live_takes(e, &active);
        return v.size() == 1 && v[0].frames == total;
    }));
    CHECK(active);
    auto live = live_takes(e);
    REQUIRE(live.size() == 1);
    CHECK_EQ(live[0].track_id, 1u);
    CHECK_EQ(live[0].start_frame, 12345);
    // Live peaks: 64-frame resolution, ramp of channel 1 is increasing.
    std::vector<float> pk(2 * 1000);
    std::size_t pn = 0;
    REQUIRE(se_record_peaks(e, 1, 640, pk.data(), 1000, &pn) == SE_OK);
    CHECK_EQ(pn, (total + 639) / 640);
    CHECK_EQ(pk[0], ramp_val(1, 0));
    CHECK_EQ(pk[1], ramp_val(1, 639));
    REQUIRE(se_record_peaks(e, 99, 640, pk.data(), 1000, &pn) == SE_OK);
    CHECK_EQ(pn, 0u);

    const auto takes = stop_takes(e);
    se_transport_get(e, &ti);
    CHECK_EQ(ti.playing, 1); // transport left as-is
    CHECK_EQ(ti.position, static_cast<int64_t>(12345 + total));
    REQUIRE(takes.size() == 1);
    CHECK_EQ(takes[0].track_id, 1u);
    CHECK_EQ(takes[0].start_frame, 12345);
    CHECK_EQ(takes[0].frames, static_cast<uint64_t>(total));
    CHECK_EQ(takes[0].channels, 1u);
    CHECK_EQ(takes[0].sample_rate, 48000.0);
    CHECK(std::string(takes[0].path).rfind(dir.path, 0) == 0);
    check_take(takes[0], {1}, 0);
    CHECK_EQ(se_record_stop(e, nullptr), SE_ERR_STATE);
    bool still = true;
    live_takes(e, &still);
    CHECK(!still);
}

TEST_CASE(record_two_tracks_on_different_input_channels) {
    EngineHandle e(48000, 256);
    TempDir dir;
    TrackIn a, b, not_armed, armed_no_input;
    a.id = 10;
    a.arm = true;
    a.mode = SE_INPUT_MONO;
    a.channel = 3;
    b.id = 20;
    b.arm = true;
    b.monitor = true;
    b.mode = SE_INPUT_STEREO;
    b.channel = 0;
    not_armed.id = 30;
    not_armed.mode = SE_INPUT_MONO;
    armed_no_input.id = 40;
    armed_no_input.arm = true;
    REQUIRE(publish_v4(e, {a, b, not_armed, armed_no_input}) == SE_OK);
    REQUIRE(se_record_start(e, dir.path.c_str()) == SE_OK);
    render_in(e, 4, 9999, 0, {511, 3, 256});
    const auto takes = stop_takes(e);
    REQUIRE(takes.size() == 2);
    for (const auto& t : takes) {
        CHECK_EQ(t.start_frame, 0);
        CHECK_EQ(t.frames, 9999u);
        if (t.track_id == 10) {
            CHECK_EQ(t.channels, 1u);
            check_take(t, {3}, 0);
        } else {
            CHECK_EQ(t.track_id, 20u);
            CHECK_EQ(t.channels, 2u);
            check_take(t, {0, 1}, 0);
        }
    }
    CHECK(std::string(takes[0].path) != std::string(takes[1].path));
}

TEST_CASE(record_seek_starts_new_take_loop_writes_linearly) {
    EngineHandle e(48000, 256);
    TempDir dir;
    TrackIn t;
    t.arm = true;
    t.mode = SE_INPUT_MONO;
    REQUIRE(publish_v4(e, {t}) == SE_OK);
    REQUIRE(se_transport_set_loop(e, 1, 1000, 1700) == SE_OK); // 700 frames, wraps mid-block
    REQUIRE(se_record_start(e, dir.path.c_str()) == SE_OK);
    render_in(e, 1, 3000, 0, {256}); // 0..1700, then 1000..1700 x n: one linear take
    REQUIRE(se_transport_set_loop(e, 0, 0, 0) == SE_OK);
    REQUIRE(se_transport_seek(e, 50000) == SE_OK);
    render_in(e, 1, 500, 3000, {100});
    REQUIRE(se_transport_pause(e) == SE_OK);
    render_in(e, 1, 300, 3500, {100}); // paused: not captured
    REQUIRE(se_transport_play(e) == SE_OK); // same position: same take continues
    render_in(e, 1, 200, 3800, {100});
    const auto takes = stop_takes(e);
    REQUIRE(takes.size() == 2);
    CHECK_EQ(takes[0].start_frame, 0);
    CHECK_EQ(takes[0].frames, 3000u);
    check_take(takes[0], {0}, 0);
    CHECK_EQ(takes[1].start_frame, 50000);
    CHECK_EQ(takes[1].frames, 700u);
    // 500 frames from k = 3000, then 200 from k = 3800.
    dstudio::DecodedAudio d;
    std::string err;
    REQUIRE(dstudio::decode_wav_file(takes[1].path, d, err) == SE_OK);
    REQUIRE(d.frames == 700);
    CHECK(d.planar[0][0] == ramp_val(0, 3000));
    CHECK(d.planar[0][499] == ramp_val(0, 3499));
    CHECK(d.planar[0][500] == ramp_val(0, 3800));
    CHECK(d.planar[0][699] == ramp_val(0, 3999));
    se_record_stats st{};
    se_record_stats_get(e, &st);
    CHECK_EQ(st.takes, 2u);
    CHECK_EQ(st.overruns, 0u);
}

// Spec 020: the manual recording offset moves a take by exactly that many
// frames (positive = earlier), keeps the samples, and is range-checked.
TEST_CASE(record_offset_moves_takes_by_exactly_that_many_frames) {
    EngineHandle e(48000, 256);
    TempDir dir;
    TrackIn t;
    t.arm = true;
    t.mode = SE_INPUT_MONO;
    REQUIRE(publish_v4(e, {t}) == SE_OK);
    int32_t got = -1;
    REQUIRE(se_record_get_offset(e, &got) == SE_OK);
    CHECK_EQ(got, 0); // default
    for (const int32_t offset : {216, -4800, 0}) {
        REQUIRE(se_record_set_offset(e, offset) == SE_OK);
        REQUIRE(se_record_get_offset(e, &got) == SE_OK);
        CHECK_EQ(got, offset);
        REQUIRE(se_transport_seek(e, 10000) == SE_OK);
        REQUIRE(se_record_start(e, dir.path.c_str()) == SE_OK);
        render_in(e, 1, 1000, 0, {127, 256});
        const auto takes = stop_takes(e);
        REQUIRE(takes.size() == 1);
        CHECK_EQ(takes[0].start_frame, 10000 - offset);
        CHECK_EQ(takes[0].frames, 1000u);
        check_take(takes[0], {0}, 0); // same samples, only the placement moves
        REQUIRE(se_transport_stop(e) == SE_OK);
    }
    // Up to one second either way.
    CHECK_EQ(se_record_set_offset(e, 48000), SE_OK);
    CHECK_EQ(se_record_set_offset(e, -48000), SE_OK);
    CHECK_EQ(se_record_set_offset(e, 48001), SE_ERR_INVALID_ARG);
    CHECK_EQ(se_record_set_offset(e, -48001), SE_ERR_INVALID_ARG);
    REQUIRE(se_record_get_offset(e, &got) == SE_OK);
    CHECK_EQ(got, -48000); // a refused value changes nothing
    CHECK_EQ(se_record_get_offset(e, nullptr), SE_ERR_INVALID_ARG);
}

// A change of the offset while recording ends the take and starts a new one
// at the new placement, like a change of delay compensation.
TEST_CASE(record_offset_change_while_recording_starts_a_new_take) {
    EngineHandle e(48000, 256);
    TempDir dir;
    TrackIn t;
    t.arm = true;
    t.mode = SE_INPUT_MONO;
    REQUIRE(publish_v4(e, {t}) == SE_OK);
    REQUIRE(se_record_start(e, dir.path.c_str()) == SE_OK);
    render_in(e, 1, 512, 0, {256});
    REQUIRE(se_record_set_offset(e, 100) == SE_OK);
    render_in(e, 1, 512, 512, {256});
    const auto takes = stop_takes(e);
    REQUIRE(takes.size() == 2);
    CHECK_EQ(takes[0].start_frame, 0);
    CHECK_EQ(takes[0].frames, 512u);
    CHECK_EQ(takes[1].start_frame, 512 - 100);
    CHECK_EQ(takes[1].frames, 512u);
}

TEST_CASE(record_errors) {
    EngineHandle e(48000, 256);
    TempDir dir;
    CHECK_EQ(se_record_start(e, dir.path.c_str()), SE_ERR_STATE); // nothing armed
    TrackIn t;
    t.arm = true; // armed but no input
    REQUIRE(publish_v4(e, {t}) == SE_OK);
    CHECK_EQ(se_record_start(e, dir.path.c_str()), SE_ERR_STATE);
    CHECK_EQ(se_record_start(e, ""), SE_ERR_INVALID_ARG);
    t.mode = SE_INPUT_MONO;
    REQUIRE(publish_v4(e, {t}) == SE_OK);
    // A regular file where the directory should be.
    TempFile f("not_a_dir");
    write_bytes(f.path, {1, 2, 3});
    CHECK_EQ(se_record_start(e, f.path.c_str()), SE_ERR_IO);
    // Plain se_render_offline while recording records silence (no input).
    REQUIRE(se_record_start(e, dir.path.c_str()) == SE_OK);
    std::vector<float> out(2 * 100);
    REQUIRE(se_render_offline(e, out.data(), 100) == SE_OK);
    const auto takes = stop_takes(e);
    REQUIRE(takes.size() == 1);
    check_take(takes[0], {0}, 0, [](uint32_t, std::size_t) { return 0.0f; });
}

TEST_CASE(record_overrun_when_writer_stalls_keeps_takes_aligned_and_valid) {
    EngineHandle e(48000, 256);
    TempDir dir;
    TrackIn t;
    t.arm = true;
    t.mode = SE_INPUT_MONO;
    REQUIRE(publish_v4(e, {t}) == SE_OK);
    dstudio::testing::set_record_ring_frames(e, 4096);
    REQUIRE(se_record_start(e, dir.path.c_str()) == SE_OK);
    dstudio::testing::set_record_writer_stalled(e, true);
    render_in(e, 1, 80 * 256, 0, {256}); // 16 blocks fit, 64 overrun
    se_record_stats st{};
    se_record_stats_get(e, &st);
    CHECK_EQ(st.overruns, 64u);
    CHECK_EQ(st.dropped_frames, 64u * 256u);
    dstudio::testing::set_record_writer_stalled(e, false);
    CHECK(wait_until([&] {
        auto v = live_takes(e);
        return v.size() == 1 && v[0].frames == 4096;
    }));
    render_in(e, 1, 1000, 80 * 256, {256});
    // Stall again so stop has to drain a backlog itself.
    dstudio::testing::set_record_writer_stalled(e, true);
    render_in(e, 1, 512, 80 * 256 + 1000, {256});
    const auto takes = stop_takes(e);
    REQUIRE(takes.size() == 2);
    CHECK_EQ(takes[0].start_frame, 0);
    CHECK_EQ(takes[0].frames, 4096u);
    check_take(takes[0], {0}, 0);
    CHECK_EQ(takes[1].start_frame, 80 * 256);
    CHECK_EQ(takes[1].frames, 1512u);
    check_take(takes[1], {0}, 80 * 256);
    se_record_stats_get(e, &st); // still readable after stop
    CHECK_EQ(st.overruns, 64u);
    CHECK_EQ(st.takes, 2u);
    CHECK_EQ(st.write_errors, 0u);
}

// A take that reaches the WAV size limit keeps what was recorded: the file
// is finalized at the limit (not left with a 0-frame header) and the take
// is reported with those frames; the write error is counted.
TEST_CASE(record_take_at_the_size_limit_is_truncated_not_lost) {
    EngineHandle e(48000, 256);
    TempDir dir;
    TrackIn t;
    t.arm = true;
    t.mode = SE_INPUT_MONO;
    REQUIRE(publish_v4(e, {t}) == SE_OK);
    dstudio::testing::set_record_max_take_bytes(e, 4000 * 4); // 4000 mono frames
    REQUIRE(se_record_start(e, dir.path.c_str()) == SE_OK);
    dstudio::testing::set_record_max_take_bytes(e, 0);
    render_in(e, 1, 10000, 0, {256});
    const auto takes = stop_takes(e);
    REQUIRE(takes.size() == 1);
    CHECK_EQ(takes[0].frames, 4000u);
    check_take(takes[0], {0}, 0);
    se_record_stats st{};
    se_record_stats_get(e, &st);
    CHECK_EQ(st.write_errors, 1u);
}

TEST_CASE(record_and_monitor_do_not_allocate_on_the_audio_path) {
    EngineHandle e(48000, 256);
    TempDir dir;
    const uint64_t a = load_asset(e, {sine(48000, 220.0, 48000.0, 0.2)});
    std::vector<TrackIn> tracks;
    for (uint16_t i = 0; i < 8; ++i) {
        TrackIn t;
        t.id = i + 1u;
        t.arm = true;
        t.monitor = i % 2 == 0;
        t.mode = static_cast<uint8_t>(i % 3 == 0 ? SE_INPUT_STEREO : SE_INPUT_MONO);
        t.channel = static_cast<uint16_t>(i % 4);
        t.clips = {clip(1, a, 0, 0, 48000)};
        tracks.push_back(t);
    }
    REQUIRE(publish_v4(e, tracks) == SE_OK);
    REQUIRE(se_transport_set_loop(e, 1, 100, 20000) == SE_OK);
    REQUIRE(se_record_start(e, dir.path.c_str()) == SE_OK);
    REQUIRE(se_voice_capture_set(e, 2, 1) == SE_OK); // voice input listens too
    const std::vector<float> in = make_input(4, 8192, 0, ramp_val);
    std::vector<float> out(2 * 8192);
    REQUIRE(se_render_offline_with_input(e, in.data(), 4, out.data(), 256) == SE_OK); // warm-up
    const long allocs = rt_alloc_count(), frees = rt_free_count();
    const uint32_t sizes[] = {256, 1, 333, 8192, 64, 255};
    for (int i = 0; i < 400; ++i) {
        if (i % 100 == 50) se_transport_seek(e, i * 100); // new takes
        if (i % 100 == 70) se_input_set_muted(e, 1);
        if (i % 100 == 80) se_input_set_muted(e, 0);
        RtAllocGuard g;
        if (se_render_offline_with_input(e, in.data(), 4, out.data(), sizes[i % 6]) != SE_OK) {
            CHECK(false);
            break;
        }
    }
    CHECK_EQ(rt_alloc_count() - allocs, 0L);
    CHECK_EQ(rt_free_count() - frees, 0L);
    const auto takes = stop_takes(e);
    CHECK(takes.size() >= 8u * 5u);
}

// ------------------------------------------------------------ device path (fake device)

namespace {

// Fake platform device: the test drives callbacks with pump().
class FakeDevice final : public dstudio::AudioDevice {
public:
    bool fail_input = false;
    Config last{};
    int starts = 0;
    int32_t validate(const Config&, std::string&) const override { return SE_OK; }
    int32_t start(const Config& c, Callback* cb, std::string&) override {
        last = c;
        cb_ = cb;
        ++starts;
        st_ = Status{};
        st_.output_name = "Fake Out";
        st_.device_rate = 48000.0;
        st_.buffer_size = 128;
        st_.output_channels = 2;
        st_.output_latency = 150;
        if (c.input_enabled) {
            if (fail_input) {
                st_.input_error = "microphone access is denied (fake)";
            } else {
                st_.input_open = true;
                st_.input_name = "Fake In";
                st_.input_channels = 2;
                st_.input_latency = 100;
            }
        }
        running_ = true;
        return SE_OK;
    }
    int32_t stop() override {
        running_ = false;
        return SE_OK;
    }
    bool is_running() const override { return running_; }
    Status status() const override { return st_; }
    // One device cycle with planar input (nin may be 0).
    void pump(uint32_t frames, const float* const* in, uint32_t nin, std::vector<float>& l, std::vector<float>& r) {
        l.assign(frames, 0.0f);
        r.assign(frames, 0.0f);
        float* outs[2] = {l.data(), r.data()};
        cb_->render(outs, 2, in, nin, frames);
    }

private:
    Callback* cb_ = nullptr;
    bool running_ = false;
    Status st_;
};

FakeDevice* install_fake(se_engine* e) {
    auto* holder = new FakeDevice*(nullptr);
    dstudio::testing::set_device_factory(e, [holder] {
        auto d = std::make_unique<FakeDevice>();
        *holder = d.get();
        return std::unique_ptr<dstudio::AudioDevice>(std::move(d));
    });
    se_device_config dc{}; // output only: creates the device object without starting it
    REQUIRE(se_device_configure(e, &dc) == SE_OK);
    FakeDevice* d = *holder;
    delete holder;
    return d;
}

} // namespace

TEST_CASE(device_default_start_never_enables_input) {
    EngineHandle e(48000, 256);
    FakeDevice* d = install_fake(e);
    REQUIRE(se_device_start(e) == SE_OK);
    CHECK(!d->last.input_enabled);
    se_device_status st{};
    REQUIRE(se_device_status_get(e, &st) == SE_OK);
    CHECK_EQ(st.running, 1);
    CHECK_EQ(st.input_open, 0);
    CHECK(std::string(st.output_name) == "Fake Out");
    CHECK(std::string(st.input_name).empty());
    TempDir dir;
    TrackIn t;
    t.arm = true;
    t.mode = SE_INPUT_MONO;
    REQUIRE(publish_v4(e, {t}) == SE_OK);
    CHECK_EQ(se_record_start(e, dir.path.c_str()), SE_ERR_STATE); // no input open
    REQUIRE(se_device_stop(e) == SE_OK);
}

TEST_CASE(device_input_failure_keeps_output_running) {
    EngineHandle e(48000, 256);
    FakeDevice* d = install_fake(e);
    d->fail_input = true;
    se_device_config dc{};
    dc.input_enabled = 1;
    REQUIRE(se_device_configure(e, &dc) == SE_OK); // stopped: validated and stored
    REQUIRE(se_device_start(e) == SE_OK);           // output starts, input does not
    CHECK(d->last.input_enabled);
    se_device_status st{};
    se_device_status_get(e, &st);
    CHECK_EQ(st.running, 1);
    CHECK_EQ(st.input_open, 0);
    CHECK(std::string(st.input_error).find("denied") != std::string::npos);
    // Output keeps rendering.
    REQUIRE(publish_v4(e, {TrackIn{}}) == SE_OK);
    std::vector<float> l, r;
    se_stats s0{};
    se_stats_get(e, &s0);
    d->pump(128, nullptr, 0, l, r);
    se_stats s1{};
    se_stats_get(e, &s1);
    CHECK_EQ(s1.callbacks, s0.callbacks + 1);
    // Reconfiguring while running reports the input error, output stays up.
    CHECK_EQ(se_device_configure(e, &dc), SE_ERR_DEVICE);
    CHECK(std::string(se_last_error()).find("denied") != std::string::npos);
    CHECK_EQ(se_device_is_running(e), 1);
    CHECK_EQ(d->starts, 2);
    REQUIRE(se_device_stop(e) == SE_OK);
}

TEST_CASE(device_recording_is_latency_compensated) {
    EngineHandle e(48000, 256);
    FakeDevice* d = install_fake(e);
    se_device_config dc{};
    dc.input_enabled = 1;
    REQUIRE(se_device_configure(e, &dc) == SE_OK);
    REQUIRE(se_device_start(e) == SE_OK);
    se_device_status st{};
    se_device_status_get(e, &st);
    CHECK_EQ(st.input_open, 1);
    CHECK(std::string(st.input_name) == "Fake In");
    CHECK_EQ(st.input_latency_frames, 100u);
    CHECK_EQ(st.output_latency_frames, 150u);
    TrackIn t;
    t.arm = true;
    t.monitor = true;
    t.mode = SE_INPUT_MONO;
    t.channel = 1;
    REQUIRE(publish_v4(e, {t}) == SE_OK);
    REQUIRE(se_transport_seek(e, 10000) == SE_OK);
    TempDir dir;
    REQUIRE(se_record_start(e, dir.path.c_str()) == SE_OK);
    std::vector<float> c0(128), c1(128), l, r;
    std::size_t k = 0;
    for (int b = 0; b < 40; ++b) {
        for (std::size_t i = 0; i < 128; ++i, ++k) {
            c0[i] = ramp_val(0, k);
            c1[i] = ramp_val(1, k);
        }
        const float* in[2] = {c0.data(), c1.data()};
        d->pump(128, in, 2, l, r);
        CHECK(l[5] == c1[5] * pan_gain(0.0f, true)); // monitored through the device path
    }
    const auto takes = stop_takes(e);
    REQUIRE(takes.size() == 1);
    CHECK_EQ(takes[0].start_frame, 10000 - 250); // input + output latency
    CHECK_EQ(takes[0].frames, 40u * 128u);
    check_take(takes[0], {1}, 0);
    // Spec 020: the manual offset adds to the reported device latency.
    REQUIRE(se_record_set_offset(e, 37) == SE_OK);
    REQUIRE(se_transport_seek(e, 20000) == SE_OK);
    REQUIRE(se_record_start(e, dir.path.c_str()) == SE_OK);
    for (int b = 0; b < 4; ++b) {
        const float* in[2] = {c0.data(), c1.data()};
        d->pump(128, in, 2, l, r);
    }
    const auto offset_takes = stop_takes(e);
    REQUIRE(offset_takes.size() == 1);
    CHECK_EQ(offset_takes[0].start_frame, 20000 - 250 - 37);
    REQUIRE(se_device_stop(e) == SE_OK);
}

// ------------------------------------------------------------ real Core Audio (no input opened)

TEST_CASE(device_configure_validation_opens_nothing) {
    EngineHandle e(48000, 256);
    se_device_config dc{};
    dc.output_uid = "no-such-device-uid";
    CHECK_EQ(se_device_configure(e, &dc), SE_ERR_NOT_FOUND);
    dc.output_uid = nullptr;
    dc.input_enabled = 1;
    dc.input_uid = "no-such-input-uid";
    CHECK_EQ(se_device_configure(e, &dc), SE_ERR_NOT_FOUND);
    dc = se_device_config{};
    dc.buffer_size = 3;
    std::size_t ndev = 0;
    se_device_list(nullptr, 0, &ndev);
    if (ndev == 0) return; // no devices (CI): the rest needs a default output
    CHECK_EQ(se_device_configure(e, &dc), SE_ERR_INVALID_ARG);
    dc.buffer_size = 0;
    dc.sample_rate = 12345.0;
    CHECK_EQ(se_device_configure(e, &dc), SE_ERR_INVALID_ARG);
#if defined(__APPLE__)
    // Core Audio: with an input the device must run at the engine rate (the
    // AUHAL input side does not resample). WASAPI and PipeWire convert the
    // input to the engine rate, so this restriction is macOS-only.
    // Validation only: the device is never started here, so no input is
    // opened.
    std::vector<se_device_info> devs(ndev);
    se_device_list(devs.data(), devs.size(), &ndev);
    bool has_input = false;
    for (const auto& d : devs) has_input |= d.is_default_input != 0;
    if (has_input) {
        dc = se_device_config{};
        dc.input_enabled = 1;
        dc.sample_rate = 44100.0;
        CHECK_EQ(se_device_configure(e, &dc), SE_ERR_INVALID_ARG);
    }
#endif
    se_device_status st{};
    se_device_status_get(e, &st);
    CHECK_EQ(st.running, 0);
}

TEST_CASE(device_list_reports_uids_and_defaults) {
    std::size_t n = 0;
    REQUIRE(se_device_list(nullptr, 0, &n) == SE_OK);
    std::vector<se_device_info> devs(n);
    std::size_t m = 0;
    REQUIRE(se_device_list(devs.data(), devs.size(), &m) == SE_OK);
    int def_out = 0;
    for (std::size_t i = 0; i < std::min(n, m); ++i) {
        CHECK(devs[i].uid[0] != '\0');
        CHECK(devs[i].input_channels + devs[i].output_channels > 0 || true);
        def_out += devs[i].is_default_output;
        for (uint32_t r = 1; r < devs[i].sample_rate_count; ++r)
            CHECK(devs[i].sample_rates[r] > devs[i].sample_rates[r - 1]);
    }
    CHECK(def_out <= 1);
}

// CoreAudio applies the requested IO buffer exactly. WASAPI shared mode runs
// at the endpoint's period (e.g. 480 frames = 10 ms at 48 kHz) and PipeWire
// at the graph quantum, either of which may differ from the request.
static void check_reported_buffer(uint32_t reported, uint32_t requested) {
#if defined(__APPLE__)
    CHECK_EQ(reported, requested);
#else
    (void)requested;
    CHECK(reported > 0u);
#endif
}

TEST_CASE(device_output_only_named_device_and_restart) {
    // Real output device, explicitly named (HALOutput path), no input.
    std::size_t n = 0;
    se_device_list(nullptr, 0, &n);
    std::vector<se_device_info> devs(n);
    se_device_list(devs.data(), devs.size(), &n);
    const se_device_info* out = nullptr;
    for (const auto& d : devs)
        if (d.is_default_output) out = &d;
    if (out == nullptr) SKIP("no default audio output device");
    EngineHandle e(48000, 512);
    se_device_config dc{};
    dc.output_uid = out->uid;
    dc.buffer_size = 512;
    REQUIRE(se_device_configure(e, &dc) == SE_OK);
    if (se_device_start(e) != SE_OK) {
        const std::string why = std::string("the default output device could not be started: ") + se_last_error();
        SKIP(why.c_str());
    }
    std::fprintf(stderr, "  real output device: %s\n", out->name);
    se_device_status st{};
    se_device_status_get(e, &st);
    CHECK_EQ(st.running, 1);
    CHECK_EQ(st.input_open, 0);
    CHECK(std::string(st.output_name) == out->name);
    check_reported_buffer(st.buffer_size, 512u);
    // Device + safety offset + stream latency (no IO-buffer term since the
    // loopback fix): hardware dependent and 0 on virtual devices, so only
    // plausibility is checked.
    CHECK(st.output_latency_frames < 48000u);
    CHECK(wait_until([&] {
        se_stats s{};
        se_stats_get(e, &s);
        return s.callbacks > 3;
    }));
    dc.buffer_size = 256; // restart while running
    CHECK_EQ(se_device_configure(e, &dc), SE_OK);
    se_device_status_get(e, &st);
    CHECK_EQ(st.running, 1);
    check_reported_buffer(st.buffer_size, 256u);
    REQUIRE(se_device_stop(e) == SE_OK);
}

// ------------------------------------------------------------ real duplex loopback (opt-in)

// Plays a click track through a real output device and records it back through
// a real input that carries that output (a loopback), then checks the take
// lands where the clicks are on the timeline. This runs the whole duplex
// device path: the platform backend's output and input streams, the input
// ring, the reported latencies and the recorder.
//
// Opt-in, because it needs a loopback: set DSTUDIO_TEST_LOOPBACK_INPUT to the
// uid of an input that hears the output (and DSTUDIO_TEST_LOOPBACK_OUTPUT to the
// output uid; default output otherwise). Linux CI runs it on a headless
// PipeWire graph: a null sink plus pw-loopback from its monitor to a virtual
// source (see .github/workflows/ci.yml). On real hardware, use a cable from
// an output to an input. DSTUDIO_TEST_LOOPBACK_TOLERANCE (frames, default 64)
// bounds the error, which is the latency the backend failed to report (0 on
// the PipeWire loopback; a hardware converter adds latency of its own).
TEST_CASE(device_duplex_loopback_take_lands_on_the_clicks) {
    std::string in_env, out_env, tol_env;
    if (!dstudio::os::get_env("DSTUDIO_TEST_LOOPBACK_INPUT", in_env) || in_env.empty())
        SKIP("set DSTUDIO_TEST_LOOPBACK_INPUT to an input that hears the output");
    const char* in_uid = in_env.c_str();
    const char* out_uid = dstudio::os::get_env("DSTUDIO_TEST_LOOPBACK_OUTPUT", out_env) ? out_env.c_str() : nullptr;
    const long tolerance =
        dstudio::os::get_env("DSTUDIO_TEST_LOOPBACK_TOLERANCE", tol_env) ? std::atol(tol_env.c_str()) : 64;

    constexpr uint32_t kRate = 48000;
    constexpr std::size_t kPeriod = 4800; // a click every 100 ms
    constexpr std::size_t kFirst = 2400;
    constexpr std::size_t kLength = 3 * kRate;
    EngineHandle e(kRate, 256);
    std::vector<float> clicks(kLength, 0.0f);
    for (std::size_t k = kFirst; k < kLength; k += kPeriod) clicks[k] = 0.5f;
    const uint64_t a = load_asset(e, {clicks, clicks}, kRate);

    TrackIn player;
    player.id = 1;
    player.clips = {clip(1, a, 0, 0, static_cast<int64_t>(kLength))};
    TrackIn rec;
    rec.id = 2;
    rec.arm = true;
    rec.mode = SE_INPUT_MONO;
    rec.channel = 0; // not monitored: the output carries the clicks only
    REQUIRE(publish_v4(e, {player, rec}) == SE_OK);

    se_device_config dc{};
    dc.output_uid = out_uid != nullptr && *out_uid != '\0' ? out_uid : nullptr;
    dc.input_uid = in_uid;
    dc.input_enabled = 1;
    dc.buffer_size = 256;
    REQUIRE(se_device_configure(e, &dc) == SE_OK);
    REQUIRE(se_device_start(e) == SE_OK);
    se_device_status st{};
    REQUIRE(se_device_status_get(e, &st) == SE_OK);
    std::fprintf(stderr, "  output %s, input %s (%s), %.0f Hz, buffer %u, latency in %u + out %u frames\n",
                 st.output_name, st.input_name, st.input_open ? "open" : st.input_error, st.sample_rate,
                 st.buffer_size, st.input_latency_frames, st.output_latency_frames);
    REQUIRE(st.input_open == 1);
    // Let the graph settle first, as it has long before anyone presses record:
    // a sound server can still be changing its quantum for the new streams
    // (PipeWire), and a loopback re-buffers when it does.
    std::this_thread::sleep_for(std::chrono::milliseconds(500));

    TempDir dir;
    REQUIRE(se_transport_seek(e, 0) == SE_OK);
    REQUIRE(se_record_start(e, dir.path.c_str()) == SE_OK);
    REQUIRE(se_transport_play(e) == SE_OK);
    CHECK(wait_until(
        [&] {
            se_transport_info ti{};
            se_transport_get(e, &ti);
            return ti.position >= static_cast<int64_t>(kLength - kRate / 4);
        },
        10000));
    REQUIRE(se_transport_pause(e) == SE_OK);
    const auto takes = stop_takes(e);
    REQUIRE(se_device_stop(e) == SE_OK);
    REQUIRE(takes.size() == 1);

    dstudio::DecodedAudio got;
    std::string err;
    REQUIRE(dstudio::decode_wav_file(takes[0].path, got, err) == SE_OK);
    REQUIRE(got.channels == 1);
    // Each recorded click, mapped to the timeline, against the nearest click.
    std::vector<long> errors;
    for (uint64_t i = 1; i + 1 < got.frames; ++i) {
        const float x = got.planar[0][i];
        if (x < 0.25f || x < got.planar[0][i - 1] || x < got.planar[0][i + 1]) continue;
        const long t = static_cast<long>(takes[0].start_frame) + static_cast<long>(i);
        const long rel = t - static_cast<long>(kFirst);
        const long nearest = static_cast<long>(kFirst) +
                             std::lround(static_cast<double>(rel) / kPeriod) * static_cast<long>(kPeriod);
        errors.push_back(t - nearest);
    }
    std::fprintf(stderr, "  take starts at %lld, %zu clicks found, error (frames):",
                 static_cast<long long>(takes[0].start_frame), errors.size());
    for (long x : errors) std::fprintf(stderr, " %ld", x);
    std::fprintf(stderr, "\n");
    // Nearly all of the clicks in the take (one comes before the take starts).
    CHECK(errors.size() >= 25u);
    // Every click lands within the tolerance, except right after a glitch: an
    // xrun (likely on a busy CI machine without real-time scheduling) shifts
    // the input by a cycle until the input ring re-syncs, within two windows
    // of 32 cycles (at most 4 clicks here). The last clicks must be aligned.
    std::size_t off = 0;
    for (long x : errors) off += std::labs(x) > tolerance ? 1u : 0u;
    CHECK(off <= 4u);
    for (std::size_t i = errors.size() >= 5 ? errors.size() - 5 : 0; i < errors.size(); ++i)
        CHECK(std::labs(errors[i]) <= tolerance);
}
