//! Builds the C++ engine with CMake (Ninja) and links it statically. Also
//! builds the out-of-process plugin scanner and the test plugin bundles; their
//! locations are baked in as DSTUDIO_PLUGIN_SCANNER_BUILT / DSTUDIO_TEST_PLUGIN_DIR
//! (the scanner path can be overridden at runtime with $DSTUDIO_PLUGIN_SCANNER).
//!
//! The VST 3 SDK and CLAP headers are fetched by CMake at pinned tags into
//! `<target>/dstudio-third-party` (shared by all profiles; see
//! engine/third_party_licenses.md). Offline builds: set
//! FETCHCONTENT_SOURCE_DIR_DSTUDIO_CLAP / FETCHCONTENT_SOURCE_DIR_DSTUDIO_VST3SDK.

/// Configures and builds the engine with CMake, then emits the link lines for
/// the target OS: system frameworks on macOS, libstdc++ and libdl on Linux
/// (libpipewire and libasound, for audio and MIDI, and libX11, when a plugin
/// editor opens, are loaded at run time, so the app starts on systems without
/// PipeWire), and the COM, shell, MMCSS, user32 and winmm libraries on Windows.
fn main() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let engine = manifest.join("../../engine");
    for sub in ["include", "src", "scanner", "test_plugins", "CMakeLists.txt"] {
        println!("cargo:rerun-if-changed={}", engine.join(sub).display());
    }
    println!("cargo:rerun-if-env-changed=FETCHCONTENT_SOURCE_DIR_DSTUDIO_CLAP");
    println!("cargo:rerun-if-env-changed=FETCHCONTENT_SOURCE_DIR_DSTUDIO_VST3SDK");
    println!("cargo:rerun-if-env-changed=DSTUDIO_ENGINE_PIPEWIRE");
    println!("cargo:rerun-if-env-changed=DSTUDIO_ENGINE_ALSA");
    // Linux PipeWire backend: ON unless DSTUDIO_ENGINE_PIPEWIRE=OFF (then
    // audio goes through ALSA only).
    // Falsy as CMake reads it: OFF/0/FALSE/NO/N/IGNORE/NOTFOUND/"" or *-NOTFOUND.
    let cmake_on = |var: &str| {
        std::env::var(var)
            .map(|v| {
                let v = v.trim().to_ascii_uppercase();
                !(matches!(v.as_str(), "" | "0" | "OFF" | "FALSE" | "NO" | "N" | "IGNORE" | "NOTFOUND") || v.ends_with("-NOTFOUND"))
            })
            .unwrap_or(true)
    };
    let pipewire = cmake_on("DSTUDIO_ENGINE_PIPEWIRE");
    // Linux plugin editor windows: ON unless DSTUDIO_ENGINE_X11=OFF (then
    // opening a plugin editor reports that the build has no X11).
    println!("cargo:rerun-if-env-changed=DSTUDIO_ENGINE_X11");
    let x11 = cmake_on("DSTUDIO_ENGINE_X11");
    // Linux ALSA audio and MIDI input: ON unless DSTUDIO_ENGINE_ALSA=OFF.
    let alsa = cmake_on("DSTUDIO_ENGINE_ALSA");
    let profile = if std::env::var("PROFILE").as_deref() == Ok("release") { "Release" } else { "RelWithDebInfo" };
    let third_party = match std::env::var_os("CARGO_TARGET_DIR") {
        Some(dir) if std::path::Path::new(&dir).is_absolute() => std::path::PathBuf::from(dir),
        _ => manifest.join("../../target"),
    }
    .join("dstudio-third-party");
    let mut cfg = cmake::Config::new(&engine);
    cfg.generator("Ninja")
        .profile(profile)
        .define("DSTUDIO_ENGINE_BUILD_TESTS", "OFF")
        .define("DSTUDIO_ENGINE_BUILD_BENCH", "OFF")
        .define("DSTUDIO_ENGINE_BUILD_TOOLS", "OFF")
        .define("DSTUDIO_ENGINE_BUILD_SCANNER", "ON")
        .define("DSTUDIO_ENGINE_BUILD_TEST_PLUGINS", "ON")
        .define("FETCHCONTENT_BASE_DIR", third_party.to_string_lossy().as_ref())
        .define("DSTUDIO_ENGINE_PIPEWIRE", if pipewire { "ON" } else { "OFF" })
        .define("DSTUDIO_ENGINE_X11", if x11 { "ON" } else { "OFF" })
        .define("DSTUDIO_ENGINE_ALSA", if alsa { "ON" } else { "OFF" })
        .build_target("dstudio_engine_rust");
    for var in ["FETCHCONTENT_SOURCE_DIR_DSTUDIO_CLAP", "FETCHCONTENT_SOURCE_DIR_DSTUDIO_VST3SDK"] {
        if let Ok(v) = std::env::var(var) {
            cfg.define(var, v);
        }
    }
    let dst = cfg.build();
    let build = dst.join("build");
    // The static library lands in the CMake build tree.
    println!("cargo:rustc-link-search=native={}", build.display());
    println!("cargo:rustc-link-lib=static=dstudio_engine");
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();
    let exe_suffix = if target_os == "windows" { ".exe" } else { "" };
    println!("cargo:rustc-env=DSTUDIO_PLUGIN_SCANNER_BUILT={}", build.join(format!("dstudio_plugin_scanner{exe_suffix}")).display());
    println!("cargo:rustc-env=DSTUDIO_TEST_PLUGIN_DIR={}", build.join("test_plugins").display());
    match target_os.as_str() {
        "macos" => {
            println!("cargo:rustc-link-lib=c++");
            println!("cargo:rustc-link-lib=objc");
            for fw in [
                "CoreAudio",
                "AudioToolbox",
                "AudioUnit",
                "CoreAudioKit",
                "CoreFoundation",
                "Foundation",
                "AppKit",
                "Accelerate",
                "CoreMIDI",
            ] {
                println!("cargo:rustc-link-lib=framework={fw}");
            }
        }
        "linux" => {
            println!("cargo:rustc-link-lib=stdc++");
            println!("cargo:rustc-link-lib=dl");
        }
        "windows" => {
            // COM (WASAPI / MMDevice), CommandLineToArgvW, MMCSS, MIDI input.
            for lib in ["ole32", "shell32", "avrt", "user32", "winmm"] {
                println!("cargo:rustc-link-lib={lib}");
            }
        }
        _ => {}
    }
}
