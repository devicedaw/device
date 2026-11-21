// Real-time safety: the process path must not touch the heap.
//
// test_main.cpp replaces global operator new/delete and counts calls made
// while the current thread is inside an RtAllocGuard. We put the guard
// around se_render_offline, which runs exactly the code the device callback
// runs (command drain, graph acquire, render_block, meters, stats).
#include "devices/device_harness.hpp"
#include "test_harness.hpp"
#include "test_util.hpp"

#include <atomic>
#include <cmath>
#include <memory>
#include <thread>

using namespace testutil;

void* volatile g_alloc_sink = nullptr;

TEST_CASE(rt_alloc_counter_is_live) {
    // Prove the instrumentation actually catches allocations.
    const long before = rt_alloc_count();
    {
        RtAllocGuard g;
        // Direct operator new call (not a new-expression), so the compiler
        // may not elide it; the volatile store keeps the pointer escaping.
        void* p = ::operator new(64);
        g_alloc_sink = p;
        ::operator delete(p);
    }
    CHECK_EQ(rt_alloc_count() - before, 1);
    CHECK(rt_free_count() > 0);
}

TEST_CASE(rt_render_path_does_not_allocate) {
    EngineHandle e(48000, 256);
    std::vector<uint64_t> assets;
    for (int i = 0; i < 4; ++i)
        assets.push_back(load_asset(e, {sine(48000, 220.0 * (i + 1), 48000.0, 0.2), sine(48000, 330.0, 48000.0)}));
    assets.push_back(load_asset(e, {sine(30000, 100.0, 48000.0)}));
    Session s;
    for (int t = 0; t < 32; ++t) {
        std::vector<se_clip_desc> clips;
        for (int c = 0; c < 4; ++c)
            clips.push_back(clip(static_cast<uint64_t>(c), assets[static_cast<std::size_t>((t + c) % 5)],
                                 c * 9000 + t, c * 7, 12000, 0.8f));
        s.add_track(static_cast<uint64_t>(t + 1), std::move(clips), 0.7f, static_cast<float>(t % 5) * 0.5f - 1.0f,
                    t % 9 == 0, false);
    }
    REQUIRE(s.publish(e) == SE_OK);

    std::vector<float> buf(2 * 8192);
    // Warm up (first-touch of any lazily-initialised state outside the guard).
    REQUIRE(se_transport_play(e) == SE_OK);
    REQUIRE(se_render_offline(e, buf.data(), 256) == SE_OK);

    const long allocs_before = rt_alloc_count();
    const long frees_before = rt_free_count();
    const uint32_t sizes[] = {256, 1, 333, 8192 /* > max_block: chunked */, 64, 255};
    for (int i = 0; i < 600; ++i) {
        // Transport commands are queued outside the guard (producer side may lock).
        if (i % 100 == 50) se_transport_seek(e, i * 100);
        if (i % 100 == 70) se_transport_pause(e);
        if (i % 100 == 75) se_transport_play(e);
        if (i % 200 == 199) se_transport_stop(e), se_transport_play(e);
        RtAllocGuard g;
        if (se_render_offline(e, buf.data(), sizes[i % 6]) != SE_OK) {
            CHECK(false);
            break;
        }
    }
    CHECK_EQ(rt_alloc_count() - allocs_before, 0);
    CHECK_EQ(rt_free_count() - frees_before, 0);

    // Also with a snapshot swap between blocks (publish outside the guard).
    Session s2;
    s2.add_track(1, {clip(1, assets[0], 0, 0, 10000)});
    REQUIRE(s2.publish(e) == SE_OK);
    {
        RtAllocGuard g;
        for (int i = 0; i < 50; ++i) se_render_offline(e, buf.data(), 256);
    }
    CHECK_EQ(rt_alloc_count() - allocs_before, 0);
    CHECK_EQ(rt_free_count() - frees_before, 0);
}

// Loop wrapping (mid-block, repeatedly), the metronome (onsets, tails across
// blocks and wraps, voice stealing) and peek-window advancement on the RT
// path, while another thread hammers se_meters_peek / se_meters_read and a
// third pushes transport/loop/metronome commands that the render drains.
TEST_CASE(rt_loop_metronome_peek_do_not_allocate) {
    EngineHandle e(48000, 256);
    std::vector<uint64_t> assets;
    for (int i = 0; i < 4; ++i) assets.push_back(load_asset(e, {sine(48000, 220.0 * (i + 1), 48000.0, 0.2)}));
    Session s;
    for (int t = 0; t < 16; ++t) {
        std::vector<se_clip_desc> clips;
        for (int c = 0; c < 3; ++c)
            clips.push_back(clip(static_cast<uint64_t>(c), assets[static_cast<std::size_t>((t + c) % 4)],
                                 c * 7000 + t, c * 3, 9000, 0.6f));
        s.add_track(static_cast<uint64_t>(t + 1), std::move(clips), 0.5f, 0.0f);
    }
    REQUIRE(s.publish(e) == SE_OK);

    se_metronome_desc m{};
    m.enabled = 1;
    m.gain = 0.5f;
    m.frames_per_beat = 333.7; // dense: overlapping-free but many onsets per block
    m.beats_per_bar = 3;
    REQUIRE(se_metronome_set(e, &m) == SE_OK);
    REQUIRE(se_transport_set_loop(e, 1, 1000, 1777) == SE_OK);
    REQUIRE(se_transport_play(e) == SE_OK);

    std::vector<float> buf(2 * 8192);
    REQUIRE(se_render_offline(e, buf.data(), 256) == SE_OK); // warm-up

    std::atomic<bool> stop{false};
    std::thread peeker([&] {
        se_meter mm[32];
        std::size_t n = 0;
        while (!stop.load()) {
            se_meters_peek(e, mm, 32, &n);
            if ((n & 1) == 0) se_meters_read(e, mm, 32, &n);
        }
    });
    std::thread commander([&] {
        int i = 0;
        while (!stop.load()) {
            if (i % 4 == 0) se_transport_set_loop(e, 1, 500 + i % 300, 1600 + i % 500);
            if (i % 7 == 0) se_transport_seek(e, 900 + i % 400);
            if (i % 11 == 0) {
                se_metronome_desc d = m;
                d.frames_per_beat = 100.0 + i % 400; // < click length: overlapping voices + stealing
                se_metronome_set(e, &d);
            }
            ++i;
            std::this_thread::yield();
        }
    });

    const long allocs_before = rt_alloc_count();
    const long frees_before = rt_free_count();
    const uint32_t sizes[] = {256, 1, 333, 8192, 64, 255, 77};
    for (int i = 0; i < 1500; ++i) {
        RtAllocGuard g;
        if (se_render_offline(e, buf.data(), sizes[i % 7]) != SE_OK) {
            CHECK(false);
            break;
        }
    }
    stop = true;
    peeker.join();
    commander.join();
    CHECK_EQ(rt_alloc_count() - allocs_before, 0);
    CHECK_EQ(rt_free_count() - frees_before, 0);
    se_transport_info t{};
    se_transport_get(e, &t);
    CHECK(t.playing == 1);
}

// A tempo map (se_session_publish_v12): the metronome on the map, a synced LFO
// asking for the song position every modulation step, seeks into the ramp and
// a republished map between blocks (publish outside the guard).
TEST_CASE(rt_tempo_map_does_not_allocate) {
    EngineHandle e(48000, 256);
    const uint64_t a = load_asset(e, {sine(48000, 220.0, 48000.0, 0.2)});
    // 600 BPM for 8 quarters (48 000 frames), then a ramp from 600 to 200 over 8 quarters.
    const double ramp_len = 60.0 * 48000.0 * 8.0 / (200.0 - 600.0) * std::log(200.0 / 600.0);
    const std::vector<se_tempo_segment> map = {{0.0, 0.0, 600.0, 600.0}, {48000.0, 8.0, 600.0, 200.0},
                                               {48000.0 + ramp_len, 16.0, 200.0, 200.0}};
    devtest::DeviceSession s;
    auto& t = s.add_track(1, a, 48000);
    devtest::DeviceSession::add_device_state(t, 40, "lfo", devtest::route_blob({0.f, 1.f, 1.f, 8.f, 100.f, 0.f}, {}));
    REQUIRE(s.publish(e, map, 0.5) == SE_OK);
    se_metronome_desc m{};
    m.enabled = 1;
    m.gain = 0.5f;
    m.frames_per_beat = 24000.0;
    m.beats_per_bar = 4;
    REQUIRE(se_metronome_set(e, &m) == SE_OK);
    REQUIRE(se_transport_play(e) == SE_OK);
    std::vector<float> buf(2 * 8192);
    REQUIRE(se_render_offline(e, buf.data(), 256) == SE_OK); // warm-up

    const long allocs_before = rt_alloc_count();
    const long frees_before = rt_free_count();
    const uint32_t sizes[] = {256, 1, 333, 8192, 64, 255, 77};
    for (int i = 0; i < 800; ++i) {
        if (i % 100 == 60) se_transport_seek(e, 50000 + i * 31);
        if (i == 400) REQUIRE(s.publish(e, map, 1.0) == SE_OK);
        RtAllocGuard g;
        if (se_render_offline(e, buf.data(), sizes[i % 7]) != SE_OK) {
            CHECK(false);
            break;
        }
    }
    CHECK_EQ(rt_alloc_count() - allocs_before, 0);
    CHECK_EQ(rt_free_count() - frees_before, 0);
}
