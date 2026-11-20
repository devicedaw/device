// Windows platform audio: dispatches between the WASAPI and ASIO backends.
//
// WASAPI serves every Windows audio endpoint and the system default. ASIO
// drivers are listed beside the endpoints ("<driver> (ASIO)", id
// "asio:<CLSID>"); picking one as output runs the stream through that driver,
// with its own inputs for recording (windows_audio.hpp, choose_backend).
#if defined(_WIN32)

#include "audio_device.hpp"
#include "dstudio_engine.h"
#include "windows_audio.hpp"

#include <memory>
#include <string>
#include <vector>

namespace dstudio {
namespace {

using windows_audio::Backend;

// AudioDevice that runs one of the two backends, picked per validate/start.
class WindowsAudioDevice final : public AudioDevice {
public:
    ~WindowsAudioDevice() override { stop(); }

    int32_t validate(const Config& c, std::string& err) const override {
        AudioDevice* d = nullptr;
        const int32_t rc = pick(c, d, err);
        return rc != SE_OK ? rc : d->validate(c, err);
    }

    int32_t start(const Config& c, Callback* callback, std::string& err) override {
        if (active_ != nullptr) {
            err = "device already started";
            return SE_ERR_STATE;
        }
        AudioDevice* d = nullptr;
        int32_t rc = pick(c, d, err);
        if (rc != SE_OK) return rc;
        rc = d->start(c, callback, err);
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
    // The backend device for `c`, created on first use.
    int32_t pick(const Config& c, AudioDevice*& out, std::string& err) const {
        const windows_audio::Choice ch = windows_audio::choose_backend(c.output_uid, c.input_enabled, c.input_uid);
        if (!ch.error.empty()) {
            err = ch.error;
            return SE_ERR_INVALID_ARG;
        }
        if (ch.backend == Backend::Asio) {
            if (!asio_) asio_ = windows_audio::create_asio_device();
            if (!asio_) {
                err = "this build has no ASIO support";
                return SE_ERR_NOT_FOUND;
            }
            out = asio_.get();
        } else {
            if (!wasapi_) wasapi_ = windows_audio::create_wasapi_device();
            out = wasapi_.get();
        }
        return SE_OK;
    }

    mutable std::unique_ptr<AudioDevice> wasapi_, asio_;
    AudioDevice* active_ = nullptr;
    AudioDevice* last_ = nullptr; // status() after stop()
};

} // namespace

// Creates the Windows audio device (WASAPI or ASIO, chosen per start).
std::unique_ptr<AudioDevice> create_platform_audio_device() { return std::make_unique<WindowsAudioDevice>(); }

// The WASAPI endpoints, then the ASIO drivers.
int32_t list_audio_devices(std::vector<DeviceInfo>& out, std::string& err) {
    const int32_t rc = windows_audio::wasapi_list(out, err);
    if (rc != SE_OK) return rc;
    windows_audio::asio_list(out);
    return SE_OK;
}

// The default render endpoint's shared-mode rate (the system default is WASAPI).
double query_default_output_sample_rate() { return windows_audio::wasapi_default_output_rate(); }

} // namespace dstudio

#endif // _WIN32
