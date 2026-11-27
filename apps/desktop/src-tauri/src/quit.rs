//! Quitting, and the menu of the application on macOS.
//!
//! # Quitting with sessions running
//!
//! Closing the window, or quitting, ends every session that runs. termx
//! asks first: the window is told (`quit-requested`, with how many run) and
//! shows a confirmation, whose answer is the `app_quit` command. Sessions
//! come back, not running, the next time termx starts. Without a running
//! session nothing is asked.
//!
//! - **A repeat while the question is shown** (a second Cmd+Q, out of the
//!   habit of double-pressing it, or a second click on the close button)
//!   shows the question again, focused, and never quits: those keys are
//!   pressed by reflex, and would end every session.
//! - **A page that never shows the question** (it is broken: it did not
//!   say `app_quit_shown` within [`UNANSWERED_GRACE`]) lets a repeat
//!   through, so that termx can always be quit.
//! - **No, Escape, a reload** of the page (`app_quit_cancel`,
//!   [`QuitState::forget`]) make the next close ask again.
//! - **Logging out, restarting or shutting down** the Mac posts
//!   `NSWorkspaceWillPowerOffNotification` before asking applications to
//!   quit: termx then quits without asking, as if the user had said yes,
//!   so that it never holds up or cancels the logout. The sessions end and
//!   come back, dormant, at the next launch. On Windows and Linux the end
//!   of a user session closes termx without going through the question.
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
use std::time::{Duration, Instant};

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

/// How long the page has to show the question before a repeated close or
/// quit is let through (see the module documentation).
pub const UNANSWERED_GRACE: Duration = Duration::from_secs(3);

/// What to do with a request to close the window or quit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuitDecision {
    /// Close, or quit, now.
    Quit,
    /// Ask the window to confirm.
    Ask,
    /// The question is out already: show it again, and do nothing else.
    Remind,
}

/// Where the question stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Question {
    #[default]
    None,
    /// Sent to the window, which did not say it showed it yet.
    Sent(Instant),
    /// On screen.
    Shown,
}

/// Remembers the question about quitting and its answer.
#[derive(Debug, Default)]
pub struct QuitState {
    confirmed: AtomicBool,
    /// The Mac is logging out, restarting or shutting down.
    powering_off: AtomicBool,
    question: Mutex<Question>,
}

impl QuitState {
    fn question(&self) -> std::sync::MutexGuard<'_, Question> {
        self.question.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// What to do with a request to close or quit, made at `now` while
    /// `running` sessions run.
    pub fn decide(&self, running: usize, now: Instant) -> QuitDecision {
        if running == 0 || self.is_confirmed() || self.powering_off.load(Ordering::SeqCst) {
            return QuitDecision::Quit;
        }
        let mut question = self.question();
        match *question {
            Question::None => {
                *question = Question::Sent(now);
                QuitDecision::Ask
            }
            // The page never showed it: it cannot, and termx must still quit.
            Question::Sent(at) if now.saturating_duration_since(at) >= UNANSWERED_GRACE => {
                QuitDecision::Quit
            }
            Question::Sent(_) | Question::Shown => QuitDecision::Remind,
        }
    }

    /// The window shows the question.
    pub fn shown(&self) {
        let mut question = self.question();
        if matches!(*question, Question::Sent(_)) {
            *question = Question::Shown;
        }
    }

    /// The question is gone without a yes (no, Escape, a reload of the
    /// page): the next close or quit asks again.
    pub fn forget(&self) {
        *self.question() = Question::None;
    }

    /// The Mac is logging out, restarting or shutting down: quit without
    /// asking.
    pub fn power_off(&self) {
        self.powering_off.store(true, Ordering::SeqCst);
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

/// Asks the window to confirm quitting, when sessions run, or shows the
/// question again. Returns whether it did, in which case the caller must
/// not quit (or close) now.
pub fn ask_before_quitting<R: Runtime>(app: &AppHandle<R>) -> bool {
    let Some(state) = app.try_state::<QuitState>() else {
        return false;
    };
    let running = running_sessions(app);
    match state.decide(running, Instant::now()) {
        QuitDecision::Quit => false,
        decision => {
            if let Err(error) = app.emit(QUIT_REQUESTED_EVENT, running) {
                // Nobody can answer: quitting goes ahead, as it did before.
                warn!(error = %error, "could not ask the window to confirm quitting");
                state.forget();
                return false;
            }
            info!(
                running,
                again = decision == QuitDecision::Remind,
                "asked to confirm quitting"
            );
            true
        }
    }
}

/// Quits without asking when the Mac logs out, restarts or shuts down,
/// which it announces before asking applications to quit. Call it once, on
/// the main thread.
#[cfg(target_os = "macos")]
// The one call into AppKit that has no safe binding: see the SAFETY note.
#[allow(unsafe_code)]
pub fn watch_power_off<R: Runtime>(app: &AppHandle<R>) {
    use std::ptr::NonNull;

    use block2::RcBlock;
    use objc2_app_kit::{NSWorkspace, NSWorkspaceWillPowerOffNotification};
    use objc2_foundation::NSNotification;

    let app = app.clone();
    let block = RcBlock::new(move |_: NonNull<NSNotification>| {
        if let Some(state) = app.try_state::<QuitState>() {
            info!("the session of the user is ending: quitting without asking");
            state.power_off();
        }
    });
    let center = NSWorkspace::sharedWorkspace().notificationCenter();
    // SAFETY: the name is a constant of AppKit, and the block lives as long
    // as the observer, which lives as long as the application.
    let observer = unsafe {
        center.addObserverForName_object_queue_usingBlock(
            Some(NSWorkspaceWillPowerOffNotification),
            None,
            None,
            &block,
        )
    };
    std::mem::forget(observer);
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
    fn nothing_is_asked_when_nothing_runs() {
        let state = QuitState::default();
        assert_eq!(state.decide(0, Instant::now()), QuitDecision::Quit);
    }

    #[test]
    fn a_repeat_while_the_question_is_shown_shows_it_again_and_never_quits() {
        let state = QuitState::default();
        let start = Instant::now();
        assert_eq!(state.decide(2, start), QuitDecision::Ask);
        state.shown();

        // A double Cmd+Q, even long after.
        assert_eq!(
            state.decide(2, start + Duration::from_millis(100)),
            QuitDecision::Remind
        );
        assert_eq!(
            state.decide(2, start + Duration::from_secs(60)),
            QuitDecision::Remind
        );
    }

    #[test]
    fn a_page_that_never_shows_the_question_cannot_keep_termx_open() {
        let state = QuitState::default();
        let start = Instant::now();
        assert_eq!(state.decide(1, start), QuitDecision::Ask);

        assert_eq!(
            state.decide(1, start + Duration::from_secs(1)),
            QuitDecision::Remind
        );
        assert_eq!(
            state.decide(1, start + UNANSWERED_GRACE),
            QuitDecision::Quit
        );
    }

    #[test]
    fn a_no_or_a_reload_makes_the_next_close_ask_again() {
        let state = QuitState::default();
        let start = Instant::now();
        state.decide(1, start);
        state.shown();

        state.forget();

        assert_eq!(
            state.decide(1, start + Duration::from_secs(9)),
            QuitDecision::Ask
        );
    }

    #[test]
    fn a_yes_or_a_logout_quits_without_asking() {
        let confirmed = QuitState::default();
        confirmed.confirm();
        assert_eq!(confirmed.decide(3, Instant::now()), QuitDecision::Quit);

        let logging_out = QuitState::default();
        logging_out.decide(3, Instant::now());
        logging_out.shown();
        logging_out.power_off();
        assert_eq!(logging_out.decide(3, Instant::now()), QuitDecision::Quit);
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
