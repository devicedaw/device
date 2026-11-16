// Recording: RT capture into preallocated per-track SPSC rings, drained by a
// non-RT writer thread into 32-bit float WAV files. See engine/README.md
// ("Input, monitoring and recording") for the threading argument.
//
//   audio thread (RT owner)               writer thread (non-RT)
//   ----------------------                ----------------------
//   capture(): per armed track            every ~5 ms (and once more at stop):
//     new take?  -> marker queue (SPSC)     pop markers, read ring up to head,
//     copy input -> float ring (SPSC)       open/finish WAV files at markers,
//     overrun    -> drop block, count       update live takes + peaks (mutex)
//
// The RT side never allocates, locks, logs or touches the disk. A ring that
// is too full for a block drops that block for that track (counted) and the
// take ends; the next block that fits starts a new take at its own timeline
// position, so every take stays sample-aligned.
#pragma once

#include "spsc_queue.hpp"
#include "wav.hpp"

#include <atomic>
#include <condition_variable>
#include <cstdint>
#include <memory>
#include <mutex>
#include <string>
#include <thread>
#include <vector>

namespace dstudio {

struct RecordTrackSpec {
    uint64_t track_id = 0;
    uint32_t input_channel = 0;
    uint32_t channels = 1; // 1 = mono input, 2 = stereo pair
};

struct RecordedTakeInfo {
    uint64_t track_id = 0;
    std::string path;
    int64_t start_frame = 0;
    uint64_t frames = 0;
    uint32_t channels = 1;
    double sample_rate = 0.0;
};

struct LiveTakeInfo {
    uint64_t track_id = 0;
    int64_t start_frame = 0;
    uint64_t frames = 0;
};

struct RecordStats {
    uint64_t overruns = 0;
    uint64_t dropped_frames = 0;
    uint64_t write_errors = 0;
    uint64_t takes = 0;
};

// Single-producer single-consumer float ring (capacity: power of two).
class FloatRing {
public:
    explicit FloatRing(std::size_t capacity_pow2);
    std::size_t capacity() const noexcept { return mask_ + 1; }
    // Producer: free space in floats.
    std::size_t free_space() const noexcept {
        return capacity() - static_cast<std::size_t>(head_.load(std::memory_order_relaxed) -
                                                     tail_.load(std::memory_order_acquire));
    }
    // Producer: interleave `frames` frames of `channels` planar sources and
    // publish them. Caller checked free_space().
    void write_interleaved(const float* const* src, uint32_t channels, uint32_t frames) noexcept;
    // Consumer.
    uint64_t head() const noexcept { return head_.load(std::memory_order_acquire); }
    uint64_t tail() const noexcept { return tail_.load(std::memory_order_relaxed); }
    // Contiguous span starting at absolute float index `pos` (< head), up to `max` floats.
    const float* span(uint64_t pos, std::size_t max, std::size_t& n) const noexcept;
    void release_to(uint64_t pos) noexcept { tail_.store(pos, std::memory_order_release); }

private:
    std::unique_ptr<float[]> buf_;
    std::size_t mask_;
    alignas(64) std::atomic<uint64_t> head_{0};
    alignas(64) std::atomic<uint64_t> tail_{0};
};

class Recorder {
public:
    // Allocates every ring and starts the writer thread. `ring_frames` is
    // rounded up to a power of two. Non-RT.
    static std::unique_ptr<Recorder> create(const std::string& dir, std::vector<RecordTrackSpec> tracks,
                                            double sample_rate, std::size_t ring_frames, std::string& err);
    ~Recorder();
    Recorder(const Recorder&) = delete;
    Recorder& operator=(const Recorder&) = delete;

    // ---- RT side (exactly one RT owner at a time)
    // Captures `frames` frames that play at timeline `position`. `in` holds
    // `in_count` input channels for this chunk; missing channels read from
    // `zeros`. `latency` (frames) is subtracted from new takes' start; a
    // different latency than the previous call (delay compensation L
    // changed) ends the takes like a seek, so every take stays aligned.
    void capture(const float* const* in, uint32_t in_count, const float* zeros, int64_t position,
                 int64_t latency, uint32_t frames) noexcept;
    // The transport wrapped from the loop end to `loop_start` right after
    // the last captured chunk: the takes continue linearly.
    void on_loop_wrap(int64_t loop_start) noexcept;

    // ---- non-RT side
    // Stops the writer after draining everything, finalizes the files.
    // The RT side must no longer call capture().
    void finish(std::vector<RecordedTakeInfo>& takes);
    void live_takes(std::vector<LiveTakeInfo>& out) const;
    // Min/max pairs of the newest take of `track_id`.
    std::size_t peaks(uint64_t track_id, uint32_t frames_per_peak, float* out_minmax, std::size_t capacity) const;
    RecordStats stats() const;
    // Test hook: while stalled the writer does not drain (stop still drains).
    void set_writer_stalled(bool stalled) noexcept { stalled_.store(stalled, std::memory_order_release); }
    // Test hook: caps each take's WAV data chunk (0 = the 4 GiB RIFF limit).
    // Applies to takes opened afterwards.
    void set_max_take_bytes(uint64_t bytes) noexcept { max_take_bytes_.store(bytes, std::memory_order_relaxed); }

    static constexpr uint32_t kPeakFrames = 64;

private:
    struct Marker {
        int64_t start_frame = 0;
        uint64_t ring_frame = 0; // index (in frames) of the take's first frame in the ring stream
    };
    struct Take {
        RecordedTakeInfo info;
        std::vector<float> peaks; // min,max per kPeakFrames
        float cur_min = 0.0f, cur_max = 0.0f;
        uint32_t cur_fill = 0;
        bool failed = false;
        bool truncated = false; // write failed; the file holds info.frames frames
    };
    struct Slot {
        RecordTrackSpec spec;
        std::unique_ptr<FloatRing> ring;
        SpscQueue<Marker, 256> markers;
        // RT-owned
        bool rt_open = false;
        uint64_t rt_frames = 0; // frames pushed into the ring so far
        // writer-owned
        std::vector<Marker> pending;
        uint64_t read_frames = 0;
        std::unique_ptr<WavFloatWriter> wav;
        int take_index = 0;
        int open_take = -1; // index into takes_
    };

    Recorder(std::string dir, double sample_rate);
    void writer_loop();
    void drain(Slot& s, bool final_pass);
    void open_take(Slot& s, const Marker& m);
    void close_take(Slot& s);
    void append(Slot& s, const float* data, std::size_t frames);

    const std::string dir_;
    const double sample_rate_;
    std::string name_prefix_;
    std::atomic<uint64_t> max_take_bytes_{0};
    std::vector<std::unique_ptr<Slot>> slots_;

    // RT continuity tracking
    bool rt_started_ = false;
    int64_t rt_next_pos_ = 0;
    int64_t rt_latency_ = 0; // compensation of the open takes (a change starts new takes)

    std::atomic<uint64_t> overruns_{0};
    std::atomic<uint64_t> dropped_frames_{0};
    std::atomic<uint64_t> write_errors_{0};
    std::atomic<uint64_t> takes_started_{0};

    // writer
    std::thread thread_;
    std::mutex wake_mutex_;
    std::condition_variable wake_;
    bool stop_ = false; // guarded by wake_mutex_
    std::atomic<bool> stalled_{false};

    mutable std::mutex info_mutex_; // guards takes_ (writer writes, readers copy)
    std::vector<Take> takes_;
};

} // namespace dstudio
