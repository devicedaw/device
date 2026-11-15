// The analyser of a built-in device's expanded window: the level spectrum of
// the device's input (Pre) or output (Post), measured on the audio thread only
// while a window asks for it.
//
// Every hop (a quarter of the frame) the last frame of the mono sum is
// Hann-windowed, transformed (dsp::RealFft) and reduced to kBins log-spaced
// bins from 20 Hz to 20 kHz: bin i is centred on 20 Hz * 1000^(i / (kBins-1))
// and takes the strongest FFT bin within half a bin spacing of it (the power
// interpolated at its centre when that is narrower than one FFT bin). Levels
// are dBFS: a full-scale sine reads 0 dB. The frame (kBins floats, floor kFloorDb) goes out through an
// AnalysisChannel; the window smooths and draws it.
#pragma once

#include "builtin.hpp"
#include "spectral.hpp"

#include <algorithm>
#include <array>
#include <cmath>
#include <vector>

namespace dstudio {

class WindowSpectrum {
public:
    static constexpr uint32_t kBins = 128;
    static constexpr float kFloorDb = -90.0f;

    // Non-RT. A 4096-point frame at 48 kHz (the frame follows the rate so the
    // resolution in Hz stays the same).
    explicit WindowSpectrum(double sample_rate) : out_(kBins) {
        const double sr = sample_rate > 0.0 ? sample_rate : 48000.0;
        const uint32_t n =
            dsp::pow2_at_least(static_cast<uint32_t>(std::max(1.0, std::round(sr / 48000.0))) * 4096u);
        fft_.prepare(n);
        hop_ = n / 4;
        ring_.assign(n, 0.0f);
        frame_.assign(n, 0.0f);
        re_.assign(fft_.bins(), 0.0f);
        im_.assign(fft_.bins(), 0.0f);
        window_.resize(n);
        double sum = 0.0;
        for (uint32_t i = 0; i < n; ++i) {
            window_[i] = static_cast<float>(0.5 - 0.5 * std::cos(2.0 * dsp::kPiD * i / n));
            sum += static_cast<double>(window_[i]);
        }
        const double ref = sum / 2.0; // a full-scale sine peaks at |X| = sum(w) / 2
        norm_ = static_cast<float>(1.0 / (ref * ref));
        const double bin_hz = sr / n;
        const double step = std::pow(1000.0, 1.0 / (kBins - 1));
        const uint32_t nyq = fft_.bins() - 1;
        for (uint32_t b = 0; b < kBins; ++b) {
            const double hz = 20.0 * std::pow(step, static_cast<double>(b));
            auto first = static_cast<uint32_t>(std::ceil(hz / std::sqrt(step) / bin_hz));
            auto last = static_cast<uint32_t>(std::floor(hz * std::sqrt(step) / bin_hz));
            // Narrower than an FFT bin: the power interpolated at the bin's centre.
            at_[b] = last < first ? static_cast<float>(std::min(hz / bin_hz, nyq - 1.0)) : -1.0f;
            first_[b] = std::min(first, nyq);
            last_[b] = std::min(last, nyq);
        }
    }

    // RT: feeds the mono sum of a stereo block (r may be null for mono).
    void push(const float* l, const float* r, uint32_t n) noexcept {
        if (l == nullptr) return;
        const auto size = static_cast<uint32_t>(ring_.size());
        for (uint32_t i = 0; i < n; ++i) {
            ring_[pos_] = r != nullptr ? 0.5f * (l[i] + r[i]) : l[i];
            pos_ = pos_ + 1 == size ? 0 : pos_ + 1;
            if (++fill_ == hop_) {
                fill_ = 0;
                analyse();
            }
        }
    }

    // Main thread: the newest frame (kBins floats); seq as AnalysisChannel::read.
    uint32_t read(float* out, uint32_t cap, uint64_t& seq) noexcept { return out_.read(out, cap, seq); }

private:
    void analyse() noexcept {
        const auto size = static_cast<uint32_t>(ring_.size());
        for (uint32_t i = 0; i < size; ++i) {
            const uint32_t j = pos_ + i < size ? pos_ + i : pos_ + i - size;
            frame_[i] = ring_[j] * window_[i];
        }
        fft_.forward(frame_.data(), re_.data(), im_.data());
        float* const o = out_.begin();
        for (uint32_t b = 0; b < kBins; ++b) {
            float p = 0.0f;
            if (at_[b] >= 0.0f) {
                const auto k = static_cast<uint32_t>(at_[b]);
                const float t = at_[b] - static_cast<float>(k);
                const float p0 = re_[k] * re_[k] + im_[k] * im_[k];
                const float p1 = re_[k + 1] * re_[k + 1] + im_[k + 1] * im_[k + 1];
                p = p0 + t * (p1 - p0);
            } else {
                for (uint32_t k = first_[b]; k <= last_[b]; ++k) p = std::max(p, re_[k] * re_[k] + im_[k] * im_[k]);
            }
            const float v = p * norm_;
            o[b] = v > 1e-12f ? std::max(kFloorDb, 10.0f * std::log10(v)) : kFloorDb;
        }
        out_.publish(kBins);
    }

    dsp::RealFft fft_;
    AnalysisChannel out_;
    uint32_t hop_ = 1024, pos_ = 0, fill_ = 0;
    float norm_ = 1.0f;
    std::vector<float> ring_, frame_, re_, im_, window_;
    std::array<uint32_t, kBins> first_{}, last_{};
    std::array<float, kBins> at_{};
};

} // namespace dstudio
