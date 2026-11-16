// Trance Gate (engine/src/devices/trance_gate.cpp): step timing against
// the song position, envelope stages, swing, pattern choice, mix, free
// running while stopped and block splitting, through the shared harness.
#include "device_harness.hpp"
#include "shaper_layout.hpp"

using namespace devtest;
using namespace testutil;

namespace {

constexpr double kRate = 48000.0;
// 120 BPM: a quarter is 24000 frames, a 1/16 step 6000.
constexpr std::size_t kStep = 6000;

DeviceRig rig() {
    DeviceRig r("trance-gate", builtin_defaults("trance-gate"), kRate, 256);
    r.set_transport(true, 120.0, 0);
    return r;
}

// Pattern 1 as a step string (x = on) of its own length.
void pattern(DeviceRig& r, const char* s) {
    uint32_t m = 0, n = 0;
    for (; s[n] != 0; ++n)
        if (s[n] == 'x') m |= 1u << n;
    r.set("p1_steps", static_cast<float>(n));
    r.set("p1_lo", static_cast<float>(m & 0xffff));
    r.set("p1_hi", static_cast<float>(m >> 16));
}

Signal dc(std::size_t n) { return Signal(n, 1.0f); }

// First frame at or after `from` where |x| rises above `level`.
std::size_t onset(const Signal& x, std::size_t from, float level) {
    for (std::size_t i = from; i < x.size(); ++i)
        if (std::fabs(x[i]) > level) return i;
    return x.size();
}

} // namespace

TEST_CASE(trance_gate_steps_follow_the_pattern) {
    auto r = rig();
    pattern(r, "x...");
    r.set("decay", 1.f);
    r.set("sustain", 100.f);
    r.set("release", 1.f);
    const Signal out = r.run_mono(dc(8 * kStep));
    // Step 0 on (full after the 3 ms attack), steps 1-3 off (released in 1 ms).
    CHECK_NEAR(out[kStep / 2], 1.0, 1e-6);
    CHECK(peak(out, kStep + 100, 4 * kStep) < 1e-6);
    // The next cycle opens again at step 4.
    CHECK_NEAR(out[4 * kStep + kStep / 2], 1.0, 1e-6);
    // Linear attack: halfway through the 3 ms attack the gain is 0.5.
    CHECK_NEAR(out[72], 72.0 / 144.0, 0.01);
}

TEST_CASE(trance_gate_envelope_decays_to_sustain_and_releases_linearly) {
    auto r = rig();
    pattern(r, "x.");
    r.set("rate", 1.f); // 1/4: 24000-frame steps
    r.set("attack", 0.5f);
    r.set("decay", 100.f);
    r.set("sustain", 25.f);
    r.set("release", 100.f);
    const Signal out = r.run_mono(dc(48000));
    // Halfway down the decay: 1 - 0.75 / 2.
    CHECK_NEAR(out[24 + 2400], 0.625, 0.01);
    CHECK_NEAR(out[20000], 0.25, 1e-4);
    // Release from 0.25 to 0 in 100 ms (4800 frames).
    CHECK_NEAR(out[24000 + 2400], 0.125, 0.01);
    CHECK(out[24000 + 4900] == 0.0f);
}

TEST_CASE(trance_gate_consecutive_steps_retrigger) {
    auto r = rig();
    pattern(r, "xx");
    r.set("decay", 20.f);
    r.set("sustain", 0.f);
    const Signal out = r.run_mono(dc(2 * kStep));
    CHECK(out[kStep - 10] < 0.01);   // decayed to the zero sustain
    CHECK(out[kStep + 150] > 0.95);  // the next on step attacks again (3 ms)
}

TEST_CASE(trance_gate_swing_delays_odd_steps) {
    for (float swing : {0.f, 50.f, 100.f}) {
        auto r = rig();
        pattern(r, ".x");
        r.set("attack", 0.5f);
        r.set("swing", swing);
        const Signal out = r.run_mono(dc(2 * kStep));
        const auto expect = static_cast<std::size_t>(kStep + kStep * swing / 200.0f);
        const std::size_t at = onset(out, 0, 1e-4f);
        CHECK(at + 1 >= expect && at <= expect + 1);
    }
}

TEST_CASE(trance_gate_tracks_the_song_position) {
    // Starting the transport mid-pattern lands on the pattern's step there.
    auto r = rig();
    pattern(r, "x.x.");
    r.set("decay", 1.f);
    r.set("sustain", 100.f);
    r.set("release", 1.f);
    r.set_transport(true, 120.0, static_cast<int64_t>(kStep)); // step 1 (off)
    const Signal out = r.run_mono(dc(2 * kStep));
    CHECK(peak(out, 0, kStep) < 1e-6);
    CHECK_NEAR(out[kStep + kStep / 2], 1.0, 1e-6);
    // Faster tempo: steps shorten.
    auto f = rig();
    pattern(f, "x.");
    f.set("release", 1.f);
    f.set_transport(true, 240.0, 0);
    const Signal fast = f.run_mono(dc(kStep));
    CHECK(peak(fast, kStep / 2 + 100, kStep) < 1e-6);
}

TEST_CASE(trance_gate_pattern_choice_and_mix) {
    auto r = rig();
    r.set("pattern", 1.f); // pattern 2 default: 21845 = x.x.x.x.x.x.x.x.
    r.set("decay", 1.f);
    r.set("sustain", 100.f);
    r.set("release", 1.f);
    const Signal out = r.run_mono(dc(2 * kStep));
    CHECK_NEAR(out[kStep / 2], 1.0, 1e-6);
    CHECK(peak(out, kStep + 100, 2 * kStep) < 1e-6);

    auto dry = rig();
    dry.set("mix", 0.f);
    const Signal x = tone(4 * kStep, 220.0, kRate, 0.5);
    const Signal y = dry.run_mono(x);
    for (std::size_t i = 0; i < x.size(); ++i)
        if (y[i] != x[i]) {
            CHECK(y[i] == x[i]);
            break;
        }
    // Mix 50: the closed gate keeps half the level.
    auto half = rig();
    pattern(half, "x.");
    half.set("mix", 50.f);
    half.set("release", 1.f);
    const Signal h = half.run_mono(dc(2 * kStep));
    CHECK_NEAR(h[kStep + kStep / 2], 0.5, 1e-4);
}

TEST_CASE(trance_gate_free_runs_while_stopped) {
    auto r = rig();
    r.set_transport(false, 120.0);
    pattern(r, "x.");
    r.set("release", 1.f);
    const Signal out = r.run_mono(dc(4 * kStep));
    CHECK(out[kStep / 2] > 0.5f);
    CHECK(peak(out, kStep + 100, 2 * kStep) < 1e-6);
    CHECK(out[2 * kStep + kStep / 2] > 0.5f);
}

TEST_CASE(trance_gate_automation_splits_keep_the_grid) {
    // An automation event mid-block must not shift the step grid.
    auto a = rig();
    auto b = rig();
    for (DeviceRig* r : {&a, &b}) {
        pattern(*r, ".x.x");
        r->set("attack", 0.5f);
    }
    const Signal x = dc(4 * kStep);
    const auto pa = a.run(x, x).first;
    const uint32_t mix = param_index(b.desc(), "mix");
    std::vector<std::pair<std::size_t, std::pair<uint32_t, float>>> ev;
    for (std::size_t f = 100; f < x.size(); f += 997) ev.push_back({f, {mix, 100.f}});
    const auto pb = b.run(x, x, nullptr, nullptr, ev).first;
    for (std::size_t i = 0; i < x.size(); ++i)
        if (pa[i] != pb[i]) {
            CHECK(pa[i] == pb[i]);
            break;
        }
}

TEST_CASE(trance_gate_runs_in_the_engine) {
    EngineHandle e(48000, 256);
    const uint64_t a = load_asset(e, {Signal(96000, 0.5f), Signal(96000, 0.5f)});
    DeviceSession s;
    auto& t = s.add_track(1, a, 96000);
    std::vector<float> v = builtin_defaults("trance-gate");
    v[8] = 2.f;   // p1_steps
    v[9] = 1.f;   // p1_lo: x.
    v[5] = 1.f;   // release
    DeviceSession::add_device(t, 21, "trance-gate", v);
    REQUIRE(s.publish(e) == SE_OK);
    REQUIRE(se_transport_play(e) == SE_OK);
    const auto out = render(e, 4 * kStep, 256);
    // Interleaved stereo: on during step 0, silent during step 1.
    CHECK(std::fabs(out[2 * (kStep / 2)]) > 0.2f);
    double closed = 0;
    for (std::size_t i = 2 * (kStep + 200); i < 2 * 2 * kStep; ++i) closed = std::max(closed, static_cast<double>(std::fabs(out[i])));
    CHECK(closed < 1e-6);
}

// The engine hands devices the song tempo in quarter notes, whatever the
// click grid: a 6/8 metronome (an eighth per click) at 90 BPM must not make
// it 180, and the tempo holds with the metronome off (offline renders).
TEST_CASE(trance_gate_tempo_is_in_quarters_whatever_the_click_grid) {
    for (const bool click_on : {true, false}) {
        EngineHandle e(48000, 256);
        const uint64_t a = load_asset(e, {Signal(96000, 0.5f), Signal(96000, 0.5f)});
        DeviceSession s;
        auto& t = s.add_track(1, a, 96000);
        std::vector<float> v = builtin_defaults("trance-gate");
        v[8] = 2.f; // p1_steps
        v[9] = 1.f; // p1_lo: x.
        v[5] = 1.f; // release
        DeviceSession::add_device(t, 21, "trance-gate", v);
        REQUIRE(s.publish(e) == SE_OK);
        // 90 BPM: a quarter is 32000 frames, the 6/8 click an eighth (16000).
        CHECK_EQ(se_transport_set_tempo(e, 0.5), SE_ERR_INVALID_ARG);
        CHECK_EQ(se_transport_set_tempo(e, std::nan("")), SE_ERR_INVALID_ARG);
        CHECK_EQ(se_transport_set_tempo(e, 2e9), SE_ERR_INVALID_ARG);
        REQUIRE(se_transport_set_tempo(e, 32000.0) == SE_OK);
        se_metronome_desc m{};
        m.enabled = click_on ? 1 : 0;
        m.gain = 0.0f;
        m.frames_per_beat = 16000.0;
        m.beats_per_bar = 6;
        REQUIRE(se_metronome_set(e, &m) == SE_OK);
        REQUIRE(se_transport_play(e) == SE_OK);
        constexpr std::size_t step = 8000; // a 1/16 at 90 BPM
        const auto out = render(e, 4 * step, 256);
        // Open through step 0, closed through step 1, open again in step 2.
        CHECK(std::fabs(out[2 * (step - 200)]) > 0.2f);
        double closed = 0;
        for (std::size_t i = 2 * (step + 200); i < 2 * 2 * step; ++i)
            closed = std::max(closed, static_cast<double>(std::fabs(out[i])));
        CHECK(closed < 1e-6);
        CHECK(std::fabs(out[2 * (2 * step + step / 2)]) > 0.2f);
    }
}
