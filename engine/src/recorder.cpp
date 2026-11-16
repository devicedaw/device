#include "recorder.hpp"

#include "platform/os.hpp"
#include "dstudio_engine.h"

#include <algorithm>
#include <chrono>
#include <cmath>
#include <ctime>
#include <filesystem>
#include <limits>

namespace dstudio {

namespace {
std::size_t next_pow2(std::size_t v) {
    std::size_t p = 1;
    while (p < v) p <<= 1;
    return p;
}
} // namespace

// ============================================================ FloatRing

FloatRing::FloatRing(std::size_t capacity_pow2)
    : buf_(std::make_unique<float[]>(capacity_pow2)), mask_(capacity_pow2 - 1) {}

void FloatRing::write_interleaved(const float* const* src, uint32_t channels, uint32_t frames) noexcept {
    const uint64_t head = head_.load(std::memory_order_relaxed);
    float* const buf = buf_.get();
    // Capacity is a power of two >= channels (1 or 2), so a frame never
    // straddles the wrap: split by frames.
    const std::size_t cap_frames = capacity() / channels;
    const std::size_t first_frame = static_cast<std::size_t>(head / channels) & (cap_frames - 1);
    const std::size_t n1 = std::min<std::size_t>(frames, cap_frames - first_frame);
    if (channels == 1) {
        std::copy_n(src[0], n1, buf + first_frame);
        std::copy_n(src[0] + n1, frames - n1, buf);
    } else {
        float* d = buf + first_frame * channels;
        for (std::size_t i = 0; i < n1; ++i, d += 2) {
            d[0] = src[0][i];
            d[1] = src[1][i];
        }
        d = buf;
        for (std::size_t i = n1; i < frames; ++i, d += 2) {
            d[0] = src[0][i];
            d[1] = src[1][i];
        }
    }
    head_.store(head + static_cast<uint64_t>(frames) * channels, std::memory_order_release);
}

const float* FloatRing::span(uint64_t pos, std::size_t max, std::size_t& n) const noexcept {
    const std::size_t off = static_cast<std::size_t>(pos) & mask_;
    n = std::min(max, capacity() - off);
    return buf_.get() + off;
}

// ============================================================ Recorder

Recorder::Recorder(std::string dir, double sample_rate) : dir_(std::move(dir)), sample_rate_(sample_rate) {}

std::unique_ptr<Recorder> Recorder::create(const std::string& dir, std::vector<RecordTrackSpec> tracks,
                                           double sample_rate, std::size_t ring_frames, std::string& err) {
    if (sample_rate != std::floor(sample_rate) || sample_rate <= 0.0 || sample_rate > 4294967295.0) {
        err = "engine sample rate is not representable in a WAV header";
        return nullptr;
    }
    std::error_code ec;
    const std::filesystem::path dir_path = os::path_from_utf8(dir);
    std::filesystem::create_directories(dir_path, ec);
    if (!std::filesystem::is_directory(dir_path, ec)) {
        err = "cannot create recording directory: " + dir;
        return nullptr;
    }
    std::unique_ptr<Recorder> r(new Recorder(dir, sample_rate));
    // File names: rec-<YYYYmmdd-HHMMSS>-<n>-t<track>-<take>.wav. Digits and
    // fixed text only, so they are safe on every file system. <n> is a
    // per-process counter so two recordings in one second differ.
    static std::atomic<unsigned> session_counter{0};
    const std::time_t now = std::time(nullptr);
    std::tm tm{};
#if defined(_WIN32)
    localtime_s(&tm, &now);
#else
    localtime_r(&now, &tm);
#endif
    char stamp[32];
    std::strftime(stamp, sizeof stamp, "%Y%m%d-%H%M%S", &tm);
    r->name_prefix_ = std::string("rec-") + stamp + "-" + std::to_string(session_counter.fetch_add(1) + 1);

    const std::size_t frames_pow2 = next_pow2(std::max<std::size_t>(ring_frames, 1024));
    for (const RecordTrackSpec& spec : tracks) {
        auto slot = std::make_unique<Slot>();
        slot->spec = spec;
        slot->ring = std::make_unique<FloatRing>(frames_pow2 * spec.channels);
        slot->pending.reserve(64);
        r->slots_.push_back(std::move(slot));
    }
    r->takes_.reserve(64);
    r->thread_ = std::thread([p = r.get()] { p->writer_loop(); });
    return r;
}

Recorder::~Recorder() {
    if (thread_.joinable()) {
        std::vector<RecordedTakeInfo> ignored;
        finish(ignored);
    }
}

// ---- RT side

void Recorder::capture(const float* const* in, uint32_t in_count, const float* zeros, int64_t position,
                       int64_t latency, uint32_t frames) noexcept {
    if (frames == 0) return;
    const bool contiguous = rt_started_ && position == rt_next_pos_ && latency == rt_latency_;
    for (auto& sp : slots_) {
        Slot& s = *sp;
        const uint32_t ch = s.spec.channels;
        const float* src[2];
        for (uint32_t c = 0; c < ch; ++c) {
            const uint32_t idx = s.spec.input_channel + c;
            src[c] = idx < in_count ? in[idx] : zeros;
        }
        const bool new_take = !contiguous || !s.rt_open;
        if (s.ring->free_space() < static_cast<std::size_t>(frames) * ch) {
            // Overrun: drop this block for this track. The take ends here;
            // the next block that fits starts a new, still aligned take.
            s.rt_open = false;
            overruns_.fetch_add(1, std::memory_order_relaxed);
            dropped_frames_.fetch_add(frames, std::memory_order_relaxed);
            continue;
        }
        if (new_take) {
            if (!s.markers.push(Marker{position - latency, s.rt_frames})) {
                s.rt_open = false; // writer far behind on markers: same as an overrun
                overruns_.fetch_add(1, std::memory_order_relaxed);
                dropped_frames_.fetch_add(frames, std::memory_order_relaxed);
                continue;
            }
            takes_started_.fetch_add(1, std::memory_order_relaxed);
            s.rt_open = true;
        }
        s.ring->write_interleaved(src, ch, frames);
        s.rt_frames += frames;
    }
    rt_started_ = true;
    rt_next_pos_ = position + frames;
    rt_latency_ = latency;
}

void Recorder::on_loop_wrap(int64_t loop_start) noexcept { rt_next_pos_ = loop_start; }

// ---- writer thread

void Recorder::writer_loop() {
    for (;;) {
        bool stopping;
        {
            std::unique_lock lock(wake_mutex_);
            wake_.wait_for(lock, std::chrono::milliseconds(5), [this] { return stop_; });
            stopping = stop_;
        }
        if (stopping || !stalled_.load(std::memory_order_acquire))
            for (auto& s : slots_) drain(*s, stopping);
        if (stopping) break;
    }
    for (auto& s : slots_) close_take(*s);
}

void Recorder::drain(Slot& s, bool /*final_pass*/) {
    const uint32_t ch = s.spec.channels;
    // Acquire the ring head first: every marker the RT side pushed before
    // writing those frames is then visible in the marker queue.
    const uint64_t head_frames = s.ring->head() / ch;
    Marker m;
    while (s.markers.pop(m)) s.pending.push_back(m);
    std::size_t next = 0;
    while (s.read_frames < head_frames) {
        if (next < s.pending.size() && s.pending[next].ring_frame == s.read_frames) {
            close_take(s);
            open_take(s, s.pending[next]);
            ++next;
        }
        uint64_t end = head_frames;
        if (next < s.pending.size() && s.pending[next].ring_frame < end) end = s.pending[next].ring_frame;
        while (s.read_frames < end) {
            std::size_t n = 0;
            const float* p = s.ring->span(s.read_frames * ch, static_cast<std::size_t>(end - s.read_frames) * ch, n);
            append(s, p, n / ch);
            s.read_frames += n / ch;
            s.ring->release_to(s.read_frames * ch);
        }
    }
    s.pending.erase(s.pending.begin(), s.pending.begin() + static_cast<std::ptrdiff_t>(next));
}

void Recorder::open_take(Slot& s, const Marker& m) {
    Take t;
    t.info.track_id = s.spec.track_id;
    t.info.start_frame = m.start_frame;
    t.info.channels = s.spec.channels;
    t.info.sample_rate = sample_rate_;
    ++s.take_index;
    namespace fs = std::filesystem;
    const std::string base =
        name_prefix_ + "-t" + std::to_string(s.spec.track_id) + "-" + std::to_string(s.take_index);
    const fs::path dir = os::path_from_utf8(dir_);
    fs::path path = dir / (base + ".wav");
    std::error_code ec;
    for (int k = 2; fs::exists(path, ec) && k < 1000; ++k) path = dir / (base + "_" + std::to_string(k) + ".wav");
    t.info.path = os::path_to_utf8(path);
    auto wav = std::make_unique<WavFloatWriter>();
    if (const uint64_t cap = max_take_bytes_.load(std::memory_order_relaxed); cap != 0) wav->set_max_data_bytes(cap);
    std::string err;
    if (wav->open(t.info.path.c_str(), s.spec.channels, static_cast<uint32_t>(sample_rate_), err) != SE_OK) {
        t.failed = true;
        write_errors_.fetch_add(1, std::memory_order_relaxed);
    } else {
        s.wav = std::move(wav);
    }
    std::lock_guard lock(info_mutex_);
    s.open_take = static_cast<int>(takes_.size());
    takes_.push_back(std::move(t));
}

void Recorder::close_take(Slot& s) {
    if (s.open_take < 0) return;
    bool failed = false;
    if (s.wav) {
        std::string err;
        failed = s.wav->finish(err) != SE_OK;
        s.wav.reset();
    }
    std::lock_guard lock(info_mutex_);
    Take& t = takes_[static_cast<std::size_t>(s.open_take)];
    if (failed && !t.failed) {
        t.failed = true;
        write_errors_.fetch_add(1, std::memory_order_relaxed);
    }
    if (t.cur_fill > 0) {
        t.peaks.push_back(t.cur_min);
        t.peaks.push_back(t.cur_max);
        t.cur_fill = 0;
    }
    s.open_take = -1;
}

void Recorder::append(Slot& s, const float* data, std::size_t frames) {
    if (s.open_take < 0 || frames == 0) return; // data before the first marker cannot happen; be safe
    const uint32_t ch = s.spec.channels;
    bool failed = false, kept = false;
    if (s.wav) {
        std::string err;
        const uint64_t before = s.wav->frames_written();
        if (s.wav->write(data, frames, err) != SE_OK) {
            // Size limit or I/O error: finalize the header so the frames
            // written so far survive; the take ends there (truncated).
            failed = true;
            frames = static_cast<std::size_t>(s.wav->frames_written() - before);
            kept = s.wav->finish(err) == SE_OK;
            s.wav.reset();
        }
    }
    std::lock_guard lock(info_mutex_);
    Take& t = takes_[static_cast<std::size_t>(s.open_take)];
    if (t.truncated) return; // nothing more reaches its file
    if (failed) {
        write_errors_.fetch_add(1, std::memory_order_relaxed);
        if (kept)
            t.truncated = true;
        else
            t.failed = true;
    }
    t.info.frames += frames;
    for (std::size_t i = 0; i < frames; ++i) {
        const float v = ch == 2 ? 0.5f * (data[2 * i] + data[2 * i + 1]) : data[i];
        if (t.cur_fill == 0) {
            t.cur_min = t.cur_max = v;
        } else {
            t.cur_min = std::min(t.cur_min, v);
            t.cur_max = std::max(t.cur_max, v);
        }
        if (++t.cur_fill == kPeakFrames) {
            t.peaks.push_back(t.cur_min);
            t.peaks.push_back(t.cur_max);
            t.cur_fill = 0;
        }
    }
}

void Recorder::finish(std::vector<RecordedTakeInfo>& out) {
    {
        std::lock_guard lock(wake_mutex_);
        stop_ = true;
    }
    wake_.notify_all();
    if (thread_.joinable()) thread_.join();
    out.clear();
    std::lock_guard lock(info_mutex_);
    for (const Take& t : takes_)
        if (!t.failed) out.push_back(t.info);
}

// ---- readers

void Recorder::live_takes(std::vector<LiveTakeInfo>& out) const {
    out.clear();
    std::lock_guard lock(info_mutex_);
    for (const Take& t : takes_) out.push_back({t.info.track_id, t.info.start_frame, t.info.frames});
}

std::size_t Recorder::peaks(uint64_t track_id, uint32_t frames_per_peak, float* out, std::size_t capacity) const {
    std::lock_guard lock(info_mutex_);
    const Take* take = nullptr;
    for (const Take& t : takes_)
        if (t.info.track_id == track_id) take = &t;
    if (take == nullptr || frames_per_peak == 0) return 0;
    const std::size_t per = std::max<std::size_t>(1, (frames_per_peak + kPeakFrames / 2) / kPeakFrames);
    const std::size_t buckets = take->peaks.size() / 2 + (take->cur_fill > 0 ? 1 : 0);
    std::size_t n = 0;
    for (std::size_t b = 0; b < buckets && n < capacity; b += per, ++n) {
        float lo = std::numeric_limits<float>::infinity(), hi = -std::numeric_limits<float>::infinity();
        for (std::size_t k = b; k < std::min(buckets, b + per); ++k) {
            const bool partial = 2 * k >= take->peaks.size();
            lo = std::min(lo, partial ? take->cur_min : take->peaks[2 * k]);
            hi = std::max(hi, partial ? take->cur_max : take->peaks[2 * k + 1]);
        }
        out[2 * n] = lo;
        out[2 * n + 1] = hi;
    }
    return n;
}

RecordStats Recorder::stats() const {
    RecordStats s;
    s.overruns = overruns_.load(std::memory_order_relaxed);
    s.dropped_frames = dropped_frames_.load(std::memory_order_relaxed);
    s.write_errors = write_errors_.load(std::memory_order_relaxed);
    s.takes = takes_started_.load(std::memory_order_relaxed);
    return s;
}

} // namespace dstudio
