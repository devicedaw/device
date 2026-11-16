// Linux platform audio: dispatches between the PipeWire and ALSA backends.
//
// Why both: PipeWire is the desktop sound server of most current
// distributions (shared devices, per-app routing, Bluetooth), but it is a
// user-space service that not every system runs. ALSA is part of the
// kernel, so it is always there, and opening a hardware PCM directly gives
// the lowest latency there is, with nothing to configure. So:
//   - The system default output (empty id) uses PipeWire when its server is
//     reachable and ALSA ("default" PCM) otherwise. If PipeWire is up but
//     its stream does not start, the default falls back to ALSA too.
//   - The device list has the PipeWire nodes and the ALSA PCMs side by side
//     ("... (ALSA)"), so a hardware device can be picked directly even on a
//     PipeWire desktop (it is busy while PipeWire itself holds it open).
//   - Input and output always use one backend (linux_audio_select.hpp).
// DSTUDIO_AUDIO_BACKEND=alsa or =pipewire forces the backend of the system
// default.
#if defined(__linux__)

#include "audio_device.hpp"
#include "dstudio_engine.h"
#include "linux_audio.hpp"
#include "linux_audio_select.hpp"
#include "os.hpp"

#include <memory>
#include <string>
#include <vector>

namespace dstudio {
namespace {

using linux_audio::Backend;

linux_audio::Preference preference() {
    std::string v;
    return os::get_env("DSTUDIO_AUDIO_BACKEND", v) ? linux_audio::parse_preference(v) : linux_audio::Preference::Auto;
}

#if defined(DSTUDIO_HAVE_PIPEWIRE)
bool pipewire_up() { return linux_audio::pipewire_server_up(); }
std::unique_ptr<AudioDevice> make_pipewire() { return linux_audio::create_pipewire_device(); }
#else
bool pipewire_up() { return false; }
std::unique_ptr<AudioDevice> make_pipewire() { return nullptr; }
#endif

// The backend for `c`, or SE_ERR_INVALID_ARG when it mixes backends.
int32_t choose(const AudioDevice::Config& c, Backend& out, std::string& err) {
    const linux_audio::Preference pref = preference();
    // Only probe the server when the choice depends on it.
    const bool needs_probe = c.output_uid.empty() && pref == linux_audio::Preference::Auto;
    const linux_audio::Choice ch =
        linux_audio::choose_backend(c.output_uid, c.input_enabled, c.input_uid, needs_probe && pipewire_up(), pref);
    if (!ch.error.empty()) {
        err = ch.error;
        return SE_ERR_INVALID_ARG;
    }
    out = ch.backend;
    return SE_OK;
}

// AudioDevice that runs one of the two backends, picked per start().
class LinuxAudioDevice final : public AudioDevice {
public:
    ~LinuxAudioDevice() override { stop(); }

    int32_t validate(const Config& c, std::string& err) const override {
        Backend b{};
        int32_t rc = choose(c, b, err);
        if (rc != SE_OK) return rc;
        AudioDevice* d = backend(b);
        if (d == nullptr) {
            err = "this build has no PipeWire support; pick an ALSA device";
            return SE_ERR_NOT_FOUND;
        }
        rc = d->validate(c, err);
        // The system default falls back to ALSA when PipeWire cannot serve it.
        if (rc != SE_OK && b == Backend::PipeWire && is_plain_default(c)) {
            std::string alsa_err;
            if (backend(Backend::Alsa)->validate(c, alsa_err) == SE_OK) {
                err.clear();
                return SE_OK;
            }
        }
        return rc;
    }

    int32_t start(const Config& c, Callback* callback, std::string& err) override {
        if (active_ != nullptr) {
            err = "device already started";
            return SE_ERR_STATE;
        }
        Backend b{};
        int32_t rc = choose(c, b, err);
        if (rc != SE_OK) return rc;
        AudioDevice* d = backend(b);
        if (d == nullptr) {
            err = "this build has no PipeWire support; pick an ALSA device";
            return SE_ERR_NOT_FOUND;
        }
        rc = d->start(c, callback, err);
        if (rc != SE_OK && b == Backend::PipeWire && is_plain_default(c)) {
            // PipeWire answered but could not play: the ALSA default still can.
            std::string alsa_err;
            AudioDevice* a = backend(Backend::Alsa);
            if (a->start(c, callback, alsa_err) == SE_OK) {
                err.clear();
                active_ = a;
                return SE_OK;
            }
            err += "; ALSA: " + alsa_err;
        }
        if (rc == SE_OK) active_ = d;
        return rc;
    }

    int32_t stop() override {
        if (active_ == nullptr) return SE_OK;
        const int32_t rc = active_->stop();
        last_ = active_;
        active_ = nullptr;
        return rc;
    }

    bool is_running() const override { return active_ != nullptr && active_->is_running(); }

    Status status() const override {
        if (active_ != nullptr) return active_->status();
        return last_ != nullptr ? last_->status() : Status{};
    }

private:
    // Default output, and default input or none: the case that may fall back.
    static bool is_plain_default(const Config& c) {
        return c.output_uid.empty() && (!c.input_enabled || c.input_uid.empty()) &&
               preference() == linux_audio::Preference::Auto;
    }

    // The backend's device, created on first use. Null for PipeWire in a
    // build without it.
    AudioDevice* backend(Backend b) const {
        if (b == Backend::Alsa) {
            if (!alsa_) alsa_ = linux_audio::create_alsa_device();
            return alsa_.get();
        }
        if (!pipewire_) pipewire_ = make_pipewire();
        return pipewire_.get();
    }

    mutable std::unique_ptr<AudioDevice> pipewire_, alsa_;
    AudioDevice* active_ = nullptr;
    AudioDevice* last_ = nullptr; // status() after stop()
};

} // namespace

// Creates the Linux audio device (PipeWire or ALSA, chosen per start).
std::unique_ptr<AudioDevice> create_platform_audio_device() { return std::make_unique<LinuxAudioDevice>(); }

// PipeWire nodes (when a server is reachable) followed by the ALSA PCMs. The
// ALSA default is marked as the default device when PipeWire is not serving
// the system default.
int32_t list_audio_devices(std::vector<DeviceInfo>& out, std::string&) {
    out.clear();
    bool pw = false;
#if defined(DSTUDIO_HAVE_PIPEWIRE)
    double rate = 0.0;
    pw = linux_audio::pipewire_list(out, rate);
#endif
    const linux_audio::Preference pref = preference();
    const bool alsa_default = pref == linux_audio::Preference::Alsa || (!pw && pref == linux_audio::Preference::Auto);
    if (alsa_default)
        for (DeviceInfo& d : out) d.is_default_input = d.is_default_output = false;
    linux_audio::alsa_list(out, alsa_default);
    return SE_OK;
}

// The PipeWire graph rate, or 0 (the engine then uses 48 kHz) without a server.
double query_default_output_sample_rate() {
#if defined(DSTUDIO_HAVE_PIPEWIRE)
    if (preference() != linux_audio::Preference::Alsa) {
        std::vector<DeviceInfo> devs;
        double rate = 0.0;
        if (linux_audio::pipewire_list(devs, rate)) return rate;
    }
#endif
    return 0.0;
}

} // namespace dstudio

#endif // __linux__
