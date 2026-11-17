// Linux PipeWire backend.
//
// Why PipeWire (and not JACK first): it is the default audio server of
// every current mainstream distribution (Fedora, Ubuntu >= 22.10, Debian 12
// desktop, Arch, ...), it serves desktop and pro-audio clients at once
// (JACK applications run on it through pipewire-jack), it resamples and
// remixes per stream, and its quantum can be lowered per client
// (node.latency) for pro-audio buffer sizes. A native JACK backend remains
// possible future work for systems running jackd without PipeWire.
//
// Streams (pw_stream, float32 interleaved at the *engine* rate; PipeWire's
// adapter converts to the graph rate and the device channel layout):
//   - Output: a playback stream, linked to the default sink or to the node
//     named by Config::output_uid (target.object = node.name).
//   - Input (only when Config::input_enabled): a separate capture stream
//     with the source's channel count. Its process callback writes into a
//     FrameRing (frame_ring.hpp) that the playback callback drains by
//     exactly its cycle length (engine contract: equal input/output
//     length). Both streams are in one node.group, so PipeWire drives them
//     from the same clock: the ring only absorbs their order within a cycle,
//     and it re-syncs when a glitch (one stream missing a cycle) would
//     otherwise shift every later recording by a quantum. If the input
//     cannot be opened the device runs output-only and status().input_error
//     says why.
// Both streams use PW_STREAM_FLAG_RT_PROCESS: callbacks run on PipeWire's
// data thread (real-time when RTKit / the rt module allows it). stop()
// destroys the streams and the context, which joins that thread, so no
// callback runs afterwards.
//
// Device ids (DeviceInfo::uid) are PipeWire node names (node.name), which
// are stable across restarts (e.g. "alsa_output.pci-0000_00_1f.3.analog-stereo").
// Enumeration, validation and the default rate use a short-lived connection
// (registry + "default" and "settings" metadata, one round trip) and never
// open a stream. Without a reachable PipeWire server there are no PipeWire
// devices and start() fails with SE_ERR_DEVICE; linux_audio_device.cpp then
// uses the ALSA backend for the system default.
//
// libpipewire is loaded with dlopen (like libasound for MIDI), not linked:
// the app must start, and play through ALSA, on systems without PipeWire.
// Only the development headers are needed to build.
//
// Rates: the graph runs at clock.rate (default 48 kHz; clock.allowed-rates
// lets PipeWire switch). Config::device_rate requests a graph rate through
// node.rate; Config::buffer_size requests a quantum through node.latency.
// PipeWire may not honour either; status() reports what is in effect.
#if defined(__linux__) && defined(DSTUDIO_HAVE_PIPEWIRE)

#include "audio_device.hpp"
#include "dstudio_engine.h"
#include "linux_audio.hpp"

#include "frame_ring.hpp"

#include <pipewire/extensions/metadata.h>
#include <pipewire/pipewire.h>
#include <spa/param/audio/format-utils.h>
#include <spa/pod/builder.h>
#include <spa/utils/dict.h>

// The pw_*_add_listener / pw_registry_bind / pw_core_* method macros expand
// to C casts (and to calls GCC's path-sensitive null check cannot follow)
// in this file.
#pragma GCC diagnostic ignored "-Wold-style-cast"
#pragma GCC diagnostic ignored "-Wnull-dereference"

#include <algorithm>
#include <atomic>
#include <cerrno>
#include <chrono>
#include <cmath>
#include <cstdlib>
#include <cstring>
#include <dlfcn.h>
#include <memory>
#include <mutex>
#include <thread>

namespace dstudio {
namespace {

constexpr uint32_t kMaxDeviceChannels = 16;
constexpr uint32_t kMaxCycleFrames = 8192; // largest quantum PipeWire uses

// The libpipewire functions this backend calls, resolved from
// libpipewire-0.3.so.0. (Everything else it uses, such as the
// pw_core_add_listener family, is inline in the headers and dispatches
// through the server objects' method tables.)
struct PwApi {
#define DSTUDIO_PW_FNS(X)                                                                                            \
    X(pw_init) X(pw_main_loop_new) X(pw_main_loop_get_loop) X(pw_main_loop_run) X(pw_main_loop_quit)                \
    X(pw_main_loop_destroy) X(pw_context_new) X(pw_context_connect) X(pw_context_destroy) X(pw_core_disconnect)     \
    X(pw_proxy_destroy) X(pw_properties_new) X(pw_properties_set) X(pw_properties_setf) X(pw_stream_new)            \
    X(pw_stream_add_listener) X(pw_stream_connect) X(pw_stream_dequeue_buffer) X(pw_stream_queue_buffer)            \
    X(pw_stream_destroy) X(pw_stream_get_time_n) X(pw_thread_loop_new) X(pw_thread_loop_get_loop)                   \
    X(pw_thread_loop_start) X(pw_thread_loop_stop) X(pw_thread_loop_destroy) X(pw_thread_loop_lock)                 \
    X(pw_thread_loop_unlock) X(pw_thread_loop_timed_wait) X(pw_thread_loop_signal)
#define DSTUDIO_PW_FIELD(name) decltype(&::name) name = nullptr;
    DSTUDIO_PW_FNS(DSTUDIO_PW_FIELD)
#undef DSTUDIO_PW_FIELD
};

// Loads libpipewire and calls pw_init once per process. Null when the
// library (or one of its functions) is missing.
const PwApi* pw_api() {
    static const PwApi* const loaded = []() -> const PwApi* {
        void* lib = dlopen("libpipewire-0.3.so.0", RTLD_NOW | RTLD_LOCAL);
        if (lib == nullptr) return nullptr;
        static PwApi a;
        bool ok = true;
#define DSTUDIO_PW_LOAD(name)                                                   \
    a.name = reinterpret_cast<decltype(a.name)>(dlsym(lib, #name));              \
    ok = ok && a.name != nullptr;
        DSTUDIO_PW_FNS(DSTUDIO_PW_LOAD)
#undef DSTUDIO_PW_LOAD
        if (!ok) return nullptr; // the library stays loaded for the process
        a.pw_init(nullptr, nullptr);
        return &a;
    }();
    return loaded;
}

// The loaded API. Only called once pw_api() returned non-null (every entry
// point checks it first).
const PwApi& pw() { return *pw_api(); }

// Formats a rate for error messages, e.g. "48000 Hz".
std::string rate_str(double r) { return std::to_string(static_cast<long>(std::lround(r))) + " Hz"; }

// ---------------------------------------------------------------- enumeration

// One audio sink or source node found in the registry.
struct PwNode {
    uint32_t id = 0;
    std::string name;        // node.name (our uid)
    std::string description; // node.description
    bool sink = false;       // media.class Audio/Sink (else Audio/Source)
    uint32_t channels = 0;   // audio.channels / audio.position
};

// What one enumeration pass found: the audio nodes, the default sink and
// source, and the graph's clock rate and allowed rates.
struct PwSnapshot {
    bool connected = false;
    std::vector<PwNode> nodes;
    std::string default_sink, default_source; // node names
    double clock_rate = 0.0;
    std::vector<double> allowed_rates;
};

// Extracts the node name from a "default" metadata value
// ({"name":"alsa_output.foo"} -> alsa_output.foo); "" if there is none.
std::string json_name(const char* value) {
    // {"name":"alsa_output.foo"} -> alsa_output.foo
    if (value == nullptr) return {};
    const char* k = std::strstr(value, "\"name\"");
    if (k == nullptr) return {};
    const char* q1 = std::strchr(k + 6, '"');
    if (q1 == nullptr) return {};
    const char* q2 = std::strchr(q1 + 1, '"');
    if (q2 == nullptr) return {};
    return std::string(q1 + 1, static_cast<std::size_t>(q2 - q1 - 1));
}

// Parses every number in a list such as "[ 44100 48000 ]" (clock.allowed-rates).
std::vector<double> parse_numbers(const char* s) {
    std::vector<double> out;
    if (s == nullptr) return out;
    while (*s != '\0') {
        if (*s >= '0' && *s <= '9') {
            char* end = nullptr;
            out.push_back(std::strtod(s, &end));
            s = end;
        } else {
            ++s;
        }
    }
    return out;
}

// Channel count from a node's properties: audio.channels, else the number of
// names in audio.position; 0 if neither is present.
uint32_t channels_from_props(const spa_dict* props) {
    if (props == nullptr) return 0;
    if (const char* c = spa_dict_lookup(props, "audio.channels")) return static_cast<uint32_t>(std::atoi(c));
    if (const char* p = spa_dict_lookup(props, "audio.position")) {
        // Count channel names in any of the forms PipeWire uses: "FL,FR",
        // "[ FL FR ]", "[ \"FL\", \"FR\" ]" (separators: , [ ] quotes, space).
        uint32_t n = 0;
        bool in_name = false;
        for (; *p != '\0'; ++p) {
            const bool sep = *p == ',' || *p == '[' || *p == ']' || *p == '"' || *p == ' ' || *p == '\t' || *p == '\n';
            if (!sep && !in_name) ++n;
            in_name = !sep;
        }
        return n;
    }
    return 0;
}

// Short-lived PipeWire connection that collects a PwSnapshot. Never opens a
// stream; a 2 s watchdog keeps an unresponsive server from hanging the caller.
class Enumerator {
public:
    // One connection, registry + metadata, two round trips (globals, then
    // the bound nodes' info and metadata properties). ~ms with a server.
    PwSnapshot run() {
        PwSnapshot snap;
        if (pw_api() == nullptr) return snap;
        loop_ = pw().pw_main_loop_new(nullptr);
        if (loop_ == nullptr) return snap;
        context_ = pw().pw_context_new(pw().pw_main_loop_get_loop(loop_), nullptr, 0);
        if (context_ != nullptr) core_ = pw().pw_context_connect(context_, nullptr, 0);
        if (core_ != nullptr) {
            snap_ = &snap;
            snap.connected = true;
            core_events_.version = PW_VERSION_CORE_EVENTS;
            core_events_.done = &Enumerator::on_done;
            core_events_.error = &Enumerator::on_error;
            pw_core_add_listener(core_, &core_listener_, &core_events_, this);
            registry_ = pw_core_get_registry(core_, PW_VERSION_REGISTRY, 0);
            if (registry_ == nullptr) {
                spa_hook_remove(&core_listener_);
                pw().pw_core_disconnect(core_);
                pw().pw_context_destroy(context_);
                pw().pw_main_loop_destroy(loop_);
                snap.connected = false;
                return snap;
            }
            registry_events_.version = PW_VERSION_REGISTRY_EVENTS;
            registry_events_.global = &Enumerator::on_global;
            pw_registry_add_listener(registry_, &registry_listener_, &registry_events_, this);
            // Watchdog: a server that never answers must not hang the caller.
            timer_ = pw_loop_add_timer(pw().pw_main_loop_get_loop(loop_), &Enumerator::on_timeout, this);
            timespec value{2, 0}, interval{0, 0};
            pw_loop_update_timer(pw().pw_main_loop_get_loop(loop_), timer_, &value, &interval, false);
            roundtrip(); // globals
            roundtrip(); // node info + metadata properties
            for (auto& n : bound_nodes_) {
                spa_hook_remove(&n->listener);
                pw().pw_proxy_destroy(static_cast<pw_proxy*>(n->proxy));
            }
            bound_nodes_.clear();
            for (auto& m : metadata_) {
                spa_hook_remove(&m->listener);
                pw().pw_proxy_destroy(static_cast<pw_proxy*>(m->proxy));
            }
            metadata_.clear();
            spa_hook_remove(&registry_listener_);
            pw().pw_proxy_destroy(reinterpret_cast<pw_proxy*>(registry_));
            spa_hook_remove(&core_listener_);
            pw().pw_core_disconnect(core_);
        }
        if (context_ != nullptr) pw().pw_context_destroy(context_);
        pw().pw_main_loop_destroy(loop_);
        return snap;
    }

    Enumerator() = default;
    Enumerator(const Enumerator&) = delete;
    Enumerator& operator=(const Enumerator&) = delete;

private:
    // A node bound for its info (full properties).
    struct NodeBind {
        Enumerator* owner = nullptr;
        uint32_t id = 0;
        void* proxy = nullptr;
        spa_hook listener{};
        pw_node_events events{};
    };
    // A bound "default" or "settings" metadata object.
    struct Metadata {
        Enumerator* owner = nullptr;
        bool defaults = false; // "default" (else "settings")
        void* proxy = nullptr;
        spa_hook listener{};
        pw_metadata_events events{};
    };

    // Runs the loop until the server has answered everything sent so far (or
    // the watchdog fires).
    void roundtrip() {
        if (timed_out_) return;
        pending_ = pw_core_sync(core_, PW_ID_CORE, 0);
        pw().pw_main_loop_run(loop_);
    }

    // Core "done" event: ends the round trip whose sync sequence matches.
    static void on_done(void* data, uint32_t id, int seq) {
        auto* self = static_cast<Enumerator*>(data);
        if (id == PW_ID_CORE && seq == self->pending_) pw().pw_main_loop_quit(self->loop_);
    }
    // Core error: ends the current round trip so the caller does not wait for it.
    static void on_error(void* data, uint32_t id, int, int, const char*) {
        auto* self = static_cast<Enumerator*>(data);
        if (id == PW_ID_CORE) pw().pw_main_loop_quit(self->loop_);
    }
    // Watchdog: the server did not answer in time; stop waiting.
    static void on_timeout(void* data, uint64_t) {
        auto* self = static_cast<Enumerator*>(data);
        self->timed_out_ = true;
        pw().pw_main_loop_quit(self->loop_);
    }

    // Registry event: records Audio/Sink and Audio/Source nodes (binding those
    // whose channel count needs the full node info) and binds the "default" and
    // "settings" metadata.
    static void on_global(void* data, uint32_t id, uint32_t, const char* type, uint32_t, const spa_dict* props) {
        auto* self = static_cast<Enumerator*>(data);
        if (props == nullptr || type == nullptr) return;
        if (std::strcmp(type, PW_TYPE_INTERFACE_Node) == 0) {
            const char* cls = spa_dict_lookup(props, PW_KEY_MEDIA_CLASS);
            const char* name = spa_dict_lookup(props, PW_KEY_NODE_NAME);
            if (cls == nullptr || name == nullptr) return;
            // Virtual sources too: filter chains (noise suppression, echo
            // cancel) and loopbacks are inputs people record from.
            const bool sink = std::strcmp(cls, "Audio/Sink") == 0;
            const bool source = std::strcmp(cls, "Audio/Source") == 0 || std::strcmp(cls, "Audio/Source/Virtual") == 0;
            if (!sink && !source) return;
            PwNode n;
            n.id = id;
            n.name = name;
            const char* desc = spa_dict_lookup(props, PW_KEY_NODE_DESCRIPTION);
            n.description = desc != nullptr ? desc : name;
            n.sink = sink;
            n.channels = channels_from_props(props);
            const bool need_info = n.channels == 0;
            self->snap_->nodes.push_back(std::move(n));
            if (need_info) {
                // Full properties (audio.channels/position) arrive with the
                // node info: bind it for the second round trip.
                auto nb = std::make_unique<NodeBind>();
                nb->owner = self;
                nb->id = id;
                nb->proxy = pw_registry_bind(self->registry_, id, PW_TYPE_INTERFACE_Node, PW_VERSION_NODE, 0);
                if (nb->proxy == nullptr) return;
                nb->events.version = PW_VERSION_NODE_EVENTS;
                nb->events.info = &Enumerator::on_node_info;
                pw_node_add_listener(static_cast<pw_node*>(nb->proxy), &nb->listener, &nb->events, nb.get());
                self->bound_nodes_.push_back(std::move(nb));
            }
        } else if (std::strcmp(type, PW_TYPE_INTERFACE_Metadata) == 0) {
            const char* mname = spa_dict_lookup(props, "metadata.name");
            if (mname == nullptr) return;
            const bool defaults = std::strcmp(mname, "default") == 0;
            if (!defaults && std::strcmp(mname, "settings") != 0) return;
            auto m = std::make_unique<Metadata>();
            m->owner = self;
            m->defaults = defaults;
            m->proxy = pw_registry_bind(self->registry_, id, PW_TYPE_INTERFACE_Metadata, PW_VERSION_METADATA, 0);
            if (m->proxy == nullptr) return;
            m->events.version = PW_VERSION_METADATA_EVENTS;
            m->events.property = &Enumerator::on_property;
            pw_metadata_add_listener(static_cast<pw_metadata*>(m->proxy), &m->listener, &m->events, m.get());
            self->metadata_.push_back(std::move(m));
        }
    }

    // Node info event: fills in the channel count of a node whose registry
    // properties did not include one.
    static void on_node_info(void* data, const pw_node_info* info) {
        auto* nb = static_cast<NodeBind*>(data);
        if (info == nullptr) return;
        for (PwNode& n : nb->owner->snap_->nodes)
            if (n.id == nb->id && n.channels == 0) n.channels = channels_from_props(info->props);
    }

    // Metadata property event: reads the default sink/source ("default") and
    // the clock rate and allowed rates ("settings").
    static int on_property(void* data, uint32_t subject, const char* key, const char*, const char* value) {
        auto* m = static_cast<Metadata*>(data);
        if (subject != PW_ID_CORE || key == nullptr) return 0;
        PwSnapshot& s = *m->owner->snap_;
        if (m->defaults) {
            if (std::strcmp(key, "default.audio.sink") == 0) s.default_sink = json_name(value);
            else if (std::strcmp(key, "default.audio.source") == 0) s.default_source = json_name(value);
        } else if (value != nullptr) {
            if (std::strcmp(key, "clock.rate") == 0 || std::strcmp(key, "clock.force-rate") == 0) {
                const double r = std::atof(value);
                if (r > 0 && (s.clock_rate == 0.0 || std::strcmp(key, "clock.force-rate") == 0)) s.clock_rate = r;
            } else if (std::strcmp(key, "clock.allowed-rates") == 0) {
                s.allowed_rates = parse_numbers(value);
            }
        }
        return 0;
    }

    pw_main_loop* loop_ = nullptr;
    pw_context* context_ = nullptr;
    pw_core* core_ = nullptr;
    pw_registry* registry_ = nullptr;
    spa_source* timer_ = nullptr;
    spa_hook core_listener_{}, registry_listener_{};
    pw_core_events core_events_{};
    pw_registry_events registry_events_{};
    std::vector<std::unique_ptr<NodeBind>> bound_nodes_;
    std::vector<std::unique_ptr<Metadata>> metadata_;
    PwSnapshot* snap_ = nullptr;
    int pending_ = 0;
    bool timed_out_ = false;
};

// Serialises enumerations (one PipeWire connection at a time).
std::mutex g_enum_mutex;

// Enumerates the server and fills in defaults: 48 kHz when the clock rate
// is unknown, and the clock rate is always among the (sorted) allowed rates.
PwSnapshot snapshot() {
    std::lock_guard lock(g_enum_mutex);
    Enumerator e;
    PwSnapshot s = e.run();
    if (s.clock_rate <= 0.0) s.clock_rate = 48000.0;
    if (std::find(s.allowed_rates.begin(), s.allowed_rates.end(), s.clock_rate) == s.allowed_rates.end())
        s.allowed_rates.push_back(s.clock_rate);
    std::sort(s.allowed_rates.begin(), s.allowed_rates.end());
    return s;
}

// Finds the sink (or source) named `uid`; an empty uid means the default
// node, or the first node of that kind when there is no default. nullptr if
// none matches.
const PwNode* find_node(const PwSnapshot& s, const std::string& uid, bool sink) {
    const std::string& want = uid.empty() ? (sink ? s.default_sink : s.default_source) : uid;
    for (const PwNode& n : s.nodes)
        if (n.sink == sink && n.name == want) return &n;
    if (uid.empty()) // no default metadata: first node of that kind
        for (const PwNode& n : s.nodes)
            if (n.sink == sink) return &n;
    return nullptr;
}

// ---------------------------------------------------------------- streams

// AudioDevice on PipeWire: a playback stream plus an optional capture stream
// feeding a FrameRing, both driven by a pw_thread_loop.
class PipeWireDevice final : public AudioDevice {
public:
    // Stops the streams and joins the data thread.
    ~PipeWireDevice() override { stop(); }

    // Checks a configuration against a fresh enumeration without opening a stream.
    int32_t validate(const Config& c, std::string& err) const override {
        const PwSnapshot s = snapshot();
        return validate_with(s, c, err);
    }

    // Opens the capture stream (when input is enabled) and the playback stream,
    // waits for playback to start, then fills status(). An input that fails to
    // start leaves the device running output-only with status().input_error set.
    int32_t start(const Config& config, Callback* callback, std::string& err) override {
        if (loop_ != nullptr) {
            err = "device already started";
            return SE_ERR_STATE;
        }
        if (callback == nullptr || config.channels == 0 || config.channels > kMaxDeviceChannels) {
            err = "invalid device configuration";
            return SE_ERR_INVALID_ARG;
        }
        if (pw_api() == nullptr) {
            err = "PipeWire is not installed (libpipewire-0.3.so.0 not found)";
            return SE_ERR_DEVICE;
        }
        const PwSnapshot snap = snapshot();
        if (!snap.connected) {
            err = "cannot connect to the PipeWire server (is pipewire running?)";
            return SE_ERR_DEVICE;
        }
        int32_t rc = validate_with(snap, config, err);
        if (rc != SE_OK) return rc;
        callback_ = callback;
        config_ = config;
        status_ = Status{};
        out_channels_ = config.channels;
        out_planar_.assign(static_cast<std::size_t>(out_channels_) * kMaxCycleFrames, 0.0f);
        for (uint32_t c = 0; c < out_channels_; ++c)
            out_ptrs_[c] = out_planar_.data() + static_cast<std::size_t>(c) * kMaxCycleFrames;

        loop_ = pw().pw_thread_loop_new("dstudio-pipewire", nullptr);
        if (loop_ == nullptr) return fail("pw_thread_loop_new failed", err);
        context_ = pw().pw_context_new(pw().pw_thread_loop_get_loop(loop_), nullptr, 0);
        if (context_ == nullptr) return fail("pw_context_new failed", err);
        if (pw().pw_thread_loop_start(loop_) < 0) return fail("pw_thread_loop_start failed", err);
        pw().pw_thread_loop_lock(loop_);
        core_ = pw().pw_context_connect(context_, nullptr, 0);
        if (core_ == nullptr) {
            pw().pw_thread_loop_unlock(loop_);
            return fail("cannot connect to the PipeWire server", err);
        }
        const PwNode* out_node = find_node(snap, config.output_uid, true);
        // Input first (its ring must exist before playback runs).
        std::string input_error;
        if (config.input_enabled) {
            const PwNode* in_node = find_node(snap, config.input_uid, false);
            in_channels_ = std::clamp<uint32_t>(in_node != nullptr && in_node->channels > 0 ? in_node->channels : 2, 1,
                                                kMaxDeviceInputChannels);
            in_planar_.assign(static_cast<std::size_t>(in_channels_) * kMaxCycleFrames, 0.0f);
            for (uint32_t c = 0; c < in_channels_; ++c)
                in_ptrs_[c] = in_planar_.data() + static_cast<std::size_t>(c) * kMaxCycleFrames;
            const uint32_t quantum = requested_quantum();
            ring_target_ = 2 * quantum;
            // Both streams share one driver (node.group below), so the fill
            // cannot drift: re-sync after any glitch that shifts it by more
            // than half a quantum (frame_ring.hpp).
            ring_.reset(in_channels_, 8 * kMaxCycleFrames, ring_target_, 4 * quantum, quantum / 2);
            make_stream(false, in_node != nullptr ? in_node->name : std::string(), in_channels_, input_error);
            in_name_ = in_node != nullptr ? in_node->description : std::string("default input");
        }
        make_stream(true, out_node != nullptr ? out_node->name : std::string(), out_channels_, err);
        if (playback_.stream == nullptr) {
            pw().pw_thread_loop_unlock(loop_);
            teardown();
            return SE_ERR_DEVICE;
        }
        // Wait (up to ~3 s) for the playback stream to run.
        const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(3);
        while (playback_.state.load() != PW_STREAM_STATE_STREAMING && playback_.state.load() != PW_STREAM_STATE_ERROR &&
               std::chrono::steady_clock::now() < deadline)
            pw().pw_thread_loop_timed_wait(loop_, 1);
        if (playback_.state.load() != PW_STREAM_STATE_STREAMING) {
            err = "the PipeWire playback stream did not start" +
                  (playback_.error.empty() ? std::string(" (no output device?)") : ": " + playback_.error);
            pw().pw_thread_loop_unlock(loop_);
            teardown();
            return SE_ERR_DEVICE;
        }
        if (capture_.stream != nullptr) {
            const auto in_deadline = std::chrono::steady_clock::now() + std::chrono::seconds(2);
            while (capture_.state.load() != PW_STREAM_STATE_STREAMING && capture_.state.load() != PW_STREAM_STATE_ERROR &&
                   std::chrono::steady_clock::now() < in_deadline)
                pw().pw_thread_loop_timed_wait(loop_, 1);
            if (capture_.state.load() != PW_STREAM_STATE_STREAMING) {
                input_error = "the PipeWire capture stream did not start" +
                              (capture_.error.empty() ? std::string() : ": " + capture_.error);
                destroy_stream(capture_);
            }
        } else if (config.input_enabled && input_error.empty()) {
            input_error = "the PipeWire capture stream could not be created";
        }
        input_live_.store(capture_.stream != nullptr, std::memory_order_release);
        pw().pw_thread_loop_unlock(loop_);

        // Status: graph rate, quantum and delays once timing is valid.
        status_.output_name = out_node != nullptr ? out_node->description : std::string("default output");
        status_.output_channels = out_node != nullptr && out_node->channels > 0 ? out_node->channels : out_channels_;
        status_.input_error = input_error;
        fill_timing(snap);
        if (capture_.stream != nullptr) {
            status_.input_open = true;
            status_.input_name = in_name_;
            status_.input_channels = in_channels_;
        }
        running_.store(true, std::memory_order_release);
        return SE_OK;
    }

    // Destroys the streams and joins the data thread; no callback runs afterwards.
    int32_t stop() override {
        running_.store(false, std::memory_order_release);
        teardown();
        return SE_OK;
    }

    // True after a successful start() while the playback stream is streaming.
    bool is_running() const override {
        return running_.load(std::memory_order_acquire) && playback_.state.load() == PW_STREAM_STATE_STREAMING;
    }
    // The rates, buffer size, latencies and device names in effect since start().
    Status status() const override { return status_; }

private:
    // One pw_stream with its listener and last reported state.
    struct StreamSlot {
        pw_stream* stream = nullptr;
        spa_hook listener{};
        pw_stream_events events{};
        std::atomic<int> state{PW_STREAM_STATE_UNCONNECTED};
        std::string error; // written under the thread-loop lock
        PipeWireDevice* owner = nullptr;
        bool playback = false;
    };

    // Checks the output and input devices, the requested graph rate (against
    // clock.allowed-rates) and the buffer size against an enumeration snapshot.
    static int32_t validate_with(const PwSnapshot& s, const Config& c, std::string& err) {
        const PwNode* out = find_node(s, c.output_uid, true);
        if (out == nullptr) {
            err = c.output_uid.empty() ? "no default output device" : "output device '" + c.output_uid + "' not found";
            if (!s.connected) err += " (cannot connect to the PipeWire server)";
            return SE_ERR_NOT_FOUND;
        }
        if (!std::isfinite(c.device_rate) || c.device_rate < 0.0) {
            err = "sample_rate must be finite and >= 0";
            return SE_ERR_INVALID_ARG;
        }
        if (c.device_rate > 0.0 &&
            std::none_of(s.allowed_rates.begin(), s.allowed_rates.end(),
                         [&](double r) { return std::fabs(r - c.device_rate) < 0.5; })) {
            err = "the PipeWire graph does not allow " + rate_str(c.device_rate) +
                  " (clock.allowed-rates); it runs at " + rate_str(s.clock_rate);
            return SE_ERR_INVALID_ARG;
        }
        if (c.input_enabled && find_node(s, c.input_uid, false) == nullptr) {
            err = c.input_uid.empty() ? "no default input device" : "input device '" + c.input_uid + "' not found";
            return SE_ERR_NOT_FOUND;
        }
        if (c.buffer_size != 0 && (c.buffer_size < 16 || c.buffer_size > 4096)) {
            err = "buffer_size must be 0 or 16..4096";
            return SE_ERR_INVALID_ARG;
        }
        return SE_OK;
    }

    // Quantum to request through node.latency: the configured buffer size, else
    // the preferred block (at least 256).
    uint32_t requested_quantum() const {
        return config_.buffer_size != 0 ? config_.buffer_size : std::max<uint32_t>(config_.preferred_block, 256);
    }

    // Records `what` in `err`, tears everything down and returns SE_ERR_DEVICE.
    int32_t fail(const char* what, std::string& err) {
        err = what;
        teardown();
        return SE_ERR_DEVICE;
    }

    // The playback or capture slot. Requires the thread-loop lock.
    StreamSlot& slot(bool playback) { return playback ? playback_ : capture_; }

    // Creates and connects the playback or capture stream (float32 interleaved at
    // the engine rate) to `target`, or to the default node when it is empty. On
    // failure the slot's stream is null and `err` says why. Requires the
    // thread-loop lock.
    StreamSlot& make_stream(bool playback, const std::string& target, uint32_t channels, std::string& err) {
        StreamSlot& s = slot(playback);
        s.owner = this;
        s.playback = playback;
        s.error.clear();
        s.state.store(PW_STREAM_STATE_UNCONNECTED);
        const std::string latency =
            std::to_string(requested_quantum()) + "/" + std::to_string(static_cast<long>(config_.sample_rate));
        pw_properties* props = pw().pw_properties_new(PW_KEY_MEDIA_TYPE, "Audio", PW_KEY_MEDIA_CATEGORY,
                                                 playback ? "Playback" : "Capture", PW_KEY_MEDIA_ROLE, "Production",
                                                 PW_KEY_APP_NAME, "Device", PW_KEY_NODE_LATENCY, latency.c_str(),
                                                 nullptr);
        if (!target.empty()) pw().pw_properties_set(props, PW_KEY_TARGET_OBJECT, target.c_str());
        // One group for both streams: PipeWire runs them from the same driver
        // (resampling the other device if input and output are different
        // hardware), so the input ring sees one clock and never drifts.
        pw().pw_properties_setf(props, PW_KEY_NODE_GROUP, "device-studio-%p", static_cast<const void*>(this));
        if (config_.device_rate > 0.0)
            pw().pw_properties_set(props, PW_KEY_NODE_RATE,
                              ("1/" + std::to_string(static_cast<long>(std::lround(config_.device_rate)))).c_str());
        s.stream = pw().pw_stream_new(core_, playback ? "Device output" : "Device input", props);
        if (s.stream == nullptr) {
            err = std::string("pw_stream_new failed: ") + std::strerror(errno);
            return s;
        }
        s.events.version = PW_VERSION_STREAM_EVENTS;
        s.events.state_changed = &PipeWireDevice::on_state;
        s.events.process = playback ? &PipeWireDevice::on_playback : &PipeWireDevice::on_capture;
        pw().pw_stream_add_listener(s.stream, &s.listener, &s.events, &s);

        uint8_t buffer[1024];
        spa_pod_builder b;
        spa_pod_builder_init(&b, buffer, sizeof buffer);
        spa_audio_info_raw info{};
        info.format = SPA_AUDIO_FORMAT_F32;
        info.rate = static_cast<uint32_t>(std::lround(config_.sample_rate));
        info.channels = channels;
        if (channels == 2) {
            info.position[0] = SPA_AUDIO_CHANNEL_FL;
            info.position[1] = SPA_AUDIO_CHANNEL_FR;
        } else if (channels == 1) {
            info.position[0] = SPA_AUDIO_CHANNEL_MONO;
        } else {
            info.flags = SPA_AUDIO_FLAG_UNPOSITIONED;
        }
        const spa_pod* params[1] = {spa_format_audio_raw_build(&b, SPA_PARAM_EnumFormat, &info)};
        const auto flags = static_cast<pw_stream_flags>(PW_STREAM_FLAG_AUTOCONNECT | PW_STREAM_FLAG_MAP_BUFFERS |
                                                        PW_STREAM_FLAG_RT_PROCESS);
        const int r = pw().pw_stream_connect(s.stream, playback ? PW_DIRECTION_OUTPUT : PW_DIRECTION_INPUT, PW_ID_ANY,
                                        flags, params, 1);
        if (r < 0) {
            err = std::string("pw_stream_connect failed: ") + std::strerror(-r);
            destroy_stream(s);
        }
        return s;
    }

    // Disconnects and destroys a slot's stream, if any. Requires the
    // thread-loop lock (or a stopped loop).
    static void destroy_stream(StreamSlot& s) {
        if (s.stream == nullptr) return;
        spa_hook_remove(&s.listener);
        pw().pw_stream_destroy(s.stream);
        s.stream = nullptr;
        s.state.store(PW_STREAM_STATE_UNCONNECTED);
    }

    // Destroys both streams, stops the thread loop and releases the core, the
    // context (which joins the data thread) and the loop. Safe to call twice.
    void teardown() {
        input_live_.store(false, std::memory_order_release);
        if (loop_ != nullptr) {
            pw().pw_thread_loop_lock(loop_);
            destroy_stream(capture_);
            destroy_stream(playback_);
            pw().pw_thread_loop_unlock(loop_);
            pw().pw_thread_loop_stop(loop_);
        }
        if (core_ != nullptr) pw().pw_core_disconnect(core_);
        core_ = nullptr;
        if (context_ != nullptr) pw().pw_context_destroy(context_); // joins the data thread
        context_ = nullptr;
        if (loop_ != nullptr) pw().pw_thread_loop_destroy(loop_);
        loop_ = nullptr;
    }

    // Fills status() with the graph rate, quantum and latencies, polling up to
    // ~0.5 s for the playback stream's timing to become valid.
    void fill_timing(const PwSnapshot& snap) {
        // Timing becomes valid after the first cycles; poll briefly.
        pw_time t{};
        bool valid = false;
        for (int i = 0; i < 50 && !valid; ++i) {
            pw().pw_thread_loop_lock(loop_);
            valid = pw().pw_stream_get_time_n(playback_.stream, &t, sizeof t) == 0 && t.rate.denom > 0;
            pw().pw_thread_loop_unlock(loop_);
            if (!valid) std::this_thread::sleep_for(std::chrono::milliseconds(10));
        }
        const double graph_rate = valid ? static_cast<double>(t.rate.denom) / std::max<uint32_t>(t.rate.num, 1)
                                        : snap.clock_rate;
        status_.device_rate = graph_rate;
        const uint32_t quantum = valid && t.size > 0 ? static_cast<uint32_t>(t.size) : requested_quantum();
        status_.buffer_size = quantum;
        // delay: ticks (graph rate) until our next sample is heard; plus
        // the samples queued in our stream.
        const int64_t delay = valid ? std::max<int64_t>(t.delay, 0) : 0;
        status_.output_latency = static_cast<uint32_t>(delay) + quantum;
        if (capture_.stream != nullptr) {
            pw_time ti{};
            int64_t in_delay = 0;
            pw().pw_thread_loop_lock(loop_);
            if (pw().pw_stream_get_time_n(capture_.stream, &ti, sizeof ti) == 0) in_delay = std::max<int64_t>(ti.delay, 0);
            pw().pw_thread_loop_unlock(loop_);
            const double to_dev = graph_rate / config_.sample_rate;
            status_.input_latency =
                static_cast<uint32_t>(in_delay) + quantum + static_cast<uint32_t>(std::lround(ring_target_ * to_dev));
        }
    }

    // Stream state event: records the state (and any error) and wakes start().
    static void on_state(void* data, pw_stream_state, pw_stream_state state, const char* error) {
        auto* s = static_cast<StreamSlot*>(data);
        if (error != nullptr) s->error = error;
        s->state.store(state);
        pw().pw_thread_loop_signal(s->owner->loop_, false);
    }

    // Data thread: the driver cycle being processed, for the input ring
    // (frame_ring.hpp). Both streams share a driver (node.group), so they see
    // the same cycle time (pw_time.now) in one cycle; their `ticks` differ by
    // the stream delays. 0 if unknown.
    static uint64_t cycle_of(pw_stream* stream) noexcept {
        pw_time t{};
        if (pw().pw_stream_get_time_n(stream, &t, sizeof t) != 0 || t.now <= 0) return 0;
        return static_cast<uint64_t>(t.now);
    }

    // Data thread: copies the captured frames (or silence for an empty
    // chunk) into the ring that on_playback drains.
    static void on_capture(void* data) {
        auto* s = static_cast<StreamSlot*>(data);
        PipeWireDevice* self = s->owner;
        pw_buffer* b = pw().pw_stream_dequeue_buffer(s->stream);
        if (b == nullptr) return;
        spa_buffer* buf = b->buffer;
        if (buf->n_datas > 0 && buf->datas[0].chunk != nullptr) {
            const spa_data& d = buf->datas[0];
            const uint32_t stride = static_cast<uint32_t>(sizeof(float)) * self->in_channels_;
            const uint32_t offset = std::min(d.chunk->offset, d.maxsize);
            const uint32_t size = std::min(d.chunk->size, d.maxsize - offset);
            const uint32_t frames = stride > 0 ? size / stride : 0;
            const bool silent = d.data == nullptr || (d.chunk->flags & SPA_CHUNK_FLAG_EMPTY) != 0;
            if (frames > 0)
                self->ring_.write(silent ? nullptr
                                         : reinterpret_cast<const float*>(static_cast<const uint8_t*>(d.data) + offset),
                                  frames, cycle_of(s->stream));
        }
        pw().pw_stream_queue_buffer(s->stream, b);
    }

    // Data thread: renders the whole requested cycle in chunks of at most
    // kMaxCycleFrames, reading the matching input from the ring, and interleaves
    // it into the PipeWire buffer.
    static void on_playback(void* data) {
        auto* s = static_cast<StreamSlot*>(data);
        PipeWireDevice* self = s->owner;
        pw_buffer* b = pw().pw_stream_dequeue_buffer(s->stream);
        if (b == nullptr) return;
        spa_buffer* buf = b->buffer;
        if (buf->n_datas == 0 || buf->datas[0].data == nullptr) {
            pw().pw_stream_queue_buffer(s->stream, b);
            return;
        }
        spa_data& d = buf->datas[0];
        const uint32_t ch = self->out_channels_;
        const uint32_t stride = static_cast<uint32_t>(sizeof(float)) * ch;
        uint32_t frames = d.maxsize / stride;
        if (b->requested > 0) frames = std::min<uint32_t>(frames, static_cast<uint32_t>(b->requested));
        // Render the whole request, in chunks of at most kMaxCycleFrames (the
        // planar scratch size); input is read from the ring chunk by chunk so
        // it stays aligned with the output.
        auto* dst = static_cast<float*>(d.data);
        const bool input = self->input_live_.load(std::memory_order_acquire);
        const uint64_t cycle = input ? cycle_of(s->stream) : 0;
        for (uint32_t done = 0; done < frames;) {
            const uint32_t n = std::min(frames - done, kMaxCycleFrames);
            const float* in_ptrs[kMaxDeviceInputChannels];
            uint32_t n_in = 0;
            if (input) {
                self->ring_.read(self->in_ptrs_, self->in_channels_, n, frames - done - n, cycle);
                for (uint32_t c = 0; c < self->in_channels_; ++c) in_ptrs[c] = self->in_ptrs_[c];
                n_in = self->in_channels_;
            }
            self->callback_->render(self->out_ptrs_, ch, n_in > 0 ? in_ptrs : nullptr, n_in, n);
            float* out = dst + static_cast<std::size_t>(done) * ch;
            for (uint32_t i = 0; i < n; ++i)
                for (uint32_t c = 0; c < ch; ++c) out[i * ch + c] = self->out_ptrs_[c][i];
            done += n;
        }
        d.chunk->offset = 0;
        d.chunk->stride = static_cast<int32_t>(stride);
        d.chunk->size = frames * stride;
        pw().pw_stream_queue_buffer(s->stream, b);
    }

    Callback* callback_ = nullptr;
    Config config_;
    Status status_;
    std::atomic<bool> running_{false};
    pw_thread_loop* loop_ = nullptr;
    pw_context* context_ = nullptr;
    pw_core* core_ = nullptr;
    StreamSlot playback_, capture_;
    std::atomic<bool> input_live_{false};
    std::string in_name_;
    uint32_t out_channels_ = 2, in_channels_ = 0, ring_target_ = 0;
    std::vector<float> out_planar_, in_planar_;
    float* out_ptrs_[kMaxDeviceChannels] = {};
    float* in_ptrs_[kMaxDeviceInputChannels] = {};
    FrameRing ring_;
};

} // namespace

namespace linux_audio {

// A PipeWire stream device (not started).
std::unique_ptr<AudioDevice> create_pipewire_device() { return std::make_unique<PipeWireDevice>(); }

// Lists every PipeWire sink and source as a device (false without a server).
bool pipewire_list(std::vector<DeviceInfo>& out, double& clock_rate) {
    out.clear();
    const PwSnapshot s = snapshot();
    clock_rate = s.connected ? s.clock_rate : 0.0;
    for (const PwNode& n : s.nodes) {
        DeviceInfo d;
        d.uid = n.name;
        d.name = n.description;
        const uint32_t ch = n.channels > 0 ? n.channels : 2;
        if (n.sink) {
            d.output_channels = ch;
            d.is_default_output = n.name == s.default_sink;
        } else {
            d.input_channels = ch;
            d.is_default_input = n.name == s.default_source;
        }
        d.sample_rates = s.allowed_rates;
        out.push_back(std::move(d));
    }
    return s.connected;
}

// One short connection; no registry walk.
bool pipewire_server_up() {
    if (pw_api() == nullptr) return false;
    pw_main_loop* loop = pw().pw_main_loop_new(nullptr);
    if (loop == nullptr) return false;
    bool up = false;
    if (pw_context* ctx = pw().pw_context_new(pw().pw_main_loop_get_loop(loop), nullptr, 0)) {
        if (pw_core* core = pw().pw_context_connect(ctx, nullptr, 0)) {
            up = true;
            pw().pw_core_disconnect(core);
        }
        pw().pw_context_destroy(ctx);
    }
    pw().pw_main_loop_destroy(loop);
    return up;
}

} // namespace linux_audio
} // namespace dstudio

#endif // __linux__ && DSTUDIO_HAVE_PIPEWIRE
