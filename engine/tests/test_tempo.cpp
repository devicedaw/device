// Tempo changes (se_session_publish_v12, specs/015-tempo-changes): the
// segment formula, the metronome on the map, the song position tempo-synced
// devices see, validation, and that a session without a map is unchanged.
#include "devices/device_harness.hpp"
#include "plugins/plugin_instance.hpp"
#include "tempo_map.hpp"
#include "test_harness.hpp"
#include "test_util.hpp"

#include <bit>
#include <cmath>
#include <vector>

using namespace testutil;

namespace {

constexpr double kRate = 48000.0;

struct Pt {
    double quarter;
    double bpm;
    bool ramp;
};

// Segments of a point list (first point at quarter 0), integrating each
// segment's length the same way the core does.
std::vector<se_tempo_segment> segments(const std::vector<Pt>& pts) {
    std::vector<se_tempo_segment> out;
    double frame = 0.0;
    for (std::size_t i = 0; i < pts.size(); ++i) {
        const bool last = i + 1 == pts.size();
        const double a = pts[i].bpm;
        const double b = !last && pts[i].ramp ? pts[i + 1].bpm : a;
        out.push_back({frame, pts[i].quarter, a, b});
        if (last) break;
        const double dq = pts[i + 1].quarter - pts[i].quarter;
        frame += a == b ? dq * 60.0 * kRate / a : 60.0 * kRate * dq / (b - a) * std::log(b / a);
    }
    return out;
}

// The frame of quarter q by numeric integration of 60 R / tempo(q): an
// independent check of the closed form.
double integrated_frame(const std::vector<Pt>& pts, double q) {
    const auto tempo = [&](double x) {
        std::size_t i = 0;
        while (i + 1 < pts.size() && pts[i + 1].quarter <= x) ++i;
        if (i + 1 == pts.size() || !pts[i].ramp) return pts[i].bpm;
        const double u = (x - pts[i].quarter) / (pts[i + 1].quarter - pts[i].quarter);
        return pts[i].bpm + u * (pts[i + 1].bpm - pts[i].bpm);
    };
    const int n = static_cast<int>(q * 4000.0);
    const double dq = q / n;
    double f = 0.0;
    for (int i = 0; i < n; ++i) f += dq * 60.0 * kRate / tempo((i + 0.5) * dq);
    return f;
}

// 480 BPM for two bars, a step to 720, a ramp from quarter 12 down to 240 at quarter 20.
const std::vector<Pt> kSong = {{0, 480, false}, {8, 720, false}, {12, 720, true}, {20, 240, false}};

int32_t set_click(se_engine* e) {
    se_metronome_desc m{};
    m.enabled = 1;
    m.gain = 0.5f;
    m.frames_per_beat = 24000.0; // ignored with a map
    m.beats_per_bar = 4;
    return se_metronome_set(e, &m);
}

std::vector<int64_t> onsets(const std::vector<float>& inter) {
    std::vector<int64_t> v;
    for (std::size_t i = 0; i < inter.size() / 2; ++i)
        if (inter[2 * i] != 0.0f && (i == 0 || inter[2 * (i - 1)] == 0.0f)) v.push_back(static_cast<int64_t>(i));
    return v;
}

std::vector<float> render_blocks(se_engine* e, uint32_t total, std::initializer_list<uint32_t> sizes) {
    std::vector<float> out(static_cast<std::size_t>(total) * 2);
    const std::vector<uint32_t> sz(sizes);
    uint32_t done = 0;
    for (std::size_t k = 0; done < total; ++k) {
        const uint32_t n = std::min(sz[k % sz.size()], total - done);
        if (se_render_offline(e, out.data() + 2 * static_cast<std::size_t>(done), n) != SE_OK) break;
        done += n;
    }
    return out;
}

} // namespace

TEST_CASE(tempo_map_formula_matches_integration) {
    const auto segs = segments(kSong);
    const dstudio::TempoMap map(segs.data(), segs.size(), kRate);
    for (double q : {0.5, 8.0, 9.25, 12.0, 13.0, 16.5, 19.99, 20.0, 27.0}) {
        const double f = map.frame_at(q);
        CHECK_NEAR(f, integrated_frame(kSong, q), 0.05);
        CHECK_NEAR(map.quarter_at(f), q, 1e-9);
    }
    CHECK_NEAR(map.bpm_at(map.frame_at(16.0)), 480.0, 1e-6); // halfway down the ramp
    CHECK_NEAR(map.bpm_at(map.frame_at(10.0)), 720.0, 1e-9);
    // Before frame 0 the first tempo holds.
    CHECK_NEAR(map.quarter_at(-6000.0), -1.0, 1e-12);
    CHECK_NEAR(map.frame_at(-2.0), -12000.0, 1e-9);

    // RtTransport::beats() follows the map when it has one.
    dstudio::RtTransport t;
    t.sample_rate = kRate;
    t.position = 70000;
    CHECK_NEAR(t.beats(), 70000.0 / kRate * 120.0 / 60.0, 1e-12);
    t.map = &map;
    CHECK_NEAR(t.beats(), map.quarter_at(70000.0), 1e-12);
}

TEST_CASE(metronome_clicks_on_the_tempo_map) {
    EngineHandle e(48000, 256);
    const auto segs = segments(kSong);
    devtest::DeviceSession s;
    s.add_track(1, 0, 0);
    REQUIRE(s.publish(e, segs, 1.0) == SE_OK);
    REQUIRE(set_click(e) == SE_OK);
    REQUIRE(se_transport_play(e) == SE_OK);
    const dstudio::TempoMap map(segs.data(), segs.size(), kRate);
    const auto total = static_cast<uint32_t>(map.frame_at(28.0)) + 10;
    const auto out = render_blocks(e, total, {256, 7, 333, 64, 1, 255});
    std::vector<int64_t> want;
    for (int k = 0; k <= 28; ++k) want.push_back(std::llround(integrated_frame(kSong, k)));
    const auto got = onsets(out);
    if (got.size() != want.size()) for (auto g : got) std::fprintf(stderr, " %lld", static_cast<long long>(g));
    REQUIRE(got.size() == want.size());
    for (std::size_t i = 0; i < want.size(); ++i) {
        CHECK(std::llabs(got[i] - want[i]) <= 1);
        if (std::llabs(got[i] - want[i]) > 1)
            std::fprintf(stderr, "  beat %zu: got %lld want %lld\n", i, static_cast<long long>(got[i]),
                         static_cast<long long>(want[i]));
    }

    // In 6/8 the click counts eighths: beat k at quarter k / 2.
    EngineHandle e2(48000, 256);
    REQUIRE(s.publish(e2, segs, 0.5) == SE_OK);
    REQUIRE(set_click(e2) == SE_OK);
    REQUIRE(se_transport_seek(e2, std::llround(map.frame_at(12.0))) == SE_OK);
    REQUIRE(se_transport_play(e2) == SE_OK);
    const auto ramp = onsets(render_blocks(e2, static_cast<uint32_t>(map.frame_at(14.0) - map.frame_at(12.0)) + 1, {256}));
    REQUIRE(ramp.size() == 5);
    const int64_t at = std::llround(map.frame_at(12.0));
    for (int k = 0; k < 5; ++k) CHECK(std::llabs(ramp[static_cast<std::size_t>(k)] + at - std::llround(map.frame_at(12.0 + 0.5 * k))) <= 1);
}

TEST_CASE(a_synced_device_sees_the_song_position_on_the_map) {
    EngineHandle e(48000, 256);
    const uint64_t a = load_asset(e, {sine(48000, 400.0, 48000.0, 0.1)});
    const auto segs = segments(kSong);
    const dstudio::TempoMap map(segs.data(), segs.size(), kRate);
    devtest::DeviceSession s;
    auto& t = s.add_track(1, a, 48000);
    // LFO (dstudio_core::builtin::lfo): sine, synced to 8 bars (32 quarters), no phase.
    devtest::DeviceSession::add_device_state(t, 40, "lfo", devtest::route_blob({0.f, 1.f, 1.f, 0.f, 100.f, 0.f}, {}));
    REQUIRE(s.publish(e, segs) == SE_OK);
    REQUIRE(se_transport_play(e) == SE_OK);
    std::vector<float> buf(2 * 256);
    // The last modulation step of each block runs at the block start plus this.
    const int64_t last = (255 / SE_MOD_GRID_FRAMES) * SE_MOD_GRID_FRAMES;
    int checked = 0;
    for (int64_t pos = 0; pos < static_cast<int64_t>(map.frame_at(24.0)); pos += 256) {
        REQUIRE(se_render_offline(e, buf.data(), 256) == SE_OK);
        float ro[3] = {};
        std::size_t n = 0;
        REQUIRE(se_builtin_readouts(e, 40, ro, 3, &n) == SE_OK);
        const double q = map.quarter_at(static_cast<double>(pos + last));
        const double want = q / 32.0 - std::floor(q / 32.0);
        CHECK_NEAR(ro[2], want, 1e-5);
        ++checked;
    }
    CHECK(checked > 500);
}

TEST_CASE(tempo_map_validation) {
    EngineHandle e(48000, 256);
    devtest::DeviceSession s;
    s.add_track(1, 0, 0);
    const auto ok = segments(kSong);
    CHECK(s.publish(e, ok, 1.0) == SE_OK);
    CHECK(s.publish(e, ok, 0.0) == SE_ERR_INVALID_ARG);
    auto bad = ok;
    bad[0].start_frame = 1.0;
    CHECK(s.publish(e, bad) == SE_ERR_INVALID_ARG);
    bad = ok;
    bad[2].start_quarter = bad[1].start_quarter;
    CHECK(s.publish(e, bad) == SE_ERR_INVALID_ARG);
    bad = ok;
    bad[3].end_bpm = 100.0; // the last segment must hold
    CHECK(s.publish(e, bad) == SE_ERR_INVALID_ARG);
    bad = ok;
    bad[1].start_bpm = std::nan("");
    CHECK(s.publish(e, bad) == SE_ERR_INVALID_ARG);
    se_session_desc_v12 d{};
    d.tempo_count = 1;
    CHECK(se_session_publish_v12(e, &d) == SE_ERR_INVALID_ARG);
    CHECK(se_session_publish_v12(e, nullptr) == SE_ERR_INVALID_ARG);
}

TEST_CASE(a_session_without_a_map_renders_bit_identically) {
    // The same clip, metronome and synced LFO through v11 and through v12 with no segments.
    const auto run = [](bool v12) {
        EngineHandle e(48000, 256);
        const uint64_t a = load_asset(e, {sine(48000, 300.0, 48000.0, 0.3), sine(48000, 450.0, 48000.0, 0.3)});
        se_track_desc_v6 t{};
        t.track_id = 1;
        t.gain = 1.0f;
        const se_clip_desc c = clip(1, a, 1000, 0, 40000);
        t.clips = &c;
        t.clip_count = 1;
        se_session_desc_v12 d{};
        d.v11.v10.v9.v8.v7.v6.tracks = &t;
        d.v11.v10.v9.v8.v7.v6.track_count = 1;
        d.v11.v10.v9.v8.v7.v6.master_gain = 1.0f;
        if (v12) {
            if (se_session_publish_v12(e, &d) != SE_OK) return std::vector<float>{};
        } else if (se_session_publish_v11(e, &d.v11) != SE_OK) {
            return std::vector<float>{};
        }
        se_transport_set_tempo(e, 21333.3);
        se_metronome_desc m{};
        m.enabled = 1;
        m.gain = 0.5f;
        m.frames_per_beat = 10666.65;
        m.beats_per_bar = 3;
        se_metronome_set(e, &m);
        se_transport_play(e);
        return render(e, 60000, 256);
    };
    const auto a = run(false);
    const auto b = run(true);
    REQUIRE(a.size() == b.size() && !a.empty());
    std::size_t diff = 0;
    for (std::size_t i = 0; i < a.size(); ++i)
        if (std::bit_cast<uint32_t>(a[i]) != std::bit_cast<uint32_t>(b[i])) ++diff;
    CHECK_EQ(diff, std::size_t{0});
}
