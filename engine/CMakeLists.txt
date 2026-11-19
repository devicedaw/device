cmake_minimum_required(VERSION 3.25)
project(dstudio_engine VERSION 0.2.0 LANGUAGES C CXX)

# With a compiler launcher (CI sets CMAKE_<LANG>_COMPILER_LAUNCHER=ccache),
# MSVC debug info goes into each object (/Z7) rather than a shared .pdb
# (/Zi), which ccache cannot cache. Must be set before any target.
if(MSVC AND CMAKE_CXX_COMPILER_LAUNCHER)
  set(CMAKE_MSVC_DEBUG_INFORMATION_FORMAT "$<$<CONFIG:Debug,RelWithDebInfo>:Embedded>")
endif()

set(CMAKE_CXX_STANDARD 23)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)

if(NOT CMAKE_BUILD_TYPE AND NOT CMAKE_CONFIGURATION_TYPES)
  set(CMAKE_BUILD_TYPE Release CACHE STRING "Build type" FORCE)
endif()

option(DSTUDIO_ENGINE_BUILD_TESTS "Build engine tests" ${PROJECT_IS_TOP_LEVEL})
option(DSTUDIO_ENGINE_BUILD_BENCH "Build engine benchmark" ${PROJECT_IS_TOP_LEVEL})
option(DSTUDIO_ENGINE_BUILD_TOOLS "Build manual check tools" ${PROJECT_IS_TOP_LEVEL})
option(DSTUDIO_ENGINE_SANITIZE "Build everything with AddressSanitizer + UndefinedBehaviorSanitizer" OFF)
option(DSTUDIO_ENGINE_WERROR "Treat warnings as errors" OFF)
option(DSTUDIO_ENGINE_BUILD_SCANNER "Build the out-of-process plugin scanner" ON)
option(DSTUDIO_ENGINE_BUILD_TEST_PLUGINS "Build the test plugin bundles (DStudioTest*.vst3/.clap)" ${PROJECT_IS_TOP_LEVEL})

if(APPLE)
  enable_language(OBJCXX)
  set(CMAKE_OBJCXX_STANDARD 23)
  set(CMAKE_OBJCXX_STANDARD_REQUIRED ON)
  set(CMAKE_OBJCXX_EXTENSIONS OFF)
endif()

# ---------------------------------------------------------------- third-party SDKs
# Fetched at configure time at pinned tags and never vendored into git
# (see third_party_licenses.md). Only headers/sources needed for hosting and
# for the test plugins are used; nothing is added via add_subdirectory.
# Set FETCHCONTENT_BASE_DIR to share one download between build trees, or
# FETCHCONTENT_SOURCE_DIR_<NAME> to use a local checkout (offline builds).
#
# Each tag is recorded with the commit it pointed at when it was pinned
# (DSTUDIO_<NAME>_TAG_COMMIT). The configure step fails if a fetched tag is at
# a different commit, so a tag moved upstream cannot change what we build.
# tools/ci/engine_pins.py (weekly "Engine pins" workflow) reports pins that
# are behind upstream or whose tag moved; docs/building.md says how to update.
include(FetchContent)
set(DSTUDIO_CLAP_TAG 1.2.10)
set(DSTUDIO_CLAP_TAG_COMMIT 195b42a004144fab0b3cf95e9c067187d15365b7)
set(DSTUDIO_VST3SDK_TAG v3.8.1_build_84)
set(DSTUDIO_VST3SDK_TAG_COMMIT 3cdf9ca5d1f5b1b21e0a86832aa4abe55607bd96)
FetchContent_Declare(dstudio_clap
  GIT_REPOSITORY https://github.com/free-audio/clap.git
  GIT_TAG ${DSTUDIO_CLAP_TAG}
  GIT_SHALLOW TRUE
  SOURCE_SUBDIR _dstudio_headers_only)
FetchContent_Declare(dstudio_vst3sdk
  GIT_REPOSITORY https://github.com/steinbergmedia/vst3sdk.git
  GIT_TAG ${DSTUDIO_VST3SDK_TAG}
  GIT_SHALLOW TRUE
  GIT_SUBMODULES pluginterfaces public.sdk
  SOURCE_SUBDIR _dstudio_sources_only)
# Time stretching for warped audio clips (src/warp.cpp): signalsmith-stretch
# and the FFT / STFT library it is built on. Header-only.
set(DSTUDIO_STRETCH_TAG 1.4.0)
set(DSTUDIO_STRETCH_TAG_COMMIT a670068d9aeb64913331d5cc29337b19a457a7df)
set(DSTUDIO_LINEAR_TAG 0.6.6)
set(DSTUDIO_LINEAR_TAG_COMMIT 7bb1c8f36794d6980080ce817c91f8da8fe56a66)
FetchContent_Declare(dstudio_signalsmith_stretch
  GIT_REPOSITORY https://github.com/Signalsmith-Audio/signalsmith-stretch.git
  GIT_TAG ${DSTUDIO_STRETCH_TAG}
  GIT_SHALLOW TRUE
  SOURCE_SUBDIR _dstudio_headers_only)
FetchContent_Declare(dstudio_signalsmith_linear
  GIT_REPOSITORY https://github.com/Signalsmith-Audio/linear.git
  GIT_TAG ${DSTUDIO_LINEAR_TAG}
  GIT_SHALLOW TRUE
  SOURCE_SUBDIR _dstudio_headers_only)
# Compressed audio import (src/decode.cpp): dr_mp3, dr_flac and dr_wav (AIFF only) from
# dr_libs, stb_vorbis from stb. Single-file C libraries, pinned by commit (neither repository
# tags all of its files). dr_libs at the commit noted in third_party_licenses.md (dr_mp3 0.7.5, dr_flac
# 0.13.5, dr_wav 0.14.7); stb at the commit noted there too (stb_vorbis 1.24).
set(DSTUDIO_DR_LIBS_COMMIT f12e197fd462e4c22873fbd70d11b1d2f2c1dc3c)
set(DSTUDIO_STB_COMMIT 9600522de53973336fa033a289858a4354cbb04c)
FetchContent_Declare(dstudio_dr_libs
  GIT_REPOSITORY https://github.com/mackron/dr_libs.git
  GIT_TAG ${DSTUDIO_DR_LIBS_COMMIT}
  SOURCE_SUBDIR _dstudio_headers_only)
FetchContent_Declare(dstudio_stb
  GIT_REPOSITORY https://github.com/nothings/stb.git
  GIT_TAG ${DSTUDIO_STB_COMMIT}
  SOURCE_SUBDIR _dstudio_headers_only)
FetchContent_MakeAvailable(dstudio_clap dstudio_vst3sdk dstudio_signalsmith_stretch dstudio_signalsmith_linear dstudio_dr_libs dstudio_stb)
# Check every fetched checkout is at its pinned commit. Skipped for a local
# checkout given with FETCHCONTENT_SOURCE_DIR_<NAME> and for a source tree
# without git metadata (an unpacked archive).
function(dstudio_verify_pin name expected)
  string(TOUPPER "${name}" upper)
  if(FETCHCONTENT_SOURCE_DIR_${upper} OR NOT EXISTS "${${name}_SOURCE_DIR}/.git")
    return()
  endif()
  find_package(Git QUIET)
  if(NOT Git_FOUND)
    return()
  endif()
  execute_process(COMMAND "${GIT_EXECUTABLE}" rev-parse HEAD
    WORKING_DIRECTORY "${${name}_SOURCE_DIR}"
    OUTPUT_VARIABLE actual OUTPUT_STRIP_TRAILING_WHITESPACE ERROR_QUIET)
  if(actual AND NOT actual STREQUAL expected)
    message(FATAL_ERROR "${name}: fetched commit ${actual} is not the pinned ${expected}. "
      "The upstream tag may have moved; see docs/building.md (Engine dependencies).")
  endif()
endfunction()
dstudio_verify_pin(dstudio_clap ${DSTUDIO_CLAP_TAG_COMMIT})
dstudio_verify_pin(dstudio_vst3sdk ${DSTUDIO_VST3SDK_TAG_COMMIT})
dstudio_verify_pin(dstudio_signalsmith_stretch ${DSTUDIO_STRETCH_TAG_COMMIT})
dstudio_verify_pin(dstudio_signalsmith_linear ${DSTUDIO_LINEAR_TAG_COMMIT})
dstudio_verify_pin(dstudio_dr_libs ${DSTUDIO_DR_LIBS_COMMIT})
dstudio_verify_pin(dstudio_stb ${DSTUDIO_STB_COMMIT})
set(DSTUDIO_DECODER_INCLUDE "${dstudio_dr_libs_SOURCE_DIR}" "${dstudio_stb_SOURCE_DIR}")
set(DSTUDIO_STRETCH_INCLUDE "${dstudio_signalsmith_stretch_SOURCE_DIR}" "${dstudio_signalsmith_linear_SOURCE_DIR}/include")
set(DSTUDIO_CLAP_INCLUDE "${dstudio_clap_SOURCE_DIR}/include")
set(DSTUDIO_VST3SDK_DIR "${dstudio_vst3sdk_SOURCE_DIR}")
# IID definitions every VST3 module needs (host and test plugin).
set(DSTUDIO_VST3_IID_SOURCES
  ${DSTUDIO_VST3SDK_DIR}/pluginterfaces/base/funknown.cpp
  ${DSTUDIO_VST3SDK_DIR}/pluginterfaces/base/coreiids.cpp
  ${DSTUDIO_VST3SDK_DIR}/public.sdk/source/vst/vstinitiids.cpp
  ${DSTUDIO_VST3SDK_DIR}/public.sdk/source/common/commoniids.cpp)
# SDK host helpers (IHostApplication / IMessage / IAttributeList).
set(DSTUDIO_VST3_HOST_SOURCES
  ${DSTUDIO_VST3_IID_SOURCES}
  ${DSTUDIO_VST3SDK_DIR}/public.sdk/source/vst/hosting/hostclasses.cpp
  ${DSTUDIO_VST3SDK_DIR}/public.sdk/source/vst/hosting/pluginterfacesupport.cpp
  ${DSTUDIO_VST3SDK_DIR}/public.sdk/source/vst/utility/stringconvert.cpp
  ${DSTUDIO_VST3SDK_DIR}/public.sdk/source/common/commonstringconvert.cpp)
# Third-party code is compiled without our warning set: the host helpers
# in their own object library (no /W4 to override), the IID files of the
# test plugins with warnings off.
set_source_files_properties(${DSTUDIO_VST3_HOST_SOURCES} PROPERTIES COMPILE_OPTIONS
  "$<IF:$<CXX_COMPILER_ID:MSVC>,/w,-w>")
add_library(dstudio_vst3_host OBJECT ${DSTUDIO_VST3_HOST_SOURCES})
target_include_directories(dstudio_vst3_host SYSTEM PRIVATE ${DSTUDIO_VST3SDK_DIR})
target_compile_features(dstudio_vst3_host PRIVATE cxx_std_23)
set_target_properties(dstudio_vst3_host PROPERTIES POSITION_INDEPENDENT_CODE ON)
# The decoder implementations, also without our warnings.
add_library(dstudio_decoders OBJECT src/decoders_impl.c)
target_include_directories(dstudio_decoders SYSTEM PRIVATE ${DSTUDIO_DECODER_INCLUDE})
target_compile_options(dstudio_decoders PRIVATE "$<IF:$<C_COMPILER_ID:MSVC>,/w,-w>")
set_target_properties(dstudio_decoders PROPERTIES POSITION_INDEPENDENT_CODE ON)

# ---------------------------------------------------------------- warnings
add_library(dstudio_engine_warnings INTERFACE)
if(MSVC)
  # C4324 "structure was padded due to alignment specifier" is the intended
  # effect of alignas(64) on the lock-free queues (false-sharing padding).
  target_compile_options(dstudio_engine_warnings INTERFACE /W4 /permissive- /wd4324)
  if(DSTUDIO_ENGINE_WERROR)
    target_compile_options(dstudio_engine_warnings INTERFACE /WX)
  endif()
else()
  target_compile_options(dstudio_engine_warnings INTERFACE
    -Wall -Wextra -Wpedantic -Wshadow -Wconversion -Wno-sign-conversion
    -Wnon-virtual-dtor -Wold-style-cast -Wcast-align -Woverloaded-virtual
    -Wnull-dereference -Wimplicit-fallthrough)
  if(DSTUDIO_ENGINE_WERROR)
    target_compile_options(dstudio_engine_warnings INTERFACE -Werror)
  endif()
endif()

if(DSTUDIO_ENGINE_SANITIZE)
  if(MSVC)
    message(FATAL_ERROR "DSTUDIO_ENGINE_SANITIZE is only wired up for clang/gcc")
  endif()
  add_compile_options(-fsanitize=address,undefined -fno-sanitize-recover=all -fno-omit-frame-pointer)
  add_link_options(-fsanitize=address,undefined)
endif()

# ---------------------------------------------------------------- library
add_library(dstudio_engine STATIC
  src/c_api.cpp
  src/engine.cpp
  src/processor.cpp
  src/recorder.cpp
  src/midi_input.cpp
  src/render_graph.cpp
  src/resampler.cpp
  src/synth.cpp
  src/warp.cpp
  src/wav.cpp
  src/decode.cpp
  src/plugins/plugin_common.cpp
  src/plugins/plugin_host.cpp
  src/plugins/plugin_modules.cpp
  src/plugins/plugin_scan.cpp
  src/plugins/au_instance.cpp
  src/plugins/vst3_instance.cpp
  src/plugins/clap_instance.cpp
  src/devices/builtin.cpp
  src/devices/bitcrusher.cpp
  src/devices/compressor.cpp
  src/devices/channel_eq.cpp
  src/devices/character_eq.cpp
  src/devices/auto_filter.cpp
  src/devices/gate.cpp
  src/devices/glue_compressor.cpp
  src/devices/limiter.cpp
  src/devices/mastering_limiter.cpp
  src/devices/transient_shaper.cpp
  src/devices/character_compressor.cpp
  src/devices/multiband_compressor.cpp
  src/devices/lfo.cpp
  src/devices/rack.cpp
  src/devices/distortion.cpp
  src/devices/soft_clipper.cpp
  src/devices/tape_saturation.cpp
  src/devices/utility.cpp
  src/devices/widener.cpp
  src/devices/dsp.cpp
  src/devices/autopanner.cpp
  src/devices/character_mod.cpp
  src/devices/chorus.cpp
  src/devices/flanger.cpp
  src/devices/phaser.cpp
  src/devices/ring_mod.cpp
  src/devices/tremolo.cpp
  src/devices/delay.cpp
  src/devices/echo.cpp
  src/devices/grains.cpp
  src/devices/pitch_shift.cpp
  src/devices/pitch_shifter.cpp
  src/devices/reverb.cpp
  src/devices/pad_sampler.cpp
  src/devices/sampler.cpp
  src/devices/sampler_engine.cpp
  src/devices/synth_v2.cpp
  src/devices/synth_v2_fx.cpp
  src/devices/declipper.cpp
  src/devices/deesser.cpp
  src/devices/dehummer.cpp
  src/devices/denoiser.cpp
  src/devices/pitch_corrector.cpp
  src/devices/pitch_detect.cpp
  src/devices/spectrum.cpp
  src/devices/tuner.cpp
  src/devices/shaper.cpp
  src/devices/trance_gate.cpp
  $<TARGET_OBJECTS:dstudio_vst3_host>
  $<TARGET_OBJECTS:dstudio_decoders>
  # Platform backends: each file is compile-guarded so exactly one is active.
  src/platform/audio_device.cpp
  src/platform/coreaudio_device.cpp
  src/platform/linux_audio_device.cpp
  src/platform/pipewire_device.cpp
  src/platform/alsa_device.cpp
  src/platform/wasapi_device.cpp
  src/platform/midi_backend_stub.cpp
  src/platform/alsa_midi_input.cpp
  src/platform/coremidi_input.cpp
  src/platform/winmm_midi_input.cpp
  src/platform/os.cpp
)
if(WIN32)
  target_sources(dstudio_engine PRIVATE src/platform/plugin_editor_win.cpp)
elseif(NOT APPLE)
  # Linux: the plugin run loop always; X11 editor windows unless DSTUDIO_ENGINE_X11
  # is OFF (then the stub reports that editors need X11).
  target_sources(dstudio_engine PRIVATE src/platform/run_loop.cpp src/platform/plugin_editor_stub.cpp
    src/platform/plugin_editor_x11.cpp)
endif()
if(APPLE)
  target_sources(dstudio_engine PRIVATE src/platform/plugin_editor_mac.mm)
  # ARC; bridging casts are C-style by necessity.
  set_source_files_properties(src/platform/plugin_editor_mac.mm PROPERTIES
    COMPILE_OPTIONS "-fobjc-arc;-Wno-old-style-cast;-Wno-cast-align")
endif()
target_include_directories(dstudio_engine
  PUBLIC
    $<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}/include>
    $<INSTALL_INTERFACE:include>
  PRIVATE
    ${CMAKE_CURRENT_SOURCE_DIR}/src)
# SDK headers are third-party: include them as system headers (no warnings).
target_include_directories(dstudio_engine SYSTEM PRIVATE ${DSTUDIO_CLAP_INCLUDE} ${DSTUDIO_VST3SDK_DIR} ${DSTUDIO_STRETCH_INCLUDE} ${DSTUDIO_DECODER_INCLUDE})
if(APPLE)
  # vDSP FFTs for signalsmith-linear (Accelerate is linked below).
  set_property(SOURCE src/warp.cpp APPEND PROPERTY COMPILE_DEFINITIONS SIGNALSMITH_USE_ACCELERATE ACCELERATE_NEW_LAPACK)
endif()
target_compile_features(dstudio_engine PUBLIC cxx_std_23)
set_target_properties(dstudio_engine PROPERTIES POSITION_INDEPENDENT_CODE ON)
target_link_libraries(dstudio_engine PRIVATE $<BUILD_INTERFACE:dstudio_engine_warnings>)
if(NOT MSVC)
  # DSP code: flag accidental float->double promotion (slow paths, surprises).
  # Not applied to tests/bench/tools, which compare in double on purpose.
  target_compile_options(dstudio_engine PRIVATE $<$<COMPILE_LANGUAGE:CXX>:-Wdouble-promotion>)
endif()

if(APPLE)
  target_link_libraries(dstudio_engine PUBLIC
    "-framework AudioToolbox" "-framework AudioUnit" "-framework CoreAudio" "-framework CoreFoundation"
    "-framework CoreAudioKit" "-framework AppKit" "-framework Foundation" "-framework Accelerate"
    "-framework CoreMIDI")
endif()
if(WIN32)
  # ole32: COM (WASAPI, MMDevice); shell32: CommandLineToArgvW; avrt: MMCSS;
  # user32: plugin editor windows; winmm: MIDI input.
  target_link_libraries(dstudio_engine PUBLIC ole32 shell32 avrt user32 winmm)
elseif(NOT APPLE)
  target_link_libraries(dstudio_engine PUBLIC ${CMAKE_DL_LIBS})
endif()
if(CMAKE_SYSTEM_NAME STREQUAL "Linux")
  # Audio backends (src/platform/linux_audio_device.cpp picks one): PipeWire
  # (pipewire_device.cpp) and ALSA (alsa_device.cpp, below). Only the
  # PipeWire headers are needed: libpipewire is loaded with dlopen, so the
  # engine runs (through ALSA) on systems without PipeWire. OFF builds an
  # engine with ALSA audio only.
  option(DSTUDIO_ENGINE_PIPEWIRE "Build the PipeWire audio backend (needs libpipewire-0.3 development files)" ON)
  if(DSTUDIO_ENGINE_PIPEWIRE)
    find_package(PkgConfig REQUIRED)
    pkg_check_modules(PIPEWIRE REQUIRED IMPORTED_TARGET libpipewire-0.3)
    target_include_directories(dstudio_engine SYSTEM PRIVATE
      $<TARGET_PROPERTY:PkgConfig::PIPEWIRE,INTERFACE_INCLUDE_DIRECTORIES>)
    target_compile_options(dstudio_engine PRIVATE
      $<TARGET_PROPERTY:PkgConfig::PIPEWIRE,INTERFACE_COMPILE_OPTIONS>)
    target_link_libraries(dstudio_engine PUBLIC ${CMAKE_DL_LIBS})
    target_compile_definitions(dstudio_engine PRIVATE DSTUDIO_HAVE_PIPEWIRE=1)
  endif()
  # Plugin editor windows (src/platform/plugin_editor_x11.cpp). Every Linux
  # plugin GUI is X11; on Wayland they run through XWayland.
  option(DSTUDIO_ENGINE_X11 "Build Linux plugin editor windows (needs libX11 development files)" ON)
  if(DSTUDIO_ENGINE_X11)
    find_package(PkgConfig REQUIRED)
    pkg_check_modules(X11 REQUIRED IMPORTED_TARGET x11)
    # Headers only: plugin_editor_x11.cpp loads libX11 with dlopen when the
    # first editor opens, so processes that never open one don't load it.
    target_include_directories(dstudio_engine PRIVATE
      $<TARGET_PROPERTY:PkgConfig::X11,INTERFACE_INCLUDE_DIRECTORIES>)
    target_compile_definitions(dstudio_engine PRIVATE DSTUDIO_HAVE_X11=1)
  endif()
  # ALSA audio (src/platform/alsa_device.cpp) and MIDI input
  # (src/platform/alsa_midi_input.cpp). OFF builds an engine without ALSA
  # devices and MIDI ports (se_midi_inject still works). Only the headers are
  # needed: libasound is loaded with dlopen when ALSA is first used.
  option(DSTUDIO_ENGINE_ALSA "Build ALSA audio and MIDI input (needs libasound2 development files)" ON)
  if(DSTUDIO_ENGINE_ALSA)
    find_package(PkgConfig REQUIRED)
    pkg_check_modules(ALSA REQUIRED alsa)
    target_include_directories(dstudio_engine SYSTEM PRIVATE ${ALSA_INCLUDE_DIRS})
    target_link_libraries(dstudio_engine PUBLIC ${CMAKE_DL_LIBS})
    target_compile_definitions(dstudio_engine PRIVATE DSTUDIO_HAVE_ALSA=1)
  endif()
endif()
find_package(Threads REQUIRED)
target_link_libraries(dstudio_engine PUBLIC Threads::Threads)

# Installed layout for the Rust `cmake` crate (it runs the install target):
#   <prefix>/lib/libdstudio_engine.a, <prefix>/include/dstudio_engine.h
install(TARGETS dstudio_engine ARCHIVE DESTINATION lib)
install(FILES include/dstudio_engine.h DESTINATION include)

# ---------------------------------------------------------------- plugin scanner
# Out-of-process scanner (see README "Plugins > Scanning"). The Rust crate
# finds it at the path baked in by build.rs, or via DSTUDIO_PLUGIN_SCANNER.
if(DSTUDIO_ENGINE_BUILD_SCANNER)
  add_executable(dstudio_plugin_scanner scanner/scanner_main.cpp)
  target_include_directories(dstudio_plugin_scanner PRIVATE src)
  target_include_directories(dstudio_plugin_scanner SYSTEM PRIVATE ${DSTUDIO_CLAP_INCLUDE} ${DSTUDIO_VST3SDK_DIR})
  target_link_libraries(dstudio_plugin_scanner PRIVATE dstudio_engine dstudio_engine_warnings)
endif()

# ---------------------------------------------------------------- test plugins
# Minimal original plugins, built as proper bundles into <build>/test_plugins:
# DStudioTestGain.vst3, DStudioTestGain.clap, DStudioTestSine.clap,
# DStudioTestLatency.clap/.vst3 (delay by N frames, report latency N) and
# DStudioTestCrash.clap (crashes in clap_entry.init; scanner blocklist test).
set(DSTUDIO_TEST_PLUGIN_DIR "${CMAKE_CURRENT_BINARY_DIR}/test_plugins")
if(DSTUDIO_ENGINE_BUILD_TEST_PLUGINS AND APPLE)
  function(dstudio_test_bundle target output ext)
    add_library(${target} MODULE ${ARGN})
    set_target_properties(${target} PROPERTIES
      BUNDLE TRUE
      BUNDLE_EXTENSION ${ext}
      OUTPUT_NAME ${output}
      LIBRARY_OUTPUT_DIRECTORY "${DSTUDIO_TEST_PLUGIN_DIR}"
      MACOSX_BUNDLE_INFO_PLIST "${CMAKE_CURRENT_SOURCE_DIR}/test_plugins/Info.plist.in"
      MACOSX_BUNDLE_BUNDLE_NAME ${output}
      MACOSX_BUNDLE_GUI_IDENTIFIER "studio.dstudio.test.${output}.${ext}"
      CXX_VISIBILITY_PRESET hidden
      VISIBILITY_INLINES_HIDDEN ON)
    target_link_libraries(${target} PRIVATE "-framework CoreFoundation")
  endfunction()
  dstudio_test_bundle(DStudioTestGainClap DStudioTestGain clap test_plugins/dstudio_test_clap.cpp)
  target_compile_definitions(DStudioTestGainClap PRIVATE DSTUDIO_TEST_CLAP_GAIN=1)
  dstudio_test_bundle(DStudioTestSineClap DStudioTestSine clap test_plugins/dstudio_test_clap.cpp)
  target_compile_definitions(DStudioTestSineClap PRIVATE DSTUDIO_TEST_CLAP_SINE=1)
  dstudio_test_bundle(DStudioTestCrashClap DStudioTestCrash clap test_plugins/dstudio_test_crash.cpp)
  dstudio_test_bundle(DStudioTestLatencyClap DStudioTestLatency clap test_plugins/dstudio_test_latency_clap.cpp)
  foreach(t DStudioTestGainClap DStudioTestSineClap DStudioTestCrashClap DStudioTestLatencyClap)
    target_include_directories(${t} SYSTEM PRIVATE ${DSTUDIO_CLAP_INCLUDE})
  endforeach()
  dstudio_test_bundle(DStudioTestGainVst3 DStudioTestGain vst3 test_plugins/dstudio_test_vst3.cpp ${DSTUDIO_VST3_IID_SOURCES})
  target_include_directories(DStudioTestGainVst3 SYSTEM PRIVATE ${DSTUDIO_VST3SDK_DIR})
  dstudio_test_bundle(DStudioTestLatencyVst3 DStudioTestLatency vst3 test_plugins/dstudio_test_latency_vst3.cpp
    ${DSTUDIO_VST3_IID_SOURCES})
  target_include_directories(DStudioTestLatencyVst3 SYSTEM PRIVATE ${DSTUDIO_VST3SDK_DIR})
  add_custom_target(dstudio_test_plugins DEPENDS DStudioTestGainClap DStudioTestSineClap DStudioTestCrashClap DStudioTestGainVst3
    DStudioTestLatencyClap DStudioTestLatencyVst3)
elseif(DSTUDIO_ENGINE_BUILD_TEST_PLUGINS)
  # Windows / Linux: a CLAP is a single shared library named *.clap; a VST3
  # is a bundle folder <name>.vst3/Contents/<arch>-win/<name>.vst3 (DLL) or
  # <name>.vst3/Contents/<arch>-linux/<name>.so. "$<1:...>" keeps
  # multi-config generators from appending a per-config subfolder.
  if(CMAKE_SYSTEM_PROCESSOR MATCHES "^(AMD64|x86_64|amd64|x64)$")
    set(_dstudio_vst3_arch x86_64)
  elseif(CMAKE_SYSTEM_PROCESSOR MATCHES "^(ARM64|arm64|aarch64)$")
    if(WIN32)
      set(_dstudio_vst3_arch arm64)
    else()
      set(_dstudio_vst3_arch aarch64)
    endif()
  else()
    set(_dstudio_vst3_arch ${CMAKE_SYSTEM_PROCESSOR})
  endif()
  function(dstudio_test_module target output dir suffix)
    add_library(${target} MODULE ${ARGN})
    set_target_properties(${target} PROPERTIES
      PREFIX ""
      SUFFIX ${suffix}
      OUTPUT_NAME ${output}
      LIBRARY_OUTPUT_DIRECTORY "$<1:${dir}>"
      RUNTIME_OUTPUT_DIRECTORY "$<1:${dir}>"
      CXX_VISIBILITY_PRESET hidden
      VISIBILITY_INLINES_HIDDEN ON)
    if(MSVC)
      target_link_options(${target} PRIVATE /NOIMPLIB /NOEXP)
    endif()
  endfunction()
  function(dstudio_test_clap target output)
    dstudio_test_module(${target} ${output} "${DSTUDIO_TEST_PLUGIN_DIR}" ".clap" ${ARGN})
    target_include_directories(${target} SYSTEM PRIVATE ${DSTUDIO_CLAP_INCLUDE})
  endfunction()
  function(dstudio_test_vst3 target output)
    if(WIN32)
      set(_dir "${DSTUDIO_TEST_PLUGIN_DIR}/${output}.vst3/Contents/${_dstudio_vst3_arch}-win")
      set(_suffix ".vst3")
    else()
      set(_dir "${DSTUDIO_TEST_PLUGIN_DIR}/${output}.vst3/Contents/${_dstudio_vst3_arch}-linux")
      set(_suffix ".so")
    endif()
    dstudio_test_module(${target} ${output} "${_dir}" "${_suffix}" ${ARGN} ${DSTUDIO_VST3_IID_SOURCES})
    target_include_directories(${target} SYSTEM PRIVATE ${DSTUDIO_VST3SDK_DIR})
  endfunction()
  dstudio_test_clap(DStudioTestGainClap DStudioTestGain test_plugins/dstudio_test_clap.cpp)
  target_compile_definitions(DStudioTestGainClap PRIVATE DSTUDIO_TEST_CLAP_GAIN=1)
  dstudio_test_clap(DStudioTestSineClap DStudioTestSine test_plugins/dstudio_test_clap.cpp)
  target_compile_definitions(DStudioTestSineClap PRIVATE DSTUDIO_TEST_CLAP_SINE=1)
  dstudio_test_clap(DStudioTestCrashClap DStudioTestCrash test_plugins/dstudio_test_crash.cpp)
  dstudio_test_clap(DStudioTestLatencyClap DStudioTestLatency test_plugins/dstudio_test_latency_clap.cpp)
  dstudio_test_vst3(DStudioTestGainVst3 DStudioTestGain test_plugins/dstudio_test_vst3.cpp)
  dstudio_test_vst3(DStudioTestLatencyVst3 DStudioTestLatency test_plugins/dstudio_test_latency_vst3.cpp)
  add_custom_target(dstudio_test_plugins DEPENDS DStudioTestGainClap DStudioTestSineClap DStudioTestCrashClap DStudioTestGainVst3
    DStudioTestLatencyClap DStudioTestLatencyVst3)
  if(WIN32)
    # Win32 clap.gui for the editor host test (plugin_editor_window_win32_*).
    dstudio_test_clap(DStudioTestGuiClap DStudioTestGui test_plugins/dstudio_test_gui_clap.cpp)
    target_link_libraries(DStudioTestGuiClap PRIVATE user32)
    add_dependencies(dstudio_test_plugins DStudioTestGuiClap)
  elseif(TARGET PkgConfig::X11)
    # X11 clap.gui (+ timer and fd support) for plugin_editor_window_x11_*.
    dstudio_test_clap(DStudioTestGuiClap DStudioTestGui test_plugins/dstudio_test_gui_clap.cpp)
    target_link_libraries(DStudioTestGuiClap PRIVATE PkgConfig::X11)
    add_dependencies(dstudio_test_plugins DStudioTestGuiClap)
  endif()
endif()

# What the Rust crate builds (build.rs): library, scanner, CLAP test gain.
add_custom_target(dstudio_engine_rust ALL DEPENDS dstudio_engine)
if(DSTUDIO_ENGINE_BUILD_SCANNER)
  add_dependencies(dstudio_engine_rust dstudio_plugin_scanner)
endif()
if(TARGET dstudio_test_plugins)
  add_dependencies(dstudio_engine_rust dstudio_test_plugins)
endif()

# ---------------------------------------------------------------- tests
if(DSTUDIO_ENGINE_BUILD_TESTS)
  enable_testing()
  add_executable(dstudio_engine_tests
    tests/test_main.cpp
    tests/test_util.cpp
    tests/test_wav.cpp
    tests/test_resampler.cpp
    tests/test_processing.cpp
    tests/test_transport.cpp
    tests/test_resources.cpp
    tests/test_concurrency.cpp
    tests/test_rt_safety.cpp
    tests/test_features.cpp
    tests/test_midi.cpp
    tests/test_midi_input.cpp
    tests/test_synth.cpp
    tests/test_recording.cpp
    tests/test_plugins.cpp
    tests/test_plugin_scan.cpp
    tests/test_pdc.cpp
    tests/test_routing.cpp
    tests/test_strip.cpp
    tests/test_automation.cpp
    tests/test_frame_ring.cpp
    tests/test_wasapi_period.cpp
    tests/test_linux_audio.cpp
    tests/test_warp.cpp
    tests/test_clip_loop.cpp
    tests/test_clip_fade.cpp
    tests/test_preview.cpp
    tests/test_decode.cpp
    tests/devices/test_builtin.cpp
    tests/devices/test_compressor.cpp
    tests/devices/test_eq.cpp
    tests/devices/test_auto_filter.cpp
    tests/devices/test_gate.cpp
    tests/devices/test_glue_compressor.cpp
    tests/devices/test_limiter.cpp
    tests/devices/test_mastering_limiter.cpp
    tests/devices/test_transient_shaper.cpp
    tests/devices/test_character_compressor.cpp
    tests/devices/test_multiband_compressor.cpp
    tests/devices/test_drive.cpp
    tests/devices/test_imaging.cpp
    tests/devices/test_modulation.cpp
    tests/devices/test_mod_routes.cpp
    tests/devices/test_character_mod.cpp
    tests/devices/test_time_fx.cpp
    tests/devices/test_drum_rack.cpp
    tests/devices/test_sampler.cpp
    tests/devices/test_synth_v2.cpp
    tests/devices/test_restoration.cpp

    tests/devices/test_pitch_corrector.cpp
    tests/devices/test_pitch_detect.cpp
    tests/devices/test_spectrum.cpp
    tests/devices/test_tuner.cpp
    tests/devices/test_shaper.cpp
    tests/devices/test_trance_gate.cpp)
  target_include_directories(dstudio_engine_tests PRIVATE src tests)
  target_link_libraries(dstudio_engine_tests PRIVATE dstudio_engine dstudio_engine_warnings)
  if(CMAKE_CXX_COMPILER_ID STREQUAL "GNU")
    # GCC 13 -O2 false positive (free-nonheap-object) on std::vector range
    # construction in test_plugins.cpp after constant-folding a loop bound.
    target_compile_options(dstudio_engine_tests PRIVATE -Wno-free-nonheap-object)
  endif()
  target_compile_definitions(dstudio_engine_tests PRIVATE
    DSTUDIO_TEST_PLUGIN_DIR="${DSTUDIO_TEST_PLUGIN_DIR}"
    DSTUDIO_PLUGIN_SCANNER_PATH="$<TARGET_FILE:dstudio_plugin_scanner>")
  if(TARGET dstudio_test_plugins)
    add_dependencies(dstudio_engine_tests dstudio_test_plugins dstudio_plugin_scanner)
  endif()
  if(TARGET DStudioTestGuiClap AND NOT WIN32)
    # plugin_editor_window_x11_*: inspects the editor windows over Xlib.
    target_link_libraries(dstudio_engine_tests PRIVATE PkgConfig::X11)
    target_compile_definitions(dstudio_engine_tests PRIVATE DSTUDIO_TEST_X11_EDITOR=1)
  endif()
  if(DSTUDIO_ENGINE_ALSA AND CMAKE_SYSTEM_NAME STREQUAL "Linux")
    # test_linux_audio.cpp: the ALSA backend against ALSA's null PCM.
    target_compile_definitions(dstudio_engine_tests PRIVATE DSTUDIO_HAVE_ALSA=1)
  endif()

  # Each TEST_CASE becomes its own CTest test. The list is produced by the
  # test binary itself after every build (cf. gtest_discover_tests).
  set(_list "${CMAKE_CURRENT_BINARY_DIR}/dstudio_engine_tests_list.cmake")
  set(_wrapper "${CMAKE_CURRENT_BINARY_DIR}/dstudio_engine_tests_include.cmake")
  add_custom_command(TARGET dstudio_engine_tests POST_BUILD
    COMMAND dstudio_engine_tests --write-ctest "${_list}" "$<TARGET_FILE:dstudio_engine_tests>"
    BYPRODUCTS "${_list}"
    COMMENT "Discovering dstudio_engine_tests test cases")
  file(WRITE "${_wrapper}"
    "if(EXISTS \"${_list}\")\n  include(\"${_list}\")\nelse()\n"
    "  add_test(dstudio_engine_tests_NOT_BUILT dstudio_engine_tests_NOT_BUILT)\nendif()\n")
  set_property(DIRECTORY APPEND PROPERTY TEST_INCLUDE_FILES "${_wrapper}")
endif()

# ---------------------------------------------------------------- bench
if(DSTUDIO_ENGINE_BUILD_BENCH)
  add_executable(dstudio_engine_bench bench/bench_render.cpp)
  target_include_directories(dstudio_engine_bench PRIVATE src)
  target_link_libraries(dstudio_engine_bench PRIVATE dstudio_engine dstudio_engine_warnings)
  target_compile_definitions(dstudio_engine_bench PRIVATE DSTUDIO_TEST_PLUGIN_DIR="${DSTUDIO_TEST_PLUGIN_DIR}")
  if(TARGET dstudio_test_plugins)
    add_dependencies(dstudio_engine_bench dstudio_test_plugins)
  endif()
  add_executable(dstudio_frame_ring_bench bench/bench_frame_ring.cpp)
  target_include_directories(dstudio_frame_ring_bench PRIVATE src)
  target_link_libraries(dstudio_frame_ring_bench PRIVATE dstudio_engine_warnings)
  add_executable(dstudio_pcm_convert_bench bench/bench_pcm_convert.cpp)
  target_include_directories(dstudio_pcm_convert_bench PRIVATE src)
  target_link_libraries(dstudio_pcm_convert_bench PRIVATE dstudio_engine_warnings)

  if(CMAKE_SYSTEM_NAME STREQUAL "Linux")
    add_executable(dstudio_run_loop_bench bench/bench_run_loop.cpp src/platform/run_loop.cpp)
    target_include_directories(dstudio_run_loop_bench PRIVATE src)
    target_link_libraries(dstudio_run_loop_bench PRIVATE dstudio_engine_warnings)
  endif()
  add_executable(dstudio_offline_plugin_bench bench/bench_offline_plugins.cpp)
  target_include_directories(dstudio_offline_plugin_bench PRIVATE src)
  target_link_libraries(dstudio_offline_plugin_bench PRIVATE dstudio_engine dstudio_engine_warnings)
  target_compile_definitions(dstudio_offline_plugin_bench PRIVATE DSTUDIO_TEST_PLUGIN_DIR="${DSTUDIO_TEST_PLUGIN_DIR}")
  if(TARGET dstudio_test_plugins)
    add_dependencies(dstudio_offline_plugin_bench dstudio_test_plugins)
  endif()
  add_executable(dstudio_warp_bench bench/bench_warp.cpp)
  target_include_directories(dstudio_warp_bench PRIVATE src)
  target_link_libraries(dstudio_warp_bench PRIVATE dstudio_engine dstudio_engine_warnings)
  add_executable(dstudio_clip_loop_bench bench/bench_clip_loop.cpp)
  target_include_directories(dstudio_clip_loop_bench PRIVATE src)
  target_link_libraries(dstudio_clip_loop_bench PRIVATE dstudio_engine dstudio_engine_warnings)
  add_executable(dstudio_clip_fade_bench bench/bench_clip_fade.cpp)
  target_include_directories(dstudio_clip_fade_bench PRIVATE src)
  target_link_libraries(dstudio_clip_fade_bench PRIVATE dstudio_engine dstudio_engine_warnings)
  add_executable(dstudio_decode_bench bench/bench_decode.cpp)
  target_include_directories(dstudio_decode_bench PRIVATE src)
  target_link_libraries(dstudio_decode_bench PRIVATE dstudio_engine dstudio_engine_warnings)
  add_executable(dstudio_device_bench bench/bench_devices.cpp)
  target_include_directories(dstudio_device_bench PRIVATE src)
  target_link_libraries(dstudio_device_bench PRIVATE dstudio_engine dstudio_engine_warnings)
  # Plugin-cache parse benchmark (the scanner's JSON reader only).
  add_executable(dstudio_scan_cache_bench bench/bench_scan_cache.cpp)
  target_include_directories(dstudio_scan_cache_bench PRIVATE scanner)
  target_link_libraries(dstudio_scan_cache_bench PRIVATE dstudio_engine_warnings)
endif()

# ---------------------------------------------------------------- tools
if(DSTUDIO_ENGINE_BUILD_TOOLS)
  add_executable(play_sine_or_wav tools/play_sine_or_wav.cpp)
  target_include_directories(play_sine_or_wav PRIVATE src)
  target_link_libraries(play_sine_or_wav PRIVATE dstudio_engine dstudio_engine_warnings)
  add_executable(record_check tools/record_check.cpp)
  target_include_directories(record_check PRIVATE src)
  target_link_libraries(record_check PRIVATE dstudio_engine dstudio_engine_warnings)
  if(APPLE)
    # Manual: opens and closes an AU editor window (needs a GUI session).
    add_executable(plugin_editor_check tools/plugin_editor_check.mm)
    set_source_files_properties(tools/plugin_editor_check.mm PROPERTIES COMPILE_OPTIONS "-fobjc-arc;-Wno-old-style-cast")
    target_include_directories(plugin_editor_check PRIVATE src)
    target_link_libraries(plugin_editor_check PRIVATE dstudio_engine dstudio_engine_warnings)
  endif()
  if(TARGET PkgConfig::X11)
    # Manual: opens a real plugin's X11 editor (needs DISPLAY).
    add_executable(plugin_editor_check_linux tools/plugin_editor_check_linux.cpp)
    target_link_libraries(plugin_editor_check_linux PRIVATE dstudio_engine dstudio_engine_warnings)
  endif()
endif()
