// Built-in device system: registry, state blobs, the generic RT-safety and
// robustness checks every registered device must pass, and the engine
// integration (chain, bypass, live parameter updates, readouts, automation,
// sidechain routing).
#include "device_harness.hpp"
#include "sampler_defaults.hpp"
#include "devices/synth_v2_params.hpp"

#include <set>

using namespace testutil;
using namespace devtest;


namespace {

// Neutral starting values for the generic checks: the middle of nothing is
// known here, so every device starts from all zeros except what the
// device-specific defaults table gives (devices must clamp anything).
std::vector<float> defaults_for(const dstudio::BuiltinDescriptor& d) {
    return {d.defaults.begin(), d.defaults.end()};
}

// Instruments with sample slots get a short loud sample in every slot so the
// generic checks exercise their voices.
void load_test_samples(DeviceRig& rig) {
    const dstudio::BuiltinDescriptor& d = rig.desc();
    std::vector<TestSample> s;
    for (uint32_t slot = 0; slot < d.sample_slots; ++slot) {
        TestSample t;
        t.slot = slot;
        t.l = noise(3000 + 97 * slot, 2.0f, slot + 1);
        if (slot % 2 == 0) t.r = noise(3000 + 97 * slot, 2.0f, slot + 7);
        if (slot == 0) t.slices = {{0.0f, 0.0f, 0.0f, 0.0f}, {0.4f, 6.0f, -1.0f, 12.0f}, {0.7f, -60.0f, 1.0f, -24.0f}};
        s.push_back(std::move(t));
    }
    rig.load_samples(std::move(s));
}

// Notes over the whole keyboard, pitch bend, sustain pedal and all-notes-off.
std::vector<NoteAt> test_notes(std::size_t n) {
    std::vector<NoteAt> v;
    uint8_t k = 0;
    for (std::size_t f = 0; f + 800 < n; f += 700, k = static_cast<uint8_t>((k + 13) % 128)) {
        v.push_back(note_on(f, k, static_cast<uint8_t>(1 + k % 127)));
        v.push_back(NoteAt{f + 10, 0xe0, static_cast<uint8_t>(k), static_cast<uint8_t>(127 - k % 128)});
        if (k % 5 == 0) v.push_back(NoteAt{f + 20, 0xb0, 64, static_cast<uint8_t>(k % 2 ? 127 : 0)});
        v.push_back(note_off(f + 400, k));
        if (k % 11 == 0) v.push_back(NoteAt{f + 450, 0xb0, 123, 0});
    }
    return v;
}

const std::vector<float> kComp = {-24.f, 4.f, 10.f, 100.f, 6.f, 0.f, 100.f, 0.f, 0.f, 0.f, 0.f};

} // namespace

TEST_CASE(builtin_registry_is_consistent) {
    std::set<std::string> ids;
    std::size_t n = 0;
    const se_builtin_desc* c = se_builtin_registry(&n);
    REQUIRE(n == dstudio::builtin_registry().size());
    REQUIRE(n > 0);
    for (std::size_t i = 0; i < n; ++i) {
        CHECK(ids.insert(c[i].id).second);
        CHECK(c[i].param_count > 0);
        std::set<std::string> params;
        for (uint32_t p = 0; p < c[i].param_count; ++p) CHECK(params.insert(c[i].param_ids[p]).second);
        CHECK(dstudio::find_builtin(c[i].id) != nullptr);
        CHECK(defaults_for(dstudio::builtin_registry()[i]).size() == c[i].param_count);
    }
    CHECK(dstudio::find_builtin("no-such-device") == nullptr);
}

TEST_CASE(builtin_state_blobs_are_validated) {
    const uint8_t* v = nullptr;
    uint32_t count = 0;
    uint64_t sc = 0;
    auto ok = state_blob({1.f, 2.f}, 7);
    CHECK(dstudio::parse_builtin_state(ok.data(), ok.size(), v, count, sc) == nullptr);
    CHECK(count == 2 && sc == 7);
    CHECK(dstudio::parse_builtin_state(nullptr, 0, v, count, sc) == nullptr);
    auto short_blob = ok;
    short_blob.resize(10);
    CHECK(dstudio::parse_builtin_state(short_blob.data(), short_blob.size(), v, count, sc) != nullptr);
    auto bad_len = ok;
    bad_len.push_back(0);
    CHECK(dstudio::parse_builtin_state(bad_len.data(), bad_len.size(), v, count, sc) != nullptr);
    auto bad_magic = ok;
    bad_magic[0] ^= 1;
    CHECK(dstudio::parse_builtin_state(bad_magic.data(), bad_magic.size(), v, count, sc) != nullptr);
    auto nan = state_blob({1.f, std::nanf("")});
    CHECK(dstudio::parse_builtin_state(nan.data(), nan.size(), v, count, sc) != nullptr);
}

// Every registered device: finite output for silence, impulses, loud noise
// and extreme parameter values; no heap use on the process path at any
// block size, with automation events splitting blocks.
TEST_CASE(every_builtin_is_rt_safe_and_robust) {
    for (const dstudio::BuiltinDescriptor& d : dstudio::builtin_registry()) {
        DeviceRig rig(d.id, defaults_for(d), 48000.0, 512);
        const std::size_t n = 48000;
        if (d.sample_slots > 0) load_test_samples(rig);
        // Instruments play a chord with overlapping notes, pitch bend and
        // the sustain pedal through every check.
        std::vector<dstudio::MidiEvent> notes;
        if (d.instrument)
            for (uint32_t k = 0; k < 12; ++k) {
                const auto at = k * 4000;
                notes.push_back({at, 0x90, static_cast<uint8_t>(36 + 7 * k), 100, 0});
                notes.push_back({at + 10, 0xe0, 0, static_cast<uint8_t>(k * 10), 0});
                notes.push_back({at + 20, 0xb0, 64, static_cast<uint8_t>(k % 4 == 0 ? 127 : 0), 0});
                notes.push_back({at + 3500, 0x80, static_cast<uint8_t>(36 + 7 * k), 0, 0});
            }
        const auto midi = [&](std::size_t frames) {
            std::vector<dstudio::MidiEvent> m;
            for (const dstudio::MidiEvent& e : notes)
                if (e.offset < frames) m.push_back(e);
            return m;
        };
        auto [sl, sr] = rig.run(Signal(n, 0.0f), Signal(n, 0.0f), nullptr, nullptr, {}, midi(n));
        CHECK(all_finite(sl) && all_finite(sr));
        if (d.instrument) CHECK(peak(sl) > 0.01 && peak(sl) < 4.0);
        auto [il, ir] = rig.run(impulse(n), impulse(n));
        CHECK(all_finite(il) && all_finite(ir));
        auto [nl, nr] = rig.run(noise(n, 4.0f, 3), noise(n, 4.0f, 4));
        CHECK(all_finite(nl) && all_finite(nr));

        // Extreme values in both directions must be clamped by the processor.
        for (float extreme : {-1e6f, 1e6f, 0.0f}) {
            for (std::size_t p = 0; p < d.params.size(); ++p) rig.instance().set_value(static_cast<uint32_t>(p), extreme);
            auto [xl, xr] = rig.run(noise(4096, 1.0f, 5), noise(4096, 1.0f, 6), nullptr, nullptr, {}, midi(4096));
            CHECK(all_finite(xl) && all_finite(xr));
            if (d.instrument) {
                auto [yl, yr] = rig.play(8192, test_notes(8192));
                CHECK(all_finite(yl) && all_finite(yr));
            }
        }
        rig.instance().update_state(state_blob(defaults_for(d)).data(), state_blob(defaults_for(d)).size());

        // Allocation-free processing, odd block sizes and automation events.
        std::vector<float> l(512), r(512), scl(512, 0.25f), scr(512, 0.25f);
        std::vector<dstudio::ParamEvent> ev;
        for (uint32_t i = 0; i < 8; ++i) ev.push_back(dstudio::ParamEvent{i * 37, 0, static_cast<double>(defaults_for(d)[0])});
        dstudio::RtTransport tr;
        const long before = rt_alloc_count();
        {
            RtAllocGuard g;
            const uint32_t sizes[] = {512, 1, 333, 64, 255, 17};
            for (int k = 0; k < 120; ++k) {
                const uint32_t frames = sizes[k % 6];
                for (uint32_t i = 0; i < frames; ++i) l[i] = r[i] = 0.5f * std::sin(0.05f * static_cast<float>(i + k));
                if (k % 3 == 0) rig.instance().set_sidechain(scl.data(), scr.data());
                const auto ne = static_cast<uint32_t>(std::count_if(ev.begin(), ev.end(), [&](const dstudio::ParamEvent& e) { return e.offset < frames; }));
                tr.playing = k % 4 < 2; // tempo-synced devices: playing and stopped transport
                const dstudio::MidiEvent play[2] = {{0, static_cast<uint8_t>(k % 4 < 2 ? 0x90 : 0x80), static_cast<uint8_t>(40 + k % 24), 90, 0},
                                                  {frames - 1, 0x90, static_cast<uint8_t>(64 + k % 12), 70, 0}};
                if (d.instrument && k % 4 == 1) {
                    rig.instance().push_note(60, 90);
                    rig.instance().push_note(60, 0);
                }
                rig.instance().run(l.data(), r.data(), frames, d.instrument ? play : nullptr, d.instrument ? 2u : 0u, tr,
                                   ev.data(), k % 2 == 0 ? ne : 0);
                tr.position += frames;
                if (k % 40 == 39) rig.instance().processor().reset();
            }
        }
        CHECK_EQ(rt_alloc_count() - before, 0);
        // Readouts are finite.
        if (!rig.desc().readouts.empty())
            for (float v : rig.readouts()) CHECK(std::isfinite(v));
    }
}

TEST_CASE(builtin_devices_run_in_engine_chains) {
    EngineHandle e(48000, 256);
    const uint64_t a = load_asset(e, {sine(192000, 1000.0, 48000.0, 0.5), sine(192000, 1000.0, 48000.0, 0.5)});
    DeviceSession s;
    auto& t = s.add_track(1, a, 192000);
    t.d.pan = 0.0f;
    std::vector<float> loud = kComp;
    loud[0] = -40.f; // threshold
    loud[1] = 10.f;  // ratio
    loud[4] = 0.f;   // hard knee
    DeviceSession::add_device(t, 11, "compressor", loud);
    REQUIRE(s.publish(e) == SE_OK);
    REQUIRE(se_plugin_is_live(e, 11) == 1);
    REQUIRE(se_transport_play(e) == SE_OK);
    auto compressed = render(e, 24000, 256);
    // Bypass: same session, device disabled -> the dry sine.
    s.tracks[0].devices[0].enabled = 0;
    REQUIRE(s.publish(e) == SE_OK);
    REQUIRE(se_transport_seek(e, 0) == SE_OK);
    auto dry = render(e, 24000, 256);
    double pc = 0, pd = 0;
    for (std::size_t i = 2 * 12000; i < compressed.size(); i += 2) {
        pc = std::max(pc, static_cast<double>(std::fabs(compressed[i])));
        pd = std::max(pd, static_cast<double>(std::fabs(dry[i])));
    }
    CHECK(db(pd) - db(pc) > 15.0); // heavy compression of a -6 dBFS sine
    CHECK(se_plugin_is_live(e, 11) == 1);   // bypass keeps the instance

    // Readouts and live parameter updates.
    s.tracks[0].devices[0].enabled = 1;
    REQUIRE(s.publish(e) == SE_OK);
    render(e, 4800, 256);
    float ro[4] = {};
    size_t nro = 0;
    REQUIRE(se_builtin_readouts(e, 11, ro, 4, &nro) == SE_OK);
    CHECK(nro == 3);
    CHECK(ro[2] < -10.0f); // gain reduction (dB)
    CHECK(ro[0] > -8.0f && ro[0] < -4.0f); // input peak ~ -6 dBFS
    REQUIRE(se_builtin_set_param(e, 11, 1, 1.0f) == SE_OK); // ratio 1:1 -> no reduction
    render(e, 24000, 256);
    REQUIRE(se_builtin_readouts(e, 11, ro, 4, &nro) == SE_OK);
    CHECK(ro[2] > -0.5f);
    CHECK(se_builtin_set_param(e, 11, 99, 1.0f) == SE_ERR_INVALID_ARG);
    CHECK(se_builtin_set_param(e, 11, 0, std::nanf("")) == SE_ERR_INVALID_ARG);
    CHECK(se_builtin_set_param(e, 12345, 0, 0.0f) == SE_ERR_NOT_FOUND);
    // A publish re-applies the device's state (ratio 10 again).
    REQUIRE(s.publish(e) == SE_OK);
    render(e, 24000, 256);
    REQUIRE(se_builtin_readouts(e, 11, ro, 4, &nro) == SE_OK);
    CHECK(ro[2] < -10.0f);

    // Unknown ids pass audio through and report a load error.
    DeviceSession bad;
    auto& bt = bad.add_track(1, a, 48000);
    DeviceSession::add_device(bt, 21, "no-such-device", {});
    REQUIRE(bad.publish(e) == SE_OK);
    se_plugin_message msgs[4];
    size_t nm = 0;
    REQUIRE(se_plugin_load_errors(e, msgs, 4, &nm) == SE_OK);
    CHECK(nm == 1);
    // Invalid state is rejected at publish.
    DeviceSession inval;
    auto& it = inval.add_track(1, a, 48000);
    DeviceSession::add_device(it, 31, "compressor", kComp);
    inval.tracks[0].states[0].pop_back();
    CHECK(inval.publish(e) == SE_ERR_INVALID_ARG);
}

TEST_CASE(builtin_automation_lanes_carry_plain_values) {
    EngineHandle e(48000, 256);
    const uint64_t a = load_asset(e, {sine(96000, 500.0, 48000.0, 0.5), sine(96000, 500.0, 48000.0, 0.5)});
    DeviceSession s;
    auto& t = s.add_track(1, a, 96000);
    std::vector<float> v = kComp;
    v[0] = -60.f;
    v[4] = 0.f;
    DeviceSession::add_device(t, 11, "compressor", v);
    // Ratio automated from 1:1 (first half) to 20:1 (second half), log lane.
    DeviceSession::add_lane(t, 11, 1, {{0, 1.0f, 0.0f}, {47999, 1.0f, 0.0f}, {48000, 20.0f, 0.0f}}, true);
    REQUIRE(s.publish(e) == SE_OK);
    REQUIRE(se_transport_play(e) == SE_OK);
    auto out = render(e, 96000, 256);
    double first = 0, second = 0;
    for (std::size_t i = 2 * 24000; i < 2 * 48000; i += 2) first = std::max(first, static_cast<double>(std::fabs(out[i])));
    for (std::size_t i = 2 * 72000; i < 2 * 96000; i += 2) second = std::max(second, static_cast<double>(std::fabs(out[i])));
    CHECK_NEAR(db(first), db(0.5 * 0.7071), 0.5); // centre pan law, no reduction
    CHECK(db(first) - db(second) > 20.0);
    // A lane on a parameter index the device does not have is inactive.
    DeviceSession::add_lane(s.tracks[0], 11, 77, {{0, 1.0f, 0.0f}});
    CHECK(s.publish(e) == SE_OK);
}

TEST_CASE(builtin_sidechain_follows_source_track) {
    EngineHandle e(48000, 256);
    const uint64_t quiet = load_asset(e, {sine(48000, 300.0, 48000.0, 0.02), sine(48000, 300.0, 48000.0, 0.02)});
    const uint64_t loud = load_asset(e, {sine(48000, 80.0, 48000.0, 0.9), sine(48000, 80.0, 48000.0, 0.9)});
    std::vector<float> v = kComp;
    v[0] = -30.f; // threshold
    v[1] = 20.f;  // ratio
    v[8] = 1.f;   // sidechainEnable
    const auto level_of_track1 = [&](bool sidechain, bool reader_first) {
        DeviceSession s;
        // The reader (track 1) is listed before its source (track 2) to
        // prove the processing order puts sources first.
        auto add_reader = [&] {
            auto& r = s.add_track(1, quiet, 48000);
            DeviceSession::add_device(r, 11, "compressor", v, true, sidechain ? 2 : 0);
        };
        if (reader_first) add_reader();
        s.add_track(2, loud, 48000); // the tap is post-fader
        if (!reader_first) add_reader();
        REQUIRE(s.publish(e) == SE_OK);
        REQUIRE(se_transport_seek(e, 0) == SE_OK);
        REQUIRE(se_transport_play(e) == SE_OK);
        render(e, 12000, 256);
        float ro[3] = {};
        size_t n = 0;
        REQUIRE(se_builtin_readouts(e, 11, ro, 3, &n) == SE_OK);
        return ro[2];
    };
    CHECK(level_of_track1(false, true) > -0.5f);  // quiet input alone: no reduction
    CHECK(level_of_track1(true, true) < -10.0f);  // loud source drives the detector
    CHECK(level_of_track1(true, false) < -10.0f);
    // Mutual sidechains form a cycle: publish succeeds, one edge is cut.
    DeviceSession cyc;
    auto& t1 = cyc.add_track(1, quiet, 48000);
    DeviceSession::add_device(t1, 11, "compressor", v, true, 2);
    auto& t2 = cyc.add_track(2, loud, 48000);
    DeviceSession::add_device(t2, 12, "compressor", v, true, 1);
    CHECK(cyc.publish(e) == SE_OK);
    render(e, 2048, 256);
    // Only a sidechain on the cycle is cut: track 1 reads track 2, which is
    // in a cycle with track 3, and keeps its sidechain.
    {
        DeviceSession dep;
        auto& r = dep.add_track(1, quiet, 48000);
        DeviceSession::add_device(r, 21, "compressor", v, true, 2);
        auto& a = dep.add_track(2, loud, 48000);
        DeviceSession::add_device(a, 22, "compressor", v, true, 3);
        auto& b = dep.add_track(3, quiet, 48000);
        DeviceSession::add_device(b, 23, "compressor", v, true, 2);
        REQUIRE(dep.publish(e) == SE_OK);
        REQUIRE(se_transport_seek(e, 0) == SE_OK);
        REQUIRE(se_transport_play(e) == SE_OK);
        render(e, 12000, 256);
        float ro[3] = {};
        size_t n = 0;
        REQUIRE(se_builtin_readouts(e, 21, ro, 3, &n) == SE_OK);
        CHECK(ro[2] < -10.0f);
    }
    // A rejected source (unknown track, or the device's own track) reads
    // silence: the loud track's own audio must not drive the detector.
    for (uint64_t source : {99u, 2u}) {
        DeviceSession rej;
        auto& t = rej.add_track(2, loud, 48000);
        DeviceSession::add_device(t, 13, "compressor", v, true, source);
        REQUIRE(rej.publish(e) == SE_OK);
        REQUIRE(se_transport_seek(e, 0) == SE_OK);
        REQUIRE(se_transport_play(e) == SE_OK);
        render(e, 12000, 256);
        float ro[3] = {};
        size_t n = 0;
        REQUIRE(se_builtin_readouts(e, 13, ro, 3, &n) == SE_OK);
        CHECK(ro[2] > -0.5f);
    }
}

TEST_CASE(builtin_readouts_keep_peaks_across_automation_segments) {
    DeviceRig rig("compressor", kComp, 48000.0, 256);
    Signal x(256, 0.0f);
    for (std::size_t i = 0; i < 128; ++i) x[i] = 0.9f;
    rig.run(x, x, nullptr, nullptr, {{128, {0u, -24.0f}}});
    const auto ro = rig.readouts();
    CHECK(ro[0] > -3.0f); // the loud first segment, not the silent last one
}

TEST_CASE(builtin_engine_path_does_not_allocate) {
    EngineHandle e(48000, 256);
    const uint64_t a = load_asset(e, {sine(48000, 440.0, 48000.0, 0.5), sine(48000, 220.0, 48000.0, 0.5)});
    DeviceSession s;
    for (uint64_t t = 1; t <= 4; ++t) {
        auto& tr = s.add_track(t, a, 48000);
        DeviceSession::add_device(tr, t * 10, "compressor", kComp, true, t > 1 ? 1 : 0);
        DeviceSession::add_device(tr, t * 10 + 1, "compressor", kComp);
        DeviceSession::add_lane(tr, t * 10, 0, {{0, -40.0f, 0.0f}, {48000, -10.0f, 0.5f}});
    }
    REQUIRE(s.publish(e) == SE_OK);
    REQUIRE(se_transport_play(e) == SE_OK);
    std::vector<float> buf(2 * 1024);
    REQUIRE(se_render_offline(e, buf.data(), 256) == SE_OK);
    const long before = rt_alloc_count();
    for (int i = 0; i < 300; ++i) {
        if (i % 50 == 25) se_builtin_set_param(e, 10, 1, 2.0f + static_cast<float>(i % 7)); // main thread, outside the guard
        RtAllocGuard g;
        REQUIRE(se_render_offline(e, buf.data(), (i % 3 == 0) ? 1 : 256) == SE_OK);
    }
    CHECK_EQ(rt_alloc_count() - before, 0);
}

TEST_CASE(builtin_metering_reports_in_and_out_peaks_without_allocating) {
    EngineHandle e(48000, 256);
    const uint64_t a = load_asset(e, {sine(48000, 440.0, 48000.0, 0.5), sine(48000, 220.0, 48000.0, 0.25)});
    DeviceSession s;
    auto& tr = s.add_track(1, a, 48000);
    DeviceSession::add_device(tr, 10, "compressor", kComp);
    REQUIRE(s.publish(e) == SE_OK);
    REQUIRE(se_transport_play(e) == SE_OK);
    std::vector<float> buf(2 * 256);
    float lv[4] = {1.f, 1.f, 1.f, 1.f};

    // Off by default: nothing is measured.
    REQUIRE(se_render_offline(e, buf.data(), 256) == SE_OK);
    REQUIRE(se_builtin_levels(e, 10, lv) == SE_OK);
    for (float v : lv) CHECK_EQ(v, 0.0f);

    REQUIRE(se_builtin_set_metering(e, 10, 1) == SE_OK);
    const long before = rt_alloc_count();
    for (int i = 0; i < 8; ++i) {
        RtAllocGuard g;
        REQUIRE(se_render_offline(e, buf.data(), 256) == SE_OK);
    }
    CHECK_EQ(rt_alloc_count() - before, 0);
    REQUIRE(se_builtin_levels(e, 10, lv) == SE_OK);
    CHECK(lv[0] > 0.45f && lv[0] <= 0.5f); // input left: the 0.5 sine
    CHECK(lv[1] > 0.2f && lv[1] <= 0.25f); // input right: the 0.25 sine
    CHECK(lv[2] > 0.0f);
    CHECK(lv[3] > 0.0f);

    // Reading resets them; switching off stops the measuring.
    REQUIRE(se_builtin_levels(e, 10, lv) == SE_OK);
    for (float v : lv) CHECK_EQ(v, 0.0f);
    REQUIRE(se_builtin_set_metering(e, 10, 0) == SE_OK);
    REQUIRE(se_render_offline(e, buf.data(), 256) == SE_OK);
    REQUIRE(se_builtin_levels(e, 10, lv) == SE_OK);
    for (float v : lv) CHECK_EQ(v, 0.0f);

    CHECK_EQ(se_builtin_set_metering(e, 999, 1), SE_ERR_NOT_FOUND);
    CHECK_EQ(se_builtin_levels(e, 999, lv), SE_ERR_NOT_FOUND);
    CHECK_EQ(se_builtin_levels(e, 10, nullptr), SE_ERR_INVALID_ARG);
}
