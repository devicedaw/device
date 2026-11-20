#include "processor.hpp"

#include "delay_line.hpp"
#include "devices/builtin.hpp"
#include "plugins/plugin_instance.hpp"
#include "rt_util.hpp"

#include <algorithm>
#include <cmath>
#include <cstring>
#include <limits>

#if defined(__ARM_NEON) || defined(__ARM_NEON__)
#include <arm_neon.h>
#elif defined(__SSE2__) || defined(_M_X64)
#include <emmintrin.h>
#endif

namespace dstudio {

RenderContext::RenderContext(uint32_t max_block)
    : max_block_(max_block), track_l_(max_block), track_r_(max_block), master_l_(max_block), master_r_(max_block),
      synth_(max_block), zeros_(max_block, 0.0f), plugin_l_(max_block), plugin_r_(max_block), gain_l_(max_block),
      gain_r_(max_block), auto_a_(max_block), auto_b_(max_block), param_events_(PluginInstance::kMaxParamEvents),
      sc_(std::size_t{2} * SE_BUILTIN_MAX_SIDECHAINS * max_block, 0.0f),
      bus_(new float[std::size_t{2} * SE_MAX_RETURNS * max_block]), send_(new float[std::size_t{2} * max_block]), mod_points_(max_block / SE_MOD_GRID_FRAMES + 2),
      mod_(std::size_t{2} * kMaxModTargets * mod_points_, 0.0f), mod_binds_(kMaxModTargets), live_(max_block),
      fade_l_(max_block), fade_r_(max_block), midi_(kMidiScratch) {}

void kernel_add_scaled(float* __restrict dst, const float* __restrict src, float gain, uint32_t n) noexcept {
    for (uint32_t i = 0; i < n; ++i) dst[i] += src[i] * gain;
}

void kernel_add_scaled2(float* __restrict dst_l, float* __restrict dst_r, const float* __restrict src_l,
                        const float* __restrict src_r, float gain_l, float gain_r, uint32_t n) noexcept {
    for (uint32_t i = 0; i < n; ++i) {
        dst_l[i] += src_l[i] * gain_l;
        dst_r[i] += src_r[i] * gain_r;
    }
}

void kernel_strip(float* __restrict l, float* __restrict r, float a, float b, uint32_t n) noexcept {
    for (uint32_t i = 0; i < n; ++i) {
        const float x = l[i];
        const float y = r[i];
        l[i] = a * x + b * y;
        r[i] = b * x + a * y;
    }
}

void kernel_strip_varying(float* __restrict l, float* __restrict r, const float* __restrict trim,
                          const float* __restrict width, uint32_t n) noexcept {
    for (uint32_t i = 0; i < n; ++i) {
        // strip_matrix per sample.
        const float a = trim[i] * 0.5f * (1.0f + width[i]);
        const float b = trim[i] * 0.5f * (1.0f - width[i]);
        const float x = l[i];
        const float y = r[i];
        l[i] = a * x + b * y;
        r[i] = b * x + a * y;
    }
}

void kernel_scale(float* __restrict buf, float gain, uint32_t n) noexcept {
    for (uint32_t i = 0; i < n; ++i) buf[i] *= gain;
}

void kernel_add_mul2(float* __restrict dst_l, float* __restrict dst_r, const float* __restrict src_l,
                     const float* __restrict src_r, const float* __restrict gain_l, const float* __restrict gain_r,
                     uint32_t n) noexcept {
    for (uint32_t i = 0; i < n; ++i) {
        dst_l[i] += src_l[i] * gain_l[i];
        dst_r[i] += src_r[i] * gain_r[i];
    }
}

void kernel_mul(float* __restrict buf, const float* __restrict gain, uint32_t n) noexcept {
    for (uint32_t i = 0; i < n; ++i) buf[i] *= gain[i];
}

namespace {

int64_t floor_mod(int64_t a, int64_t m) noexcept {
    const int64_t r = a % m;
    return r < 0 ? r + m : r;
}

int64_t floor_div(int64_t a, int64_t b) noexcept {
    const int64_t q = a / b;
    return (a % b != 0 && (a < 0) != (b < 0)) ? q - 1 : q;
}

// Grid of plugin parameter events (timeline frames) and the most lane
// points sent per lane and processed segment.
constexpr int64_t kParamGrid = 64;
constexpr uint32_t kMaxLanePoints = 64;

} // namespace

float kernel_peak(const float* __restrict buf, uint32_t n) noexcept {
    // Max-abs reduction. Compilers will not auto-vectorise this without
    // -ffast-math (std::max has NaN-ordering semantics), so use intrinsics.
    uint32_t i = 0;
    float m = 0.0f;
#if defined(__ARM_NEON) || defined(__ARM_NEON__)
    float32x4_t v0 = vdupq_n_f32(0.0f), v1 = vdupq_n_f32(0.0f);
    for (; i + 8 <= n; i += 8) {
        v0 = vmaxq_f32(v0, vabsq_f32(vld1q_f32(buf + i)));
        v1 = vmaxq_f32(v1, vabsq_f32(vld1q_f32(buf + i + 4)));
    }
    m = vmaxvq_f32(vmaxq_f32(v0, v1));
#elif defined(__SSE2__) || defined(_M_X64)
    const __m128 abs_mask = _mm_castsi128_ps(_mm_set1_epi32(0x7fffffff));
    __m128 v0 = _mm_setzero_ps(), v1 = _mm_setzero_ps();
    for (; i + 8 <= n; i += 8) {
        v0 = _mm_max_ps(v0, _mm_and_ps(_mm_loadu_ps(buf + i), abs_mask));
        v1 = _mm_max_ps(v1, _mm_and_ps(_mm_loadu_ps(buf + i + 4), abs_mask));
    }
    alignas(16) float lanes[4];
    _mm_store_ps(lanes, _mm_max_ps(v0, v1));
    m = std::max(std::max(lanes[0], lanes[1]), std::max(lanes[2], lanes[3]));
#endif
    for (; i < n; ++i) m = std::max(m, std::fabs(buf[i]));
    return m;
}

// RT: the chain meter after a device's card (PluginInstance::set_output_metering).
void meter_output(PluginInstance& p, const float* l, const float* r, uint32_t frames) noexcept {
    if (!p.output_metering()) return;
    p.store_output_peaks(kernel_peak(l, frames), kernel_peak(r, frames));
}

namespace {

#if defined(_MSC_VER)
#define DSTUDIO_NOINLINE __declspec(noinline)
#else
#define DSTUDIO_NOINLINE __attribute__((noinline))
#endif

// Routing of the tracks that are not plain (TrackNode::plain: sends, a return,
// or an output into a return's bus). Out of line, so the loop over plain
// tracks in render_block() carries none of it.
struct Router {
    const RenderGraph& g;
    RenderContext& ctx;
    DelayLine* const* lines; // null: delay compensation off
    std::size_t track_count;
    uint32_t frames;
    uint64_t gen;
    float* ml;
    float* mr;

    // Edge delay line of send `s` (null when not needed).
    DelayLine* send_line(uint32_t s) const noexcept {
        return lines != nullptr && g.sends[s].edge_delay != 0
                   ? lines[g.pdc.send_index(track_count, g.fx.size(), s)]
                   : nullptr;
    }
    // Edge delay line of the output of track `t` (null when not needed).
    DelayLine* out_line(const TrackNode& track, std::size_t t) const noexcept {
        return lines != nullptr && track.out_delay != 0
                   ? lines[g.pdc.out_index(track_count, g.fx.size(), g.sends.size(), t)]
                   : nullptr;
    }
    // Bus `bus` as a destination, cleared by its first writer in the block.
    void bus_dst(int32_t bus, float*& dl, float*& dr) const noexcept {
        const auto b = static_cast<uint32_t>(bus);
        dl = ctx.bus_l(b);
        dr = ctx.bus_r(b);
        if (!ctx.bus_used(b)) {
            std::memset(dl, 0, sizeof(float) * frames);
            std::memset(dr, 0, sizeof(float) * frames);
            ctx.bus_used(b) = true;
        }
    }
    // Whether the bus of a return received anything in this block.
    bool bus_in(const TrackNode& track) const noexcept {
        return track.bus >= 0 && ctx.bus_used(static_cast<uint32_t>(track.bus));
    }
    // Whether an edge line of the track still has a tail to give.
    DSTUDIO_NOINLINE bool edge_pending(const TrackNode& track, std::size_t t) const noexcept {
        if (lines == nullptr || !track.audible) return false;
        if (DelayLine* l = out_line(track, t); l != nullptr && !l->idle(track.out_delay, gen)) return true;
        for (uint32_t s = track.send_begin; s < track.send_begin + track.send_count; ++s)
            if (DelayLine* l = send_line(s); l != nullptr && !l->idle(g.sends[s].edge_delay, gen)) return true;
        return false;
    }
    // The track plays nothing in this block: its edge lines advance silently.
    DSTUDIO_NOINLINE void discard_edges(const TrackNode& track, std::size_t t) const noexcept {
        if (DelayLine* l = out_line(track, t)) l->discard(frames, gen);
        for (uint32_t s = track.send_begin; s < track.send_begin + track.send_count; ++s)
            if (DelayLine* l = send_line(s)) l->discard(frames, gen);
    }
    // The track's signal (tl, tr) into (dl, dr): post-fader (static gains,
    // or the automated gains gl/gr when `automated`) or pre-fader, times `s`,
    // through `line`.
    void mix(const TrackNode& track, const float* tl, const float* tr, bool automated, const float* gl,
             const float* gr, float* dl, float* dr, bool post, float s, DelayLine* line, uint32_t d,
             bool silent) const noexcept {
        if (line == nullptr) {
            if (!post) {
                kernel_add_scaled2(dl, dr, tl, tr, s, s, frames);
            } else if (!automated) {
                kernel_add_scaled2(dl, dr, tl, tr, track.gain_l * s, track.gain_r * s, frames);
            } else if (s == 1.0f) {
                kernel_add_mul2(dl, dr, tl, tr, gl, gr, frames);
            } else {
                for (uint32_t i = 0; i < frames; ++i) {
                    dl[i] += tl[i] * gl[i] * s;
                    dr[i] += tr[i] * gr[i] * s;
                }
            }
            return;
        }
        float* const xl = ctx.send_l();
        float* const xr = ctx.send_r();
        if (post && automated) {
            for (uint32_t i = 0; i < frames; ++i) {
                xl[i] = tl[i] * gl[i] * s;
                xr[i] = tr[i] * gr[i] * s;
            }
        } else {
            const float a = post ? track.gain_l * s : s;
            const float b = post ? track.gain_r * s : s;
            for (uint32_t i = 0; i < frames; ++i) {
                xl[i] = tl[i] * a;
                xr[i] = tr[i] * b;
            }
        }
        line->process(xl, xr, frames, d, gen, silent);
        kernel_add_scaled(dl, xl, 1.0f, frames);
        kernel_add_scaled(dr, xr, 1.0f, frames);
    }
    // Sends (pre-fader taps, then post-fader) and the output into the master
    // or a return's bus. Order doesn't matter: each lands in its own destination.
    // `silent`: (tl, tr) is silence fed only to drain the edge lines' tails.
    DSTUDIO_NOINLINE void play(const TrackNode& track, std::size_t t, const float* tl, const float* tr, bool automated,
                               const float* gl, const float* gr, bool silent) const noexcept {
        const auto run_sends = [&](bool pre) {
            for (uint32_t s = track.send_begin; s < track.send_begin + track.send_count; ++s) {
                const SendNode& send = g.sends[s];
                if (send.pre_fader != pre) continue;
                DelayLine* const line = send_line(s);
                if (!track.send_live) {
                    if (line != nullptr) line->discard(frames, gen);
                    continue;
                }
                float* dl = nullptr;
                float* dr = nullptr;
                bus_dst(send.bus, dl, dr);
                mix(track, tl, tr, automated, gl, gr, dl, dr, !pre, send.gain, line, send.edge_delay, silent);
            }
        };
        run_sends(true);
        DelayLine* const line = out_line(track, t);
        if (!track.main_on) {
            if (line != nullptr) line->discard(frames, gen);
        } else {
            float* dl = ml;
            float* dr = mr;
            if (track.out_bus >= 0) bus_dst(track.out_bus, dl, dr);
            mix(track, tl, tr, automated, gl, gr, dl, dr, true, 1.0f, line, track.out_delay, silent);
        }
        run_sends(false);
    }
};

} // namespace

BlockPeaks render_block(const RenderGraph* graph, int64_t position, bool playing, uint32_t frames,
                        RenderContext& ctx, bool update_meters, uint32_t peek_slot,
                        const InstrumentInputs& inst, const InputView& input) noexcept {
    float* const ml = ctx.master_l();
    float* const mr = ctx.master_r();
    std::memset(ml, 0, sizeof(float) * frames);
    std::memset(mr, 0, sizeof(float) * frames);
    if (graph == nullptr || frames == 0) return {};
    const bool live_input = input.count > 0 && graph->uses_input;
    const bool plugins_on = inst.transport != nullptr && graph->has_plugins;
    DelayLine* const* const lines = graph->pdc.need.empty() ? nullptr : inst.lines;
    if (!playing && inst.synths == nullptr && !live_input && !plugins_on && lines == nullptr) return {};
    const auto input_channel = [&](uint32_t c) { return c < input.count ? input.channels[c] : ctx.zeros(); };
    const uint64_t gen = inst.pdc_gen;

    // ---- automation helpers
    // Output offsets [0, frames) seen `delay` samples late -> timeline pieces.
    const auto pieces = [&](uint32_t delay, auto&& f) {
        if (inst.history != nullptr)
            inst.history->pieces(position, playing, frames, delay, f);
        else
            linear_pieces(position, playing, frames, delay, f);
    };
    // Value of a lane at the block start (the first timeline piece), for
    // switches applied per block (SE_AUTO_DEVICE_ON).
    const auto block_value = [&](const AutoLane& lane, uint32_t delay) -> float {
        float v = 0.0f;
        bool first = true;
        pieces(delay, [&](uint32_t, uint32_t, int64_t tl0, bool) {
            if (first) v = auto_eval(lane_view(*graph, lane), tl0);
            first = false;
        });
        return v;
    };
    // A device's power this block: its on lane, else the static switch.
    const auto slot_on = [&](const FxSlot& s, uint32_t delay) -> bool {
        if (s.on_lane < 0) return s.enabled;
        return block_value(graph->lanes[static_cast<uint32_t>(s.on_lane)], delay) >= 0.5f;
    };
    // dst[i] = map(lane value) for the block (ramps, see auto_fill).
    const auto fill_lane = [&](const AutoLane& lane, uint32_t delay, auto&& map, float* dst) {
        const LaneView v = lane_view(*graph, lane);
        pieces(delay, [&](uint32_t i0, uint32_t i1, int64_t tl0, bool moving) {
            if (moving) {
                auto_fill(v, tl0, i1 - i0, map, dst + i0);
            } else {
                const float g = map(auto_eval(v, tl0));
                for (uint32_t i = i0; i < i1; ++i) dst[i] = g;
            }
        });
    };
    // Parameter events of `count` lanes from `begin` (sorted by offset):
    // the value at the block start (and each timeline piece), every
    // kParamGrid timeline frames and at every point, unchanged values
    // skipped. Returns the event count.
    // Lanes of modulated plugin parameters are left out: the modulation
    // events below carry their values (`mods` / `nmods`: the device's targets).
    const auto plugin_events = [&](uint32_t begin, uint32_t count, uint32_t delay, const ModTarget* mods = nullptr,
                                   uint32_t nmods = 0) -> uint32_t {
        ParamEvent* const ev = ctx.param_events();
        // (static: MSVC does not see a constexpr local used in a nested lambda, C4189)
        static constexpr uint32_t cap = PluginInstance::kMaxParamEvents;
        uint32_t n = 0;
        for (uint32_t li = begin; li < begin + count; ++li) {
            const AutoLane& lane = graph->lanes[li];
            if (!lane.active) continue;
            bool modulated = false;
            for (uint32_t m = 0; m < nmods && !modulated; ++m)
                modulated = mods[m].plugin && mods[m].active && mods[m].lane == static_cast<int32_t>(li);
            if (modulated) continue;
            const LaneView v = lane_view(*graph, lane);
            uint32_t points_left = kMaxLanePoints;
            bool have_last = false;
            float last = 0.0f;
            const auto emit = [&](uint32_t offset, float value) {
                if (have_last && value == last) return;
                if (n >= cap) return;
                have_last = true;
                last = value;
                if (lane.target == SE_AUTO_BUILTIN) { // plain values, no mapping
                    ev[n++] = ParamEvent{offset, lane.param_id, static_cast<double>(value)};
                    return;
                }
                const double nv = std::clamp(static_cast<double>(value), 0.0, 1.0);
                ev[n++] = ParamEvent{offset, lane.param_id, lane.native_min + nv * (lane.native_max - lane.native_min)};
            };
            pieces(delay, [&](uint32_t i0, uint32_t i1, int64_t tl0, bool moving) {
                emit(i0, auto_eval(v, tl0));
                if (!moving) return;
                const int64_t tl_end = tl0 + (i1 - i0);
                uint32_t idx = auto_upper(v, tl0);
                int64_t grid = (floor_div(tl0, kParamGrid) + 1) * kParamGrid;
                for (;;) {
                    const bool point_next = idx < v.n && v.p[idx].frame <= grid;
                    const int64_t next = point_next ? v.p[idx].frame : grid;
                    if (next >= tl_end) break;
                    const bool on_grid = next == grid;
                    if (on_grid) grid += kParamGrid;
                    if (point_next) {
                        while (idx < v.n && v.p[idx].frame == next) ++idx;
                        if (!on_grid) {
                            if (points_left == 0) continue; // point budget spent: grid only
                            --points_left;
                        }
                    }
                    emit(i0 + static_cast<uint32_t>(next - tl0), auto_eval(v, next));
                }
            });
        }
        std::sort(ev, ev + n, [](const ParamEvent& a, const ParamEvent& b) { return a.offset < b.offset; });
        return n;
    };

    // ---- modulation helpers (se_mod_route)
    const uint32_t mod_points = std::min(ctx.mod_points(), (frames + SE_MOD_GRID_FRAMES - 1) / SE_MOD_GRID_FRAMES);
    // Offsets and absolute values (NaN = none) of one target per grid point.
    const auto mod_values = [&](const ModTarget& t, float* off, float* abs) -> bool {
        bool has_abs = false;
        for (uint32_t k = 0; k < mod_points; ++k) {
            off[k] = 0.0f;
            abs[k] = std::numeric_limits<float>::quiet_NaN();
        }
        for (uint32_t ri = t.route_begin; ri < t.route_begin + t.route_count; ++ri) {
            const ModRoute& r = graph->mod_routes[ri];
            const FxSlot& s = graph->fx[r.src];
            BuiltinInstance* const b = s.inst != nullptr && slot_on(s, 0) ? s.inst->as_builtin() : nullptr;
            if (b == nullptr || r.slot >= b->mod_outputs()) continue;
            const float span = r.max - r.min;
            for (uint32_t k = 0; k < mod_points; ++k) {
                const float* vg = b->mod_point(k, r.slot);
                if (r.absolute) {
                    abs[k] = r.min + span * vg[0];
                    has_abs = true;
                } else {
                    off[k] += vg[1] * (r.min + span * vg[0]);
                }
            }
        }
        return has_abs;
    };
    // Built-in targets: bindings applied inside the instance's process().
    const auto bind_builtin = [&](BuiltinInstance* b, uint32_t begin, uint32_t count) {
        BuiltinMod* const binds = ctx.mod_binds();
        uint32_t n = 0;
        for (uint32_t i = begin; i < begin + count && n < RenderContext::kMaxModTargets; ++i) {
            const ModTarget& t = graph->mod_targets[i];
            if (t.plugin || !t.active || t.param_id >= b->param_count()) continue;
            float* const off = ctx.mod_off(n);
            float* const abs = ctx.mod_abs(n);
            const bool has_abs = mod_values(t, off, abs);
            binds[n++] = BuiltinMod{t.param_id, t.log, t.p_min, t.p_max, t.p_skew, t.p_step, off, has_abs ? abs : nullptr};
        }
        b->set_modulation(binds, n);
    };
    // Plugin targets: events (appended after `n` lane events, then sorted)
    // with the modulated value at every grid point where it changes.
    // Timeline position of block offset `off` for audio `delay` frames late (loops, seeks).
    const auto timeline_at = [&](uint32_t delay, uint32_t off) -> int64_t {
        int64_t at = position;
        pieces(delay, [&](uint32_t i0, uint32_t i1, int64_t tl0, bool moving) {
            if (off >= i0 && off < i1) at = moving ? tl0 + static_cast<int64_t>(off - i0) : tl0;
        });
        return at;
    };
    const auto plugin_mod_events = [&](uint32_t begin, uint32_t count, uint32_t n, uint32_t delay) -> uint32_t {
        ParamEvent* const ev = ctx.param_events();
        static constexpr uint32_t cap = PluginInstance::kMaxParamEvents;
        float* const off = ctx.mod_off(0);
        float* const abs = ctx.mod_abs(0);
        for (uint32_t i = begin; i < begin + count; ++i) {
            const ModTarget& t = graph->mod_targets[i];
            if (!t.plugin || !t.active || t.base == nullptr) continue;
            mod_values(t, off, abs);
            const float base = t.base->load(std::memory_order_relaxed);
            float last = -1.0f;
            for (uint32_t k = 0; k < mod_points && n < cap; ++k) {
                float b0 = base;
                if (t.lane >= 0) {
                    const int64_t at = timeline_at(delay, k * SE_MOD_GRID_FRAMES);
                    b0 = std::clamp(auto_eval(lane_view(*graph, graph->lanes[static_cast<uint32_t>(t.lane)]), at), 0.0f, 1.0f);
                }
                const float v = std::clamp((std::isnan(abs[k]) ? b0 : abs[k]) + off[k], 0.0f, 1.0f);
                if (v == last) continue;
                last = v;
                ev[n++] = ParamEvent{k * SE_MOD_GRID_FRAMES, t.param_id, t.native_min + static_cast<double>(v) * (t.native_max - t.native_min)};
            }
        }
        std::sort(ev, ev + n, [](const ParamEvent& a, const ParamEvent& b) { return a.offset < b.offset; });
        return n;
    };
    // Bases of parameters that lost their last route (RenderGraph::mod_restores),
    // put first so a lane event at offset 0 still wins.
    const bool restoring = plugins_on && !graph->mod_restores.empty() && !graph->restores_done.load(std::memory_order_relaxed);
    const auto add_restores = [&](uint64_t device, uint32_t n) -> uint32_t {
        if (!restoring) return n;
        ParamEvent* const ev = ctx.param_events();
        static constexpr uint32_t cap = PluginInstance::kMaxParamEvents;
        uint32_t r = 0;
        for (const RenderGraph::ModRestore& m : graph->mod_restores) r += m.device_id == device ? 1u : 0u;
        if (r == 0) return n;
        r = std::min(r, cap);
        n = std::min(n, cap - r);
        std::memmove(ev + r, ev, sizeof(ParamEvent) * n);
        uint32_t k = 0;
        for (const RenderGraph::ModRestore& m : graph->mod_restores)
            if (m.device_id == device && k < r) ev[k++] = ParamEvent{0, m.param_id, m.value};
        return n + r;
    };

    float* const tl = ctx.track_l();
    float* const tr = ctx.track_r();
    float* const sb = ctx.synth();
    const int64_t block_start = position;
    const int64_t block_end = position + frames;

    const std::size_t track_count = graph->tracks.size();
    const bool ordered = graph->order.size() == track_count;
    for (uint32_t k = 0; k < graph->sc_taps; ++k) {
        std::memset(ctx.sc_l(k), 0, sizeof(float) * frames);
        std::memset(ctx.sc_r(k), 0, sizeof(float) * frames);
    }
    // Return buses: cleared by their first writer in this block.
    for (uint32_t k = 0; k < graph->bus_count; ++k) ctx.bus_used(k) = false;
    const Router router{*graph, ctx, lines, track_count, frames, gen, ml, mr};
    for (std::size_t k = 0; k < track_count; ++k) {
        const std::size_t t = ordered ? graph->order[k] : k;
        const TrackNode& track = graph->tracks[t];
        // A plain track (no sends, not a return, output on the master; out_delay is then 0)
        // skips the routing work and plays into the master as before routing existed.
        const bool plain = track.plain;
        // Routing: what the bus of a return received, and whether an edge
        // delay line of the track's sends or output still has a tail.
        const bool bus_in = !plain && router.bus_in(track);
        const bool edge_pending = !plain && router.edge_pending(track, t);
        Synth* const synth = track.instrument && !track.has_instrument_plugin && inst.synths != nullptr
                                 ? inst.synths[t]
                                 : nullptr;
        bool iplug_on = track.instrument_plugin_enabled;
        if (track.switch_lanes && track.iplug_on_lane >= 0) [[unlikely]]
            iplug_on = block_value(graph->lanes[static_cast<uint32_t>(track.iplug_on_lane)], 0) >= 0.5f;
        PluginInstance* const iplug = plugins_on && iplug_on ? track.instrument_plugin : nullptr;
        const bool track_plugins = plugins_on && (track.fx_count > 0 || iplug != nullptr);
        const bool routed = live_input && track.input_mode != SE_INPUT_NONE && (track.monitor || track.arm);
        // MIDI input (live path): this block's events when the track receives
        // input, else note-offs for keys it still holds (routing changed).
        TrackMidi* const tmidi = inst.midi != nullptr && t < graph->midi_tracks.size() ? graph->midi_tracks[t] : nullptr;
        MidiEvent* const midi_ev = ctx.midi_scratch();
        uint32_t n_midi = 0;
        if (tmidi != nullptr && (inst.midi->count > 0 || tmidi->held_count > 0)) {
            const bool midi_routed =
                graph->midi_explicit ? (track.arm || track.monitor) : track.track_id == inst.midi->focus;
            n_midi = tmidi->events_for(inst.midi->events, inst.midi->count, midi_routed, midi_ev,
                                       RenderContext::kMidiScratch);
        }
        // Delay compensation lines of this track (null when not needed).
        DelayLine* main_line = nullptr;
        DelayLine* src_line = nullptr;
        bool pending = false; // a line still has output to give
        if (lines != nullptr) {
            main_line = track.pdc_delay != 0 ? lines[t] : nullptr;
            src_line = track.pdc_src_delay != 0 ? lines[graph->pdc.src_index(track_count, t)] : nullptr;
            if (!track.audible) { // not heard: drop what the lines hold (they restart silent)
                if (main_line != nullptr) main_line->discard(frames, gen);
                if (src_line != nullptr) src_line->discard(frames, gen);
                main_line = src_line = nullptr;
            }
            pending = (main_line != nullptr && !main_line->idle(track.pdc_delay, gen)) ||
                      (src_line != nullptr && !src_line->idle(track.pdc_src_delay, gen));
        }
        if (synth == nullptr && !routed && !track_plugins && (!track.audible || !playing) && !pending &&
            !bus_in && !edge_pending) {
            if (main_line != nullptr) main_line->discard(frames, gen);
            if (src_line != nullptr) src_line->discard(frames, gen);
            if (!plain) router.discard_edges(track, t);
            continue;
        }
        // Modulation sources run first: their outputs drive this block's targets.
        if (plugins_on)
            for (uint32_t k2 = track.msrc_begin; k2 < track.msrc_begin + track.msrc_count; ++k2) {
                const FxSlot& s = graph->fx[graph->mod_sources[k2]];
                BuiltinInstance* const b = s.inst != nullptr && slot_on(s, 0) ? s.inst->as_builtin() : nullptr;
                if (b == nullptr) continue;
                const uint32_t np = s.lane_count > 0 ? plugin_events(s.lane_begin, s.lane_count, 0) : 0;
                b->run_modulation(frames, *inst.transport, ctx.param_events(), np);
            }
        // Runs `line` over the track buffer: the buffer's content if any,
        // else silence while the line still has a tail, else just advances.
        bool any = false;
        const auto delay_track = [&](DelayLine* line, uint32_t d) {
            if (any) {
                line->process(tl, tr, frames, d, gen);
            } else if (!line->idle(d, gen)) {
                std::memset(tl, 0, sizeof(float) * frames);
                std::memset(tr, 0, sizeof(float) * frames);
                any = true;
                line->process(tl, tr, frames, d, gen, /*silent_input=*/true);
            } else {
                line->discard(frames, gen);
            }
        };

        // A return track's source: its bus.
        if (bus_in && track.audible) {
            std::memcpy(tl, ctx.bus_l(static_cast<uint32_t>(track.bus)), sizeof(float) * frames);
            std::memcpy(tr, ctx.bus_r(static_cast<uint32_t>(track.bus)), sizeof(float) * frames);
            any = true;
        }

        const ClipNode* clip = graph->clips.data() + track.clip_begin;
        const ClipNode* const clip_end = track.audible && playing ? clip + track.clip_count : clip;
        for (; clip != clip_end; ++clip) {
            if (clip->timeline_start >= block_end) break; // sorted by start
            if (clip->timeline_end <= block_start) continue;
            if (!any) {
                std::memset(tl, 0, sizeof(float) * frames);
                std::memset(tr, 0, sizeof(float) * frames);
                any = true;
            }
            const int64_t from = std::max(clip->timeline_start, block_start);
            const int64_t to = std::min(clip->timeline_end, block_end);
            WarpVoice* voice = nullptr;
            if (clip->warp >= 0) {
                const int32_t v = graph->warps[static_cast<std::size_t>(clip->warp)].voice;
                voice = v >= 0 && inst.warp_voices != nullptr ? inst.warp_voices[v] : nullptr;
            }
            // A block inside a fade renders the clip on its own first, then adds it with the fade gain.
            const bool fading = clip_fades_in(*clip, from, to);
            if (fading) {
                std::memset(ctx.fade_l(), 0, sizeof(float) * static_cast<std::size_t>(to - from));
                std::memset(ctx.fade_r(), 0, sizeof(float) * static_cast<std::size_t>(to - from));
            }
            // One run per pass over the content (several when the loop wraps in this block).
            for (int64_t at = from; at < to;) {
                const int64_t c = clip_content(*clip, at - clip->timeline_start);
                const int64_t n = clip->loop_end > 0 ? std::min(to - at, clip->loop_end - c) : to - at;
                float* const dl = fading ? ctx.fade_l() + (at - from) : tl + (at - block_start);
                float* const dr = fading ? ctx.fade_r() + (at - from) : tr + (at - block_start);
                if (clip->warp >= 0) {
                    const int64_t ct = clip->timeline_start + c;
                    render_warped_clip(*graph, *clip, voice, ct, ct + n, dl, dr);
                } else {
                    const int64_t src = clip->source_offset + c;
                    kernel_add_scaled(dl, clip->src_l + src, clip->gain, static_cast<uint32_t>(n));
                    kernel_add_scaled(dr, clip->src_r + src, clip->gain, static_cast<uint32_t>(n));
                }
                at += n;
            }
            if (fading) {
                const float* fl = ctx.fade_l();
                const float* fr = ctx.fade_r();
                const auto off = static_cast<uint32_t>(from - block_start);
                const auto n = static_cast<uint32_t>(to - from);
                // Linear ramps as multiplies (clip_fade_gain, without its divides per frame).
                const float r0 = static_cast<float>(from - clip->timeline_start);
                const float left0 = static_cast<float>(clip->timeline_end - from);
                const float fi = static_cast<float>(clip->fade_in);
                const float fo = static_cast<float>(clip->fade_out);
                const float inv_in = clip->fade_in > 0 ? 1.0f / fi : 0.0f;
                const float inv_out = clip->fade_out > 0 ? 1.0f / fo : 0.0f;
                for (uint32_t i = 0; i < n; ++i) {
                    const float r = r0 + static_cast<float>(i);
                    const float left = left0 - static_cast<float>(i);
                    const float g = (r < fi ? r * inv_in : 1.0f) * (left < fo ? left * inv_out : 1.0f);
                    tl[off + i] += fl[i] * g;
                    tr[off + i] += fr[i] * g;
                }
            }
        }
        // Clips line up with the instrument plugin's (latent) output.
        if (src_line != nullptr) delay_track(src_line, track.pdc_src_delay);
        bool synth_out = false;
        if (synth != nullptr && track.synth_lane_count == 0) {
            synth_out = synth->render(graph->notes.data() + track.note_begin, track.note_count,
                                      &graph->synth_params[t], graph->seq, inst.transport_gen, position, frames,
                                      playing, sb);
        } else if (synth != nullptr) {
            // Automated synth parameters: segments ending on the 16-frame
            // timeline grid, each with the lane values at its first frame.
            se_synth_params& sp = ctx.synth_params();
            sp = graph->synth_params[t];
            for (uint32_t i = 0; i < frames;) {
                const int64_t at = playing ? position + i : position;
                const uint32_t n = playing ? std::min<uint32_t>(frames - i, static_cast<uint32_t>(
                                                     Synth::kControlFrames - floor_mod(at, Synth::kControlFrames)))
                                           : frames - i;
                for (uint32_t li = track.synth_lane_begin; li < track.synth_lane_begin + track.synth_lane_count; ++li) {
                    const AutoLane& lane = graph->lanes[li];
                    if (float* f = synth_param_field(sp, lane.synth_param)) *f = auto_eval(lane_view(*graph, lane), at);
                }
                if (synth->render(graph->notes.data() + track.note_begin, track.note_count, &sp, graph->seq,
                                  inst.transport_gen, position + (playing ? i : 0), n, playing, sb + i))
                    synth_out = true;
                else
                    std::memset(sb + i, 0, sizeof(float) * n);
                i += n;
            }
        }
        // Live input on the built-in Synth: its own LiveSynth, summed with the notes.
        if (synth != nullptr && tmidi != nullptr && tmidi->live != nullptr) {
            LiveSynth& live = *tmidi->live;
            for (uint32_t i = 0; i < n_midi; ++i) {
                const uint8_t kind = midi_ev[i].status & 0xF0;
                if (kind == 0x90) live.note_on(midi_ev[i].data1, midi_ev[i].data2);
                else if (kind == 0x80) live.note_off(midi_ev[i].data1);
            }
            float* const lb = ctx.live_synth();
            if (live.render(&graph->synth_params[t], frames, lb)) {
                if (synth_out) {
                    kernel_add_scaled(sb, lb, 1.0f, frames);
                } else {
                    std::memcpy(sb, lb, sizeof(float) * frames);
                    synth_out = true;
                }
            }
        }
        if (synth_out && track.audible) {
            if (any) {
                kernel_add_scaled(tl, sb, 1.0f, frames);
                kernel_add_scaled(tr, sb, 1.0f, frames);
            } else {
                std::memcpy(tl, sb, sizeof(float) * frames);
                std::memcpy(tr, sb, sizeof(float) * frames);
                any = true;
            }
        }
        if (iplug != nullptr) {
            float* const pl = ctx.plugin_l();
            float* const pr = ctx.plugin_r();
            std::memset(pl, 0, sizeof(float) * frames);
            std::memset(pr, 0, sizeof(float) * frames);
            // Live notes (se_preview_note) at the first frame, ahead of the track's notes; built-in
            // instruments take theirs inside process().
            MidiEvent* const ev = iplug->event_scratch();
            // MIDI input follows them, also at the first frame; live notes leave it room.
            const uint32_t live_cap = PluginInstance::kMaxEvents - std::min(n_midi, PluginInstance::kMaxEvents);
            uint32_t nl = iplug->as_builtin() != nullptr ? 0 : iplug->take_live(ev, live_cap);
            const uint32_t ni = std::min(n_midi, PluginInstance::kMaxEvents - nl);
            if (ni > 0) std::memcpy(ev + nl, midi_ev, sizeof(MidiEvent) * ni);
            nl += ni;
            const uint32_t ne = nl + iplug->feeder().build(graph->notes.data() + track.note_begin, track.note_count,
                                                           graph->seq, inst.transport_gen, position, frames, playing,
                                                           ev + nl, PluginInstance::kMaxEvents - nl);
            const ModTarget* const im = track.imod_count > 0 ? graph->mod_targets.data() + track.imod_begin : nullptr;
            uint32_t np = track.iplug_lane_count > 0 ? plugin_events(track.iplug_lane_begin, track.iplug_lane_count, 0, im, track.imod_count) : 0;
            if (track.imod_count > 0) np = plugin_mod_events(track.imod_begin, track.imod_count, np, 0);
            np = add_restores(track.instrument_device_id, np);
            iplug->run(pl, pr, frames, ev, ne, *inst.transport, ctx.param_events(), np);
            meter_output(*iplug, pl, pr, frames);
            if (track.audible) {
                if (any) {
                    kernel_add_scaled(tl, pl, 1.0f, frames);
                    kernel_add_scaled(tr, pr, 1.0f, frames);
                } else {
                    std::memcpy(tl, pl, sizeof(float) * frames);
                    std::memcpy(tr, pr, sizeof(float) * frames);
                    any = true;
                }
            }
        }
        // Monitoring input: only the playback source is compensated.
        if (main_line != nullptr && track.pdc_pre) delay_track(main_line, track.pdc_delay);
        if (routed) {
            const float* il = input_channel(track.input_channel);
            const float* ir = track.input_mode == SE_INPUT_STEREO ? input_channel(track.input_channel + 1u) : il;
            if (track.monitor) {
                if (track.audible) {
                    if (any) {
                        kernel_add_scaled(tl, il, 1.0f, frames);
                        kernel_add_scaled(tr, ir, 1.0f, frames);
                    } else {
                        std::memcpy(tl, il, sizeof(float) * frames);
                        std::memcpy(tr, ir, sizeof(float) * frames);
                        any = true;
                    }
                }
            } else if (update_meters) {
                // Armed, not monitoring: the meter shows the raw input level.
                TrackMeter& m = graph->meters[t];
                const float pl = kernel_peak(il, frames);
                const float pr = ir == il ? pl : kernel_peak(ir, frames);
                atomic_store_max(m.peak_l, pl);
                atomic_store_max(m.peak_r, pr);
                atomic_store_max(m.win_l[peek_slot], pl);
                atomic_store_max(m.win_r[peek_slot], pr);
            }
        }
        if (plugins_on && track.fx_count > 0) {
            if (!any) {
                std::memset(tl, 0, sizeof(float) * frames);
                std::memset(tr, 0, sizeof(float) * frames);
                any = true;
            }
            const FxSlot* slot = graph->fx.data() + track.fx_begin;
            for (uint32_t i = 0; i < track.fx_count; ++i, ++slot) {
                DelayLine* const bypass = lines != nullptr && slot->pdc_bypass_delay != 0
                                              ? lines[graph->pdc.slot_index(track_count, track.fx_begin + i)]
                                              : nullptr;
                if (slot->inst != nullptr && slot_on(*slot, lines != nullptr ? slot->auto_delay : 0)) {
                    if (slot->sc_wanted)
                        if (BuiltinInstance* b = slot->inst->as_builtin()) {
                            const auto k2 = static_cast<uint32_t>(slot->sc_tap);
                            b->set_sidechain(slot->sc_tap >= 0 ? ctx.sc_l(k2) : ctx.zeros(),
                                             slot->sc_tap >= 0 ? ctx.sc_r(k2) : ctx.zeros());
                        }
                    const ModTarget* const sm = slot->mod_count > 0 ? graph->mod_targets.data() + slot->mod_begin : nullptr;
                    uint32_t np = slot->lane_count > 0
                                      ? plugin_events(slot->lane_begin, slot->lane_count,
                                                      lines != nullptr ? slot->auto_delay : 0, sm, slot->mod_count)
                                      : 0;
                    if (slot->mod_count > 0) {
                        if (BuiltinInstance* b = slot->inst->as_builtin())
                            bind_builtin(b, slot->mod_begin, slot->mod_count);
                        else
                            np = plugin_mod_events(slot->mod_begin, slot->mod_count, np,
                                                   lines != nullptr ? slot->auto_delay : 0);
                    }
                    np = add_restores(slot->device_id, np);
                    slot->inst->run(tl, tr, frames, nullptr, 0, *inst.transport, ctx.param_events(), np);
                    if (bypass != nullptr) bypass->discard(frames, gen);
                } else if (bypass != nullptr) {
                    bypass->process(tl, tr, frames, slot->pdc_bypass_delay, gen); // same timing as the plugin
                }
                if (slot->inst != nullptr) meter_output(*slot->inst, tl, tr, frames);
            }
        }
        if (main_line != nullptr && !track.pdc_pre) delay_track(main_line, track.pdc_delay);
        bool tail_only = false; // silence fed through the edge lines to drain their tails
        if (!any && edge_pending) {
            std::memset(tl, 0, sizeof(float) * frames);
            std::memset(tr, 0, sizeof(float) * frames);
            any = tail_only = true;
        }
        if (!any || !track.audible) {
            if (!plain) router.discard_edges(track, t);
            continue;
        }
        if (track.switch_lanes && (track.trim_lane >= 0 || track.width_lane >= 0 || track.mono_lane >= 0)) [[unlikely]] {
            // Automated strip: per-sample trim gain and width (the gain
            // buffers are free here; the fader fills them afresh below).
            const uint32_t d = lines != nullptr ? track.auto_delay : 0;
            float* const trim = ctx.gain_l();
            float* const width = ctx.gain_r();
            if (track.trim_lane >= 0) {
                fill_lane(graph->lanes[static_cast<uint32_t>(track.trim_lane)], d,
                          [](float db) { return std::pow(10.0f, std::clamp(db, -24.0f, 24.0f) / 20.0f); }, trim);
            } else {
                std::fill(trim, trim + frames, track.strip_trim);
            }
            if (track.width_lane >= 0) {
                fill_lane(graph->lanes[static_cast<uint32_t>(track.width_lane)], d,
                          [](float w) { return std::clamp(w, 0.0f, 2.0f); }, width);
            } else {
                std::fill(width, width + frames, track.strip_width);
            }
            if (track.mono_lane >= 0) {
                float* const keep = ctx.auto_a();
                fill_lane(graph->lanes[static_cast<uint32_t>(track.mono_lane)], d,
                          [](float v) { return v >= 0.5f ? 0.0f : 1.0f; }, keep);
                kernel_mul(width, keep, frames);
            } else if (track.strip_mono) {
                std::fill(width, width + frames, 0.0f);
            }
            kernel_strip_varying(tl, tr, trim, width, frames);
        } else if (track.strip_on) {
            kernel_strip(tl, tr, track.strip_a, track.strip_b, frames);
        }
        // Mute lane (SE_AUTO_MUTE): silences the track's signal before the
        // fader, its sends and its meter, like the static mute.
        if (track.switch_lanes && track.mute_lane >= 0) [[unlikely]] {
            float* const mg = ctx.auto_a();
            fill_lane(graph->lanes[static_cast<uint32_t>(track.mute_lane)], lines != nullptr ? track.auto_delay : 0,
                      [](float v) { return v >= 0.5f ? 0.0f : 1.0f; }, mg);
            kernel_mul(tl, mg, frames);
            kernel_mul(tr, mg, frames);
        }

        const bool automated = track.vol_lane >= 0 || track.pan_lane >= 0;
        float* const gl = ctx.gain_l();
        float* const gr = ctx.gain_r();
        float meter_gl = std::fabs(track.gain_l);
        float meter_gr = std::fabs(track.gain_r);
        if (!automated) {
            if (plain) kernel_add_scaled2(ml, mr, tl, tr, track.gain_l, track.gain_r, frames);
            if (track.sc_tap >= 0) {
                const auto k2 = static_cast<uint32_t>(track.sc_tap);
                kernel_add_scaled2(ctx.sc_l(k2), ctx.sc_r(k2), tl, tr, track.gain_l, track.gain_r, frames);
            }
        } else {
            // Automated fader: the lanes replace the static gain / pan.
            const uint32_t d = lines != nullptr ? track.auto_delay : 0;
            if (track.vol_lane >= 0) {
                fill_lane(graph->lanes[track.vol_lane], d, auto_db_to_gain, gl);
            } else {
                for (uint32_t i = 0; i < frames; ++i) gl[i] = track.gain;
            }
            if (track.pan_lane >= 0) {
                float* const pa = ctx.auto_a();
                float* const pb = ctx.auto_b();
                const AutoLane& lane = graph->lanes[track.pan_lane];
                fill_lane(lane, d, [](float p) { float l = 0, r = 0; constant_power_pan(p, l, r); return l; }, pa);
                fill_lane(lane, d, [](float p) { float l = 0, r = 0; constant_power_pan(p, l, r); return r; }, pb);
                for (uint32_t i = 0; i < frames; ++i) {
                    gr[i] = gl[i] * pb[i];
                    gl[i] *= pa[i];
                }
            } else {
                float pl = 0.0f, pr = 0.0f;
                constant_power_pan(track.pan, pl, pr);
                for (uint32_t i = 0; i < frames; ++i) {
                    gr[i] = gl[i] * pr;
                    gl[i] *= pl;
                }
            }
            if (plain) kernel_add_mul2(ml, mr, tl, tr, gl, gr, frames);
            if (track.sc_tap >= 0) {
                const auto k2 = static_cast<uint32_t>(track.sc_tap);
                kernel_add_mul2(ctx.sc_l(k2), ctx.sc_r(k2), tl, tr, gl, gr, frames);
            }
            meter_gl = kernel_peak(gl, frames);
            meter_gr = kernel_peak(gr, frames);
        }
        if (!plain) router.play(track, t, tl, tr, automated, gl, gr, tail_only);
        if (update_meters) {
            TrackMeter& m = graph->meters[t];
            const float pl = kernel_peak(tl, frames) * meter_gl;
            const float pr = kernel_peak(tr, frames) * meter_gr;
            atomic_store_max(m.peak_l, pl);
            atomic_store_max(m.peak_r, pr);
            atomic_store_max(m.win_l[peek_slot], pl);
            atomic_store_max(m.win_r[peek_slot], pr);
        }
    }

    if (restoring) graph->restores_done.store(true, std::memory_order_release);
    if (graph->master_vol_lane >= 0) {
        float* const gm = ctx.auto_a();
        fill_lane(graph->lanes[graph->master_vol_lane], lines != nullptr ? graph->pdc.total : 0, auto_db_to_gain, gm);
        kernel_mul(ml, gm, frames);
        kernel_mul(mr, gm, frames);
    } else if (graph->master_gain != 1.0f) {
        kernel_scale(ml, graph->master_gain, frames);
        kernel_scale(mr, graph->master_gain, frames);
    }
    if (!update_meters) return {};
    return {kernel_peak(ml, frames), kernel_peak(mr, frames)};
}

} // namespace dstudio
