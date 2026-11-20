// Plugin instances (Audio Unit, VST3, CLAP) behind one small interface.
//
// Threading (see engine/README.md "Plugins"):
//  - Creation, state save, latency queries, editors and destruction run on
//    the host's main thread (the thread that publishes sessions).
//  - process() runs on whoever owns the RT path (device callback or an
//    offline render / export holding the RT ownership). It never allocates,
//    locks or does I/O on our side; the plugin's own code is out of our
//    control.
//  - set_parameter() is a test/host hook: main thread, delivered to the
//    processor through a lock-free queue at the next process() call.
#pragma once

#include "../spsc_queue.hpp"
#include "../synth.hpp" // NoteEvent
#include "param_bus.hpp"

#include <atomic>
#include <cstdint>
#include <cstring>
#include <memory>
#include <mutex>
#include <string>
#include <vector>

namespace dstudio {

enum class PluginFormat : uint8_t { AudioUnit = 0, Vst3 = 1, Clap = 2, Builtin = 3 };

class BuiltinInstance;

const char* plugin_format_name(PluginFormat f) noexcept;

// Everything needed to (re)create an instance. `state` is only consulted at
// creation.
struct PluginSpec {
    uint64_t device_id = 0;
    PluginFormat format = PluginFormat::AudioUnit;
    std::string plugin_id;
    std::string path;
    std::string name;  // display name
    std::string track; // track display name ("" if unknown)
    bool instrument = false; // used as a track's instrument plugin
    std::vector<uint8_t> state;

    // Same plugin in the same role (an instance can be reused).
    bool same_plugin(const PluginSpec& o) const noexcept {
        return format == o.format && plugin_id == o.plugin_id && path == o.path && instrument == o.instrument;
    }
};

// A short MIDI channel message at a sample offset inside the block.
struct MidiEvent {
    uint32_t offset = 0;
    uint8_t status = 0;
    uint8_t data1 = 0;
    uint8_t data2 = 0;
    uint8_t _pad = 0;
};

// Transport as seen by plugins for one processed segment.
struct RtTransport {
    int64_t position = 0;     // timeline frame of the segment's first sample
    bool playing = false;
    double sample_rate = 48000.0;
    double tempo_bpm = 120.0; // quarter notes per minute (se_transport_set_tempo), else 120
    // Quarter notes since frame 0 at `position` (constant tempo from 0).
    double beats() const noexcept { return static_cast<double>(position) / sample_rate * tempo_bpm / 60.0; }
};

// Converts a track's note list (timeline frames) into sample-accurate MIDI
// note on/off events, one segment at a time. Mirrors Synth's event model:
// note-on at `start`, note-off at `end`; a change of `transport_gen` or a
// timeline discontinuity sends note-offs for every sounding note (+ CC 123);
// a republish (new graph seq) re-matches sounding notes by (start, pitch),
// releasing the ones that disappeared. RT-safe; mutated only by the RT owner.
class NoteFeeder {
public:
    static constexpr uint32_t kMaxSounding = 128;
    // Writes events sorted by offset (note-offs first at equal offsets);
    // returns the count (events beyond `cap` are dropped).
    uint32_t build(const NoteEvent* notes, uint32_t note_count, uint64_t graph_seq, uint64_t transport_gen,
                   int64_t position, uint32_t frames, bool playing, MidiEvent* out, uint32_t cap) noexcept;
    uint32_t sounding() const noexcept { return count_; }

private:
    struct Sounding {
        int64_t start = 0;
        int64_t end = 0;
        uint8_t pitch = 0;
    };
    Sounding sounding_[kMaxSounding];
    uint32_t count_ = 0;
    uint64_t seen_seq_ = ~uint64_t{0};
    uint64_t seen_gen_ = 0;
    int64_t expected_pos_ = 0;
    bool have_pos_ = false;
};

// An open editor window (platform layer). Main thread only.
class PluginEditor {
public:
    virtual ~PluginEditor() = default;
    virtual bool is_open() const noexcept = 0; // false once the user closed the window
    virtual void close() noexcept = 0;         // idempotent
    virtual void bring_to_front() noexcept = 0;
};

// One automation parameter event inside a processed segment (sorted by
// offset). `value` is format-native: VST3 normalized, CLAP and AU plain.
struct ParamEvent {
    uint32_t offset = 0;
    uint32_t id = 0;
    double value = 0.0;
};

// Parameter description (main thread queries).
struct ParamInfo {
    uint32_t id = 0;
    std::string name;
    std::string group;
    float default_normalized = 0.0f;
    uint32_t steps = 0;
    bool automatable = true;
    double min = 0.0; // native range (VST3: 0..1)
    double max = 1.0;
};

// Parameter change from the main thread to the processor.
struct ParamChange {
    uint32_t id = 0;   // format parameter id (AU parameter id, VST3 ParamID, CLAP clap_id)
    double value = 0;  // VST3: normalized 0..1; AU/CLAP: plain value
};

class PluginInstance {
public:
    static constexpr uint32_t kMaxEvents = 512; // per processed segment
    static constexpr uint32_t kMaxParamEvents = 2048; // automation events per processed segment

    virtual ~PluginInstance() = default;
    PluginInstance(const PluginInstance&) = delete;
    PluginInstance& operator=(const PluginInstance&) = delete;

    const PluginSpec& spec() const noexcept { return spec_; }

    // RT entry point used by render_block: process() unless the instance is
    // suspended for a restart (then effects pass audio through and
    // instruments are silent). `in_process_` / `suspended_` form a Dekker
    // pair with suspend(): either this call sees the suspension or
    // suspend() waits until the call has returned.
    void run(float* l, float* r, uint32_t frames, const MidiEvent* events, uint32_t event_count,
             const RtTransport& transport, const ParamEvent* params = nullptr, uint32_t param_count = 0) noexcept {
        in_process_.store(true, std::memory_order_seq_cst);
        if (!suspended_.load(std::memory_order_seq_cst)) {
            process(l, r, frames, events, event_count, transport, params, param_count);
        } else if (spec_.instrument) {
            std::memset(l, 0, sizeof(float) * frames);
            std::memset(r, 0, sizeof(float) * frames);
        }
        in_process_.store(false, std::memory_order_release);
    }

    // RT. Effects process l/r in place. Instruments overwrite l/r with
    // their output; `events` are this segment's note events, `params` its
    // automation events (sorted by offset, at most kMaxParamEvents).
    virtual void process(float* l, float* r, uint32_t frames, const MidiEvent* events, uint32_t event_count,
                         const RtTransport& transport, const ParamEvent* params, uint32_t param_count) noexcept = 0;

    // ---- restarts and latency (main thread)
    // Set from any thread by the format host when the plugin asks to be
    // restarted (CLAP request_restart, VST3 restartComponent(kLatencyChanged)).
    void request_restart() noexcept { restart_requested_.store(true, std::memory_order_release); }
    bool take_restart_request() noexcept { return restart_requested_.exchange(false, std::memory_order_acq_rel); }
    // Keeps the RT side out of process() until resume(); waits (main thread
    // only) for a process() call in flight to return.
    void suspend() noexcept;
    void resume() noexcept { suspended_.store(false, std::memory_order_seq_cst); }
    // Deactivate + re-activate (format specific), called between suspend()
    // and resume(). Default: nothing to do.
    virtual void restart() noexcept {}
    // Latency as last read with refresh_latency() (what delay compensation
    // uses; guarded by the PluginHost mutex).
    uint32_t cached_latency() const noexcept { return cached_latency_; }
    // Re-reads latency(); true if it changed.
    bool refresh_latency() {
        const uint32_t l = latency();
        const bool changed = l != cached_latency_;
        cached_latency_ = l;
        return changed;
    }

    // Main thread.
    virtual bool save_state(std::vector<uint8_t>& out, std::string& err) = 0;
    virtual uint32_t latency() = 0;
    virtual std::unique_ptr<PluginEditor> open_editor(const std::string& title, std::string& err) = 0;
    // Called on the main thread right before destruction (the RT side can
    // no longer reach the instance).
    virtual void shutdown() noexcept {}
    // CLAP request_callback etc.
    virtual void on_main_thread() noexcept {}
    // Built-in devices (devices/builtin.hpp): the instance itself, else null.
    virtual BuiltinInstance* as_builtin() noexcept { return nullptr; }
    // Main thread, every publish of an existing instance: the device's state
    // as published. Plugins only read state at creation (default: ignore);
    // built-in devices take their current parameter values from it.
    virtual void update_state(const uint8_t* data, std::size_t len) noexcept {
        (void)data;
        (void)len;
    }

    // ---- parameters (main thread; see se_plugin_params)
    // Visible parameters in the plugin's order.
    virtual void params(std::vector<ParamInfo>& out) { out.clear(); }
    // Native range of an automatable target (false: no such parameter).
    virtual bool param_range(uint32_t id, double& min, double& max) {
        (void)id;
        min = 0.0;
        max = 1.0;
        return false;
    }
    virtual bool param_value(uint32_t id, float& normalized) {
        (void)id;
        normalized = 0.0f;
        return false;
    }
    virtual bool param_text(uint32_t id, float normalized, std::string& out) {
        (void)id;
        (void)normalized;
        out.clear();
        return false;
    }
    // CLAP: the plugin asked for a parameter flush (host params.request_flush).
    virtual bool wants_flush() const noexcept { return false; }
    // Runs that flush with the audio-thread role; the caller guarantees
    // nothing processes meanwhile (the engine holds the RT path).
    virtual void flush_params() noexcept {}

    // Test hook: perform an editor-style gesture (begin, set, end) through
    // the format's own notification path, where the host can do that (AU).
    virtual bool simulate_editor_gesture(uint32_t id, float normalized) {
        (void)id;
        (void)normalized;
        return false;
    }

    // Editor-originated parameter changes are reported here (set once by the
    // PluginHost right after creation, before any processing).
    void attach_bus(ParamBus* bus) noexcept { bus_.store(bus, std::memory_order_release); }

    // Main thread: queue a parameter change for the processor (and update
    // the controller side where the format has one). False if the queue is full.
    bool set_parameter(uint32_t id, double value) noexcept;

    // RT: the instrument slot's note feeder.
    NoteFeeder& feeder() noexcept { return feeder_; }
    MidiEvent* event_scratch() noexcept { return events_; }

    // ---- live notes (se_builtin_note, se_preview_note), played at the first
    // frame of the next block on top of the track's notes.
    static constexpr uint32_t kLiveEvents = 256;
    // Main thread (one producer): queue a note, velocity 0 = off. False when
    // the queue is full or the instance plays no notes.
    bool push_note(uint8_t pitch, uint8_t velocity) noexcept {
        if (!plays_notes()) return false;
        MidiEvent e;
        e.status = velocity > 0 ? 0x90 : 0x80;
        e.data1 = pitch & 0x7f;
        e.data2 = velocity > 0 ? (velocity & 0x7f) : 64;
        return live_.push(e);
    }
    virtual bool plays_notes() const noexcept { return spec_.instrument; }

    // ---- chain output meter (the level meter after the device's card).
    // Off by default; while on, the audio thread keeps the peak of the
    // device's output (or of the audio passing it while bypassed).
    void set_output_metering(bool on) noexcept {
        out_metering_.store(on, std::memory_order_relaxed);
        if (!on) {
            out_peak_[0].store(0.0f, std::memory_order_relaxed);
            out_peak_[1].store(0.0f, std::memory_order_relaxed);
        }
    }
    bool output_metering() const noexcept { return out_metering_.load(std::memory_order_relaxed); }
    // RT: raise the held peaks (linear) to at least l / r.
    void store_output_peaks(float l, float r) noexcept {
        for (int c = 0; c < 2; ++c) {
            const float v = c == 0 ? l : r;
            float cur = out_peak_[c].load(std::memory_order_relaxed);
            while (v > cur && !out_peak_[c].compare_exchange_weak(cur, v, std::memory_order_relaxed)) {
            }
        }
    }
    // Main thread: the held peaks since the previous call (reading resets them).
    void take_output_peaks(float* out) noexcept {
        out[0] = out_peak_[0].exchange(0.0f, std::memory_order_relaxed);
        out[1] = out_peak_[1].exchange(0.0f, std::memory_order_relaxed);
    }
    // RT: moves queued live notes to `out` (at most `cap`); returns the count.
    uint32_t take_live(MidiEvent* out, uint32_t cap) noexcept {
        uint32_t n = 0;
        while (n < cap && live_.pop(out[n])) ++n;
        return n;
    }

protected:
    explicit PluginInstance(PluginSpec spec) : spec_(std::move(spec)) {}
    // Main thread hook for set_parameter (controller side). Default: nothing.
    virtual void on_parameter_set(const ParamChange&) noexcept {}
    // RT: next queued parameter change.
    bool pop_param(ParamChange& c) noexcept { return params_.pop(c); }
    bool push_param(const ParamChange& c) noexcept;
    // Any thread: report an editor change, stamped with the transport
    // position (report_change_at: with a known frame).
    void report_change(uint32_t id, float normalized, bool begin, bool end) noexcept {
        if (ParamBus* b = bus_.load(std::memory_order_acquire)) report_change_at(id, normalized, b->position(), begin, end);
    }
    void report_change_at(uint32_t id, float normalized, int64_t frame, bool begin, bool end) noexcept {
        ParamBus* const b = bus_.load(std::memory_order_acquire);
        if (b == nullptr) return;
        ParamChangeRecord r;
        r.device_id = spec_.device_id;
        r.frame = frame;
        r.param_id = id;
        r.normalized = normalized;
        r.gesture_begin = begin;
        r.gesture_end = end;
        b->push(r);
    }
    ParamBus* bus() const noexcept { return bus_.load(std::memory_order_acquire); }

    PluginSpec spec_;

private:
    std::atomic<ParamBus*> bus_{nullptr};

protected:

private:
    std::atomic<bool> in_process_{false};
    std::atomic<bool> suspended_{false};
    std::atomic<bool> restart_requested_{false};
    std::atomic<bool> out_metering_{false};
    std::atomic<float> out_peak_[2]{0.0f, 0.0f};
    uint32_t cached_latency_ = 0;
    std::mutex param_mutex_; // serialises producers (SPSC producer side)
    SpscQueue<ParamChange, 256> params_;
    SpscQueue<MidiEvent, kLiveEvents> live_;
    NoteFeeder feeder_;
    MidiEvent events_[kMaxEvents];
};

// Creates and prepares an instance (main thread): stereo in/out at
// `sample_rate` / `max_block`, `spec.state` applied. Null + `err` on failure.
std::shared_ptr<PluginInstance> create_plugin_instance(const PluginSpec& spec, double sample_rate, uint32_t max_block,
                                                       std::string& err);

// Format factories (each returns null + err on failure or when the format is
// not available on this platform).
std::shared_ptr<PluginInstance> create_au_instance(const PluginSpec& spec, double sample_rate, uint32_t max_block,
                                                   std::string& err);
std::shared_ptr<PluginInstance> create_vst3_instance(const PluginSpec& spec, double sample_rate, uint32_t max_block,
                                                     std::string& err);
std::shared_ptr<PluginInstance> create_clap_instance(const PluginSpec& spec, double sample_rate, uint32_t max_block,
                                                     std::string& err);

// True on the platform's main thread (macOS: pthread_main_np; Linux: the
// initial thread, tid == pid; Windows: the thread that ran the static
// initialisers).
bool on_platform_main_thread() noexcept;

} // namespace dstudio
