//! Quitting, and the menu of the application on macOS.
//!
//! # Quitting with sessions running
//!
//! Closing the window, or quitting, ends every session that runs. termx
//! asks first: the window is told (`quit-requested`, with how many run) and
//! shows a confirmation, whose answer is the `app_quit` command. Sessions
//! come back, not running, the next time termx starts. Without a running
//! session nothing is asked. If the window does not answer (its page is
//! broken), closing it a second time goes through: termx can always be
//! quit. Cancelling (`app_quit_cancel`) makes the next close ask again.
//!
//! # The menu
//!
//! On macOS the application has a menu bar of its own instead of the
//! default one, whose Close Window item binds Cmd+W to closing the window,
//! and with it every session. Here Cmd+W closes the focused pane, which
//! leaves its session running, and does nothing when there is one pane:
//! the one shortcut people press out of habit loses nothing. Closing a
//! session is Shift+Cmd+W, and asks when it runs. The Edit menu is the
//! standard one, which is what makes copy, paste and select all work in
//! the webview. The other items show the main actions with their
//! shortcuts; choosing one sends its command to the window
//! (`menu-command`), which runs it as if its keys had been pressed.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use termx_core::diagnostics::tracing::{info, warn};

use crate::workspace::WorkspaceState;

/// The event that asks the window to confirm quitting. Its payload is the
/// number of sessions that run.
pub const QUIT_REQUESTED_EVENT: &str = "quit-requested";

/// The event that carries the id of a menu item to the window.
pub const MENU_COMMAND_EVENT: &str = "menu-command";

/// The id of the Quit item, handled here rather than by the window.
const QUIT_ITEM: &str = "quit";

/// Remembers that the user confirmed quitting, so that the window closing
/// and the application exiting are not asked about again.
#[derive(Debug, Default)]
pub struct QuitState {
    confirmed: AtomicBool,
    /// A question was sent to the window and not answered yet.
    asked: Mutex<bool>,
}

impl QuitState {
    /// Whether to ask the window now: `false` when a question is still
    /// unanswered, which lets a second close or quit through.
    #[must_use]
    pub fn ask(&self) -> bool {
        let mut asked = self.asked.lock().unwrap_or_else(PoisonError::into_inner);
        !std::mem::replace(&mut *asked, true)
    }

    /// The user said no: the next close or quit asks again.
    pub fn cancel(&self) {
        *self.asked.lock().unwrap_or_else(PoisonError::into_inner) = false;
    }

    /// Records that the user chose to quit.
    pub fn confirm(&self) {
        self.confirmed.store(true, Ordering::SeqCst);
    }

    /// Whether the user chose to quit.
    #[must_use]
    pub fn is_confirmed(&self) -> bool {
        self.confirmed.load(Ordering::SeqCst)
    }
}

/// How many sessions of the workspace have a process.
#[must_use]
pub fn running_sessions<R: Runtime>(app: &AppHandle<R>) -> usize {
    app.try_state::<WorkspaceState>().map_or(0, |state| {
        state
            .workspace()
            .snapshot()
            .sessions
            .iter()
            .filter(|session| session.state.is_active())
            .count()
    })
}

/// Whether quitting now must be confirmed first: something runs and the
/// user did not say yes yet.
#[must_use]
pub fn needs_confirmation(running: usize, confirmed: bool) -> bool {
    running > 0 && !confirmed
}

/// Asks the window to confirm quitting, when sessions run. Returns whether
/// it did, in which case the caller must not quit (or close) now.
pub fn ask_before_quitting<R: Runtime>(app: &AppHandle<R>) -> bool {
    let Some(state) = app.try_state::<QuitState>() else {
        return false;
    };
    let running = running_sessions(app);
    if !needs_confirmation(running, state.is_confirmed()) {
        return false;
    }
    if !state.ask() {
        // Asked already, and closed again before any answer.
        info!(running, "quitting without an answer to the confirmation");
        return false;
    }
    if let Err(error) = app.emit(QUIT_REQUESTED_EVENT, running) {
        // Nobody can answer: quitting goes ahead, as it did before.
        warn!(error = %error, "could not ask the window to confirm quitting");
        return false;
    }
    info!(running, "asked to confirm quitting");
    true
}

/// Quits, once the user confirmed it (or nothing runs).
pub fn quit<R: Runtime>(app: &AppHandle<R>) {
    if let Some(state) = app.try_state::<QuitState>() {
        state.confirm();
    }
    app.exit(0);
}

/// The items of the menu that run a command of the window: id, label and
/// accelerator. The ids are those of the key bindings of the frontend
/// (`keybindings.ts`), whose shortcuts on macOS these are.
pub const MENU_COMMANDS: &[(&str, &str, &str)] = &[
    ("newSession", "New Session…", "Shift+Cmd+T"),
    (
        "newSiblingSession",
        "New Claude Session in This Project",
        "Shift+Cmd+N",
    ),
    ("splitRight", "Split Right", "Cmd+D"),
    ("splitDown", "Split Down", "Shift+Cmd+D"),
    ("closePane", "Close Pane", "Cmd+W"),
    ("closeSession", "Close Session…", "Shift+Cmd+W"),
    ("renameSession", "Rename Session", "F2"),
    ("toggleSidebar", "Show or Hide the Sidebar", "Cmd+B"),
    ("focusSidebar", "Go to the Sidebar", "Cmd+0"),
    ("nextSession", "Next Session", "Shift+Cmd+]"),
    ("previousSession", "Previous Session", "Shift+Cmd+["),
];

fn command_item<R: Runtime>(app: &AppHandle<R>, id: &str) -> tauri::Result<MenuItem<R>> {
    let (_, label, accelerator) = MENU_COMMANDS
        .iter()
        .find(|(candidate, _, _)| *candidate == id)
        .copied()
        .unwrap_or((id, id, ""));
    // F2 is shown, not bound: in a terminal it belongs to the program.
    let accelerator = (accelerator != "F2").then_some(accelerator);
    MenuItem::with_id(app, id, label, true, accelerator)
}

/// The menu bar of the application on macOS.
///
/// # Errors
///
/// Fails when the system refuses to make a menu.
pub fn build_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let item = |id| command_item(app, id);
    let separator = || PredefinedMenuItem::separator(app);
    let application = Submenu::with_items(
        app,
        "termx",
        true,
        &[
            &PredefinedMenuItem::about(app, None, None)?,
            &separator()?,
            &PredefinedMenuItem::services(app, None)?,
            &separator()?,
            &PredefinedMenuItem::hide(app, None)?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::show_all(app, None)?,
            &separator()?,
            &MenuItem::with_id(app, QUIT_ITEM, "Quit termx", true, Some("Cmd+Q"))?,
        ],
    )?;
    let file = Submenu::with_items(
        app,
        "File",
        true,
        &[
            &item("newSession")?,
            &item("newSiblingSession")?,
            &separator()?,
            &item("splitRight")?,
            &item("splitDown")?,
            &separator()?,
            &item("renameSession")?,
            &item("closePane")?,
            &item("closeSession")?,
        ],
    )?;
    let edit = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &separator()?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;
    let view = Submenu::with_items(
        app,
        "View",
        true,
        &[
            &item("toggleSidebar")?,
            &item("focusSidebar")?,
            &separator()?,
            &item("nextSession")?,
            &item("previousSession")?,
            &separator()?,
            &PredefinedMenuItem::fullscreen(app, None)?,
        ],
    )?;
    let window = Submenu::with_items(
        app,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::maximize(app, None)?,
        ],
    )?;
    Menu::with_items(app, &[&application, &file, &edit, &view, &window])
}

/// Handles the choice of a menu item.
pub fn on_menu_event<R: Runtime>(app: &AppHandle<R>, id: &str) {
    if id == QUIT_ITEM {
        if !ask_before_quitting(app) {
            quit(app);
        }
        return;
    }
    if MENU_COMMANDS.iter().any(|(command, _, _)| *command == id)
        && let Err(error) = app.emit(MENU_COMMAND_EVENT, id)
    {
        warn!(error = %error, "could not send a menu command to the window");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quitting_is_confirmed_only_when_something_runs() {
        assert!(!needs_confirmation(0, false));
        assert!(needs_confirmation(2, false));
        assert!(!needs_confirmation(2, true));
    }

    #[test]
    fn a_second_close_without_an_answer_goes_through() {
        let state = QuitState::default();
        assert!(state.ask());
        assert!(!state.ask(), "the question is still open");
        state.cancel();
        assert!(state.ask(), "after a no, the next close asks again");
    }

    #[test]
    fn a_confirmation_is_remembered() {
        let state = QuitState::default();
        assert!(!state.is_confirmed());
        state.confirm();
        assert!(state.is_confirmed());
    }

    #[test]
    fn cmd_w_closes_a_pane_and_never_a_window() {
        let close_pane = MENU_COMMANDS
            .iter()
            .find(|(id, _, _)| *id == "closePane")
            .unwrap();
        assert_eq!(close_pane.2, "Cmd+W");
        let accelerators: Vec<&str> = MENU_COMMANDS.iter().map(|(_, _, keys)| *keys).collect();
        let mut unique = accelerators.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            unique.len(),
            accelerators.len(),
            "an accelerator is used twice"
        );
    }
    // Building the menu itself is not tested: the system only makes menus
    // on the main thread, which a test does not run on.
}
