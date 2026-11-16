// Test-only entry points (not part of the public ABI). Used by the C++ test
// harness, which links the static library directly.
#pragma once

#include "platform/audio_device.hpp"
#include "platform/midi_backend.hpp"
#include "dstudio_engine.h"

#include <cstddef>
#include <cstdint>
#include <functional>
#include <memory>

namespace dstudio::testing {

// Replace the platform device (fake devices drive Engine's device path
// without hardware). Call before the first start/configure.
void set_device_factory(se_engine* e, std::function<std::unique_ptr<AudioDevice>()> factory);
// While stalled, the recording writer thread does not drain the rings
// (stop still drains and finalizes). Applies to the current recording.
void set_record_writer_stalled(se_engine* e, bool stalled);
// Ring size (frames per track) for the next se_record_start; 0 = default (2 s).
void set_record_ring_frames(se_engine* e, std::size_t frames);
// Caps each take's WAV data chunk, in bytes, for the next se_record_start
// (0 = the 4 GiB RIFF limit), so tests can hit the limit cheaply.
void set_record_max_take_bytes(se_engine* e, uint64_t bytes);
// Background warp renders on/off (off: warped clips always stretch live).
void set_warp_renders_enabled(se_engine* e, bool enabled);
// Plugin instances created so far (successful or not).
uint64_t plugin_instantiations(se_engine* e);
// Queue a parameter change for a live device (AU: parameter id + plain
// value; VST3: ParamID + normalized value; CLAP: clap_id + plain value).
bool plugin_set_parameter(se_engine* e, uint64_t device_id, uint32_t param_id, double value);
// Perform an editor-style gesture (begin, value, end) through the format's
// own notification path (AU: AUEventListenerNotify + AUParameterSet). False
// where the format has no host-side path (VST3/CLAP: use the test plugins'
// hidden gesture parameters instead).
bool plugin_simulate_gesture(se_engine* e, uint64_t device_id, uint32_t param_id, float normalized);
// Replace the MIDI backend (fake ports). Call before any se_midi_* call.
void set_midi_backend(se_engine* e, std::unique_ptr<MidiBackend> backend);

} // namespace dstudio::testing
