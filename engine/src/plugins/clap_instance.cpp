// CLAP hosting: clap_entry -> factory -> create_plugin -> init -> activate.
//
// Threads: the host "main thread" is the thread that created the instance
// (the publishing thread); clap_host_thread_check reports it. The "audio
// thread" is whoever is inside process(), plus shutdown()'s single
// stop_processing call, made after the RT side can no longer reach the
// instance (CLAP allows the audio-thread role to move between threads as
// long as calls never overlap).
#include "plugin_instance.hpp"

#include "plugin_editor.hpp"
#include "plugin_modules.hpp"

#include <clap/clap.h>

#include <algorithm>
#include <atomic>
#include <cmath>
#include <cstring>
#include <thread>

#if defined(__APPLE__)
#include <dispatch/dispatch.h>
#endif
#if defined(__linux__)
#include "../platform/run_loop.hpp"
#endif

namespace dstudio {

namespace {

thread_local bool t_clap_audio_thread = false;

struct AudioThreadScope {
    AudioThreadScope() noexcept { t_clap_audio_thread = true; }
    ~AudioThreadScope() { t_clap_audio_thread = false; }
};

union ClapEventStorage {
    clap_event_header_t header;
    clap_event_note_t note;
    clap_event_midi_t midi;
    clap_event_param_value_t param;
};

const clap_input_events_t kNoInputEvents{
    nullptr, [](const clap_input_events*) -> uint32_t { return 0; },
    [](const clap_input_events*, uint32_t) -> const clap_event_header_t* { return nullptr; }};

class ClapInstance final : public PluginInstance, public std::enable_shared_from_this<ClapInstance> {
public:
    static constexpr uint32_t kMaxParamEvents = 64;

    ClapInstance(PluginSpec spec, double rate, uint32_t max_block)
        : PluginInstance(std::move(spec)), rate_(rate), max_block_(max_block), in_l_(max_block), in_r_(max_block),
          out_l_(max_block), out_r_(max_block), main_thread_(std::this_thread::get_id()),
          event_store_(kMaxEvents + kMaxParamEvents + PluginInstance::kMaxParamEvents) {
        host_.clap_version = CLAP_VERSION;
        host_.host_data = this;
        host_.name = "Device";
        host_.vendor = "Device";
        host_.url = "";
        host_.version = "0.4.0";
        host_.get_extension = &ClapInstance::host_get_extension;
        host_.request_restart = [](const clap_host* h) { self_of(h)->request_restart(); };
        host_.request_process = [](const clap_host*) {};
        host_.request_callback = &ClapInstance::host_request_callback;
    }

    ~ClapInstance() override { shutdown(); }

    bool init(std::string& err) {
        mod_ = load_clap_module(spec_.path, err);
        if (!mod_) return false;
        const auto* factory =
            static_cast<const clap_plugin_factory_t*>(mod_->entry->get_factory(CLAP_PLUGIN_FACTORY_ID));
        if (factory == nullptr) {
            err = "no CLAP plugin factory in " + spec_.path;
            return false;
        }
        plugin_ = factory->create_plugin(factory, &host_, spec_.plugin_id.c_str());
        if (plugin_ == nullptr) {
            err = "CLAP plugin '" + spec_.plugin_id + "' not found in " + spec_.path;
            return false;
        }
        if (!plugin_->init(plugin_)) {
            err = "CLAP plugin init failed";
            plugin_->destroy(plugin_);
            plugin_ = nullptr;
#if defined(__linux__)
            run_loop::remove_all(this); // fds and timers registered during init
#endif
            return false;
        }
        const auto ext = [&](const char* id) { return plugin_->get_extension(plugin_, id); };
        audio_ports_ = static_cast<const clap_plugin_audio_ports_t*>(ext(CLAP_EXT_AUDIO_PORTS));
        note_ports_ = static_cast<const clap_plugin_note_ports_t*>(ext(CLAP_EXT_NOTE_PORTS));
        state_ = static_cast<const clap_plugin_state_t*>(ext(CLAP_EXT_STATE));
        latency_ = static_cast<const clap_plugin_latency_t*>(ext(CLAP_EXT_LATENCY));
        gui_ = static_cast<const clap_plugin_gui_t*>(ext(CLAP_EXT_GUI));
        params_ = static_cast<const clap_plugin_params_t*>(ext(CLAP_EXT_PARAMS));

        // Audio ports: main input (effects) and main output, stereo or mono.
        in_channels_ = port_channels(true);
        out_channels_ = port_channels(false);
        if (out_channels_ == 0 || out_channels_ > 2) {
            err = "unsupported CLAP output layout (need a mono or stereo main output)";
            return false;
        }
        if (!spec_.instrument && (in_channels_ == 0 || in_channels_ > 2)) {
            err = "unsupported CLAP input layout (an effect needs a mono or stereo main input)";
            return false;
        }
        if (in_channels_ > 2) in_channels_ = 0;
        if (note_ports_ != nullptr && note_ports_->count(plugin_, true) > 0) {
            clap_note_port_info_t info{};
            if (note_ports_->get(plugin_, 0, true, &info)) {
                use_midi_ = (info.supported_dialects & CLAP_NOTE_DIALECT_CLAP) == 0 &&
                            (info.supported_dialects & CLAP_NOTE_DIALECT_MIDI) != 0;
                note_port_ = true;
            }
        }

        if (!spec_.state.empty()) {
            if (state_ == nullptr) {
                err = "saved state given but the plugin has no clap.state extension";
                return false;
            }
            if (!load_state(spec_.state)) {
                err = "the plugin rejected the saved state";
                return false;
            }
        }
        spec_.state.clear();
        spec_.state.shrink_to_fit();
        build_param_cache();

        if (!plugin_->activate(plugin_, rate_, 1, max_block_)) {
            err = "CLAP plugin activate failed";
            return false;
        }
        activated_ = true;

        in_ptrs_[0] = in_l_.data();
        in_ptrs_[1] = in_r_.data();
        out_ptrs_[0] = out_l_.data();
        out_ptrs_[1] = out_r_.data();
        in_buf_ = clap_audio_buffer_t{in_ptrs_, nullptr, in_channels_, 0, 0};
        out_buf_ = clap_audio_buffer_t{out_ptrs_, nullptr, out_channels_, 0, 0};
        in_events_.ctx = this;
        in_events_.size = [](const clap_input_events* l) -> uint32_t {
            return static_cast<const ClapInstance*>(l->ctx)->event_count_;
        };
        in_events_.get = [](const clap_input_events* l, uint32_t i) -> const clap_event_header_t* {
            auto* self = static_cast<const ClapInstance*>(l->ctx);
            return i < self->event_count_ ? &self->event_store_[i].header : nullptr;
        };
        out_events_.ctx = this;
        out_events_.try_push = [](const clap_output_events* o, const clap_event_header_t* h) {
            static_cast<ClapInstance*>(o->ctx)->on_output_event(h);
            return true;
        };
        return true;
    }

    void process(float* l, float* r, uint32_t frames, const MidiEvent* events, uint32_t event_count,
                 const RtTransport& t, const ParamEvent* auto_events, uint32_t auto_count) noexcept override {
        AudioThreadScope audio;
        if (!activated_ || failed_) return pass(l, r, frames);
        if (!processing_) {
            if (!plugin_->start_processing(plugin_)) {
                failed_ = true;
                return pass(l, r, frames);
            }
            processing_ = true;
        }
        out_frame_ = t.position;
        // Events: main-thread parameter changes at 0, then automation and
        // notes merged by time (both already sorted).
        event_count_ = 0;
        ParamChange pc;
        while (event_count_ < kMaxParamEvents && pop_param(pc)) push_param_event(0, pc.id, pc.value);
        uint32_t a = 0;
        for (uint32_t i = 0; i < event_count && note_port_; ++i) {
            const MidiEvent& m = events[i];
            for (; a < auto_count && auto_events[a].offset <= m.offset; ++a)
                push_param_event(auto_events[a].offset, auto_events[a].id, auto_events[a].value);
            const uint8_t kind = m.status & 0xF0;
            if (use_midi_) {
                clap_event_midi_t& e = event_store_[event_count_++].midi;
                e.header = {sizeof e, m.offset, CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_MIDI, 0};
                e.port_index = 0;
                e.data[0] = m.status;
                e.data[1] = m.data1;
                e.data[2] = m.data2;
            } else if (kind == 0x90 || kind == 0x80) {
                clap_event_note_t& e = event_store_[event_count_++].note;
                e.header = {sizeof e, m.offset, CLAP_CORE_EVENT_SPACE_ID,
                            static_cast<uint16_t>(kind == 0x90 ? CLAP_EVENT_NOTE_ON : CLAP_EVENT_NOTE_OFF), 0};
                e.note_id = -1;
                e.port_index = 0;
                e.channel = 0;
                e.key = m.data1;
                e.velocity = kind == 0x90 ? m.data2 / 127.0 : 0.0;
            }
        }
        for (; a < auto_count; ++a) push_param_event(auto_events[a].offset, auto_events[a].id, auto_events[a].value);
        transport_ = clap_event_transport_t{};
        transport_.header = {sizeof transport_, 0, CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_TRANSPORT, 0};
        transport_.flags = CLAP_TRANSPORT_HAS_TEMPO | CLAP_TRANSPORT_HAS_BEATS_TIMELINE |
                           CLAP_TRANSPORT_HAS_SECONDS_TIMELINE | CLAP_TRANSPORT_HAS_TIME_SIGNATURE |
                           (t.playing ? static_cast<uint32_t>(CLAP_TRANSPORT_IS_PLAYING) : 0u);
        transport_.tempo = t.tempo_bpm;
        transport_.song_pos_beats = static_cast<clap_beattime>(t.beats() * static_cast<double>(CLAP_BEATTIME_FACTOR));
        transport_.song_pos_seconds = static_cast<clap_sectime>(static_cast<double>(t.position) / t.sample_rate *
                                                                static_cast<double>(CLAP_SECTIME_FACTOR));
        transport_.tsig_num = 4;
        transport_.tsig_denom = 4;

        if (in_channels_ == 2) {
            std::memcpy(in_l_.data(), l, sizeof(float) * frames);
            std::memcpy(in_r_.data(), r, sizeof(float) * frames);
        } else if (in_channels_ == 1) {
            for (uint32_t i = 0; i < frames; ++i) in_l_[i] = 0.5f * (l[i] + r[i]);
        }
        clap_process_t p{};
        p.steady_time = steady_time_;
        p.frames_count = frames;
        p.transport = &transport_;
        p.audio_inputs = in_channels_ > 0 ? &in_buf_ : nullptr;
        p.audio_inputs_count = in_channels_ > 0 ? 1 : 0;
        p.audio_outputs = &out_buf_;
        p.audio_outputs_count = 1;
        p.in_events = &in_events_;
        p.out_events = &out_events_;
        const clap_process_status st = plugin_->process(plugin_, &p);
        steady_time_ += frames;
        if (st == CLAP_PROCESS_ERROR) return pass(l, r, frames);
        std::memcpy(l, out_l_.data(), sizeof(float) * frames);
        std::memcpy(r, out_channels_ > 1 ? out_r_.data() : out_l_.data(), sizeof(float) * frames);
    }

    // ---- parameters (main thread)
    void params(std::vector<ParamInfo>& out) override {
        out.clear();
        if (params_ == nullptr) return;
        const uint32_t n = params_->count(plugin_);
        for (uint32_t i = 0; i < n; ++i) {
            clap_param_info_t pi{};
            if (!params_->get_info(plugin_, i, &pi) || (pi.flags & CLAP_PARAM_IS_HIDDEN) != 0) continue;
            out.push_back(to_info(pi));
        }
    }

    bool param_range(uint32_t id, double& min, double& max) override {
        const CachedParam* c = cached(id);
        if (c == nullptr) return false;
        min = c->min;
        max = c->max;
        return true;
    }

    bool param_value(uint32_t id, float& normalized) override {
        const CachedParam* c = cached(id);
        double v = 0.0;
        if (c == nullptr || params_ == nullptr || !params_->get_value(plugin_, id, &v)) return false;
        normalized = c->normalize(v);
        return true;
    }

    bool param_text(uint32_t id, float normalized, std::string& out) override {
        const CachedParam* c = cached(id);
        if (c == nullptr || params_ == nullptr) return false;
        char buf[256] = {};
        if (!params_->value_to_text(plugin_, id, c->plain(normalized), buf, sizeof buf)) return false;
        out = buf;
        return true;
    }

    bool wants_flush() const noexcept override { return flush_requested_.load(std::memory_order_acquire); }

    void flush_params() noexcept override {
        if (!flush_requested_.exchange(false) || params_ == nullptr || plugin_ == nullptr) return;
        AudioThreadScope audio; // active plugin: flush is an audio-thread call; nothing processes meanwhile
        out_frame_ = bus() != nullptr ? bus()->position() : 0;
        params_->flush(plugin_, &kNoInputEvents, &out_events_);
    }

    bool save_state(std::vector<uint8_t>& out, std::string& err) override {
        if (state_ == nullptr) {
            err = "plugin has no clap.state extension";
            return false;
        }
        out.clear();
        clap_ostream_t os{&out, [](const clap_ostream* s, const void* buf, uint64_t size) -> int64_t {
                              auto* v = static_cast<std::vector<uint8_t>*>(s->ctx);
                              const auto* b = static_cast<const uint8_t*>(buf);
                              v->insert(v->end(), b, b + size);
                              return static_cast<int64_t>(size);
                          }};
        if (!state_->save(plugin_, &os)) {
            err = "clap.state save failed";
            return false;
        }
        return true;
    }

    uint32_t latency() override { return latency_ != nullptr && activated_ ? latency_->get(plugin_) : 0; }

    std::unique_ptr<PluginEditor> open_editor(const std::string& title, std::string& err) override {
        if (gui_ == nullptr) {
            err = "the plugin has no editor (no clap.gui extension)";
            return nullptr;
        }
        std::weak_ptr<ClapInstance> weak = weak_from_this();
        return create_clap_editor(plugin_, gui_, title, [weak](ClapEditorHost* h) {
            if (auto self = weak.lock()) {
                // Requests queued for an earlier window must not reach this one.
                self->pending_resize_.store(0, std::memory_order_relaxed);
                self->pending_closed_.store(false, std::memory_order_relaxed);
                self->editor_host_ = h;
            }
        }, err);
    }

    void shutdown() noexcept override {
        if (plugin_ == nullptr) return;
        if (processing_) {
            AudioThreadScope audio; // the RT side is gone; we take the audio role once
            plugin_->stop_processing(plugin_);
            processing_ = false;
        }
        if (activated_) plugin_->deactivate(plugin_);
        activated_ = false;
        plugin_->destroy(plugin_);
        plugin_ = nullptr;
#if defined(__linux__)
        run_loop::remove_all(this); // fds and timers the plugin left registered
#endif
        mod_.reset();
    }

    // Latency change protocol: the plugin called request_restart (e.g. after
    // a parameter changed its latency). Called by the PluginHost on the main
    // thread while the instance is suspended (the RT side cannot be inside
    // process()): stop_processing (taking the audio-thread role once, as in
    // shutdown()), deactivate, activate. process() calls start_processing
    // again on the next block. A failed activate leaves the instance passing
    // audio through (effects) / silent (instruments).
    void restart() noexcept override {
        if (plugin_ == nullptr) return;
        if (processing_) {
            AudioThreadScope audio;
            plugin_->stop_processing(plugin_);
            processing_ = false;
        }
        if (activated_) plugin_->deactivate(plugin_);
        activated_ = plugin_->activate(plugin_, rate_, 1, max_block_);
    }

    void on_main_thread() noexcept override {
        // GUI requests made off the main thread (clap.gui allows any thread).
        if (const uint64_t r = pending_resize_.exchange(0, std::memory_order_acq_rel); r != 0 && editor_host_ != nullptr)
            editor_host_->request_resize(static_cast<uint32_t>(r >> 32), static_cast<uint32_t>(r));
        if (pending_closed_.exchange(false, std::memory_order_acq_rel) && editor_host_ != nullptr)
            editor_host_->plugin_closed();
        if (plugin_ != nullptr && callback_requested_.exchange(false)) plugin_->on_main_thread(plugin_);
    }

private:
    struct CachedParam {
        clap_id id = 0;
        double min = 0.0, max = 1.0;
        float normalize(double v) const noexcept {
            return max > min ? static_cast<float>(std::clamp((v - min) / (max - min), 0.0, 1.0)) : 0.0f;
        }
        double plain(float n) const noexcept { return min + std::clamp(static_cast<double>(n), 0.0, 1.0) * (max - min); }
    };

    static ParamInfo to_info(const clap_param_info_t& pi) {
        ParamInfo info;
        info.id = pi.id;
        info.name = std::string(pi.name, strnlen(pi.name, sizeof pi.name));
        info.group = std::string(pi.module, strnlen(pi.module, sizeof pi.module));
        info.min = pi.min_value;
        info.max = pi.max_value;
        const double span = pi.max_value - pi.min_value;
        info.default_normalized =
            span > 0.0 ? static_cast<float>(std::clamp((pi.default_value - pi.min_value) / span, 0.0, 1.0)) : 0.0f;
        info.steps = (pi.flags & CLAP_PARAM_IS_STEPPED) != 0 && span > 0.0 ? static_cast<uint32_t>(std::lround(span)) : 0;
        info.automatable = (pi.flags & CLAP_PARAM_IS_AUTOMATABLE) != 0 && (pi.flags & CLAP_PARAM_IS_READONLY) == 0;
        return info;
    }

    // Ranges by id, read once on the main thread at creation (immutable
    // afterwards, so the audio thread can normalize output events).
    void build_param_cache() {
        if (params_ == nullptr) return;
        const uint32_t n = params_->count(plugin_);
        for (uint32_t i = 0; i < n; ++i) {
            clap_param_info_t pi{};
            if (!params_->get_info(plugin_, i, &pi)) continue;
            cache_.push_back(CachedParam{pi.id, pi.min_value, pi.max_value});
        }
        std::sort(cache_.begin(), cache_.end(), [](const CachedParam& a, const CachedParam& b) { return a.id < b.id; });
        last_norm_ = std::make_unique<std::atomic<float>[]>(cache_.size());
        for (std::size_t i = 0; i < cache_.size(); ++i) {
            double v = 0.0;
            last_norm_[i].store(params_->get_value(plugin_, cache_[i].id, &v) ? cache_[i].normalize(v) : 0.0f);
        }
    }

    const CachedParam* cached(clap_id id) const noexcept {
        auto it = std::lower_bound(cache_.begin(), cache_.end(), id,
                                   [](const CachedParam& c, clap_id v) { return c.id < v; });
        return it != cache_.end() && it->id == id ? &*it : nullptr;
    }

    void push_param_event(uint32_t time, uint32_t id, double value) noexcept {
        if (event_count_ >= event_store_.size()) return;
        clap_event_param_value_t& e = event_store_[event_count_++].param;
        e = clap_event_param_value_t{};
        e.header = {sizeof e, time, CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_PARAM_VALUE, 0};
        e.param_id = id;
        e.note_id = -1;
        e.port_index = -1;
        e.channel = -1;
        e.key = -1;
        e.value = value;
        if (const CachedParam* c = cached(id)) last_norm_[static_cast<std::size_t>(c - cache_.data())].store(c->normalize(value));
    }

    // Output events of process() / flush(): editor gestures and values.
    void on_output_event(const clap_event_header_t* h) noexcept {
        if (h == nullptr || h->space_id != CLAP_CORE_EVENT_SPACE_ID) return;
        const int64_t frame = out_frame_ + static_cast<int64_t>(h->time);
        if (h->type == CLAP_EVENT_PARAM_VALUE) {
            const auto* e = reinterpret_cast<const clap_event_param_value_t*>(h);
            const CachedParam* c = cached(e->param_id);
            if (c == nullptr) return;
            const float n = c->normalize(e->value);
            last_norm_[static_cast<std::size_t>(c - cache_.data())].store(n);
            report_change_at(e->param_id, n, frame, false, false);
        } else if (h->type == CLAP_EVENT_PARAM_GESTURE_BEGIN || h->type == CLAP_EVENT_PARAM_GESTURE_END) {
            const auto* e = reinterpret_cast<const clap_event_param_gesture_t*>(h);
            const CachedParam* c = cached(e->param_id);
            if (c == nullptr) return;
            const float n = last_norm_[static_cast<std::size_t>(c - cache_.data())].load();
            const bool begin = h->type == CLAP_EVENT_PARAM_GESTURE_BEGIN;
            report_change_at(e->param_id, n, frame, begin, !begin);
        }
    }

    // Failure path: effects pass audio through, instruments are silent.
    void pass(float* l, float* r, uint32_t frames) noexcept {
        if (!spec_.instrument) return;
        std::memset(l, 0, sizeof(float) * frames);
        std::memset(r, 0, sizeof(float) * frames);
    }

    uint32_t port_channels(bool input) {
        if (audio_ports_ == nullptr || audio_ports_->count(plugin_, input) == 0) return 0;
        clap_audio_port_info_t info{};
        if (!audio_ports_->get(plugin_, 0, input, &info)) return 0;
        return info.channel_count;
    }

    bool load_state(const std::vector<uint8_t>& bytes) {
        struct Reader {
            const std::vector<uint8_t>* v;
            std::size_t pos;
        } reader{&bytes, 0};
        clap_istream_t is{&reader, [](const clap_istream* s, void* buf, uint64_t size) -> int64_t {
                              auto* rd = static_cast<Reader*>(s->ctx);
                              const std::size_t n = std::min<std::size_t>(size, rd->v->size() - rd->pos);
                              std::memcpy(buf, rd->v->data() + rd->pos, n);
                              rd->pos += n;
                              return static_cast<int64_t>(n);
                          }};
        return state_->load(plugin_, &is);
    }

    static ClapInstance* self_of(const clap_host* h) { return static_cast<ClapInstance*>(h->host_data); }
#if defined(__linux__)
    // clap.posix-fd-support flags -> run loop events.
    static uint32_t to_run_loop(clap_posix_fd_flags_t f) {
        return ((f & CLAP_POSIX_FD_READ) != 0 ? run_loop::kRead : 0u) |
               ((f & CLAP_POSIX_FD_WRITE) != 0 ? run_loop::kWrite : 0u);
    }
#endif

    static const void* host_get_extension(const clap_host* h, const char* id) {
        static const clap_host_thread_check_t thread_check{
            [](const clap_host* host) { return std::this_thread::get_id() == self_of(host)->main_thread_; },
            [](const clap_host*) { return t_clap_audio_thread; }};
        static const clap_host_log_t log{[](const clap_host*, clap_log_severity, const char*) {}};
        static const clap_host_latency_t latency{[](const clap_host*) {}};
        static const clap_host_state_t state{[](const clap_host*) {}};
        static const clap_host_params_t params{
            [](const clap_host*, clap_param_rescan_flags) {}, [](const clap_host*, clap_id, clap_param_clear_flags) {},
            [](const clap_host* host) { self_of(host)->flush_requested_.store(true, std::memory_order_release); }};
        static const clap_host_gui_t gui{
            [](const clap_host*) {},
            [](const clap_host* host, uint32_t w, uint32_t hgt) {
                auto* self = self_of(host);
                if (std::this_thread::get_id() != self->main_thread_) {
                    // Any thread may ask; the editor is main-thread only, so
                    // the size is applied from on_main_thread().
                    self->pending_resize_.store((uint64_t{w} << 32) | hgt, std::memory_order_release);
                    self->wake_main_thread();
                    return true;
                }
                self->pending_resize_.store(0, std::memory_order_relaxed); // superseded by this size
                return self->editor_host_ != nullptr && self->editor_host_->request_resize(w, hgt);
            },
            [](const clap_host*) { return false; },
            [](const clap_host*) { return false; },
            [](const clap_host* host, bool) {
                auto* self = self_of(host);
                if (std::this_thread::get_id() != self->main_thread_) {
                    self->pending_closed_.store(true, std::memory_order_release); // see request_resize
                    self->wake_main_thread();
                    return;
                }
                if (self->editor_host_ != nullptr) self->editor_host_->plugin_closed();
            }};
        const std::string_view s(id);
        if (s == CLAP_EXT_THREAD_CHECK) return &thread_check;
        if (s == CLAP_EXT_LOG) return &log;
        if (s == CLAP_EXT_LATENCY) return &latency;
        if (s == CLAP_EXT_STATE) return &state;
        if (s == CLAP_EXT_PARAMS) return &params;
        if (s == CLAP_EXT_GUI) return &gui;
#if defined(__linux__)
        // Linux has no system event loop: plugins hand the host their fds
        // (their X connection) and timers, run from se_plugin_idle.
        static const clap_host_timer_support_t timers{
            [](const clap_host* host, uint32_t period_ms, clap_id* out_id) {
                auto* self = self_of(host);
                if (out_id == nullptr || self->plugin_ == nullptr) return false;
                const uint32_t t = run_loop::add_timer(self, period_ms, [self](uint32_t tid) {
                    if (self->plugin_ == nullptr) return;
                    if (self->timer_ext_ == nullptr)
                        self->timer_ext_ = static_cast<const clap_plugin_timer_support_t*>(
                            self->plugin_->get_extension(self->plugin_, CLAP_EXT_TIMER_SUPPORT));
                    if (self->timer_ext_ != nullptr) self->timer_ext_->on_timer(self->plugin_, tid);
                });
                *out_id = t;
                return t != 0;
            },
            [](const clap_host* host, clap_id timer) { return run_loop::remove_timer(self_of(host), timer); }};
        static const clap_host_posix_fd_support_t fds{
            [](const clap_host* host, int fd, clap_posix_fd_flags_t flags) {
                auto* self = self_of(host);
                if (self->plugin_ == nullptr) return false;
                return run_loop::add_fd(self, fd, to_run_loop(flags), [self](int f, uint32_t ready) {
                    if (self->plugin_ == nullptr) return;
                    if (self->fd_ext_ == nullptr)
                        self->fd_ext_ = static_cast<const clap_plugin_posix_fd_support_t*>(
                            self->plugin_->get_extension(self->plugin_, CLAP_EXT_POSIX_FD_SUPPORT));
                    if (self->fd_ext_ == nullptr) return;
                    clap_posix_fd_flags_t got = 0;
                    if ((ready & run_loop::kRead) != 0) got |= CLAP_POSIX_FD_READ;
                    if ((ready & run_loop::kWrite) != 0) got |= CLAP_POSIX_FD_WRITE;
                    if ((ready & run_loop::kError) != 0) got |= CLAP_POSIX_FD_ERROR;
                    self->fd_ext_->on_fd(self->plugin_, f, got);
                });
            },
            [](const clap_host* host, int fd, clap_posix_fd_flags_t flags) {
                return run_loop::modify_fd(self_of(host), fd, to_run_loop(flags));
            },
            [](const clap_host* host, int fd) { return run_loop::remove_fd(self_of(host), fd); }};
        if (s == CLAP_EXT_TIMER_SUPPORT) return &timers;
        if (s == CLAP_EXT_POSIX_FD_SUPPORT) return &fds;
#endif
        (void)h;
        return nullptr;
    }

    static void host_request_callback(const clap_host* h) {
        auto* self = self_of(h);
        self->callback_requested_.store(true);
        self->wake_main_thread();
    }

    // Makes on_main_thread() run soon. Every platform runs it from
    // se_plugin_idle; macOS also queues it on the main dispatch queue, except
    // from the audio thread, where queuing would allocate.
    void wake_main_thread() {
#if defined(__APPLE__)
        if (t_clap_audio_thread) return; // no allocation in process(): se_plugin_idle picks it up
        // Run on the main queue (the app's run loop); the instance may be
        // gone by then, hence the weak reference.
        std::weak_ptr<ClapInstance> weak;
        try {
            weak = weak_from_this();
        } catch (...) {
        }
        if (weak.expired()) return; // during init: picked up by se_plugin_idle
        auto* ctx = new std::weak_ptr<ClapInstance>(std::move(weak));
        dispatch_async_f(dispatch_get_main_queue(), ctx, [](void* c) {
            auto* w = static_cast<std::weak_ptr<ClapInstance>*>(c);
            if (auto inst = w->lock()) inst->on_main_thread();
            delete w;
        });
#endif
    }

    double rate_;
    uint32_t max_block_;
    std::shared_ptr<ClapModule> mod_;
    clap_host_t host_{};
    const clap_plugin_t* plugin_ = nullptr;
    const clap_plugin_audio_ports_t* audio_ports_ = nullptr;
    const clap_plugin_note_ports_t* note_ports_ = nullptr;
    const clap_plugin_state_t* state_ = nullptr;
    const clap_plugin_latency_t* latency_ = nullptr;
    const clap_plugin_gui_t* gui_ = nullptr;
    const clap_plugin_params_t* params_ = nullptr;
    std::vector<CachedParam> cache_;
    std::unique_ptr<std::atomic<float>[]> last_norm_; // per cache_ entry: last value seen (gesture records)
    std::atomic<bool> flush_requested_{false};
    int64_t out_frame_ = 0; // transport frame of the current process()/flush() (output event stamps)
    uint32_t in_channels_ = 0, out_channels_ = 0;
    bool use_midi_ = false;
    bool note_port_ = false;
    bool activated_ = false;
    std::vector<float> in_l_, in_r_, out_l_, out_r_;
    float* in_ptrs_[2] = {};
    float* out_ptrs_[2] = {};
    clap_audio_buffer_t in_buf_{}, out_buf_{};
    clap_input_events_t in_events_{};
    clap_output_events_t out_events_{};
    std::thread::id main_thread_;
    std::atomic<bool> callback_requested_{false};
    ClapEditorHost* editor_host_ = nullptr; // main thread
    std::atomic<uint64_t> pending_resize_{0}; // (w << 32) | h asked off the main thread, 0 = none
    std::atomic<bool> pending_closed_{false}; // gui closed reported off the main thread
#if defined(__linux__)
    // The plugin's timer and fd callbacks (looked up on first use).
    const clap_plugin_timer_support_t* timer_ext_ = nullptr;
    const clap_plugin_posix_fd_support_t* fd_ext_ = nullptr;
#endif
    // RT-owned
    bool processing_ = false;
    bool failed_ = false;
    int64_t steady_time_ = 0;
    clap_event_transport_t transport_{};
    uint32_t event_count_ = 0;
    std::vector<ClapEventStorage> event_store_;
};

} // namespace

std::shared_ptr<PluginInstance> create_clap_instance(const PluginSpec& spec, double sample_rate, uint32_t max_block,
                                                     std::string& err) {
    if (spec.path.empty()) {
        err = "CLAP device has no bundle path";
        return nullptr;
    }
    auto inst = std::make_shared<ClapInstance>(spec, sample_rate, max_block);
    if (!inst->init(err)) return nullptr;
    return inst;
}

} // namespace dstudio
