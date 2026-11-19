// Shared-mode period choice for the WASAPI backend (wasapi_device.cpp).
// Plain arithmetic, no Windows headers, so it is unit-tested on every OS
// (tests/test_wasapi_period.cpp).
#pragma once

#include <cstdint>

namespace dstudio {

// What IAudioClient3::GetSharedModeEnginePeriod reports for a format, in
// endpoint frames.
struct SharedPeriods {
    uint32_t default_frames = 0;     // the period of a plain IAudioClient stream (usually 10 ms)
    uint32_t fundamental_frames = 0; // supported periods are multiples of this...
    uint32_t min_frames = 0;         // ...between min and max
    uint32_t max_frames = 0;
};

// The small shared-mode period to ask IAudioClient3 for, given the buffer
// size the user picked (`requested`, endpoint frames; 0 = device default).
// Rounds up to the next supported period. 0 means "open a plain stream":
// nothing smaller than the default period was asked for, or the endpoint
// cannot run a smaller one.
inline uint32_t small_shared_period(uint32_t requested, const SharedPeriods& p) {
    if (requested == 0 || p.fundamental_frames == 0 || p.min_frames == 0 || p.min_frames > p.max_frames) return 0;
    if (requested >= p.default_frames) return 0;
    // Supported periods are multiples of the fundamental period in [min, max].
    uint32_t f = requested;
    const uint32_t over = f % p.fundamental_frames;
    if (over != 0) f += p.fundamental_frames - over;
    if (f < p.min_frames) f = p.min_frames;
    if (f > p.max_frames) f = p.max_frames;
    return f < p.default_frames ? f : 0;
}

} // namespace dstudio
