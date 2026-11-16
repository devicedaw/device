// Offline transport, non-destructive meters (se_meters_peek), loop playback
// and the metronome.
#include "test_harness.hpp"
#include "test_util.hpp"
#include "wav.hpp"

#include <atomic>
#include <bit>
#include <chrono>
#include <cmath>
#include <thread>

using namespace testutil;

namespace {

se_transport_info tget(se_engine* e) {
    se_transport_info t{};
    se_transport_get(e, &t);
    return t;
}

se_meter peek_master(se_engine* e) {
    se_meter m[4]{};
    std::size_t n = 0;
    se_meters_peek(e, m, 4, &n);
    return m[0];
}

int32_t set_metronome(se_engine* e, bool on, float gain, double fpb, uint32_t bpb) {
    se_metronome_desc d{};
    d.enabled = on ? 1 : 0;
    d.gain = gain;
    d.frames_per_beat = fpb;
    d.beats_per_bar = bpb;
    return se_metronome_set(e, &d);
}

// Renders `total` frames cycling through odd block sizes.
std::vector<float> render_odd(se_engine* e, uint32_t total, std::initializer_list<uint32_t> sizes) {
    std::vector<float> out(static_cast<std::size_t>(total) * 2);
    const std::vector<uint32_t> sz(sizes);
    uint32_t done = 0;
    for (std::size_t k = 0; done < total; ++k) {
        const uint32_t n = std::min(sz[k % sz.size()], total - done);
        if (se_render_offline(e, out.data() + 2 * static_cast<std::size_t>(done), n) != SE_OK)
            throw std::runtime_error("render failed");
        done += n;
    }
    return out;
}

// Frames (left channel) where a click starts: non-zero after a zero (or at 0).
std::vector<int64_t> onsets(const std::vector<float>& inter) {
    std::vector<int64_t> v;
    const std::size_t frames = inter.size() / 2;
    for (std::size_t i = 0; i < frames; ++i)
        if (inter[2 * i] != 0.0f && (i == 0 || inter[2 * (i - 1)] == 0.0f)) v.push_back(static_cast<int64_t>(i));
    return v;
}

int zero_crossings(const std::vector<float>& inter, std::size_t from, std::size_t n) {
    int c = 0;
    for (std::size_t i = from + 1; i < from + n; ++i)
        if ((inter[2 * i - 2] < 0.0f) != (inter[2 * i] < 0.0f)) ++c;
    return c;
}

} // namespace

// ---------------------------------------------------------------- #1 offline transport

TEST_CASE(offline_transport_applies_immediately) {
    EngineHandle e(48000, 64);
    REQUIRE(se_transport_seek(e, 12345) == SE_OK);
    CHECK_EQ(tget(e).position, 12345); // no render in between
    REQUIRE(se_transport_play(e) == SE_OK);
    CHECK_EQ(tget(e).playing, 1);
    REQUIRE(se_transport_pause(e) == SE_OK);
    CHECK_EQ(tget(e).playing, 0);
    CHECK_EQ(tget(e).position, 12345);
    REQUIRE(se_transport_seek(e, -7) == SE_OK);
    CHECK_EQ(tget(e).position, -7);
    REQUIRE(se_transport_play(e) == SE_OK);
    REQUIRE(se_transport_stop(e) == SE_OK);
    CHECK_EQ(tget(e).position, 0);
    CHECK_EQ(tget(e).playing, 0);

    // Rendering still advances from the synchronously applied state.
    REQUIRE(se_transport_seek(e, 100) == SE_OK);
    REQUIRE(se_transport_play(e) == SE_OK);
    render(e, 50, 7);
    CHECK_EQ(tget(e).position, 150);
    REQUIRE(se_transport_seek(e, 3) == SE_OK);
    CHECK_EQ(tget(e).position, 3);
}

// Commands pushed while another thread renders offline are either applied
// by that render or synchronously after it releases the path: none may be
// stranded in the queue (Dekker handshake in Engine::release_rt_owner).
TEST_CASE(offline_transport_no_stranded_commands_under_concurrent_render) {
    EngineHandle e(48000, 64);
    std::atomic<bool> stop{false};
    std::thread renderer([&] {
        std::vector<float> buf(2 * 256);
        uint32_t sizes[] = {1, 17, 64, 200};
        std::size_t k = 0;
        while (!stop.load()) se_render_offline(e, buf.data(), sizes[k++ % 4]);
    });
    // Transport stays paused, so the final position must be the last seek.
    // A full queue (the renderer descheduled under CPU load) is the
    // documented SE_ERR_STATE: retry the same seek until it is accepted.
    for (int round = 0; round < 2000; ++round) {
        int32_t rc = SE_OK;
        while ((rc = se_transport_seek(e, round)) == SE_ERR_STATE) std::this_thread::yield();
        REQUIRE(rc == SE_OK);
        if (round % 97 == 0) std::this_thread::yield();
    }
    const int64_t last = 1999;
    stop = true;
    renderer.join();
    CHECK_EQ(tget(e).position, last);

    // Once the renderer has gone, a command is visible right away again.
    REQUIRE(se_transport_seek(e, 42) == SE_OK);
    CHECK_EQ(tget(e).position, 42);
}

// Device start/stop with commands in flight. Skipped when no output device
// can be opened (CI, Linux/Windows stubs).
TEST_CASE(transport_commands_across_device_start_stop) {
    EngineHandle e(48000, 256);
    if (se_device_start(e) != SE_OK) {
        const std::string why = std::string("no audio output device could be opened: ") + se_last_error();
        SKIP(why.c_str());
    }
    // While running, commands are queued for the callback.
    REQUIRE(se_transport_seek(e, 1000) == SE_OK);
    REQUIRE(se_device_stop(e) == SE_OK);
    // Whether the callback consumed it or device_stop drained it, it is
    // visible now (the transport is paused, so the position is exact).
    CHECK_EQ(tget(e).position, 1000);
    REQUIRE(se_transport_seek(e, 77) == SE_OK);
    CHECK_EQ(tget(e).position, 77);

    // Stress: start/stop cycles on one thread (with gaps where the device
    // is off, so commands switch between queued and synchronous paths)
    // while this thread keeps pushing seeks.
    std::atomic<bool> done{false};
    std::atomic<int> start_fail{0};
    std::thread toggler([&] {
        for (int i = 0; i < 8; ++i) {
            if (se_device_start(e) != SE_OK) start_fail++;
            std::this_thread::sleep_for(std::chrono::milliseconds(12));
            se_device_stop(e);
            std::this_thread::sleep_for(std::chrono::milliseconds(4));
        }
        done = true;
    });
    // While the device runs the 256-entry queue can fill (SE_ERR_STATE, the
    // documented behaviour); the last *accepted* seek must win.
    int64_t last = 77;
    int accepted = 0, rejected = 0;
    for (int i = 0; !done.load(); ++i) {
        const int32_t rc = se_transport_seek(e, i);
        if (rc == SE_OK) {
            last = i;
            ++accepted;
        } else if (rc == SE_ERR_STATE) {
            ++rejected;
        } else {
            CHECK_EQ(rc, SE_OK);
        }
        std::this_thread::sleep_for(std::chrono::microseconds(20));
    }
    toggler.join();
    std::fprintf(stderr, "  seeks accepted %d, rejected (queue full) %d\n", accepted, rejected);
    REQUIRE(se_device_stop(e) == SE_OK);
    CHECK_EQ(start_fail.load(), 0);
    CHECK_EQ(tget(e).position, last);
}

// ---------------------------------------------------------------- #2 meters_peek

TEST_CASE(meters_peek_is_non_destructive_and_windowed) {
    EngineHandle e(48000, 256);
    const uint64_t a = load_asset(e, {constant(1000, 0.5f)});
    Session s;
    s.add_track(7, {clip(1, a, 0, 0, 1000)}, 1.0f, -1.0f); // hard left: L = 0.5, R = 0
    REQUIRE(s.publish(e) == SE_OK);
    REQUIRE(se_transport_play(e) == SE_OK);
    render(e, 64, 64); // one block of signal
    REQUIRE(se_transport_pause(e) == SE_OK);

    // Polling many times between renders keeps returning the peak.
    for (int i = 0; i < 5; ++i) {
        se_meter m[4]{};
        std::size_t n = 0;
        REQUIRE(se_meters_peek(e, m, 4, &n) == SE_OK);
        REQUIRE(n == 2);
        CHECK_EQ(m[0].track_id, uint64_t{0});
        CHECK_NEAR(m[0].peak_l, 0.5, 1e-6);
        CHECK_EQ(m[0].peak_r, 0.0f);
        CHECK_EQ(m[1].track_id, uint64_t{7});
        CHECK_NEAR(m[1].peak_l, 0.5, 1e-6);
    }
    // se_meters_read still resets and is unaffected by peeking.
    se_meter r[4]{};
    std::size_t rn = 0;
    REQUIRE(se_meters_read(e, r, 4, &rn) == SE_OK);
    CHECK_NEAR(r[0].peak_l, 0.5, 1e-6);
    CHECK_NEAR(r[1].peak_l, 0.5, 1e-6);
    REQUIRE(se_meters_read(e, r, 4, &rn) == SE_OK);
    CHECK_EQ(r[0].peak_l, 0.0f);
    CHECK_EQ(r[1].peak_l, 0.0f);
    CHECK_NEAR(peek_master(e).peak_l, 0.5, 1e-6); // read did not clear the window

    // Window: 5 buckets x 480 frames at 48 kHz. The signal ended 64 frames
    // into bucket 0; it drops out after 5 bucket advances (2336 frames).
    render(e, 1900, 100); // paused -> silence
    CHECK_NEAR(peek_master(e).peak_l, 0.5, 1e-6);
    render(e, 500, 100); // 2400 frames of silence in total
    CHECK_EQ(peek_master(e).peak_l, 0.0f);
    se_meter m[4]{};
    std::size_t n = 0;
    REQUIRE(se_meters_peek(e, m, 4, &n) == SE_OK);
    CHECK_EQ(m[1].peak_l, 0.0f);

    // capacity 0 / partial capacity behave like se_meters_read.
    REQUIRE(se_meters_peek(e, nullptr, 0, &n) == SE_OK);
    CHECK_EQ(n, std::size_t{0});
    CHECK_EQ(se_meters_peek(e, nullptr, 3, &n), SE_ERR_INVALID_ARG);
}

TEST_CASE(meters_peek_large_block_counts_as_one_bucket) {
    EngineHandle e(48000, 4096);
    const uint64_t a = load_asset(e, {constant(10000, 0.25f)});
    Session s;
    s.add_track(1, {clip(1, a, 0, 0, 10000)}, 1.0f, -1.0f);
    REQUIRE(s.publish(e) == SE_OK);
    REQUIRE(se_transport_play(e) == SE_OK);
    render(e, 4096, 4096); // one block > window: still visible afterwards
    CHECK_NEAR(peek_master(e).peak_l, 0.25, 1e-6);
    REQUIRE(se_transport_pause(e) == SE_OK);
    render(e, 4096, 4096); // one big silent block flushes the whole window
    CHECK_EQ(peek_master(e).peak_l, 0.0f);
}

// ---------------------------------------------------------------- #3 loop

TEST_CASE(loop_wrap_is_sample_accurate) {
    EngineHandle e(48000, 64);
    const auto src = ramp(20000);
    const uint64_t a = load_asset(e, {src});
    Session s;
    s.add_track(1, {clip(1, a, 0, 0, 20000)}, 1.0f, -1.0f); // L = src exactly, R = 0
    REQUIRE(s.publish(e) == SE_OK);

    const int64_t start = 1000, end = 1137; // 137 frames: wraps inside blocks
    CHECK_EQ(se_transport_set_loop(e, 1, 100, 100), SE_ERR_INVALID_ARG);
    CHECK_EQ(se_transport_set_loop(e, 1, 200, 100), SE_ERR_INVALID_ARG);
    CHECK_EQ(se_transport_set_loop(e, 0, 200, 100), SE_OK); // disabling ignores the range
    REQUIRE(se_transport_set_loop(e, 1, start, end) == SE_OK);
    REQUIRE(se_transport_seek(e, 990) == SE_OK); // enters the loop from before it
    REQUIRE(se_transport_play(e) == SE_OK);

    // Expected: the concatenation of timeline frames with the wrap applied.
    const uint32_t total = 1000;
    std::vector<int64_t> expect_pos(total);
    int64_t p = 990;
    for (uint32_t i = 0; i < total; ++i) {
        expect_pos[i] = p;
        if (++p == end) p = start;
    }
    std::vector<float> out(2 * total);
    const uint32_t sizes[] = {7, 13, 31, 64, 100 /* > max_block: chunked */, 1, 59};
    uint32_t done = 0;
    bool in_range = true;
    for (std::size_t k = 0; done < total; ++k) {
        const uint32_t n = std::min(sizes[k % 7], total - done);
        REQUIRE(se_render_offline(e, out.data() + 2 * static_cast<std::size_t>(done), n) == SE_OK);
        done += n;
        const int64_t pos = tget(e).position;
        if (done >= 10 && (pos < start || pos >= end)) in_range = false;
        if (pos != (done < total ? expect_pos[done] : p)) {
            CHECK_EQ(pos, done < total ? expect_pos[done] : p);
            break;
        }
    }
    CHECK(in_range);
    std::size_t mismatch = total;
    for (uint32_t i = 0; i < total; ++i)
        if (std::bit_cast<uint32_t>(out[2 * i]) !=
                std::bit_cast<uint32_t>(src[static_cast<std::size_t>(expect_pos[i])]) ||
            out[2 * i + 1] != 0.0f) {
            mismatch = i;
            break;
        }
    CHECK_EQ(mismatch, std::size_t{total});

    // Disabling mid-play continues linearly past the old end.
    REQUIRE(se_transport_seek(e, 1130) == SE_OK);
    REQUIRE(se_transport_set_loop(e, 0, 0, 0) == SE_OK);
    auto lin = render(e, 20, 7);
    for (int i = 0; i < 20; ++i) CHECK(lin[2 * static_cast<std::size_t>(i)] == src[1130 + static_cast<std::size_t>(i)]);
    CHECK_EQ(tget(e).position, 1150);

    // Loop enabled but playhead past the end: linear.
    REQUIRE(se_transport_set_loop(e, 1, start, end) == SE_OK);
    render(e, 100, 33);
    CHECK_EQ(tget(e).position, 1250);

    // Paused: no wrap, no movement.
    REQUIRE(se_transport_seek(e, 1136) == SE_OK);
    REQUIRE(se_transport_pause(e) == SE_OK);
    render(e, 10, 10);
    CHECK_EQ(tget(e).position, 1136);
}

TEST_CASE(loop_shorter_than_a_block_wraps_repeatedly) {
    EngineHandle e(48000, 256);
    const auto src = ramp(1000);
    const uint64_t a = load_asset(e, {src});
    Session s;
    s.add_track(1, {clip(1, a, 0, 0, 1000)}, 1.0f, -1.0f);
    REQUIRE(s.publish(e) == SE_OK);
    REQUIRE(se_transport_set_loop(e, 1, 10, 13) == SE_OK); // 3-frame loop
    REQUIRE(se_transport_seek(e, 10) == SE_OK);
    REQUIRE(se_transport_play(e) == SE_OK);
    auto out = render(e, 256, 256);
    bool ok = true;
    for (int i = 0; i < 256; ++i) ok = ok && out[2 * static_cast<std::size_t>(i)] == src[10 + static_cast<std::size_t>(i % 3)];
    CHECK(ok);
    CHECK_EQ(tget(e).position, 10 + 256 % 3);
}

// ---------------------------------------------------------------- #4 metronome

TEST_CASE(metronome_clicks_at_exact_frames_across_blocks) {
    EngineHandle e(48000, 64);
    const double fpb = 1000.5; // fractional: beat k at llround(k * fpb)
    CHECK_EQ(set_metronome(e, true, 0.5f, 0.5, 4), SE_ERR_INVALID_ARG);
    CHECK_EQ(set_metronome(e, true, 0.5f, fpb, 0), SE_ERR_INVALID_ARG);
    CHECK_EQ(set_metronome(e, true, -1.0f, fpb, 4), SE_ERR_INVALID_ARG);
    CHECK_EQ(set_metronome(e, true, 0.5f, std::nan(""), 4), SE_ERR_INVALID_ARG);
    // A huge grid would overflow the beat-frame arithmetic (mix_metronome hung).
    CHECK_EQ(set_metronome(e, true, 0.5f, 1e19, 4), SE_ERR_INVALID_ARG);
    CHECK_EQ(set_metronome(e, true, 0.5f, SE_MAX_FRAMES_PER_BEAT * 2, 4), SE_ERR_INVALID_ARG);
    CHECK_EQ(se_metronome_set(e, nullptr), SE_ERR_INVALID_ARG);
    REQUIRE(set_metronome(e, true, 0.5f, fpb, 4) == SE_OK);

    // Stopped: silent.
    auto out = render(e, 2000, 64);
    CHECK(onsets(out).empty());

    REQUIRE(se_transport_play(e) == SE_OK);
    const uint32_t total = 10000;
    out = render_odd(e, total, {7, 333, 1, 64, 129, 13, 200});
    std::vector<int64_t> expect;
    for (int64_t k = 0; std::llround(static_cast<double>(k) * fpb) < total; ++k)
        expect.push_back(std::llround(static_cast<double>(k) * fpb));
    const auto got = onsets(out);
    CHECK(got == expect);
    if (got != expect)
        for (std::size_t i = 0; i < std::min(got.size(), expect.size()); ++i)
            std::fprintf(stderr, "  onset %zu: got %lld expect %lld\n", i, static_cast<long long>(got[i]),
                         static_cast<long long>(expect[i]));

    // Block-size independent: identical to one big render.
    EngineHandle e2(48000, 64);
    REQUIRE(set_metronome(e2, true, 0.5f, fpb, 4) == SE_OK);
    REQUIRE(se_transport_play(e2) == SE_OK);
    const auto ref = render(e2, total, total);
    std::size_t diff = out.size();
    for (std::size_t i = 0; i < out.size(); ++i)
        if (std::bit_cast<uint32_t>(out[i]) != std::bit_cast<uint32_t>(ref[i])) {
            diff = i;
            break;
        }
    CHECK_EQ(diff, out.size());

    // Downbeat (k % 4 == 0) is 1.2 kHz, other beats 800 Hz: count zero
    // crossings over the 15 ms click (720 frames).
    const int zc_down = zero_crossings(out, static_cast<std::size_t>(expect[0]), 700);
    const int zc_beat = zero_crossings(out, static_cast<std::size_t>(expect[1]), 700);
    const int zc_down2 = zero_crossings(out, static_cast<std::size_t>(expect[4]), 700);
    CHECK_NEAR(zc_down / 2.0 / (700.0 / 48000.0), 1200.0, 100.0);
    CHECK_NEAR(zc_beat / 2.0 / (700.0 / 48000.0), 800.0, 100.0);
    CHECK_EQ(zc_down, zc_down2);
    // Left == right, and the click peak is ~gain.
    float peak = 0.0f;
    bool lr_equal = true;
    for (std::size_t i = 0; i < total; ++i) {
        peak = std::max(peak, std::fabs(out[2 * i]));
        lr_equal = lr_equal && out[2 * i] == out[2 * i + 1];
    }
    CHECK(lr_equal);
    CHECK(peak > 0.4f && peak <= 0.5f);

    // Disabled -> silent; pause -> silent.
    REQUIRE(set_metronome(e, false, 0.0f, 0.0, 0) == SE_OK); // values ignored when disabling
    CHECK(onsets(render(e, 5000, 64)).empty());
    REQUIRE(set_metronome(e, true, 0.5f, fpb, 4) == SE_OK);
    REQUIRE(se_transport_pause(e) == SE_OK);
    CHECK(onsets(render(e, 5000, 64)).empty());
}

TEST_CASE(metronome_is_after_master_gain_and_excluded_from_meters) {
    EngineHandle e(48000, 128);
    const uint64_t a = load_asset(e, {constant(5000, 0.8f)});
    Session s;
    s.add_track(1, {clip(1, a, 0, 0, 5000)});
    s.master_gain = 0.0f; // mix fully down: only the click remains
    REQUIRE(s.publish(e) == SE_OK);
    REQUIRE(set_metronome(e, true, 0.3f, 2000.0, 4) == SE_OK);
    REQUIRE(se_transport_play(e) == SE_OK);
    auto out = render(e, 4000, 128);
    float peak = 0.0f;
    for (float v : out) peak = std::max(peak, std::fabs(v));
    CHECK(peak > 0.25f && peak <= 0.3f);
    const auto o = onsets(out);
    CHECK(o.size() == 2 && o[0] == 0 && o[1] == 2000);
    CHECK_EQ(peek_master(e).peak_l, 0.0f); // meters measure the mix
}

TEST_CASE(metronome_across_loop_wrap) {
    EngineHandle e(48000, 64);
    const double fpb = 1000.0;
    REQUIRE(set_metronome(e, true, 0.5f, fpb, 4) == SE_OK);
    // Loop [2500, 5300): beats 3000 (k=3), 4000 (k=4, downbeat), 5000 (k=5).
    // Beat 5000's click (720 frames) is only 300 frames old at the wrap: its
    // tail must continue after the wrap, and 2500..2919 has no onset.
    const int64_t start = 2500, end = 5300;
    REQUIRE(se_transport_set_loop(e, 1, start, end) == SE_OK);
    REQUIRE(se_transport_play(e) == SE_OK);

    const uint32_t total = 12000;
    auto out = render_odd(e, total, {61, 7, 64, 333, 2, 129});
    std::vector<int64_t> tl(total); // timeline frame of each output frame
    int64_t p = 0;
    std::vector<int64_t> expect;
    for (uint32_t i = 0; i < total; ++i) {
        tl[i] = p;
        if (p % 1000 == 0) expect.push_back(i);
        if (++p == end) p = start;
    }
    CHECK(onsets(out) == expect);
    const int64_t pos = tget(e).position;
    CHECK(pos >= start && pos < end);

    // The click starting at timeline 5000 (k=5, a regular beat) must be the
    // exact same waveform as the one at 1000 (k=1), even though it straddles
    // the wrap.
    const std::size_t ref = 1000;
    std::size_t wrap_click = 0;
    for (int64_t o : expect)
        if (tl[static_cast<std::size_t>(o)] == 5000) {
            wrap_click = static_cast<std::size_t>(o);
            break;
        }
    REQUIRE(wrap_click != 0);
    bool same = true;
    for (std::size_t i = 0; i < 720; ++i) same = same && out[2 * (wrap_click + i)] == out[2 * (ref + i)];
    CHECK(same);
    CHECK(out[2 * (wrap_click + 300)] != 0.0f); // first frame after the wrap is still the tail
    CHECK_EQ(out[2 * (wrap_click + 720)], 0.0f); // and the click then ends
    // Downbeat at 4000 differs from the beat waveform.
    std::size_t down = 0;
    for (int64_t o : expect)
        if (tl[static_cast<std::size_t>(o)] == 4000) {
            down = static_cast<std::size_t>(o);
            break;
        }
    CHECK(zero_crossings(out, down, 700) > zero_crossings(out, ref, 700) + 6);
}

// ---------------------------------------------------------------- #5 export ignores loop/metronome

TEST_CASE(export_ignores_loop_and_metronome) {
    EngineHandle e(48000, 64);
    const auto src = ramp(6000);
    const uint64_t a = load_asset(e, {src});
    Session s;
    s.add_track(1, {clip(1, a, 0, 0, 6000)}, 1.0f, -1.0f);
    REQUIRE(s.publish(e) == SE_OK);
    REQUIRE(se_transport_set_loop(e, 1, 100, 300) == SE_OK);
    REQUIRE(set_metronome(e, true, 1.0f, 480.0, 4) == SE_OK);
    REQUIRE(se_transport_play(e) == SE_OK);
    render(e, 1000, 64); // live playback is looping with clicks

    TempFile f("export_loop_metro.wav");
    REQUIRE(se_export_wav(e, f.path.c_str(), 0, 5000) == SE_OK);
    dstudio::DecodedAudio d;
    std::string err;
    REQUIRE(dstudio::decode_wav_file(f.path.c_str(), d, err) == SE_OK);
    REQUIRE(d.frames == 5000);
    bool linear = true;
    for (std::size_t i = 0; i < 5000; ++i)
        linear = linear && d.planar[0][i] == src[i] && d.planar[1][i] == 0.0f;
    CHECK(linear);

    // Metronome only (no clips): export is silent.
    Session empty;
    REQUIRE(empty.publish(e) == SE_OK);
    TempFile f2("export_metro_only.wav");
    REQUIRE(se_export_wav(e, f2.path.c_str(), 0, 3000) == SE_OK);
    dstudio::DecodedAudio d2;
    REQUIRE(dstudio::decode_wav_file(f2.path.c_str(), d2, err) == SE_OK);
    bool silent = true;
    for (std::size_t i = 0; i < d2.frames; ++i) silent = silent && d2.planar[0][i] == 0.0f && d2.planar[1][i] == 0.0f;
    CHECK(silent);
    // ...while live playback of the same session does click.
    REQUIRE(se_transport_seek(e, 0) == SE_OK);
    REQUIRE(se_transport_set_loop(e, 0, 0, 0) == SE_OK);
    CHECK(!onsets(render(e, 1000, 64)).empty());
}
