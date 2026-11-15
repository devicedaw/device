fn main() {
    slint_build::compile("ui/app.slint").expect("slint build failed");
    windows_icon();
}

/// device-studio.exe carries the app icon (icons/device.ico, made by tools/art/make-icons.mjs),
/// so Explorer, the taskbar and shortcuts show it.
#[cfg(windows)]
fn windows_icon() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let ico = std::path::Path::new(&dir).join("icons").join("device.ico");
    println!("cargo:rerun-if-changed={}", ico.display());
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let rc = out.join("device.rc");
    // rc.exe takes forward slashes, which need no escaping in its string literals.
    std::fs::write(&rc, format!("1 ICON \"{}\"\n", ico.display().to_string().replace('\\', "/"))).expect("write device.rc");
    embed_resource::compile(&rc, embed_resource::NONE).manifest_optional().expect("compile the icon resource");
}

#[cfg(not(windows))]
fn windows_icon() {}
