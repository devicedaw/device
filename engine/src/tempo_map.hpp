// Tempo map (se_session_publish_v12, specs/015-tempo-changes): musical time on
// the audio thread for the metronome and for the tempo and song position
// devices and plugins see. Clips, notes and automation arrive in frames
// already; nothing else converts.
//
// Segment i starts at frame f0 and quarter q0 with tempo a BPM and runs to the
// next segment's start quarter q1, where a ramp reaches b (a hold keeps b = a;
// the last segment holds). With R the sample rate and k = (b - a) / (q1 - q0):
//   hold: q(f) = q0 + (f - f0) * a / (60 R),   f(q) = f0 + (q - q0) * 60 R / a
//   ramp: f(q) = f0 + (60 R / k) * ln(1 + k (q - q0) / a)
//         q(f) = q0 + (a / k) * (exp(k (f - f0) / (60 R)) - 1)
//         tempo(f) = a + k (q(f) - q0)
// Before frame 0 the first segment's tempo holds. The core evaluates the same
// formula (crates/dstudio-core/src/tempo.rs). Everything here is noexcept,
// allocation-free and O(log segments).
#pragma once

#include "dstudio_engine.h"

#include <cmath>
#include <cstddef>

namespace dstudio {

class TempoMap {
public:
    TempoMap() noexcept = default;
    TempoMap(const se_tempo_segment* segments, std::size_t count, double sample_rate) noexcept
        : s_(segments), n_(count), rate_(sample_rate) {}

    bool empty() const noexcept { return n_ == 0; }

    // Quarter notes from bar 1 at timeline frame f.
    double quarter_at(double f) const noexcept {
        const std::size_t i = by_frame(f);
        const se_tempo_segment& g = s_[i];
        const double dt = (f - g.start_frame) / (60.0 * rate_);
        const double k = slope(i, f);
        if (k == 0.0) return g.start_quarter + dt * g.start_bpm;
        return g.start_quarter + g.start_bpm / k * std::expm1(k * dt);
    }

    // Timeline frame (unrounded) of quarter q.
    double frame_at(double q) const noexcept {
        const std::size_t i = by_quarter(q);
        const se_tempo_segment& g = s_[i];
        const double x = q - g.start_quarter;
        const double k = q < 0.0 ? 0.0 : slope_of(i);
        if (k == 0.0) return g.start_frame + x * 60.0 * rate_ / g.start_bpm;
        return g.start_frame + 60.0 * rate_ / k * std::log1p(k * x / g.start_bpm);
    }

    // Tempo in BPM at timeline frame f.
    double bpm_at(double f) const noexcept {
        const std::size_t i = by_frame(f);
        const se_tempo_segment& g = s_[i];
        const double k = slope(i, f);
        if (k == 0.0) return g.start_bpm;
        return g.start_bpm * std::exp(k * (f - g.start_frame) / (60.0 * rate_));
    }

private:
    // BPM per quarter of segment i (0 for a hold or the last segment).
    double slope_of(std::size_t i) const noexcept {
        const se_tempo_segment& g = s_[i];
        if (i + 1 >= n_ || g.end_bpm == g.start_bpm) return 0.0;
        return (g.end_bpm - g.start_bpm) / (s_[i + 1].start_quarter - g.start_quarter);
    }
    // The same, holding before frame 0.
    double slope(std::size_t i, double f) const noexcept { return f < 0.0 ? 0.0 : slope_of(i); }

    // Last segment starting at or before f (the first one before frame 0).
    std::size_t by_frame(double f) const noexcept {
        std::size_t lo = 0, hi = n_;
        while (hi - lo > 1) {
            const std::size_t mid = lo + (hi - lo) / 2;
            if (s_[mid].start_frame <= f) lo = mid; else hi = mid;
        }
        return lo;
    }
    std::size_t by_quarter(double q) const noexcept {
        std::size_t lo = 0, hi = n_;
        while (hi - lo > 1) {
            const std::size_t mid = lo + (hi - lo) / 2;
            if (s_[mid].start_quarter <= q) lo = mid; else hi = mid;
        }
        return lo;
    }

    const se_tempo_segment* s_ = nullptr;
    std::size_t n_ = 0;
    double rate_ = 48000.0;
};

} // namespace dstudio
