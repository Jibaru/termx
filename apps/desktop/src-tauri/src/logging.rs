//! Wires the diagnostics of `termx-core` into the application: where the log
//! files live on each platform, and when logging starts and stops.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use tauri::{AppHandle, Manager};
use termx_core::diagnostics::tracing;
use termx_core::diagnostics::{Diagnostics, LogConfig, RateLimiter};

/// Environment variable that overrides the log directory. Useful for
/// development and for tests that launch the real application.
pub const LOG_DIR_ENV_VAR: &str = "TERMX_LOG_DIR";

/// Name of the application directory inside the platform locations.
const APP_DIR: &str = "termx";

/// Records the frontend may write in one go before it is slowed down.
const FRONTEND_BURST: u32 = 20;
/// After a burst, the frontend may write one record per interval.
const FRONTEND_INTERVAL: Duration = Duration::from_secs(1);

/// Application state for diagnostics, managed by Tauri.
pub struct DiagnosticsState {
    /// Set once logging has started. Stays empty when it could not start.
    diagnostics: OnceLock<Diagnostics>,
    /// Keeps a frontend error loop from flooding the log.
    pub(crate) frontend_limiter: Mutex<RateLimiter>,
}

impl DiagnosticsState {
    /// State with logging not started yet.
    #[must_use]
    pub fn new() -> Self {
        Self {
            diagnostics: OnceLock::new(),
            frontend_limiter: Mutex::new(RateLimiter::new(FRONTEND_BURST, FRONTEND_INTERVAL)),
        }
    }

    /// State for a logger that is already running.
    #[must_use]
    pub fn with_diagnostics(diagnostics: Diagnostics) -> Self {
        let state = Self::new();
        // The cell was created empty on the line above.
        let _ = state.diagnostics.set(diagnostics);
        state
    }

    /// The running logger, if logging could be started.
    #[must_use]
    pub fn diagnostics(&self) -> Option<&Diagnostics> {
        self.diagnostics.get()
    }
}

impl Default for DiagnosticsState {
    fn default() -> Self {
        Self::new()
    }
}

/// Returns the log directory for the operating system `os` (a value of
/// [`std::env::consts::OS`]).
///
/// | Platform | Directory                                                    |
/// | -------- | ------------------------------------------------------------ |
/// | macOS    | `~/Library/Logs/termx`                                       |
/// | Windows  | `%LOCALAPPDATA%\termx\logs`                                  |
/// | Linux    | `$XDG_STATE_HOME/termx/logs`, or `~/.local/state/termx/logs` |
///
/// Returns `None` when the base directory it needs is unknown.
#[must_use]
pub fn log_dir_for(
    os: &str,
    home: Option<&Path>,
    local_data: Option<&Path>,
    xdg_state_home: Option<&Path>,
) -> Option<PathBuf> {
    match os {
        "macos" => Some(home?.join("Library").join("Logs").join(APP_DIR)),
        "windows" => Some(local_data?.join(APP_DIR).join("logs")),
        _ => {
            // The XDG specification says to ignore relative paths.
            let state = match xdg_state_home.filter(|path| path.is_absolute()) {
                Some(state) => state.to_owned(),
                None => home?.join(".local").join("state"),
            };
            Some(state.join(APP_DIR).join("logs"))
        }
    }
}

/// Resolves the log directory of the running application.
fn log_dir(app: &AppHandle) -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(LOG_DIR_ENV_VAR).filter(|dir| !dir.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    let paths = app.path();
    let home = paths.home_dir().ok();
    let local_data = paths.local_data_dir().ok();
    let xdg_state_home = std::env::var_os("XDG_STATE_HOME").map(PathBuf::from);
    log_dir_for(
        std::env::consts::OS,
        home.as_deref(),
        local_data.as_deref(),
        xdg_state_home.as_deref(),
    )
}

/// Starts logging. Called first thing when the application is set up.
///
/// Logging is never a reason to refuse to start: if the log cannot be opened,
/// the problem is printed to stderr and the application runs without it.
pub fn start(app: &AppHandle) {
    let Some(dir) = log_dir(app) else {
        eprintln!("termx: no log directory could be determined; logging is disabled");
        return;
    };

    let mut config = LogConfig::new(dir, app.package_info().version.to_string());
    config.stderr = cfg!(debug_assertions);
    config.webview_version = tauri::webview_version().ok();

    match Diagnostics::init(config) {
        Ok(diagnostics) => {
            if let Some(state) = app.try_state::<DiagnosticsState>() {
                let _ = state.diagnostics.set(diagnostics);
            }
        }
        Err(error) => eprintln!("termx: logging is disabled: {error}"),
    }
}

/// Writes out what is still queued. Called when the application exits.
pub fn stop(app: &AppHandle) {
    if let Some(diagnostics) = app
        .try_state::<DiagnosticsState>()
        .as_deref()
        .and_then(DiagnosticsState::diagnostics)
    {
        tracing::info!("termx exiting");
        diagnostics.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::log_dir_for;

    fn home() -> PathBuf {
        Path::new("home").join("ana")
    }

    #[test]
    fn macos_logs_go_to_the_user_library() {
        let dir = log_dir_for("macos", Some(&home()), None, None).unwrap();

        assert_eq!(dir, home().join("Library").join("Logs").join("termx"));
    }

    #[test]
    fn windows_logs_go_to_local_app_data() {
        let local = home().join("AppData").join("Local");

        let dir = log_dir_for("windows", Some(&home()), Some(&local), None).unwrap();

        assert_eq!(dir, local.join("termx").join("logs"));
    }

    #[test]
    fn linux_logs_go_to_the_xdg_state_directory() {
        let state = std::env::temp_dir().join("state");
        assert!(state.is_absolute());

        let dir = log_dir_for("linux", Some(&home()), None, Some(&state)).unwrap();

        assert_eq!(dir, state.join("termx").join("logs"));
    }

    #[test]
    fn linux_falls_back_to_the_default_state_directory() {
        let expected = home()
            .join(".local")
            .join("state")
            .join("termx")
            .join("logs");

        assert_eq!(
            log_dir_for("linux", Some(&home()), None, None).unwrap(),
            expected
        );
        // A relative XDG_STATE_HOME is invalid and ignored.
        let relative = Path::new("relative");
        assert_eq!(
            log_dir_for("linux", Some(&home()), None, Some(relative)).unwrap(),
            expected
        );
    }

    #[test]
    fn an_unknown_base_directory_gives_no_log_directory() {
        assert_eq!(log_dir_for("macos", None, None, None), None);
        assert_eq!(log_dir_for("windows", Some(&home()), None, None), None);
        assert_eq!(log_dir_for("linux", None, None, None), None);
    }
}
