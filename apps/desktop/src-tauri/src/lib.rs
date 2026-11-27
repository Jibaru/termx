//! The termx desktop application: a thin Tauri shell around `termx-core`.
//!
//! This crate only wires the core library to the webview. Application logic
//! belongs in `termx-core`; the commands exposed to the frontend live in the
//! [`commands`] module, one file per feature area.

use tauri::webview::PageLoadEvent;
use tauri::{Manager, WindowEvent};
use termx_core::diagnostics::tracing;

/// The commands exposed to the frontend. Register every command here.
///
/// A macro, so that the tests which call the commands as the webview does
/// (`ipc_tests`) register exactly the same list as the application.
macro_rules! command_handlers {
    () => {
        tauri::generate_handler![
            crate::commands::app::core_version,
            crate::commands::diagnostics::log_frontend_event,
            crate::commands::diagnostics::open_log_folder,
            crate::commands::diagnostics::export_diagnostics,
            crate::commands::workspace::workspace_subscribe,
            crate::commands::workspace::workspace_snapshot,
            crate::commands::workspace::workspace_refresh_git,
            crate::commands::workspace::folder_pick,
            crate::commands::workspace::recent_folder_forget,
            crate::commands::session::session_create,
            crate::commands::session::session_create_sibling,
            crate::commands::session::session_rename,
            crate::commands::session::session_reorder,
            crate::commands::session::project_reorder,
            crate::commands::session::session_start,
            crate::commands::session::session_close,
            crate::commands::session::session_stop,
            crate::commands::session::session_remove,
            crate::commands::session::session_report_title,
            crate::commands::session::session_report_cwd,
            crate::commands::session::session_write,
            crate::commands::session::session_resize,
            crate::commands::session::session_ack,
            crate::commands::layout::layout_split,
            crate::commands::layout::layout_close_pane,
            crate::commands::layout::layout_move_pane,
            crate::commands::layout::layout_resize_split,
            crate::commands::layout::layout_focus_pane,
            crate::commands::layout::layout_show_session,
        ]
    };
}

pub mod commands;
#[cfg(test)]
mod contract;
#[cfg(test)]
mod ipc_tests;
pub mod logging;
pub mod terminal;
#[cfg(test)]
mod test_support;
pub mod workspace;

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
        // Only used from Rust, for the folder picker: the webview is given
        // no permission to open dialogs itself.
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(state)
        .manage(terminal::TerminalState::start())
        .manage(workspace::WorkspaceState::start())
        .invoke_handler(command_handlers!())
        .setup(|app| {
            // Before the window shows anything, so that it does not jump.
            if let (Some(window), Some(state)) = (
                app.get_webview_window(MAIN_WINDOW),
                app.try_state::<workspace::WorkspaceState>(),
            ) {
                workspace::restore_window(&window, state.workspace());
            }
            Ok(())
        })
        // A page that loads has no terminal yet: terminals that are still
        // running for this window belong to a page that was reloaded, and
        // nothing can display them any more. Their sessions stay, dormant.
        .on_page_load(|webview, payload| {
            if payload.event() == PageLoadEvent::Started {
                release_window(webview.app_handle(), webview.label());
            }
        })
        .on_window_event(|window, event| match event {
            WindowEvent::Destroyed => release_window(window.app_handle(), window.label()),
            WindowEvent::Moved(_) | WindowEvent::Resized(_) if window.label() == MAIN_WINDOW => {
                if let Some(state) = window.try_state::<workspace::WorkspaceState>() {
                    workspace::remember_window(window, state.workspace());
                }
            }
            _ => {}
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
            // Then the workspace, which saves what is to be restored.
            if let Some(workspace) = app.try_state::<workspace::WorkspaceState>() {
                workspace.shutdown();
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

/// Label of the window of the application, as in `tauri.conf.json`.
const MAIN_WINDOW: &str = "main";

/// Ends what the window `label` was shown: its subscription to the
/// workspace and its terminals.
///
/// The terminals are taken away at once, so that those a reloaded page
/// starts next are not mistaken for the old ones, and their sessions become
/// dormant. Their processes are then killed without blocking the event
/// loop: one that resists is waited for on another thread. If the
/// application exits in the meantime, shutting the sessions down waits for
/// that thread to be done with its processes, and kills those it had not
/// reached yet.
fn release_window(app: &tauri::AppHandle, label: &str) {
    if let Some(workspace) = app.try_state::<workspace::WorkspaceState>() {
        workspace.unsubscribe(label);
    }
    let Some(terminal) = app.try_state::<terminal::TerminalState>() else {
        return;
    };
    let terminals = terminal.detach_owned_by(label);
    if terminals.is_empty() {
        return;
    }
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("termx-close-sessions".to_owned())
        .spawn(move || {
            if let Some(terminal) = app.try_state::<terminal::TerminalState>() {
                terminal.close_detached(&terminals);
            }
        });
    if let Err(error) = spawned {
        // They are killed when the application exits at the latest.
        tracing::warn!(error = %error, "could not start the thread that closes sessions");
    }
}
