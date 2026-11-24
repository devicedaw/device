// Voice input tap (spec 024): while the assistant listens, the audio thread
// copies one input channel (or the average of a stereo pair) into a mono ring
// that the app drains between frames for speech recognition.
//
//   audio thread: write(inputs, count, n)  --> ring (lock-free, no allocation)
//   app thread:   read(out, capacity)      <-- ring (one reader)
//
// The ring is allocated once with the engine. Samples that do not fit while the
// app is slow are dropped and counted; the app reads every frame, so at 48 kHz
// the 2^19-sample ring (about 11 s) never fills in practice.
#pragma once

#include <algorithm>
#include <atomic>
#include <cmath>
#include <cstddef>
#include <cstdint>
#include <memory>

namespace dstudio {

class VoiceTap {
public:
    static constexpr std::size_t kCapacity = std::size_t{1} << 19;

    VoiceTap() : ring_(new float[kCapacity]()) {}

    // Which input to capture: channel < 0 turns the tap off. Turning it on
    // empties the ring and the peak. MAIN THREAD.
    void set(int32_t channel, bool stereo) noexcept {
        source_.store(-1, std::memory_order_seq_cst);
        // The audio thread may still be inside write() with the old source;
        // emptying from the reader side is safe: the reader owns tail_.
        tail_.store(head_.load(std::memory_order_acquire), std::memory_order_release);
        peak_.store(0.0f, std::memory_order_relaxed);
        dropped_.store(0, std::memory_order_relaxed);
        if (channel >= 0) source_.store(channel * 2 + (stereo ? 1 : 0), std::memory_order_seq_cst);
    }

    bool active() const noexcept { return source_.load(std::memory_order_relaxed) >= 0; }

    // AUDIO THREAD. `in` holds `count` channel pointers of `n` frames. A
    // channel the device does not have reads as silence.
    void write(const float* const* in, uint32_t count, uint32_t n) noexcept {
        const int32_t src = source_.load(std::memory_order_relaxed);
        if (src < 0 || n == 0) return;
        const uint32_t ch = static_cast<uint32_t>(src >> 1);
        const bool stereo = (src & 1) != 0;
        const float* a = ch < count ? in[ch] : nullptr;
        const float* b = stereo && ch + 1 < count ? in[ch + 1] : nullptr;
        const std::size_t head = head_.load(std::memory_order_relaxed);
        const std::size_t tail = tail_.load(std::memory_order_acquire);
        const std::size_t room = kCapacity - (head - tail);
        const std::size_t take = std::min<std::size_t>(n, room);
        float peak = peak_.load(std::memory_order_relaxed);
        for (std::size_t i = 0; i < take; ++i) {
            float v = 0.0f;
            if (a != nullptr) v = b != nullptr ? 0.5f * (a[i] + b[i]) : a[i];
            ring_[(head + i) & (kCapacity - 1)] = v;
            peak = std::max(peak, std::fabs(v));
        }
        peak_.store(peak, std::memory_order_relaxed);
        head_.store(head + take, std::memory_order_release);
        if (take < n) dropped_.fetch_add(n - take, std::memory_order_relaxed);
    }

    // Pops up to `capacity` samples in order. ONE READER (the app).
    std::size_t read(float* out, std::size_t capacity) noexcept {
        const std::size_t tail = tail_.load(std::memory_order_relaxed);
        const std::size_t head = head_.load(std::memory_order_acquire);
        const std::size_t take = std::min(capacity, head - tail);
        for (std::size_t i = 0; i < take; ++i) out[i] = ring_[(tail + i) & (kCapacity - 1)];
        tail_.store(tail + take, std::memory_order_release);
        return take;
    }

    // The largest absolute sample written since the last call; resets it.
    float take_peak() noexcept { return peak_.exchange(0.0f, std::memory_order_relaxed); }
    uint64_t dropped() const noexcept { return dropped_.load(std::memory_order_relaxed); }

private:
    std::unique_ptr<float[]> ring_;
    std::atomic<int32_t> source_{-1}; // channel * 2 + stereo, or -1 = off
    std::atomic<float> peak_{0.0f};
    std::atomic<uint64_t> dropped_{0};
    alignas(64) std::atomic<std::size_t> head_{0}; // written by the audio thread
    alignas(64) std::atomic<std::size_t> tail_{0}; // written by the reader
};

} // namespace dstudio
