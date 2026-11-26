//! Wires the diagnostics of `termx-core` into the application: where the log
//! files live on each platform, and when logging starts and stops.
//!
//! Logging starts before the Tauri builder runs, so that a failure to build
//! the application or to create its window is recorded. That is why the
//! directories are resolved from the environment here instead of through
//! Tauri's path resolver, which only exists once the application is built.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

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
    /// The running logger. Empty when logging could not start.
    diagnostics: Option<Diagnostics>,
    /// Keeps a frontend error loop from flooding the log.
    pub(crate) frontend_limiter: Mutex<RateLimiter>,
}

impl DiagnosticsState {
    /// State for an application that runs without logging.
    #[must_use]
    pub fn new() -> Self {
        Self {
            diagnostics: None,
            frontend_limiter: Mutex::new(RateLimiter::new(FRONTEND_BURST, FRONTEND_INTERVAL)),
        }
    }

    /// State for a logger that is already running.
    #[must_use]
    pub fn with_diagnostics(diagnostics: Diagnostics) -> Self {
        Self {
            diagnostics: Some(diagnostics),
            ..Self::new()
        }
    }

    /// The running logger, if logging could be started.
    #[must_use]
    pub fn diagnostics(&self) -> Option<&Diagnostics> {
        self.diagnostics.as_ref()
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

/// Resolves the log directory from the environment of the process.
///
/// `variable` looks up an environment variable; `home` is the home directory
/// of the user. [`LOG_DIR_ENV_VAR`] wins over the platform location.
#[must_use]
pub fn log_dir_from_env(
    os: &str,
    variable: impl Fn(&str) -> Option<OsString>,
    home: Option<&Path>,
) -> Option<PathBuf> {
    let directory = |name: &str| {
        variable(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    if let Some(dir) = directory(LOG_DIR_ENV_VAR) {
        return Some(dir);
    }
    // What `%LOCALAPPDATA%` is on a default Windows installation.
    let local_data =
        directory("LOCALAPPDATA").or_else(|| Some(home?.join("AppData").join("Local")));
    let xdg_state_home = directory("XDG_STATE_HOME");
    log_dir_for(os, home, local_data.as_deref(), xdg_state_home.as_deref())
}

/// Starts logging. Called first thing when the application starts, before
/// anything that can fail.
///
/// Logging is never a reason to refuse to start: if the log cannot be opened,
/// the problem is printed to stderr and the application runs without it.
#[must_use]
pub fn start() -> DiagnosticsState {
    let home = std::env::home_dir();
    let dir = log_dir_from_env(
        std::env::consts::OS,
        |name| std::env::var_os(name),
        home.as_deref(),
    );
    let Some(dir) = dir else {
        eprintln!("termx: no log directory could be determined; logging is disabled");
        return DiagnosticsState::new();
    };

    let mut config = LogConfig::new(dir, env!("CARGO_PKG_VERSION"));
    config.stderr = cfg!(debug_assertions);
    config.webview_version = tauri::webview_version().ok();

    match Diagnostics::init(config) {
        Ok(diagnostics) => DiagnosticsState::with_diagnostics(diagnostics),
        Err(error) => {
            eprintln!("termx: logging is disabled: {error}");
            DiagnosticsState::new()
        }
    }
}

/// Writes out what is still queued. Called when the application exits.
pub fn stop(diagnostics: Option<&Diagnostics>) {
    if let Some(diagnostics) = diagnostics {
        tracing::info!("termx exiting");
        diagnostics.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use std::ffi::OsString;

    use super::{LOG_DIR_ENV_VAR, log_dir_for, log_dir_from_env};

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

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(value))
        }
    }

    #[test]
    fn the_log_directory_is_resolved_without_a_running_application() {
        let state = std::env::temp_dir().join("state");
        let local = std::env::temp_dir().join("Local");
        let state_text = state.to_string_lossy();
        let local_text = local.to_string_lossy();
        let variables = [
            ("XDG_STATE_HOME", &*state_text),
            ("LOCALAPPDATA", &*local_text),
        ];

        assert_eq!(
            log_dir_from_env("linux", env(&variables), Some(&home())).unwrap(),
            state.join("termx").join("logs")
        );
        assert_eq!(
            log_dir_from_env("windows", env(&variables), Some(&home())).unwrap(),
            local.join("termx").join("logs")
        );
        assert_eq!(
            log_dir_from_env("macos", env(&variables), Some(&home())).unwrap(),
            home().join("Library").join("Logs").join("termx")
        );
    }

    #[test]
    fn windows_falls_back_to_the_default_local_app_data_directory() {
        assert_eq!(
            log_dir_from_env("windows", env(&[]), Some(&home())).unwrap(),
            home()
                .join("AppData")
                .join("Local")
                .join("termx")
                .join("logs")
        );
        assert_eq!(log_dir_from_env("windows", env(&[]), None), None);
    }

    #[test]
    fn the_override_variable_wins_and_an_empty_one_is_ignored() {
        let custom = std::env::temp_dir().join("custom-logs");
        let custom_text = custom.to_string_lossy();

        assert_eq!(
            log_dir_from_env(
                "linux",
                env(&[(LOG_DIR_ENV_VAR, &*custom_text)]),
                Some(&home())
            )
            .unwrap(),
            custom
        );
        assert_eq!(
            log_dir_from_env("macos", env(&[(LOG_DIR_ENV_VAR, "")]), Some(&home())).unwrap(),
            home().join("Library").join("Logs").join("termx")
        );
    }
}
