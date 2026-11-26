//! Commands changing the layout of the main area: the tree of panes.
//!
//! Every command validates what it is asked: an operation that would break
//! the layout is refused and changes nothing. The new layout reaches the
//! frontend through the subscription of
//! [`workspace_subscribe`](super::workspace::workspace_subscribe); a
//! frontend that wants to feel instant applies the change itself first.

use tauri::State;
use termx_core::workspace::{Direction, Edge, PaneId, Placement, SessionKey, SplitId};

use super::error::{CommandResult, logged};
use crate::workspace::WorkspaceState;

/// Splits a pane in two and returns the identifier of the new pane, which
/// takes the focus and shows `session` (or nothing).
///
/// `direction` is `"row"` for side by side or `"column"` for stacked.
/// `placement` is `"after"` (right or below, the default) or `"before"`.
///
/// # Errors
///
/// Fails when the pane or the session does not exist, or when the layout
/// has as many panes as it can.
#[tauri::command]
pub fn layout_split(
    state: State<'_, WorkspaceState>,
    pane: u32,
    direction: Direction,
    placement: Option<Placement>,
    session: Option<SessionKey>,
) -> CommandResult<u32> {
    logged("layout_split", || {
        state
            .workspace()
            .split_pane(
                PaneId::from_raw(pane),
                direction,
                placement.unwrap_or_default(),
                session,
            )
            .map(PaneId::as_u32)
    })
}

/// Closes a pane: its neighbour takes its place. The last pane is emptied
/// instead. The session it showed keeps running.
///
/// # Errors
///
/// Fails when the pane does not exist.
#[tauri::command]
pub fn layout_close_pane(state: State<'_, WorkspaceState>, pane: u32) -> CommandResult<()> {
    logged("layout_close_pane", || {
        state.workspace().close_pane(PaneId::from_raw(pane))
    })
}

/// Moves a pane to an edge of another (`"left"`, `"right"`, `"top"`,
/// `"bottom"`), or exchanges what the two show (`"center"`).
///
/// # Errors
///
/// Fails when either pane does not exist, or when they are the same.
#[tauri::command]
pub fn layout_move_pane(
    state: State<'_, WorkspaceState>,
    pane: u32,
    target: u32,
    edge: Edge,
) -> CommandResult<()> {
    logged("layout_move_pane", || {
        state
            .workspace()
            .move_pane(PaneId::from_raw(pane), PaneId::from_raw(target), edge)
    })
}

/// Sets the share of a split given to its first child, and returns the
/// ratio that was set: the value is kept within 0.1 and 0.9.
///
/// # Errors
///
/// Fails when the split does not exist or the ratio is not a number.
#[tauri::command]
pub fn layout_resize_split(
    state: State<'_, WorkspaceState>,
    split: u32,
    ratio: f64,
) -> CommandResult<f64> {
    logged("layout_resize_split", || {
        state
            .workspace()
            .resize_split(SplitId::from_raw(split), ratio)
    })
}

/// Gives the focus to a pane.
///
/// # Errors
///
/// Fails when the pane does not exist.
#[tauri::command]
pub fn layout_focus_pane(state: State<'_, WorkspaceState>, pane: u32) -> CommandResult<()> {
    logged("layout_focus_pane", || {
        state.workspace().focus_pane(PaneId::from_raw(pane))
    })
}

/// Shows a session in a pane, or empties the pane when `session` is
/// `null`. A session that was shown in another pane moves.
///
/// # Errors
///
/// Fails when the pane or the session does not exist.
#[tauri::command]
pub fn layout_show_session(
    state: State<'_, WorkspaceState>,
    pane: u32,
    session: Option<SessionKey>,
) -> CommandResult<()> {
    logged("layout_show_session", || {
        state
            .workspace()
            .show_session(PaneId::from_raw(pane), session)
    })
}
