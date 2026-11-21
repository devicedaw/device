// Platform audio-device abstraction. All platform-specific code lives in
// engine/src/platform/; the rest of the engine only sees this interface.
//
// Exactly one backend translation unit provides create_platform_audio_device(),
// list_audio_devices() and query_default_output_sample_rate():
//   coreaudio_device.cpp  macOS  (AUHAL; private aggregate device for duplex)
//   windows_audio_device.cpp  Windows (WASAPI shared mode, wasapi_device.cpp, or an ASIO
//                             driver, asio_device.cpp, picked per setup)
//   linux_audio_device.cpp    Linux  (PipeWire, pipewire_device.cpp, or ALSA, alsa_device.cpp)
//   audio_device.cpp      fallback for any other platform
#pragma once

#include <cstdint>
#include <memory>
#include <string>
#include <vector>

namespace dstudio {

// Most device input channels handed to the engine (more are ignored).
inline constexpr uint32_t kMaxDeviceInputChannels = 64;

// One audio device as list_audio_devices() reports it. `uid` is the stable
// id passed back in Config::output_uid / input_uid.
struct DeviceInfo {
    std::string uid;
    std::string name;
    uint32_t input_channels = 0;
    uint32_t output_channels = 0;
    std::vector<double> sample_rates; // supported standard nominal rates, ascending
    bool is_default_input = false;
    bool is_default_output = false;
};

// One platform audio stream (output plus optional input) driving a Callback.
class AudioDevice {
public:
    // Implemented by the engine. Called on the device's real-time thread.
    class Callback {
    public:
        // `out` holds `num_out` non-interleaved float32 buffers of `frames`
        // frames each; all must be filled. `in` holds `num_in` (0 when no
        // input is open or it failed this cycle) input channels of the same
        // cycle and length. Must be RT safe.
        virtual void render(float* const* out, uint32_t num_out, const float* const* in, uint32_t num_in,
                            uint32_t frames) noexcept = 0;

    protected:
        ~Callback() = default;
    };

    // What to open and at which rates/sizes.
    struct Config {
        double sample_rate = 48000.0; // engine rate
        uint32_t channels = 2;
        uint32_t preferred_block = 256;
        // Device setup (se_device_configure). Defaults = the historical
        // behaviour: system default output, no input, rate/buffer untouched.
        std::string output_uid;     // "" = system default output
        bool input_enabled = false; // false: never open an input
        std::string input_uid;      // "" = system default input
        double device_rate = 0.0;   // 0 = leave (output only) / engine rate (with input)
        uint32_t buffer_size = 0;   // 0 = leave
    };

    // What is actually in effect after start().
    struct Status {
        std::string output_name;
        std::string input_name; // empty when no input is open
        std::string input_error;
        bool input_open = false;
        double device_rate = 0.0;
        uint32_t buffer_size = 0;
        uint32_t input_channels = 0;
        uint32_t output_channels = 0;
        uint32_t input_latency = 0;  // device frames
        uint32_t output_latency = 0; // device frames
    };

    virtual ~AudioDevice() = default;

    // Checks a configuration without opening anything (no input is touched).
    virtual int32_t validate(const Config& config, std::string& err) const = 0;
    // Opens the configured devices and starts the callback. Returns an
    // se_result code; `err` gets a description on failure. If the output
    // opens but the requested input does not, returns SE_OK with
    // status().input_open == false and status().input_error set.
    virtual int32_t start(const Config& config, Callback* callback, std::string& err) = 0;
    // Stops the stream. When this returns the callback is no longer running
    // and will not be called again.
    virtual int32_t stop() = 0;
    // True while the stream is running (false after stop() or a device loss).
    virtual bool is_running() const = 0;
    // Valid after a successful start (and kept after stop).
    virtual Status status() const = 0;
};

// Creates this platform's audio device (not yet started).
std::unique_ptr<AudioDevice> create_platform_audio_device();

// Enumerates devices (reads properties only; never opens a device).
int32_t list_audio_devices(std::vector<DeviceInfo>& out, std::string& err);

// Nominal sample rate of the default output device, or 0 if unknown.
double query_default_output_sample_rate();

// Standard nominal rates reported by list_audio_devices.
inline constexpr double kStandardRates[] = {8000,  11025, 16000, 22050, 32000,  44100,  48000, 64000,
                                            88200, 96000, 128000, 176400, 192000, 352800, 384000, 768000};

} // namespace dstudio
