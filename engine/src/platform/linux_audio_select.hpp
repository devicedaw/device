// Linux audio: which backend serves a device configuration, and how ALSA
// device ids map to ALSA PCM names. Pure functions (no ALSA or PipeWire
// calls), so they build and are tested on every platform.
//
// Two backends (linux_audio_device.cpp dispatches between them):
//   - PipeWire (pipewire_device.cpp): the desktop sound server. Ids are
//     PipeWire node names ("alsa_output.pci-0000_00_1f.3.analog-stereo").
//   - ALSA (alsa_device.cpp): the kernel sound API, always there. Ids start
//     with "alsa:": "alsa:default" (the ALSA default PCM), "alsa:hw:CARD=PCH,DEV=0"
//     (one hardware PCM, opened directly), or "alsa:<any PCM name>" for
//     devices defined in an asoundrc.
// The system default (empty output id) is PipeWire when its server is
// reachable and ALSA otherwise. DSTUDIO_AUDIO_BACKEND=alsa (or pipewire)
// forces one for the system default.
#pragma once

#include <string>
#include <string_view>

namespace dstudio::linux_audio {

enum class Backend { PipeWire, Alsa };

// What DSTUDIO_AUDIO_BACKEND asks for (anything else is Auto).
enum class Preference { Auto, PipeWire, Alsa };

inline constexpr std::string_view kAlsaPrefix = "alsa:";
inline constexpr std::string_view kAlsaDefaultUid = "alsa:default";

// True for ids of the ALSA backend ("alsa:...").
inline bool is_alsa_uid(std::string_view uid) noexcept { return uid.substr(0, kAlsaPrefix.size()) == kAlsaPrefix; }

// Parses DSTUDIO_AUDIO_BACKEND ("alsa", "pipewire"; case-insensitive).
inline Preference parse_preference(std::string_view v) noexcept {
    auto eq = [&](std::string_view want) {
        if (v.size() != want.size()) return false;
        for (std::size_t i = 0; i < v.size(); ++i) {
            char c = v[i];
            if (c >= 'A' && c <= 'Z') c = static_cast<char>(c - 'A' + 'a');
            if (c != want[i]) return false;
        }
        return true;
    };
    if (eq("alsa")) return Preference::Alsa;
    if (eq("pipewire")) return Preference::PipeWire;
    return Preference::Auto;
}

// The ALSA PCM name behind an ALSA id: "alsa:hw:CARD=PCH,DEV=0" ->
// "hw:CARD=PCH,DEV=0", "alsa:default" -> "default". "" (the backend's own
// default) -> "default".
inline std::string alsa_pcm_name(std::string_view uid) {
    if (uid.empty()) return "default";
    if (is_alsa_uid(uid)) uid.remove_prefix(kAlsaPrefix.size());
    return uid.empty() ? std::string("default") : std::string(uid);
}

// True for a direct hardware PCM ("hw:..."), which alsa_device.cpp opens
// without ALSA's conversion plugins when the format allows it.
inline bool is_hw_pcm(std::string_view pcm) noexcept { return pcm.substr(0, 3) == "hw:"; }

// The conversion-plugin variant of a PCM, used when the device cannot run
// the engine's rate, sample format or channel count directly: "hw:X" ->
// "plughw:X", "dmix" -> "plug:dmix". "" for names that already convert
// ("default", "plug...").
inline std::string plug_pcm_name(std::string_view pcm) {
    if (is_hw_pcm(pcm)) return "plug" + std::string(pcm);
    if (pcm == "default" || pcm.substr(0, 4) == "plug") return {};
    return "plug:" + std::string(pcm);
}

// The id for one hardware PCM: card id (snd_ctl_card_info_get_id, stable
// across reboots, unlike the card number) and device number.
inline std::string alsa_hw_uid(std::string_view card_id, int device) {
    return std::string(kAlsaPrefix) + "hw:CARD=" + std::string(card_id) + ",DEV=" + std::to_string(device);
}

// Display name of a hardware PCM: "HDA Intel PCH: ALC892 Analog (ALSA)".
inline std::string alsa_hw_name(std::string_view card_name, std::string_view pcm_name) {
    std::string n(card_name);
    if (!pcm_name.empty() && pcm_name != card_name) n += ": " + std::string(pcm_name);
    return n + " (ALSA)";
}

// Which backend runs a configuration, or why none can.
struct Choice {
    Backend backend = Backend::PipeWire;
    std::string error; // non-empty: the configuration mixes backends
};

// Picks the backend: a non-empty output id decides by its form; the system
// default output follows an ALSA input id, then the preference, then
// whether a PipeWire server is reachable. Input and output must use one
// backend (they run from one clock and one callback).
inline Choice choose_backend(std::string_view output_uid, bool input_enabled, std::string_view input_uid,
                             bool pipewire_up, Preference pref) {
    Choice c;
    const bool in_alsa = input_enabled && is_alsa_uid(input_uid);
    const bool in_pw = input_enabled && !input_uid.empty() && !in_alsa;
    if (!output_uid.empty()) {
        c.backend = is_alsa_uid(output_uid) ? Backend::Alsa : Backend::PipeWire;
    } else if (in_alsa) {
        c.backend = Backend::Alsa;
    } else if (in_pw) {
        c.backend = Backend::PipeWire;
    } else if (pref == Preference::Alsa) {
        c.backend = Backend::Alsa;
    } else if (pref == Preference::PipeWire) {
        c.backend = Backend::PipeWire;
    } else {
        c.backend = pipewire_up ? Backend::PipeWire : Backend::Alsa;
    }
    if ((c.backend == Backend::Alsa && in_pw) || (c.backend == Backend::PipeWire && in_alsa))
        c.error = c.backend == Backend::Alsa
                      ? "the input is a PipeWire device but the output is an ALSA device: pick both from the same audio system"
                      : "the input is an ALSA device but the output is a PipeWire device: pick both from the same audio system";
    return c;
}

} // namespace dstudio::linux_audio
