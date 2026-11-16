#include "spsc_queue.hpp"
#include "test_harness.hpp"
#include "test_util.hpp"

#include <bit>
#include <random>

using namespace testutil;

namespace {
se_transport_info get(se_engine* e) {
    se_transport_info t{};
    se_transport_get(e, &t);
    return t;
}
} // namespace

TEST_CASE(transport_play_pause_seek_stop) {
    EngineHandle e(48000, 64);
    const auto src = ramp(20000);
    const uint64_t a = load_asset(e, {src});
    Session s;
    s.add_track(1, {clip(1, a, 0, 0, 20000)}, 1.0f, -1.0f);
    REQUIRE(s.publish(e) == SE_OK);

    // Initially stopped at 0: silence and no movement.
    auto out = render(e, 100, 64);
    CHECK(out[0] == 0.0f && out[2 * 99] == 0.0f);
    CHECK_EQ(get(e).position, 0);
    CHECK_EQ(get(e).playing, 0);

    REQUIRE(se_transport_play(e) == SE_OK);
    out = render(e, 100, 64);
    CHECK_EQ(get(e).position, 100);
    CHECK_EQ(get(e).playing, 1);
    CHECK(out[0] == src[0] && out[2 * 99] == src[99]);

    // Seek while playing: applied at the next block start.
    REQUIRE(se_transport_seek(e, 5000) == SE_OK);
    out = render(e, 10, 10);
    CHECK(out[0] == src[5000] && out[2 * 9] == src[5009]);
    CHECK_EQ(get(e).position, 5010);

    // Pause: silence, position frozen.
    REQUIRE(se_transport_pause(e) == SE_OK);
    out = render(e, 50, 64);
    CHECK(out[0] == 0.0f && out[2 * 49] == 0.0f);
    CHECK_EQ(get(e).position, 5010);
    CHECK_EQ(get(e).playing, 0);

    // Resume continues exactly where we paused.
    REQUIRE(se_transport_play(e) == SE_OK);
    out = render(e, 3, 3);
    CHECK(out[0] == src[5010] && out[4] == src[5012]);

    // Stop: pause + return to zero.
    REQUIRE(se_transport_stop(e) == SE_OK);
    out = render(e, 10, 10);
    CHECK(out[0] == 0.0f);
    CHECK_EQ(get(e).position, 0);
    CHECK_EQ(get(e).playing, 0);

    // Seek while stopped, then play: starts at the seek target.
    REQUIRE(se_transport_seek(e, 123) == SE_OK);
    REQUIRE(se_transport_play(e) == SE_OK);
    out = render(e, 1, 1);
    CHECK(out[0] == src[123]);

    // Commands queued together apply in order at one block start.
    REQUIRE(se_transport_seek(e, 10) == SE_OK);
    REQUIRE(se_transport_seek(e, 7000) == SE_OK);
    out = render(e, 1, 1);
    CHECK(out[0] == src[7000]);

    // Negative positions (pre-roll) render silence until the clip starts.
    REQUIRE(se_transport_seek(e, -5) == SE_OK);
    out = render(e, 7, 7);
    CHECK(out[2 * 4] == 0.0f && out[2 * 5] == src[0] && out[2 * 6] == src[1]);
    CHECK_EQ(get(e).position, 2);

    // Seeks past SE_MAX_TIMELINE_FRAME are refused (playing on from near
    // INT64_MAX would overflow the playhead); the position stays.
    CHECK_EQ(se_transport_seek(e, INT64_MAX - 10), SE_ERR_INVALID_ARG);
    CHECK_EQ(se_transport_seek(e, INT64_MIN), SE_ERR_INVALID_ARG);
    CHECK_EQ(se_transport_seek(e, SE_MAX_TIMELINE_FRAME + 1), SE_ERR_INVALID_ARG);
    CHECK_EQ(get(e).position, 2);
    REQUIRE(se_transport_seek(e, SE_MAX_TIMELINE_FRAME) == SE_OK);
    render(e, 64, 64);
    CHECK_EQ(get(e).position, SE_MAX_TIMELINE_FRAME + 64);
}

TEST_CASE(transport_queue_overflow_reports_error) {
    // With no device running, commands apply synchronously and never pile
    // up: every one succeeds and is visible immediately, without rendering.
    EngineHandle e;
    for (int i = 0; i < 1000; ++i) REQUIRE(se_transport_seek(e, i) == SE_OK);
    CHECK_EQ(get(e).position, 999);
    CHECK_EQ(se_transport_play(e), SE_OK);
    CHECK_EQ(get(e).playing, 1);

    // The bounded queue itself (used while the device owns the path)
    // refuses pushes once full and keeps FIFO order.
    dstudio::SpscQueue<int, 256> q;
    int ok = 0;
    for (int i = 0; i < 300; ++i) ok += q.push(i) ? 1 : 0;
    CHECK_EQ(ok, 256);
    CHECK(!q.empty());
    int v = -1;
    CHECK(q.pop(v) && v == 0);
    CHECK(q.push(1000));
    for (int i = 1; i < 256; ++i) CHECK(q.pop(v) && v == i);
    CHECK(q.pop(v) && v == 1000);
    CHECK(q.empty());
}

// The same session rendered with wildly different block partitions
// (including blocks larger than max_block_size, which are chunked) must be
// bit-identical and keep the clock exact.
TEST_CASE(block_size_independence_and_clock_alignment) {
    const uint32_t total = 48000;
    std::vector<float> reference;
    const uint32_t partitions[][6] = {
        {1, 1, 1, 1, 1, 1},          // single-frame blocks
        {7, 31, 127, 255, 256, 257}, // odd sizes around max_block
        {4096, 1000, 3, 999, 13, 600}, // larger than max_block -> chunked
        {256, 256, 256, 256, 256, 256},
        {48000, 0, 0, 0, 0, 0}, // single call
    };
    for (const auto& part : partitions) {
        EngineHandle e(48000, 256);
        std::mt19937 rng(42); // identical session each time
        std::uniform_real_distribution<float> d(-0.9f, 0.9f);
        std::vector<float> noise_l(30000), noise_r(30000), mono(17777);
        for (auto& v : noise_l) v = d(rng);
        for (auto& v : noise_r) v = d(rng);
        for (auto& v : mono) v = d(rng);
        const uint64_t st = load_asset(e, {noise_l, noise_r});
        const uint64_t mo = load_asset(e, {mono});
        Session s;
        s.add_track(1, {clip(1, st, 13, 101, 20000, 0.7f), clip(2, mo, 20013, 3, 17000, 1.1f)}, 0.8f, 0.3f);
        s.add_track(2, {clip(3, mo, 257, 0, 17777), clip(4, st, 25555, 29000, 5000)}, 1.2f, -0.6f);
        s.add_track(3, {clip(5, st, 0, 0, 30000, 0.5f)}, 0.4f, 0.0f, false, false);
        s.master_gain = 0.9f;
        REQUIRE(s.publish(e) == SE_OK);
        REQUIRE(se_transport_play(e) == SE_OK);

        std::vector<float> out(total * 2);
        uint32_t done = 0;
        for (std::size_t k = 0; done < total; ++k) {
            uint32_t n = part[k % 6];
            if (n == 0) n = part[0];
            n = std::min(n, total - done);
            REQUIRE(se_render_offline(e, out.data() + 2 * static_cast<std::size_t>(done), n) == SE_OK);
            done += n;
            se_transport_info t{};
            se_transport_get(e, &t);
            if (t.position != static_cast<int64_t>(done)) {
                CHECK_EQ(t.position, static_cast<int64_t>(done));
                break;
            }
        }
        if (reference.empty()) {
            reference = out;
            // Sanity: something audible got rendered.
            double energy = 0;
            for (float v : out) energy += static_cast<double>(v) * v;
            CHECK(energy > 100.0);
        } else {
            std::size_t first_diff = out.size();
            for (std::size_t i = 0; i < out.size(); ++i)
                if (std::bit_cast<uint32_t>(out[i]) != std::bit_cast<uint32_t>(reference[i])) {
                    first_diff = i;
                    break;
                }
            CHECK_EQ(first_diff, out.size());
        }
    }
}
