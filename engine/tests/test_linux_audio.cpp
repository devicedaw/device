// Linux audio: backend choice and ALSA ids (platform/linux_audio_select.hpp)
// and the hardware sample conversion (platform/pcm_convert.hpp) run on every
// OS. On Linux the ALSA backend itself runs against ALSA's "null" PCM, which
// alsa-lib always defines, so no sound card or sound server is needed.
#include "test_harness.hpp"
#include "test_util.hpp"

#include "platform/linux_audio_select.hpp"
#include "platform/pcm_convert.hpp"

#include <algorithm>
#include <chrono>
#include <cmath>
#include <string>
#include <thread>
#include <vector>

namespace la = dstudio::linux_audio;
using testutil::EngineHandle;

TEST_CASE(linux_audio_default_output_prefers_pipewire_then_alsa) {
    // System default: PipeWire when its server answers, else ALSA.
    CHECK(la::choose_backend("", false, "", true, la::Preference::Auto).backend == la::Backend::PipeWire);
    CHECK(la::choose_backend("", false, "", false, la::Preference::Auto).backend == la::Backend::Alsa);
    // DSTUDIO_AUDIO_BACKEND overrides the default either way.
    CHECK(la::choose_backend("", false, "", true, la::Preference::Alsa).backend == la::Backend::Alsa);
    CHECK(la::choose_backend("", false, "", false, la::Preference::PipeWire).backend == la::Backend::PipeWire);
    CHECK(la::parse_preference("ALSA") == la::Preference::Alsa);
    CHECK(la::parse_preference("pipewire") == la::Preference::PipeWire);
    CHECK(la::parse_preference("jack") == la::Preference::Auto);
    // A picked device decides by its id, whatever the server state.
    CHECK(la::choose_backend("alsa:hw:CARD=PCH,DEV=0", false, "", true, la::Preference::Auto).backend == la::Backend::Alsa);
    CHECK(la::choose_backend("alsa_output.pci.analog-stereo", false, "", false, la::Preference::Alsa).backend ==
          la::Backend::PipeWire);
    // The default output follows an ALSA input.
    const la::Choice c = la::choose_backend("", true, "alsa:hw:CARD=USB,DEV=0", true, la::Preference::Auto);
    CHECK(c.backend == la::Backend::Alsa);
    CHECK(c.error.empty());
}

TEST_CASE(linux_audio_input_and_output_must_share_a_backend) {
    CHECK(!la::choose_backend("alsa:default", true, "alsa_input.usb", false, la::Preference::Auto).error.empty());
    CHECK(!la::choose_backend("alsa_output.usb", true, "alsa:default", true, la::Preference::Auto).error.empty());
    // A default input goes with either output.
    CHECK(la::choose_backend("alsa:default", true, "", true, la::Preference::Auto).error.empty());
    CHECK(la::choose_backend("alsa_output.usb", true, "", true, la::Preference::Auto).error.empty());
    // Output-only ignores a stale input id.
    CHECK(la::choose_backend("alsa:default", false, "alsa_input.usb", true, la::Preference::Auto).error.empty());
}

TEST_CASE(linux_audio_alsa_ids_map_to_pcm_names) {
    const std::string uid = la::alsa_hw_uid("PCH", 3);
    CHECK(uid == "alsa:hw:CARD=PCH,DEV=3");
    CHECK(la::is_alsa_uid(uid));
    CHECK(!la::is_alsa_uid("alsa_output.pci-0000_00_1f.3.analog-stereo")); // a PipeWire node name
    CHECK(la::alsa_pcm_name(uid) == "hw:CARD=PCH,DEV=3");
    CHECK(la::alsa_pcm_name("") == "default");
    CHECK(la::alsa_pcm_name("alsa:default") == "default");
    CHECK(la::alsa_pcm_name("alsa:pipewire") == "pipewire");
    // Conversion fallbacks: plughw for hardware, plug: for plain names, none
    // for PCMs that already convert.
    CHECK(la::plug_pcm_name("hw:CARD=PCH,DEV=3") == "plughw:CARD=PCH,DEV=3");
    CHECK(la::plug_pcm_name("dmix") == "plug:dmix");
    CHECK(la::plug_pcm_name("default").empty());
    CHECK(la::plug_pcm_name("plughw:0,0").empty());
    CHECK(la::alsa_hw_name("HDA Intel PCH", "ALC892 Analog") == "HDA Intel PCH: ALC892 Analog (ALSA)");
    CHECK(la::alsa_hw_name("USB Audio", "USB Audio") == "USB Audio (ALSA)");
}

TEST_CASE(pcm_convert_round_trips_every_format) {
    using dstudio::pcm::Format;
    const std::vector<float> l = {0.0f, 0.5f, -0.5f, 0.999f, -1.0f, 2.0f, -3.0f, std::nanf("")};
    const std::vector<float> r = {0.25f, -0.25f, 0.125f, -0.75f, 1.0f, 0.0f, 0.1f, -0.1f};
    const auto n = static_cast<uint32_t>(l.size());
    const float* src[2] = {l.data(), r.data()};
    for (Format f : {Format::F32, Format::S32, Format::S24, Format::S24_3, Format::S16}) {
        // 4 device channels: channels 2 and 3 must come out silent.
        std::vector<uint8_t> raw(static_cast<std::size_t>(n) * 4 * dstudio::pcm::bytes_per_sample(f), 0xAB);
        dstudio::pcm::interleave(f, src, 2, 4, n, raw.data());
        std::vector<std::vector<float>> back(4, std::vector<float>(n, 9.0f));
        float* dst[4] = {back[0].data(), back[1].data(), back[2].data(), back[3].data()};
        dstudio::pcm::deinterleave(f, raw.data(), 4, n, dst);
        const float tol = f == Format::S16 ? 1.0f / 16384.0f : (f == Format::F32 ? 0.0f : 1.0f / 4000000.0f);
        for (uint32_t i = 0; i < n; ++i) {
            const float want_l = f == Format::F32 ? l[i] : (std::isnan(l[i]) ? 0.0f : std::clamp(l[i], -1.0f, 1.0f));
            if (f == Format::F32 && std::isnan(l[i])) {
                CHECK(std::isnan(back[0][i]));
            } else {
                CHECK(std::fabs(back[0][i] - want_l) <= tol);
            }
            CHECK(std::fabs(back[1][i] - r[i]) <= tol);
            CHECK_EQ(back[2][i], 0.0f);
            CHECK_EQ(back[3][i], 0.0f);
        }
    }
}

#if defined(__linux__) && defined(DSTUDIO_HAVE_ALSA)

namespace {

// Starts the engine on `uid` and waits until at least `cycles` callbacks ran.
bool runs(se_engine* e, uint64_t cycles) {
    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(3);
    se_stats s{};
    while (std::chrono::steady_clock::now() < deadline) {
        se_stats_get(e, &s);
        if (s.callbacks >= cycles) return true;
        std::this_thread::sleep_for(std::chrono::milliseconds(5));
    }
    return false;
}

} // namespace

TEST_CASE(alsa_device_runs_the_engine_with_the_chosen_buffer_size) {
    EngineHandle e(48000, 256);
    se_device_config dc{};
    dc.output_uid = "alsa:null";
    dc.buffer_size = 128;
    if (se_device_configure(e, &dc) != SE_OK) SKIP("libasound is not installed");
    REQUIRE(se_device_start(e) == SE_OK);
    CHECK(runs(e, 20));
    CHECK_EQ(se_device_is_running(e), 1);
    se_device_status st{};
    REQUIRE(se_device_status_get(e, &st) == SE_OK);
    CHECK_EQ(st.running, 1);
    CHECK_EQ(st.buffer_size, 128u);
    CHECK_EQ(st.output_channels, 2u);
    CHECK(std::fabs(st.sample_rate - 48000.0) < 0.5);
    CHECK(std::string(st.output_name).find("ALSA") != std::string::npos);
    REQUIRE(se_device_stop(e) == SE_OK);
    CHECK_EQ(se_device_is_running(e), 0);
    // A restart on the same device object works (the thread and PCMs are gone).
    dc.buffer_size = 512;
    REQUIRE(se_device_configure(e, &dc) == SE_OK);
    REQUIRE(se_device_start(e) == SE_OK);
    CHECK(runs(e, 30));
    se_device_status_get(e, &st);
    CHECK_EQ(st.buffer_size, 512u);
    REQUIRE(se_device_stop(e) == SE_OK);
}

TEST_CASE(alsa_device_duplex_opens_the_input) {
    EngineHandle e(48000, 256);
    se_device_config dc{};
    dc.output_uid = "alsa:null";
    dc.input_uid = "alsa:null";
    dc.input_enabled = 1;
    if (se_device_configure(e, &dc) != SE_OK) SKIP("libasound is not installed");
    REQUIRE(se_device_start(e) == SE_OK);
    CHECK(runs(e, 20));
    se_device_status st{};
    REQUIRE(se_device_status_get(e, &st) == SE_OK);
    CHECK_EQ(st.input_open, 1);
    CHECK(std::string(st.input_error).empty());
    CHECK(st.input_channels >= 1u);
    REQUIRE(se_device_stop(e) == SE_OK);
}

TEST_CASE(alsa_device_rejects_mixed_backends_and_bad_buffers) {
    EngineHandle e(48000, 256);
    se_device_config dc{};
    dc.output_uid = "alsa:null";
    dc.input_uid = "some_pipewire_source";
    dc.input_enabled = 1;
    CHECK_EQ(se_device_configure(e, &dc), SE_ERR_INVALID_ARG);
    dc.input_enabled = 0;
    dc.buffer_size = 8;
    CHECK_EQ(se_device_configure(e, &dc), SE_ERR_INVALID_ARG);
    dc.buffer_size = 0;
    dc.output_uid = "alsa:hw:CARD=NoSuchCard,DEV=0";
    CHECK_EQ(se_device_configure(e, &dc), SE_ERR_NOT_FOUND);
}

TEST_CASE(alsa_devices_are_listed) {
    size_t n = 0;
    REQUIRE(se_device_list(nullptr, 0, &n) == SE_OK);
    std::vector<se_device_info> devs(n);
    REQUIRE(se_device_list(devs.data(), devs.size(), &n) == SE_OK);
    bool alsa_default = false;
    for (const se_device_info& d : devs) alsa_default = alsa_default || std::string(d.uid) == "alsa:default";
    CHECK(alsa_default);
}

#endif
