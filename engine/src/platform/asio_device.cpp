// Windows ASIO backend: plays and records through an installed ASIO driver.
//
// Drivers are found in the registry (HKLM\SOFTWARE\ASIO\<name>, value
// CLSID) and loaded in-process with CoCreateInstance; the IASIO interface
// and its types come from Steinberg's ASIO SDK (fetched by CMake, used
// under its GPLv3 option, see third_party_licenses.md). Only the SDK
// headers are used.
//
// Stream: the first `channels` outputs (2) and, with an input, every input
// up to kMaxDeviceInputChannels, all at the engine rate (ASIO does no rate
// conversion: a driver that cannot run at it is refused). The driver calls
// bufferSwitch on its own thread; we convert its buffers to planar float,
// call the engine and convert back (asio_convert.hpp), then call
// outputReady when the driver supports it.
//
// One ASIO stream per process: the driver callbacks carry no context, so the
// running device is a process-wide pointer. A driver's reset request (its
// control panel changed the rate or buffer, for instance) ends the stream:
// is_running() turns false, as for a removed WASAPI device.
//
// Device list: one entry per driver that loads, with its channel counts and
// the standard rates it accepts. Probing loads the driver briefly, so the
// result is cached for the process (never re-probed while it streams).
#if defined(_WIN32)

#include "audio_device.hpp"
#include "windows_audio.hpp"

#if !defined(DSTUDIO_HAVE_ASIO)

namespace dstudio {
std::unique_ptr<AudioDevice> windows_audio::create_asio_device() { return nullptr; }
void windows_audio::asio_list(std::vector<DeviceInfo>&) {}
} // namespace dstudio

#else

#include "asio_convert.hpp"
#include "os.hpp"
#include "dstudio_engine.h"

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>
// After windows.h:
#include <objbase.h>

// The SDK headers are third-party C: no warnings from them.
#pragma warning(push, 0)
#include "iasiodrv.h"
#pragma warning(pop)

#include <algorithm>
#include <atomic>
#include <cmath>
#include <cstdio>
#include <map>
#include <mutex>
#include <string>
#include <thread>
#include <vector>

namespace dstudio {
namespace {

using asio_convert::SampleFormat;

constexpr uint32_t kMaxOutputs = 16;

SampleFormat format_of(ASIOSampleType t) {
    switch (t) {
    case ASIOSTInt16LSB: return SampleFormat::Int16;
    case ASIOSTInt24LSB: return SampleFormat::Int24;
    case ASIOSTInt32LSB: return SampleFormat::Int32;
    case ASIOSTFloat32LSB: return SampleFormat::Float32;
    case ASIOSTFloat64LSB: return SampleFormat::Float64;
    case ASIOSTInt32LSB16: return SampleFormat::Int32Align16;
    case ASIOSTInt32LSB18: return SampleFormat::Int32Align18;
    case ASIOSTInt32LSB20: return SampleFormat::Int32Align20;
    case ASIOSTInt32LSB24: return SampleFormat::Int32Align24;
    default: return SampleFormat::Unsupported;
    }
}

// One registered driver.
struct DriverEntry {
    std::string name;  // registry key name
    std::string clsid; // "{...}"
};

// Installed drivers from HKLM\SOFTWARE\ASIO (64-bit view).
std::vector<DriverEntry> registered_drivers() {
    std::vector<DriverEntry> out;
    HKEY root = nullptr;
    if (RegOpenKeyExW(HKEY_LOCAL_MACHINE, L"SOFTWARE\\ASIO", 0, KEY_READ | KEY_WOW64_64KEY, &root) != ERROR_SUCCESS)
        return out;
    for (DWORD i = 0;; ++i) {
        wchar_t key[256];
        DWORD key_len = 256;
        if (RegEnumKeyExW(root, i, key, &key_len, nullptr, nullptr, nullptr, nullptr) != ERROR_SUCCESS) break;
        wchar_t clsid[64];
        DWORD bytes = sizeof clsid;
        if (RegGetValueW(root, key, L"CLSID", RRF_RT_REG_SZ, nullptr, clsid, &bytes) != ERROR_SUCCESS) continue;
        out.push_back({os::narrow(key), os::narrow(clsid)});
    }
    RegCloseKey(root);
    return out;
}

// COM for the calling thread (ASIO drivers expect an apartment-threaded
// caller; an existing MTA is used as it is).
class ComScope {
public:
    ComScope() {
        const HRESULT hr = CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);
        owned_ = SUCCEEDED(hr);
        ok_ = owned_ || hr == RPC_E_CHANGED_MODE;
    }
    ~ComScope() {
        if (owned_) CoUninitialize();
    }
    ComScope(const ComScope&) = delete;
    ComScope& operator=(const ComScope&) = delete;
    bool ok() const noexcept { return ok_; }
    // Keeps COM initialised (when destroyed on another thread than it was made on).
    void abandon() noexcept { owned_ = false; }

private:
    bool owned_ = false;
    bool ok_ = false;
};

// A loaded and initialised driver; released on destruction.
class Driver {
public:
    Driver() = default;
    ~Driver() { reset(); }
    Driver(const Driver&) = delete;
    Driver& operator=(const Driver&) = delete;

    // Loads the driver with this CLSID and calls init(). False with `err`.
    bool load(const std::string& clsid, std::string& err) {
        reset();
        CLSID id{};
        if (FAILED(CLSIDFromString(os::widen(clsid).c_str(), &id))) {
            err = "not an ASIO driver id: " + clsid;
            return false;
        }
        void* p = nullptr;
        const HRESULT hr = CoCreateInstance(id, nullptr, CLSCTX_INPROC_SERVER, id, &p);
        if (FAILED(hr) || p == nullptr) {
            char buf[16];
            std::snprintf(buf, sizeof buf, "0x%08lX", static_cast<unsigned long>(hr));
            err = std::string("the ASIO driver could not be loaded (HRESULT ") + buf + ")";
            return false;
        }
        iasio_ = static_cast<IASIO*>(p);
        if (iasio_->init(GetDesktopWindow()) != ASIOTrue) {
            char msg[128] = {};
            iasio_->getErrorMessage(msg);
            err = std::string("the ASIO driver did not start") + (msg[0] ? std::string(": ") + msg : std::string());
            reset();
            return false;
        }
        return true;
    }

    void reset() {
        if (iasio_ != nullptr) iasio_->Release();
        iasio_ = nullptr;
    }

    IASIO* operator->() const { return iasio_; }
    explicit operator bool() const { return iasio_ != nullptr; }

private:
    IASIO* iasio_ = nullptr;
};

// What a driver reports, for the device list and validation.
struct Probe {
    uint32_t inputs = 0, outputs = 0;
    std::vector<double> rates;
    long min_buf = 0, max_buf = 0, pref_buf = 0, gran = 0;
};

bool probe(IASIO* d, Probe& p) {
    long in = 0, out = 0;
    if (d->getChannels(&in, &out) != ASE_OK) return false;
    p.inputs = static_cast<uint32_t>(std::max(0L, in));
    p.outputs = static_cast<uint32_t>(std::max(0L, out));
    p.rates.clear();
    for (const double r : kStandardRates)
        if (d->canSampleRate(r) == ASE_OK) p.rates.push_back(r);
    if (d->getBufferSize(&p.min_buf, &p.max_buf, &p.pref_buf, &p.gran) != ASE_OK) return false;
    return true;
}

// Process-wide cache of driver probes, by CLSID.
std::mutex g_probe_mutex;
std::map<std::string, Probe> g_probes;

bool cached_probe(const std::string& clsid, Probe& p) {
    std::lock_guard lock(g_probe_mutex);
    const auto it = g_probes.find(clsid);
    if (it == g_probes.end()) return false;
    p = it->second;
    return true;
}

void store_probe(const std::string& clsid, const Probe& p) {
    std::lock_guard lock(g_probe_mutex);
    g_probes[clsid] = p;
}

std::string clsid_of(const std::string& uid) { return uid.substr(windows_audio::kAsioPrefix.size()); }

// The registered name of a driver, or its CLSID.
std::string driver_name(const std::string& clsid) {
    for (const DriverEntry& e : registered_drivers())
        if (_stricmp(e.clsid.c_str(), clsid.c_str()) == 0) return e.name;
    return clsid;
}

class AsioDevice;
std::atomic<AsioDevice*> g_active{nullptr};

class AsioDevice final : public AudioDevice {
public:
    ~AsioDevice() override { stop(); }

    // Loads the driver (or uses the probe of the running one) and checks
    // channels, rate and buffer size.
    int32_t validate(const Config& c, std::string& err) const override {
        if (!std::isfinite(c.device_rate) || c.device_rate < 0.0) {
            err = "sample_rate must be finite and >= 0";
            return SE_ERR_INVALID_ARG;
        }
        if (c.buffer_size != 0 && (c.buffer_size < 16 || c.buffer_size > 4096)) {
            err = "buffer_size must be 0 or 16..4096";
            return SE_ERR_INVALID_ARG;
        }
        const std::string clsid = clsid_of(c.output_uid);
        Probe p;
        if (!cached_probe(clsid, p)) {
            ComScope com;
            Driver d;
            if (!com.ok() || !d.load(clsid, err)) return SE_ERR_NOT_FOUND;
            if (!probe(d.operator->(), p)) {
                err = "the ASIO driver reports no channels";
                return SE_ERR_DEVICE;
            }
            store_probe(clsid, p);
        }
        const std::string name = driver_name(clsid);
        if (p.outputs < c.channels) {
            err = name + " has fewer than " + std::to_string(c.channels) + " outputs";
            return SE_ERR_INVALID_ARG;
        }
        if (c.input_enabled && p.inputs == 0) {
            err = name + " has no inputs";
            return SE_ERR_INVALID_ARG;
        }
        const auto runs_at = [&](double r) {
            return std::any_of(p.rates.begin(), p.rates.end(), [&](double x) { return std::fabs(x - r) < 0.5; });
        };
        // ASIO does no rate conversion: the driver must run at the engine rate.
        if (!runs_at(c.sample_rate) ||
            (c.device_rate > 0.0 && std::fabs(c.device_rate - c.sample_rate) >= 0.5)) {
            err = name + " cannot run at the session rate (" + std::to_string(std::lround(c.sample_rate)) +
                  " Hz); ASIO does not convert rates";
            return SE_ERR_INVALID_ARG;
        }
        return SE_OK;
    }

    int32_t start(const Config& c, Callback* callback, std::string& err) override {
        if (running_.load()) {
            err = "device already started";
            return SE_ERR_STATE;
        }
        if (callback == nullptr || c.channels == 0 || c.channels > kMaxOutputs) {
            err = "invalid device configuration";
            return SE_ERR_INVALID_ARG;
        }
        AsioDevice* expected = nullptr;
        if (!g_active.compare_exchange_strong(expected, this)) {
            err = "another ASIO stream is already running in this process";
            return SE_ERR_STATE;
        }
        const int32_t rc = open(c, callback, err);
        if (rc != SE_OK) {
            close();
            return rc;
        }
        return SE_OK;
    }

    int32_t stop() override {
        close();
        return SE_OK;
    }

    bool is_running() const override { return running_.load() && !reset_requested_.load(); }
    Status status() const override { return status_; }

    // ---- driver callbacks (driver thread)
    void on_buffer_switch(long index) noexcept {
        if (!running_.load(std::memory_order_acquire) || index < 0 || index > 1) return;
        const uint32_t n = buffer_frames_;
        for (uint32_t ch = 0; ch < n_in_; ++ch)
            asio_convert::to_float(infos_[n_out_ + ch].buffers[index], in_ptrs_[ch], n, in_fmt_[ch]);
        callback_->render(out_ptrs_, n_out_, n_in_ > 0 ? const_in_ptrs_ : nullptr, n_in_, n);
        for (uint32_t ch = 0; ch < n_out_; ++ch)
            asio_convert::from_float(out_ptrs_[ch], infos_[ch].buffers[index], n, out_fmt_[ch]);
        if (post_output_) driver_->outputReady();
    }

    void on_reset_request() noexcept { reset_requested_.store(true); }

private:
    int32_t open(const Config& c, Callback* callback, std::string& err) {
        com_ = std::make_unique<ComScope>();
        com_thread_ = std::this_thread::get_id();
        int32_t rc = validate(c, err);
        if (rc != SE_OK) return rc;
        const std::string clsid = clsid_of(c.output_uid);
        if (!com_->ok() || !driver_.load(clsid, err)) return SE_ERR_DEVICE;
        Probe p;
        if (!probe(driver_.operator->(), p)) {
            err = "the ASIO driver reports no channels";
            return SE_ERR_DEVICE;
        }
        store_probe(clsid, p);

        ASIOSampleRate cur = 0.0;
        if (driver_->getSampleRate(&cur) != ASE_OK || std::fabs(cur - c.sample_rate) >= 0.5) {
            if (driver_->setSampleRate(c.sample_rate) != ASE_OK) {
                err = "the ASIO driver did not switch to " + std::to_string(std::lround(c.sample_rate)) + " Hz";
                return SE_ERR_DEVICE;
            }
        }
        // Buffer sizes can depend on the rate (often a fixed duration): ask again.
        driver_->getBufferSize(&p.min_buf, &p.max_buf, &p.pref_buf, &p.gran);

        n_out_ = c.channels;
        n_in_ = c.input_enabled ? std::min<uint32_t>(p.inputs, kMaxDeviceInputChannels) : 0;
        buffer_frames_ = asio_convert::choose_buffer_size(c.buffer_size, p.min_buf, p.max_buf, p.pref_buf, p.gran);
        if (buffer_frames_ == 0) {
            err = "the ASIO driver reports no usable buffer size";
            return SE_ERR_DEVICE;
        }

        // Channel formats. An output we cannot write fails the stream; an
        // input we cannot read leaves the stream output-only.
        std::string input_error;
        for (uint32_t ch = 0; ch < n_out_ + n_in_; ++ch) {
            ASIOChannelInfo ci{};
            ci.channel = static_cast<long>(ch < n_out_ ? ch : ch - n_out_);
            ci.isInput = ch < n_out_ ? ASIOFalse : ASIOTrue;
            const SampleFormat f =
                driver_->getChannelInfo(&ci) == ASE_OK ? format_of(ci.type) : SampleFormat::Unsupported;
            if (ch < n_out_) {
                if (f == SampleFormat::Unsupported) {
                    err = "the ASIO driver's output sample format is not supported";
                    return SE_ERR_DEVICE;
                }
                out_fmt_[ch] = f;
            } else if (f == SampleFormat::Unsupported) {
                input_error = "the ASIO driver's input sample format is not supported";
                n_in_ = 0;
                break;
            } else {
                in_fmt_[ch - n_out_] = f;
            }
        }

        infos_.assign(n_out_ + n_in_, ASIOBufferInfo{});
        for (uint32_t ch = 0; ch < n_out_ + n_in_; ++ch) {
            infos_[ch].isInput = ch < n_out_ ? ASIOFalse : ASIOTrue;
            infos_[ch].channelNum = static_cast<long>(ch < n_out_ ? ch : ch - n_out_);
        }
        callbacks_.bufferSwitch = &cb_buffer_switch;
        callbacks_.sampleRateDidChange = &cb_rate_changed;
        callbacks_.asioMessage = &cb_message;
        callbacks_.bufferSwitchTimeInfo = &cb_buffer_switch_time_info;
        if (driver_->createBuffers(infos_.data(), static_cast<long>(infos_.size()), static_cast<long>(buffer_frames_),
                                   &callbacks_) != ASE_OK) {
            char msg[128] = {};
            driver_->getErrorMessage(msg);
            err = std::string("the ASIO driver could not create its buffers") +
                  (msg[0] ? std::string(": ") + msg : std::string());
            return SE_ERR_DEVICE;
        }
        buffers_created_ = true;

        // Planar engine buffers (allocated here, before the stream starts).
        planar_.assign(static_cast<std::size_t>(n_out_ + n_in_) * buffer_frames_, 0.0f);
        for (uint32_t ch = 0; ch < n_out_; ++ch)
            out_ptrs_[ch] = planar_.data() + static_cast<std::size_t>(ch) * buffer_frames_;
        for (uint32_t ch = 0; ch < n_in_; ++ch) {
            in_ptrs_[ch] = planar_.data() + static_cast<std::size_t>(n_out_ + ch) * buffer_frames_;
            const_in_ptrs_[ch] = in_ptrs_[ch];
        }
        callback_ = callback;
        post_output_ = driver_->outputReady() == ASE_OK;

        long in_lat = 0, out_lat = 0;
        driver_->getLatencies(&in_lat, &out_lat);
        const std::string name = driver_name(clsid);
        status_ = Status{};
        status_.output_name = name + " (ASIO)";
        status_.device_rate = c.sample_rate;
        status_.buffer_size = buffer_frames_;
        status_.output_channels = n_out_;
        status_.output_latency = static_cast<uint32_t>(std::max(0L, out_lat));
        status_.input_error = c.input_enabled ? input_error : std::string();
        if (n_in_ > 0) {
            status_.input_open = true;
            status_.input_name = status_.output_name;
            status_.input_channels = n_in_;
            status_.input_latency = static_cast<uint32_t>(std::max(0L, in_lat));
        }

        reset_requested_.store(false);
        running_.store(true, std::memory_order_release);
        if (driver_->start() != ASE_OK) {
            running_.store(false);
            err = "the ASIO driver did not start streaming";
            return SE_ERR_DEVICE;
        }
        started_ = true;
        return SE_OK;
    }

    // Stops and releases everything; safe to call twice.
    void close() {
        running_.store(false, std::memory_order_release);
        if (driver_) {
            if (started_) driver_->stop();
            if (buffers_created_) driver_->disposeBuffers();
        }
        started_ = false;
        buffers_created_ = false;
        driver_.reset();
        AsioDevice* self = this;
        g_active.compare_exchange_strong(self, nullptr);
        // COM is balanced only on the thread that initialised it.
        if (com_ && com_thread_ != std::this_thread::get_id()) com_->abandon(); // leaks one init, never misbalances
        com_.reset();
    }

    static void cb_buffer_switch(long index, ASIOBool) {
        if (AsioDevice* d = g_active.load(std::memory_order_acquire)) d->on_buffer_switch(index);
    }
    static ASIOTime* cb_buffer_switch_time_info(ASIOTime* t, long index, ASIOBool) {
        cb_buffer_switch(index, ASIOTrue);
        return t;
    }
    static void cb_rate_changed(ASIOSampleRate) {
        if (AsioDevice* d = g_active.load()) d->on_reset_request();
    }
    static long cb_message(long selector, long value, void*, double*) {
        switch (selector) {
        case kAsioSelectorSupported:
            return value == kAsioEngineVersion || value == kAsioResetRequest || value == kAsioResyncRequest ||
                           value == kAsioLatenciesChanged || value == kAsioSupportsTimeInfo
                       ? 1L
                       : 0L;
        case kAsioEngineVersion: return 2L;
        case kAsioResetRequest:
            if (AsioDevice* d = g_active.load()) d->on_reset_request();
            return 1L;
        case kAsioResyncRequest:
        case kAsioLatenciesChanged:
        case kAsioSupportsTimeInfo: return 1L;
        default: return 0L;
        }
    }

    Driver driver_;
    std::unique_ptr<ComScope> com_;
    std::thread::id com_thread_;
    ASIOCallbacks callbacks_{};
    std::vector<ASIOBufferInfo> infos_; // outputs first, then inputs
    std::vector<float> planar_;
    float* out_ptrs_[kMaxOutputs] = {};
    float* in_ptrs_[kMaxDeviceInputChannels] = {};
    const float* const_in_ptrs_[kMaxDeviceInputChannels] = {};
    SampleFormat out_fmt_[kMaxOutputs] = {};
    SampleFormat in_fmt_[kMaxDeviceInputChannels] = {};
    uint32_t n_out_ = 0, n_in_ = 0, buffer_frames_ = 0;
    Callback* callback_ = nullptr;
    bool post_output_ = false;
    bool buffers_created_ = false;
    bool started_ = false;
    std::atomic<bool> running_{false};
    std::atomic<bool> reset_requested_{false};
    Status status_;
};

} // namespace

std::unique_ptr<AudioDevice> windows_audio::create_asio_device() { return std::make_unique<AsioDevice>(); }

void windows_audio::asio_list(std::vector<DeviceInfo>& out) {
    const std::vector<DriverEntry> drivers = registered_drivers();
    if (drivers.empty()) return;
    ComScope com;
    for (const DriverEntry& e : drivers) {
        Probe p;
        if (!cached_probe(e.clsid, p)) {
            // Never load a second instance of the driver that is streaming.
            if (g_active.load() != nullptr || !com.ok()) continue;
            Driver d;
            std::string err;
            if (!d.load(e.clsid, err) || !probe(d.operator->(), p)) continue;
            store_probe(e.clsid, p);
        }
        DeviceInfo info;
        info.uid = std::string(kAsioPrefix) + e.clsid;
        info.name = e.name + " (ASIO)";
        info.input_channels = p.inputs;
        info.output_channels = p.outputs;
        info.sample_rates = p.rates;
        out.push_back(std::move(info));
    }
}

} // namespace dstudio

#endif // DSTUDIO_HAVE_ASIO
#endif // _WIN32
