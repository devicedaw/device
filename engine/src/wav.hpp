// Hand-written RIFF/WAVE reader and 32-bit float writer. Non-RT only.
//
// Supported input:
//   - WAVE_FORMAT_PCM (1): 8-bit unsigned, 16/24/32-bit signed integer
//   - WAVE_FORMAT_IEEE_FLOAT (3): 32-bit and 64-bit float
//   - WAVE_FORMAT_EXTENSIBLE (0xFFFE) wrapping either of the above
//     (e.g. 24-bit samples in 32-bit containers)
//   - 1 or 2 channels. Files with more than 2 channels are REJECTED with
//     SE_ERR_UNSUPPORTED_FORMAT (no implicit downmix: a correct downmix needs
//     the channel mask / layout policy, which belongs to a later milestone).
//   - RF64 / BW64 (64-bit sizes from the ds64 chunk), within the importer's 2 GB file cap
// Not supported: compressed formats (ADPCM, mu-law, ...).
#pragma once

#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <string>
#include <vector>

namespace dstudio {

struct DecodedAudio {
    uint32_t channels = 0;
    double sample_rate = 0.0;
    uint64_t frames = 0;
    std::vector<float> planar[2];
};

// Return value is an se_result code; `err` receives a human-readable reason.
int32_t decode_wav_memory(const uint8_t* data, std::size_t size, DecodedAudio& out, std::string& err);
int32_t decode_wav_file(const char* utf8_path, DecodedAudio& out, std::string& err);
int32_t read_file_bytes(const char* utf8_path, std::vector<uint8_t>& bytes, std::string& err);

// Any importable file, by content: WAV (above), AIFF / AIFC, FLAC, Ogg Vorbis, MP3 (decode.cpp).
// Mono and stereo only, like WAV.
int32_t decode_audio_memory(const uint8_t* data, std::size_t size, DecodedAudio& out, std::string& err);
int32_t decode_audio_file(const char* utf8_path, DecodedAudio& out, std::string& err);

// Streaming writer for WAVE_FORMAT_IEEE_FLOAT, 32-bit, interleaved.
class WavFloatWriter {
public:
    WavFloatWriter() = default;
    ~WavFloatWriter();
    WavFloatWriter(const WavFloatWriter&) = delete;
    WavFloatWriter& operator=(const WavFloatWriter&) = delete;

    int32_t open(const char* utf8_path, uint32_t channels, uint32_t sample_rate, std::string& err);
    int32_t write(const float* interleaved, uint64_t frames, std::string& err);
    // Patches the RIFF/data/fact sizes and closes the file.
    int32_t finish(std::string& err);
    // Caps the data chunk below the 4 GiB RIFF limit (tests). A write that
    // would pass the cap writes the frames that fit and fails with
    // SE_ERR_INVALID_ARG.
    void set_max_data_bytes(uint64_t bytes) noexcept { max_data_bytes_ = bytes; }
    uint64_t frames_written() const noexcept { return frames_written_; }

private:
    std::FILE* file_ = nullptr;
    uint32_t channels_ = 0;
    uint32_t sample_rate_ = 0;
    uint64_t frames_written_ = 0;
    uint64_t max_data_bytes_ = UINT64_MAX;
};

// Convenience: write a whole interleaved buffer.
int32_t write_wav_float32(const char* utf8_path, const float* interleaved, uint64_t frames, uint32_t channels,
                          uint32_t sample_rate, std::string& err);

} // namespace dstudio
