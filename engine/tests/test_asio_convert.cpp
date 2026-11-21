// ASIO buffer conversion and buffer size choice (platform/asio_convert.hpp).
// Runs on every OS, without an ASIO driver.
#include "test_harness.hpp"

#include "platform/asio_convert.hpp"

#include <cmath>
#include <cstdint>
#include <cstring>
#include <vector>

using namespace dstudio::asio_convert;

namespace {
// Writes `in` in format f and reads it back.
std::vector<float> round_trip(const std::vector<float>& in, SampleFormat f) {
    std::vector<uint8_t> raw(in.size() * bytes_per_sample(f));
    from_float(in.data(), raw.data(), static_cast<uint32_t>(in.size()), f);
    std::vector<float> out(in.size());
    to_float(raw.data(), out.data(), static_cast<uint32_t>(in.size()), f);
    return out;
}
} // namespace

TEST_CASE(asio_formats_round_trip_within_their_resolution) {
    const std::vector<float> v{0.0f, 0.5f, -0.5f, 0.25f, -1.0f, 0.999f};
    const SampleFormat formats[] = {SampleFormat::Int16,        SampleFormat::Int24,        SampleFormat::Int32,
                                    SampleFormat::Float32,      SampleFormat::Float64,      SampleFormat::Int32Align16,
                                    SampleFormat::Int32Align20, SampleFormat::Int32Align24};
    for (const SampleFormat f : formats) {
        const std::vector<float> r = round_trip(v, f);
        for (std::size_t i = 0; i < v.size(); ++i) CHECK(std::fabs(r[i] - v[i]) < 1.0f / 16384.0f);
    }
}

TEST_CASE(asio_int_formats_clip_and_keep_their_layout) {
    // Full scale clips to the largest code instead of wrapping.
    const float over[] = {1.5f, -1.5f};
    int16_t s16[2];
    from_float(over, s16, 2, SampleFormat::Int16);
    CHECK_EQ(s16[0], 32767);
    CHECK_EQ(s16[1], -32768);
    // Packed 24-bit is little-endian, 3 bytes per sample.
    const float half = 0.5f;
    uint8_t s24[3];
    from_float(&half, s24, 1, SampleFormat::Int24);
    CHECK(s24[0] == 0x00 && s24[1] == 0x00 && s24[2] == 0x40);
    // A right-aligned 24-bit value in a 32-bit container.
    int32_t a24 = 0;
    from_float(&half, &a24, 1, SampleFormat::Int32Align24);
    CHECK_EQ(a24, 0x400000);
}

TEST_CASE(asio_buffer_size_follows_the_driver_granularity) {
    CHECK_EQ(choose_buffer_size(0, 64, 2048, 512, -1), 512u);  // preferred
    CHECK_EQ(choose_buffer_size(100, 64, 2048, 512, -1), 128u); // next power of two
    CHECK_EQ(choose_buffer_size(16, 64, 2048, 512, -1), 64u);   // below the minimum
    CHECK_EQ(choose_buffer_size(4096, 64, 2048, 512, -1), 2048u);
    CHECK_EQ(choose_buffer_size(100, 32, 1024, 256, 32), 128u); // steps of 32
    CHECK_EQ(choose_buffer_size(128, 256, 256, 256, 0), 256u);  // fixed size
}
