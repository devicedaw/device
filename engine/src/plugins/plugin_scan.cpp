#include "plugin_scan.hpp"

#include "plugin_modules.hpp"

#include <clap/clap.h>
#include <pluginterfaces/base/ipluginbase.h>
#include <pluginterfaces/gui/iplugview.h>
#include <pluginterfaces/vst/ivstaudioprocessor.h>
#include <pluginterfaces/vst/ivstcomponent.h>
#include <pluginterfaces/vst/ivsteditcontroller.h>
#include <public.sdk/source/vst/hosting/hostclasses.h>

#include <cstring>

#if defined(__APPLE__)
#include "au_util.hpp"
#include <AudioToolbox/AudioToolbox.h>
#endif

namespace dstudio {

#if defined(__APPLE__)

std::vector<AuComponent> enumerate_audio_units() {
    std::vector<AuComponent> out;
    for (OSType type : {kAudioUnitType_Effect, kAudioUnitType_MusicEffect, kAudioUnitType_MusicDevice}) {
        AudioComponentDescription d{};
        d.componentType = type;
        AudioComponent c = nullptr;
        while ((c = AudioComponentFindNext(c, &d)) != nullptr) {
            AudioComponentDescription cd{};
            if (AudioComponentGetDescription(c, &cd) != noErr) continue;
            AuComponent info;
            info.id = au_id_string(cd);
            CFStringRef name = nullptr;
            if (AudioComponentCopyName(c, &name) == noErr && name != nullptr) {
                const std::string full = cf_to_string(name);
                CFRelease(name);
                // "Vendor: Name"
                if (const auto colon = full.find(": "); colon != std::string::npos) {
                    info.vendor = full.substr(0, colon);
                    info.name = full.substr(colon + 2);
                } else {
                    info.name = full;
                }
            }
            if (info.name.empty()) info.name = info.id;
            UInt32 version = 0;
            AudioComponentGetVersion(c, &version);
            info.version = version;
            out.push_back(std::move(info));
        }
    }
    return out;
}

bool scan_audio_unit(const std::string& id, std::vector<ScannedPlugin>& out, std::string& err) {
    AudioComponentDescription d{};
    if (!parse_au_id(id, d)) {
        err = "invalid Audio Unit id";
        return false;
    }
    AudioComponent c = AudioComponentFindNext(nullptr, &d);
    if (c == nullptr) {
        err = "Audio Unit not found";
        return false;
    }
    std::string full;
    CFStringRef name = nullptr;
    if (AudioComponentCopyName(c, &name) == noErr && name != nullptr) {
        full = cf_to_string(name);
        CFRelease(name);
    }
    ScannedPlugin p;
    p.format = "audio_unit";
    p.id = id;
    if (const auto colon = full.find(": "); colon != std::string::npos) {
        p.vendor = full.substr(0, colon);
        p.name = full.substr(colon + 2);
    } else {
        p.name = full.empty() ? id : full;
    }
    UInt32 version = 0;
    AudioComponentGetVersion(c, &version);
    p.version = std::to_string(version >> 16) + "." + std::to_string((version >> 8) & 0xff) + "." +
                std::to_string(version & 0xff);
    p.instrument = d.componentType == kAudioUnitType_MusicDevice;

    AudioComponentInstance unit = nullptr;
    const OSStatus st = AudioComponentInstanceNew(c, &unit);
    if (st != noErr || unit == nullptr) {
        err = "AudioComponentInstanceNew failed (OSStatus " + std::to_string(st) + ")";
        return false;
    }
    const auto channels = [&](AudioUnitScope scope) -> uint32_t {
        UInt32 count = 0, size = sizeof count;
        if (AudioUnitGetProperty(unit, kAudioUnitProperty_ElementCount, scope, 0, &count, &size) != noErr || count == 0)
            return 0;
        AudioStreamBasicDescription f{};
        size = sizeof f;
        if (AudioUnitGetProperty(unit, kAudioUnitProperty_StreamFormat, scope, 0, &f, &size) != noErr) return 0;
        return f.mChannelsPerFrame;
    };
    p.audio_inputs = channels(kAudioUnitScope_Input);
    p.audio_outputs = channels(kAudioUnitScope_Output);
    p.has_editor = true; // custom Cocoa view, or the generic AUGenericView
    AudioComponentInstanceDispose(unit);
    out.push_back(std::move(p));
    return true;
}

#else

std::vector<AuComponent> enumerate_audio_units() { return {}; }
bool scan_audio_unit(const std::string&, std::vector<ScannedPlugin>&, std::string& err) {
    err = "Audio Units are only available on macOS";
    return false;
}

#endif

bool scan_vst3_bundle(const std::string& path, std::vector<ScannedPlugin>& out, std::string& err) {
    using namespace Steinberg;
    using namespace Steinberg::Vst;
    auto mod = load_vst3_module(path, err);
    if (!mod) return false;
    IPluginFactory* f = mod->factory;
    PFactoryInfo finfo;
    f->getFactoryInfo(&finfo);
    IPluginFactory2* f2 = nullptr;
    f->queryInterface(IPluginFactory2::iid, reinterpret_cast<void**>(&f2));
    static HostApplication* host = [] {
        auto* h = new HostApplication();
        h->addRef();
        return h;
    }();

    const int32 n = f->countClasses();
    for (int32 i = 0; i < n; ++i) {
        PClassInfo ci;
        if (f->getClassInfo(i, &ci) != kResultOk || std::strcmp(ci.category, kVstAudioEffectClass) != 0) continue;
        ScannedPlugin p;
        p.format = "vst3";
        p.id = vst3_uid_to_string(ci.cid);
        p.path = path;
        p.name = ci.name;
        p.vendor = finfo.vendor;
        PClassInfo2 c2;
        if (f2 != nullptr && f2->getClassInfo2(i, &c2) == kResultOk) {
            if (c2.vendor[0] != 0) p.vendor = c2.vendor;
            p.version = c2.version;
            p.instrument = std::strstr(c2.subCategories, "Instrument") != nullptr;
        }
        IComponent* comp = nullptr;
        if (f->createInstance(ci.cid, IComponent::iid, reinterpret_cast<void**>(&comp)) == kResultOk && comp) {
            if (comp->initialize(host) == kResultOk) {
                const auto main_channels = [&](BusDirection dir) -> uint32_t {
                    if (comp->getBusCount(kAudio, dir) < 1) return 0;
                    BusInfo bi{};
                    return comp->getBusInfo(kAudio, dir, 0, bi) == kResultOk ? static_cast<uint32_t>(bi.channelCount)
                                                                             : 0;
                };
                p.audio_inputs = main_channels(kInput);
                p.audio_outputs = main_channels(kOutput);
                // Editor: create the controller and ask for a view.
                IEditController* ctrl = nullptr;
                bool separate = false;
                if (comp->queryInterface(IEditController::iid, reinterpret_cast<void**>(&ctrl)) != kResultOk)
                    ctrl = nullptr;
                TUID ccid;
                if (ctrl == nullptr && comp->getControllerClassId(ccid) == kResultOk &&
                    f->createInstance(ccid, IEditController::iid, reinterpret_cast<void**>(&ctrl)) == kResultOk &&
                    ctrl != nullptr) {
                    separate = true;
                    if (ctrl->initialize(host) != kResultOk) {
                        ctrl->release();
                        ctrl = nullptr;
                    }
                }
                if (ctrl != nullptr) {
                    if (IPlugView* v = ctrl->createView(ViewType::kEditor)) {
                        p.has_editor = true;
                        v->release();
                    }
                    if (separate) ctrl->terminate();
                    ctrl->release();
                }
                comp->terminate();
            }
            comp->release();
        }
        out.push_back(std::move(p));
    }
    if (f2 != nullptr) f2->release();
    if (out.empty()) {
        err = "no audio effect/instrument classes in the VST3 bundle";
        return false;
    }
    return true;
}

// Loads the CLAP module at `path` and lists its plugins (id, name, vendor,
// kind, editor and audio ports); false with `err` set on failure.
bool scan_clap_bundle(const std::string& path, std::vector<ScannedPlugin>& out, std::string& err) {
    auto mod = load_clap_module(path, err);
    if (!mod) return false;
    const auto* factory = static_cast<const clap_plugin_factory_t*>(mod->entry->get_factory(CLAP_PLUGIN_FACTORY_ID));
    if (factory == nullptr) {
        err = "no CLAP plugin factory";
        return false;
    }
    clap_host_t host{};
    host.clap_version = CLAP_VERSION;
    host.name = "Device scanner";
    host.vendor = "Device";
    host.url = "";
    host.version = "0.5.0";
    host.get_extension = [](const clap_host*, const char*) -> const void* { return nullptr; };
    host.request_restart = [](const clap_host*) {};
    host.request_process = [](const clap_host*) {};
    host.request_callback = [](const clap_host*) {};

    const uint32_t n = factory->get_plugin_count(factory);
    for (uint32_t i = 0; i < n; ++i) {
        const clap_plugin_descriptor_t* d = factory->get_plugin_descriptor(factory, i);
        if (d == nullptr || d->id == nullptr) continue;
        ScannedPlugin p;
        p.format = "clap";
        p.id = d->id;
        p.path = path;
        p.name = d->name != nullptr ? d->name : d->id;
        p.vendor = d->vendor != nullptr ? d->vendor : "";
        p.version = d->version != nullptr ? d->version : "";
        if (d->features != nullptr)
            for (const char* const* f = d->features; *f != nullptr; ++f)
                if (std::strcmp(*f, CLAP_PLUGIN_FEATURE_INSTRUMENT) == 0) p.instrument = true;
        if (const clap_plugin_t* plug = factory->create_plugin(factory, &host, d->id)) {
            if (plug->init(plug)) {
                const auto* ports =
                    static_cast<const clap_plugin_audio_ports_t*>(plug->get_extension(plug, CLAP_EXT_AUDIO_PORTS));
                const auto main_channels = [&](bool input) -> uint32_t {
                    if (ports == nullptr || ports->count(plug, input) == 0) return 0;
                    clap_audio_port_info_t info{};
                    return ports->get(plug, 0, input, &info) ? info.channel_count : 0;
                };
                p.audio_inputs = main_channels(true);
                p.audio_outputs = main_channels(false);
                const auto* gui = static_cast<const clap_plugin_gui_t*>(plug->get_extension(plug, CLAP_EXT_GUI));
#if defined(__APPLE__)
                const char* api = CLAP_WINDOW_API_COCOA;
#elif defined(_WIN32)
                const char* api = CLAP_WINDOW_API_WIN32;
#else
                const char* api = CLAP_WINDOW_API_X11;
#endif
                p.has_editor = gui != nullptr && gui->is_api_supported(plug, api, false);
            }
            plug->destroy(plug);
        }
        out.push_back(std::move(p));
    }
    if (out.empty()) {
        err = "no plugins in the CLAP bundle";
        return false;
    }
    return true;
}

} // namespace dstudio
