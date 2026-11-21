// Offline render benchmark: N tracks x M clips through se_render_offline
// (the device-callback process path) at 48 kHz / 256 frames.
// Prints measurements only; no pass/fail thresholds.
//
// usage: dstudio_engine_bench [blocks=4000] [clips_per_track=4] [features=0]
// A last table renders 8 armed + monitored tracks fed by injected input,
// with recording off and on (the recording writer thread runs concurrently).
//   features=1: also enable a 48 001-frame loop (odd length, so the wrap
//   lands mid-block) and the metronome at 120 BPM.
// A pattern table renders instrument tracks playing step-sequencer patterns
// (4 to 16 rows, up to 64 steps a bar split into 16 repeats each).
// A second table renders MIDI: instrument tracks (no clips) with 8 notes
// held for the whole run plus one short note retriggered every 100 ms, so
// 8-9 synth voices sound on every track in every block.
// A plugin table renders 32 audio tracks (4 clips each) with two test-gain
// inserts per track (the DStudioTestGain CLAP and VST3 bundles built with the
// engine), i.e. 64 plugin process calls per block.
// A delay compensation table renders the same 32 audio tracks with one
// DStudioTestLatency insert (N frames) on the first track, PDC on and off:
// N = 0 (L = 0, nothing to compensate) and N = 4096 (31 tracks + the
// metronome line delayed by 4096 frames).
// An automation table renders the 32 tracks with the CLAP + VST3 gain
// inserts and 4 lanes per track (volume, pan, and the gain parameter of each
// insert), each with a curved point every 100 ms (se_session_publish_v6),
// against the same session without lanes, and with 3 switch lanes more per
// track (mute, flipping every 100 ms, and an on/off lane per insert, kept on).
// A MIDI input table renders built-in synth tracks with live input injected
// (se_midi_inject) to none, the focus track, or every track.
// A note preview table renders 32 CLAP instrument tracks with piano-roll
// preview notes (se_preview_note) queued between blocks, against none.
// A tempo map table renders the 32 tracks with the CLAP + VST3 gain inserts
// (the plugin host asks for the song position every block) and the
// metronome, with one tempo against a 64-point map (se_session_publish_v12;
// a point every quarter note, alternating holds and ramps between 90 and
// 150 BPM).
#include "platform/os.hpp"
#include "dstudio_engine.h"

#include <algorithm>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <filesystem>
#include <numbers>
#include <string>
#include <thread>
#include <vector>

namespace {

constexpr double kRate = 48000.0;
constexpr uint32_t kBlock = 256;

bool write_float_wav(const std::string& path, const std::vector<float>& l, const std::vector<float>& r) {
    std::FILE* f = dstudio::os::fopen_utf8(path.c_str(), "wb");
    if (!f) return false;
    const auto frames = static_cast<uint32_t>(l.size());
    const uint32_t data = frames * 8;
    auto u32 = [&](uint32_t v) { std::fwrite(&v, 4, 1, f); }; // little-endian host assumed (macOS/arm64, x86)
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

struct Result {
    double us_per_block_avg;
    double us_per_block_p99;
    double us_per_block_max;
    double rt_ratio;
};

// plugins: 0 = none (se_session_publish), 1 = CLAP gain + VST3 gain per
// track, 2 = two CLAP gains per track (se_session_publish_v5), 3 = one
// DStudioTestLatency (N = `latency`) on track 1 only, delay compensation
// `pdc` on/off. routing (se_session_publish_v9, plugins 0): 1 = two return
// tracks, every track sending to both (one post-, one pre-fader); 2 = the
// same with a DStudioTestLatency (N = `latency`) on the first return, so every
// track's output also runs through an edge delay line; 3 = routing 1 plus a
// send from the first return into the second (spec 016). strip
// (se_session_publish_v10, plugins 0, routing 0): 1 = trim and width on every
// track; 2 = trim and mono on every track.
Result run(uint32_t tracks, uint32_t clips_per_track, uint32_t blocks, const std::vector<std::string>& files,
           bool features, int plugins = 0, uint32_t latency = 0, bool pdc = true, int automation = 0,
           float preview_rate = 0.0f, const std::string& preview_file = {}, int routing = 0, int strip = 0,
           bool tempo = false) {
    se_config cfg{SE_ABI_VERSION, kRate, kBlock, 2};
    se_engine* e = se_engine_create(&cfg);
    if (!e) {
        std::fprintf(stderr, "create failed: %s\n", se_last_error());
        std::exit(1);
    }
    std::vector<uint64_t> assets;
    for (const auto& f : files) {
        se_asset_info info{};
        if (se_asset_load_file(e, f.c_str(), &info) != SE_OK) {
            std::fprintf(stderr, "load failed: %s\n", se_last_error());
            std::exit(1);
        }
        assets.push_back(info.asset_id);
    }
    // Every track has back-to-back (slightly overlapping) clips covering the
    // whole rendered range, so all tracks are active in every block.
    const int64_t total = static_cast<int64_t>(blocks) * kBlock;
    const int64_t clip_len = total / clips_per_track + 1024;
    std::vector<std::vector<se_clip_desc>> clips(tracks);
    std::vector<se_track_desc> tds(tracks);
    for (uint32_t t = 0; t < tracks; ++t) {
        for (uint32_t c = 0; c < clips_per_track; ++c) {
            se_clip_desc cd{};
            cd.clip_id = c + 1;
            cd.asset_id = assets[(t + c) % assets.size()];
            cd.timeline_start = static_cast<int64_t>(c) * (total / clips_per_track);
            cd.source_offset = (t * 131) % 1000;
            cd.length = clip_len;
            cd.gain = 0.5f;
            clips[t].push_back(cd);
        }
        se_track_desc& td = tds[t];
        td = se_track_desc{};
        td.track_id = t + 1;
        td.gain = 0.25f;
        td.pan = std::sin(static_cast<float>(t));
        td.clips = clips[t].data();
        td.clip_count = clips_per_track;
    }
    se_session_desc sd{tds.data(), tracks, 0.5f};
    const std::string clap_gain = std::string(DSTUDIO_TEST_PLUGIN_DIR) + "/DStudioTestGain.clap";
    const std::string vst3_gain = std::string(DSTUDIO_TEST_PLUGIN_DIR) + "/DStudioTestGain.vst3";
    const std::string clap_latency = std::string(DSTUDIO_TEST_PLUGIN_DIR) + "/DStudioTestLatency.clap";
    const uint8_t latency_state[4] = {static_cast<uint8_t>(latency), static_cast<uint8_t>(latency >> 8),
                                      static_cast<uint8_t>(latency >> 16), static_cast<uint8_t>(latency >> 24)};
    se_pdc_set_enabled(e, pdc ? 1 : 0);
    std::vector<std::vector<se_device_desc>> devices(tracks);
    std::vector<se_track_desc_v5> t5(tracks);
    for (uint32_t t = 0; t < tracks && plugins != 0; ++t) {
        for (int k = 0; k < 2 && plugins != 3; ++k) {
            se_device_desc d{};
            d.device_id = 1000 + t * 2 + static_cast<uint32_t>(k);
            const bool vst3 = plugins == 1 && k == 1;
            d.format = static_cast<uint8_t>(vst3 ? SE_PLUGIN_VST3 : SE_PLUGIN_CLAP);
            d.plugin_id = vst3 ? "5A17E5A16E0000018A1E7E576A170001" : "studio.dstudio.test.gain";
            d.path = vst3 ? vst3_gain.c_str() : clap_gain.c_str();
            d.enabled = 1;
            devices[t].push_back(d);
        }
        if (plugins == 3 && t == 0) {
            se_device_desc d{};
            d.device_id = 1000;
            d.format = SE_PLUGIN_CLAP;
            d.plugin_id = "studio.dstudio.test.latency";
            d.path = clap_latency.c_str();
            d.state = latency_state;
            d.state_len = 4;
            d.enabled = 1;
            devices[t].push_back(d);
        }
        se_track_desc_v5& d = t5[t];
        d = se_track_desc_v5{};
        d.track_id = tds[t].track_id;
        d.gain = tds[t].gain;
        d.pan = tds[t].pan;
        d.clips = tds[t].clips;
        d.clip_count = tds[t].clip_count;
        d.devices = devices[t].data();
        d.device_count = static_cast<uint32_t>(devices[t].size());
    }
    se_session_desc_v5 sd5{t5.data(), tracks, 0.5f};
    // Automation: volume, pan and one gain lane per insert, a curved point
    // every 4800 frames; automation 2 adds a mute lane and an on/off lane per
    // insert (stepped).
    std::vector<std::vector<se_auto_point>> points;
    std::vector<std::vector<se_auto_lane_desc>> lanes(tracks);
    std::vector<se_track_desc_v6> t6(tracks);
    const auto make_points = [&](float lo, float hi, uint32_t seed) {
        std::vector<se_auto_point> v;
        for (int64_t f = 0, k = 0; f <= total + 48000; f += 4800, ++k)
            v.push_back({f, (k + seed) % 2 == 0 ? lo : hi, static_cast<float>((k + seed) % 5) * 0.4f - 0.8f});
        return v;
    };
    if (automation) {
        points.reserve(static_cast<std::size_t>(tracks) * 4);
        for (uint32_t t = 0; t < tracks; ++t) {
            const auto add = [&](uint8_t target, uint64_t device, std::vector<se_auto_point> pts, uint32_t flags = 0) {
                points.push_back(std::move(pts));
                se_auto_lane_desc l{};
                l.target = target;
                l.device_id = device;
                l.flags = flags;
                l.points = points.back().data();
                l.point_count = static_cast<uint32_t>(points.back().size());
                lanes[t].push_back(l);
            };
            add(SE_AUTO_VOLUME, 0, make_points(-24.0f, -6.0f, t));
            add(SE_AUTO_PAN, 0, make_points(-0.8f, 0.8f, t + 1));
            for (const se_device_desc& d : devices[t]) add(SE_AUTO_PLUGIN, d.device_id, make_points(0.2f, 0.9f, t + 2));
            if (automation == 2) {
                add(SE_AUTO_MUTE, 0, make_points(0.0f, 1.0f, t), SE_AUTO_FLAG_STEPPED);
                for (const se_device_desc& d : devices[t])
                    add(SE_AUTO_DEVICE_ON, d.device_id, make_points(1.0f, 1.0f, t), SE_AUTO_FLAG_STEPPED);
            }
            t6[t] = se_track_desc_v6{};
            std::memcpy(&t6[t], &t5[t], sizeof(se_track_desc_v5));
            t6[t].lanes = lanes[t].data();
            t6[t].lane_count = static_cast<uint32_t>(lanes[t].size());
        }
    }
    se_session_desc_v6 sd6{t6.data(), tracks, 0.5f, nullptr, 0, 0};
    // Routing: the plain tracks plus two returns, each track sending to both.
    std::vector<se_track_desc_v6> r6;
    std::vector<se_track_route> routes;
    std::vector<se_send_desc> sends;
    std::vector<se_device_desc> return_fx;
    se_session_desc_v9 sd9{};
    std::vector<se_track_strip> strips;
    se_session_desc_v10 sd10{};
    if (routing != 0 || strip != 0) {
        for (uint32_t t = 0; t < tracks; ++t) {
            se_track_desc_v6 d{};
            d.track_id = tds[t].track_id;
            d.gain = tds[t].gain;
            d.pan = tds[t].pan;
            d.clips = tds[t].clips;
            d.clip_count = tds[t].clip_count;
            r6.push_back(d);
            if (strip != 0) strips.push_back({d.track_id, 0.8f, 1.5f, static_cast<uint8_t>(strip == 2 ? 1 : 0), {}});
            if (routing == 0) continue;
            sends.push_back({d.track_id, 9001, 0.3f, 0, {}});
            sends.push_back({d.track_id, 9002, 0.2f, 1, {}});
        }
        if (routing == 2) {
            se_device_desc d{};
            d.device_id = 1000;
            d.format = SE_PLUGIN_CLAP;
            d.plugin_id = "studio.dstudio.test.latency";
            d.path = clap_latency.c_str();
            d.state = latency_state;
            d.state_len = 4;
            d.enabled = 1;
            return_fx.push_back(d);
        }
        for (uint64_t id : {9001ull, 9002ull}) {
            if (routing == 0) break;
            se_track_desc_v6 d{};
            d.track_id = id;
            d.gain = 0.7f;
            if (id == 9001 && !return_fx.empty()) {
                d.devices = return_fx.data();
                d.device_count = 1;
            }
            r6.push_back(d);
            routes.push_back({id, 0, 1, {}});
        }
        if (routing == 3) sends.push_back({9001, 9002, 0.5f, 0, {}}); // Return A into Return B
        sd9.v8.v7.v6 = se_session_desc_v6{r6.data(), static_cast<uint32_t>(r6.size()), 0.5f, nullptr, 0, 0};
        sd9.routes = routes.data();
        sd9.route_count = static_cast<uint32_t>(routes.size());
        sd9.sends = sends.data();
        sd9.send_count = static_cast<uint32_t>(sends.size());
        sd10.v9 = sd9;
        sd10.strips = strips.data();
        sd10.strip_count = static_cast<uint32_t>(strips.size());
    }
    // Tempo map: 64 points a quarter note apart, alternately holding and
    // ramping between 90 and 150 BPM (segment frames from the formula).
    std::vector<se_tempo_segment> segs;
    se_session_desc_v12 sd12{};
    if (tempo) {
        double frame = 0.0;
        for (int i = 0; i < 64; ++i) {
            const double a = i % 4 < 2 ? 90.0 : 150.0;
            const double b = i == 63 || i % 2 == 0 ? a : (i % 4 == 1 ? 150.0 : 90.0);
            segs.push_back({frame, static_cast<double>(i), a, b});
            frame += a == b ? 60.0 * kRate / a : 60.0 * kRate / (b - a) * std::log(b / a);
        }
        for (uint32_t t = 0; t < tracks; ++t) {
            t6[t] = se_track_desc_v6{};
            std::memcpy(&t6[t], &t5[t], sizeof(se_track_desc_v5));
        }
        sd12.v11.v10.v9.v8.v7.v6 = sd6;
        sd12.tempo = segs.data();
        sd12.tempo_count = static_cast<uint32_t>(segs.size());
        sd12.click_quarters = 1.0;
    }
    const int32_t published = tempo         ? se_session_publish_v12(e, &sd12)
                              : strip != 0    ? se_session_publish_v10(e, &sd10)
                              : routing != 0 ? se_session_publish_v9(e, &sd9)
                              : automation    ? se_session_publish_v6(e, &sd6)
                              : plugins == 0 ? se_session_publish(e, &sd)
                                             : se_session_publish_v5(e, &sd5);
    if (published != SE_OK) {
        std::fprintf(stderr, "publish failed: %s\n", se_last_error());
        std::exit(1);
    }
    if (plugins != 0 || routing == 2) {
        size_t errors = 0;
        se_plugin_load_errors(e, nullptr, 0, &errors);
        if (errors != 0) {
            std::fprintf(stderr, "test plugins failed to load (built? %s)\n", DSTUDIO_TEST_PLUGIN_DIR);
            std::exit(1);
        }
    }
    se_transport_play(e);
    if (features) {
        // Odd loop length so the wrap lands mid-block; 120 BPM click.
        se_transport_set_loop(e, 1, 1000, 1000 + 48001);
        se_metronome_desc m{};
        m.enabled = 1;
        m.gain = 0.5f;
        m.frames_per_beat = 24000.0;
        m.beats_per_bar = 4;
        se_metronome_set(e, &m);
    }

    std::vector<float> buf(kBlock * 2);
    for (int i = 0; i < 50; ++i) se_render_offline(e, buf.data(), kBlock); // warm-up
    se_transport_seek(e, 0);
    if (preview_rate > 0.0f) {
        se_asset_info pi{};
        if (se_asset_load_file(e, preview_file.c_str(), &pi) != SE_OK ||
            se_preview_play(e, pi.asset_id, 0, 1.0f, preview_rate, 0) != SE_OK) {
            std::fprintf(stderr, "preview failed: %s\n", se_last_error());
            std::exit(1);
        }
    }

    std::vector<double> times(blocks);
    const auto t_all = std::chrono::steady_clock::now();
    for (uint32_t b = 0; b < blocks; ++b) {
        const auto t0 = std::chrono::steady_clock::now();
        se_render_offline(e, buf.data(), kBlock);
        times[b] = std::chrono::duration<double, std::micro>(std::chrono::steady_clock::now() - t0).count();
    }
    const double wall_us = std::chrono::duration<double, std::micro>(std::chrono::steady_clock::now() - t_all).count();
    se_engine_destroy(e);

    std::vector<double> sorted = times;
    std::sort(sorted.begin(), sorted.end());
    Result r{};
    r.us_per_block_avg = wall_us / blocks;
    r.us_per_block_p99 = sorted[static_cast<std::size_t>(0.99 * (blocks - 1))];
    r.us_per_block_max = sorted.empty() ? 0.0 : sorted.back();
    const double audio_us = static_cast<double>(blocks) * kBlock / kRate * 1e6;
    r.rt_ratio = audio_us / wall_us;
    return r;
}

// Piano-roll note preview (se_preview_note): 32 instrument tracks, each with
// the DStudioTestSine CLAP instrument playing 8 held notes and a note
// retriggered every 100 ms. mode 0: no previews; 1: one key held and
// released every 100 ms on track 1 (a person clicking keys); 2: a preview
// note on and off on every track in every block (a stress figure). The
// previews are queued between the timed renders, as the UI thread would.
Result run_preview_notes(uint32_t tracks, uint32_t blocks, int mode) {
    se_config cfg{SE_ABI_VERSION, kRate, kBlock, 2};
    se_engine* e = se_engine_create(&cfg);
    if (!e) {
        std::fprintf(stderr, "create failed: %s\n", se_last_error());
        std::exit(1);
    }
    const int64_t total = static_cast<int64_t>(blocks) * kBlock;
    const std::string sine = std::string(DSTUDIO_TEST_PLUGIN_DIR) + "/DStudioTestSine.clap";
    std::vector<std::vector<se_note_desc>> notes(tracks);
    std::vector<se_device_desc> insts(tracks);
    std::vector<se_track_desc_v5> tds(tracks);
    for (uint32_t t = 0; t < tracks; ++t) {
        for (uint32_t v = 0; v < 8; ++v) {
            se_note_desc n{};
            n.start = static_cast<int64_t>(v) * 7;
            n.end = total + 48000;
            n.pitch = static_cast<uint8_t>(36 + (t * 5 + v * 7) % 48);
            n.velocity = static_cast<uint8_t>(60 + v * 8);
            notes[t].push_back(n);
        }
        for (int64_t s = 4800; s < total; s += 4800) {
            se_note_desc n{};
            n.start = s + t;
            n.end = s + t + 1200;
            n.pitch = static_cast<uint8_t>(72 + t % 12);
            n.velocity = 90;
            notes[t].push_back(n);
        }
        se_device_desc& d = insts[t];
        d = se_device_desc{};
        d.device_id = 1000 + t;
        d.format = SE_PLUGIN_CLAP;
        d.path = sine.c_str();
        d.plugin_id = "studio.dstudio.test.sine";
        d.enabled = 1;
        se_track_desc_v5& td = tds[t];
        td = se_track_desc_v5{};
        td.track_id = t + 1;
        td.gain = 0.25f;
        td.pan = std::sin(static_cast<float>(t));
        td.instrument = 1;
        td.notes = notes[t].data();
        td.note_count = static_cast<uint32_t>(notes[t].size());
        td.instrument_plugin = &insts[t];
        td.track_name = "Track";
    }
    se_session_desc_v5 sd{};
    sd.tracks = tds.data();
    sd.track_count = tracks;
    sd.master_gain = 0.5f;
    if (se_session_publish_v5(e, &sd) != SE_OK) {
        std::fprintf(stderr, "publish failed: %s\n", se_last_error());
        std::exit(1);
    }
    std::vector<float> buf(2 * kBlock);
    se_transport_play(e);
    for (int i = 0; i < 16; ++i) se_render_offline(e, buf.data(), kBlock); // warm-up
    const uint32_t every = static_cast<uint32_t>(kRate / 10 / kBlock);   // ~100 ms
    std::vector<double> times(blocks);
    double wall_us = 0.0;
    for (uint32_t b = 0; b < blocks; ++b) {
        if (mode == 1 && b % every == 0) se_preview_note(e, 1000, static_cast<uint8_t>(48 + (b / every) % 24), (b / every) % 2 ? 0 : 102);
        if (mode == 2)
            for (uint32_t t = 0; t < tracks; ++t) se_preview_note(e, 1000 + t, static_cast<uint8_t>(60 + t % 12), b % 2 ? 0 : 102);
        const auto t0 = std::chrono::steady_clock::now();
        se_render_offline(e, buf.data(), kBlock);
        times[b] = std::chrono::duration<double, std::micro>(std::chrono::steady_clock::now() - t0).count();
        wall_us += times[b];
    }
    se_engine_destroy(e);
    std::vector<double> sorted = times;
    std::sort(sorted.begin(), sorted.end());
    Result r{};
    r.us_per_block_avg = wall_us / blocks;
    r.us_per_block_p99 = sorted[static_cast<std::size_t>(0.99 * (blocks - 1))];
    r.us_per_block_max = sorted.empty() ? 0.0 : sorted.back();
    r.rt_ratio = static_cast<double>(blocks) * kBlock / kRate * 1e6 / wall_us;
    return r;
}

Result run_midi(uint32_t tracks, uint32_t blocks, const se_synth_params* params) {
    se_config cfg{SE_ABI_VERSION, kRate, kBlock, 2};
    se_engine* e = se_engine_create(&cfg);
    if (!e) {
        std::fprintf(stderr, "create failed: %s\n", se_last_error());
        std::exit(1);
    }
    const int64_t total = static_cast<int64_t>(blocks) * kBlock;
    std::vector<std::vector<se_note_desc>> notes(tracks);
    std::vector<se_track_desc_v3> tds(tracks);
    for (uint32_t t = 0; t < tracks; ++t) {
        for (uint32_t v = 0; v < 8; ++v) {
            se_note_desc n{};
            n.start = static_cast<int64_t>(v) * 7; // staggered onsets
            n.end = total + 48000;
            n.pitch = static_cast<uint8_t>(36 + (t * 5 + v * 7) % 48);
            n.velocity = static_cast<uint8_t>(60 + v * 8);
            notes[t].push_back(n);
        }
        for (int64_t s = 4800; s < total; s += 4800) { // retriggered short note
            se_note_desc n{};
            n.start = s + t;
            n.end = s + t + 1200;
            n.pitch = static_cast<uint8_t>(72 + t % 12);
            n.velocity = 90;
            notes[t].push_back(n);
        }
        se_track_desc_v3& td = tds[t];
        td = se_track_desc_v3{};
        td.track_id = t + 1;
        td.gain = 0.25f;
        td.pan = std::sin(static_cast<float>(t));
        td.instrument = 1;
        td.notes = notes[t].data();
        td.note_count = static_cast<uint32_t>(notes[t].size());
        td.synth = params;
    }
    se_session_desc_v3 sd{tds.data(), tracks, 0.5f};
    if (se_session_publish_v3(e, &sd) != SE_OK) {
        std::fprintf(stderr, "publish failed: %s\n", se_last_error());
        std::exit(1);
    }
    se_transport_play(e);
    std::vector<float> buf(kBlock * 2);
    // No warm-up seek here: a seek would fade the held voices. Time from 0.
    std::vector<double> times(blocks);
    const auto t_all = std::chrono::steady_clock::now();
    for (uint32_t b = 0; b < blocks; ++b) {
        const auto t0 = std::chrono::steady_clock::now();
        se_render_offline(e, buf.data(), kBlock);
        times[b] = std::chrono::duration<double, std::micro>(std::chrono::steady_clock::now() - t0).count();
    }
    const double wall_us = std::chrono::duration<double, std::micro>(std::chrono::steady_clock::now() - t_all).count();
    se_engine_destroy(e);
    if (times.empty()) return Result{}; // also tells GCC -Wnull-dereference that back() is safe
    std::vector<double> sorted = times;
    std::sort(sorted.begin(), sorted.end());
    Result r{};
    r.us_per_block_avg = wall_us / blocks;
    r.us_per_block_p99 = sorted[static_cast<std::size_t>(0.99 * (blocks - 1))];
    r.us_per_block_max = sorted.empty() ? 0.0 : sorted.back();
    r.rt_ratio = static_cast<double>(blocks) * kBlock / kRate * 1e6 / wall_us;
    return r;
}

// MIDI input: `tracks` built-in synth tracks with 8 held sequenced voices
// each (as run_midi, default params), plus live input: a 4-note chord
// injected every 10 blocks and released 5 blocks later.
//   mode 0: no input at all (the baseline: MIDI input code idle)
//   mode 1: input to the focus track only (one track plays live notes)
//   mode 2: every track monitoring (each track plays the live chord too)
Result run_midi_input(uint32_t tracks, uint32_t blocks, int mode) {
    se_config cfg{SE_ABI_VERSION, kRate, kBlock, 2};
    se_engine* e = se_engine_create(&cfg);
    if (!e) {
        std::fprintf(stderr, "create failed: %s\n", se_last_error());
        std::exit(1);
    }
    const int64_t total = static_cast<int64_t>(blocks) * kBlock;
    std::vector<std::vector<se_note_desc>> notes(tracks);
    std::vector<se_track_desc_v4> tds(tracks);
    for (uint32_t t = 0; t < tracks; ++t) {
        for (uint32_t v = 0; v < 8; ++v) {
            se_note_desc n{};
            n.start = static_cast<int64_t>(v) * 7;
            n.end = total + 48000;
            n.pitch = static_cast<uint8_t>(36 + (t * 5 + v * 7) % 48);
            n.velocity = static_cast<uint8_t>(60 + v * 8);
            notes[t].push_back(n);
        }
        se_track_desc_v4& td = tds[t];
        td = se_track_desc_v4{};
        td.track_id = t + 1;
        td.gain = 0.25f;
        td.pan = std::sin(static_cast<float>(t));
        td.instrument = 1;
        td.monitor = mode == 2 ? 1 : 0;
        td.notes = notes[t].data();
        td.note_count = static_cast<uint32_t>(notes[t].size());
    }
    se_session_desc_v4 sd{tds.data(), tracks, 0.5f};
    if (se_session_publish_v4(e, &sd) != SE_OK) {
        std::fprintf(stderr, "publish failed: %s\n", se_last_error());
        std::exit(1);
    }
    se_midi_set_focus(e, mode == 1 ? 1 : 0);
    se_transport_play(e);
    std::vector<float> buf(kBlock * 2);
    std::vector<double> times(blocks);
    const uint8_t chord[4] = {60, 64, 67, 71};
    double wall_us = 0.0; // render time only: injecting (the MIDI thread's work) is not timed
    for (uint32_t b = 0; b < blocks; ++b) {
        if (mode != 0 && b % 10 == 0)
            for (uint8_t p : chord) se_midi_inject(e, 0x90, p, 100);
        if (mode != 0 && b % 10 == 5)
            for (uint8_t p : chord) se_midi_inject(e, 0x80, p, 0);
        const auto t0 = std::chrono::steady_clock::now();
        se_render_offline(e, buf.data(), kBlock);
        times[b] = std::chrono::duration<double, std::micro>(std::chrono::steady_clock::now() - t0).count();
        wall_us += times[b];
    }
    se_engine_destroy(e);
    if (times.empty()) return Result{};
    std::vector<double> sorted = times;
    std::sort(sorted.begin(), sorted.end());
    Result r{};
    r.us_per_block_avg = wall_us / blocks;
    r.us_per_block_p99 = sorted[static_cast<std::size_t>(0.99 * (blocks - 1))];
    r.us_per_block_max = sorted.empty() ? 0.0 : sorted.back();
    r.rt_ratio = static_cast<double>(blocks) * kBlock / kRate * 1e6 / wall_us;
    return r;
}

// Pattern: `tracks` instrument tracks playing a step pattern as dstudio-core
// bakes and repeats it (one bar of 4/4 at 120 BPM): `rows` keys, a hit on
// every one of `steps` steps per bar, each split into `repeat` hits with
// the gate gap, repeated for the whole run. This is the note density the step
// sequencer can publish; the engine plays it as ordinary notes.
Result run_pattern(uint32_t tracks, uint32_t blocks, uint32_t rows, uint32_t steps, uint32_t repeat) {
    se_config cfg{SE_ABI_VERSION, kRate, kBlock, 2};
    se_engine* e = se_engine_create(&cfg);
    if (!e) {
        std::fprintf(stderr, "create failed: %s\n", se_last_error());
        std::exit(1);
    }
    const int64_t total = static_cast<int64_t>(blocks) * kBlock;
    const double bar = 2.0 * kRate;
    const double step = bar / steps;
    const double span = step / repeat;
    const double gap = 0.02 * kRate / 2.0; // 0.02 beat at 120 BPM
    const double gate = repeat > 1 ? std::max(span * 0.5, span - gap) : step;
    std::vector<std::vector<se_note_desc>> notes(tracks);
    std::vector<se_track_desc_v3> tds(tracks);
    for (uint32_t t = 0; t < tracks; ++t) {
        for (double b0 = 0; b0 < static_cast<double>(total); b0 += bar) {
            for (uint32_t s = 0; s < steps; ++s) {
                for (uint32_t k = 0; k < repeat; ++k) {
                    for (uint32_t row = 0; row < rows; ++row) {
                        se_note_desc n{};
                        n.start = static_cast<int64_t>(b0 + s * step + k * span);
                        n.end = n.start + static_cast<int64_t>(gate);
                        n.pitch = static_cast<uint8_t>(36 + row);
                        n.velocity = static_cast<uint8_t>(70 + (s * 7 + row) % 50);
                        notes[t].push_back(n);
                    }
                }
            }
        }
        std::sort(notes[t].begin(), notes[t].end(), [](const se_note_desc& a, const se_note_desc& b) { return a.start < b.start; });
        se_track_desc_v3& td = tds[t];
        td = se_track_desc_v3{};
        td.track_id = t + 1;
        td.gain = 0.25f;
        td.instrument = 1;
        td.notes = notes[t].data();
        td.note_count = static_cast<uint32_t>(notes[t].size());
    }
    se_session_desc_v3 sd{tds.data(), tracks, 0.5f};
    if (se_session_publish_v3(e, &sd) != SE_OK) {
        std::fprintf(stderr, "publish failed: %s\n", se_last_error());
        std::exit(1);
    }
    se_transport_play(e);
    std::vector<float> buf(kBlock * 2);
    std::vector<double> times(blocks);
    const auto t_all = std::chrono::steady_clock::now();
    for (uint32_t b = 0; b < blocks; ++b) {
        const auto t0 = std::chrono::steady_clock::now();
        se_render_offline(e, buf.data(), kBlock);
        times[b] = std::chrono::duration<double, std::micro>(std::chrono::steady_clock::now() - t0).count();
    }
    const double wall_us = std::chrono::duration<double, std::micro>(std::chrono::steady_clock::now() - t_all).count();
    se_engine_destroy(e);
    std::sort(times.begin(), times.end());
    Result r{};
    r.us_per_block_avg = wall_us / blocks;
    r.us_per_block_p99 = times[static_cast<std::size_t>(0.99 * (blocks - 1))];
    r.us_per_block_max = times.back();
    r.rt_ratio = static_cast<double>(blocks) * kBlock / kRate * 1e6 / wall_us;
    return r;
}

// Input: `tracks` armed + monitored audio tracks (mono inputs spread over 8
// injected channels, every 3rd one stereo), optionally recording to a temp
// dir, rendered through se_render_offline_with_input.
Result run_input(uint32_t tracks, uint32_t blocks, bool record) {
    se_config cfg{SE_ABI_VERSION, kRate, kBlock, 2};
    se_engine* e = se_engine_create(&cfg);
    if (!e) {
        std::fprintf(stderr, "create failed: %s\n", se_last_error());
        std::exit(1);
    }
    constexpr uint32_t kInCh = 8;
    std::vector<se_track_desc_v4> tds(tracks);
    for (uint32_t t = 0; t < tracks; ++t) {
        se_track_desc_v4& td = tds[t];
        td = se_track_desc_v4{};
        td.track_id = t + 1;
        td.gain = 0.25f;
        td.pan = std::sin(static_cast<float>(t));
        td.arm = 1;
        td.monitor = 1;
        td.input_mode = static_cast<uint8_t>(t % 3 == 0 ? SE_INPUT_STEREO : SE_INPUT_MONO);
        td.input_channel = static_cast<uint16_t>(t % (kInCh - 1));
    }
    se_session_desc_v4 sd{tds.data(), tracks, 0.5f};
    if (se_session_publish_v4(e, &sd) != SE_OK) {
        std::fprintf(stderr, "publish failed: %s\n", se_last_error());
        std::exit(1);
    }
    std::vector<float> in(static_cast<std::size_t>(kBlock) * kInCh);
    for (std::size_t i = 0; i < in.size(); ++i) in[i] = 0.1f * std::sin(0.01f * static_cast<float>(i));
    const auto dir = std::filesystem::temp_directory_path() / "dstudio_bench_record";
    if (record && se_record_start(e, dstudio::os::path_to_utf8(dir).c_str()) != SE_OK) {
        std::fprintf(stderr, "record failed: %s\n", se_last_error());
        std::exit(1);
    }
    se_transport_play(e);
    std::vector<float> buf(kBlock * 2);
    for (int i = 0; i < 50; ++i) se_render_offline_with_input(e, in.data(), kInCh, buf.data(), kBlock); // warm-up
    // Offline rendering runs thousands of times faster than real time, so
    // with recording on we pause (untimed) every 256 blocks (~1.4 s of
    // audio, under the 2 s ring) to let the writer thread catch up, and time
    // only the render calls. Otherwise every block would hit the (cheaper)
    // overrun path instead of the copy we want to measure.
    std::vector<double> times(blocks);
    double wall_us = 0.0;
    for (uint32_t b = 0; b < blocks; ++b) {
        if (record && b % 256 == 255) std::this_thread::sleep_for(std::chrono::milliseconds(30));
        const auto t0 = std::chrono::steady_clock::now();
        se_render_offline_with_input(e, in.data(), kInCh, buf.data(), kBlock);
        times[b] = std::chrono::duration<double, std::micro>(std::chrono::steady_clock::now() - t0).count();
        wall_us += times[b];
    }
    if (record) {
        std::size_t n = 0;
        se_record_stop(e, &n);
        se_record_stats st{};
        se_record_stats_get(e, &st);
        if (st.overruns != 0) std::printf("   (recording overruns: %llu)\n", static_cast<unsigned long long>(st.overruns));
        std::filesystem::remove_all(dir);
    }
    se_engine_destroy(e);
    if (times.empty()) return Result{}; // also tells GCC -Wnull-dereference that back() is safe
    std::vector<double> sorted = times;
    std::sort(sorted.begin(), sorted.end());
    Result r{};
    r.us_per_block_avg = wall_us / blocks;
    r.us_per_block_p99 = sorted[static_cast<std::size_t>(0.99 * (blocks - 1))];
    r.us_per_block_max = sorted.empty() ? 0.0 : sorted.back();
    r.rt_ratio = static_cast<double>(blocks) * kBlock / kRate * 1e6 / wall_us;
    return r;
}

} // namespace

int main(int argc, char** argv) {
    const uint32_t blocks = argc > 1 ? static_cast<uint32_t>(std::atoi(argv[1])) : 4000;
    const uint32_t clips_per_track = argc > 2 ? static_cast<uint32_t>(std::atoi(argv[2])) : 4;
    const bool features = argc > 3 && std::atoi(argv[3]) != 0;

    // A handful of 20 s stereo assets (decoded once per engine).
    std::vector<std::string> files;
    const auto dir = std::filesystem::temp_directory_path();
    const std::size_t frames = static_cast<std::size_t>(kRate * 20);
    for (int a = 0; a < 4; ++a) {
        std::vector<float> l(frames), r(frames);
        for (std::size_t i = 0; i < frames; ++i) {
            const double t = static_cast<double>(i) / kRate;
            l[i] = static_cast<float>(0.3 * std::sin(2 * std::numbers::pi * (110.0 * (a + 1)) * t));
            r[i] = static_cast<float>(0.3 * std::sin(2 * std::numbers::pi * (165.0 * (a + 1)) * t));
        }
        const std::string path = dstudio::os::path_to_utf8(dir / ("dstudio_bench_asset_" + std::to_string(a) + ".wav"));
        if (!write_float_wav(path, l, r)) {
            std::fprintf(stderr, "cannot write %s\n", path.c_str());
            return 1;
        }
        files.push_back(path);
    }

    const double deadline_us = kBlock / kRate * 1e6;
    std::printf("dstudio_engine_bench: %u blocks x %u frames @ %.0f Hz (deadline %.1f us/block), %u clips/track%s\n",
                blocks, kBlock, kRate, deadline_us, clips_per_track, features ? ", loop + metronome" : "");
    std::printf("%8s %14s %14s %14s %16s\n", "tracks", "avg us/block", "p99 us/block", "max us/block",
                "realtime ratio");
    for (uint32_t tracks : {8u, 32u, 128u}) {
        const Result r = run(tracks, clips_per_track, blocks, files, features);
        std::printf("%8u %14.2f %14.2f %14.2f %15.1fx\n", tracks, r.us_per_block_avg, r.us_per_block_p99,
                    r.us_per_block_max, r.rt_ratio);
    }
    std::printf("\nPlugins: 32 audio tracks x %u clips + 2 test-gain inserts per track\n", clips_per_track);
    std::printf("%22s %14s %14s %14s %16s\n", "inserts", "avg us/block", "p99 us/block", "max us/block",
                "realtime ratio");
    for (int mode : {0, 1, 2}) {
        const Result r = run(32, clips_per_track, blocks, files, features, mode);
        const char* name = mode == 0 ? "none (baseline)" : mode == 1 ? "CLAP gain + VST3 gain" : "2 x CLAP gain";
        std::printf("%22s %14.2f %14.2f %14.2f %15.1fx\n", name, r.us_per_block_avg, r.us_per_block_p99,
                    r.us_per_block_max, r.rt_ratio);
    }
    std::printf("\nDelay compensation: 32 audio tracks x %u clips, one DStudioTestLatency insert (N frames) on track 1\n",
                clips_per_track);
    std::printf("%22s %14s %14s %14s %16s\n", "N / PDC", "avg us/block", "p99 us/block", "max us/block",
                "realtime ratio");
    for (const uint32_t n : {0u, 4096u})
        for (const bool pdc : {false, true}) {
            const Result r = run(32, clips_per_track, blocks, files, features, 3, n, pdc);
            char name[32];
            std::snprintf(name, sizeof name, "N=%u, PDC %s", n, pdc ? "on" : "off");
            std::printf("%22s %14.2f %14.2f %14.2f %15.1fx\n", name, r.us_per_block_avg, r.us_per_block_p99,
                        r.us_per_block_max, r.rt_ratio);
        }
    std::printf("\nAutomation: 32 audio tracks x %u clips + CLAP gain + VST3 gain, 4 lanes per track "
                "(volume, pan, 2 plugin params; a point every 100 ms), then 3 switch lanes more "
                "(mute, 2 inserts on/off)\n",
                clips_per_track);
    std::printf("%22s %14s %14s %14s %16s\n", "lanes", "avg us/block", "p99 us/block", "max us/block",
                "realtime ratio");
    for (const int automation : {0, 1, 2}) {
        const Result r = run(32, clips_per_track, blocks, files, features, 1, 0, true, automation);
        const char* name = automation == 0 ? "none (same session)" : automation == 1 ? "32 x 4 lanes" : "32 x 7 lanes";
        std::printf("%22s %14.2f %14.2f %14.2f %15.1fx\n", name,
                    r.us_per_block_avg, r.us_per_block_p99, r.us_per_block_max, r.rt_ratio);
    }
    std::printf("\nReturns: 32 audio tracks x %u clips, each sending to 2 return tracks (one post-, one pre-fader)\n",
                clips_per_track);
    std::printf("%22s %14s %14s %14s %16s\n", "routing", "avg us/block", "p99 us/block", "max us/block",
                "realtime ratio");
    for (const int routing : {0, 1, 3, 2}) {
        const Result r = run(32, clips_per_track, blocks, files, features, 0, 1024, true, false, 0.0f, {}, routing);
        const char* name = routing == 0   ? "none (same tracks)"
                           : routing == 1 ? "2 returns, 64 sends"
                           : routing == 3 ? "+ A into B"
                                          : "+ 1024-frame return";
        std::printf("%22s %14.2f %14.2f %14.2f %15.1fx\n", name, r.us_per_block_avg, r.us_per_block_p99,
                    r.us_per_block_max, r.rt_ratio);
    }
    std::printf("\nChannel strip: 32 audio tracks x %u clips, trim + width or trim + mono on every track\n",
                clips_per_track);
    std::printf("%22s %14s %14s %14s %16s\n", "strip", "avg us/block", "p99 us/block", "max us/block",
                "realtime ratio");
    for (const int strip : {0, 1, 2}) {
        const Result r = run(32, clips_per_track, blocks, files, features, 0, 0, true, false, 0.0f, {}, 0, strip);
        const char* name = strip == 0 ? "none (same tracks)" : strip == 1 ? "trim + width" : "trim + mono";
        std::printf("%22s %14.2f %14.2f %14.2f %15.1fx\n", name, r.us_per_block_avg, r.us_per_block_p99,
                    r.us_per_block_max, r.rt_ratio);
    }
    std::printf("\nTempo map: 32 audio tracks x %u clips + CLAP gain + VST3 gain, metronome on\n", clips_per_track);
    std::printf("%22s %14s %14s %14s %16s\n", "tempo", "avg us/block", "p99 us/block", "max us/block",
                "realtime ratio");
    for (const bool tempo : {false, true}) {
        const Result r = run(32, clips_per_track, blocks, files, true, 1, 0, true, 0, 0.0f, {}, 0, 0, tempo);
        std::printf("%22s %14.2f %14.2f %14.2f %15.1fx\n", tempo ? "64-point map" : "one tempo (same)", r.us_per_block_avg,
                    r.us_per_block_p99, r.us_per_block_max, r.rt_ratio);
    }
    std::printf("\nBrowser preview voice: 32 audio tracks x %u clips, plus one stereo preview sounding in every block\n",
                clips_per_track);
    std::printf("%22s %14s %14s %14s %16s\n", "preview", "avg us/block", "p99 us/block", "max us/block",
                "realtime ratio");
    // A preview long enough to sound through every timed block at the fastest rate.
    const std::size_t pv_frames = static_cast<std::size_t>(1.5 * blocks * kBlock) + 4 * kBlock;
    std::vector<float> pv_l(pv_frames), pv_r(pv_frames);
    for (std::size_t i = 0; i < pv_frames; ++i) {
        pv_l[i] = static_cast<float>(0.2 * std::sin(0.013 * static_cast<double>(i)));
        pv_r[i] = static_cast<float>(0.2 * std::sin(0.017 * static_cast<double>(i)));
    }
    const std::string pv_path = dstudio::os::path_to_utf8(dir / "dstudio_bench_preview.wav");
    if (!write_float_wav(pv_path, pv_l, pv_r)) {
        std::fprintf(stderr, "cannot write %s\n", pv_path.c_str());
        return 1;
    }
    for (const float rate : {0.0f, 1.0f, 1.5f}) {
        const Result r = run(32, clips_per_track, blocks, files, features, 0, 0, true, false, rate, pv_path);
        char name[32];
        if (rate == 0.0f)
            std::snprintf(name, sizeof name, "none (baseline)");
        else
            std::snprintf(name, sizeof name, "rate %.1f", static_cast<double>(rate));
        std::printf("%22s %14.2f %14.2f %14.2f %15.1fx\n", name, r.us_per_block_avg, r.us_per_block_p99,
                    r.us_per_block_max, r.rt_ratio);
    }
    std::filesystem::remove(pv_path);
    std::printf("\nPiano-roll note preview: 32 instrument tracks, DStudioTestSine CLAP instrument, 8-9 notes each\n");
    std::printf("%22s %14s %14s %14s %16s\n", "previews", "avg us/block", "p99 us/block", "max us/block",
                "realtime ratio");
    for (const int mode : {0, 1, 2}) {
        const Result r = run_preview_notes(32, blocks, mode);
        const char* name = mode == 0 ? "none (baseline)" : mode == 1 ? "a key every 100 ms" : "32 per block";
        std::printf("%22s %14.2f %14.2f %14.2f %15.1fx\n", name, r.us_per_block_avg, r.us_per_block_p99,
                    r.us_per_block_max, r.rt_ratio);
    }
    for (const auto& f : files) std::filesystem::remove(f);

    // Expensive patch: 4-voice unison, cross FM, sub + noise, resonant
    // band-pass with filter envelope, LFO on pitch / cutoff / amplitude.
    se_synth_params heavy{};
    se_synth_params_default(&heavy);
    heavy.unison = 4;
    heavy.unison_detune_cents = 20.0f;
    heavy.cross_fm = 0.6f;
    heavy.osc1_wave = SE_WAVE_TRIANGLE;
    heavy.osc1_shape = 0.4f;
    heavy.osc2_wave = SE_WAVE_SINE;
    heavy.osc2_shape = 0.3f;
    heavy.sub_level = 0.4f;
    heavy.noise_level = 0.05f;
    heavy.filter_mode = SE_FILTER_BANDPASS;
    heavy.filter_resonance = 0.5f;
    heavy.filter_env_amount = 0.4f;
    heavy.lfo_to_pitch_cents = 10.0f;
    heavy.lfo_to_cutoff_oct = 0.5f;
    heavy.lfo_to_amp = 0.2f;
    const struct {
        const char* name;
        const se_synth_params* params;
    } patches[] = {{"default params", nullptr}, {"heavy patch: unison 4 + cross FM + sub/noise + LFO", &heavy}};
    for (const auto& patch : patches) {
        std::printf("\nMIDI (%s): instrument tracks x 8 held voices (+1 retriggered note / 100 ms), no clips\n",
                    patch.name);
        std::printf("%8s %14s %14s %14s %16s\n", "tracks", "avg us/block", "p99 us/block", "max us/block",
                    "realtime ratio");
        for (uint32_t tracks : {8u, 32u, 128u}) {
            const Result r = run_midi(tracks, blocks, patch.params);
            std::printf("%8u %14.2f %14.2f %14.2f %15.1fx\n", tracks, r.us_per_block_avg, r.us_per_block_p99,
                        r.us_per_block_max, r.rt_ratio);
        }
    }

    std::printf("\nMIDI input: built-in synth tracks x 8 held voices, a live 4-note chord every 10 blocks (5 held)\n");
    std::printf("%8s %22s %14s %14s %14s %16s\n", "tracks", "live input", "avg us/block", "p99 us/block",
                "max us/block", "realtime ratio");
    for (uint32_t tracks : {8u, 32u}) {
        for (int mode : {0, 1, 2}) {
            const Result r = run_midi_input(tracks, blocks, mode);
            const char* name = mode == 0 ? "none" : mode == 1 ? "focus track" : "every track monitors";
            std::printf("%8u %22s %14.2f %14.2f %14.2f %15.1fx\n", tracks, name, r.us_per_block_avg,
                        r.us_per_block_p99, r.us_per_block_max, r.rt_ratio);
        }
    }

    std::printf("\nPattern: instrument tracks (default synth) playing a step pattern, 1 bar at 120 BPM repeated\n");
    std::printf("%8s %22s %14s %14s %14s %16s\n", "tracks", "rows x steps x repeat", "avg us/block", "p99 us/block",
                "max us/block", "realtime ratio");
    const struct {
        uint32_t rows, steps, repeat;
    } patterns[] = {{4, 16, 1}, {16, 16, 1}, {16, 32, 4}, {16, 64, 16}};
    for (uint32_t tracks : {8u, 32u}) {
        for (const auto& p : patterns) {
            const Result r = run_pattern(tracks, blocks, p.rows, p.steps, p.repeat);
            char shape[32];
            std::snprintf(shape, sizeof shape, "%u x %u x %u", p.rows, p.steps, p.repeat);
            std::printf("%8u %22s %14.2f %14.2f %14.2f %15.1fx\n", tracks, shape, r.us_per_block_avg, r.us_per_block_p99,
                        r.us_per_block_max, r.rt_ratio);
        }
    }

    std::printf("\nInput: armed + monitored audio tracks, injected 8-channel input (no clips)\n");
    std::printf("%8s %10s %14s %14s %14s %16s\n", "tracks", "recording", "avg us/block", "p99 us/block",
                "max us/block", "realtime ratio");
    for (bool record : {false, true}) {
        const Result r = run_input(8, blocks, record);
        std::printf("%8u %10s %14.2f %14.2f %14.2f %15.1fx\n", 8u, record ? "on" : "off", r.us_per_block_avg,
                    r.us_per_block_p99, r.us_per_block_max, r.rt_ratio);
    }
    return 0;
}
