//! Commands about the workspace as a whole: subscribing to its changes,
//! the recent folders and the folder picker.

use std::path::PathBuf;

use tauri::ipc::Channel;
use tauri::{Runtime, State, Webview, WebviewWindow};
use tauri_plugin_dialog::DialogExt;
use termx_core::workspace::{Delta, Snapshot};

use super::error::{CommandResult, logged};
use crate::workspace::WorkspaceState;

/// Subscribes the window to the workspace. Returns everything needed to
/// draw it, and from then on sends what changes to `on_event`, coalesced.
/// See [`crate::workspace`].
///
/// A window that subscribes again (after a reload) replaces its previous
/// subscription.
///
/// # Errors
///
/// Fails when the changes cannot be watched.
#[tauri::command]
pub fn workspace_subscribe<R: Runtime>(
    webview: Webview<R>,
    state: State<'_, WorkspaceState>,
    on_event: Channel<Delta>,
) -> CommandResult<Snapshot> {
    logged("workspace_subscribe", || {
        state.subscribe(webview.label(), on_event)
    })
}

/// Returns the workspace as it is now, without subscribing.
///
/// # Errors
///
/// Only fails if reading the workspace panicked.
#[tauri::command]
pub fn workspace_snapshot(state: State<'_, WorkspaceState>) -> CommandResult<Snapshot> {
    logged("workspace_snapshot", || {
        Ok::<_, String>(state.workspace().snapshot())
    })
}

/// Asks for the Git branch of every session to be read again now, for
/// instance when the window gets the focus back. Returns at once; what
/// changed arrives through the subscription.
///
/// # Errors
///
/// Only fails if asking panicked.
#[tauri::command]
pub fn workspace_refresh_git(state: State<'_, WorkspaceState>) -> CommandResult<()> {
    logged("workspace_refresh_git", || {
        state.workspace().refresh_git();
        Ok::<_, String>(())
    })
}

/// Shows the folder picker of the system and returns the folder that was
/// chosen, or `null` when the dialog was dismissed.
///
/// The dialog is opened from here, not by the webview, which is given no
/// permission to open dialogs or to read the file system.
///
/// # Errors
///
/// Fails when the chosen folder has no path on this machine.
#[tauri::command(async)]
pub fn folder_pick<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, WorkspaceState>,
) -> CommandResult<Option<String>> {
    logged("folder_pick", || {
        let mut dialog = window
            .dialog()
            .file()
            .set_parent(&window)
            .set_title("Choose a folder for the session");
        // Start where the last session was created, else at home.
        let start = state
            .workspace()
            .recent_folders()
            .into_iter()
            .find(|folder| folder.is_dir())
            .or_else(|| state.default_folder().map(PathBuf::from));
        if let Some(start) = start {
            dialog = dialog.set_directory(start);
        }
        let Some(picked) = dialog.blocking_pick_folder() else {
            return Ok(None);
        };
        let folder = picked
            .into_path()
            .map_err(|error| format!("the chosen folder cannot be used: {error}"))?;
        Ok::<_, String>(Some(folder.to_string_lossy().into_owned()))
    })
}

/// Removes a folder from the recent folders. Returns whether it was one.
///
/// # Errors
///
/// Only fails if removing it panicked.
#[tauri::command]
pub fn recent_folder_forget(
    state: State<'_, WorkspaceState>,
    folder: PathBuf,
) -> CommandResult<bool> {
    logged("recent_folder_forget", || {
        Ok::<_, String>(state.workspace().forget_recent_folder(&folder))
    })
}
