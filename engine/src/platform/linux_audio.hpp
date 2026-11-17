// Linux audio backends behind the platform interface (audio_device.hpp).
// linux_audio_device.cpp implements create_platform_audio_device() and the
// enumeration functions on top of these two; linux_audio_select.hpp decides
// which one serves a configuration.
#pragma once

#if defined(__linux__)

#include "audio_device.hpp"

#include <memory>
#include <vector>

namespace dstudio::linux_audio {

// ---- PipeWire (pipewire_device.cpp). libpipewire is loaded with dlopen, so
// all of these work (reporting no server) on systems without it.

// A PipeWire stream device (not started).
std::unique_ptr<AudioDevice> create_pipewire_device();
// Lists the PipeWire sinks and sources and the graph's clock rate. False
// (and an empty list) when libpipewire is missing or no server answers.
bool pipewire_list(std::vector<DeviceInfo>& out, double& clock_rate);
// True when libpipewire loads and a server accepts a connection.
bool pipewire_server_up();

// ---- ALSA (alsa_device.cpp). libasound is loaded with dlopen too; without
// it there are no ALSA devices.

// An ALSA PCM device (not started).
std::unique_ptr<AudioDevice> create_alsa_device();
// Appends "alsa:default" and every hardware PCM of every card. `defaults`:
// mark "alsa:default" as the default input and output.
void alsa_list(std::vector<DeviceInfo>& out, bool defaults);
// True when libasound loads.
bool alsa_available();

} // namespace dstudio::linux_audio

#endif
