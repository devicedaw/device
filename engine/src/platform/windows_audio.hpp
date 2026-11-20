// Windows audio backends behind the platform interface (audio_device.hpp).
// windows_audio_device.cpp implements create_platform_audio_device() and the
// enumeration functions on top of these two; choose_backend() below decides
// which one serves a configuration.
//
//   wasapi_device.cpp  WASAPI shared mode: every Windows audio endpoint.
//   asio_device.cpp    ASIO drivers (only when built with DSTUDIO_HAVE_ASIO).
//
// The selection logic is plain string handling, so it is declared outside
// the _WIN32 guard and unit-tested on every OS (tests/test_windows_audio.cpp).
#pragma once

#include "audio_device.hpp"

#include <memory>
#include <string>
#include <string_view>
#include <vector>

namespace dstudio::windows_audio {

// Device ids of ASIO drivers start with this; the rest is the driver's CLSID.
inline constexpr std::string_view kAsioPrefix = "asio:";

inline bool is_asio_uid(std::string_view uid) { return uid.substr(0, kAsioPrefix.size()) == kAsioPrefix; }

enum class Backend { Wasapi, Asio };

struct Choice {
    Backend backend = Backend::Wasapi;
    std::string error; // non-empty: the configuration mixes backends
};

// The backend for an output/input pair. An ASIO driver plays and records
// through one driver, so its input is the same driver: an empty input id
// ("default input") then means that driver's inputs. Mixing an ASIO driver
// with a Windows endpoint is an error.
inline Choice choose_backend(std::string_view output_uid, bool input_enabled, std::string_view input_uid) {
    const bool out_asio = is_asio_uid(output_uid);
    const bool in_asio = input_enabled && is_asio_uid(input_uid);
    if (out_asio) {
        if (input_enabled && !input_uid.empty() && input_uid != output_uid)
            return {Backend::Asio, "an ASIO driver records from its own inputs: pick the same driver as input"};
        return {Backend::Asio, {}};
    }
    if (in_asio) return {Backend::Wasapi, "an ASIO driver's inputs need the same driver as output"};
    return {Backend::Wasapi, {}};
}

#if defined(_WIN32)
// ---- WASAPI (wasapi_device.cpp).
std::unique_ptr<AudioDevice> create_wasapi_device();
int32_t wasapi_list(std::vector<DeviceInfo>& out, std::string& err);
double wasapi_default_output_rate();

// ---- ASIO (asio_device.cpp). Without DSTUDIO_HAVE_ASIO these report no
// drivers and create nothing.
std::unique_ptr<AudioDevice> create_asio_device();
// Appends one device per installed ASIO driver that loads ("<name> (ASIO)").
void asio_list(std::vector<DeviceInfo>& out);
#endif

} // namespace dstudio::windows_audio
