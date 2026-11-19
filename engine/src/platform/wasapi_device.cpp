// Windows WASAPI backend (shared mode, event driven, float32).
//
// Streams
//   - Output: one render IAudioClient on the chosen endpoint (default: the
//     console default render endpoint). Shared mode with
//     AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUTOCONVERTPCM | SRC_DEFAULT_QUALITY:
//     we hand WASAPI float32 at the *engine* rate and the engine's channel
//     count; the audio engine converts to the endpoint's mix format (rate and
//     channel matrix), like the AUHAL converter on macOS.
//   - Input (only when Config::input_enabled): a separate capture
//     IAudioClient, also converted to the engine rate, with the endpoint's
//     own channel count. Capture packets go into a FrameRing (frame_ring.hpp)
//     that the render side drains by exactly its cycle length, so the engine
//     sees input and output of equal length. The ring absorbs the drift
//     between two endpoint clocks (priming, trim on overfill, zero-fill and
//     re-prime on underrun). If the input cannot be opened the stream runs
//     output-only and status().input_error says why (as on macOS).
//
// Threads: start() spawns one audio thread that initialises COM (MTA),
// opens and starts both clients, reports the result back to start(), then
// waits on the two event handles. It runs under MMCSS "Pro Audio". All COM
// objects of a stream are created, used and released on that thread. stop()
// signals it and joins; afterwards the callback is never called again.
//
// Rates: in shared mode an endpoint runs at the rate configured in the
// Windows Sound control panel (its mix format); Config::device_rate can only
// confirm that rate. The engine rate is reached through AUTOCONVERTPCM.
// Status reports the endpoint rate and latencies in endpoint frames.
//
// Buffer: a buffer_size below the endpoint's default period (10 ms) opens the
// stream through IAudioClient3 with the smallest supported shared period at or
// above it (wasapi_period.hpp), when the endpoint runs at the engine rate and
// offers one; otherwise a plain stream at the default period.
//
// Not implemented (documented in engine/README.md): exclusive mode, ASIO
// (SDK licence), device-change notifications (a removed endpoint stops the
// stream; is_running() turns false).
#if defined(_WIN32)

#include "audio_device.hpp"
#include "frame_ring.hpp"
#include "os.hpp"
#include "wasapi_period.hpp"
#include "dstudio_engine.h"

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>
// After windows.h:
#include <audioclient.h>
#include <avrt.h>
#include <mmdeviceapi.h>
#include <mmreg.h>
#include <propsys.h>
#include <wrl/client.h>

#include <algorithm>
#include <atomic>
#include <cmath>
#include <cstring>
#include <future>
#include <thread>

namespace dstudio {
namespace {

using Microsoft::WRL::ComPtr;

constexpr uint32_t kMaxDeviceChannels = 16;
constexpr REFERENCE_TIME kHnsPerSecond = 10000000;

// PKEY_Device_FriendlyName and KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, defined
// here so no INITGUID translation unit is needed.
constexpr PROPERTYKEY kPkeyFriendlyName = {
    {0xa45c254e, 0xdf1c, 0x4efd, {0x80, 0x20, 0x67, 0xd1, 0x46, 0xa8, 0x50, 0xe0}}, 14};
constexpr GUID kSubtypeFloat = {0x00000003, 0x0000, 0x0010, {0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71}};

// Error message for a failed call: "<what> failed (HRESULT 0x...)", plus a
// plain-language reason for the common audio-client errors.
std::string hr_error(const char* what, HRESULT hr) {
    char buf[16];
    std::snprintf(buf, sizeof buf, "0x%08lX", static_cast<unsigned long>(hr));
    std::string msg = std::string(what) + " failed (HRESULT " + buf + ")";
    if (hr == AUDCLNT_E_DEVICE_INVALIDATED) msg += ": the device was removed or disabled";
    else if (hr == AUDCLNT_E_DEVICE_IN_USE) msg += ": the device is in exclusive use by another application";
    else if (hr == AUDCLNT_E_UNSUPPORTED_FORMAT) msg += ": format not supported";
    else if (hr == E_ACCESSDENIED) msg += ": access denied (Settings > Privacy > Microphone?)";
    else if (hr == AUDCLNT_E_SERVICE_NOT_RUNNING) msg += ": the Windows Audio service is not running";
    return msg;
}

// COM for the calling thread. Tolerates a thread that is already an STA
// (e.g. the app's UI thread): COM is usable there, and is not uninitialised
// by us.
class ComScope {
public:
    // Joins the multithreaded apartment, or accepts an existing STA.
    ComScope() {
        const HRESULT hr = CoInitializeEx(nullptr, COINIT_MULTITHREADED);
        owned_ = SUCCEEDED(hr); // S_OK or S_FALSE (already MTA): balance it
        ok_ = owned_ || hr == RPC_E_CHANGED_MODE;
    }
    // Uninitialises COM only if this scope initialised it.
    ~ComScope() {
        if (owned_) CoUninitialize();
    }
    ComScope(const ComScope&) = delete;
    ComScope& operator=(const ComScope&) = delete;
    // True when COM is usable on this thread.
    bool ok() const noexcept { return ok_; }

private:
    bool owned_ = false;
    bool ok_ = false;
};

// Creates the endpoint enumerator; null with `err` set on failure.
ComPtr<IMMDeviceEnumerator> enumerator(std::string& err) {
    ComPtr<IMMDeviceEnumerator> e;
    const HRESULT hr =
        CoCreateInstance(__uuidof(MMDeviceEnumerator), nullptr, CLSCTX_ALL, IID_PPV_ARGS(e.GetAddressOf()));
    if (FAILED(hr)) {
        err = hr_error("creating the MMDeviceEnumerator", hr);
        return nullptr;
    }
    return e;
}

// An endpoint's id string (our DeviceInfo::uid) as UTF-8; "" if unavailable.
std::string endpoint_id(IMMDevice* d) {
    LPWSTR id = nullptr;
    if (d == nullptr || FAILED(d->GetId(&id)) || id == nullptr) return {};
    std::string s = os::narrow(id);
    CoTaskMemFree(id);
    return s;
}

// An endpoint's friendly name as UTF-8; "" if unavailable.
std::string endpoint_name(IMMDevice* d) {
    ComPtr<IPropertyStore> props;
    if (d == nullptr || FAILED(d->OpenPropertyStore(STGM_READ, props.GetAddressOf()))) return {};
    PROPVARIANT v;
    PropVariantInit(&v);
    std::string name;
    if (SUCCEEDED(props->GetValue(kPkeyFriendlyName, &v)) && v.vt == VT_LPWSTR && v.pwszVal != nullptr)
        name = os::narrow(v.pwszVal);
    PropVariantClear(&v);
    return name;
}

// Endpoint by id ("" = console default) of the given flow; null if absent.
ComPtr<IMMDevice> resolve(IMMDeviceEnumerator* e, const std::string& uid, EDataFlow flow) {
    ComPtr<IMMDevice> d;
    if (e == nullptr) return d;
    if (uid.empty()) {
        if (FAILED(e->GetDefaultAudioEndpoint(flow, eConsole, d.GetAddressOf()))) return nullptr;
    } else {
        if (FAILED(e->GetDevice(os::widen(uid).c_str(), d.GetAddressOf()))) return nullptr;
        // An id of the other flow is "not found" for this direction.
        ComPtr<IMMEndpoint> ep;
        EDataFlow f = eAll;
        if (FAILED(d.As(&ep)) || FAILED(ep->GetDataFlow(&f)) || f != flow) return nullptr;
        DWORD state = 0;
        if (FAILED(d->GetState(&state)) || state != DEVICE_STATE_ACTIVE) return nullptr;
    }
    return d;
}

// An endpoint's shared-mode mix format.
struct MixInfo {
    uint32_t channels = 0;
    double rate = 0.0;
    DWORD channel_mask = 0;
};

// Mix format of an endpoint (activates an IAudioClient, opens no stream).
bool mix_info(IMMDevice* d, MixInfo& out) {
    ComPtr<IAudioClient> client;
    if (d == nullptr ||
        FAILED(d->Activate(__uuidof(IAudioClient), CLSCTX_ALL, nullptr, reinterpret_cast<void**>(client.GetAddressOf()))))
        return false;
    WAVEFORMATEX* mix = nullptr;
    if (FAILED(client->GetMixFormat(&mix)) || mix == nullptr) return false;
    out.channels = mix->nChannels;
    out.rate = static_cast<double>(mix->nSamplesPerSec);
    out.channel_mask = 0;
    if (mix->wFormatTag == WAVE_FORMAT_EXTENSIBLE && mix->cbSize >= 22)
        out.channel_mask = reinterpret_cast<const WAVEFORMATEXTENSIBLE*>(mix)->dwChannelMask;
    CoTaskMemFree(mix);
    return true;
}

// A float32 WAVEFORMATEXTENSIBLE with the given channels, rate and speaker mask.
WAVEFORMATEXTENSIBLE float_format(uint32_t channels, double rate, DWORD mask) {
    WAVEFORMATEXTENSIBLE f{};
    f.Format.wFormatTag = WAVE_FORMAT_EXTENSIBLE;
    f.Format.nChannels = static_cast<WORD>(channels);
    f.Format.nSamplesPerSec = static_cast<DWORD>(std::lround(rate));
    f.Format.wBitsPerSample = 32;
    f.Format.nBlockAlign = static_cast<WORD>(channels * sizeof(float));
    f.Format.nAvgBytesPerSec = f.Format.nSamplesPerSec * f.Format.nBlockAlign;
    f.Format.cbSize = sizeof(WAVEFORMATEXTENSIBLE) - sizeof(WAVEFORMATEX);
    f.Samples.wValidBitsPerSample = 32;
    f.dwChannelMask = mask;
    f.SubFormat = kSubtypeFloat;
    return f;
}

// Formats a rate for error messages, e.g. "48000 Hz".
std::string rate_str(double r) { return std::to_string(static_cast<long>(std::lround(r))) + " Hz"; }

// One shared-mode, event-driven stream (render or capture) at the engine
// rate. Owned and used by the audio thread only.
struct Stream {
    ComPtr<IMMDevice> device;
    ComPtr<IAudioClient> client;
    ComPtr<IAudioRenderClient> render;
    ComPtr<IAudioCaptureClient> capture;
    HANDLE event = nullptr;
    uint32_t channels = 0;
    uint32_t buffer_frames = 0; // stream (engine-rate) frames
    MixInfo mix;
    double period_s = 0.0; // device period
    double latency_s = 0.0; // IAudioClient::GetStreamLatency
    uint32_t small_period_frames = 0; // IAudioClient3 period in endpoint frames; 0 = plain stream
    bool started = false;

    // Stops and releases the stream.
    ~Stream() { close(); }

    // Stops the client if it was started and releases every COM object and the
    // event handle. Safe to call twice.
    void close() {
        if (client && started) client->Stop();
        started = false;
        render.Reset();
        capture.Reset();
        client.Reset();
        device.Reset();
        if (event != nullptr) CloseHandle(event);
        event = nullptr;
    }

    // Opens a shared-mode, event-driven float32 client on `dev` at the engine
    // rate. `want_channels` 0 means the endpoint's own channel count (capped).
    // `buffer_size` 0 lets WASAPI choose the buffer. On failure returns the
    // HRESULT with `err` set.
    HRESULT open(IMMDevice* dev, bool is_render, uint32_t want_channels, double engine_rate, uint32_t buffer_size,
                 std::string& err) {
        device = dev;
        if (!mix_info(dev, mix)) {
            err = "cannot read the endpoint's mix format";
            return E_FAIL;
        }
        channels = want_channels != 0 ? want_channels : std::min<uint32_t>(mix.channels, kMaxDeviceInputChannels);
        HRESULT hr = dev->Activate(__uuidof(IAudioClient), CLSCTX_ALL, nullptr,
                                   reinterpret_cast<void**>(client.GetAddressOf()));
        if (FAILED(hr)) {
            err = hr_error("IMMDevice::Activate(IAudioClient)", hr);
            return hr;
        }
        DWORD mask = 0;
        if (channels == mix.channels) mask = mix.channel_mask;
        else if (channels == 2) mask = SPEAKER_FRONT_LEFT | SPEAKER_FRONT_RIGHT;
        else if (channels == 1) mask = SPEAKER_FRONT_CENTER;
        const WAVEFORMATEXTENSIBLE fmt = float_format(channels, engine_rate, mask);
        // A buffer smaller than the default period: a small shared-mode period
        // through IAudioClient3, when the endpoint offers one and runs at the
        // engine rate (that path does no rate conversion). Otherwise, or if
        // it fails, a plain stream.
        small_period_frames = 0;
        if (buffer_size != 0 && std::fabs(mix.rate - engine_rate) < 0.5) open_small_period(dev, fmt, buffer_size);
        if (small_period_frames == 0) {
            // Requested endpoint buffer: the configured buffer size, else 0
            // (the engine's default, about two device periods).
            const REFERENCE_TIME duration =
                buffer_size != 0 ? static_cast<REFERENCE_TIME>(std::llround(buffer_size * 1e7 / engine_rate)) : 0;
            const DWORD flags = AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM |
                                AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
            hr = client->Initialize(AUDCLNT_SHAREMODE_SHARED, flags, duration, 0,
                                    reinterpret_cast<const WAVEFORMATEX*>(&fmt), nullptr);
            if (FAILED(hr)) {
                err = hr_error("IAudioClient::Initialize", hr);
                return hr;
            }
        }
        event = CreateEventW(nullptr, FALSE, FALSE, nullptr);
        if (event == nullptr || FAILED(hr = client->SetEventHandle(event))) {
            err = hr_error("IAudioClient::SetEventHandle", event == nullptr ? E_FAIL : hr);
            return FAILED(hr) ? hr : E_FAIL;
        }
        UINT32 frames = 0;
        if (FAILED(hr = client->GetBufferSize(&frames))) {
            err = hr_error("IAudioClient::GetBufferSize", hr);
            return hr;
        }
        buffer_frames = frames;
        REFERENCE_TIME def_period = 0, min_period = 0, lat = 0;
        if (small_period_frames != 0) period_s = small_period_frames / mix.rate;
        else if (SUCCEEDED(client->GetDevicePeriod(&def_period, &min_period)))
            period_s = static_cast<double>(def_period) / kHnsPerSecond;
        if (SUCCEEDED(client->GetStreamLatency(&lat))) latency_s = static_cast<double>(lat) / kHnsPerSecond;
        if (is_render) hr = client->GetService(IID_PPV_ARGS(render.GetAddressOf()));
        else hr = client->GetService(IID_PPV_ARGS(capture.GetAddressOf()));
        if (FAILED(hr)) {
            err = hr_error(is_render ? "GetService(IAudioRenderClient)" : "GetService(IAudioCaptureClient)", hr);
            return hr;
        }
        return S_OK;
    }

    // Opens an IAudioClient3 on `dev` with the smallest supported period at or
    // above `requested` frames and, on success, makes it `client` and sets
    // small_period_frames. S_FALSE when the endpoint has no period below its
    // default; a failure HRESULT when a call failed. In both cases `client`
    // is left as it was (activated, not initialised) for a plain stream.
    HRESULT open_small_period(IMMDevice* dev, const WAVEFORMATEXTENSIBLE& fmt, uint32_t requested) {
        ComPtr<IAudioClient3> c3;
        HRESULT hr = dev->Activate(__uuidof(IAudioClient3), CLSCTX_ALL, nullptr, reinterpret_cast<void**>(c3.GetAddressOf()));
        if (FAILED(hr)) return hr; // before Windows 10
        SharedPeriods p;
        hr = c3->GetSharedModeEnginePeriod(reinterpret_cast<const WAVEFORMATEX*>(&fmt), &p.default_frames,
                                           &p.fundamental_frames, &p.min_frames, &p.max_frames);
        if (FAILED(hr)) return hr;
        const uint32_t period = small_shared_period(requested, p);
        if (period == 0) return S_FALSE;
        hr = c3->InitializeSharedAudioStream(AUDCLNT_STREAMFLAGS_EVENTCALLBACK, period,
                                             reinterpret_cast<const WAVEFORMATEX*>(&fmt), nullptr);
        if (FAILED(hr)) return hr;
        // Another stream may already run the endpoint at another period; the
        // engine period is shared, so report the one in effect.
        WAVEFORMATEX* cur = nullptr;
        UINT32 cur_period = period;
        if (SUCCEEDED(c3->GetCurrentSharedModeEnginePeriod(&cur, &cur_period)) && cur != nullptr) CoTaskMemFree(cur);
        client = c3;
        small_period_frames = cur_period;
        return S_OK;
    }
};

// AudioDevice on WASAPI shared mode: a render stream plus an optional
// capture stream feeding a FrameRing, all run by one audio thread.
class WasapiDevice final : public AudioDevice {
public:
    // Stops the audio thread if it is running.
    ~WasapiDevice() override { stop(); }

    // Checks the output and input endpoints, Config::device_rate (when nonzero
    // it must equal the output endpoint's shared-mode rate; the engine's
    // sample_rate is converted and not constrained) and the buffer size,
    // without opening a stream.
    int32_t validate(const Config& c, std::string& err) const override {
        ComScope com;
        if (!com.ok()) {
            err = "COM initialisation failed";
            return SE_ERR_DEVICE;
        }
        auto e = enumerator(err);
        if (!e) return SE_ERR_DEVICE;
        const ComPtr<IMMDevice> out = resolve(e.Get(), c.output_uid, eRender);
        if (!out) {
            err = c.output_uid.empty() ? "no default output device" : "output device '" + c.output_uid + "' not found";
            return SE_ERR_NOT_FOUND;
        }
        MixInfo om;
        if (!mix_info(out.Get(), om) || om.channels == 0) {
            err = endpoint_name(out.Get()) + " has no usable output format";
            return SE_ERR_INVALID_ARG;
        }
        if (!std::isfinite(c.device_rate) || c.device_rate < 0.0) {
            err = "sample_rate must be finite and >= 0";
            return SE_ERR_INVALID_ARG;
        }
        if (c.device_rate > 0.0 && std::fabs(c.device_rate - om.rate) >= 0.5) {
            err = endpoint_name(out.Get()) + " runs at " + rate_str(om.rate) +
                  " in shared mode (set in the Windows Sound control panel); " + rate_str(c.device_rate) +
                  " is not available";
            return SE_ERR_INVALID_ARG;
        }
        if (c.input_enabled) {
            const ComPtr<IMMDevice> in = resolve(e.Get(), c.input_uid, eCapture);
            if (!in) {
                err = c.input_uid.empty() ? "no default input device" : "input device '" + c.input_uid + "' not found";
                return SE_ERR_NOT_FOUND;
            }
            MixInfo im;
            if (!mix_info(in.Get(), im) || im.channels == 0) {
                err = endpoint_name(in.Get()) + " has no input channels";
                return SE_ERR_INVALID_ARG;
            }
        }
        if (c.buffer_size != 0 && (c.buffer_size < 16 || c.buffer_size > 4096)) {
            err = "buffer_size must be 0 or 16..4096";
            return SE_ERR_INVALID_ARG;
        }
        return SE_OK;
    }

    // Validates, then starts the audio thread and waits for it to report
    // whether the streams opened. An input that fails to open leaves the device
    // running output-only with status().input_error set.
    int32_t start(const Config& config, Callback* callback, std::string& err) override {
        if (thread_.joinable()) {
            err = "device already started";
            return SE_ERR_STATE;
        }
        if (callback == nullptr || config.channels == 0 || config.channels > kMaxDeviceChannels) {
            err = "invalid device configuration";
            return SE_ERR_INVALID_ARG;
        }
        const int32_t vrc = validate(config, err);
        if (vrc != SE_OK) return vrc;
        callback_ = callback;
        config_ = config;
        status_ = Status{};
        stop_event_ = CreateEventW(nullptr, TRUE, FALSE, nullptr);
        if (stop_event_ == nullptr) {
            err = "CreateEvent failed";
            return SE_ERR_DEVICE;
        }
        std::promise<std::pair<int32_t, std::string>> opened;
        auto result = opened.get_future();
        thread_ = std::thread([this, p = std::move(opened)]() mutable { run(p); });
        const auto [rc, msg] = result.get();
        if (rc != SE_OK) {
            thread_.join();
            CloseHandle(stop_event_);
            stop_event_ = nullptr;
            status_ = Status{};
            err = msg;
            return rc;
        }
        return SE_OK;
    }

    // Signals the audio thread and joins it; the callback is not called afterwards.
    int32_t stop() override {
        if (thread_.joinable()) {
            SetEvent(stop_event_);
            thread_.join();
        }
        if (stop_event_ != nullptr) CloseHandle(stop_event_);
        stop_event_ = nullptr;
        running_.store(false, std::memory_order_release);
        return SE_OK;
    }

    // True while the audio thread is running its loop.
    bool is_running() const override { return running_.load(std::memory_order_acquire); }
    // The rates, buffer size, latencies and endpoint names in effect since start().
    Status status() const override { return status_; }

private:
    // Audio thread: opens both streams, reports the result to start() through
    // `opened`, runs the loop under MMCSS "Pro Audio", then closes the streams.
    void run(std::promise<std::pair<int32_t, std::string>>& opened) {
        ComScope com;
        if (!com.ok()) {
            opened.set_value({SE_ERR_DEVICE, "COM initialisation failed on the audio thread"});
            return;
        }
        Stream out, in;
        std::string err;
        auto e = enumerator(err);
        if (!e) {
            opened.set_value({SE_ERR_DEVICE, err});
            return;
        }
        const ComPtr<IMMDevice> out_dev = resolve(e.Get(), config_.output_uid, eRender);
        if (!out_dev) {
            opened.set_value({SE_ERR_NOT_FOUND, "the output device disappeared"});
            return;
        }
        if (FAILED(out.open(out_dev.Get(), true, config_.channels, config_.sample_rate, config_.buffer_size, err))) {
            opened.set_value({SE_ERR_DEVICE, err});
            return;
        }
        // Input: failures fall back to output-only (status().input_error).
        std::string input_error;
        if (config_.input_enabled) {
            const ComPtr<IMMDevice> in_dev = resolve(e.Get(), config_.input_uid, eCapture);
            if (!in_dev) input_error = "the input device disappeared";
            else if (FAILED(in.open(in_dev.Get(), false, 0, config_.sample_rate, config_.buffer_size, input_error)))
                in.close();
            if (!input_error.empty()) in.close();
        }
        const bool duplex = in.client != nullptr;

        // Buffers (allocated here, before the stream starts).
        const uint32_t max_frames = std::max<uint32_t>(out.buffer_frames, 64);
        out_planar_.assign(static_cast<std::size_t>(out.channels) * max_frames, 0.0f);
        for (uint32_t c = 0; c < out.channels; ++c)
            out_ptrs_[c] = out_planar_.data() + static_cast<std::size_t>(c) * max_frames;
        uint32_t ring_target = 0;
        if (duplex) {
            in_planar_.assign(static_cast<std::size_t>(in.channels) * max_frames, 0.0f);
            for (uint32_t c = 0; c < in.channels; ++c)
                in_ptrs_[c] = in_planar_.data() + static_cast<std::size_t>(c) * max_frames;
            // Target: one capture period + one render period of headroom.
            const auto period_frames = [&](const Stream& s) {
                return static_cast<uint32_t>(std::lround(std::max(s.period_s, 0.003) * config_.sample_rate));
            };
            ring_target = period_frames(in) + period_frames(out);
            ring_.reset(in.channels, 8 * (in.buffer_frames + out.buffer_frames + ring_target), ring_target,
                        2 * (period_frames(in) + period_frames(out)));
        }

        // Prefill the render buffer with silence, then start.
        BYTE* data = nullptr;
        if (SUCCEEDED(out.render->GetBuffer(out.buffer_frames, &data)))
            out.render->ReleaseBuffer(out.buffer_frames, AUDCLNT_BUFFERFLAGS_SILENT);
        HRESULT hr = S_OK;
        if (duplex && FAILED(hr = in.client->Start())) {
            input_error = hr_error("starting the capture stream", hr);
            in.close();
        } else if (duplex) {
            in.started = true;
        }
        if (FAILED(hr = out.client->Start())) {
            opened.set_value({SE_ERR_DEVICE, hr_error("starting the render stream", hr)});
            return;
        }
        out.started = true;

        // Status (endpoint frames).
        const double out_rate = out.mix.rate > 0 ? out.mix.rate : config_.sample_rate;
        const double to_dev = out_rate / config_.sample_rate;
        status_.output_name = endpoint_name(out.device.Get());
        status_.device_rate = out_rate;
        status_.buffer_size = static_cast<uint32_t>(std::lround(out.period_s * out_rate));
        status_.output_channels = out.mix.channels;
        status_.output_latency =
            static_cast<uint32_t>(std::lround(out.buffer_frames * to_dev + out.latency_s * out_rate));
        status_.input_error = input_error;
        if (in.client != nullptr) {
            status_.input_open = true;
            status_.input_name = endpoint_name(in.device.Get());
            status_.input_channels = in.channels;
            status_.input_latency = static_cast<uint32_t>(
                std::lround((in.period_s + in.latency_s) * out_rate + ring_target * to_dev));
        }
        running_.store(true, std::memory_order_release);
        opened.set_value({SE_OK, {}});

        DWORD task_index = 0;
        HANDLE mmcss = AvSetMmThreadCharacteristicsW(L"Pro Audio", &task_index);
        loop(out, in.client != nullptr ? &in : nullptr);
        if (mmcss != nullptr) AvRevertMmThreadCharacteristics(mmcss);
        running_.store(false, std::memory_order_release);
        in.close();
        out.close();
    }

    // Audio-thread loop: waits on the stop, render and capture events, drains
    // capture packets and renders a cycle on each render event. Returns on
    // stop() or on a device error (which ends the stream).
    void loop(Stream& out, Stream* in) {
        HANDLE waits[3] = {stop_event_, out.event, in != nullptr ? in->event : nullptr};
        const DWORD count = in != nullptr ? 3 : 2;
        for (;;) {
            const DWORD w = WaitForMultipleObjects(count, waits, FALSE, 2000);
            if (w == WAIT_OBJECT_0) return; // stop()
            if (w == WAIT_TIMEOUT) {
                // No event for 2 s: the endpoint stalled or was removed.
                UINT32 pad = 0;
                if (FAILED(out.client->GetCurrentPadding(&pad))) return;
                continue;
            }
            if (w == WAIT_OBJECT_0 + 2 && in != nullptr) {
                if (!drain_capture(*in)) return;
                continue;
            }
            if (w != WAIT_OBJECT_0 + 1) return; // WAIT_FAILED
            if (in != nullptr && !drain_capture(*in)) return;
            if (!render_cycle(out, in)) return;
        }
    }

    // Moves every available capture packet into the ring.
    bool drain_capture(Stream& in) {
        for (;;) {
            UINT32 packet = 0;
            if (FAILED(in.capture->GetNextPacketSize(&packet))) return false;
            if (packet == 0) return true;
            BYTE* data = nullptr;
            UINT32 frames = 0;
            DWORD flags = 0;
            const HRESULT hr = in.capture->GetBuffer(&data, &frames, &flags, nullptr, nullptr);
            if (hr == AUDCLNT_S_BUFFER_EMPTY) return true;
            if (FAILED(hr)) return false;
            const bool silent = (flags & AUDCLNT_BUFFERFLAGS_SILENT) != 0 || data == nullptr;
            ring_.write(silent ? nullptr : reinterpret_cast<const float*>(data), frames);
            in.capture->ReleaseBuffer(frames);
        }
    }

    // Fills the free part of the render buffer: reads the same number of input
    // frames from the ring, calls the engine and interleaves its output. False on
    // a device error.
    bool render_cycle(Stream& out, Stream* in) {
        UINT32 padding = 0;
        if (FAILED(out.client->GetCurrentPadding(&padding))) return false;
        const uint32_t frames = out.buffer_frames > padding ? out.buffer_frames - padding : 0;
        if (frames == 0) return true;
        BYTE* data = nullptr;
        if (FAILED(out.render->GetBuffer(frames, &data)) || data == nullptr) return false;
        const float* in_ptrs[kMaxDeviceInputChannels];
        uint32_t n_in = 0;
        if (in != nullptr) {
            ring_.read(in_ptrs_, in->channels, frames);
            for (uint32_t c = 0; c < in->channels; ++c) in_ptrs[c] = in_ptrs_[c];
            n_in = in->channels;
        }
        callback_->render(out_ptrs_, out.channels, n_in > 0 ? in_ptrs : nullptr, n_in, frames);
        auto* dst = reinterpret_cast<float*>(data);
        for (uint32_t i = 0; i < frames; ++i)
            for (uint32_t c = 0; c < out.channels; ++c) dst[i * out.channels + c] = out_ptrs_[c][i];
        return SUCCEEDED(out.render->ReleaseBuffer(frames, 0));
    }

    Callback* callback_ = nullptr;
    Config config_;
    Status status_;
    std::thread thread_;
    HANDLE stop_event_ = nullptr;
    std::atomic<bool> running_{false};
    // Audio-thread buffers (sized before the stream starts).
    std::vector<float> out_planar_, in_planar_;
    float* out_ptrs_[kMaxDeviceChannels] = {};
    float* in_ptrs_[kMaxDeviceInputChannels] = {};
    FrameRing ring_;
};

} // namespace

// Creates the Windows audio device (WASAPI shared mode).
std::unique_ptr<AudioDevice> create_platform_audio_device() { return std::make_unique<WasapiDevice>(); }

// Lists every active render and capture endpoint as a device, each with its
// shared-mode rate. No devices (and no error) without a Windows audio stack.
int32_t list_audio_devices(std::vector<DeviceInfo>& out, std::string& err) {
    out.clear();
    ComScope com;
    if (!com.ok()) {
        err = "COM initialisation failed";
        return SE_ERR_DEVICE;
    }
    auto e = enumerator(err);
    if (!e) {
        // No audio stack (e.g. a server without the Audio service): no devices.
        err.clear();
        return SE_OK;
    }
    for (const EDataFlow flow : {eRender, eCapture}) {
        std::string default_id;
        {
            ComPtr<IMMDevice> d;
            if (SUCCEEDED(e->GetDefaultAudioEndpoint(flow, eConsole, d.GetAddressOf()))) default_id = endpoint_id(d.Get());
        }
        ComPtr<IMMDeviceCollection> coll;
        if (FAILED(e->EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE, coll.GetAddressOf()))) continue;
        UINT n = 0;
        coll->GetCount(&n);
        for (UINT i = 0; i < n; ++i) {
            ComPtr<IMMDevice> d;
            if (FAILED(coll->Item(i, d.GetAddressOf()))) continue;
            MixInfo mix;
            if (!mix_info(d.Get(), mix)) continue;
            DeviceInfo info;
            info.uid = endpoint_id(d.Get());
            if (info.uid.empty()) continue;
            info.name = endpoint_name(d.Get());
            if (info.name.empty()) info.name = info.uid;
            // Windows endpoints are one-directional (a headset has a render
            // and a capture endpoint with different ids).
            if (flow == eRender) {
                info.output_channels = mix.channels;
                info.is_default_output = info.uid == default_id;
            } else {
                info.input_channels = mix.channels;
                info.is_default_input = info.uid == default_id;
            }
            // Shared mode: the endpoint runs at its mix-format rate only.
            info.sample_rates.push_back(mix.rate);
            out.push_back(std::move(info));
        }
    }
    return SE_OK;
}

// The default render endpoint's shared-mode rate, or 0 if there is none.
double query_default_output_sample_rate() {
    ComScope com;
    if (!com.ok()) return 0.0;
    std::string err;
    auto e = enumerator(err);
    if (!e) return 0.0;
    const ComPtr<IMMDevice> d = resolve(e.Get(), "", eRender);
    MixInfo mix;
    return d && mix_info(d.Get(), mix) ? mix.rate : 0.0;
}

} // namespace dstudio

#endif // _WIN32
