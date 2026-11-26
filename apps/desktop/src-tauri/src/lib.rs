//! The termx desktop application: a thin Tauri shell around `termx-core`.
//!
//! This crate only wires the core library to the webview. Application logic
//! belongs in `termx-core`; the commands exposed to the frontend live in the
//! [`commands`] module, one file per feature area.

pub mod commands;

/// Builds and runs the Tauri application. Blocks until the app exits.
///
/// # Panics
///
/// Panics if the Tauri runtime fails to start.
pub fn run() {
    tauri::Builder::default()
        // Register every command exposed to the frontend here.
        .invoke_handler(tauri::generate_handler![commands::app::core_version])
        .run(tauri::generate_context!())
        .expect("failed to run the termx application");
}
