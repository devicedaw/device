#include "wav.hpp"

#include "platform/os.hpp"
#include "dstudio_engine.h"

#include <algorithm>
#include <bit>
#include <cstring>
#include <filesystem>
#include <limits>
#include <utility>

namespace dstudio {
namespace {

constexpr uint16_t kFormatPcm = 0x0001;
constexpr uint16_t kFormatFloat = 0x0003;
constexpr uint16_t kFormatExtensible = 0xFFFE;

// KSDATAFORMAT_SUBTYPE_* GUIDs are {XXXXXXXX-0000-0010-8000-00AA00389B71};
// bytes 2..15 (after the 16-bit format tag) must match this suffix.
constexpr uint8_t kGuidSuffix[14] = {0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80,
                                     0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71};

uint16_t rd16(const uint8_t* p) noexcept { return static_cast<uint16_t>(p[0] | (p[1] << 8)); }
uint32_t rd32(const uint8_t* p) noexcept {
    return static_cast<uint32_t>(p[0]) | (static_cast<uint32_t>(p[1]) << 8) | (static_cast<uint32_t>(p[2]) << 16) |
           (static_cast<uint32_t>(p[3]) << 24);
}
uint64_t rd64(const uint8_t* p) noexcept {
    return static_cast<uint64_t>(rd32(p)) | (static_cast<uint64_t>(rd32(p + 4)) << 32);
}

void wr16(uint8_t* p, uint16_t v) noexcept {
    p[0] = static_cast<uint8_t>(v);
    p[1] = static_cast<uint8_t>(v >> 8);
}
void wr32(uint8_t* p, uint32_t v) noexcept {
    for (int i = 0; i < 4; ++i) p[i] = static_cast<uint8_t>(v >> (8 * i));
}

struct FmtInfo {
    uint16_t format = 0; // resolved (extensible unwrapped)
    uint16_t channels = 0;
    uint32_t sample_rate = 0;
    uint16_t block_align = 0;
    uint16_t bits = 0; // container bits per sample
};

template <class Conv>
void deinterleave(const uint8_t* src, uint64_t frames, uint32_t channels, uint32_t bytes_per_sample,
                  DecodedAudio& out, Conv conv) {
    for (uint32_t c = 0; c < channels; ++c) out.planar[c].resize(frames);
    const uint32_t stride = bytes_per_sample * channels;
    for (uint64_t f = 0; f < frames; ++f) {
        const uint8_t* frame = src + f * stride;
        for (uint32_t c = 0; c < channels; ++c) out.planar[c][f] = conv(frame + c * bytes_per_sample);
    }
}

} // namespace

int32_t decode_wav_memory(const uint8_t* data, std::size_t size, DecodedAudio& out, std::string& err) {
    out = DecodedAudio{};
    if (data == nullptr || size < 12) {
        err = "file too small to be a WAV file";
        return SE_ERR_UNSUPPORTED_FORMAT;
    }
    const bool wide = std::memcmp(data, "RF64", 4) == 0 || std::memcmp(data, "BW64", 4) == 0;
    if ((!wide && std::memcmp(data, "RIFF", 4) != 0) || std::memcmp(data + 8, "WAVE", 4) != 0) {
        err = "not a RIFF/WAVE file";
        return SE_ERR_UNSUPPORTED_FORMAT;
    }

    // RF64 (EBU Tech 3306) and BW64 (ITU-R BS.2088) carry 64-bit sizes in a ds64 chunk that must
    // come first: RIFF size, data size, sample count, then a table of other oversized chunks.
    // A 32-bit chunk size of 0xFFFFFFFF means "look it up in ds64".
    uint64_t ds64_data = 0;
    std::vector<std::pair<uint32_t, uint64_t>> ds64_table;
    if (wide) {
        if (size < 12 + 8 + 28 || std::memcmp(data + 12, "ds64", 4) != 0 || rd32(data + 16) < 28) {
            err = "RF64/BW64 file without a ds64 chunk";
            return SE_ERR_UNSUPPORTED_FORMAT;
        }
        const uint8_t* d = data + 20;
        const uint32_t ds_size = rd32(data + 16);
        ds64_data = rd64(d + 8);
        const uint32_t entries = rd32(d + 24);
        for (uint32_t i = 0; i < entries && 28 + (i + 1) * 12ull <= ds_size && 20 + 28 + (i + 1) * 12ull <= size; ++i) {
            const uint8_t* e = d + 28 + i * 12;
            ds64_table.emplace_back(rd32(e), rd64(e + 4));
        }
    }

    FmtInfo fmt;
    bool have_fmt = false;
    const uint8_t* pcm = nullptr;
    uint64_t pcm_bytes = 0;

    std::size_t pos = 12;
    while (pos + 8 <= size) {
        const uint8_t* ck = data + pos;
        uint64_t ck_size = rd32(ck + 4);
        if (wide && ck_size == 0xFFFFFFFFu) {
            if (std::memcmp(ck, "data", 4) == 0) {
                ck_size = ds64_data;
            } else {
                const auto it = std::find_if(ds64_table.begin(), ds64_table.end(),
                                             [&](const auto& e) { return e.first == rd32(ck); });
                if (it == ds64_table.end()) break;
                ck_size = it->second;
            }
        }
        const std::size_t body = pos + 8;
        const std::size_t avail = size - body;

        if (std::memcmp(ck, "fmt ", 4) == 0) {
            if (ck_size < 16 || avail < 16) {
                err = "truncated fmt chunk";
                return SE_ERR_UNSUPPORTED_FORMAT;
            }
            const uint8_t* f = data + body;
            fmt.format = rd16(f);
            fmt.channels = rd16(f + 2);
            fmt.sample_rate = rd32(f + 4);
            fmt.block_align = rd16(f + 12);
            fmt.bits = rd16(f + 14);
            if (fmt.format == kFormatExtensible) {
                if (ck_size < 40 || avail < 40) {
                    err = "truncated WAVE_FORMAT_EXTENSIBLE fmt chunk";
                    return SE_ERR_UNSUPPORTED_FORMAT;
                }
                const uint8_t* guid = f + 24;
                if (std::memcmp(guid + 2, kGuidSuffix, sizeof kGuidSuffix) != 0) {
                    err = "unknown WAVE_FORMAT_EXTENSIBLE sub-format GUID";
                    return SE_ERR_UNSUPPORTED_FORMAT;
                }
                fmt.format = rd16(guid);
            }
            have_fmt = true;
        } else if (std::memcmp(ck, "data", 4) == 0) {
            pcm = data + body;
            // Streamed/truncated files may carry a bogus size; clamp to the file.
            pcm_bytes = ck_size > avail ? avail : ck_size;
            if (have_fmt) break; // fmt must precede data in practice; stop early
        }

        // Chunks are word aligned (odd sizes carry a pad byte).
        const uint64_t next = static_cast<uint64_t>(body) + ck_size + (ck_size & 1u);
        if (next < body) break;
        if (next > size) break;
        pos = static_cast<std::size_t>(next);
    }

    if (!have_fmt) {
        err = "missing fmt chunk";
        return SE_ERR_UNSUPPORTED_FORMAT;
    }
    if (pcm == nullptr) {
        err = "missing data chunk";
        return SE_ERR_UNSUPPORTED_FORMAT;
    }
    if (fmt.channels == 0) {
        err = "fmt chunk declares zero channels";
        return SE_ERR_UNSUPPORTED_FORMAT;
    }
    if (fmt.channels > 2) {
        err = "only mono and stereo WAV files are supported (got " + std::to_string(fmt.channels) + " channels)";
        return SE_ERR_UNSUPPORTED_FORMAT;
    }
    if (fmt.sample_rate == 0) {
        err = "fmt chunk declares a zero sample rate";
        return SE_ERR_UNSUPPORTED_FORMAT;
    }
    if (fmt.bits == 0 || fmt.bits % 8 != 0) {
        err = "unsupported bits per sample: " + std::to_string(fmt.bits);
        return SE_ERR_UNSUPPORTED_FORMAT;
    }
    const uint32_t bytes_per_sample = fmt.bits / 8u;
    if (fmt.block_align != bytes_per_sample * fmt.channels) {
        err = "inconsistent block alignment in fmt chunk";
        return SE_ERR_UNSUPPORTED_FORMAT;
    }

    const uint64_t frames = pcm_bytes / fmt.block_align;
    out.channels = fmt.channels;
    out.sample_rate = fmt.sample_rate;
    out.frames = frames;

    if (fmt.format == kFormatPcm) {
        switch (fmt.bits) {
        case 8:
            deinterleave(pcm, frames, fmt.channels, 1, out,
                         [](const uint8_t* p) { return (static_cast<float>(p[0]) - 128.0f) * (1.0f / 128.0f); });
            break;
        case 16:
            deinterleave(pcm, frames, fmt.channels, 2, out, [](const uint8_t* p) {
                return static_cast<float>(static_cast<int16_t>(rd16(p))) * (1.0f / 32768.0f);
            });
            break;
        case 24:
            deinterleave(pcm, frames, fmt.channels, 3, out, [](const uint8_t* p) {
                const uint32_t u = static_cast<uint32_t>(p[0]) << 8 | static_cast<uint32_t>(p[1]) << 16 |
                                   static_cast<uint32_t>(p[2]) << 24;
                return static_cast<float>(static_cast<int32_t>(u) >> 8) * (1.0f / 8388608.0f);
            });
            break;
        case 32:
            deinterleave(pcm, frames, fmt.channels, 4, out, [](const uint8_t* p) {
                return static_cast<float>(static_cast<double>(static_cast<int32_t>(rd32(p))) * (1.0 / 2147483648.0));
            });
            break;
        default:
            err = "unsupported PCM bit depth: " + std::to_string(fmt.bits);
            out = DecodedAudio{};
            return SE_ERR_UNSUPPORTED_FORMAT;
        }
    } else if (fmt.format == kFormatFloat) {
        if (fmt.bits == 32) {
            deinterleave(pcm, frames, fmt.channels, 4, out,
                         [](const uint8_t* p) { return std::bit_cast<float>(rd32(p)); });
        } else if (fmt.bits == 64) {
            deinterleave(pcm, frames, fmt.channels, 8, out,
                         [](const uint8_t* p) { return static_cast<float>(std::bit_cast<double>(rd64(p))); });
        } else {
            err = "unsupported float bit depth: " + std::to_string(fmt.bits);
            out = DecodedAudio{};
            return SE_ERR_UNSUPPORTED_FORMAT;
        }
    } else {
        err = "unsupported WAV format tag 0x" + [](uint16_t v) {
            char buf[8];
            std::snprintf(buf, sizeof buf, "%04X", v);
            return std::string(buf);
        }(fmt.format);
        out = DecodedAudio{};
        return SE_ERR_UNSUPPORTED_FORMAT;
    }
    return SE_OK;
}

int32_t read_file_bytes(const char* utf8_path, std::vector<uint8_t>& bytes, std::string& err) {
    std::FILE* f = os::fopen_utf8(utf8_path, "rb"); // _wfopen on Windows
    if (f == nullptr) {
        err = std::string("cannot open file: ") + utf8_path;
        return SE_ERR_IO;
    }
    bytes.clear();
    // Anything WavFloatWriter writes (and any plain RIFF/AIFF: 32-bit chunk
    // sizes) fits; the size is read 64-bit (ftell's long is 32-bit on Windows).
    constexpr uint64_t kMaxImportBytes = (uint64_t{1} << 32) + 8;
    std::error_code ec;
    const std::uintmax_t size = std::filesystem::file_size(os::path_from_utf8(utf8_path), ec);
    if (ec) {
        std::fclose(f);
        err = std::string("cannot read file size: ") + utf8_path;
        return SE_ERR_IO;
    }
    if (size > kMaxImportBytes || size > std::numeric_limits<std::size_t>::max()) {
        std::fclose(f);
        err = std::string("file is larger than 4 GB: ") + utf8_path;
        return SE_ERR_IO;
    }
    const auto len = static_cast<std::size_t>(size);
    if (len > 0) bytes.resize(len);
    // In chunks: some C runtimes mishandle single reads past INT_MAX bytes.
    constexpr std::size_t kChunk = std::size_t{64} << 20;
    std::size_t got = 0;
    while (got < bytes.size()) {
        const std::size_t want = std::min(kChunk, bytes.size() - got);
        const std::size_t n = std::fread(bytes.data() + got, 1, want, f);
        got += n;
        if (n != want) break;
    }
    const bool io_error = std::ferror(f) != 0;
    std::fclose(f);
    if (io_error || got != bytes.size()) {
        err = std::string("read error: ") + utf8_path;
        return SE_ERR_IO;
    }
    return SE_OK;
}

int32_t decode_wav_file(const char* utf8_path, DecodedAudio& out, std::string& err) {
    std::vector<uint8_t> bytes;
    const int32_t rc = read_file_bytes(utf8_path, bytes, err);
    return rc != SE_OK ? rc : decode_wav_memory(bytes.data(), bytes.size(), out, err);
}

// ---------------------------------------------------------------- writer

namespace {
// RIFF header (12) + fmt chunk (8 + 18) + fact chunk (8 + 4) + data header (8)
constexpr std::size_t kHeaderSize = 12 + 26 + 12 + 8;

void build_header(uint8_t* h, uint32_t channels, uint32_t sample_rate, uint64_t frames) {
    const uint64_t data_bytes = frames * channels * 4u;
    std::memcpy(h, "RIFF", 4);
    wr32(h + 4, static_cast<uint32_t>(kHeaderSize - 8 + data_bytes));
    std::memcpy(h + 8, "WAVE", 4);
    std::memcpy(h + 12, "fmt ", 4);
    wr32(h + 16, 18);
    wr16(h + 20, kFormatFloat);
    wr16(h + 22, static_cast<uint16_t>(channels));
    wr32(h + 24, sample_rate);
    wr32(h + 28, sample_rate * channels * 4u);
    wr16(h + 32, static_cast<uint16_t>(channels * 4u));
    wr16(h + 34, 32);
    wr16(h + 36, 0); // cbSize
    std::memcpy(h + 38, "fact", 4);
    wr32(h + 42, 4);
    wr32(h + 46, static_cast<uint32_t>(frames));
    std::memcpy(h + 50, "data", 4);
    wr32(h + 54, static_cast<uint32_t>(data_bytes));
}
} // namespace

WavFloatWriter::~WavFloatWriter() {
    if (file_ != nullptr) std::fclose(file_);
}

int32_t WavFloatWriter::open(const char* utf8_path, uint32_t channels, uint32_t sample_rate, std::string& err) {
    if (file_ != nullptr || channels == 0 || channels > 2 || sample_rate == 0) {
        err = "invalid WAV writer parameters";
        return SE_ERR_INVALID_ARG;
    }
    file_ = os::fopen_utf8(utf8_path, "wb"); // _wfopen on Windows
    if (file_ == nullptr) {
        err = std::string("cannot create file: ") + utf8_path;
        return SE_ERR_IO;
    }
    channels_ = channels;
    sample_rate_ = sample_rate;
    frames_written_ = 0;
    uint8_t header[kHeaderSize];
    build_header(header, channels, sample_rate, 0);
    if (std::fwrite(header, 1, kHeaderSize, file_) != kHeaderSize) {
        err = "write error";
        return SE_ERR_IO;
    }
    return SE_OK;
}

int32_t WavFloatWriter::write(const float* interleaved, uint64_t frames, std::string& err) {
    if (file_ == nullptr) {
        err = "WAV writer not open";
        return SE_ERR_STATE;
    }
    const uint64_t max_bytes = std::min<uint64_t>(std::numeric_limits<uint32_t>::max() - kHeaderSize, max_data_bytes_);
    const uint64_t max_frames = max_bytes / (channels_ * 4u);
    if (frames_written_ + frames > max_frames) {
        // Write what still fits, so the file ends exactly at the limit.
        const uint64_t room = max_frames > frames_written_ ? max_frames - frames_written_ : 0;
        if (room > 0 && write(interleaved, room, err) != SE_OK) return SE_ERR_IO;
        err = "WAV output exceeds 4 GiB (RF64 not supported)";
        return SE_ERR_INVALID_ARG;
    }
    const std::size_t samples = static_cast<std::size_t>(frames) * channels_;
    if constexpr (std::endian::native == std::endian::little) {
        if (std::fwrite(interleaved, sizeof(float), samples, file_) != samples) {
            err = "write error";
            return SE_ERR_IO;
        }
    } else {
        for (std::size_t i = 0; i < samples; ++i) {
            uint8_t b[4];
            wr32(b, std::bit_cast<uint32_t>(interleaved[i]));
            if (std::fwrite(b, 1, 4, file_) != 4) {
                err = "write error";
                return SE_ERR_IO;
            }
        }
    }
    frames_written_ += frames;
    return SE_OK;
}

int32_t WavFloatWriter::finish(std::string& err) {
    if (file_ == nullptr) {
        err = "WAV writer not open";
        return SE_ERR_STATE;
    }
    uint8_t header[kHeaderSize];
    build_header(header, channels_, sample_rate_, frames_written_);
    const bool ok = std::fseek(file_, 0, SEEK_SET) == 0 && std::fwrite(header, 1, kHeaderSize, file_) == kHeaderSize;
    const bool close_ok = std::fclose(file_) == 0;
    file_ = nullptr;
    if (!ok || !close_ok) {
        err = "error finalising WAV header";
        return SE_ERR_IO;
    }
    return SE_OK;
}

int32_t write_wav_float32(const char* utf8_path, const float* interleaved, uint64_t frames, uint32_t channels,
                          uint32_t sample_rate, std::string& err) {
    WavFloatWriter w;
    int32_t rc = w.open(utf8_path, channels, sample_rate, err);
    if (rc == SE_OK && frames > 0) rc = w.write(interleaved, frames, err);
    if (rc == SE_OK) rc = w.finish(err);
    return rc;
}

} // namespace dstudio
