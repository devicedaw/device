// WAV import benchmark: decode_wav_memory and decode_audio_file on a 60 s stereo 24-bit file
// written as plain RIFF, RF64 and BW64 (ds64 sizes, data size 0xFFFFFFFF). Prints measurements
// only; no pass/fail thresholds. Also times decode_audio_memory on the FLAC, Ogg Vorbis and MP3
// test fixtures (0.1 s each, tests/decode_fixtures.hpp), 600 decodes per run (60 s of audio), to
// catch a regression when the third-party decoders are updated.
//
// usage: dstudio_decode_bench [reps=20]
#include "platform/os.hpp"
#include "dstudio_engine.h"
#include "wav.hpp"
#include "../tests/decode_fixtures.hpp"

#include <algorithm>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <filesystem>
#include <numbers>
#include <string>
#include <vector>

namespace {

void put32(std::vector<uint8_t>& b, uint32_t v) {
    for (int i = 0; i < 4; ++i) b.push_back(static_cast<uint8_t>(v >> (8 * i)));
}
void put64(std::vector<uint8_t>& b, uint64_t v) {
    for (int i = 0; i < 8; ++i) b.push_back(static_cast<uint8_t>(v >> (8 * i)));
}
void tag(std::vector<uint8_t>& b, const char* t) { b.insert(b.end(), t, t + 4); }

std::vector<uint8_t> make(const char* kind, uint32_t frames) {
    const bool wide = std::strcmp(kind, "RIFF") != 0;
    const uint64_t data = static_cast<uint64_t>(frames) * 6;
    std::vector<uint8_t> b;
    tag(b, kind);
    put32(b, wide ? 0xFFFFFFFFu : static_cast<uint32_t>(4 + 24 + 8 + data));
    tag(b, "WAVE");
    if (wide) {
        tag(b, "ds64");
        put32(b, 28);
        put64(b, 4 + 36 + 24 + 8 + data);
        put64(b, data);
        put64(b, frames);
        put32(b, 0);
    }
    tag(b, "fmt ");
    put32(b, 16);
    b.push_back(1); b.push_back(0); b.push_back(2); b.push_back(0);
    put32(b, 48000);
    put32(b, 48000 * 6);
    b.push_back(6); b.push_back(0); b.push_back(24); b.push_back(0);
    tag(b, "data");
    put32(b, wide ? 0xFFFFFFFFu : static_cast<uint32_t>(data));
    for (uint32_t f = 0; f < frames; ++f) {
        const double t = f / 48000.0;
        for (double hz : {220.0, 330.0}) {
            const int32_t s = static_cast<int32_t>(std::lround(std::sin(2.0 * std::numbers::pi * hz * t) * 0.5 * 8388607.0));
            b.push_back(static_cast<uint8_t>(s));
            b.push_back(static_cast<uint8_t>(s >> 8));
            b.push_back(static_cast<uint8_t>(s >> 16));
        }
    }
    return b;
}

double median(std::vector<double> v) {
    std::sort(v.begin(), v.end());
    return v[v.size() / 2];
}

} // namespace

int main(int argc, char** argv) {
    const int reps = argc > 1 ? std::max(1, std::atoi(argv[1])) : 20;
    const uint32_t frames = 60 * 48000;
    std::printf("WAV import, 60 s stereo 24-bit 48 kHz (%.1f MB), median of %d runs\n\n", frames * 6 / 1e6, reps);
    std::printf("| container | decode_wav_memory ms | decode_audio_file ms (read + decode) |\n|---|---:|---:|\n");
    for (const char* kind : {"RIFF", "RF64", "BW64"}) {
        const std::vector<uint8_t> bytes = make(kind, frames);
        const auto path = std::filesystem::temp_directory_path() / (std::string("dstudio_decode_bench_") + kind + ".wav");
        const std::string p8 = dstudio::os::path_to_utf8(path);
        if (std::FILE* f = dstudio::os::fopen_utf8(p8.c_str(), "wb")) {
            std::fwrite(bytes.data(), 1, bytes.size(), f);
            std::fclose(f);
        }
        std::vector<double> mem, file;
        bool ok = true;
        for (int i = 0; i < reps; ++i) {
            dstudio::DecodedAudio d;
            std::string err;
            auto t0 = std::chrono::steady_clock::now();
            ok &= dstudio::decode_wav_memory(bytes.data(), bytes.size(), d, err) == SE_OK && d.frames == frames;
            mem.push_back(std::chrono::duration<double, std::milli>(std::chrono::steady_clock::now() - t0).count());
            t0 = std::chrono::steady_clock::now();
            ok &= dstudio::decode_audio_file(p8.c_str(), d, err) == SE_OK && d.frames == frames;
            file.push_back(std::chrono::duration<double, std::milli>(std::chrono::steady_clock::now() - t0).count());
        }
        std::filesystem::remove(path);
        if (!ok) {
            std::printf("| %s | unsupported | unsupported |\n", kind);
            continue;
        }
        std::printf("| %s | %.2f | %.2f |\n", kind, median(mem), median(file));
    }

    // Compressed import: the same 0.1 s fixture decoded 600 times per run.
    struct Fixture {
        const char* name;
        const uint8_t* data;
        std::size_t size;
    };
    const Fixture compressed[] = {
        {"FLAC", fixtures::kToneFlac, sizeof fixtures::kToneFlac},
        {"Ogg Vorbis", fixtures::kToneOgg, sizeof fixtures::kToneOgg},
        {"MP3", fixtures::kToneMp3, sizeof fixtures::kToneMp3},
    };
    constexpr int kDecodes = 600;
    std::printf("\nCompressed import, 0.1 s stereo 48 kHz fixture x %d (60 s of audio), median of %d runs\n\n", kDecodes, reps);
    std::printf("| format | decode_audio_memory ms per 60 s |\n|---|---:|\n");
    for (const Fixture& fx : compressed) {
        std::vector<double> runs;
        bool ok = true;
        for (int i = 0; i < reps; ++i) {
            const auto t0 = std::chrono::steady_clock::now();
            for (int k = 0; k < kDecodes; ++k) {
                dstudio::DecodedAudio d;
                std::string err;
                ok &= dstudio::decode_audio_memory(fx.data, fx.size, d, err) == SE_OK && d.frames > 0;
            }
            runs.push_back(std::chrono::duration<double, std::milli>(std::chrono::steady_clock::now() - t0).count());
        }
        if (!ok) {
            std::printf("| %s | failed |\n", fx.name);
            continue;
        }
        std::printf("| %s | %.2f |\n", fx.name, median(runs));
    }
    return 0;
}
