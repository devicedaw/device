// Which Windows backend serves a device setup (platform/windows_audio.hpp).
// Runs on every OS, without audio hardware.
#include "test_harness.hpp"

#include "platform/windows_audio.hpp"

using dstudio::windows_audio::Backend;
using dstudio::windows_audio::choose_backend;

namespace {
constexpr const char* kAsio = "asio:{835F8B5B-F66F-4E5B-9FAF-8A4E6C3C9B1A}";
constexpr const char* kOtherAsio = "asio:{11111111-2222-3333-4444-555555555555}";
constexpr const char* kEndpoint = "{0.0.0.00000000}.{02cee85c-375f-42f0-b678-9f484aa601a2}";
} // namespace

TEST_CASE(windows_audio_asio_output_records_from_the_same_driver) {
    CHECK(choose_backend(kAsio, false, "").backend == Backend::Asio);
    CHECK(choose_backend(kAsio, true, "").error.empty()); // default input: the driver's inputs
    CHECK(choose_backend(kAsio, true, kAsio).error.empty());
    CHECK(!choose_backend(kAsio, true, kOtherAsio).error.empty());
    CHECK(!choose_backend(kAsio, true, kEndpoint).error.empty());
}

TEST_CASE(windows_audio_endpoints_stay_on_wasapi) {
    CHECK(choose_backend("", false, "").backend == Backend::Wasapi);
    CHECK(choose_backend(kEndpoint, true, "").error.empty());
    CHECK(!choose_backend(kEndpoint, true, kAsio).error.empty()); // ASIO input needs ASIO output
    CHECK(!choose_backend("", true, kAsio).error.empty());
}
