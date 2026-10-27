//! Windows only: embed the app icon, manifest and version info into `xmlcraft.exe`, so it shows in Explorer, the taskbar, the Start
//! menu and Alt-Tab, and is per-monitor DPI aware.
//!
//! On every other target this does nothing. A missing resource compiler is a warning, so a cross-compile from macOS or Linux
//! still links, unless `XMLCRAFT_REQUIRE_WINRES=1` turns it into an error (for release builds).

#[path = "src/windows_manifest.rs"]
mod windows_manifest;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/app-icon/xmlcraft.ico");
    println!("cargo:rerun-if-changed=src/windows_manifest.rs");
    println!("cargo:rerun-if-env-changed=XMLCRAFT_REQUIRE_WINRES");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("../../assets/app-icon/xmlcraft.ico")
        .set_manifest(windows_manifest::WINDOWS_MANIFEST)
        .set("ProductName", "XmlCraft")
        .set("FileDescription", "XmlCraft XML editor")
        .set("LegalCopyright", "Copyright (c) the XmlCraft contributors. MIT OR Apache-2.0.")
        .set("OriginalFilename", "xmlcraft.exe")
        .set("InternalName", "xmlcraft");
    if let Err(e) = res.compile() {
        if std::env::var_os("XMLCRAFT_REQUIRE_WINRES").is_some() {
            println!("cargo::error=embedding Windows resources failed: {e}");
            return;
        }
        println!("cargo:warning=xmlcraft.exe built without icon/version resources: {e}");
    }
}
