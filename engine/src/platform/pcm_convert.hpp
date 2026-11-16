// Sample conversion between the engine's planar float32 buffers and the
// interleaved integer or float frames of a hardware PCM (alsa_device.cpp).
// Pure, allocation-free and RT safe; tested and benchmarked on every
// platform.
#pragma once

#include <algorithm>
#include <bit>
#include <cmath>
#include <cstdint>
#include <cstring>

namespace dstudio::pcm {

// Interleaved little-endian sample formats a hardware PCM may need.
enum class Format : uint8_t {
    F32,   // float32
    S32,   // int32, full scale
    S24,   // int32 container, 24 significant low bits
    S24_3, // packed 3-byte int24
    S16,   // int16
};

inline constexpr uint32_t bytes_per_sample(Format f) noexcept {
    switch (f) {
    case Format::F32:
    case Format::S32:
    case Format::S24: return 4;
    case Format::S24_3: return 3;
    case Format::S16: return 2;
    }
    return 4;
}

namespace detail {

// Scales to a signed integer of `bits` bits (rounded to nearest), clamped
// to its range; NaN becomes 0. Full scale is 2^(bits-1), as in from_int.
inline int32_t to_int(float x, int bits) noexcept {
    if (x != x) return 0;
    const double scale = static_cast<double>(int64_t{1} << (bits - 1));
    const double v = std::clamp(static_cast<double>(x) * scale, -scale, scale - 1.0);
    return static_cast<int32_t>(v < 0.0 ? v - 0.5 : v + 0.5);
}

inline float from_int(int32_t v, int bits) noexcept {
    return static_cast<float>(static_cast<double>(v) / static_cast<double>(int64_t{1} << (bits - 1)));
}

// Little-endian store/load of the low `Bytes` bytes (sign-extended on load).
// Fixed sizes so the copies compile to plain moves (a 3-byte memcpy does not).
template <uint32_t Bytes> inline void store_le(uint8_t* p, int32_t v) noexcept {
    if constexpr (Bytes == 4 && std::endian::native == std::endian::little) {
        std::memcpy(p, &v, 4);
    } else {
        const auto u = static_cast<uint32_t>(v);
        for (uint32_t b = 0; b < Bytes; ++b) p[b] = static_cast<uint8_t>(u >> (8 * b));
    }
}

template <uint32_t Bytes> inline int32_t load_le(const uint8_t* p) noexcept {
    uint32_t u = 0;
    if constexpr (Bytes == 4 && std::endian::native == std::endian::little) {
        std::memcpy(&u, p, 4);
    } else {
        for (uint32_t b = 0; b < Bytes; ++b) u |= static_cast<uint32_t>(p[b]) << (8 * b);
    }
    constexpr uint32_t shift = 32 - 8 * Bytes; // sign-extend
    return static_cast<int32_t>(u << shift) >> shift;
}

// Interleaves into an integer format of `Bytes` bytes per sample and `Bits`
// significant bits.
template <uint32_t Bytes, int Bits>
inline void interleave_int(const float* const* src, uint32_t used, uint32_t dev_channels, uint32_t frames,
                           uint8_t* dst) noexcept {
    for (uint32_t i = 0; i < frames; ++i) {
        uint8_t* frame = dst + static_cast<std::size_t>(i) * dev_channels * Bytes;
        for (uint32_t c = 0; c < used; ++c) store_le<Bytes>(frame + c * Bytes, to_int(src[c][i], Bits));
        std::memset(frame + used * Bytes, 0, static_cast<std::size_t>(dev_channels - used) * Bytes);
    }
}

// Deinterleaves `Bytes`-byte samples whose value is the low `LoadBytes`
// bytes, with `Bits` significant bits.
template <uint32_t Bytes, uint32_t LoadBytes, int Bits>
inline void deinterleave_int(const uint8_t* src, uint32_t dev_channels, uint32_t frames, float* const* dst) noexcept {
    for (uint32_t i = 0; i < frames; ++i) {
        const uint8_t* frame = src + static_cast<std::size_t>(i) * dev_channels * Bytes;
        for (uint32_t c = 0; c < dev_channels; ++c) dst[c][i] = from_int(load_le<LoadBytes>(frame + c * Bytes), Bits);
    }
}

} // namespace detail

// Writes `frames` interleaved frames of `dev_channels` channels to `dst`.
// Channel c < src_channels comes from src[c]; channels beyond are silent.
inline void interleave(Format f, const float* const* src, uint32_t src_channels, uint32_t dev_channels,
                       uint32_t frames, uint8_t* dst) noexcept {
    const uint32_t used = std::min(src_channels, dev_channels);
    if (f == Format::F32) {
        auto* out = reinterpret_cast<float*>(dst);
        for (uint32_t i = 0; i < frames; ++i) {
            float* frame = out + static_cast<std::size_t>(i) * dev_channels;
            for (uint32_t c = 0; c < used; ++c) frame[c] = src[c][i];
            for (uint32_t c = used; c < dev_channels; ++c) frame[c] = 0.0f;
        }
        return;
    }
    if (f == Format::S16) {
        auto* out = reinterpret_cast<int16_t*>(dst);
        for (uint32_t i = 0; i < frames; ++i) {
            int16_t* frame = out + static_cast<std::size_t>(i) * dev_channels;
            for (uint32_t c = 0; c < used; ++c) frame[c] = static_cast<int16_t>(detail::to_int(src[c][i], 16));
            for (uint32_t c = used; c < dev_channels; ++c) frame[c] = 0;
        }
        return;
    }
    switch (f) {
    case Format::S32: detail::interleave_int<4, 32>(src, used, dev_channels, frames, dst); break;
    case Format::S24: detail::interleave_int<4, 24>(src, used, dev_channels, frames, dst); break;
    case Format::S24_3: detail::interleave_int<3, 24>(src, used, dev_channels, frames, dst); break;
    default: break;
    }
}

// Reads `frames` interleaved frames of `dev_channels` channels from `src`
// into dst[0..dev_channels).
inline void deinterleave(Format f, const uint8_t* src, uint32_t dev_channels, uint32_t frames,
                         float* const* dst) noexcept {
    if (f == Format::F32) {
        const auto* in = reinterpret_cast<const float*>(src);
        for (uint32_t i = 0; i < frames; ++i)
            for (uint32_t c = 0; c < dev_channels; ++c) dst[c][i] = in[static_cast<std::size_t>(i) * dev_channels + c];
        return;
    }
    if (f == Format::S16) {
        const auto* in = reinterpret_cast<const int16_t*>(src);
        for (uint32_t i = 0; i < frames; ++i)
            for (uint32_t c = 0; c < dev_channels; ++c)
                dst[c][i] = detail::from_int(in[static_cast<std::size_t>(i) * dev_channels + c], 16);
        return;
    }
    switch (f) {
    case Format::S32: detail::deinterleave_int<4, 4, 32>(src, dev_channels, frames, dst); break;
    // S24 in a 4-byte container: the value is the low 24 bits, sign-extended.
    case Format::S24: detail::deinterleave_int<4, 3, 24>(src, dev_channels, frames, dst); break;
    case Format::S24_3: detail::deinterleave_int<3, 3, 24>(src, dev_channels, frames, dst); break;
    default: break;
    }
}

} // namespace dstudio::pcm
