// Sample conversion cost of the ALSA backend's audio thread
// (platform/pcm_convert.hpp): per period, the output interleave (engine
// float planar -> device format) and, in duplex, the input deinterleave
// (device format -> float planar). Everything else on that thread is the
// engine's render and the ALSA read/write calls.
//
//   dstudio_pcm_convert_bench [periods=20000]
#include "platform/pcm_convert.hpp"

#include <algorithm>
#include <chrono>
#include <cstdio>
#include <cstdlib>
#include <vector>

namespace {

using dstudio::pcm::Format;

// Average ns per period: interleave 2 engine channels into `dev_ch` device
// channels, then deinterleave `dev_ch` channels back.
double run(Format f, uint32_t frames, uint32_t dev_ch, uint32_t periods) {
    std::vector<std::vector<float>> planar(dev_ch, std::vector<float>(frames));
    for (uint32_t c = 0; c < dev_ch; ++c)
        for (uint32_t i = 0; i < frames; ++i) planar[c][i] = 0.001f * static_cast<float>((i * 7 + c * 13) % 997) - 0.5f;
    std::vector<const float*> src;
    std::vector<float*> dst;
    for (auto& p : planar) {
        src.push_back(p.data());
        dst.push_back(p.data());
    }
    std::vector<uint8_t> raw(static_cast<std::size_t>(frames) * dev_ch * dstudio::pcm::bytes_per_sample(f));
    double sink = 0.0;
    const auto t0 = std::chrono::steady_clock::now();
    for (uint32_t k = 0; k < periods; ++k) {
        dstudio::pcm::interleave(f, src.data(), 2, dev_ch, frames, raw.data());
        dstudio::pcm::deinterleave(f, raw.data(), dev_ch, frames, dst.data());
        sink += static_cast<double>(planar[0][k % frames]);
    }
    const auto t1 = std::chrono::steady_clock::now();
    if (sink > 1e30) std::printf("%f\n", sink); // keep the loop
    return std::chrono::duration<double, std::nano>(t1 - t0).count() / periods;
}

const char* name(Format f) {
    switch (f) {
    case Format::F32: return "float32";
    case Format::S32: return "s32";
    case Format::S24: return "s24";
    case Format::S24_3: return "s24_3";
    case Format::S16: return "s16";
    }
    return "?";
}

} // namespace

int main(int argc, char** argv) {
    const uint32_t periods = argc > 1 ? static_cast<uint32_t>(std::max(1, std::atoi(argv[1]))) : 20000;
    std::printf("dstudio_pcm_convert_bench: %u periods of 256 frames, ns per period (out + in, best of 3)\n", periods);
    std::printf("%-8s %10s %12s\n", "format", "channels", "ns/period");
    for (Format f : {Format::F32, Format::S32, Format::S24, Format::S24_3, Format::S16})
        for (uint32_t ch : {2u, 8u}) {
            double best = 1e30;
            for (int rep = 0; rep < 3; ++rep) best = std::min(best, run(f, 256, ch, periods));
            std::printf("%-8s %10u %12.1f\n", name(f), ch, best);
        }
    return 0;
}
