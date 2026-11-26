//! The error type of Tauri commands, and the one place where command
//! failures are logged.
//!
//! Every fallible command returns [`CommandResult`] and builds its result
//! with [`logged`]:
//!
//! ```text
//! #[tauri::command]
//! pub fn rename_session(id: u32, title: String) -> CommandResult<()> {
//!     logged("rename_session", sessions::rename(id, &title))
//! }
//! ```
//!
//! That writes exactly one `ERROR` record per failure, with the command name,
//! and hands the frontend a [`CommandError`] it can show.

use std::fmt::Display;

use serde::Serialize;
use termx_core::diagnostics::{redact, tracing};

/// Target of the records written for failed commands.
pub const COMMAND_TARGET: &str = "termx::command";

/// What the frontend receives when a command fails.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommandError {
    /// Name of the command that failed.
    pub command: &'static str,
    /// Description of the failure, with the home directory replaced by `~`.
    pub message: String,
}

/// Result of a Tauri command.
pub type CommandResult<T> = Result<T, CommandError>;

impl CommandError {
    /// Logs the failure of `command` and returns the error for the frontend.
    ///
    /// Prefer [`logged`], which calls this for the `Err` case.
    pub fn log(command: &'static str, error: impl Display) -> Self {
        let message = redact::text(&error.to_string()).into_owned();
        tracing::error!(target: COMMAND_TARGET, command, error = %message, "command failed");
        Self { command, message }
    }
}

impl Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.command, self.message)
    }
}

impl std::error::Error for CommandError {}

/// Turns the result of a command body into a [`CommandResult`], logging the
/// error once if there is one.
///
/// # Errors
///
/// Returns the error of `result`, converted to a [`CommandError`].
pub fn logged<T, E: Display>(command: &'static str, result: Result<T, E>) -> CommandResult<T> {
    result.map_err(|error| CommandError::log(command, error))
}

#[cfg(test)]
mod tests {
    use termx_core::diagnostics::tracing;

    use super::{COMMAND_TARGET, CommandError, logged};
    use crate::test_support::TestLog;

    #[test]
    fn a_successful_command_logs_nothing() {
        let log = TestLog::new();

        let result = tracing::dispatcher::with_default(&log.dispatch, || {
            logged("answer", Ok::<_, String>(42))
        });

        assert_eq!(result, Ok(42));
        assert!(log.finish().is_empty());
    }

    #[test]
    fn a_failed_command_is_logged_once_with_its_name() {
        let log = TestLog::new();

        let result = tracing::dispatcher::with_default(&log.dispatch, || {
            logged::<(), _>("rename_session", Err("no session with id 7"))
        });

        assert_eq!(
            result,
            Err(CommandError {
                command: "rename_session",
                message: "no session with id 7".to_owned(),
            })
        );
        let records = log.finish();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["level"], "ERROR");
        assert_eq!(records[0]["target"], COMMAND_TARGET);
        assert_eq!(records[0]["msg"], "command failed");
        assert_eq!(records[0]["fields"]["command"], "rename_session");
        assert_eq!(records[0]["fields"]["error"], "no session with id 7");
    }

    #[test]
    fn the_frontend_receives_the_command_and_the_message() {
        let error = CommandError {
            command: "open_log_folder",
            message: "not available".to_owned(),
        };

        assert_eq!(
            serde_json::to_value(&error).unwrap(),
            serde_json::json!({"command": "open_log_folder", "message": "not available"})
        );
        assert_eq!(error.to_string(), "open_log_folder: not available");
    }
}
