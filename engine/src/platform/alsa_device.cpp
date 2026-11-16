// Linux ALSA backend: the kernel sound API, with no sound server.
//
// Why: ALSA is part of the kernel, so it works on every Linux system,
// including the ones without PipeWire (linux_audio_device.cpp uses it for
// the system default then). Opening a hardware PCM directly ("hw:") also
// gives the lowest latency there is, with nothing to configure.
//
// Devices (DeviceInfo::uid, see linux_audio_select.hpp):
//   - "alsa:default": the ALSA default PCM (dmix/dsnoop on a bare system, or
//     whatever the system's asoundrc routes it to).
//   - "alsa:hw:CARD=<id>,DEV=<n>": one hardware PCM, found by walking the
//     cards (snd_card_next) and their PCM devices (snd_ctl_pcm_next_device).
//     The card id is stable across reboots; the card number is not. One
//     DeviceInfo carries both directions of a PCM device.
//   - "alsa:<name>": any other PCM name (an asoundrc device, "pipewire", ...).
// A hardware PCM is first opened as is; if it cannot run the engine's rate
// or the PCM has no float/int format we convert ourselves, it is reopened
// through ALSA's plug plugin ("plughw:"), which converts rate, format and
// channels. Listing opens each hardware PCM briefly (non-blocking, without
// configuring it) to read its channel count; a PCM that is busy (held by
// PipeWire or another program) is listed with 2 channels.
//
// Streams: interleaved read/write access, a period of Config::buffer_size
// frames (else the preferred block, at least 256) and 2 periods of playback
// buffer, at the engine rate. One thread (SCHED_FIFO when the system allows
// it) runs the cycle:
//   output only: wait until a period is free, render it, write it.
//   duplex:      read one captured period, render with it, write the output.
// Input and output are linked (snd_pcm_link) when the driver allows it, so
// they start on the same sample; the playback buffer is primed with silence
// so the duplex cycle never waits on it. Input and output on two different
// cards run on two clocks: the drift ends in an xrun after a while, which
// restarts both streams (as every xrun does). A device that disappears
// (unplugged) stops the stream: is_running() turns false.
//
// libasound is loaded with dlopen (as for MIDI input): only the headers are
// needed to build, and without libasound there are no ALSA devices.
#if defined(__linux__) && defined(DSTUDIO_HAVE_ALSA)

#include "audio_device.hpp"
#include "dstudio_engine.h"
#include "linux_audio.hpp"
#include "linux_audio_select.hpp"
#include "pcm_convert.hpp"

#include <alsa/asoundlib.h>

#include <algorithm>
#include <atomic>
#include <cerrno>
#include <cmath>
#include <cstring>
#include <dlfcn.h>
#include <pthread.h>
#include <sched.h>
#include <string>
#include <thread>
#include <vector>

// The ALSA headers' accessor macros expand to C casts.
#pragma GCC diagnostic ignored "-Wold-style-cast"

namespace dstudio {
namespace {

// The libasound functions this backend uses, resolved from libasound.so.2.
struct AlsaPcmApi {
#define DSTUDIO_ALSA_PCM_FNS(X)                                                                                     \
    X(snd_pcm_open) X(snd_pcm_close) X(snd_pcm_hw_params_sizeof) X(snd_pcm_hw_params_any)                          \
    X(snd_pcm_hw_params_set_access) X(snd_pcm_hw_params_set_format) X(snd_pcm_hw_params_set_channels)              \
    X(snd_pcm_hw_params_set_channels_near) X(snd_pcm_hw_params_get_channels_max) X(snd_pcm_hw_params_test_channels) \
    X(snd_pcm_hw_params_set_rate_resample) X(snd_pcm_hw_params_set_rate) X(snd_pcm_hw_params_set_period_size_near) \
    X(snd_pcm_hw_params_set_periods_near) X(snd_pcm_hw_params) X(snd_pcm_hw_params_get_period_size)                \
    X(snd_pcm_hw_params_get_buffer_size) X(snd_pcm_hw_params_get_channels) X(snd_pcm_sw_params_sizeof)             \
    X(snd_pcm_sw_params_current) X(snd_pcm_sw_params_set_start_threshold) X(snd_pcm_sw_params_set_avail_min)       \
    X(snd_pcm_sw_params) X(snd_pcm_prepare) X(snd_pcm_drop) X(snd_pcm_link) X(snd_pcm_unlink) X(snd_pcm_writei)    \
    X(snd_pcm_readi) X(snd_pcm_wait) X(snd_pcm_recover) X(snd_pcm_start) X(snd_pcm_state) X(snd_strerror)          \
    X(snd_card_next) X(snd_ctl_open) X(snd_ctl_close) X(snd_ctl_card_info_sizeof) X(snd_ctl_card_info)             \
    X(snd_ctl_card_info_get_id) X(snd_ctl_card_info_get_name) X(snd_ctl_pcm_next_device) X(snd_pcm_info_sizeof)    \
    X(snd_pcm_info_set_device) X(snd_pcm_info_set_subdevice) X(snd_pcm_info_set_stream) X(snd_ctl_pcm_info)        \
    X(snd_pcm_info_get_name)
#define DSTUDIO_ALSA_PCM_FIELD(name) decltype(&::name) name = nullptr;
    DSTUDIO_ALSA_PCM_FNS(DSTUDIO_ALSA_PCM_FIELD)
#undef DSTUDIO_ALSA_PCM_FIELD
};

// Null when libasound (or one of its functions) is missing.
const AlsaPcmApi* api() {
    static const AlsaPcmApi* const loaded = []() -> const AlsaPcmApi* {
        void* lib = dlopen("libasound.so.2", RTLD_NOW | RTLD_LOCAL);
        if (lib == nullptr) return nullptr;
        static AlsaPcmApi a;
        bool ok = true;
#define DSTUDIO_ALSA_PCM_LOAD(name)                                             \
    a.name = reinterpret_cast<decltype(a.name)>(dlsym(lib, #name));              \
    ok = ok && a.name != nullptr;
        DSTUDIO_ALSA_PCM_FNS(DSTUDIO_ALSA_PCM_LOAD)
#undef DSTUDIO_ALSA_PCM_LOAD
        return ok ? &a : nullptr; // the library stays loaded for the process
    }();
    return loaded;
}

constexpr uint32_t kMaxOutChannels = 64;  // device channels we write (engine fills 16, rest silent)
constexpr uint32_t kMaxRenderChannels = 16;
constexpr uint32_t kMaxPeriod = 8192;
constexpr int kWaitMs = 200;              // one wait; a cycle never takes this long
constexpr int kLostAfterTimeouts = 10;    // 2 s without the device moving: it is gone

// A zeroed, correctly sized opaque ALSA record (what the *_alloca macros make).
struct Blob {
    explicit Blob(std::size_t n) : bytes(n, 0) {}
    template <class T> T* as() { return reinterpret_cast<T*>(bytes.data()); }
    std::vector<unsigned char> bytes;
};

std::string alsa_error(const AlsaPcmApi& a, int rc) { return a.snd_strerror(rc); }

// One opened, configured PCM.
struct Pcm {
    snd_pcm_t* h = nullptr;
    std::string name; // the PCM name actually opened
    pcm::Format format = pcm::Format::F32;
    uint32_t channels = 0;
    uint32_t period = 0;
    uint32_t buffer = 0;
    std::vector<uint8_t> raw; // one period of device-format frames
};

// Sets the first sample format of ours that the PCM accepts.
bool pick_format(const AlsaPcmApi& a, snd_pcm_t* h, snd_pcm_hw_params_t* hw, pcm::Format& out) {
    static constexpr std::pair<snd_pcm_format_t, pcm::Format> kFormats[] = {
        {SND_PCM_FORMAT_FLOAT_LE, pcm::Format::F32}, {SND_PCM_FORMAT_S32_LE, pcm::Format::S32},
        {SND_PCM_FORMAT_S24_3LE, pcm::Format::S24_3}, {SND_PCM_FORMAT_S24_LE, pcm::Format::S24},
        {SND_PCM_FORMAT_S16_LE, pcm::Format::S16}};
    for (const auto& [alsa_fmt, ours] : kFormats)
        if (a.snd_pcm_hw_params_set_format(h, hw, alsa_fmt) == 0) {
            out = ours;
            return true;
        }
    return false;
}

// Opens `name` for `stream` and configures it: interleaved, our format,
// exactly `rate`, `want_channels` (playback: nearest; capture: as many as
// the device has, up to the cap), a period near `period` and `periods`
// periods. On failure returns false with `err` set and nothing open.
bool open_configured(const AlsaPcmApi& a, const std::string& name, snd_pcm_stream_t stream, uint32_t rate,
                     uint32_t period, uint32_t periods, Pcm& out, std::string& err) {
    snd_pcm_t* h = nullptr;
    int rc = a.snd_pcm_open(&h, name.c_str(), stream, SND_PCM_NONBLOCK);
    if (rc < 0) {
        err = "cannot open ALSA device '" + name + "': " + alsa_error(a, rc) +
              (rc == -EBUSY ? " (in use by PipeWire or another program)" : "");
        return false;
    }
    auto fail = [&](const std::string& what, int code) {
        err = "ALSA device '" + name + "': " + what + (code < 0 ? ": " + alsa_error(a, code) : std::string());
        a.snd_pcm_close(h);
        return false;
    };
    Blob hwb(a.snd_pcm_hw_params_sizeof());
    auto* hw = hwb.as<snd_pcm_hw_params_t>();
    if ((rc = a.snd_pcm_hw_params_any(h, hw)) < 0) return fail("no configuration", rc);
    if ((rc = a.snd_pcm_hw_params_set_access(h, hw, SND_PCM_ACCESS_RW_INTERLEAVED)) < 0)
        return fail("no interleaved access", rc);
    pcm::Format fmt{};
    if (!pick_format(a, h, hw, fmt)) return fail("no supported sample format", 0);
    unsigned int ch = 2;
    if (stream == SND_PCM_STREAM_CAPTURE) {
        unsigned int max = 0;
        if (a.snd_pcm_hw_params_get_channels_max(hw, &max) == 0 && max > 0)
            ch = std::min<unsigned int>(max, kMaxDeviceInputChannels);
    }
    if (a.snd_pcm_hw_params_test_channels(h, hw, ch) == 0) {
        rc = a.snd_pcm_hw_params_set_channels(h, hw, ch);
    } else {
        rc = a.snd_pcm_hw_params_set_channels_near(h, hw, &ch);
    }
    if (rc < 0) return fail("no usable channel count", rc);
    a.snd_pcm_hw_params_set_rate_resample(h, hw, 0); // the plug variant resamples instead
    if ((rc = a.snd_pcm_hw_params_set_rate(h, hw, rate, 0)) < 0)
        return fail("does not run at " + std::to_string(rate) + " Hz", rc);
    snd_pcm_uframes_t p = period;
    int dir = 0;
    if ((rc = a.snd_pcm_hw_params_set_period_size_near(h, hw, &p, &dir)) < 0) return fail("period size", rc);
    unsigned int n = periods;
    dir = 0;
    if ((rc = a.snd_pcm_hw_params_set_periods_near(h, hw, &n, &dir)) < 0) return fail("period count", rc);
    if ((rc = a.snd_pcm_hw_params(h, hw)) < 0) return fail("cannot apply the configuration", rc);
    snd_pcm_uframes_t got_period = 0, got_buffer = 0;
    unsigned int got_ch = 0;
    a.snd_pcm_hw_params_get_period_size(hw, &got_period, &dir);
    a.snd_pcm_hw_params_get_buffer_size(hw, &got_buffer);
    a.snd_pcm_hw_params_get_channels(hw, &got_ch);
    if (got_period == 0 || got_period > kMaxPeriod || got_ch == 0 ||
        (stream == SND_PCM_STREAM_PLAYBACK && got_ch > kMaxOutChannels))
        return fail("unusable period size or channel count", 0);

    Blob swb(a.snd_pcm_sw_params_sizeof());
    auto* sw = swb.as<snd_pcm_sw_params_t>();
    a.snd_pcm_sw_params_current(h, sw);
    a.snd_pcm_sw_params_set_avail_min(h, sw, got_period);
    // Playback starts once the primed buffer is full; capture with the
    // first read (or with the linked playback).
    a.snd_pcm_sw_params_set_start_threshold(h, sw, stream == SND_PCM_STREAM_PLAYBACK ? got_buffer : 1);
    if ((rc = a.snd_pcm_sw_params(h, sw)) < 0) return fail("cannot apply the software parameters", rc);

    out.h = h;
    out.name = name;
    out.format = fmt;
    out.channels = got_ch;
    out.period = static_cast<uint32_t>(got_period);
    out.buffer = static_cast<uint32_t>(got_buffer);
    out.raw.assign(static_cast<std::size_t>(out.period) * got_ch * pcm::bytes_per_sample(fmt), 0);
    return true;
}

// Opens the PCM behind an ALSA id: directly, then (for a hardware PCM or a
// plain named PCM) through the plug plugin, which converts what the device
// cannot do itself. `err` keeps the most useful message.
bool open_pcm(const AlsaPcmApi& a, const std::string& uid, snd_pcm_stream_t stream, uint32_t rate, uint32_t period,
              uint32_t periods, Pcm& out, std::string& err) {
    const std::string name = linux_audio::alsa_pcm_name(uid);
    if (open_configured(a, name, stream, rate, period, periods, out, err)) return true;
    const std::string plug = linux_audio::plug_pcm_name(name);
    if (plug.empty() || err.find("in use by") != std::string::npos) return false;
    std::string plug_err;
    if (open_configured(a, plug, stream, rate, period, periods, out, plug_err)) return true;
    err += "; " + plug_err;
    return false;
}

void close_pcm(const AlsaPcmApi& a, Pcm& p) {
    if (p.h != nullptr) a.snd_pcm_close(p.h);
    p.h = nullptr;
}

// Asks for SCHED_FIFO on the calling thread; keeps the normal policy when
// the system does not allow it (no rtprio limit).
void try_realtime() {
    sched_param sp{};
    sp.sched_priority = std::min(70, sched_get_priority_max(SCHED_FIFO));
    pthread_setschedparam(pthread_self(), SCHED_FIFO, &sp);
}

class AlsaDevice final : public AudioDevice {
public:
    ~AlsaDevice() override { stop(); }

    int32_t validate(const Config& c, std::string& err) const override {
        if (api() == nullptr) {
            err = "ALSA is not available (libasound.so.2 not found)";
            return SE_ERR_DEVICE;
        }
        if (!std::isfinite(c.device_rate) || c.device_rate < 0.0) {
            err = "sample_rate must be finite and >= 0";
            return SE_ERR_INVALID_ARG;
        }
        if (c.device_rate > 0.0 && std::fabs(c.device_rate - c.sample_rate) >= 0.5) {
            err = "ALSA devices run at the project rate (" + std::to_string(std::lround(c.sample_rate)) + " Hz)";
            return SE_ERR_INVALID_ARG;
        }
        if (c.buffer_size != 0 && (c.buffer_size < 16 || c.buffer_size > 4096)) {
            err = "buffer_size must be 0 or 16..4096";
            return SE_ERR_INVALID_ARG;
        }
        if (!known(c.output_uid, true)) {
            err = "output device '" + c.output_uid + "' not found";
            return SE_ERR_NOT_FOUND;
        }
        if (c.input_enabled && !known(c.input_uid, false)) {
            err = "input device '" + c.input_uid + "' not found";
            return SE_ERR_NOT_FOUND;
        }
        return SE_OK;
    }

    int32_t start(const Config& c, Callback* callback, std::string& err) override {
        if (thread_.joinable()) {
            err = "device already started";
            return SE_ERR_STATE;
        }
        if (callback == nullptr || c.channels == 0 || c.channels > kMaxRenderChannels) {
            err = "invalid device configuration";
            return SE_ERR_INVALID_ARG;
        }
        int32_t rc = validate(c, err);
        if (rc != SE_OK) return rc;
        const AlsaPcmApi& a = *api();
        callback_ = callback;
        status_ = Status{};
        const auto rate = static_cast<uint32_t>(std::lround(c.sample_rate));
        const uint32_t period = c.buffer_size != 0 ? c.buffer_size : std::max<uint32_t>(c.preferred_block, 256);
        const std::string out_uid = c.output_uid.empty() ? std::string(linux_audio::kAlsaDefaultUid) : c.output_uid;
        if (!open_pcm(a, out_uid, SND_PCM_STREAM_PLAYBACK, rate, period, 2, out_, err)) return SE_ERR_DEVICE;
        render_channels_ = std::min(out_.channels, kMaxRenderChannels);
        out_planar_.assign(static_cast<std::size_t>(render_channels_) * out_.period, 0.0f);
        for (uint32_t ch = 0; ch < render_channels_; ++ch)
            out_ptrs_[ch] = out_planar_.data() + static_cast<std::size_t>(ch) * out_.period;

        std::string input_error;
        if (c.input_enabled) {
            const std::string in_uid = c.input_uid.empty() ? std::string(linux_audio::kAlsaDefaultUid) : c.input_uid;
            // The capture period must match the playback period: one cycle reads
            // one and writes the other.
            if (!open_pcm(a, in_uid, SND_PCM_STREAM_CAPTURE, rate, out_.period, 4, in_, input_error)) {
                close_pcm(a, in_);
            } else if (in_.period != out_.period) {
                input_error = "the input cannot use the output's period of " + std::to_string(out_.period) + " frames";
                close_pcm(a, in_);
            } else {
                in_planar_.assign(static_cast<std::size_t>(in_.channels) * in_.period, 0.0f);
                for (uint32_t ch = 0; ch < in_.channels; ++ch)
                    in_ptrs_[ch] = in_planar_.data() + static_cast<std::size_t>(ch) * in_.period;
                linked_ = a.snd_pcm_link(out_.h, in_.h) == 0;
            }
        }
        xruns_.store(0, std::memory_order_relaxed);
        lost_.store(false, std::memory_order_relaxed);
        stop_.store(false, std::memory_order_relaxed);
        if (!prime()) {
            err = "ALSA device '" + out_.name + "' did not start";
            teardown();
            return SE_ERR_DEVICE;
        }
        status_.output_name = display_name(out_uid);
        status_.output_channels = out_.channels;
        status_.device_rate = c.sample_rate;
        status_.buffer_size = out_.period;
        status_.output_latency = out_.buffer;
        status_.input_error = input_error;
        if (in_.h != nullptr) {
            status_.input_open = true;
            status_.input_name = display_name(c.input_uid.empty() ? std::string(linux_audio::kAlsaDefaultUid) : c.input_uid);
            status_.input_channels = in_.channels;
            status_.input_latency = in_.period;
        }
        running_.store(true, std::memory_order_release);
        thread_ = std::thread([this] { run(); });
        return SE_OK;
    }

    int32_t stop() override {
        running_.store(false, std::memory_order_release);
        stop_.store(true, std::memory_order_release);
        if (thread_.joinable()) thread_.join();
        teardown();
        return SE_OK;
    }

    bool is_running() const override {
        return running_.load(std::memory_order_acquire) && !lost_.load(std::memory_order_acquire);
    }

    Status status() const override { return status_; }

    // Test/bench hook: xruns (and restarts) since start().
    uint32_t xruns() const { return xruns_.load(std::memory_order_relaxed); }

private:
    // True if `uid` is the default, a named PCM, or a hardware PCM that exists.
    static bool known(const std::string& uid, bool output) {
        const std::string pcm = linux_audio::alsa_pcm_name(uid);
        if (!linux_audio::is_hw_pcm(pcm)) return true; // opened (and checked) by name
        std::vector<DeviceInfo> devs;
        linux_audio::alsa_list(devs, false);
        return std::any_of(devs.begin(), devs.end(), [&](const DeviceInfo& d) {
            return d.uid == uid && (output ? d.output_channels : d.input_channels) > 0;
        });
    }

    static std::string display_name(const std::string& uid) {
        std::vector<DeviceInfo> devs;
        linux_audio::alsa_list(devs, false);
        for (const DeviceInfo& d : devs)
            if (d.uid == uid) return d.name;
        return linux_audio::alsa_pcm_name(uid) + " (ALSA)";
    }

    void teardown() {
        if (const AlsaPcmApi* a = api()) {
            if (linked_ && in_.h != nullptr) a->snd_pcm_unlink(in_.h);
            if (out_.h != nullptr) a->snd_pcm_drop(out_.h);
            if (in_.h != nullptr) a->snd_pcm_drop(in_.h);
            close_pcm(*a, in_);
            close_pcm(*a, out_);
        }
        linked_ = false;
    }

    // Prepares the streams and fills the playback buffer with silence, which
    // starts playback (and a linked capture). Unlinked capture starts with
    // its first read. False if the device does not accept it.
    bool prime() noexcept {
        const AlsaPcmApi& a = *api();
        if (a.snd_pcm_prepare(out_.h) < 0) return false;
        if (in_.h != nullptr && !linked_ && a.snd_pcm_prepare(in_.h) < 0) return false;
        std::fill(out_.raw.begin(), out_.raw.end(), uint8_t{0});
        for (uint32_t done = 0; done < out_.buffer;) {
            const uint32_t n = std::min(out_.period, out_.buffer - done);
            const snd_pcm_sframes_t w = a.snd_pcm_writei(out_.h, out_.raw.data(), n);
            if (w == -EAGAIN) {
                // The buffer is full: playback is running.
                if (a.snd_pcm_state(out_.h) == SND_PCM_STATE_RUNNING) break;
                if (a.snd_pcm_wait(out_.h, kWaitMs) < 0) return false;
                continue;
            }
            if (w < 0) return false;
            done += static_cast<uint32_t>(w);
        }
        if (a.snd_pcm_state(out_.h) != SND_PCM_STATE_RUNNING && a.snd_pcm_start(out_.h) < 0) return false;
        return true;
    }

    // After an xrun or a suspend: restart both streams from a primed buffer.
    // False when the device is gone.
    bool restart(int code) noexcept {
        const AlsaPcmApi& a = *api();
        xruns_.fetch_add(1, std::memory_order_relaxed);
        if (code == -ENODEV || code == -EBADFD || code == -ENOTTY) return false;
        if (code == -ESTRPIPE) {
            // Suspended: wait for the resume, then prepare (snd_pcm_recover).
            if (a.snd_pcm_recover(out_.h, code, 1) < 0) return false;
        }
        a.snd_pcm_drop(out_.h);
        if (in_.h != nullptr && !linked_) a.snd_pcm_drop(in_.h);
        return prime();
    }

    // Waits for `pcm` (timeouts counted); false when it stopped answering or
    // failed in a way restart() cannot fix.
    bool wait(snd_pcm_t* h, int& timeouts) noexcept {
        const int r = api()->snd_pcm_wait(h, kWaitMs);
        if (r == 0) return ++timeouts < kLostAfterTimeouts;
        timeouts = 0;
        if (r < 0) return restart(r);
        return true;
    }

    // Reads one captured period into the raw buffer. False to stop.
    bool read_period(int& timeouts) noexcept {
        const AlsaPcmApi& a = *api();
        const uint32_t frame_bytes = in_.channels * pcm::bytes_per_sample(in_.format);
        for (uint32_t got = 0; got < in_.period;) {
            if (stop_.load(std::memory_order_acquire)) return false;
            const snd_pcm_sframes_t r =
                a.snd_pcm_readi(in_.h, in_.raw.data() + static_cast<std::size_t>(got) * frame_bytes, in_.period - got);
            if (r == -EAGAIN) {
                if (!wait(in_.h, timeouts)) return false;
                continue;
            }
            if (r < 0) {
                if (!restart(static_cast<int>(r))) return false;
                got = 0; // a fresh cycle after the restart
                continue;
            }
            timeouts = 0;
            got += static_cast<uint32_t>(r);
        }
        return true;
    }

    // Writes the rendered period. False to stop.
    bool write_period(int& timeouts) noexcept {
        const AlsaPcmApi& a = *api();
        const uint32_t frame_bytes = out_.channels * pcm::bytes_per_sample(out_.format);
        for (uint32_t done = 0; done < out_.period;) {
            if (stop_.load(std::memory_order_acquire)) return false;
            const snd_pcm_sframes_t w =
                a.snd_pcm_writei(out_.h, out_.raw.data() + static_cast<std::size_t>(done) * frame_bytes, out_.period - done);
            if (w == -EAGAIN) {
                if (!wait(out_.h, timeouts)) return false;
                continue;
            }
            if (w < 0) {
                // Underrun: the restart primes a full buffer of silence, so the
                // rest of this period is dropped.
                return restart(static_cast<int>(w));
            }
            timeouts = 0;
            done += static_cast<uint32_t>(w);
        }
        return true;
    }

    // The audio thread (see the file comment for the cycle).
    void run() noexcept {
        try_realtime();
        int timeouts = 0;
        const uint32_t n = out_.period;
        while (!stop_.load(std::memory_order_acquire)) {
            const float* in_ptrs[kMaxDeviceInputChannels];
            uint32_t n_in = 0;
            if (in_.h != nullptr) {
                if (!read_period(timeouts)) break;
                pcm::deinterleave(in_.format, in_.raw.data(), in_.channels, n, in_ptrs_);
                for (uint32_t c = 0; c < in_.channels; ++c) in_ptrs[c] = in_ptrs_[c];
                n_in = in_.channels;
            } else if (!wait(out_.h, timeouts)) {
                break;
            }
            if (stop_.load(std::memory_order_acquire)) break;
            callback_->render(out_ptrs_, render_channels_, n_in > 0 ? in_ptrs : nullptr, n_in, n);
            pcm::interleave(out_.format, out_ptrs_, render_channels_, out_.channels, n, out_.raw.data());
            if (!write_period(timeouts)) break;
        }
        if (!stop_.load(std::memory_order_acquire)) lost_.store(true, std::memory_order_release);
    }

    Callback* callback_ = nullptr;
    Status status_;
    Pcm out_, in_;
    bool linked_ = false;
    uint32_t render_channels_ = 2;
    std::vector<float> out_planar_, in_planar_;
    float* out_ptrs_[kMaxRenderChannels] = {};
    float* in_ptrs_[kMaxDeviceInputChannels] = {};
    std::thread thread_;
    std::atomic<bool> running_{false}, stop_{false}, lost_{false};
    std::atomic<uint32_t> xruns_{0};
};

// Reads the channel count of one direction of a hardware PCM by opening it
// without configuring it. 2 when it cannot be opened (busy).
uint32_t probe_channels(const AlsaPcmApi& a, const std::string& pcm, snd_pcm_stream_t stream) {
    snd_pcm_t* h = nullptr;
    if (a.snd_pcm_open(&h, pcm.c_str(), stream, SND_PCM_NONBLOCK) < 0) return 2;
    Blob hwb(a.snd_pcm_hw_params_sizeof());
    auto* hw = hwb.as<snd_pcm_hw_params_t>();
    unsigned int max = 0;
    if (a.snd_pcm_hw_params_any(h, hw) >= 0) a.snd_pcm_hw_params_get_channels_max(hw, &max);
    a.snd_pcm_close(h);
    if (max == 0) return 2;
    return std::min<uint32_t>(max, stream == SND_PCM_STREAM_CAPTURE ? kMaxDeviceInputChannels : kMaxOutChannels);
}

} // namespace

namespace linux_audio {

std::unique_ptr<AudioDevice> create_alsa_device() { return std::make_unique<AlsaDevice>(); }

bool alsa_available() { return api() != nullptr; }

void alsa_list(std::vector<DeviceInfo>& out, bool defaults) {
    const AlsaPcmApi* ap = api();
    if (ap == nullptr) return;
    const AlsaPcmApi& a = *ap;
    DeviceInfo def;
    def.uid = std::string(kAlsaDefaultUid);
    def.name = "ALSA default";
    def.input_channels = def.output_channels = 2;
    def.is_default_input = def.is_default_output = defaults;
    out.push_back(std::move(def));
    Blob card_info(a.snd_ctl_card_info_sizeof());
    Blob pcm_info(a.snd_pcm_info_sizeof());
    auto* ci = card_info.as<snd_ctl_card_info_t>();
    auto* pi = pcm_info.as<snd_pcm_info_t>();
    int card = -1;
    while (a.snd_card_next(&card) == 0 && card >= 0) {
        snd_ctl_t* ctl = nullptr;
        if (a.snd_ctl_open(&ctl, ("hw:" + std::to_string(card)).c_str(), 0) < 0) continue;
        if (a.snd_ctl_card_info(ctl, ci) < 0) {
            a.snd_ctl_close(ctl);
            continue;
        }
        const std::string card_id = a.snd_ctl_card_info_get_id(ci);
        const std::string card_name = a.snd_ctl_card_info_get_name(ci);
        int dev = -1;
        while (a.snd_ctl_pcm_next_device(ctl, &dev) == 0 && dev >= 0) {
            DeviceInfo d;
            d.uid = alsa_hw_uid(card_id, dev);
            const std::string pcm = alsa_pcm_name(d.uid);
            std::string pcm_name;
            for (snd_pcm_stream_t s : {SND_PCM_STREAM_PLAYBACK, SND_PCM_STREAM_CAPTURE}) {
                a.snd_pcm_info_set_device(pi, static_cast<unsigned int>(dev));
                a.snd_pcm_info_set_subdevice(pi, 0);
                a.snd_pcm_info_set_stream(pi, s);
                if (a.snd_ctl_pcm_info(ctl, pi) < 0) continue;
                if (pcm_name.empty()) pcm_name = a.snd_pcm_info_get_name(pi);
                (s == SND_PCM_STREAM_PLAYBACK ? d.output_channels : d.input_channels) = probe_channels(a, pcm, s);
            }
            if (d.input_channels == 0 && d.output_channels == 0) continue;
            d.name = alsa_hw_name(card_name, pcm_name);
            out.push_back(std::move(d));
        }
        a.snd_ctl_close(ctl);
    }
}

} // namespace linux_audio
} // namespace dstudio

#else
#if defined(__linux__)

// Built without the ALSA headers: no ALSA devices.
#include "dstudio_engine.h"
#include "linux_audio.hpp"

namespace dstudio::linux_audio {

namespace {
class NoAlsaDevice final : public AudioDevice {
public:
    int32_t validate(const Config&, std::string& err) const override {
        err = "this build has no ALSA support";
        return SE_ERR_DEVICE;
    }
    int32_t start(const Config&, Callback*, std::string& err) override {
        err = "this build has no ALSA support";
        return SE_ERR_DEVICE;
    }
    int32_t stop() override { return SE_OK; }
    bool is_running() const override { return false; }
    Status status() const override { return {}; }
};
} // namespace

std::unique_ptr<AudioDevice> create_alsa_device() { return std::make_unique<NoAlsaDevice>(); }
bool alsa_available() { return false; }
void alsa_list(std::vector<DeviceInfo>&, bool) {}

} // namespace dstudio::linux_audio

#endif
#endif
