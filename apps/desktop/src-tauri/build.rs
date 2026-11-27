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
//!
//! It also decides whether the `e2e` feature may compile: see
//! [`allow_e2e_only_where_asked`].

use std::path::PathBuf;

/// Set to `1` by the builds made for the end-to-end suite (`pnpm e2e:build`
/// and the `End-to-end` workflow), and by nothing else.
const ALLOW_E2E_ENV_VAR: &str = "TERMX_ALLOW_E2E";

fn main() {
    allow_e2e_only_where_asked();

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

/// The `e2e` feature embeds a `WebDriver` server, so `lib.rs` refuses to
/// compile it unless this sets `termx_e2e_allowed`: only for a build that
/// asked for it with [`ALLOW_E2E_ENV_VAR`], and never with a profile that
/// inherits from `release`, whatever its debug assertions.
fn allow_e2e_only_where_asked() {
    println!("cargo::rustc-check-cfg=cfg(termx_e2e_allowed)");
    println!("cargo::rerun-if-env-changed={ALLOW_E2E_ENV_VAR}");
    let asked = std::env::var(ALLOW_E2E_ENV_VAR).is_ok_and(|value| value == "1");
    // `PROFILE` is `release` for every profile that inherits from it.
    let release = std::env::var("PROFILE").is_ok_and(|profile| profile == "release");
    if asked && !release {
        println!("cargo::rustc-cfg=termx_e2e_allowed");
    }
}
