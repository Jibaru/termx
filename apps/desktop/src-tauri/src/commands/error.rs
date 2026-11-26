//! The error type of Tauri commands, and the one place where command
//! failures are logged.
//!
//! Every command that can fail or panic returns [`CommandResult`] and runs
//! its body inside [`logged`] (or [`logged_async`] for an `async fn`):
//!
//! ```text
//! #[tauri::command]
//! pub fn rename_session(id: u32, title: String) -> CommandResult<()> {
//!     logged("rename_session", || sessions::rename(id, &title))
//! }
//! ```
//!
//! That writes exactly one `ERROR` record per failure, with the command name,
//! and hands the frontend a [`CommandError`] it can show.
//!
//! It also catches panics. A panic that escapes a command is never
//! acceptable: in a synchronous command it unwinds into the platform's event
//! loop and can abort the whole application, and in an asynchronous one it
//! leaves the promise in the frontend pending forever. Inside the wrappers a
//! panic is recorded by the panic hook and the command fails with an ordinary
//! error instead. Only commands whose body cannot fail or panic, such as one
//! that returns a constant, may skip the wrapper.

use std::any::Any;
use std::fmt::Display;
use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::task::Poll;

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

/// What the frontend is told when a command panicked. The details are in
/// the log, written by the panic hook.
pub const PANIC_MESSAGE: &str = "termx hit an internal error; the details are in the log";

/// Runs the body of a command. An error is logged once and converted to a
/// [`CommandError`]; a panic is turned into an error as well.
///
/// # Errors
///
/// Returns the error of `body`, or an error saying that it panicked.
pub fn logged<T, E: Display>(
    command: &'static str,
    body: impl FnOnce() -> Result<T, E>,
) -> CommandResult<T> {
    // The body is not used again after a panic, so observing state it may
    // have left half-updated is not a concern here.
    match catch_unwind(AssertUnwindSafe(body)) {
        Ok(result) => result.map_err(|error| CommandError::log(command, error)),
        Err(payload) => Err(panicked(command, payload.as_ref())),
    }
}

/// [`logged`] for the body of an `async fn` command: also catches a panic at
/// any point of the future, so that the promise in the frontend always
/// settles.
///
/// # Errors
///
/// Returns the error of `body`, or an error saying that it panicked.
pub async fn logged_async<T, E: Display>(
    command: &'static str,
    body: impl Future<Output = Result<T, E>>,
) -> CommandResult<T> {
    let mut body = Box::pin(body);
    let outcome = std::future::poll_fn(|context| {
        match catch_unwind(AssertUnwindSafe(|| body.as_mut().poll(context))) {
            Ok(Poll::Pending) => Poll::Pending,
            Ok(Poll::Ready(result)) => Poll::Ready(Ok(result)),
            Err(payload) => Poll::Ready(Err(payload)),
        }
    })
    .await;

    match outcome {
        Ok(result) => result.map_err(|error| CommandError::log(command, error)),
        Err(payload) => Err(panicked(command, payload.as_ref())),
    }
}

/// Logs that `command` panicked and returns the error for the frontend. The
/// panic itself was already recorded, with its backtrace, by the panic hook.
fn panicked(command: &'static str, payload: &(dyn Any + Send)) -> CommandError {
    let panic = payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("panic with a non-string payload");
    tracing::error!(target: COMMAND_TARGET, command, panic, "command panicked");
    CommandError {
        command,
        message: PANIC_MESSAGE.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use termx_core::diagnostics::tracing;

    use super::{COMMAND_TARGET, CommandError, PANIC_MESSAGE, logged, logged_async};
    use crate::test_support::TestLog;

    #[test]
    fn a_successful_command_logs_nothing() {
        let log = TestLog::new();

        let result = tracing::dispatcher::with_default(&log.dispatch, || {
            logged("answer", || Ok::<_, String>(42))
        });

        assert_eq!(result, Ok(42));
        assert!(log.finish().is_empty());
    }

    #[test]
    fn a_failed_command_is_logged_once_with_its_name() {
        let log = TestLog::new();

        let result = tracing::dispatcher::with_default(&log.dispatch, || {
            logged::<(), _>("rename_session", || Err("no session with id 7"))
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

    fn fail(reason: &str) -> Result<u32, String> {
        panic!("{reason}")
    }

    #[test]
    fn a_panicking_command_fails_with_an_error_instead_of_unwinding() {
        let log = TestLog::new();

        let result = tracing::dispatcher::with_default(&log.dispatch, || {
            logged("resize", || fail("index out of bounds"))
        });

        assert_eq!(
            result,
            Err(CommandError {
                command: "resize",
                message: PANIC_MESSAGE.to_owned(),
            })
        );
        let records = log.finish();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["level"], "ERROR");
        assert_eq!(records[0]["msg"], "command panicked");
        assert_eq!(records[0]["fields"]["command"], "resize");
        assert_eq!(records[0]["fields"]["panic"], "index out of bounds");
    }

    #[test]
    fn an_async_command_resolves_with_its_value_or_its_error() {
        let log = TestLog::new();
        let _default = tracing::dispatcher::set_default(&log.dispatch);

        let value =
            tauri::async_runtime::block_on(logged_async("answer", async { Ok::<_, String>(42) }));
        let error = tauri::async_runtime::block_on(logged_async("answer", async {
            Err::<u32, _>("no answer")
        }));

        assert_eq!(value, Ok(42));
        assert_eq!(error.unwrap_err().message, "no answer");
        assert_eq!(log.finish().len(), 1);
    }

    #[test]
    fn a_panicking_async_command_settles_with_an_error() {
        let log = TestLog::new();

        let _default = tracing::dispatcher::set_default(&log.dispatch);
        let result = tauri::async_runtime::block_on(logged_async("spawn_session", async {
            // Panic after the first suspension point, not on the first poll.
            std::future::ready(()).await;
            fail("shell vanished")
        }));

        assert_eq!(result.unwrap_err().message, PANIC_MESSAGE);
        let records = log.finish();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["msg"], "command panicked");
        assert_eq!(records[0]["fields"]["panic"], "shell vanished");
    }
}
