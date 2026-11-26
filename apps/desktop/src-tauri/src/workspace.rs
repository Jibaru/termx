//! Wires the workspace of `termx-core` into the application: where its
//! state file lives on each platform, how its changes reach the webview,
//! and how the geometry of the window is remembered.
//!
//! [`WorkspaceState`] is the application state behind the workspace
//! commands (`commands::workspace`, `commands::session`,
//! `commands::layout`).
//!
//! # What the frontend receives
//!
//! A window subscribes once, with a [`Channel`]. It gets a
//! [`Snapshot`] in answer and, from then on, a [`Delta`] on the channel for
//! every change: the sessions that changed, as they are now, and the parts
//! of the workspace that changed. Changes that come within
//! [`EVENT_COALESCE`] of each other are sent as one delta. The frontend
//! never polls.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tauri::ipc::Channel;
use termx_core::diagnostics::tracing::{debug, info, warn};
use termx_core::workspace::{
    Delta, Snapshot, Subscription, WindowGeometry, Workspace, WorkspaceConfig,
};

/// Environment variable that overrides the directory of the state file.
/// Useful for development and for tests that launch the real application,
/// which must not touch the state of the person running them.
pub const STATE_DIR_ENV_VAR: &str = "TERMX_STATE_DIR";

/// Name of the application directory inside the platform locations.
const APP_DIR: &str = "termx";

/// Changes of the workspace that come within this time of each other are
/// sent to the webview as one message. Short enough not to be seen, long
/// enough to turn a burst (a divider being dragged, sessions restored)
/// into one update.
pub const EVENT_COALESCE: Duration = Duration::from_millis(30);

/// Returns the directory of the state file for the operating system `os`
/// (a value of [`std::env::consts::OS`]).
///
/// | Platform | Directory                                          |
/// | -------- | -------------------------------------------------- |
/// | macOS    | `~/Library/Application Support/termx`              |
/// | Windows  | `%LOCALAPPDATA%\termx`                             |
/// | Linux    | `$XDG_STATE_HOME/termx`, or `~/.local/state/termx` |
///
/// The state names folders of this machine, so it is kept in the locations
/// that are not synchronised to other machines.
///
/// Returns `None` when the base directory it needs is unknown.
#[must_use]
pub fn state_dir_for(
    os: &str,
    home: Option<&Path>,
    local_data: Option<&Path>,
    xdg_state_home: Option<&Path>,
) -> Option<PathBuf> {
    match os {
        "macos" => Some(
            home?
                .join("Library")
                .join("Application Support")
                .join(APP_DIR),
        ),
        "windows" => Some(local_data?.join(APP_DIR)),
        _ => {
            // The XDG specification says to ignore relative paths.
            let state = match xdg_state_home.filter(|path| path.is_absolute()) {
                Some(state) => state.to_owned(),
                None => home?.join(".local").join("state"),
            };
            Some(state.join(APP_DIR))
        }
    }
}

/// Resolves the directory of the state file from the environment of the
/// process. [`STATE_DIR_ENV_VAR`] wins over the platform location.
#[must_use]
pub fn state_dir_from_env(
    os: &str,
    variable: impl Fn(&str) -> Option<OsString>,
    home: Option<&Path>,
) -> Option<PathBuf> {
    let directory = |name: &str| {
        variable(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    if let Some(dir) = directory(STATE_DIR_ENV_VAR) {
        return Some(dir);
    }
    // What `%LOCALAPPDATA%` is on a default Windows installation.
    let local_data =
        directory("LOCALAPPDATA").or_else(|| Some(home?.join("AppData").join("Local")));
    let xdg_state_home = directory("XDG_STATE_HOME");
    state_dir_for(os, home, local_data.as_deref(), xdg_state_home.as_deref())
}

/// Application state for the workspace, managed by Tauri.
pub struct WorkspaceState {
    workspace: Arc<Workspace>,
    /// Where a session starts when no folder is given.
    home: Option<PathBuf>,
    /// The subscription of each window, by label.
    subscriptions: Mutex<HashMap<String, Subscription>>,
}

impl WorkspaceState {
    /// Opens the workspace of the user. This never fails: without a
    /// directory for its state, or with a state file that cannot be used,
    /// the workspace works and says in its snapshot that it is not saved.
    #[must_use]
    pub fn start() -> Self {
        let home = std::env::home_dir();
        let dir = state_dir_from_env(
            std::env::consts::OS,
            |name| std::env::var_os(name),
            home.as_deref(),
        );
        let config = match dir {
            Some(dir) => WorkspaceConfig::new(dir),
            None => {
                warn!("no directory for the state could be determined; nothing will be saved");
                WorkspaceConfig::in_memory()
            }
        };
        Self::with_workspace(Workspace::open(config), home)
    }

    /// State around a workspace that is already open.
    #[must_use]
    pub fn with_workspace(workspace: Workspace, home: Option<PathBuf>) -> Self {
        Self {
            workspace: Arc::new(workspace),
            home,
            subscriptions: Mutex::new(HashMap::new()),
        }
    }

    /// The workspace.
    #[must_use]
    pub fn workspace(&self) -> &Arc<Workspace> {
        &self.workspace
    }

    /// The folder a session starts in when none is given: the home
    /// directory of the user.
    #[must_use]
    pub fn default_folder(&self) -> Option<&Path> {
        self.home.as_deref()
    }

    /// Subscribes the window `label` to the changes of the workspace and
    /// returns the state to start from. Every change made after that
    /// state was read is sent to `channel`; some made just before may be
    /// sent as well, which is harmless, since a delta carries current
    /// values.
    ///
    /// A window has one subscription: an earlier one is ended.
    ///
    /// # Errors
    ///
    /// Fails when the thread that forwards the changes cannot be started.
    pub fn subscribe(&self, label: &str, channel: Channel<Delta>) -> Result<Snapshot, String> {
        // Subscribed before the snapshot is read, so that no change falls
        // between the two.
        let subscription = self.workspace.subscribe();
        let snapshot = self.workspace.snapshot();

        let forwarder = std::thread::Builder::new()
            .name("termx-workspace-events".to_owned())
            .spawn({
                let subscription = subscription.clone();
                let workspace = Arc::clone(&self.workspace);
                move || forward(&workspace, &subscription, &channel)
            });
        if let Err(error) = forwarder {
            subscription.close();
            return Err(format!("the workspace cannot be watched: {error}"));
        }

        let previous = self
            .subscriptions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(label.to_owned(), subscription);
        if let Some(previous) = previous {
            previous.close();
        }
        Ok(snapshot)
    }

    /// Ends the subscription of the window `label`, if it has one. Used
    /// when a window goes away or reloads its page.
    pub fn unsubscribe(&self, label: &str) {
        let subscription = self
            .subscriptions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(label);
        if let Some(subscription) = subscription {
            subscription.close();
        }
    }

    /// Saves the workspace and ends every subscription. Called when the
    /// application exits.
    pub fn shutdown(&self) {
        self.workspace.shutdown();
        self.subscriptions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }
}

/// Sends the changes of a subscription to a webview until either ends.
fn forward(workspace: &Workspace, subscription: &Subscription, channel: &Channel<Delta>) {
    while let Some(changes) = subscription.recv_coalesced(EVENT_COALESCE) {
        let delta = workspace.delta(&changes);
        if let Err(error) = channel.send(delta) {
            // The webview is gone or was reloaded: nobody is listening.
            debug!(error = %error, "stopped sending workspace changes to a window");
            subscription.close();
            break;
        }
    }
}

/// The geometry of a window, as it is remembered: in logical pixels, so
/// that it means the same on a display of another density.
///
/// `previous` is what was remembered before. A window that is maximized,
/// minimized or full screen keeps the position and size it had before, so
/// that leaving that state later gives a sensible window.
#[must_use]
pub fn geometry_of(
    position: (i32, i32),
    size: (u32, u32),
    scale_factor: f64,
    maximized: bool,
    keep_previous: bool,
    previous: Option<WindowGeometry>,
) -> Option<WindowGeometry> {
    if keep_previous {
        return previous.map(|previous| WindowGeometry {
            maximized,
            ..previous
        });
    }
    if !scale_factor.is_finite() || scale_factor <= 0.0 {
        return None;
    }
    let geometry = WindowGeometry {
        x: f64::from(position.0) / scale_factor,
        y: f64::from(position.1) / scale_factor,
        width: f64::from(size.0) / scale_factor,
        height: f64::from(size.1) / scale_factor,
        maximized,
    };
    geometry.is_valid().then_some(geometry)
}

/// The area of a display, in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DisplayArea {
    /// Horizontal position of the top-left corner.
    pub x: f64,
    /// Vertical position of the top-left corner.
    pub y: f64,
    /// Width.
    pub width: f64,
    /// Height.
    pub height: f64,
}

/// Whether a window with this geometry would be reachable on one of the
/// displays: enough of its title bar must be on screen to grab it. A
/// position remembered from a display that is no longer connected is not
/// restored.
#[must_use]
pub fn is_reachable(geometry: &WindowGeometry, displays: &[DisplayArea]) -> bool {
    /// How much of the top edge must be visible, in logical pixels.
    const GRIP: f64 = 80.0;

    displays.iter().any(|display| {
        let left = geometry.x.max(display.x);
        let right = (geometry.x + geometry.width).min(display.x + display.width);
        let top_visible =
            geometry.y >= display.y && geometry.y <= display.y + display.height - GRIP;
        right - left >= GRIP && top_visible
    })
}

/// Gives the window the size, and when it is still reachable the position,
/// it had when termx last ran. Failures are logged and otherwise ignored:
/// the window then keeps its default geometry.
pub fn restore_window(window: &tauri::WebviewWindow, workspace: &Workspace) {
    let Some(geometry) = workspace.window_geometry().filter(WindowGeometry::is_valid) else {
        return;
    };
    let displays: Vec<DisplayArea> = window
        .available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|monitor| {
            let scale = monitor.scale_factor();
            DisplayArea {
                x: f64::from(monitor.position().x) / scale,
                y: f64::from(monitor.position().y) / scale,
                width: f64::from(monitor.size().width) / scale,
                height: f64::from(monitor.size().height) / scale,
            }
        })
        .collect();

    let size = tauri::LogicalSize::new(geometry.width, geometry.height);
    if let Err(error) = window.set_size(size) {
        warn!(error = %error, "the size of the window could not be restored");
    }
    if is_reachable(&geometry, &displays) {
        let position = tauri::LogicalPosition::new(geometry.x, geometry.y);
        if let Err(error) = window.set_position(position) {
            warn!(error = %error, "the position of the window could not be restored");
        }
    }
    if geometry.maximized
        && let Err(error) = window.maximize()
    {
        warn!(error = %error, "the window could not be maximized");
    }
    info!(maximized = geometry.maximized, "window geometry restored");
}

/// Remembers the geometry of the window after it was moved or resized.
/// Saving it is left to the workspace, which waits for the changes to
/// stop.
pub fn remember_window(window: &tauri::Window, workspace: &Workspace) {
    let (Ok(position), Ok(size), Ok(scale_factor)) = (
        window.outer_position(),
        window.inner_size(),
        window.scale_factor(),
    ) else {
        return;
    };
    let maximized = window.is_maximized().unwrap_or(false);
    let keep_previous = maximized
        || window.is_minimized().unwrap_or(false)
        || window.is_fullscreen().unwrap_or(false);
    let geometry = geometry_of(
        (position.x, position.y),
        (size.width, size.height),
        scale_factor,
        maximized,
        keep_previous,
        workspace.window_geometry(),
    );
    if let Some(geometry) = geometry {
        workspace.set_window_geometry(geometry);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{self, Receiver};

    use tauri::ipc::InvokeResponseBody;
    use termx_core::workspace::{NewSession, SessionKind};

    use super::*;

    const TIMEOUT: Duration = Duration::from_secs(30);

    fn home() -> PathBuf {
        Path::new("home").join("ana")
    }

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(value))
        }
    }

    #[test]
    fn the_state_lives_in_the_local_location_of_each_platform() {
        let local = home().join("AppData").join("Local");
        let absolute = std::env::temp_dir().join("xdg-state");

        assert_eq!(
            state_dir_for("macos", Some(&home()), None, None).unwrap(),
            home()
                .join("Library")
                .join("Application Support")
                .join("termx")
        );
        assert_eq!(
            state_dir_for("windows", Some(&home()), Some(&local), None).unwrap(),
            local.join("termx")
        );
        assert_eq!(
            state_dir_for("linux", Some(&home()), None, Some(&absolute)).unwrap(),
            absolute.join("termx")
        );
        assert_eq!(
            state_dir_for("linux", Some(&home()), None, Some(Path::new("relative"))).unwrap(),
            home().join(".local").join("state").join("termx")
        );
        assert_eq!(state_dir_for("macos", None, None, None), None);
        assert_eq!(state_dir_for("windows", Some(&home()), None, None), None);
        assert_eq!(state_dir_for("linux", None, None, None), None);
    }

    #[test]
    fn the_state_directory_is_resolved_from_the_environment() {
        let dir = state_dir_from_env("windows", env(&[]), Some(&home())).unwrap();
        assert_eq!(dir, home().join("AppData").join("Local").join("termx"));

        let custom = std::env::temp_dir().join("elsewhere");
        let custom_text = custom.to_string_lossy().into_owned();
        let dir = state_dir_from_env(
            "macos",
            env(&[(STATE_DIR_ENV_VAR, &custom_text)]),
            Some(&home()),
        )
        .unwrap();
        assert_eq!(dir, custom);

        // An empty override is ignored.
        let dir =
            state_dir_from_env("macos", env(&[(STATE_DIR_ENV_VAR, "")]), Some(&home())).unwrap();
        assert!(dir.ends_with(Path::new("Application Support").join("termx")));

        assert_eq!(state_dir_from_env("linux", env(&[]), None), None);
    }

    /// A channel whose messages arrive in the returned receiver instead of
    /// in a webview.
    fn channel() -> (Channel<Delta>, Receiver<serde_json::Value>) {
        let (sender, receiver) = mpsc::channel();
        let channel = Channel::new(move |body| {
            let InvokeResponseBody::Json(json) = body else {
                panic!("a delta was not sent as JSON");
            };
            let _ = sender.send(serde_json::from_str(&json).expect("invalid JSON"));
            Ok(())
        });
        (channel, receiver)
    }

    fn state() -> WorkspaceState {
        WorkspaceState::with_workspace(Workspace::open(WorkspaceConfig::in_memory()), None)
    }

    #[test]
    fn a_window_gets_a_snapshot_and_then_the_changes() {
        let dir = tempfile::tempdir().unwrap();
        let state = state();
        let existing = state
            .workspace()
            .create_session(NewSession::new(SessionKind::Shell, dir.path()))
            .unwrap();
        let (channel, deltas) = channel();

        let snapshot = state.subscribe("main", channel).unwrap();

        assert_eq!(snapshot.sessions, std::slice::from_ref(&existing));
        let created = state
            .workspace()
            .create_session(NewSession::new(SessionKind::Claude, dir.path()))
            .unwrap();
        let delta = deltas.recv_timeout(TIMEOUT).unwrap();
        assert!(delta["revision"].as_u64().unwrap() > snapshot.revision);
        assert_eq!(delta["sessions"].as_array().unwrap().len(), 1);
        assert_eq!(delta["sessions"][0]["id"], created.id.as_str());
        assert_eq!(delta["sessions"][0]["kind"], "claude");
        assert_eq!(delta["sessions"][0]["state"]["status"], "starting");
        assert_eq!(
            delta["sessionOrder"],
            serde_json::json!([existing.id.as_str(), created.id.as_str()])
        );
        assert_eq!(delta["removed"], serde_json::json!([]));
        assert!(delta["layout"].is_object());
        assert!(delta["recentFolders"].is_array());
    }

    #[test]
    fn a_burst_of_changes_is_sent_as_few_messages() {
        let dir = tempfile::tempdir().unwrap();
        let state = state();
        let session = state
            .workspace()
            .create_session(NewSession::new(SessionKind::Shell, dir.path()))
            .unwrap();
        let (channel, deltas) = channel();
        state.subscribe("main", channel).unwrap();

        for n in 0..500 {
            state
                .workspace()
                .rename_session(&session.id, Some(&format!("name {n}")))
                .unwrap();
        }

        let mut messages = 0;
        let mut last = serde_json::Value::Null;
        while let Ok(delta) = deltas.recv_timeout(Duration::from_millis(500)) {
            messages += 1;
            last = delta;
        }
        assert!(
            (1..=20).contains(&messages),
            "500 changes were sent as {messages} messages"
        );
        // Whatever was coalesced, the last message has the last value.
        assert_eq!(last["sessions"][0]["title"], "name 499");
    }

    #[test]
    fn a_window_that_subscribes_again_replaces_its_subscription() {
        let dir = tempfile::tempdir().unwrap();
        let state = state();
        let (first, first_deltas) = channel();
        let (second, second_deltas) = channel();
        state.subscribe("main", first).unwrap();

        state.subscribe("main", second).unwrap();
        state
            .workspace()
            .create_session(NewSession::new(SessionKind::Shell, dir.path()))
            .unwrap();

        second_deltas.recv_timeout(TIMEOUT).unwrap();
        // The first channel hears nothing more; its sender is dropped once
        // its thread has ended.
        assert!(first_deltas.recv_timeout(TIMEOUT).is_err());
    }

    #[test]
    fn unsubscribing_and_shutting_down_stop_the_messages() {
        let dir = tempfile::tempdir().unwrap();
        let state = state();
        let (main, main_deltas) = channel();
        let (other, other_deltas) = channel();
        state.subscribe("main", main).unwrap();
        state.subscribe("other", other).unwrap();

        state.unsubscribe("main");
        state.unsubscribe("no such window");
        state
            .workspace()
            .create_session(NewSession::new(SessionKind::Shell, dir.path()))
            .unwrap();

        other_deltas.recv_timeout(TIMEOUT).unwrap();
        assert!(main_deltas.recv_timeout(TIMEOUT).is_err());

        state.shutdown();
        assert!(other_deltas.recv_timeout(TIMEOUT).is_err());
    }

    #[test]
    fn a_window_whose_channel_fails_is_no_longer_sent_anything() {
        let dir = tempfile::tempdir().unwrap();
        let state = state();
        let (sender, sent) = mpsc::channel();
        let failing = Channel::new(move |_| {
            let _ = sender.send(());
            Err(tauri::Error::WebviewNotFound)
        });
        state.subscribe("main", failing).unwrap();

        for _ in 0..3 {
            state
                .workspace()
                .create_session(NewSession::new(SessionKind::Shell, dir.path()))
                .unwrap();
            std::thread::sleep(EVENT_COALESCE * 3);
        }

        sent.recv_timeout(TIMEOUT).unwrap();
        assert!(sent.recv_timeout(Duration::from_millis(300)).is_err());
    }

    #[test]
    fn the_geometry_is_remembered_in_logical_pixels() {
        let geometry = geometry_of((200, 100), (2400, 1600), 2.0, false, false, None).unwrap();

        assert_eq!(
            (geometry.x, geometry.y, geometry.width, geometry.height),
            (100.0, 50.0, 1200.0, 800.0)
        );
        assert!(!geometry.maximized);
        assert_eq!(geometry_of((0, 0), (0, 600), 1.0, false, false, None), None);
        assert_eq!(
            geometry_of((0, 0), (800, 600), 0.0, false, false, None),
            None
        );
        assert_eq!(
            geometry_of((0, 0), (800, 600), f64::NAN, false, false, None),
            None
        );
    }

    #[test]
    fn a_maximized_window_keeps_the_geometry_it_had_before() {
        let before = geometry_of((200, 100), (1200, 800), 1.0, false, false, None);

        let maximized = geometry_of((0, 0), (2560, 1440), 1.0, true, true, before).unwrap();

        assert!(maximized.maximized);
        assert_eq!(
            (maximized.x, maximized.y, maximized.width, maximized.height),
            (200.0, 100.0, 1200.0, 800.0)
        );
        // Nothing to keep: nothing to remember.
        assert_eq!(
            geometry_of((0, 0), (2560, 1440), 1.0, true, true, None),
            None
        );
    }

    #[test]
    fn a_position_is_only_restored_where_the_window_can_be_reached() {
        let laptop = DisplayArea {
            x: 0.0,
            y: 0.0,
            width: 1440.0,
            height: 900.0,
        };
        let left = DisplayArea {
            x: -1920.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
        };
        let at = |x: f64, y: f64| WindowGeometry {
            x,
            y,
            width: 1200.0,
            height: 800.0,
            maximized: false,
        };

        assert!(is_reachable(&at(100.0, 50.0), &[laptop]));
        // Partly off the right edge is fine while it can be grabbed.
        assert!(is_reachable(&at(1300.0, 50.0), &[laptop]));
        assert!(!is_reachable(&at(1400.0, 50.0), &[laptop]));
        // On a display that is gone.
        assert!(!is_reachable(&at(-1800.0, 50.0), &[laptop]));
        assert!(is_reachable(&at(-1800.0, 50.0), &[laptop, left]));
        // Title bar above or below every display.
        assert!(!is_reachable(&at(100.0, -200.0), &[laptop]));
        assert!(!is_reachable(&at(100.0, 880.0), &[laptop]));
        assert!(!is_reachable(&at(100.0, 50.0), &[]));
    }
}
