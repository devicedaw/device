#include "render_graph.hpp"

#include "devices/builtin.hpp"

#include <algorithm>
#include <cmath>
#include <cstring>
#include <limits>
#include <numbers>
#include <tuple>
#include <unordered_map>

namespace dstudio {

void constant_power_pan(float pan, float& left, float& right) noexcept {
    const double p = std::clamp(static_cast<double>(pan), -1.0, 1.0);
    const double theta = (p + 1.0) * (std::numbers::pi / 4.0);
    left = static_cast<float>(std::cos(theta));
    right = static_cast<float>(std::sin(theta));
    // Make the extremes exact so hard-panned material is bit-transparent.
    if (p <= -1.0) right = 0.0f;
    if (p >= 1.0) left = 0.0f;
}

int32_t build_render_graph(const se_session_desc& desc, const AssetLookup& lookup,
                           std::unique_ptr<RenderGraph>& out, std::string& err) {
    if (desc.track_count > 0 && desc.tracks == nullptr) {
        err = "session has track_count > 0 but tracks == NULL";
        return SE_ERR_INVALID_ARG;
    }
    std::vector<se_track_desc_v3> tracks(desc.track_count);
    for (uint32_t t = 0; t < desc.track_count; ++t) {
        const se_track_desc& a = desc.tracks[t];
        se_track_desc_v3& b = tracks[t];
        b = se_track_desc_v3{};
        b.track_id = a.track_id;
        b.gain = a.gain;
        b.pan = a.pan;
        b.mute = a.mute;
        b.solo = a.solo;
        b.clips = a.clips;
        b.clip_count = a.clip_count;
    }
    se_session_desc_v3 d3{};
    d3.tracks = tracks.data();
    d3.track_count = desc.track_count;
    d3.master_gain = desc.master_gain;
    return build_render_graph(d3, lookup, out, err);
}

int32_t build_render_graph(const se_session_desc_v2& desc, const AssetLookup& lookup,
                           std::unique_ptr<RenderGraph>& out, std::string& err) {
    if (desc.track_count > 0 && desc.tracks == nullptr) {
        err = "session has track_count > 0 but tracks == NULL";
        return SE_ERR_INVALID_ARG;
    }
    std::vector<se_track_desc_v3> tracks(desc.track_count);
    for (uint32_t t = 0; t < desc.track_count; ++t) {
        const se_track_desc_v2& a = desc.tracks[t];
        se_track_desc_v3& b = tracks[t];
        b = se_track_desc_v3{};
        b.track_id = a.track_id;
        b.gain = a.gain;
        b.pan = a.pan;
        b.mute = a.mute;
        b.solo = a.solo;
        b.instrument = a.instrument;
        b.clips = a.clips;
        b.clip_count = a.clip_count;
        b.notes = a.notes;
        b.note_count = a.note_count;
        b.synth = nullptr;
    }
    se_session_desc_v3 d3{};
    d3.tracks = tracks.data();
    d3.track_count = desc.track_count;
    d3.master_gain = desc.master_gain;
    return build_render_graph(d3, lookup, out, err);
}

int32_t build_render_graph(const se_session_desc_v3& desc, const AssetLookup& lookup,
                           std::unique_ptr<RenderGraph>& out, std::string& err) {
    if (desc.track_count > 0 && desc.tracks == nullptr) {
        err = "session has track_count > 0 but tracks == NULL";
        return SE_ERR_INVALID_ARG;
    }
    std::vector<se_track_desc_v4> tracks(desc.track_count);
    for (uint32_t t = 0; t < desc.track_count; ++t) {
        const se_track_desc_v3& a = desc.tracks[t];
        se_track_desc_v4& b = tracks[t];
        b = se_track_desc_v4{};
        b.track_id = a.track_id;
        b.gain = a.gain;
        b.pan = a.pan;
        b.mute = a.mute;
        b.solo = a.solo;
        b.instrument = a.instrument;
        b.clips = a.clips;
        b.clip_count = a.clip_count;
        b.notes = a.notes;
        b.note_count = a.note_count;
        b.synth = a.synth;
    }
    se_session_desc_v4 d4{};
    d4.tracks = tracks.data();
    d4.track_count = desc.track_count;
    d4.master_gain = desc.master_gain;
    return build_render_graph(d4, lookup, out, err);
}

int32_t build_render_graph(const se_session_desc_v4& desc, const AssetLookup& lookup,
                           std::unique_ptr<RenderGraph>& out, std::string& err) {
    if (desc.track_count > 0 && desc.tracks == nullptr) {
        err = "session has track_count > 0 but tracks == NULL";
        return SE_ERR_INVALID_ARG;
    }
    std::vector<se_track_desc_v5> tracks(desc.track_count);
    for (uint32_t t = 0; t < desc.track_count; ++t) {
        const se_track_desc_v4& a = desc.tracks[t];
        se_track_desc_v5& b = tracks[t];
        b = se_track_desc_v5{};
        b.track_id = a.track_id;
        b.gain = a.gain;
        b.pan = a.pan;
        b.mute = a.mute;
        b.solo = a.solo;
        b.instrument = a.instrument;
        b.arm = a.arm;
        b.monitor = a.monitor;
        b.input_mode = a.input_mode;
        b.input_channel = a.input_channel;
        b.clips = a.clips;
        b.clip_count = a.clip_count;
        b.notes = a.notes;
        b.note_count = a.note_count;
        b.synth = a.synth;
    }
    se_session_desc_v5 d5{};
    d5.tracks = tracks.data();
    d5.track_count = desc.track_count;
    d5.master_gain = desc.master_gain;
    return build_render_graph(d5, lookup, out, err);
}

void strip_matrix(float trim, float width, bool mono, float& a, float& b) noexcept {
    const float w = mono ? 0.0f : width;
    // L' = trim * (M + w S), R' = trim * (M - w S) with M = (L + R) / 2, S = (L - R) / 2.
    a = trim * 0.5f * (1.0f + w);
    b = trim * 0.5f * (1.0f - w);
}

namespace {

const char* validate_device(const se_device_desc& d) {
    if (d.device_id == 0) return "device_id must not be 0";
    if (d.format > SE_PLUGIN_BUILTIN) return "invalid plugin format";
    if (d.plugin_id == nullptr) return "plugin_id is NULL";
    if (d.state_len > 0 && d.state == nullptr) return "state_len > 0 but state == NULL";
    if (d.format == SE_PLUGIN_BUILTIN) {
        const uint8_t* values = nullptr;
        uint32_t count = 0;
        uint64_t sc = 0;
        if (const char* why = parse_builtin_state(d.state, d.state_len, values, count, sc)) return why;
    }
    return nullptr;
}

} // namespace

int32_t build_render_graph(const se_session_desc_v5& desc, const AssetLookup& lookup,
                           std::unique_ptr<RenderGraph>& out, std::string& err) {
    if (desc.track_count > 0 && desc.tracks == nullptr) {
        err = "session has track_count > 0 but tracks == NULL";
        return SE_ERR_INVALID_ARG;
    }
    std::vector<se_track_desc_v6> tracks(desc.track_count);
    for (uint32_t t = 0; t < desc.track_count; ++t) {
        const se_track_desc_v5& a = desc.tracks[t];
        se_track_desc_v6& b = tracks[t];
        b = se_track_desc_v6{};
        std::memcpy(&b, &a, sizeof a); // v6 extends v5 field by field (checked in c_api.cpp)
        b.lanes = nullptr;
        b.lane_count = 0;
    }
    se_session_desc_v6 d6{};
    d6.tracks = tracks.data();
    d6.track_count = desc.track_count;
    d6.master_gain = desc.master_gain;
    return build_render_graph(d6, lookup, out, err);
}

const char* validate_lane(const se_auto_lane_desc& d) noexcept {
    if (d.target > SE_AUTO_MONO) return "invalid automation target";
    if ((d.flags & ~static_cast<uint32_t>(SE_AUTO_FLAG_STEPPED)) != 0) return "unknown automation lane flags";
    if (d.target == SE_AUTO_SYNTH && d.synth_param >= SE_SYNTH_P_COUNT) return "invalid synth parameter";
    if (d.point_count > 0 && d.points == nullptr) return "point_count > 0 but points == NULL";
    float lo = -std::numeric_limits<float>::infinity(), hi = std::numeric_limits<float>::infinity();
    if (d.target == SE_AUTO_SYNTH) synth_param_range(d.synth_param, lo, hi);
    for (uint32_t i = 0; i < d.point_count; ++i) {
        const se_auto_point& q = d.points[i];
        if (i > 0 && q.frame < d.points[i - 1].frame) return "automation points must be sorted by frame";
        if (!std::isfinite(q.value) || !std::isfinite(q.curve)) return "automation point is not finite";
        if (q.curve < -1.0f || q.curve > 1.0f) return "automation curve must be -1..1";
        if (q.value < lo || q.value > hi) return "synth automation value outside the parameter's range";
    }
    return nullptr;
}

namespace {

// The track's static mute, unless a mute lane replaces it (SE_AUTO_MUTE).
bool static_mute(const se_track_desc_v6& td) noexcept {
    if (td.mute == 0) return false;
    for (uint32_t i = 0; i < td.lane_count; ++i)
        if (td.lanes[i].target == SE_AUTO_MUTE && td.lanes[i].point_count > 0) return false;
    return true;
}

uint32_t add_lane(RenderGraph& g, const se_auto_lane_desc& d) {
    AutoLane l;
    l.point_begin = static_cast<uint32_t>(g.auto_points.size());
    l.point_count = d.point_count;
    l.target = d.target;
    l.log = d.logarithmic != 0;
    l.synth_param = d.synth_param;
    l.param_id = d.param_id;
    l.device_id = d.device_id;
    l.step = (d.flags & SE_AUTO_FLAG_STEPPED) != 0;
    const std::size_t at = g.auto_points.size();
    g.auto_points.resize(at + d.point_count);
    for (uint32_t i = 0; i < d.point_count; ++i)
        g.auto_points[at + i] = AutoPoint{d.points[i].frame, d.points[i].value, d.points[i].curve};
    g.lanes.push_back(l);
    return static_cast<uint32_t>(g.lanes.size() - 1);
}

// Sidechains (built-in devices): which tracks feed a tap, and a processing
// order in which every source comes before the tracks that read it.
// Sources that would form a cycle, are unknown, or exceed the tap count are
// dropped (their readers get silence).
// Routing edges (a track into the return it sends or plays to) are fixed;
// only sidechain edges are cut.
void plan_sidechains(RenderGraph& g) {
    const std::size_t n = g.tracks.size();
    g.order.resize(n);
    for (std::size_t t = 0; t < n; ++t) g.order[t] = static_cast<uint32_t>(t);
    // No sidechains and no returns: no edges, the track order stands.
    if (g.bus_count == 0 && std::none_of(g.fx.begin(), g.fx.end(), [](const FxSlot& f) { return f.sc_source != 0; }))
        return;
    std::unordered_map<uint64_t, uint32_t> index;
    for (std::size_t t = 0; t < n; ++t) index.emplace(g.tracks[t].track_id, static_cast<uint32_t>(t));
    // Edges source -> reader (track indices).
    std::vector<std::vector<uint32_t>> readers(n);
    std::vector<uint32_t> indegree(n, 0);
    bool any = false;
    std::vector<uint32_t> bus_track(g.bus_count, 0);
    for (std::size_t t = 0; t < n; ++t)
        if (g.tracks[t].bus >= 0) bus_track[static_cast<std::size_t>(g.tracks[t].bus)] = static_cast<uint32_t>(t);
    const auto route = [&](std::size_t from, int32_t bus) {
        const uint32_t to = bus_track[static_cast<std::size_t>(bus)];
        if (std::find(readers[from].begin(), readers[from].end(), to) != readers[from].end()) return;
        readers[from].push_back(to);
        ++indegree[to];
        any = true;
    };
    for (std::size_t t = 0; t < n; ++t) {
        const TrackNode& node = g.tracks[t];
        if (node.out_bus >= 0) route(t, node.out_bus);
        for (uint32_t s = node.send_begin; s < node.send_begin + node.send_count; ++s) route(t, g.sends[s].bus);
    }
    for (std::size_t t = 0; t < n; ++t) {
        const TrackNode& node = g.tracks[t];
        for (uint32_t i = 0; i < node.fx_count; ++i) {
            FxSlot& slot = g.fx[node.fx_begin + i];
            auto it = slot.sc_source != 0 ? index.find(slot.sc_source) : index.end();
            if (it == index.end() || it->second == t) {
                slot.sc_source = 0;
                continue;
            }
            readers[it->second].push_back(static_cast<uint32_t>(t));
            ++indegree[t];
            any = true;
        }
    }
    if (!any) return;
    // Kahn's algorithm, stable (lowest index first among ready tracks).
    std::vector<uint32_t> order;
    order.reserve(n);
    std::vector<bool> done(n, false);
    while (order.size() < n) {
        bool progressed = false;
        for (uint32_t t = 0; t < n; ++t) {
            if (done[t] || indegree[t] != 0) continue;
            done[t] = true;
            order.push_back(t);
            for (uint32_t r : readers[t]) --indegree[r];
            progressed = true;
            break;
        }
        if (progressed) continue;
        // Cycle: take the lowest remaining track that reads a sidechain on a
        // cycle (its source is reachable from the reader) and cut those
        // sidechains (a cycle always has one: routing alone is acyclic).
        // A sidechain that only depends on a cycle is kept.
        const auto reaches = [&](uint32_t from, uint32_t to) {
            std::vector<bool> seen(n, false);
            std::vector<uint32_t> stack{from};
            seen[from] = true;
            while (!stack.empty()) {
                const uint32_t at = stack.back();
                stack.pop_back();
                if (at == to) return true;
                for (uint32_t r : readers[at])
                    if (!done[r] && !seen[r]) {
                        seen[r] = true;
                        stack.push_back(r);
                    }
            }
            return false;
        };
        bool cut = false;
        for (uint32_t t = 0; t < n && !cut; ++t) {
            if (done[t]) continue;
            const TrackNode& node = g.tracks[t];
            for (uint32_t i = 0; i < node.fx_count; ++i) {
                FxSlot& slot = g.fx[node.fx_begin + i];
                if (slot.sc_source == 0) continue;
                const uint32_t s = index.at(slot.sc_source);
                if (!done[s] && reaches(t, s)) {
                    auto& rs = readers[s];
                    if (auto at = std::find(rs.begin(), rs.end(), t); at != rs.end()) rs.erase(at);
                    slot.sc_source = 0;
                    --indegree[t];
                    cut = true;
                }
            }
        }
        if (!cut) break; // unreachable with acyclic routing; keeps the loop finite
    }
    g.order = std::move(order);
    // Taps: one per distinct source track, in order of first use.
    for (std::size_t t = 0; t < n; ++t) {
        const TrackNode& node = g.tracks[t];
        for (uint32_t i = 0; i < node.fx_count; ++i) {
            FxSlot& slot = g.fx[node.fx_begin + i];
            if (slot.sc_source == 0) continue;
            TrackNode& src = g.tracks[index.at(slot.sc_source)];
            if (src.sc_tap < 0 && g.sc_taps < SE_BUILTIN_MAX_SIDECHAINS) src.sc_tap = static_cast<int32_t>(g.sc_taps++);
            slot.sc_tap = src.sc_tap;
        }
    }
}

// Channel strip (se_session_publish_v10): validates the strip settings and
// folds each into its track's matrix.
int32_t apply_strips(RenderGraph& g, const se_session_desc_v10& desc, std::string& err) {
    if (desc.strip_count == 0) return SE_OK;
    if (desc.strips == nullptr) {
        err = "session has strip_count > 0 but strips == NULL";
        return SE_ERR_INVALID_ARG;
    }
    std::unordered_map<uint64_t, uint32_t> index;
    for (std::size_t t = 0; t < g.tracks.size(); ++t) index.emplace(g.tracks[t].track_id, static_cast<uint32_t>(t));
    std::vector<bool> seen(g.tracks.size(), false);
    for (uint32_t i = 0; i < desc.strip_count; ++i) {
        const se_track_strip& st = desc.strips[i];
        const auto it = index.find(st.track_id);
        if (it == index.end()) {
            err = "strip names unknown track " + std::to_string(st.track_id);
            return SE_ERR_INVALID_ARG;
        }
        if (seen[it->second]) {
            err = "track " + std::to_string(st.track_id) + " has more than one strip";
            return SE_ERR_INVALID_ARG;
        }
        seen[it->second] = true;
        if (!std::isfinite(st.trim) || st.trim < 0.0f || st.trim > 16.0f) {
            err = "track " + std::to_string(st.track_id) + " has a trim outside 0..16";
            return SE_ERR_INVALID_ARG;
        }
        if (!std::isfinite(st.width) || st.width < 0.0f || st.width > 2.0f) {
            err = "track " + std::to_string(st.track_id) + " has a width outside 0..2";
            return SE_ERR_INVALID_ARG;
        }
        TrackNode& node = g.tracks[it->second];
        strip_matrix(st.trim, st.width, st.mono != 0, node.strip_a, node.strip_b);
        node.strip_trim = st.trim;
        node.strip_width = st.width;
        node.strip_mono = st.mono != 0;
        node.strip_on = node.strip_a != 1.0f || node.strip_b != 0.0f;
    }
    return SE_OK;
}

// Routing (se_session_publish_v9): validates the routes and sends, marks
// the returns and their buses, the outputs and sends, and collapses mute and
// solo into audible / main_on / send_live (see dstudio_engine.h). Without
// routes this gives every track main_on = audible, as before.
int32_t apply_routing(RenderGraph& g, const se_session_desc_v9& desc9, std::string& err) {
    const se_session_desc_v6& desc = desc9.v8.v7.v6;
    if (desc9.route_count > 0 && desc9.routes == nullptr) {
        err = "session has route_count > 0 but routes == NULL";
        return SE_ERR_INVALID_ARG;
    }
    if (desc9.send_count > 0 && desc9.sends == nullptr) {
        err = "session has send_count > 0 but sends == NULL";
        return SE_ERR_INVALID_ARG;
    }
    if (desc9.route_count == 0 && desc9.send_count == 0) {
        // No returns, outputs or sends: every track plays into the master when audible
        // (the same result as below, without the lookups).
        for (TrackNode& node : g.tracks) node.main_on = node.send_live = node.audible;
        return SE_OK;
    }
    const std::size_t n = g.tracks.size();
    std::unordered_map<uint64_t, uint32_t> index;
    for (std::size_t t = 0; t < n; ++t) index.emplace(g.tracks[t].track_id, static_cast<uint32_t>(t));
    const auto find = [&](uint64_t id) -> int64_t {
        auto it = index.find(id);
        return it == index.end() ? -1 : static_cast<int64_t>(it->second);
    };
    std::vector<uint64_t> output(n, 0);
    std::vector<bool> routed(n, false);
    for (uint32_t i = 0; i < desc9.route_count; ++i) {
        const se_track_route& r = desc9.routes[i];
        const int64_t t = find(r.track_id);
        if (t < 0) {
            err = "route names unknown track " + std::to_string(r.track_id);
            return SE_ERR_INVALID_ARG;
        }
        if (routed[static_cast<std::size_t>(t)]) {
            err = "track " + std::to_string(r.track_id) + " is routed more than once";
            return SE_ERR_INVALID_ARG;
        }
        routed[static_cast<std::size_t>(t)] = true;
        g.tracks[static_cast<std::size_t>(t)].is_return = r.is_return != 0;
        output[static_cast<std::size_t>(t)] = r.output_track_id;
    }
    // Buses, in track order.
    for (std::size_t t = 0; t < n; ++t) {
        TrackNode& node = g.tracks[t];
        if (!node.is_return) continue;
        const se_track_desc_v6& td = desc.tracks[t];
        const std::string who = "return track " + std::to_string(node.track_id);
        if (g.bus_count >= SE_MAX_RETURNS) {
            err = "more than " + std::to_string(SE_MAX_RETURNS) + " return tracks";
            return SE_ERR_INVALID_ARG;
        }
        if (td.clip_count > 0 || td.note_count > 0 || td.instrument != 0 || td.input_mode != SE_INPUT_NONE ||
            td.arm != 0 || td.monitor != 0) {
            err = who + " has clips, notes, an instrument or input";
            return SE_ERR_INVALID_ARG;
        }
        if (output[t] != 0) {
            err = who + " must play into the master";
            return SE_ERR_INVALID_ARG;
        }
        node.bus = static_cast<int32_t>(g.bus_count++);
    }
    const auto bus_of = [&](uint64_t id, uint64_t from) -> int32_t {
        const int64_t t = id == from ? -1 : find(id);
        return t < 0 ? -1 : g.tracks[static_cast<std::size_t>(t)].bus;
    };
    for (std::size_t t = 0; t < n; ++t) {
        if (output[t] == 0) continue;
        TrackNode& node = g.tracks[t];
        node.out_bus = bus_of(output[t], node.track_id);
        if (node.out_bus < 0) {
            err = "track " + std::to_string(node.track_id) + " plays into " + std::to_string(output[t]) + ", not a return track";
            return SE_ERR_INVALID_ARG;
        }
    }
    // Sends, grouped per track in description order.
    std::vector<std::vector<SendNode>> per_track(n);
    for (uint32_t i = 0; i < desc9.send_count; ++i) {
        const se_send_desc& s = desc9.sends[i];
        const int64_t t = find(s.track_id);
        if (t < 0) {
            err = "send from unknown track " + std::to_string(s.track_id);
            return SE_ERR_INVALID_ARG;
        }
        auto& list = per_track[static_cast<std::size_t>(t)];
        const std::string who = "send from track " + std::to_string(s.track_id) + " to " + std::to_string(s.return_track_id);
        const int32_t bus = bus_of(s.return_track_id, s.track_id);
        if (bus < 0) {
            err = who + ": sends go into a return track other than the sender";
            return SE_ERR_INVALID_ARG;
        }
        if (!std::isfinite(s.gain) || s.gain < 0.0f || s.gain > 4.0f) {
            err = who + ": gain must be finite in 0..4";
            return SE_ERR_INVALID_ARG;
        }
        for (const SendNode& o : list)
            if (o.bus == bus) {
                err = who + ": the track already sends there";
                return SE_ERR_INVALID_ARG;
            }
        SendNode node;
        node.return_id = s.return_track_id;
        node.bus = bus;
        node.gain = s.gain;
        node.pre_fader = s.pre_fader != 0;
        list.push_back(node);
    }
    for (std::size_t t = 0; t < n; ++t) {
        TrackNode& node = g.tracks[t];
        node.send_begin = static_cast<uint32_t>(g.sends.size());
        node.send_count = static_cast<uint32_t>(per_track[t].size());
        g.sends.insert(g.sends.end(), per_track[t].begin(), per_track[t].end());
    }
    // A return may send into other returns (a group track's sends), but the
    // buses must not form a cycle: depth-first over return -> return sends.
    {
        std::vector<int32_t> bus_track(g.bus_count, -1);
        for (std::size_t t = 0; t < n; ++t)
            if (g.tracks[t].bus >= 0) bus_track[static_cast<std::size_t>(g.tracks[t].bus)] = static_cast<int32_t>(t);
        std::vector<uint8_t> state(g.bus_count, 0); // 0 new, 1 on the path, 2 done
        std::vector<std::pair<uint32_t, uint32_t>> stack; // (bus, next send to look at)
        for (uint32_t b0 = 0; b0 < g.bus_count; ++b0) {
            if (state[b0] != 0) continue;
            stack.push_back({b0, 0});
            state[b0] = 1;
            while (!stack.empty()) {
                auto& [b, i] = stack.back();
                const TrackNode& node = g.tracks[static_cast<std::size_t>(bus_track[b])];
                if (i == node.send_count) {
                    state[b] = 2;
                    stack.pop_back();
                    continue;
                }
                const auto to = static_cast<uint32_t>(g.sends[node.send_begin + i++].bus);
                if (state[to] == 1) {
                    err = "sends between return track " + std::to_string(node.track_id) + " and " +
                          std::to_string(g.tracks[static_cast<std::size_t>(bus_track[to])].track_id) + " form a loop";
                    return SE_ERR_INVALID_ARG;
                }
                if (state[to] == 0) {
                    state[to] = 1;
                    stack.push_back({to, 0});
                }
            }
        }
    }
    // Mute and solo.
    bool solo_track = false, solo_return = false;
    for (std::size_t t = 0; t < n; ++t)
        (g.tracks[t].is_return ? solo_return : solo_track) |= desc.tracks[t].solo != 0;
    for (std::size_t t = 0; t < n; ++t) {
        TrackNode& node = g.tracks[t];
        const se_track_desc_v6& td = desc.tracks[t];
        const bool on = !static_mute(td);
        if (node.is_return) {
            node.main_on = on && (!solo_return || td.solo != 0);
            node.send_live = node.main_on;
        } else {
            node.send_live = on && (!solo_track || td.solo != 0);
            node.main_on = node.out_bus >= 0 ? node.send_live : on && ((!solo_track && !solo_return) || td.solo != 0);
        }
        node.audible = node.main_on || (node.send_live && node.send_count > 0);
        node.plain = node.send_count == 0 && node.out_bus < 0 && node.bus < 0;
    }
    return SE_OK;
}

} // namespace

int32_t build_render_graph(const se_session_desc_v6& desc, const AssetLookup& lookup,
                           std::unique_ptr<RenderGraph>& out, std::string& err) {
    se_session_desc_v7 d7{};
    d7.v6 = desc;
    return build_render_graph(d7, lookup, out, err);
}

int32_t build_render_graph(const se_session_desc_v7& desc, const AssetLookup& lookup,
                           std::unique_ptr<RenderGraph>& out, std::string& err) {
    se_session_desc_v8 d8{};
    d8.v7 = desc;
    return build_render_graph(d8, lookup, out, err);
}

int32_t build_render_graph(const se_session_desc_v8& desc, const AssetLookup& lookup,
                           std::unique_ptr<RenderGraph>& out, std::string& err) {
    se_session_desc_v9 d9{};
    d9.v8 = desc;
    return build_render_graph(d9, lookup, out, err);
}

int32_t build_render_graph(const se_session_desc_v9& desc, const AssetLookup& lookup,
                           std::unique_ptr<RenderGraph>& out, std::string& err) {
    se_session_desc_v10 d10{};
    d10.v9 = desc;
    return build_render_graph(d10, lookup, out, err);
}

int32_t build_render_graph(const se_session_desc_v10& desc, const AssetLookup& lookup,
                           std::unique_ptr<RenderGraph>& out, std::string& err) {
    se_session_desc_v11 d11{};
    d11.v10 = desc;
    return build_render_graph(d11, lookup, out, err);
}

int32_t build_render_graph(const se_session_desc_v11& desc11, const AssetLookup& lookup,
                           std::unique_ptr<RenderGraph>& out, std::string& err) {
    const se_session_desc_v10& desc10 = desc11.v10;
    const se_session_desc_v9& desc9 = desc10.v9;
    const se_session_desc_v8& desc8 = desc9.v8;
    const se_session_desc_v7& desc7 = desc8.v7;
    const se_session_desc_v6& desc = desc7.v6;
    if (desc.track_count > 0 && desc.tracks == nullptr) {
        err = "session has track_count > 0 but tracks == NULL";
        return SE_ERR_INVALID_ARG;
    }
    if (desc7.warp_count > 0 && desc7.warps == nullptr) {
        err = "session has warp_count > 0 but warps == NULL";
        return SE_ERR_INVALID_ARG;
    }
    std::unordered_map<uint64_t, const se_clip_warp_desc*> warp_of;
    for (uint32_t i = 0; i < desc7.warp_count; ++i) {
        const se_clip_warp_desc& w = desc7.warps[i];
        if (const char* why = validate_warp(w)) {
            err = "clip " + std::to_string(w.clip_id) + " warp: " + why;
            return SE_ERR_INVALID_ARG;
        }
        if (!warp_of.emplace(w.clip_id, &w).second) {
            err = "clip " + std::to_string(w.clip_id) + " is warped more than once";
            return SE_ERR_INVALID_ARG;
        }
    }
    std::unordered_map<uint64_t, uint32_t> warp_hits; // clip id -> clips carrying it
    if (desc8.loop_count > 0 && desc8.loops == nullptr) {
        err = "session has loop_count > 0 but loops == NULL";
        return SE_ERR_INVALID_ARG;
    }
    std::unordered_map<uint64_t, const se_clip_loop_desc*> loop_of;
    for (uint32_t i = 0; i < desc8.loop_count; ++i) {
        const se_clip_loop_desc& l = desc8.loops[i];
        if (l.loop_start < 0 || l.loop_end <= l.loop_start) {
            err = "clip " + std::to_string(l.clip_id) + " loop: needs 0 <= loop_start < loop_end";
            return SE_ERR_INVALID_ARG;
        }
        if (!loop_of.emplace(l.clip_id, &l).second) {
            err = "clip " + std::to_string(l.clip_id) + " is looped more than once";
            return SE_ERR_INVALID_ARG;
        }
    }
    std::unordered_map<uint64_t, uint32_t> loop_hits;
    if (desc11.fade_count > 0 && desc11.fades == nullptr) {
        err = "session has fade_count > 0 but fades == NULL";
        return SE_ERR_INVALID_ARG;
    }
    // Faded clips sorted by id, each with the number of clips that use it. Nearly every
    // session has none, so this stays empty and costs nothing per clip.
    struct FadeEntry {
        const se_clip_fade_desc* desc;
        uint32_t hits;
    };
    std::vector<FadeEntry> fade_of;
    fade_of.reserve(desc11.fade_count);
    for (uint32_t i = 0; i < desc11.fade_count; ++i) {
        const se_clip_fade_desc& f = desc11.fades[i];
        if (f.fade_in < 0 || f.fade_out < 0) {
            err = "clip " + std::to_string(f.clip_id) + " fades: needs fade_in >= 0 and fade_out >= 0";
            return SE_ERR_INVALID_ARG;
        }
        fade_of.push_back({&f, 0});
    }
    std::sort(fade_of.begin(), fade_of.end(),
              [](const FadeEntry& x, const FadeEntry& y) { return x.desc->clip_id < y.desc->clip_id; });
    for (std::size_t i = 1; i < fade_of.size(); ++i) {
        if (fade_of[i].desc->clip_id == fade_of[i - 1].desc->clip_id) {
            err = "clip " + std::to_string(fade_of[i].desc->clip_id) + " has fades more than once";
            return SE_ERR_INVALID_ARG;
        }
    }
    if (desc.master_lane_count > 0 && desc.master_lanes == nullptr) {
        err = "session has master_lane_count > 0 but master_lanes == NULL";
        return SE_ERR_INVALID_ARG;
    }
    if (!std::isfinite(desc.master_gain)) {
        err = "master_gain is not finite";
        return SE_ERR_INVALID_ARG;
    }

    auto g = std::make_unique<RenderGraph>();
    g->master_gain = desc.master_gain;
    g->tracks.reserve(desc.track_count);
    g->synth_params.reserve(desc.track_count);

    bool any_solo = false;
    for (uint32_t t = 0; t < desc.track_count; ++t) any_solo |= desc.tracks[t].solo != 0;

    std::unordered_map<uint64_t, AssetRef> used; // dedupe keep-alive refs

    std::unordered_map<uint64_t, uint64_t> device_ids; // device id -> track id
    for (uint32_t t = 0; t < desc.track_count; ++t) {
        const se_track_desc_v6& td = desc.tracks[t];
        if (td.device_count > 0 && td.devices == nullptr) {
            err = "track " + std::to_string(td.track_id) + " has device_count > 0 but devices == NULL";
            return SE_ERR_INVALID_ARG;
        }
        const auto check_device = [&](const se_device_desc& d) -> int32_t {
            if (const char* why = validate_device(d)) {
                err = "track " + std::to_string(td.track_id) + " device " + std::to_string(d.device_id) + ": " + why;
                return SE_ERR_INVALID_ARG;
            }
            if (!device_ids.emplace(d.device_id, td.track_id).second) {
                err = "device id " + std::to_string(d.device_id) + " is used more than once";
                return SE_ERR_INVALID_ARG;
            }
            return SE_OK;
        };
        for (uint32_t i = 0; i < td.device_count; ++i)
            if (const int32_t rc = check_device(td.devices[i]); rc != SE_OK) return rc;
        if (td.instrument_plugin != nullptr && td.instrument != 0)
            if (const int32_t rc = check_device(*td.instrument_plugin); rc != SE_OK) return rc;
        if (td.clip_count > 0 && td.clips == nullptr) {
            err = "track " + std::to_string(td.track_id) + " has clip_count > 0 but clips == NULL";
            return SE_ERR_INVALID_ARG;
        }
        if (!std::isfinite(td.gain) || !std::isfinite(td.pan)) {
            err = "track " + std::to_string(td.track_id) + " has a non-finite gain or pan";
            return SE_ERR_INVALID_ARG;
        }
        if (td.note_count > 0 && td.notes == nullptr) {
            err = "track " + std::to_string(td.track_id) + " has note_count > 0 but notes == NULL";
            return SE_ERR_INVALID_ARG;
        }

        if (td.input_mode > SE_INPUT_STEREO) {
            err = "track " + std::to_string(td.track_id) + " has an invalid input_mode";
            return SE_ERR_INVALID_ARG;
        }

        se_synth_params params = default_synth_params();
        if (td.synth != nullptr) {
            params = *td.synth;
            std::memset(params._pad, 0, sizeof params._pad); // reserved bytes are ignored
            if (const char* why = validate_synth_params(params)) {
                err = "track " + std::to_string(td.track_id) + ": " + why;
                return SE_ERR_INVALID_ARG;
            }
        }

        if (td.lane_count > 0 && td.lanes == nullptr) {
            err = "track " + std::to_string(td.track_id) + " has lane_count > 0 but lanes == NULL";
            return SE_ERR_INVALID_ARG;
        }
        for (uint32_t i = 0; i < td.lane_count; ++i)
            if (const char* why = validate_lane(td.lanes[i])) {
                err = "track " + std::to_string(td.track_id) + " automation lane " + std::to_string(i) + ": " + why;
                return SE_ERR_INVALID_ARG;
            }

        TrackNode node;
        node.track_id = td.track_id;
        float pl = 0.0f, pr = 0.0f;
        constant_power_pan(td.pan, pl, pr);
        node.gain_l = td.gain * pl;
        node.gain_r = td.gain * pr;
        node.gain = td.gain;
        node.pan = td.pan;
        // Mute always wins; with any solo active only soloed tracks are audible.
        node.audible = !static_mute(td) && (!any_solo || td.solo != 0);
        node.clip_begin = static_cast<uint32_t>(g->clips.size());
        node.arm = td.arm != 0;
        node.monitor = td.monitor != 0;
        node.input_mode = td.input_mode;
        node.input_channel = td.input_channel;
        if (node.input_mode != SE_INPUT_NONE && (node.arm || node.monitor)) g->uses_input = true;

        for (uint32_t c = 0; c < td.clip_count; ++c) {
            const se_clip_desc& cd = td.clips[c];
            if (cd.source_offset < 0 || cd.length < 0) {
                err = "clip " + std::to_string(cd.clip_id) + " has a negative source_offset or length";
                return SE_ERR_INVALID_ARG;
            }
            if (!std::isfinite(cd.gain)) {
                err = "clip " + std::to_string(cd.clip_id) + " has a non-finite gain";
                return SE_ERR_INVALID_ARG;
            }
            if (cd.length > std::numeric_limits<int64_t>::max() - std::max<int64_t>(cd.timeline_start, 0)) {
                err = "clip " + std::to_string(cd.clip_id) + " extends past the end of the timeline";
                return SE_ERR_INVALID_ARG;
            }

            // Fades (se_session_publish_v11), clamped to the clip's played range once it is known.
            const se_clip_fade_desc* fade = nullptr;
            if (!fade_of.empty()) {
                const auto ft = std::lower_bound(fade_of.begin(), fade_of.end(), cd.clip_id,
                                                 [](const FadeEntry& e, uint64_t id) { return e.desc->clip_id < id; });
                if (ft != fade_of.end() && ft->desc->clip_id == cd.clip_id) {
                    ++ft->hits;
                    fade = ft->desc;
                }
            }
            const auto set_fades = [fade](ClipNode& node_clip) {
                if (fade == nullptr) return;
                const int64_t len = node_clip.timeline_end - node_clip.timeline_start;
                node_clip.fade_in = std::clamp<int64_t>(fade->fade_in, 0, len);
                node_clip.fade_out = std::clamp<int64_t>(fade->fade_out, 0, len - node_clip.fade_in);
            };

            AssetRef asset;
            if (auto it = used.find(cd.asset_id); it != used.end()) {
                asset = it->second;
            } else {
                asset = lookup(cd.asset_id);
                if (!asset) {
                    err = "clip " + std::to_string(cd.clip_id) + " references unknown asset " +
                          std::to_string(cd.asset_id);
                    return SE_ERR_NOT_FOUND;
                }
                used.emplace(cd.asset_id, asset);
            }

            const auto asset_frames = static_cast<int64_t>(asset->frames);
            // The loop end, clamped to the content there is; 0 = plays once.
            const se_clip_loop_desc* loop = nullptr;
            if (auto lt = loop_of.find(cd.clip_id); lt != loop_of.end()) {
                ++loop_hits[cd.clip_id];
                loop = lt->second;
            }
            const auto loop_end_within = [&](int64_t content) {
                if (loop == nullptr) return int64_t{0};
                const int64_t end = std::min({loop->loop_end, content, cd.length});
                return end > loop->loop_start ? end : int64_t{0};
            };
            if (auto wt = warp_of.find(cd.clip_id); wt != warp_of.end()) {
                // Warped: the clip covers its timeline frames; the map says what plays.
                ++warp_hits[cd.clip_id];
                if (cd.length <= 0 || asset_frames == 0) continue;
                ClipNode clip;
                clip.src_l = asset->channel(0);
                clip.src_r = asset->channel(1);
                clip.timeline_start = cd.timeline_start;
                clip.timeline_end = cd.timeline_start + cd.length;
                clip.source_offset = cd.source_offset;
                clip.gain = cd.gain;
                clip.source_frames = asset_frames;
                if ((clip.loop_end = loop_end_within(cd.length)) > 0) clip.loop_start = loop->loop_start;
                // A looped clip only ever plays (and renders) content below its loop end.
                clip.warp = add_warp(*g, *wt->second, asset, cd.timeline_start,
                                     clip.loop_end > 0 ? clip.loop_end : cd.length);
                set_fades(clip);
                g->clips.push_back(clip);
                continue;
            }
            // Clamp the played range to the asset (a looped clip: its loop); drop empty clips.
            if (cd.source_offset >= asset_frames) continue;
            const int64_t loop_end = loop_end_within(asset_frames - cd.source_offset);
            const int64_t len = loop_end > 0 ? cd.length : std::min(cd.length, asset_frames - cd.source_offset);
            if (len <= 0) continue;

            ClipNode clip;
            clip.src_l = asset->channel(0);
            clip.src_r = asset->channel(1);
            clip.timeline_start = cd.timeline_start;
            clip.timeline_end = cd.timeline_start + len;
            clip.source_offset = cd.source_offset;
            clip.gain = cd.gain;
            clip.source_frames = asset_frames;
            if ((clip.loop_end = loop_end) > 0) clip.loop_start = loop->loop_start;
            set_fades(clip);
            g->clips.push_back(clip);
        }

        node.clip_count = static_cast<uint32_t>(g->clips.size()) - node.clip_begin;
        std::sort(g->clips.begin() + node.clip_begin, g->clips.end(),
                  [](const ClipNode& a, const ClipNode& b) { return a.timeline_start < b.timeline_start; });

        // Notes: validated always, kept only for instrument tracks.
        node.instrument = td.instrument != 0;
        node.note_begin = static_cast<uint32_t>(g->notes.size());
        for (uint32_t n = 0; n < td.note_count; ++n) {
            const se_note_desc& nd = td.notes[n];
            if (nd.pitch > 127 || nd.velocity == 0 || nd.velocity > 127) {
                err = "track " + std::to_string(td.track_id) + " note " + std::to_string(n) +
                      ": pitch must be 0..127 and velocity 1..127";
                return SE_ERR_INVALID_ARG;
            }
            if (!node.instrument || nd.end <= nd.start) continue;
            g->notes.push_back(NoteEvent{nd.start, nd.end, nd.pitch, nd.velocity});
        }
        node.note_count = static_cast<uint32_t>(g->notes.size()) - node.note_begin;
        node.fx_begin = static_cast<uint32_t>(g->fx.size());
        node.fx_count = td.device_count;
        for (uint32_t i = 0; i < td.device_count; ++i) {
            const se_device_desc& dd = td.devices[i];
            FxSlot slot{nullptr, dd.device_id, dd.enabled != 0, 0};
            if (dd.format == SE_PLUGIN_BUILTIN) {
                const uint8_t* values = nullptr;
                uint32_t count = 0;
                parse_builtin_state(dd.state, dd.state_len, values, count, slot.sc_source); // validated above
                slot.sc_wanted = slot.sc_source != 0;
                if (slot.sc_source == td.track_id) slot.sc_source = 0;
            }
            g->fx.push_back(slot);
        }
        node.has_instrument_plugin = node.instrument && td.instrument_plugin != nullptr;
        if (node.has_instrument_plugin) node.instrument_device_id = td.instrument_plugin->device_id;
        node.instrument_plugin_enabled = node.has_instrument_plugin && td.instrument_plugin->enabled != 0;
        if (node.fx_count > 0 || node.has_instrument_plugin) g->has_plugins = true;

        // Automation lanes (last lane per target wins; empty lanes ignored).
        {
            const auto last_of = [&](auto&& pred) -> const se_auto_lane_desc* {
                for (uint32_t i = td.lane_count; i-- > 0;)
                    if (td.lanes[i].point_count > 0 && pred(td.lanes[i])) return &td.lanes[i];
                return nullptr;
            };
            if (const auto* d = last_of([](const se_auto_lane_desc& l) { return l.target == SE_AUTO_VOLUME; }))
                node.vol_lane = static_cast<int32_t>(add_lane(*g, *d));
            if (const auto* d = last_of([](const se_auto_lane_desc& l) { return l.target == SE_AUTO_PAN; }))
                node.pan_lane = static_cast<int32_t>(add_lane(*g, *d));
            if (const auto* d = last_of([](const se_auto_lane_desc& l) { return l.target == SE_AUTO_MUTE; }))
                node.mute_lane = static_cast<int32_t>(add_lane(*g, *d));
            if (const auto* d = last_of([](const se_auto_lane_desc& l) { return l.target == SE_AUTO_TRIM; }))
                node.trim_lane = static_cast<int32_t>(add_lane(*g, *d));
            if (const auto* d = last_of([](const se_auto_lane_desc& l) { return l.target == SE_AUTO_WIDTH; }))
                node.width_lane = static_cast<int32_t>(add_lane(*g, *d));
            if (const auto* d = last_of([](const se_auto_lane_desc& l) { return l.target == SE_AUTO_MONO; }))
                node.mono_lane = static_cast<int32_t>(add_lane(*g, *d));
            // Power switch lanes (SE_AUTO_DEVICE_ON), one per device.
            const auto on_lane = [&](uint64_t device_id) -> int32_t {
                const auto* d = last_of([device_id](const se_auto_lane_desc& l) {
                    return l.target == SE_AUTO_DEVICE_ON && l.device_id == device_id;
                });
                return d != nullptr ? static_cast<int32_t>(add_lane(*g, *d)) : -1;
            };
            if (node.has_instrument_plugin) node.iplug_on_lane = on_lane(td.instrument_plugin->device_id);
            node.switch_lanes = node.mute_lane >= 0 || node.trim_lane >= 0 || node.width_lane >= 0 ||
                                node.mono_lane >= 0 || node.iplug_on_lane >= 0;
            for (uint32_t i = 0; i < node.fx_count; ++i) {
                FxSlot& slot = g->fx[node.fx_begin + i];
                slot.on_lane = on_lane(slot.device_id);
            }
            node.synth_lane_begin = static_cast<uint32_t>(g->lanes.size());
            if (node.instrument && !node.has_instrument_plugin)
                for (uint16_t sp = 0; sp < SE_SYNTH_P_COUNT; ++sp)
                    if (const auto* d = last_of([sp](const se_auto_lane_desc& l) {
                            return l.target == SE_AUTO_SYNTH && l.synth_param == sp;
                        }))
                        add_lane(*g, *d);
            node.synth_lane_count = static_cast<uint32_t>(g->lanes.size()) - node.synth_lane_begin;
            // Plugin lanes of one device: one per parameter (the last given).
            const auto device_lanes = [&](uint64_t device_id, uint32_t& begin, uint32_t& count) {
                begin = static_cast<uint32_t>(g->lanes.size());
                for (uint32_t i = 0; i < td.lane_count; ++i) {
                    const se_auto_lane_desc& d = td.lanes[i];
                    const bool device_lane = d.target == SE_AUTO_PLUGIN || d.target == SE_AUTO_BUILTIN;
                    if (!device_lane || d.device_id != device_id || d.point_count == 0) continue;
                    bool later = false;
                    for (uint32_t j = i + 1; j < td.lane_count && !later; ++j)
                        later = td.lanes[j].target == d.target && td.lanes[j].device_id == device_id &&
                                td.lanes[j].param_id == d.param_id && td.lanes[j].point_count > 0;
                    if (!later) add_lane(*g, d);
                }
                count = static_cast<uint32_t>(g->lanes.size()) - begin;
                if (count > 0) g->has_plugin_lanes = true;
            };
            if (node.has_instrument_plugin)
                device_lanes(td.instrument_plugin->device_id, node.iplug_lane_begin, node.iplug_lane_count);
            for (uint32_t i = 0; i < node.fx_count; ++i) {
                FxSlot& slot = g->fx[node.fx_begin + i];
                device_lanes(slot.device_id, slot.lane_begin, slot.lane_count);
            }
        }
        // Modulation: built-in devices carrying routes are sources; their
        // routes are grouped per target device and parameter.
        {
            struct Pending {
                int32_t target; // device index in the track, -1 = instrument plugin
                uint32_t param;
                bool plugin;
                se_mod_route r;
                uint32_t src;
            };
            std::vector<Pending> pend;
            node.msrc_begin = static_cast<uint32_t>(g->mod_sources.size());
            for (uint32_t i = 0; i < td.device_count; ++i) {
                const se_device_desc& dd = td.devices[i];
                if (dd.format != SE_PLUGIN_BUILTIN) continue;
                BuiltinStateView view;
                parse_builtin_state(dd.state, dd.state_len, view); // validated above
                const uint8_t* const routes = view.routes;
                const uint32_t rc = view.route_count;
                if (rc == 0) {
                    const BuiltinDescriptor* const bd = dd.plugin_id != nullptr ? find_builtin(dd.plugin_id) : nullptr;
                    if (bd == nullptr || !bd->mod_source) continue;
                }
                const uint32_t src = node.fx_begin + i;
                g->fx[src].mod_source = true;
                g->mod_sources.push_back(src);
                for (uint32_t k = 0; k < rc; ++k) {
                    se_mod_route r;
                    std::memcpy(&r, routes + sizeof r * k, sizeof r);
                    int32_t target = -2;
                    for (uint32_t j = 0; j < td.device_count; ++j)
                        if (j != i && td.devices[j].device_id == r.target_device_id) target = static_cast<int32_t>(j);
                    if (target == -2 && node.has_instrument_plugin && td.instrument_plugin->device_id == r.target_device_id)
                        target = -1;
                    if (target != -2) pend.push_back(Pending{target, r.param_id, r.kind == SE_MOD_PLUGIN, r, src});
                }
            }
            node.msrc_count = static_cast<uint32_t>(g->mod_sources.size()) - node.msrc_begin;
            std::stable_sort(pend.begin(), pend.end(), [](const Pending& a, const Pending& b) {
                return std::tie(a.target, a.plugin, a.param) < std::tie(b.target, b.plugin, b.param);
            });
            for (std::size_t a = 0; a < pend.size();) {
                std::size_t b = a;
                while (b < pend.size() && pend[b].target == pend[a].target && pend[b].plugin == pend[a].plugin &&
                       pend[b].param == pend[a].param)
                    ++b;
                const se_mod_route& r0 = pend[a].r;
                ModTarget mt;
                mt.param_id = pend[a].param;
                mt.plugin = pend[a].plugin;
                mt.log = r0.log != 0;
                mt.p_min = r0.p_min;
                mt.p_max = r0.p_max;
                mt.p_skew = r0.p_skew;
                mt.p_step = r0.p_step;
                mt.route_begin = static_cast<uint32_t>(g->mod_routes.size());
                for (std::size_t k = a; k < b; ++k)
                    g->mod_routes.push_back(ModRoute{pend[k].src, pend[k].r.slot, pend[k].r.absolute != 0, pend[k].r.min, pend[k].r.max});
                mt.route_count = static_cast<uint32_t>(b - a);
                const auto at = static_cast<uint32_t>(g->mod_targets.size());
                if (pend[a].target < 0) {
                    if (node.imod_count++ == 0) node.imod_begin = at;
                } else {
                    FxSlot& s = g->fx[node.fx_begin + static_cast<uint32_t>(pend[a].target)];
                    if (s.mod_count++ == 0) s.mod_begin = at;
                }
                g->mod_targets.push_back(mt);
                a = b;
            }
        }
        std::stable_sort(g->notes.begin() + node.note_begin, g->notes.end(), [](const NoteEvent& a, const NoteEvent& b) {
            return a.start != b.start ? a.start < b.start : a.pitch < b.pitch;
        });
        g->tracks.push_back(node);
        g->synth_params.push_back(params);
    }

    for (uint32_t i = desc.master_lane_count; i-- > 0;) {
        const se_auto_lane_desc& d = desc.master_lanes[i];
        if (const char* why = validate_lane(d)) {
            err = std::string("master automation lane ") + std::to_string(i) + ": " + why;
            return SE_ERR_INVALID_ARG;
        }
    }
    for (uint32_t i = desc.master_lane_count; i-- > 0;)
        if (desc.master_lanes[i].target == SE_AUTO_VOLUME && desc.master_lanes[i].point_count > 0) {
            g->master_vol_lane = static_cast<int32_t>(add_lane(*g, desc.master_lanes[i]));
            break;
        }

    g->assets.reserve(used.size());
    for (auto& [id, ref] : used) g->assets.push_back(std::move(ref));

    // Every warp must name exactly one clip of the session.
    for (const auto& [id, w] : warp_of) {
        const auto hit = warp_hits.find(id);
        if (hit == warp_hits.end()) {
            err = "warp names unknown clip " + std::to_string(id);
            return SE_ERR_INVALID_ARG;
        }
        if (hit->second > 1) {
            err = "warped clip id " + std::to_string(id) + " is used by more than one clip";
            return SE_ERR_INVALID_ARG;
        }
    }
    for (const auto& [id, l] : loop_of) {
        const auto hit = loop_hits.find(id);
        if (hit == loop_hits.end()) {
            err = "loop names unknown clip " + std::to_string(id);
            return SE_ERR_INVALID_ARG;
        }
        if (hit->second > 1) {
            err = "looped clip id " + std::to_string(id) + " is used by more than one clip";
            return SE_ERR_INVALID_ARG;
        }
    }
    for (const FadeEntry& e : fade_of) {
        if (e.hits == 0) {
            err = "fades name unknown clip " + std::to_string(e.desc->clip_id);
            return SE_ERR_INVALID_ARG;
        }
        if (e.hits > 1) {
            err = "faded clip id " + std::to_string(e.desc->clip_id) + " is used by more than one clip";
            return SE_ERR_INVALID_ARG;
        }
    }
    assign_warp_voices(*g);
    g->warp_voice_refs.resize(g->warp_voice_keys.size());
    g->warp_voices.assign(g->warp_voice_keys.size(), nullptr);

    g->synth_refs.resize(g->tracks.size());
    g->synths.assign(g->tracks.size(), nullptr);
    g->midi_refs.resize(g->tracks.size());
    g->midi_tracks.assign(g->tracks.size(), nullptr);
    g->midi_explicit = std::any_of(g->tracks.begin(), g->tracks.end(),
                                   [](const TrackNode& t) { return t.instrument && (t.arm || t.monitor); });
    if (const int32_t rc = apply_routing(*g, desc9, err); rc != SE_OK) return rc;
    if (const int32_t rc = apply_strips(*g, desc10, err); rc != SE_OK) return rc;
    plan_sidechains(*g);

    g->meter_count = g->tracks.size();
    g->meters = std::make_unique<TrackMeter[]>(g->meter_count);
    for (std::size_t i = 0; i < g->meter_count; ++i) g->meters[i].track_id = g->tracks[i].track_id;

    out = std::move(g);
    return SE_OK;
}

std::unique_ptr<RenderGraph> clone_render_graph(const RenderGraph& g) {
    auto c = std::make_unique<RenderGraph>();
    c->master_gain = g.master_gain;
    c->uses_input = g.uses_input;
    c->tracks = g.tracks;
    c->clips = g.clips;
    c->notes = g.notes;
    c->fx = g.fx;
    c->sends = g.sends;
    c->bus_count = g.bus_count;
    c->order = g.order;
    c->sc_taps = g.sc_taps;
    c->has_plugins = g.has_plugins;
    c->lanes = g.lanes;
    c->auto_points = g.auto_points;
    c->mod_targets = g.mod_targets;
    c->mod_routes = g.mod_routes;
    c->mod_sources = g.mod_sources;
    c->mod_base_refs = g.mod_base_refs;
    // Restores the audio thread has not sent yet go with the clone.
    if (!g.restores_done.load(std::memory_order_acquire)) c->mod_restores = g.mod_restores;
    c->master_vol_lane = g.master_vol_lane;
    c->has_plugin_lanes = g.has_plugin_lanes;
    c->synth_params = g.synth_params;
    c->assets = g.assets;
    c->synth_refs = g.synth_refs;
    c->synths = g.synths;
    c->midi_refs = g.midi_refs;
    c->midi_tracks = g.midi_tracks;
    c->midi_explicit = g.midi_explicit;
    c->meter_count = g.meter_count;
    c->meters = std::make_unique<TrackMeter[]>(c->meter_count);
    for (std::size_t i = 0; i < c->meter_count; ++i) {
        const TrackMeter& from = g.meters[i];
        TrackMeter& to = c->meters[i];
        to.track_id = from.track_id;
        to.peak_l.store(from.peak_l.load(std::memory_order_relaxed), std::memory_order_relaxed);
        to.peak_r.store(from.peak_r.load(std::memory_order_relaxed), std::memory_order_relaxed);
        for (uint32_t k = 0; k < kPeekBuckets; ++k) {
            to.win_l[k].store(from.win_l[k].load(std::memory_order_relaxed), std::memory_order_relaxed);
            to.win_r[k].store(from.win_r[k].load(std::memory_order_relaxed), std::memory_order_relaxed);
        }
    }
    c->pdc = g.pdc;
    c->line_refs = g.line_refs;
    c->lines = g.lines;
    c->sample_rate = g.sample_rate;
    c->warps = g.warps;
    c->warp_segments = g.warp_segments;
    c->warp_slices = g.warp_slices;
    c->warp_voice_keys = g.warp_voice_keys;
    c->warp_voice_refs = g.warp_voice_refs;
    c->warp_voices = g.warp_voices;
    return c;
}

} // namespace dstudio
