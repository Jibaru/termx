//! The termx desktop application: a thin Tauri shell around `termx-core`.
//!
//! This crate only wires the core library to the webview. Application logic
//! belongs in `termx-core`; the commands exposed to the frontend live in the
//! [`commands`] module, one file per feature area.

use tauri::webview::PageLoadEvent;
use tauri::{Manager, WindowEvent};
use termx_core::diagnostics::tracing;

pub mod commands;
pub mod logging;
pub mod terminal;
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

    raise_open_file_limit();

    let built = tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_opener::init())
        .manage(state)
        .manage(terminal::TerminalState::start())
        // Register every command exposed to the frontend here.
        .invoke_handler(tauri::generate_handler![
            commands::app::core_version,
            commands::diagnostics::log_frontend_event,
            commands::diagnostics::open_log_folder,
            commands::diagnostics::export_diagnostics,
            commands::session::session_create,
            commands::session::session_write,
            commands::session::session_resize,
            commands::session::session_ack,
            commands::session::session_close,
        ])
        // A page that loads has no terminal yet: sessions that are still
        // running for this window belong to a page that was reloaded, and
        // nothing can display them any more.
        .on_page_load(|webview, payload| {
            if payload.event() == PageLoadEvent::Started {
                close_sessions_of(webview.app_handle(), webview.label());
            }
        })
        .on_window_event(|window, event| {
            if matches!(event, WindowEvent::Destroyed) {
                close_sessions_of(window.app_handle(), window.label());
            }
        })
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
        if matches!(event, tauri::RunEvent::Exit) {
            // Sessions first: no shell may outlive the application, and
            // killing them is worth a record in the log.
            if let Some(terminal) = app.try_state::<terminal::TerminalState>() {
                terminal.shutdown();
            }
            if let Some(state) = app.try_state::<logging::DiagnosticsState>() {
                logging::stop(&state);
            }
        }
    });
}

/// Raises the limit on open files, which caps the number of terminals: each
/// one needs several descriptors, and macOS only allows 256 by default.
fn raise_open_file_limit() {
    match termx_core::pty::raise_open_file_limit() {
        Ok(Some(limit)) => tracing::info!(limit, "open file limit"),
        // Windows has no such limit.
        Ok(None) => {}
        Err(error) => {
            tracing::warn!(error = %error, "could not raise the open file limit");
        }
    }
}

/// Closes the sessions of the window `label` without blocking the event
/// loop: a process that resists being killed is waited for on another
/// thread. Sessions still running when the application exits are killed
/// there, whatever this thread got done.
fn close_sessions_of(app: &tauri::AppHandle, label: &str) {
    let app = app.clone();
    let label = label.to_owned();
    let spawned = std::thread::Builder::new()
        .name("termx-close-sessions".to_owned())
        .spawn(move || {
            if let Some(terminal) = app.try_state::<terminal::TerminalState>() {
                terminal.close_owned_by(&label);
            }
        });
    if let Err(error) = spawned {
        tracing::warn!(error = %error, "could not start the thread that closes sessions");
    }
}
