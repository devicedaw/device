// Built-in device benchmark: per-block DSP cost of every registered device
// (every compressor character and dynamics mode, the Synth with held notes,
// the restoration devices), the modulation sources (LFO, chain head), the
// parameter update paths, and a 32-track session with two compressors per
// track, with and without an LFO moving both thresholds.
//
//   dstudio_device_bench [blocks]      (Release build; numbers in us per 256-frame block)
#include "../tests/devices/shaper_layout.hpp"
#include "devices/builtin.hpp"
#include "devices/synth_v2_params.hpp"
#include "platform/os.hpp"
#include "dstudio_engine.h"

#include <algorithm>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <filesystem>
#include <initializer_list>
#include <string>
#include <utility>
#include <vector>

using Clock = std::chrono::steady_clock;

namespace {

constexpr double kRate = 48000.0;
constexpr uint32_t kBlock = 256;
const std::vector<float> kCompressor = {-24.f, 4.f, 10.f, 100.f, 6.f, 0.f, 100.f, 0.f, 0.f, 0.f, 0.f};
// Defaults of the dynamics devices (registry order), see dstudio_core::builtin.
const std::vector<float> kLimiter = {-0.3f, 100.f, 5.f, 1.f, 100.f, 0.f, 0.f};
const std::vector<float> kGate = {-40.f, 10.f, 0.5f, 50.f, 100.f, 5.f, 80.f, 100.f};
const std::vector<float> kGlue = {-15.f, 1.f, 5.f, 4.f, 0.f, 100.f, 50.f, 24.f, 0.f, 0.f, 0.f};
const std::vector<float> kTransient = {0.f, 0.f, 50.f, 0.f, 100.f, 30.f, 2000.f, 0.f};
const std::vector<float> kMastering = {0.f, -1.f, 150.f, 1.f, 0.f, 2.f, 10.f, 75.f, 100.f, 0.f, 2.f, 1.f, 0.f, 2.f, 0.f, 0.f, 0.f};

std::vector<float> with(std::vector<float> v, std::initializer_list<std::pair<std::size_t, float>> set) {
    for (auto [i, x] : set) v[i] = x;
    return v;
}
// dstudio_core::builtin::character_compressor / multiband_compressor defaults.
const std::vector<float> kCharacter = {0.f,  -18.f, 4.f, 6.f,  10.f, 200.f, 0.f, 0.f,   24.f, 1.f,    100.f, 0.f, 0.f, 0.f,    0.f, 100.f,
                                       0.f,  0.f,   0.f, 80.f, 1000.f, 0.f, 1.f, 0.f,   0.f,  0.f,    50.f,  8.f, 0.f, 0.f,    150.f, 1.f,
                                       0.f,  0.f,   600.f, 1.f, 0.f,  0.f,  2500.f, 1.f, 0.f,  0.f,    8000.f, 1.f, 0.f, 0.f};
const std::vector<float> kMultiband = {120.f, 2500.f, -18.f, 2.5f, 30.f, 200.f, 0.f, -20.f, 2.f, 15.f, 120.f, 0.f, -22.f, 2.f, 5.f, 80.f, 0.f,
                                       100.f, 0.f,    0.f,   -60.f, 1.f, 0.f,   -60.f, 1.f, 0.f, -60.f, 1.f, 0.f, 100.f, 100.f, 100.f, 100.f};

// LFO: Sine, Sync off, 2 Hz, 1/4, depth 100 %, phase 0.
const std::vector<float> kLfo = {0.f, 0.f, 2.f, 8.f, 100.f, 0.f};

// v2 state: values, then routes moving parameter 0 (Threshold, -60..0 dB) of each target by +-25 %.
std::vector<uint8_t> blob_v2(const std::vector<float>& v, const std::vector<uint64_t>& targets) {
    se_builtin_state_v2 h{SE_BUILTIN_STATE_MAGIC2, static_cast<uint32_t>(v.size()), 0, 0, 0, static_cast<uint32_t>(targets.size()), 0};
    std::vector<uint8_t> out(sizeof h + 4 * v.size() + sizeof(se_mod_route) * targets.size());
    std::memcpy(out.data(), &h, sizeof h);
    if (!v.empty()) std::memcpy(out.data() + sizeof h, v.data(), 4 * v.size()); // memcpy from null is UB even for 0 bytes
    for (std::size_t i = 0; i < targets.size(); ++i) {
        se_mod_route r{targets[i], 0, SE_MOD_BUILTIN, 0, 0, 0, -0.25f, 0.25f, -60.f, 0.f, 1.f, 0.f};
        std::memcpy(out.data() + sizeof h + 4 * v.size() + i * sizeof r, &r, sizeof r);
    }
    return out;
}

// Defaults of the EQ and filter devices (registry order) and busy settings.
std::vector<float> channel_eq(bool busy) {
    std::vector<float> v = {20.f,   0.f, 0.71f, 0.f, 75.f,   0.f, 1.f,  1.f, 100.f,  0.f, 0.6f,  1.f, 250.f, 0.f, 0.3f,
                            1.f,    1040.f, 0.f, 0.41f, 1.f, 2500.f, 0.f, 0.2f, 1.f, 7500.f, 0.f, 1.f, 1.f, 20000.f, 0.f,
                            0.71f,  0.f, 0.f, 4.f, 1.f, 0.f, 0.f, 0.f, 0.f, 2.f, 5.f, 0.f, 0.f, 0.f};
    if (busy) { // every band on and moving the sound, 48 dB/oct pass filters
        for (int b = 0; b < 8; ++b) {
            v[static_cast<std::size_t>(4 * b + 1)] = (b % 2 ? -4.f : 5.f);
            v[static_cast<std::size_t>(4 * b + 3)] = 1.f;
        }
        v[0] = 40.f;
        v[28] = 16000.f;
        v[41] = v[42] = 3.f;
    }
    return v;
}

enum class Ceq { Defaults, Bands, Dynamic, Valve4x, Everything };
std::vector<float> character_eq(Ceq mode) {
    std::vector<float> v;
    for (int b = 0; b < 12; ++b) v.insert(v.end(), {0.f, 0.f, 1000.f, 0.f, 0.7071f, 1.f, 0.f, 0.f, 0.f});
    v.insert(v.end(), {0.f, 0.f, 1.f, 300.f, 0.f, 1.f, 100.f, 0.f, 0.f, 0.f, 0.f});
    for (int b = 0; b < 12; ++b) v.insert(v.end(), {0.f, 0.f, 1.f, -30.f, 15.f, 150.f});
    v.insert(v.end(), {0.f, 50.f});
    for (float f : {250.f, 1000.f, 4000.f, 10000.f}) v.insert(v.end(), {0.f, f, 1.f, 0.f});
    for (int b = 0; b < 12; ++b) v.push_back(0.f);
    v.push_back(0.f);
    const auto at = [&](int i) -> float& { return v[static_cast<std::size_t>(i)]; };
    if (mode != Ceq::Defaults && mode != Ceq::Valve4x) {
        const float types[12] = {3.f, 1.f, 6.f, 0.f, 0.f, 0.f, 0.f, 5.f, 0.f, 0.f, 2.f, 4.f};
        for (int b = 0; b < 12; ++b) {
            at(9 * b) = 1.f;
            at(9 * b + 1) = types[b];
            at(9 * b + 2) = 40.f * std::pow(1.8f, static_cast<float>(b));
            at(9 * b + 3) = (b % 2 ? -3.f : 4.f);
            at(9 * b + 5) = 3.f;
            at(9 * b + 8) = 4.f;
        }
    }
    if (mode == Ceq::Dynamic || mode == Ceq::Everything) {
        for (int b = 0; b < 12; b += 3) {
            at(119 + 6 * b) = 1.f;
            at(120 + 6 * b) = -6.f;
            at(209 + b) = 40.f;
        }
    }
    if (mode == Ceq::Valve4x || mode == Ceq::Everything) {
        at(112) = 1.f;
        at(113) = 1.5f;
        at(115) = 1.f;
    }
    if (mode == Ceq::Everything) {
        at(109) = 40.f; // warmth
        at(118) = 1.f;  // auto gain
        at(191) = 1.f;  // tamer + all four bands
        for (int t = 0; t < 4; ++t) at(193 + 4 * t) = 1.f;
    }
    return v;
}

std::vector<float> auto_filter(int mode) {
    std::vector<float> v = {0.f, 0.f, 20000.f, 0.f, 0.f, 0.f, 1.f, 100.f, 0.f, 1.f, 4.f, 0.f, 0.f, 0.f, 100.f, 0.f, 0.f};
    if (mode >= 1) { // 24 dB/oct, resonant, envelope and LFO moving the cutoff
        v[1] = 1.f;
        v[2] = 800.f;
        v[3] = 0.6f;
        v[5] = 24.f;
        v[8] = 24.f;
        v[9] = 3.f;
    }
    if (mode >= 2) { // plus drive and a stereo LFO (two cutoffs)
        v[4] = 12.f;
        v[13] = 90.f;
    }
    return v;
}

std::vector<uint8_t> blob(const std::vector<float>& v, uint64_t sc = 0) {
    se_builtin_state h{SE_BUILTIN_STATE_MAGIC, static_cast<uint32_t>(v.size()), sc};
    std::vector<uint8_t> out(sizeof h + 4 * v.size());
    std::memcpy(out.data(), &h, sizeof h);
    if (!v.empty()) std::memcpy(out.data() + sizeof h, v.data(), 4 * v.size()); // memcpy from null is UB even for 0 bytes
    return out;
}

bool write_float_wav(const std::string& path, const std::vector<float>& l, const std::vector<float>& r) {
    std::FILE* f = dstudio::os::fopen_utf8(path.c_str(), "wb");
    if (!f) return false;
    const auto frames = static_cast<uint32_t>(l.size());
    const uint32_t data = frames * 8;
    auto u32 = [&](uint32_t v) { std::fwrite(&v, 4, 1, f); };
    auto u16 = [&](uint16_t v) { std::fwrite(&v, 2, 1, f); };
    std::fwrite("RIFF", 1, 4, f);
    u32(36 + data);
    std::fwrite("WAVEfmt ", 1, 8, f);
    u32(16);
    u16(3);
    u16(2);
    u32(48000);
    u32(48000 * 8);
    u16(8);
    u16(32);
    std::fwrite("data", 1, 4, f);
    u32(data);
    for (uint32_t i = 0; i < frames; ++i) {
        std::fwrite(&l[i], 4, 1, f);
        std::fwrite(&r[i], 4, 1, f);
    }
    return std::fclose(f) == 0;
}

struct Stats {
    double avg = 0, p99 = 0, max = 0;
};

Stats summarize(std::vector<double>& t) {
    std::sort(t.begin(), t.end());
    Stats s;
    for (double x : t) s.avg += x;
    s.avg /= static_cast<double>(t.size());
    s.p99 = t[static_cast<std::size_t>(0.99 * static_cast<double>(t.size() - 1))];
    s.max = t.back();
    return s;
}

double us(Clock::duration d) { return std::chrono::duration<double, std::micro>(d).count(); }

// events_per_block automation events on parameter event_param, values base + i * step.
// tone: a harmonic line (A3 with 5 Hz vibrato, 8 partials) instead of noise,
// so pitch-tracking devices run their voiced path.
Stats bench_kernel(const char* id, std::vector<float> values, uint32_t blocks, uint32_t events_per_block,
                   const std::vector<dstudio::MidiEvent>& notes = {}, uint32_t event_param = 0, double base = -30.0,
                   double step = 1.0, bool tone = false) {
    dstudio::PluginSpec spec;
    spec.device_id = 1;
    spec.format = dstudio::PluginFormat::Builtin;
    spec.plugin_id = id;
    spec.state = blob(values);
    const dstudio::BuiltinDescriptor* d = dstudio::find_builtin(id);
    spec.instrument = d != nullptr && d->instrument;
    std::string err;
    auto inst = dstudio::create_builtin_instance(spec, kRate, kBlock, err);
    if (!inst) {
        std::fprintf(stderr, "%s: %s\n", id, err.c_str());
        std::exit(1);
    }
    std::vector<float> l(kBlock), r(kBlock);
    uint32_t seed = 7;
    double phase = 0.0;
    uint64_t frame = 0;
    std::vector<dstudio::ParamEvent> ev;
    for (uint32_t i = 0; i < events_per_block; ++i)
        ev.push_back(dstudio::ParamEvent{i * (kBlock / std::max(1u, events_per_block)), event_param, base + step * i});
    dstudio::RtTransport tr;
    std::vector<double> t;
    t.reserve(blocks);
    for (uint32_t b = 0; b < blocks + 200; ++b) {
        for (uint32_t i = 0; i < kBlock; ++i, ++frame) {
            if (tone) {
                const double sec = static_cast<double>(frame) / kRate;
                phase += 6.283185307179586 * 220.0 * std::pow(2.0, 0.25 * std::sin(6.283185307179586 * 5.0 * sec) / 12.0) / kRate;
                double s = 0.0;
                for (int h = 1; h <= 8; ++h) s += std::sin(phase * h) / h;
                l[i] = r[i] = static_cast<float>(0.2 * s);
                continue;
            }
            seed = seed * 1664525u + 1013904223u;
            const float n = static_cast<float>(seed >> 8) / 8388608.0f - 1.0f;
            l[i] = 0.8f * n;
            r[i] = 0.7f * n;
        }
        const auto t0 = Clock::now();
        const bool first = b == 0 && !notes.empty();
        inst->run(l.data(), r.data(), kBlock, first ? notes.data() : nullptr, first ? static_cast<uint32_t>(notes.size()) : 0, tr,
                  ev.data(), static_cast<uint32_t>(ev.size()));
        const auto t1 = Clock::now();
        if (b >= 200) t.push_back(us(t1 - t0));
    }
    return summarize(t);
}

// Modulation source cost: run_modulation per block (the grid of source outputs).
Stats bench_source(const char* id, std::vector<float> values, uint32_t blocks) {
    dstudio::PluginSpec spec;
    spec.device_id = 1;
    spec.format = dstudio::PluginFormat::Builtin;
    spec.plugin_id = id;
    spec.state = blob(values);
    std::string err;
    auto inst = dstudio::create_builtin_instance(spec, kRate, kBlock, err);
    if (!inst) {
        std::fprintf(stderr, "%s: %s\n", id, err.c_str());
        std::exit(1);
    }
    auto* b = static_cast<dstudio::BuiltinInstance*>(inst.get());
    dstudio::RtTransport tr;
    tr.sample_rate = kRate;
    tr.playing = true;
    tr.tempo_bpm = 120.0;
    std::vector<double> t;
    t.reserve(blocks);
    for (uint32_t k = 0; k < blocks + 200; ++k) {
        tr.position = static_cast<int64_t>(k) * kBlock;
        const auto t0 = Clock::now();
        b->run_modulation(kBlock, tr, nullptr, 0);
        const auto t1 = Clock::now();
        if (k >= 200) t.push_back(us(t1 - t0));
    }
    return summarize(t);
}

// Instruments with one stereo sample (4 s) in every slot. Every block gets
// note-ons from notes (cycled) so about voices voices keep sounding.
Stats bench_instrument(const char* id, std::vector<float> values, uint32_t blocks, const std::vector<uint8_t>& notes,
                       uint32_t hits_every) {
    const dstudio::BuiltinDescriptor* d = dstudio::find_builtin(id);
    dstudio::PluginSpec spec;
    spec.device_id = 1;
    spec.format = dstudio::PluginFormat::Builtin;
    spec.plugin_id = id;
    spec.instrument = true;
    spec.state = blob(values);
    std::string err;
    auto inst = dstudio::create_builtin_instance(spec, kRate, kBlock, err);
    if (!inst || d == nullptr) {
        std::fprintf(stderr, "%s: %s\n", id, err.c_str());
        std::exit(1);
    }
    auto asset = std::make_shared<dstudio::Asset>();
    asset->id = 1;
    asset->channels = 2;
    asset->frames = 4 * 48000;
    asset->source_sample_rate = kRate;
    for (auto& ch : asset->planar) ch.resize(asset->frames);
    uint32_t seed = 11;
    for (uint64_t i = 0; i < asset->frames; ++i) {
        seed = seed * 1664525u + 1013904223u;
        const float n = static_cast<float>(seed >> 8) / 8388608.0f - 1.0f;
        asset->planar[0][i] = 0.3f * std::sin(0.03f * static_cast<float>(i)) + 0.1f * n;
        asset->planar[1][i] = 0.3f * std::sin(0.031f * static_cast<float>(i)) + 0.1f * n;
    }
    // v2 state: every slot holds the asset.
    se_builtin_state_v2 h{SE_BUILTIN_STATE_MAGIC2, static_cast<uint32_t>(values.size()), 0, d->sample_slots, 0, 0, 0};
    std::vector<uint8_t> st(sizeof h + 4 * values.size() + sizeof(se_builtin_sample) * d->sample_slots);
    std::memcpy(st.data(), &h, sizeof h);
    std::memcpy(st.data() + sizeof h, values.data(), 4 * values.size());
    for (uint32_t s = 0; s < d->sample_slots; ++s) {
        se_builtin_sample e{1, s, 0};
        std::memcpy(st.data() + sizeof h + 4 * values.size() + sizeof e * s, &e, sizeof e);
    }
    dstudio::BuiltinInstance* b = inst->as_builtin();
    b->update_state(st.data(), st.size());
    b->bind_samples([&](uint64_t) { return dstudio::AssetRef(asset); });
    std::vector<float> l(kBlock), r(kBlock);
    dstudio::RtTransport tr;
    std::vector<double> t;
    t.reserve(blocks);
    std::size_t next = 0;
    for (uint32_t blk = 0; blk < blocks + 200; ++blk) {
        dstudio::MidiEvent ev[2];
        uint32_t ne = 0;
        if (blk % hits_every == 0) {
            const uint8_t note = notes[next++ % notes.size()];
            ev[ne++] = dstudio::MidiEvent{0, 0x80, note, 64, 0};
            ev[ne++] = dstudio::MidiEvent{17, 0x90, note, 110, 0};
        }
        std::memset(l.data(), 0, 4 * kBlock);
        std::memset(r.data(), 0, 4 * kBlock);
        const auto t0 = Clock::now();
        inst->run(l.data(), r.data(), kBlock, ev, ne, tr);
        const auto t1 = Clock::now();
        if (blk >= 200) t.push_back(us(t1 - t0));
    }
    return summarize(t);
}

// Sampler values in registry order (sampler-steam definition defaults).
std::vector<float> sampler_values() {
    const auto d = dstudio::find_builtin("sampler-steam")->defaults;
    return {d.begin(), d.end()};
}

} // namespace

int main(int argc, char** argv) {
    const uint32_t blocks = argc > 1 ? static_cast<uint32_t>(std::atoi(argv[1])) : 20000;
    const double deadline = 1e6 * kBlock / kRate;
    std::printf("dstudio_device_bench: %u blocks x %u frames @ %.0f Hz (deadline %.1f us/block), stereo noise in\n\n", blocks,
                kBlock, kRate, deadline);
    std::printf("%-34s %12s %12s %12s %14s\n", "device", "avg us", "p99 us", "max us", "x real-time");
    const auto row = [&](const char* name, Stats s) {
        std::printf("%-34s %12.3f %12.3f %12.3f %13.0fx\n", name, s.avg, s.p99, s.max, deadline / s.avg);
    };
    const char* styles[] = {"Clean", "Opto", "Punch", "Smooth"};
    for (int st = 0; st < 4; ++st) {
        std::vector<float> v = kCompressor;
        v.at(7) = static_cast<float>(st);
        char name[64];
        std::snprintf(name, sizeof name, "compressor (%s)", styles[st]);
        row(name, bench_kernel("compressor", v, blocks, 0));
    }
    row("compressor (Clean, 16 auto events)", bench_kernel("compressor", kCompressor, blocks, 16));
    row("channeleq (defaults)", bench_kernel("channeleq", channel_eq(false), blocks, 0));
    row("channeleq (8 bands, 48 dB/oct)", bench_kernel("channeleq", channel_eq(true), blocks, 0));
    row("channeleq (8 bands, 16 freq events)", bench_kernel("channeleq", channel_eq(true), blocks, 16, {}, 12, 200.0, 40.0));
    row("character-eq (defaults)", bench_kernel("character-eq", character_eq(Ceq::Defaults), blocks, 0));
    row("character-eq (12 bands)", bench_kernel("character-eq", character_eq(Ceq::Bands), blocks, 0));
    row("character-eq (12 bands, 4 dyn+drive)", bench_kernel("character-eq", character_eq(Ceq::Dynamic), blocks, 0));
    row("character-eq (valve 4x only)", bench_kernel("character-eq", character_eq(Ceq::Valve4x), blocks, 0));
    row("character-eq (everything on)", bench_kernel("character-eq", character_eq(Ceq::Everything), blocks, 0));
    row("character-eq (12 bands, 16 freq events)", bench_kernel("character-eq", character_eq(Ceq::Bands), blocks, 16, {}, 29, 200.0, 40.0));
    row("auto-filter (defaults)", bench_kernel("auto-filter", auto_filter(0), blocks, 0));
    row("auto-filter (24 dB, env + LFO)", bench_kernel("auto-filter", auto_filter(1), blocks, 0));
    row("auto-filter (+ drive, stereo LFO)", bench_kernel("auto-filter", auto_filter(2), blocks, 0));
    row("limiter (true peak)", bench_kernel("limiter", with(kLimiter, {{5, 12.f}}), blocks, 0));
    row("limiter (sample peak)", bench_kernel("limiter", with(kLimiter, {{5, 12.f}, {3, 0.f}}), blocks, 0));
    row("gate", bench_kernel("gate", with(kGate, {{0, -10.f}}), blocks, 0));
    row("glue-compressor (color 50%)", bench_kernel("glue-compressor", kGlue, blocks, 0));
    row("glue-compressor (color 0, auto rel.)", bench_kernel("glue-compressor", with(kGlue, {{6, 0.f}}), blocks, 0));
    row("transient-shaper (shaping + clip)", bench_kernel("transient-shaper", with(kTransient, {{0, 50.f}, {1, -50.f}, {7, 1.f}}), blocks, 0));
    for (int os = 0; os < 4; ++os) {
        char name[64];
        std::snprintf(name, sizeof name, "mastering-limiter (%dx, -8 dB)", 1 << os);
        row(name, bench_kernel("mastering-limiter", with(kMastering, {{0, -8.f}, {10, static_cast<float>(os)}}), blocks, 0));
    }
    row("mastering-limiter (4x, M/S, dither)", bench_kernel("mastering-limiter", with(kMastering, {{0, -8.f}, {9, 1.f}, {12, 3.f}}), blocks, 0));
    row("lfo (modulation grid, Sine)", bench_source("lfo", kLfo, blocks));
    {
        std::vector<float> v = kLfo;
        v[0] = 6.f;
        v[1] = 1.f;
        row("lfo (modulation grid, Smooth Random)", bench_source("lfo", v, blocks));
    }
    row("device-rack (8 macros)", bench_source("device-rack", std::vector<float>(8, 50.f), blocks));
    // Restoration: defaults, plus the heaviest setting of each.
    row("declipper (defaults, 4x)", bench_kernel("declipper", {-1.f, -0.3f, 1.f, 100.f}, blocks, 0));
    row("declipper (-3 dB, clipping, 8x)", bench_kernel("declipper", {-3.f, -0.3f, 2.f, 100.f}, blocks, 0));
    row("deesser (defaults)", bench_kernel("deesser", {7000.f, 1.f, -36.f, 8.f, 0.f, 100.f}, blocks, 0));
    row("dehummer (defaults, 4 notches)", bench_kernel("dehummer", {60.f, 4.f, 5.f, -30.f, 100.f}, blocks, 0));
    row("dehummer (8 notches)", bench_kernel("dehummer", {60.f, 8.f, 5.f, -30.f, 100.f}, blocks, 0));
    row("denoiser (defaults)", bench_kernel("denoiser", {-40.f, -20.f, 50.f, 5.f, 50.f, 100.f}, blocks, 0));

    const char* models[] = {"Clean", "Vari-Mu", "Bus VCA", "FET", "Opto", "RMS VCA"};
    for (int m = 0; m < 6; ++m) {
        std::vector<float> v = kCharacter;
        v.at(0) = static_cast<float>(m);
        char name[64];
        std::snprintf(name, sizeof name, "character-compressor (%s)", models[m]);
        row(name, bench_kernel("character-compressor", v, blocks, 0));
    }
    {
        std::vector<float> v = kCharacter;
        v[0] = 3.f;  // FET
        v[14] = 1.f; // 4x
        row("character-compressor (FET, 4x)", bench_kernel("character-compressor", v, blocks, 0));
        v = kCharacter;
        v[13] = 5.f;                                    // lookahead
        v[18] = 1.f, v[21] = 3.f;                       // SC HPF + EQ
        v[25] = 1.f, v[29] = 1.f, v[33] = 1.f, v[32] = -12.f, v[36] = -12.f; // level rider, two weights
        row("character-compressor (Clean, all on)", bench_kernel("character-compressor", v, blocks, 0));
    }
    row("multiband-compressor (defaults)", bench_kernel("multiband-compressor", kMultiband, blocks, 0));
    {
        std::vector<float> v = kMultiband;
        for (int b = 0; b < 3; ++b) v[static_cast<std::size_t>(20 + 3 * b)] = -40.f, v[static_cast<std::size_t>(21 + 3 * b)] = 4.f; // upward on
        row("multiband-compressor (up + down, 16 auto events)", bench_kernel("multiband-compressor", v, blocks, 16, {}, 2)); // low threshold
    }
    const char* drive_modes[] = {"Overdrive", "Fuzz", "Tube", "Hard Clip"};
    for (int m = 0; m < 4; ++m) {
        char name[64];
        std::snprintf(name, sizeof name, "distortion (%s, 4x)", drive_modes[m]);
        row(name, bench_kernel("distortion", {70.f, 6000.f, 100.f, -6.f, static_cast<float>(m), 30.f, 0.f}, blocks, 0));
    }
    const char* tape_modes[] = {"Reel", "Cassette", "VHS"};
    for (int m = 0; m < 3; ++m) {
        char name[64];
        std::snprintf(name, sizeof name, "tapesaturation (%s, 4x)", tape_modes[m]);
        row(name, bench_kernel("tapesaturation", {70.f, 50.f, 40.f, -40.f, 100.f, static_cast<float>(m), 0.f}, blocks, 0));
    }
    const char* shapes[] = {"Smooth", "Warm", "Hard"};
    for (int m = 0; m < 3; ++m) {
        char name[64];
        std::snprintf(name, sizeof name, "soft-clipper (%s, 4x)", shapes[m]);
        row(name, bench_kernel("soft-clipper", {12.f, -3.f, 50.f, static_cast<float>(m), 0.f, 100.f, 0.f}, blocks, 0));
    }
    row("bitcrusher (8 bit, 4x down, jitter)", bench_kernel("bitcrusher", {8.f, 4.f, 100.f, 50.f, 30.f}, blocks, 0));
    row("widener (200%)", bench_kernel("widener", {200.f, 0.f, 0.f, 120.f, 100.f}, blocks, 0));
    row("utility (width, bass mono, DC)", bench_kernel("utility", {-3.f, 10.f, 150.f, 0.f, 0.f, 1.f, 0.f, 1.f, 120.f, 0.f, 0.f, 1.f}, blocks, 0));
    // Modulation devices at their defaults, then the heavier settings.
    const auto defaults = [](const char* id) {
        const dstudio::BuiltinDescriptor* d = dstudio::find_builtin(id);
        return std::vector<float>(d->defaults.begin(), d->defaults.end());
    };
    const auto tweak = [&](const char* id, std::initializer_list<std::pair<const char*, float>> set) {
        const dstudio::BuiltinDescriptor* d = dstudio::find_builtin(id);
        std::vector<float> v = defaults(id);
        for (const auto& [p, x] : set)
            for (std::size_t i = 0; i < d->params.size(); ++i)
                if (std::strcmp(d->params[i], p) == 0) v[i] = x;
        return v;
    };
    for (const char* id : {"tremolo", "autopanner", "chorus", "flanger", "phaser", "ring-mod"}) row(id, bench_kernel(id, defaults(id), blocks, 0));
    row("chorus (4 voices, feedback 60%)", bench_kernel("chorus", tweak("chorus", {{"voices", 3.0f}, {"feedback", 60.0f}}), blocks, 0));
    row("flanger (Thru-Zero)", bench_kernel("flanger", tweak("flanger", {{"mode", 2.0f}}), blocks, 0));
    row("phaser (12 stages)", bench_kernel("phaser", tweak("phaser", {{"stages", 3.0f}}), blocks, 0));
    row("ring-mod (Noise, spread 100%)", bench_kernel("ring-mod", tweak("ring-mod", {{"modulator", 4.0f}, {"spread", 100.0f}}), blocks, 0));
    const char* circuits[] = {"Single BBD", "Crossed", "Twin BBD", "Ensemble", "Tape", "Photocell", "Flanger", "Phaser", "Tremolo", "Rotary", "Panner"};
    for (int c = 0; c < 11; ++c) {
        char name[64];
        std::snprintf(name, sizeof name, "character-mod (%s)", circuits[c]);
        row(name, bench_kernel("character-mod", tweak("character-mod", {{"model", static_cast<float>(c)}}), blocks, 0));
    }
    row("character-mod (Phaser Barber)", bench_kernel("character-mod", tweak("character-mod", {{"model", 7.0f}, {"mode_phaser", 4.0f}}), blocks, 0));
    row("trance-gate (defaults)", bench_kernel("trance-gate", devtest::builtin_defaults("trance-gate"), blocks, 0));
    row("trance-gate (16 auto events)", bench_kernel("trance-gate", devtest::builtin_defaults("trance-gate"), blocks, 16));
    {
        using devtest::shaper_lane;
        const std::vector<float> base = devtest::builtin_defaults("shaper");
        if (base.size() <= static_cast<size_t>(shaper_lane(6, 3))) {
            std::fprintf(stderr, "shaper has %zu parameters, fewer than the lanes need\n", base.size());
            return 1;
        }
        row("shaper (defaults: Volume lane)", bench_kernel("shaper", base, blocks, 0));
        const char* lanes[] = {"Volume", "Time", "Width", "Pan", "Filter", "Crush", "Pitch"};
        for (int lane = 1; lane < 7; ++lane) {
            std::vector<float> v = base;
            v[shaper_lane(0, 0)] = 0.f;
            v[shaper_lane(lane, 0)] = 1.f;
            char name[64];
            std::snprintf(name, sizeof name, "shaper (%s lane only)", lanes[lane]);
            row(name, bench_kernel("shaper", v, blocks, 0));
        }
        std::vector<float> all = base;
        for (int lane = 0; lane < 7; ++lane) all[shaper_lane(lane, 0)] = 1.f;
        row("shaper (all 7 lanes)", bench_kernel("shaper", all, blocks, 0));
        for (int lane = 0; lane < 7; ++lane) all[shaper_lane(lane, 3)] = 1.f; // Band: Lows
        row("shaper (all 7 lanes, band split)", bench_kernel("shaper", all, blocks, 0));
    }
    // Synth: notes held from the first block (sustain keeps every voice busy).
    const std::vector<float> synth(std::begin(dstudio::synth_v2::kDefaults), std::end(dstudio::synth_v2::kDefaults));
    const auto chord = [](uint32_t n) {
        std::vector<dstudio::MidiEvent> v;
        for (uint32_t i = 0; i < n; ++i) v.push_back(dstudio::MidiEvent{0, 0x90, static_cast<uint8_t>(36 + 5 * i), 100, 0});
        return v;
    };
    row("synth-v2 (idle, sleeping)", bench_kernel("synth-v2", synth, blocks, 0));
    row("synth-v2 (default, 1 note)", bench_kernel("synth-v2", synth, blocks, 0, chord(1)));
    row("synth-v2 (default, 8 notes)", bench_kernel("synth-v2", synth, blocks, 0, chord(8)));
    {
        std::vector<float> heavy = synth;
        heavy[17] = 7.f;                                           // unison 8
        for (uint32_t s = 0; s < 4; ++s) heavy[184 + 79 * s] = 1.f; // every effect slot on
        row("synth-v2 (16 notes, unison 8, 4 FX)", bench_kernel("synth-v2", heavy, blocks, 0, chord(16)));
    }
    // Analyzers and pitch: parameters in registry order.
    const std::vector<float> pitch = {100.f, 0.f, 1.f, 20.f, 15.f, 100.f, 0.f};
    row("pitch-corrector (C major, tone)", bench_kernel("pitch-corrector", pitch, blocks, 0, {}, 0, -30.0, 1.0, true));
    row("pitch-corrector (noise, unvoiced)", bench_kernel("pitch-corrector", pitch, blocks, 0));
    row("tuner (tone)", bench_kernel("tuner", {0.f, 0.f, 0.f}, blocks, 0, {}, 0, -30.0, 1.0, true));
    std::vector<float> spectrum = {3.f, 0.f, 0.f, 30.f, 4.5f, 0.f, 90.f, 1.f, 30.f, 50.f, 1.f, 0.f, 3.f};
    row("spectrum (FFT 4096, stereo)", bench_kernel("spectrum", spectrum, blocks, 0));
    spectrum[2] = 5.f;
    row("spectrum (FFT 4096, Mid+Side)", bench_kernel("spectrum", spectrum, blocks, 0));
    spectrum[0] = 5.f;
    spectrum[2] = 0.f;
    row("spectrum (FFT 16384, stereo)", bench_kernel("spectrum", spectrum, blocks, 0));
    // Instruments: voices kept sounding by a new note every few blocks.
    const std::vector<uint8_t> keys = {48, 55, 60, 64, 67, 71, 72, 76};
    {
        std::vector<float> v = sampler_values();
        v[2] = 1; // poly
        row("sampler-steam (One Shot, 8 voices)", bench_instrument("sampler-steam", v, blocks, keys, 4));
        v[0] = 0;   // Classic
        v[41] = 1;  // loop
        v[18] = 1;  // filter on, LP 24
        v[20] = 1200;
        v[21] = 0.5f;
        v[22] = 0.4f; // drive
        v[24] = 0.5f; // env depth
        v[31] = 0.3f; // LFO amount
        v[34] = 1;    // LFO -> cutoff
        row("sampler-steam (Classic+filter, 8 v)", bench_instrument("sampler-steam", v, blocks, keys, 4));
        v = sampler_values();
        v[2] = 1;
        v[4] = 3; // warp quarter note
        row("sampler-steam (warp, 8 voices)", bench_instrument("sampler-steam", v, blocks, keys, 4));
        v = sampler_values();
        row("sampler-steam (mono, 1 voice)", bench_instrument("sampler-steam", v, blocks, keys, 8));
    }
    {
        std::vector<float> pad = {0, 0, 0, 0, 0, 0, 1, 0, 0, 0};
        row("pad-sampler (4 voices)", bench_instrument("pad-sampler", pad, blocks, {36}, 8));
        std::vector<float> rack;
        for (int p = 0; p < 16; ++p) {
            rack.push_back(1);
            rack.insert(rack.end(), pad.begin(), pad.end());
        }
        std::vector<uint8_t> pads;
        for (uint8_t n = 36; n < 52; ++n) pads.push_back(n);
        row("drum-rack (16 pads, 1 hit/block)", bench_instrument("drum-rack", rack, blocks, pads, 1));
        rack[4 * 11 + 10] = 3; // pad 5 warped
        row("drum-rack (busy, 1 warped pad)", bench_instrument("drum-rack", rack, blocks, pads, 1));
    }

    // Time and pitch devices, one row per mode / machine.
    const std::pair<const char*, std::vector<float>> time_fx[] = {
        {"delay (Tape)", tweak("delay", {{"mode", 0.f}})},
        {"delay (Digital)", tweak("delay", {{"mode", 1.f}})},
        {"delay (BBD)", tweak("delay", {{"mode", 2.f}})},
        {"echo (Tape Loop)", tweak("echo", {{"machine", 0.f}})},
        {"echo (Drum)", tweak("echo", {{"machine", 1.f}})},
        {"echo (Cartridge)", tweak("echo", {{"machine", 2.f}})},
        {"reverb (Room)", tweak("reverb", {{"mode2", 0.f}})},
        {"reverb (Hall)", tweak("reverb", {{"mode2", 1.f}})},
        {"reverb (Plate)", tweak("reverb", {{"mode2", 2.f}})},
        {"reverb (Spring)", tweak("reverb", {{"mode2", 3.f}})},
        {"reverb (Shimmer)", tweak("reverb", {{"mode2", 4.f}})},
        {"reverb (Haze)", tweak("reverb", {{"mode2", 5.f}})},
        {"pitch-shifter (+7 st)", tweak("pitch-shifter", {{"semitones", 7.f}})},
        {"pitch-shifter (+7 st, formant)", tweak("pitch-shifter", {{"semitones", 7.f}, {"formant_preserve", 1.f}})},
        {"grains (Cloud)", tweak("grains", {{"mode", 0.f}})},
        {"grains (Rhythm)", tweak("grains", {{"mode", 1.f}})},
    };
    for (const auto& [name, values] : time_fx) {
        const std::string id(name, std::strchr(name, ' '));
        row(name, bench_kernel(id.c_str(), values, blocks, 0));
    }

    // Parameter update paths and session cost: 32 tracks, one clip and two
    // compressors each.
    const std::string wav = (std::filesystem::temp_directory_path() / "dstudio_device_bench.wav").string();
    {
        std::vector<float> l(8 * 48000), r(8 * 48000);
        uint32_t seed = 3;
        for (std::size_t i = 0; i < l.size(); ++i) {
            seed = seed * 1664525u + 1013904223u;
            l[i] = 0.5f * (static_cast<float>(seed >> 8) / 8388608.0f - 1.0f);
            r[i] = 0.9f * l[i];
        }
        if (!write_float_wav(wav, l, r)) {
            std::fprintf(stderr, "cannot write %s\n", wav.c_str());
            return 1;
        }
    }
    se_config cfg{SE_ABI_VERSION, kRate, kBlock, 2};
    se_engine* e = se_engine_create(&cfg);
    if (!e) {
        std::fprintf(stderr, "create failed: %s\n", se_last_error());
        return 1;
    }
    se_asset_info info{};
    if (se_asset_load_file(e, wav.c_str(), &info) != SE_OK) {
        std::fprintf(stderr, "load failed: %s\n", se_last_error());
        return 1;
    }
    const auto state = blob(kCompressor);
    std::vector<se_clip_desc> clips(32);
    // Per track: [LFO, compressor, compressor]; the LFO moves both thresholds.
    std::vector<se_device_desc> devs(96);
    std::vector<std::vector<uint8_t>> lfo_states(32);
    std::vector<se_track_desc_v5> tracks(32);
    for (uint32_t t = 0; t < 32; ++t) {
        clips[t] = se_clip_desc{};
        clips[t].clip_id = t + 1;
        clips[t].asset_id = info.asset_id;
        clips[t].source_offset = (t * 131) % 1000;
        clips[t].length = 7 * 48000;
        clips[t].gain = 1.0f;
        for (uint32_t k = 0; k < 2; ++k) {
            se_device_desc& d = devs[3 * t + 1 + k];
            d = se_device_desc{};
            d.device_id = 1000 + 2 * t + k;
            d.plugin_id = "compressor";
            d.format = SE_PLUGIN_BUILTIN;
            d.enabled = 1;
            d.state = state.data();
            d.state_len = state.size();
        }
        lfo_states[t] = blob_v2(kLfo, {1000 + 2 * t, 1000 + 2 * t + 1});
        se_device_desc& l = devs[3 * t];
        l = se_device_desc{};
        l.device_id = 5000 + t;
        l.plugin_id = "lfo";
        l.format = SE_PLUGIN_BUILTIN;
        l.enabled = 1;
        l.state = lfo_states[t].data();
        l.state_len = lfo_states[t].size();
        se_track_desc_v5& td = tracks[t];
        td = se_track_desc_v5{};
        td.track_id = t + 1;
        td.gain = 0.25f;
        td.clips = &clips[t];
        td.clip_count = 1;
        td.devices = &devs[3 * t + 1];
        td.device_count = 2;
    }
    se_session_desc_v5 s{tracks.data(), 32, 0.5f};
    if (se_session_publish_v5(e, &s) != SE_OK) {
        std::fprintf(stderr, "publish failed: %s\n", se_last_error());
        return 1;
    }
    std::printf("\nParameter updates (main thread, 64 live compressors)\n");
    {
        const int n = 200000;
        const auto t0 = Clock::now();
        for (int i = 0; i < n; ++i)
            se_builtin_set_param(e, 1000 + static_cast<uint64_t>(i & 63), 0, -20.0f - static_cast<float>(i & 7));
        std::printf("  %-32s %10.3f us/call\n", "se_builtin_set_param", us(Clock::now() - t0) / n);
    }
    {
        const int n = 2000;
        const auto t0 = Clock::now();
        for (int i = 0; i < n; ++i) se_session_publish_v5(e, &s);
        std::printf("  %-32s %10.3f us/publish\n", "republish (unchanged chain)", us(Clock::now() - t0) / n);
    }

    std::printf("\nSession: 32 audio tracks x 1 clip, offline render\n");
    std::printf("%-34s %12s %12s %12s %14s\n", "inserts", "avg us", "p99 us", "max us", "x real-time");
    std::vector<float> buf(2 * kBlock);
    const uint32_t session_blocks = std::min(blocks, 1200u);
    const char* names[] = {"none (same session)", "2 compressors per track (64)", "LFO + 2 compressors, 2 routes/track"};
    for (int with = 2; with >= 0; --with) {
        for (uint32_t t = 0; t < 32; ++t) {
            tracks[t].devices = &devs[3 * t + (with == 2 ? 0 : 1)];
            tracks[t].device_count = with == 2 ? 3 : with == 1 ? 2 : 0;
        }
        se_session_publish_v5(e, &s);
        se_transport_seek(e, 0);
        se_transport_play(e);
        std::vector<double> t;
        for (uint32_t b = 0; b < session_blocks + 50; ++b) {
            const auto t0 = Clock::now();
            se_render_offline(e, buf.data(), kBlock);
            const auto t1 = Clock::now();
            if (b >= 50) t.push_back(us(t1 - t0));
        }
        row(names[with], summarize(t));
    }
    // Chain meters (the level meter after each device's card): metering on for
    // all 64 compressors, the UI reading every peak about once per frame.
    {
        std::vector<uint64_t> ids;
        for (uint32_t t = 0; t < 32; ++t) {
            tracks[t].devices = &devs[3 * t + 1];
            tracks[t].device_count = 2;
            ids.push_back(1000 + 2 * t);
            ids.push_back(1000 + 2 * t + 1);
        }
        se_session_publish_v5(e, &s);
        for (uint64_t id : ids) se_device_set_output_metering(e, id, 1);
        se_transport_seek(e, 0);
        se_transport_play(e);
        std::vector<float> peaks(2 * ids.size());
        std::vector<double> t;
        for (uint32_t b = 0; b < session_blocks + 50; ++b) {
            const auto t0 = Clock::now();
            se_render_offline(e, buf.data(), kBlock);
            const auto t1 = Clock::now();
            if (b >= 50) t.push_back(us(t1 - t0));
            if (b % 4 == 0) se_device_output_peaks(e, ids.data(), ids.size(), peaks.data());
        }
        row("2 compressors/track, chain meters on", summarize(t));
        const int n = 20000;
        const auto r0 = Clock::now();
        for (int i = 0; i < n; ++i) se_device_output_peaks(e, ids.data(), ids.size(), peaks.data());
        std::printf("  %-32s %10.3f us/call\n", "se_device_output_peaks (64 ids)", us(Clock::now() - r0) / n);
        for (uint64_t id : ids) se_device_set_output_metering(e, id, 0);
    }
    se_engine_destroy(e);
    std::filesystem::remove(wav);
    return 0;
}
