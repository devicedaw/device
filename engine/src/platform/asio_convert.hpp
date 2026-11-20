// Sample conversion between ASIO driver buffers and the engine's float32.
// Plain arithmetic, no ASIO SDK or Windows headers, so it is unit-tested on
// every OS (tests/test_asio_convert.cpp). asio_device.cpp maps the driver's
// ASIOSampleType to SampleFormat. Little-endian formats only (Windows x64).
#pragma once

#include <cstdint>
#include <cstring>

namespace dstudio::asio_convert {

enum class SampleFormat {
    Unsupported,
    Int16,   // 16-bit
    Int24,   // packed 24-bit (3 bytes)
    Int32,   // 32-bit, full scale
    Float32, // IEEE float
    Float64, // IEEE double
    // 32-bit containers holding a smaller right-aligned value.
    Int32Align16,
    Int32Align18,
    Int32Align20,
    Int32Align24,
};

// Bytes per sample in the driver buffer; 0 for Unsupported.
inline uint32_t bytes_per_sample(SampleFormat f) {
    switch (f) {
    case SampleFormat::Int16: return 2;
    case SampleFormat::Int24: return 3;
    case SampleFormat::Float64: return 8;
    case SampleFormat::Unsupported: return 0;
    default: return 4;
    }
}

namespace detail {
// Value bits of an integer format.
inline uint32_t int_bits(SampleFormat f) {
    switch (f) {
    case SampleFormat::Int16:
    case SampleFormat::Int32Align16: return 16;
    case SampleFormat::Int32Align18: return 18;
    case SampleFormat::Int32Align20: return 20;
    case SampleFormat::Int24:
    case SampleFormat::Int32Align24: return 24;
    default: return 32;
    }
}

inline int32_t read_int(const uint8_t* p, SampleFormat f) {
    if (f == SampleFormat::Int16) {
        int16_t v;
        std::memcpy(&v, p, 2);
        return v;
    }
    if (f == SampleFormat::Int24) {
        const uint32_t u = static_cast<uint32_t>(p[0]) | (static_cast<uint32_t>(p[1]) << 8) |
                           (static_cast<uint32_t>(p[2]) << 16);
        return static_cast<int32_t>(u << 8) >> 8; // sign-extend
    }
    int32_t v;
    std::memcpy(&v, p, 4);
    return v;
}

inline void write_int(uint8_t* p, SampleFormat f, int32_t v) {
    if (f == SampleFormat::Int16) {
        const int16_t s = static_cast<int16_t>(v);
        std::memcpy(p, &s, 2);
    } else if (f == SampleFormat::Int24) {
        const uint32_t u = static_cast<uint32_t>(v);
        p[0] = static_cast<uint8_t>(u);
        p[1] = static_cast<uint8_t>(u >> 8);
        p[2] = static_cast<uint8_t>(u >> 16);
    } else {
        std::memcpy(p, &v, 4);
    }
}
} // namespace detail

// Driver buffer -> float32 in [-1, 1). Unsupported formats give silence.
inline void to_float(const void* src, float* dst, uint32_t frames, SampleFormat f) {
    const auto* p = static_cast<const uint8_t*>(src);
    if (f == SampleFormat::Float32) {
        std::memcpy(dst, p, static_cast<std::size_t>(frames) * 4);
        return;
    }
    if (f == SampleFormat::Float64) {
        for (uint32_t i = 0; i < frames; ++i) {
            double d;
            std::memcpy(&d, p + 8 * static_cast<std::size_t>(i), 8);
            dst[i] = static_cast<float>(d);
        }
        return;
    }
    if (f == SampleFormat::Unsupported) {
        std::memset(dst, 0, static_cast<std::size_t>(frames) * sizeof(float));
        return;
    }
    const uint32_t bytes = bytes_per_sample(f);
    const double scale = 1.0 / static_cast<double>(int64_t{1} << (detail::int_bits(f) - 1));
    for (uint32_t i = 0; i < frames; ++i)
        dst[i] = static_cast<float>(detail::read_int(p + static_cast<std::size_t>(i) * bytes, f) * scale);
}

// float32 -> driver buffer, clipped to full scale. Unsupported formats are
// left untouched.
inline void from_float(const float* src, void* dst, uint32_t frames, SampleFormat f) {
    auto* p = static_cast<uint8_t*>(dst);
    if (f == SampleFormat::Float32) {
        std::memcpy(p, src, static_cast<std::size_t>(frames) * 4);
        return;
    }
    if (f == SampleFormat::Float64) {
        for (uint32_t i = 0; i < frames; ++i) {
            const double d = src[i];
            std::memcpy(p + 8 * static_cast<std::size_t>(i), &d, 8);
        }
        return;
    }
    if (f == SampleFormat::Unsupported) return;
    const uint32_t bytes = bytes_per_sample(f);
    const double full = static_cast<double>(int64_t{1} << (detail::int_bits(f) - 1));
    const double hi = full - 1.0;
    for (uint32_t i = 0; i < frames; ++i) {
        double v = static_cast<double>(src[i]) * full;
        v = v > hi ? hi : (v < -full ? -full : v);
        const auto n = static_cast<int32_t>(v >= 0.0 ? v + 0.5 : v - 0.5);
        detail::write_int(p + static_cast<std::size_t>(i) * bytes, f, n);
    }
}

// The buffer size to create, from what ASIOGetBufferSize reports and the
// size picked (`requested`, 0 = the driver's preferred size). Granularity:
// -1 = powers of two, 0 = only `preferred` (min == max), n > 0 = steps of n
// from min. Rounds up to the next allowed size, within [min, max].
inline uint32_t choose_buffer_size(uint32_t requested, long min, long max, long preferred, long granularity) {
    if (min <= 0 || max < min) return preferred > 0 ? static_cast<uint32_t>(preferred) : 0;
    if (requested == 0 || granularity == 0) {
        const long p = preferred < min ? min : (preferred > max ? max : preferred);
        return static_cast<uint32_t>(p);
    }
    long r = static_cast<long>(requested);
    if (r <= min) return static_cast<uint32_t>(min);
    if (r >= max) return static_cast<uint32_t>(max);
    if (granularity < 0) {
        long p = 1;
        while (p < r) p *= 2;
        return static_cast<uint32_t>(p > max ? max : p);
    }
    const long over = (r - min) % granularity;
    if (over != 0) r += granularity - over;
    return static_cast<uint32_t>(r > max ? max : r);
}

} // namespace dstudio::asio_convert
