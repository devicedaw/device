#include "builtin.hpp"

#include <algorithm>
#include <cmath>
#include <cstring>

namespace dstudio {

// ---------------------------------------------------------------- registry
// One line per built-in device; the descriptor lives next to its processor.
extern const BuiltinDescriptor kCompressorDescriptor;
extern const BuiltinDescriptor kChannelEqDescriptor;
extern const BuiltinDescriptor kCharacterEqDescriptor;
extern const BuiltinDescriptor kAutoFilterDescriptor;
extern const BuiltinDescriptor kGlueCompressorDescriptor;
extern const BuiltinDescriptor kLimiterDescriptor;
extern const BuiltinDescriptor kMasteringLimiterDescriptor;
extern const BuiltinDescriptor kGateDescriptor;
extern const BuiltinDescriptor kTransientShaperDescriptor;
extern const BuiltinDescriptor kLfoDescriptor;
extern const BuiltinDescriptor kRackDescriptor;
extern const BuiltinDescriptor kSamplerDescriptor;
extern const BuiltinDescriptor kDrumRackDescriptor;
extern const BuiltinDescriptor kPadSamplerDescriptor;
extern const BuiltinDescriptor kDistortionDescriptor;
extern const BuiltinDescriptor kTapeSaturationDescriptor;
extern const BuiltinDescriptor kSoftClipperDescriptor;
extern const BuiltinDescriptor kBitCrusherDescriptor;
extern const BuiltinDescriptor kWidenerDescriptor;
extern const BuiltinDescriptor kUtilityDescriptor;
extern const BuiltinDescriptor kChorusDescriptor;
extern const BuiltinDescriptor kCharacterModDescriptor;
extern const BuiltinDescriptor kFlangerDescriptor;
extern const BuiltinDescriptor kPhaserDescriptor;
extern const BuiltinDescriptor kTremoloDescriptor;
extern const BuiltinDescriptor kAutopannerDescriptor;
extern const BuiltinDescriptor kRingModDescriptor;
extern const BuiltinDescriptor kDelayDescriptor;
extern const BuiltinDescriptor kEchoDescriptor;
extern const BuiltinDescriptor kGrainsDescriptor;
extern const BuiltinDescriptor kReverbDescriptor;
extern const BuiltinDescriptor kPitchShifterDescriptor;
extern const BuiltinDescriptor kCharacterCompressorDescriptor;
extern const BuiltinDescriptor kMultibandCompressorDescriptor;
extern const BuiltinDescriptor kSynthV2Descriptor;
extern const BuiltinDescriptor kDeClipperDescriptor;
extern const BuiltinDescriptor kDeEsserDescriptor;
extern const BuiltinDescriptor kDeHummerDescriptor;
extern const BuiltinDescriptor kDeNoiserDescriptor;
extern const BuiltinDescriptor kPitchCorrectorDescriptor;
extern const BuiltinDescriptor kTunerDescriptor;
extern const BuiltinDescriptor kSpectrumDescriptor;
extern const BuiltinDescriptor kTranceGateDescriptor;
extern const BuiltinDescriptor kShaperDescriptor;

namespace {

const BuiltinDescriptor* const kBuiltinRegistry[] = {
    &kCompressorDescriptor,
    &kChannelEqDescriptor,
    &kCharacterEqDescriptor,
    &kAutoFilterDescriptor,
    &kGlueCompressorDescriptor,
    &kLimiterDescriptor,
    &kMasteringLimiterDescriptor,
    &kGateDescriptor,
    &kTransientShaperDescriptor,
    &kLfoDescriptor,
    &kRackDescriptor,
    &kSamplerDescriptor,
    &kDrumRackDescriptor,
    &kPadSamplerDescriptor,
    &kDistortionDescriptor,
    &kTapeSaturationDescriptor,
    &kSoftClipperDescriptor,
    &kBitCrusherDescriptor,
    &kWidenerDescriptor,
    &kUtilityDescriptor,
    &kChorusDescriptor,
    &kCharacterModDescriptor,
    &kFlangerDescriptor,
    &kPhaserDescriptor,
    &kTremoloDescriptor,
    &kAutopannerDescriptor,
    &kRingModDescriptor,
    &kDelayDescriptor,
    &kEchoDescriptor,
    &kGrainsDescriptor,
    &kReverbDescriptor,
    &kPitchShifterDescriptor,
    &kCharacterCompressorDescriptor,
    &kMultibandCompressorDescriptor,
    &kSynthV2Descriptor,
    &kDeClipperDescriptor, &kDeEsserDescriptor, &kDeHummerDescriptor, &kDeNoiserDescriptor,
    &kPitchCorrectorDescriptor,
    &kTunerDescriptor,
    &kSpectrumDescriptor,
    &kTranceGateDescriptor,
    &kShaperDescriptor,
};

struct CRegistry {
    std::vector<BuiltinDescriptor> list;
    std::vector<se_builtin_desc> c;
    CRegistry() {
        for (const BuiltinDescriptor* d : kBuiltinRegistry) list.push_back(*d);
        for (const BuiltinDescriptor& d : list)
            c.push_back(se_builtin_desc{d.id, d.params.data(), d.readouts.data(), static_cast<uint32_t>(d.params.size()),
                                        static_cast<uint32_t>(d.readouts.size()), d.defaults.data(), d.sample_slots,
                                        static_cast<uint8_t>(d.instrument ? 1 : 0), {0, 0, 0}});
    }
};

const CRegistry& registry() {
    static const CRegistry r;
    return r;
}

} // namespace

std::span<const BuiltinDescriptor> builtin_registry() noexcept { return registry().list; }

const BuiltinDescriptor* find_builtin(std::string_view id) noexcept {
    for (const BuiltinDescriptor& d : registry().list)
        if (id == d.id) return &d;
    return nullptr;
}

const se_builtin_desc* builtin_registry_c(std::size_t& count) noexcept {
    count = registry().c.size();
    return registry().c.data();
}

const char* parse_builtin_state(const uint8_t* data, std::size_t len, BuiltinStateView& out) noexcept {
    out = BuiltinStateView{};
    if (data == nullptr || len == 0) return nullptr;
    if (len < sizeof(se_builtin_state)) return "built-in state shorter than its header";
    uint32_t magic;
    std::memcpy(&magic, data, 4);
    if (magic == SE_BUILTIN_STATE_MAGIC) {
        se_builtin_state h;
        std::memcpy(&h, data, sizeof h);
        if (len != sizeof h + std::size_t{4} * h.value_count) return "built-in state length does not match value_count";
        out.values = data + sizeof h;
        out.value_count = h.value_count;
        out.sidechain = h.sidechain_track_id;
    } else if (magic == SE_BUILTIN_STATE_MAGIC2) {
        if (len < sizeof(se_builtin_state_v2)) return "built-in state shorter than its header";
        se_builtin_state_v2 h;
        std::memcpy(&h, data, sizeof h);
        const std::size_t want = sizeof h + std::size_t{4} * h.value_count + sizeof(se_builtin_sample) * h.sample_count +
                                 sizeof(se_builtin_slice) * h.slice_count + sizeof(se_mod_route) * std::size_t{h.route_count};
        if (len != want) return "built-in state length does not match its counts";
        out.values = data + sizeof h;
        out.value_count = h.value_count;
        out.sidechain = h.sidechain_track_id;
        out.samples = out.values + std::size_t{4} * h.value_count;
        out.sample_count = h.sample_count;
        out.slices = out.samples + sizeof(se_builtin_sample) * h.sample_count;
        out.slice_count = h.slice_count;
        out.routes = out.slices + sizeof(se_builtin_slice) * h.slice_count;
        out.route_count = h.route_count;
        uint64_t total = 0;
        for (uint32_t i = 0; i < h.sample_count; ++i) {
            se_builtin_sample s;
            std::memcpy(&s, out.samples + sizeof s * i, sizeof s);
            if (s.slice_count > SE_BUILTIN_MAX_SLICES) return "built-in sample has too many slices";
            total += s.slice_count;
        }
        if (total != h.slice_count) return "built-in sample slice counts do not add up";
        for (uint32_t i = 0; i < h.slice_count; ++i) {
            se_builtin_slice s;
            std::memcpy(&s, out.slices + sizeof s * i, sizeof s);
            if (!std::isfinite(s.start) || !std::isfinite(s.gain_db) || !std::isfinite(s.pan) || !std::isfinite(s.pitch))
                return "built-in slice value is not finite";
            if (s.start < 0.0f || s.start >= 1.0f) return "built-in slice start outside 0..1";
        }
    } else {
        return "built-in state has a wrong magic";
    }
    for (uint32_t i = 0; i < out.value_count; ++i) {
        float v;
        std::memcpy(&v, out.values + 4 * i, 4);
        if (!std::isfinite(v)) return "built-in state value is not finite";
    }
    for (uint32_t i = 0; i < out.route_count; ++i) {
        se_mod_route r;
        std::memcpy(&r, out.routes + sizeof r * i, sizeof r);
        if (r.kind > SE_MOD_PLUGIN) return "modulation route has an unknown kind";
        if (!std::isfinite(r.min) || !std::isfinite(r.max) || !std::isfinite(r.p_min) || !std::isfinite(r.p_max) ||
            !std::isfinite(r.p_skew) || !std::isfinite(r.p_step))
            return "modulation route range is not finite";
    }
    return nullptr;
}

// ---------------------------------------------------------------- analysis

AnalysisChannel::AnalysisChannel(uint32_t capacity)
    : capacity_(std::max<uint32_t>(1, capacity)), buf_(new float[std::size_t{3} * std::max<uint32_t>(1, capacity)]()) {}

void AnalysisChannel::publish(uint32_t count) noexcept {
    count_[back_] = std::min(count, capacity_);
    seq_[back_] = ++next_seq_;
    back_ = middle_.exchange(back_ | kFresh, std::memory_order_acq_rel) & 3u;
}

uint32_t AnalysisChannel::read(float* out, uint32_t cap, uint64_t& seq) noexcept {
    if ((middle_.load(std::memory_order_acquire) & kFresh) != 0)
        front_ = middle_.exchange(front_, std::memory_order_acq_rel) & 3u;
    seq = seq_[front_];
    const uint32_t n = std::min(count_[front_], cap);
    if (n > 0) std::memcpy(out, buf_.get() + std::size_t{front_} * capacity_, sizeof(float) * n);
    return count_[front_];
}

float mod_to_norm(const BuiltinMod& m, float v) noexcept {
    if (!(m.max > m.min)) return 0.0f;
    v = std::clamp(v, m.min, m.max);
    float n;
    if (m.log) {
        n = m.min > 0.0f ? std::log(v / m.min) / std::log(m.max / m.min) : 0.0f;
    } else {
        n = std::clamp((v - m.min) / (m.max - m.min), 0.0f, 1.0f);
        if (m.skew != 1.0f && n > 0.0f) n = std::pow(n, m.skew);
    }
    return std::isfinite(n) ? std::clamp(n, 0.0f, 1.0f) : 0.0f;
}

float mod_from_norm(const BuiltinMod& m, float n) noexcept {
    if (!(m.max > m.min)) return std::isfinite(m.min) ? m.min : 0.0f;
    n = std::isfinite(n) ? std::clamp(n, 0.0f, 1.0f) : 0.0f;
    float v;
    if (m.log && m.min > 0.0f) {
        v = m.min * std::pow(m.max / m.min, n); // max > min > 0
    } else {
        if (m.skew != 1.0f && m.skew > 0.0f && n > 0.0f) n = std::pow(n, 1.0f / m.skew);
        v = m.min + (m.max - m.min) * n;
    }
    v = std::clamp(v, m.min, m.max);
    if (m.step > 0.0f && !m.log) v = std::clamp(m.min + std::floor((v - m.min) / m.step + 0.501f) * m.step, m.min, m.max);
    return v;
}

const char* parse_builtin_state(const uint8_t* data, std::size_t len, const uint8_t*& values, uint32_t& count,
                                uint64_t& sidechain) noexcept {
    BuiltinStateView v;
    const char* err = parse_builtin_state(data, len, v);
    values = err == nullptr ? v.values : nullptr;
    count = err == nullptr ? v.value_count : 0;
    sidechain = err == nullptr ? v.sidechain : 0;
    return err;
}

// ---------------------------------------------------------------- instance

BuiltinInstance::BuiltinInstance(PluginSpec spec, const BuiltinDescriptor& desc, std::unique_ptr<BuiltinProcessor> proc,
                                 double sample_rate, uint32_t max_block)
    : PluginInstance(std::move(spec)), desc_(desc), proc_(std::move(proc)),
      target_(new std::atomic<float>[std::max<std::size_t>(1, desc.params.size())]), current_(desc.params.size(), 0.0f),
      readout_atomic_(new std::atomic<float>[std::max<std::size_t>(1, desc.readouts.size())]),
      readout_scratch_(desc.readouts.size(), 0.0f), readout_segment_(desc.readouts.size(), 0.0f),
      midi_scratch_(desc.instrument ? kMaxEvents + kLiveEvents : 0) {
    if (desc_.instrument) merged_.resize(kMaxEvents + kLiveEvents);
    for (std::size_t i = 0; i < desc_.params.size(); ++i) target_[i].store(0.0f, std::memory_order_relaxed);
    for (std::size_t i = 0; i < desc_.readouts.size(); ++i)
        readout_atomic_[i].store(i < desc_.idle_readouts.size() ? desc_.idle_readouts[i] : 0.0f, std::memory_order_relaxed);
    update_state(spec_.state.data(), spec_.state.size());
    for (std::size_t i = 0; i < desc_.params.size(); ++i) current_[i] = target_[i].load(std::memory_order_relaxed);
    modded_.assign(desc_.params.size(), 0.0f);
    mod_params_.assign(desc_.params.size(), 0.0f);
    proc_->prepare(sample_rate, max_block);
    if (const uint32_t cap = proc_->analysis_capacity(); cap > 0) analysis_ = std::make_unique<AnalysisChannel>(cap);
    proc_->reset();
    mod_outputs_ = proc_->mod_outputs();
    if (mod_outputs_ > 0) mod_out_.assign(2 * std::size_t{mod_outputs_} * (max_block / kGrid + 2), 0.0f);
}

void BuiltinInstance::run_modulation(uint32_t frames, const RtTransport& transport, const ParamEvent* params,
                                     uint32_t param_count) noexcept {
    if (mod_outputs_ == 0) return;
    const auto n_params = static_cast<uint32_t>(mod_params_.size());
    float* const cur = mod_params_.data();
    for (uint32_t i = 0; i < n_params; ++i) cur[i] = target_[i].load(std::memory_order_relaxed);
    const uint32_t cap = static_cast<uint32_t>(mod_out_.size() / (2 * std::size_t{mod_outputs_}));
    uint32_t e = 0;
    for (uint32_t k = 0, pos = 0; pos < frames && k < cap; ++k, pos += kGrid) {
        while (e < param_count && params[e].offset <= pos) {
            if (params[e].id < n_params) cur[params[e].id] = static_cast<float>(params[e].value);
            ++e;
        }
        RtTransport t = transport;
        if (t.playing) t.position += pos;
        BuiltinContext ctx;
        ctx.transport = &t;
        proc_->modulate(cur, std::min(kGrid, frames - pos), ctx, mod_out_.data() + 2 * std::size_t{k} * mod_outputs_);
    }
}

void BuiltinInstance::set_value(uint32_t index, float value) noexcept {
    if (index < param_count() && std::isfinite(value)) target_[index].store(value, std::memory_order_relaxed);
}

float BuiltinInstance::value(uint32_t index) const noexcept {
    return index < param_count() ? target_[index].load(std::memory_order_relaxed) : 0.0f;
}

void BuiltinInstance::update_state(const uint8_t* data, std::size_t len) noexcept {
    BuiltinStateView view;
    if (parse_builtin_state(data, len, view) != nullptr || view.values == nullptr) return;
    const uint32_t count = std::min(view.value_count, param_count());
    for (uint32_t i = 0; i < count; ++i) {
        float v;
        std::memcpy(&v, view.values + 4 * i, 4);
        target_[i].store(v, std::memory_order_relaxed);
    }
    if (desc_.sample_slots == 0) return;
    // The sample table is resolved into assets by bind_samples() (it needs
    // the engine's asset registry). Main thread: allocation is fine here.
    try {
        std::vector<SampleEntry> table;
        table.reserve(view.sample_count);
        const uint8_t* sl = view.slices;
        for (uint32_t i = 0; i < view.sample_count; ++i) {
            se_builtin_sample s;
            std::memcpy(&s, view.samples + sizeof s * i, sizeof s);
            SampleEntry e;
            e.asset = s.asset_id;
            e.slot = s.slot;
            for (uint32_t k = 0; k < s.slice_count; ++k, sl += sizeof(se_builtin_slice)) {
                se_builtin_slice c;
                std::memcpy(&c, sl, sizeof c);
                e.slices.push_back(BuiltinSlice{c.start, c.gain_db, c.pan, c.pitch});
            }
            if (e.slot < desc_.sample_slots) table.push_back(std::move(e));
        }
        table_ = std::move(table);
    } catch (...) {
        // Out of memory: keep the previous table.
    }
}

BuiltinInstance::~BuiltinInstance() {
    delete pending_.exchange(nullptr);
    delete retired_.exchange(nullptr);
    delete active_;
}

void BuiltinInstance::reclaim() noexcept { delete retired_.exchange(nullptr, std::memory_order_acq_rel); }

void BuiltinInstance::bind_samples(const AssetLookup& lookup) {
    if (desc_.sample_slots == 0) return;
    reclaim();
    // Rebuild when the table changed or an asset appeared / disappeared
    // (a file finished loading, or was released).
    std::vector<uint64_t> present;
    for (const SampleEntry& e : table_)
        if (lookup && lookup(e.asset)) present.push_back(e.asset);
    if (bound_once_ && table_ == bound_ && present == bound_present_) return;
    auto set = std::make_unique<SampleSet>();
    set->views.resize(desc_.sample_slots);
    std::size_t nslices = 0;
    for (const SampleEntry& e : table_) nslices += e.slices.size();
    set->slices.reserve(nslices);
    for (const SampleEntry& e : table_) {
        AssetRef a = lookup ? lookup(e.asset) : nullptr;
        if (!a || a->frames == 0 || a->channels == 0) continue;
        BuiltinSample& v = set->views[e.slot];
        v.ch[0] = a->channel(0);
        v.ch[1] = a->channel(a->channels > 1 ? 1 : 0);
        v.frames = a->frames;
        v.channels = a->channels;
        const Asset::Octave* oct = a->octaves();
        for (int L = 0; L < Asset::kOctaves; ++L) {
            v.oct[L][0] = oct[L].channel(0);
            v.oct[L][1] = oct[L].channel(a->channels > 1 ? 1 : 0);
            v.oct_frames[L] = oct[L].frames;
        }
        v.slice_count = static_cast<uint32_t>(e.slices.size());
        v.slices = nullptr;
        if (!e.slices.empty()) {
            v.slices = set->slices.data() + set->slices.size(); // reserved: no reallocation
            set->slices.insert(set->slices.end(), e.slices.begin(), e.slices.end());
        }
        set->refs.push_back(std::move(a));
    }
    bound_ = table_;
    bound_present_ = std::move(present);
    bound_once_ = true;
    // A set the RT side never picked up can go right away.
    delete pending_.exchange(set.release(), std::memory_order_acq_rel);
    // The RT side may have adopted an older set meanwhile: free it so the
    // newest one is not held back behind it.
    reclaim();
}

// RT: adopt the newest sample set. The previous one waits in retired_ until
// the main thread frees it; nothing is adopted while one still waits.
void BuiltinInstance::take_samples() noexcept {
    if (retired_.load(std::memory_order_acquire) != nullptr) return;
    SampleSet* next = pending_.exchange(nullptr, std::memory_order_acq_rel);
    if (next == nullptr) return;
    SampleSet* old = active_;
    active_ = next;
    retired_.store(old, std::memory_order_release);
}

uint32_t BuiltinInstance::analysis(float* out, uint32_t capacity, uint64_t& seq) noexcept {
    seq = 0;
    return analysis_ ? analysis_->read(out, capacity, seq) : 0;
}

uint32_t BuiltinInstance::latency() {
    std::vector<float> v(param_count());
    for (uint32_t i = 0; i < param_count(); ++i) v[i] = target_[i].load(std::memory_order_relaxed);
    // The stored values give the latency before the first block; automation can hold others.
    return std::max(proc_->latency_for(v.data()), applied_latency_.load(std::memory_order_relaxed));
}

uint32_t BuiltinInstance::readouts(float* out, uint32_t capacity) const noexcept {
    const auto n = static_cast<uint32_t>(desc_.readouts.size());
    for (uint32_t i = 0; i < n && i < capacity; ++i) out[i] = readout_atomic_[i].load(std::memory_order_relaxed);
    return n;
}

namespace {
// Peak of a block into a running maximum (RT; the main thread resets it to 0).
void accumulate_peak(std::atomic<float>& slot, const float* x, uint32_t frames) noexcept {
    if (x == nullptr) return;
    float p = 0.0f;
    for (uint32_t i = 0; i < frames; ++i) p = std::max(p, std::fabs(x[i]));
    if (!(p > 0.0f)) return; // also drops NaN
    float cur = slot.load(std::memory_order_relaxed);
    while (p > cur && !slot.compare_exchange_weak(cur, p, std::memory_order_relaxed)) {
    }
}
} // namespace

void BuiltinInstance::process(float* l, float* r, uint32_t frames, const MidiEvent* events, uint32_t event_count,
                              const RtTransport& transport,
                              const ParamEvent* params, uint32_t param_count) noexcept {
    const bool metering = metering_.load(std::memory_order_relaxed);
    if (metering) {
        accumulate_peak(peaks_[0], l, frames);
        accumulate_peak(peaks_[1], r, frames);
    }
    const auto n_params = static_cast<uint32_t>(current_.size());
    float* const cur = current_.data();
    for (uint32_t i = 0; i < n_params; ++i) cur[i] = target_[i].load(std::memory_order_relaxed);
    BuiltinContext ctx;
    ctx.sc_l = sc_l_;
    ctx.sc_r = sc_r_;
    ctx.transport = &transport;
    ctx.analysis = analysis_.get();
    if (desc_.sample_slots > 0) {
        take_samples();
        if (active_ != nullptr) {
            ctx.samples = active_->views.data();
            ctx.sample_count = static_cast<uint32_t>(active_->views.size());
        }
    }
    // Instruments: live notes (at the first frame) ahead of the track's
    // notes, which arrive sorted by offset.
    uint32_t ne = 0;
    if (desc_.instrument) {
        const auto cap = static_cast<uint32_t>(merged_.size());
        ne = take_live(merged_.data(), kLiveEvents);
        for (uint32_t i = 0; i < event_count && ne < cap; ++i) merged_[ne++] = events[i];
    }
    const MidiEvent* const evs = desc_.instrument ? merged_.data() : events;
    const uint32_t evn = desc_.instrument ? ne : event_count;
    float* const ro = readout_scratch_.data();
    // Automation events split the block (sorted by offset): every segment
    // runs with the values in force at its first frame. Modulation splits it
    // further at every grid point (SE_MOD_GRID_FRAMES). The block's readouts
    // keep, per readout, the segment value farthest from its idle value
    // (highest peak, deepest reduction), or for state readouts the last
    // segment's values. MIDI events go to the segment they fall in, with
    // segment-relative offsets.
    const bool modulated = mod_count_ > 0;
    float* const run_params = modulated ? modded_.data() : cur;
    MidiEvent* const seg_events = midi_scratch_.data();
    uint32_t pos = 0, e = 0, m = 0, lat = 0;
    while (pos < frames) {
        while (e < param_count && params[e].offset <= pos) {
            if (params[e].id < n_params) cur[params[e].id] = static_cast<float>(params[e].value);
            ++e;
        }
        uint32_t end = e < param_count ? std::min(frames, params[e].offset) : frames;
        if (modulated) {
            const uint32_t k = pos / kGrid;
            end = std::min(end, (k + 1) * kGrid);
            std::memcpy(run_params, cur, sizeof(float) * n_params);
            for (uint32_t j = 0; j < mod_count_; ++j) {
                const BuiltinMod& md = mods_[j];
                if (md.index >= n_params) continue;
                const float a = md.abs != nullptr ? md.abs[k] : std::nanf("");
                const float n = (std::isnan(a) ? mod_to_norm(md, cur[md.index]) : a) + (md.off != nullptr ? md.off[k] : 0.0f);
                run_params[md.index] = mod_from_norm(md, n);
            }
        }
        lat = std::max(lat, proc_->latency_for(cur));
        BuiltinContext seg = ctx;
        seg.offset = pos;
        if (seg.sc_l != nullptr) {
            seg.sc_l += pos;
            seg.sc_r += pos;
        }
        uint32_t nm = 0;
        for (; m < evn && (evs[m].offset < end || end == frames); ++m) {
            if (nm == midi_scratch_.size()) continue;
            MidiEvent ev = evs[m];
            ev.offset = ev.offset > pos ? std::min(ev.offset, end - 1) - pos : 0;
            seg_events[nm++] = ev;
        }
        seg.events = nm > 0 ? seg_events : nullptr;
        seg.event_count = nm;
        if (pos == 0) {
            proc_->process(l, r, end, run_params, seg, ro);
        } else {
            float* const sr = readout_segment_.data();
            proc_->process(l + pos, r + pos, end - pos, run_params, seg, sr);
            if (desc_.state_readouts) {
                std::copy(sr, sr + readout_segment_.size(), ro);
                pos = end;
                continue;
            }
            for (std::size_t i = 0; i < readout_segment_.size(); ++i) {
                const float idle = i < desc_.idle_readouts.size() ? desc_.idle_readouts[i] : 0.0f;
                if (std::fabs(sr[i] - idle) > std::fabs(ro[i] - idle)) ro[i] = sr[i];
            }
        }
        pos = end;
    }
    for (std::size_t i = 0; i < readout_scratch_.size(); ++i) readout_atomic_[i].store(ro[i], std::memory_order_relaxed);
    applied_latency_.store(lat, std::memory_order_relaxed);
    if (metering) {
        accumulate_peak(peaks_[2], l, frames);
        accumulate_peak(peaks_[3], r, frames);
    }
    sc_l_ = sc_r_ = nullptr;
    mods_ = nullptr;
    mod_count_ = 0;
}

bool BuiltinInstance::save_state(std::vector<uint8_t>& out, std::string&) {
    se_builtin_state h{SE_BUILTIN_STATE_MAGIC, param_count(), 0};
    out.resize(sizeof h + 4 * std::size_t{param_count()});
    std::memcpy(out.data(), &h, sizeof h);
    for (uint32_t i = 0; i < param_count(); ++i) {
        const float v = value(i);
        std::memcpy(out.data() + sizeof h + 4 * i, &v, 4);
    }
    return true;
}

std::unique_ptr<PluginEditor> BuiltinInstance::open_editor(const std::string&, std::string& err) {
    err = "built-in devices have no plugin window";
    return nullptr;
}

void BuiltinInstance::params(std::vector<ParamInfo>& out) {
    out.clear();
    for (uint32_t i = 0; i < param_count(); ++i) {
        ParamInfo p;
        p.id = i;
        p.name = desc_.params[i];
        p.min = -1e30;
        p.max = 1e30;
        p.automatable = proc_->automatable(i);
        out.push_back(std::move(p));
    }
}

bool BuiltinInstance::param_range(uint32_t id, double& min, double& max) {
    // Built-in lanes carry plain values (SE_AUTO_BUILTIN): no mapping.
    min = 0.0;
    max = 1.0;
    return id < param_count();
}

std::shared_ptr<PluginInstance> create_builtin_instance(const PluginSpec& spec, double sample_rate, uint32_t max_block,
                                                        std::string& err) {
    const BuiltinDescriptor* d = find_builtin(spec.plugin_id);
    if (d == nullptr) {
        err = "unknown built-in device \"" + spec.plugin_id + "\"";
        return nullptr;
    }
    if (spec.instrument != d->instrument) {
        err = d->instrument ? "built-in instruments go in a track's instrument slot"
                            : "built-in effects can't be used as an instrument";
        return nullptr;
    }
    if (!spec.instrument && d->instrument) {
        err = "built-in instruments go in a track's instrument slot, not in its effect chain";
        return nullptr;
    }
    return std::make_shared<BuiltinInstance>(spec, *d, d->create(), sample_rate, max_block);
}

} // namespace dstudio
