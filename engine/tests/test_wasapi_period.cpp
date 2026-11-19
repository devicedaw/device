// small_shared_period: which IAudioClient3 period the WASAPI backend asks for
// (platform/wasapi_period.hpp). Runs on every OS, without audio hardware.
#include "test_harness.hpp"

#include "platform/wasapi_period.hpp"

using dstudio::SharedPeriods;
using dstudio::small_shared_period;

namespace {
// The inbox Windows HD Audio driver at 48 kHz: 10 ms default, 128..480 in steps of 32.
constexpr SharedPeriods kHda{480, 32, 128, 480};
// An endpoint that only runs its default period (e.g. HDMI on many GPUs).
constexpr SharedPeriods kFixed{480, 480, 480, 480};
} // namespace

TEST_CASE(wasapi_period_rounds_up_to_a_supported_period) {
    CHECK_EQ(small_shared_period(128, kHda), 128u);
    CHECK_EQ(small_shared_period(256, kHda), 256u);
    CHECK_EQ(small_shared_period(200, kHda), 224u); // next multiple of 32
    CHECK_EQ(small_shared_period(64, kHda), 128u);  // below the minimum: the minimum
}

TEST_CASE(wasapi_period_falls_back_to_a_plain_stream) {
    CHECK_EQ(small_shared_period(0, kHda), 0u);    // device default asked for
    CHECK_EQ(small_shared_period(480, kHda), 0u);  // not smaller than the default
    CHECK_EQ(small_shared_period(1024, kHda), 0u); // larger buffers use the plain path
    CHECK_EQ(small_shared_period(470, kHda), 0u);  // rounds up to the default itself
    CHECK_EQ(small_shared_period(128, kFixed), 0u);
    CHECK_EQ(small_shared_period(128, SharedPeriods{}), 0u); // nothing reported
}
