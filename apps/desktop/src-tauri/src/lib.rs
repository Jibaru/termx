//! The termx desktop application: a thin Tauri shell around `termx-core`.
//!
//! This crate only wires the core library to the webview. Application logic
//! belongs in `termx-core`; the commands exposed to the frontend live in the
//! [`commands`] module, one file per feature area.

pub mod commands;
pub mod logging;
#[cfg(test)]
mod test_support;

/// Builds and runs the Tauri application. Blocks until the app exits.
///
/// # Panics
///
/// Panics if the Tauri runtime fails to start.
pub fn run() {
    let app = tauri::Builder::default()
        .manage(logging::DiagnosticsState::new())
        .setup(|app| {
            // Logging comes first, so that everything after it is recorded.
            logging::start(app.handle());
            Ok(())
        })
        // Register every command exposed to the frontend here.
        .invoke_handler(tauri::generate_handler![
            commands::app::core_version,
            commands::diagnostics::log_frontend_event,
            commands::diagnostics::open_log_folder,
            commands::diagnostics::export_diagnostics,
        ])
        .build(tauri::generate_context!())
        .expect("failed to build the termx application");

    app.run(|app, event| {
        if matches!(event, tauri::RunEvent::Exit) {
            logging::stop(app);
        }
    });
}
