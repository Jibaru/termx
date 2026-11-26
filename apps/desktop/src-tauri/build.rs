//! Build script of the desktop application.
//!
//! Besides what every Tauri application needs, it makes sure that **every**
//! executable of this crate carries the application manifest on Windows,
//! the test binaries included.
//!
//! The manifest asks for version 6 of the common controls, which the
//! system dialogs need. `tauri-build` embeds it, but only in the
//! application itself: a test binary linked with the same libraries then
//! fails to start at all (`STATUS_ENTRYPOINT_NOT_FOUND`), before a single
//! test runs. So the default manifest is turned off and the same one,
//! `windows-app-manifest.xml`, is handed to the linker for all targets.

use std::path::PathBuf;

fn main() {
    // Of the target, not of the machine that builds.
    let windows = std::env::var_os("CARGO_CFG_WINDOWS").is_some();
    let msvc = std::env::var("CARGO_CFG_TARGET_ENV").is_ok_and(|env| env == "msvc");
    if !(windows && msvc) {
        tauri_build::build();
        return;
    }

    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default())
        .join("windows-app-manifest.xml");
    println!("cargo:rerun-if-changed={}", manifest.display());
    println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());

    let attributes = tauri_build::Attributes::new()
        .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest());
    if let Err(error) = tauri_build::try_build(attributes) {
        panic!("tauri-build failed: {error:#}");
    }
}
