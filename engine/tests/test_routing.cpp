// Return tracks, sends and output routing (se_session_publish_v9, spec 002):
// levels pre- and post-fader, outputs into a return, mute and solo, validation,
// delay compensation across buses, export parity and RT safety.
//
// Tracks are panned hard left (constant-power balance: left gain 1, right
// gain 0), so the left channel carries the signal unscaled.
#include "test_harness.hpp"
#include "test_util.hpp"
#include "wav.hpp"

#include "delay_line.hpp"

#include <cmath>
#include <string>
#include <vector>

using namespace testutil;

namespace {

const std::string kClapLatency = std::string(DSTUDIO_TEST_PLUGIN_DIR) + "/DStudioTestLatency.clap";

std::vector<std::vector<uint8_t>> g_states; // device states outlive every publish

se_device_desc clap_latency(uint64_t id, uint32_t n) {
    g_states.push_back({static_cast<uint8_t>(n), static_cast<uint8_t>(n >> 8), static_cast<uint8_t>(n >> 16),
                        static_cast<uint8_t>(n >> 24)});
    se_device_desc d{};
    d.device_id = id;
    d.name = "Latency";
    d.plugin_id = "studio.dstudio.test.latency";
    d.path = kClapLatency.c_str();
    d.format = SE_PLUGIN_CLAP;
    d.enabled = 1;
    d.state = g_states.back().data();
    d.state_len = g_states.back().size();
    return d;
}

struct RSession {
    struct Track {
        se_track_desc_v6 d{};
        std::vector<se_clip_desc> clips;
        std::vector<se_device_desc> devices;
    };
    std::vector<Track> tracks;
    std::vector<se_track_route> routes;
    std::vector<se_send_desc> sends;

    Track& audio(uint64_t id, std::vector<se_clip_desc> clips, float gain = 1.0f) {
        Track t;
        t.d.track_id = id;
        t.d.gain = gain;
        t.d.pan = -1.0f;
        t.clips = std::move(clips);
        tracks.push_back(std::move(t));
        return tracks.back();
    }
    Track& ret(uint64_t id, float gain = 1.0f) {
        Track& t = audio(id, {}, gain);
        routes.push_back(se_track_route{id, 0, 1, {}});
        return t;
    }
    void out(uint64_t id, uint64_t to) { routes.push_back(se_track_route{id, to, 0, {}}); }
    void send(uint64_t from, uint64_t to, float gain, bool pre = false) {
        sends.push_back(se_send_desc{from, to, gain, static_cast<uint8_t>(pre ? 1 : 0), {}});
    }
    int32_t publish(se_engine* e) {
        std::vector<se_track_desc_v6> v;
        for (Track& t : tracks) {
            se_track_desc_v6 d = t.d;
            d.clips = t.clips.data();
            d.clip_count = static_cast<uint32_t>(t.clips.size());
            d.devices = t.devices.data();
            d.device_count = static_cast<uint32_t>(t.devices.size());
            v.push_back(d);
        }
        se_session_desc_v9 s{};
        s.v8.v7.v6.tracks = v.data();
        s.v8.v7.v6.track_count = static_cast<uint32_t>(v.size());
        s.v8.v7.v6.master_gain = 1.0f;
        s.routes = routes.data();
        s.route_count = static_cast<uint32_t>(routes.size());
        s.sends = sends.data();
        s.send_count = static_cast<uint32_t>(sends.size());
        return se_session_publish_v9(e, &s);
    }
};

// Left channel of `frames` rendered from frame 0.
std::vector<float> play_left(se_engine* e, uint32_t frames) {
    se_transport_seek(e, 0);
    se_transport_play(e);
    const std::vector<float> inter = render(e, frames, 256);
    std::vector<float> l(frames);
    for (uint32_t i = 0; i < frames; ++i) l[i] = inter[2 * i];
    return l;
}

// A constant 0.5 clip covering frames 0..4096 on track `id`.
std::vector<se_clip_desc> steady(se_engine* e, uint64_t id) {
    const uint64_t a = load_asset(e, {constant(4096, 0.5f), constant(4096, 0.5f)});
    return {clip(id, a, 0, 0, 4096)};
}

} // namespace

TEST_CASE(routing_post_fader_send_reaches_the_master_through_a_return) {
    EngineHandle e;
    RSession s;
    s.audio(1, steady(e, 1), 0.5f); // left: 0.5 * 0.5 = 0.25 at the fader
    s.ret(2);
    s.send(1, 2, 0.5f); // post-fader: 0.125 into the return
    REQUIRE(s.publish(e) == SE_OK);
    const auto l = play_left(e, 1024);
    CHECK_NEAR(l[100], 0.375f, 1e-6f);
    // The fader moves the send with it.
    s.tracks[0].d.gain = 0.0f;
    REQUIRE(s.publish(e) == SE_OK);
    CHECK_NEAR(play_left(e, 1024)[100], 0.0f, 1e-7f);
}

TEST_CASE(routing_pre_fader_send_ignores_volume_and_pan) {
    EngineHandle e;
    RSession s;
    s.audio(1, steady(e, 1), 0.0f); // fader down: nothing direct
    s.ret(2, 0.5f);
    s.send(1, 2, 0.5f, true); // pre-fader: 0.25 on both sides into the return
    REQUIRE(s.publish(e) == SE_OK);
    CHECK_NEAR(play_left(e, 1024)[100], 0.125f, 1e-6f); // return gain 0.5
}

TEST_CASE(routing_output_into_a_return_replaces_the_master) {
    EngineHandle e;
    RSession s;
    s.audio(1, steady(e, 1));
    s.ret(2, 0.5f);
    s.out(1, 2);
    REQUIRE(s.publish(e) == SE_OK);
    CHECK_NEAR(play_left(e, 1024)[100], 0.25f, 1e-6f); // only through the return
}

TEST_CASE(routing_mute_and_solo) {
    EngineHandle e;
    RSession s;
    s.audio(1, steady(e, 1), 0.5f);  // direct 0.25, send 0.25 * 0.5 = 0.125
    s.audio(3, steady(e, 3), 0.25f); // direct 0.125, send 0.125 * 0.5 = 0.0625
    s.ret(2);
    s.send(1, 2, 0.5f);
    s.send(3, 2, 0.5f);
    REQUIRE(s.publish(e) == SE_OK);
    CHECK_NEAR(play_left(e, 1024)[100], 0.5625f, 1e-6f);
    // A muted track sends nothing.
    s.tracks[0].d.mute = 1;
    REQUIRE(s.publish(e) == SE_OK);
    CHECK_NEAR(play_left(e, 1024)[100], 0.1875f, 1e-6f);
    s.tracks[0].d.mute = 0;
    // Soloing track 1 keeps the return playing, fed by track 1 only.
    s.tracks[0].d.solo = 1;
    REQUIRE(s.publish(e) == SE_OK);
    CHECK_NEAR(play_left(e, 1024)[100], 0.375f, 1e-6f);
    s.tracks[0].d.solo = 0;
    // Soloing the return plays only the return, fed by every track.
    s.tracks[2].d.solo = 1;
    REQUIRE(s.publish(e) == SE_OK);
    CHECK_NEAR(play_left(e, 1024)[100], 0.1875f, 1e-6f);
    // Muting the return silences it.
    s.tracks[2].d.solo = 0;
    s.tracks[2].d.mute = 1;
    REQUIRE(s.publish(e) == SE_OK);
    CHECK_NEAR(play_left(e, 1024)[100], 0.375f, 1e-6f);
}

TEST_CASE(routing_validation) {
    EngineHandle e;
    const auto c = steady(e, 1);
    const auto bad = [&](auto&& setup) {
        RSession s;
        s.audio(1, c);
        s.audio(3, {});
        s.ret(2);
        setup(s);
        return s.publish(e) == SE_ERR_INVALID_ARG;
    };
    CHECK(bad([](RSession& s) { s.send(1, 3, 1.0f); }));  // not a return
    CHECK(bad([](RSession& s) { s.send(2, 2, 1.0f); }));  // a return into itself
    CHECK(bad([](RSession& s) { s.ret(4); s.send(2, 4, 1.0f); s.send(4, 2, 1.0f); })); // returns in a loop
    CHECK(bad([](RSession& s) { s.send(1, 9, 1.0f); }));  // unknown
    CHECK(bad([](RSession& s) { s.send(9, 2, 1.0f); }));  // unknown sender
    CHECK(bad([](RSession& s) { s.send(1, 2, 5.0f); }));  // gain out of range
    CHECK(bad([](RSession& s) { s.send(1, 2, NAN); }));   // not finite
    CHECK(bad([](RSession& s) { s.send(1, 2, 1.0f); s.send(1, 2, 0.5f); })); // twice
    CHECK(bad([](RSession& s) { s.out(1, 3); }));         // output to a non-return
    CHECK(bad([](RSession& s) { s.out(1, 1); }));         // into itself
    CHECK(bad([](RSession& s) { s.out(3, 2); s.out(3, 2); })); // routed twice
    CHECK(bad([](RSession& s) { s.ret(4); s.routes.back().output_track_id = 2; })); // return into a return
    CHECK(bad([](RSession& s) { s.routes.push_back(se_track_route{1, 0, 1, {}}); })); // a return with clips
    CHECK(bad([](RSession& s) {
        for (uint64_t id = 10; id < 10 + SE_MAX_RETURNS; ++id) {
            s.audio(id, {});
            s.routes.push_back(se_track_route{id, 0, 1, {}});
        }
    })); // one return too many
    // And a good one, listed out of order (the return first): fine.
    RSession s;
    s.ret(2);
    s.audio(1, c);
    s.send(1, 2, 1.0f);
    REQUIRE(s.publish(e) == SE_OK);
    CHECK_NEAR(play_left(e, 1024)[100], 1.0f, 1e-6f);
}

TEST_CASE(routing_a_return_sends_into_another_return) {
    // A group track (published as a return) with a send to a reverb return.
    EngineHandle e;
    RSession s;
    s.audio(1, steady(e, 1)); // 0.5 left
    s.ret(2);                 // the group
    s.ret(4, 0.5f);           // the return it sends to
    s.out(1, 2);
    s.send(2, 4, 0.5f); // post-fader: 0.25 into the return, which plays it at 0.5
    REQUIRE(s.publish(e) == SE_OK);
    CHECK_NEAR(play_left(e, 1024)[100], 0.5f + 0.125f, 1e-6f);
    // Listed with the receiving return first: the same.
    RSession r;
    r.ret(4, 0.5f);
    r.audio(1, steady(e, 1));
    r.ret(2);
    r.out(1, 2);
    r.send(2, 4, 0.5f);
    REQUIRE(r.publish(e) == SE_OK);
    CHECK_NEAR(play_left(e, 1024)[100], 0.625f, 1e-6f);
    // A muted group sends nothing either.
    r.tracks[2].d.mute = 1;
    REQUIRE(r.publish(e) == SE_OK);
    CHECK_NEAR(play_left(e, 1024)[100], 0.0f, 1e-7f);
}

TEST_CASE(routing_delay_compensation_aligns_direct_and_return_paths) {
    EngineHandle e;
    std::vector<float> imp(8000, 0.0f);
    imp[0] = 0.5f;
    const uint64_t a = load_asset(e, {imp});
    RSession s;
    s.audio(1, {clip(1, a, 2000, 0, 8000)}); // direct and into the return
    s.ret(2).devices = {clap_latency(20, 700)};
    s.send(1, 2, 1.0f);
    s.audio(3, {clip(3, a, 2000, 0, 8000)}).devices = {clap_latency(30, 300)}; // into the return through 300 frames
    s.send(3, 2, 1.0f);
    s.out(3, 2);
    REQUIRE(s.publish(e) == SE_OK);
    // Return input at 300 (track 3), output at 1000: the master is 1000 late.
    se_pdc_report r{};
    std::vector<se_pdc_track> tr(8);
    std::size_t n = 0;
    REQUIRE(se_pdc_report_get(e, &r, tr.data(), tr.size(), &n) == SE_OK);
    CHECK_EQ(r.total_frames, 1000u);
    const auto l = play_left(e, 6000);
    std::vector<std::size_t> nz;
    for (std::size_t i = 0; i < l.size(); ++i)
        if (l[i] != 0.0f) nz.push_back(i);
    // Track 1 direct (0.5) + track 1 through the return (0.5) + track 3 through
    // the return twice (send and output: 1.0), all at 2000 + 1000.
    REQUIRE(nz.size() == 1u);
    CHECK_EQ(nz[0], std::size_t{3000});
    CHECK_NEAR(l[3000], 2.0f, 1e-6f);
    // Off: nothing is delayed, so the paths spread out.
    REQUIRE(se_pdc_set_enabled(e, 0) == SE_OK);
    const auto off = play_left(e, 6000);
    CHECK_NEAR(off[2000], 0.5f, 1e-6f); // track 1 direct
    CHECK_NEAR(off[2700], 0.5f, 1e-6f); // track 1 through the return
    CHECK_NEAR(off[3000], 1.0f, 1e-6f); // track 3 through its chain and the return
}

// A group (a return) with a 400-frame chain sends into a return with a
// 200-frame chain: the group's own output and its send through the return
// land together, 600 frames late, alongside a plain track.
TEST_CASE(routing_delay_compensation_through_a_group_send) {
    EngineHandle e;
    std::vector<float> imp(8000, 0.0f);
    imp[0] = 0.5f;
    const uint64_t a = load_asset(e, {imp});
    RSession s;
    s.audio(1, {clip(1, a, 2000, 0, 8000)}); // into the group
    s.ret(2).devices = {clap_latency(20, 400)}; // the group
    s.ret(4).devices = {clap_latency(40, 200)}; // the return it sends to
    s.out(1, 2);
    s.send(2, 4, 1.0f);
    s.audio(5, {clip(5, a, 2000, 0, 8000)}); // straight to the master
    REQUIRE(s.publish(e) == SE_OK);
    se_pdc_report r{};
    std::vector<se_pdc_track> tr(8);
    std::size_t n = 0;
    REQUIRE(se_pdc_report_get(e, &r, tr.data(), tr.size(), &n) == SE_OK);
    CHECK_EQ(r.total_frames, 600u);
    const auto l = play_left(e, 6000);
    std::vector<std::size_t> nz;
    for (std::size_t i = 0; i < l.size(); ++i)
        if (l[i] != 0.0f) nz.push_back(i);
    // The group (0.5), the group through the return (0.5) and track 5 (0.5).
    REQUIRE(nz.size() == 1u);
    CHECK_EQ(nz[0], std::size_t{2600});
    CHECK_NEAR(l[2600], 1.5f, 1e-6f);
}

// A line fed silence only to play out its tail goes idle once the tail has
// played (so a track whose clips ended stops paying for its lines).
TEST_CASE(routing_delay_line_goes_idle_after_its_tail) {
    constexpr uint32_t kDelay = 100, kBlock = 64;
    dstudio::DelayLine line(dstudio::DelayLine::capacity_for(kDelay, kBlock), 2);
    std::vector<float> l(kBlock, 1.0f), r(kBlock, 1.0f);
    line.process(l.data(), r.data(), kBlock, kDelay, 0);
    CHECK(!line.idle(kDelay, 0));
    int blocks = 0;
    float heard = 0.0f;
    while (!line.idle(kDelay, 0) && blocks < 10) {
        std::fill(l.begin(), l.end(), 0.0f);
        std::fill(r.begin(), r.end(), 0.0f);
        line.process(l.data(), r.data(), kBlock, kDelay, 0, /*silent_input=*/true);
        for (float x : l) heard += x;
        ++blocks;
    }
    CHECK(line.idle(kDelay, 0));
    CHECK_EQ(blocks, 2);                 // 100 frames of delay: two 64-frame blocks
    CHECK_NEAR(heard, 64.0f, 1e-6f);     // the whole block came out
}

// An output edge line belongs to its destination: moving the output to
// another return drops what the line still held for the old one.
TEST_CASE(routing_output_edge_line_does_not_follow_a_new_destination) {
    EngineHandle e;
    std::vector<float> imp(4000, 0.0f);
    imp[0] = 0.5f;
    const uint64_t a = load_asset(e, {imp});
    RSession s;
    s.audio(1, {clip(1, a, 2000, 0, 4000)});
    s.ret(2);        // A: track 1's output
    s.ret(3);        // B: track 1's send
    s.ret(4, 0.25f); // C: track 1's output after the republish
    // A silent track 700 frames late into A and C: those receive 700 frames
    // after B, so track 1's output reaches them through a 700-frame edge line.
    s.audio(5, {}).devices = {clap_latency(50, 700)};
    s.out(5, 2);
    s.send(5, 4, 1.0f);
    s.send(1, 3, 1.0f);
    s.out(1, 2);
    REQUIRE(s.publish(e) == SE_OK);
    se_transport_seek(e, 0);
    se_transport_play(e);
    const std::vector<float> first = render(e, 2304, 256); // the impulse (2000) is in the edge line
    float early = 0.0f;
    for (std::size_t i = 0; i < first.size(); i += 2) early += std::fabs(first[i]);
    CHECK_EQ(early, 0.0f);
    s.routes.back() = se_track_route{1, 4, 0, {}};
    REQUIRE(s.publish(e) == SE_OK);
    const std::vector<float> rest = render(e, 768, 256);
    std::vector<std::size_t> nz;
    for (std::size_t i = 0; i < rest.size(); i += 2)
        if (rest[i] != 0.0f) nz.push_back(2304 + i / 2);
    // Only the send through B, at 2000 + 700; nothing replayed into C.
    REQUIRE(nz.size() == 1u);
    CHECK_EQ(nz[0], std::size_t{2700});
    CHECK_NEAR(rest[2 * (2700 - 2304)], 0.5f, 1e-6f);
}

TEST_CASE(routing_export_matches_the_live_render_and_does_not_allocate) {
    EngineHandle e;
    const uint64_t a = load_asset(e, {sine(48000, 220.0, 48000.0), sine(48000, 330.0, 48000.0)});
    RSession s;
    s.audio(1, {clip(1, a, 0, 0, 48000)}, 0.7f).d.pan = 0.3f;
    s.audio(3, {clip(3, a, 100, 0, 40000)}, 0.5f).d.pan = -0.2f;
    s.ret(2, 0.8f).devices = {clap_latency(20, 333)};
    s.ret(4, 0.6f);
    s.send(1, 2, 0.4f);
    s.send(1, 4, 0.9f, true);
    s.send(3, 2, 0.25f);
    s.out(3, 4);
    REQUIRE(s.publish(e) == SE_OK);
    TempFile f("routing_export.wav");
    REQUIRE(se_export_wav(e, f.path.c_str(), 0, 20000) == SE_OK);
    dstudio::DecodedAudio d;
    std::string err;
    REQUIRE(dstudio::decode_wav_file(f.path.c_str(), d, err) == SE_OK);
    REQUIRE(d.frames == uint64_t{20000});
    se_transport_seek(e, 0);
    se_transport_play(e);
    std::vector<float> buf(2 * 256);
    REQUIRE(se_render_offline(e, buf.data(), 256) == SE_OK); // warm-up
    se_transport_seek(e, 0);
    // Live output is the whole mix late by L (333 frames); the export is aligned to the timeline.
    constexpr std::size_t kL = 333;
    std::vector<float> live;
    const long before = rt_alloc_count();
    for (int i = 0; i < 20500 / 250; ++i) {
        {
            RtAllocGuard g;
            REQUIRE(se_render_offline(e, buf.data(), 250) == SE_OK);
        }
        live.insert(live.end(), buf.begin(), buf.begin() + 500);
    }
    CHECK_EQ(rt_alloc_count() - before, 0L);
    double worst = 0.0;
    for (std::size_t i = 0; i < 20000; ++i) {
        worst = std::max(worst, static_cast<double>(std::fabs(live[2 * (i + kL)] - d.planar[0][i])));
        worst = std::max(worst, static_cast<double>(std::fabs(live[2 * (i + kL) + 1] - d.planar[1][i])));
    }
    CHECK(worst < 1e-6);
    // Something came through the returns at all.
    float peak = 0.0f;
    for (float v : live) peak = std::max(peak, std::fabs(v));
    CHECK(peak > 0.1f);
}
