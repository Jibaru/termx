//! The termx desktop application: a thin Tauri shell around `termx-core`.
//!
//! This crate only wires the core library to the webview. Application logic
//! belongs in `termx-core`; the commands exposed to the frontend live in the
//! [`commands`] module, one file per feature area.

use tauri::Manager;
use termx_core::diagnostics::tracing;

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
    // Logging comes first, before anything that can fail, so that a failure
    // to build the application or to create its window is recorded.
    let state = logging::start();
    let diagnostics = state.diagnostics().cloned();

    let built = tauri::Builder::default()
        .manage(state)
        // Register every command exposed to the frontend here.
        .invoke_handler(tauri::generate_handler![
            commands::app::core_version,
            commands::diagnostics::log_frontend_event,
            commands::diagnostics::open_log_folder,
            commands::diagnostics::export_diagnostics,
        ])
        .build(tauri::generate_context!());

    let app = match built {
        Ok(app) => app,
        Err(error) => {
            tracing::error!(error = %error, "the application could not be started");
            if let Some(diagnostics) = diagnostics {
                diagnostics.shutdown();
            }
            panic!("failed to build the termx application: {error}");
        }
    };

    app.run(|app, event| {
        if matches!(event, tauri::RunEvent::Exit)
            && let Some(state) = app.try_state::<logging::DiagnosticsState>()
        {
            logging::stop(&state);
        }
    });
}
