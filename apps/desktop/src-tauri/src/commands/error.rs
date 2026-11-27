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
//! panic is recorded once, by the panic hook, in a record that also names the
//! command, and the command fails with an ordinary error instead. Only commands whose body cannot fail or panic, such as one
//! that returns a constant, may skip the wrapper.

use std::fmt::Display;
use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::task::Poll;

use serde::Serialize;
use termx_core::diagnostics::{redact, tracing, with_panic_context};
use termx_core::pty::PtyError;
use termx_core::workspace::{LaunchFailure, LayoutError, WorkspaceError};

/// Target of the records written for failed commands.
pub const COMMAND_TARGET: &str = "termx::command";

/// What kind of failure a command had, for the frontend to act on without
/// reading the message. Serialized in camelCase. More kinds may be added:
/// a frontend treats one it does not know as [`Internal`](Self::Internal).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum ErrorKind {
    /// The shell cannot find `claude`.
    ClaudeNotFound,
    /// The folder of a session is gone, or the folder given is not one.
    FolderMissing,
    /// A background session has no usable command.
    InvalidCommand,
    /// The system refused to start the process.
    SpawnFailed,
    /// The shell did not get to the command.
    LaunchUnconfirmed,
    /// The disk is full.
    StorageFull,
    /// The session, project, pane or split does not exist.
    NotFound,
    /// An argument is not acceptable: a folder that is not absolute, a
    /// ratio that is not a number, a layout that cannot take more panes.
    InvalidArgument,
    /// The session is running, and the operation needs one that is not.
    SessionRunning,
    /// The settings of Claude Code are not valid JSON, or not in a shape
    /// termx can change safely. termx changed nothing.
    InvalidSettings,
    /// The settings of Claude Code declare another marketplace named
    /// `termx`, or the user turned the plugin off: theirs to decide.
    Conflict,
    /// A file cannot be written: read-only, or not the user's.
    PermissionDenied,
    /// The hook sender of termx (`termx-cli`) is not next to the
    /// application.
    SenderMissing,
    /// Anything else, including a command that panicked.
    Internal,
}

/// A failure of a command: its kind and a message for the user.
///
/// Every error type a command meets converts into it, choosing its kind; a
/// plain message is [`ErrorKind::Internal`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    /// What kind of failure it is.
    pub kind: ErrorKind,
    /// What happened, for the user.
    pub message: String,
}

impl Failure {
    /// A failure of `kind` with `message`.
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self::new(ErrorKind::Internal, message)
    }
}

impl From<&str> for Failure {
    fn from(message: &str) -> Self {
        Self::new(ErrorKind::Internal, message)
    }
}

impl From<LaunchFailure> for Failure {
    fn from(failure: LaunchFailure) -> Self {
        let kind = match &failure {
            LaunchFailure::ClaudeNotFound { .. } => ErrorKind::ClaudeNotFound,
            LaunchFailure::FolderMissing { .. } => ErrorKind::FolderMissing,
            LaunchFailure::InvalidCommand { .. } => ErrorKind::InvalidCommand,
            LaunchFailure::SpawnFailed { .. } => ErrorKind::SpawnFailed,
            LaunchFailure::LaunchUnconfirmed { .. } => ErrorKind::LaunchUnconfirmed,
            _ => ErrorKind::Internal,
        };
        Self::new(kind, failure.to_string())
    }
}

impl From<LayoutError> for Failure {
    fn from(error: LayoutError) -> Self {
        let kind = match error {
            LayoutError::UnknownPane(_) | LayoutError::UnknownSplit(_) => ErrorKind::NotFound,
            _ => ErrorKind::InvalidArgument,
        };
        Self::new(kind, error.to_string())
    }
}

impl From<WorkspaceError> for Failure {
    fn from(error: WorkspaceError) -> Self {
        let kind = match error {
            WorkspaceError::Layout(layout) => return layout.into(),
            WorkspaceError::Launch(failure) => return failure.into(),
            WorkspaceError::UnknownSession(_) | WorkspaceError::UnknownProject => {
                ErrorKind::NotFound
            }
            WorkspaceError::FolderNotFound(_) => ErrorKind::FolderMissing,
            WorkspaceError::InvalidCommand => ErrorKind::InvalidCommand,
            WorkspaceError::SessionRunning(_) => ErrorKind::SessionRunning,
            WorkspaceError::InvalidFolder | WorkspaceError::InvalidClaudeSessionId => {
                ErrorKind::InvalidArgument
            }
            _ => ErrorKind::Internal,
        };
        Self::new(kind, error.to_string())
    }
}

impl From<PtyError> for Failure {
    fn from(error: PtyError) -> Self {
        let kind = match &error {
            PtyError::UnknownSession(_) => ErrorKind::NotFound,
            PtyError::InvalidSize { .. } => ErrorKind::InvalidArgument,
            PtyError::Spawn { .. } | PtyError::Open(_) => ErrorKind::SpawnFailed,
            _ => ErrorKind::Internal,
        };
        Self::new(kind, error.to_string())
    }
}

impl From<std::io::Error> for Failure {
    fn from(error: std::io::Error) -> Self {
        let kind = match error.kind() {
            std::io::ErrorKind::StorageFull | std::io::ErrorKind::QuotaExceeded => {
                ErrorKind::StorageFull
            }
            std::io::ErrorKind::NotFound => ErrorKind::NotFound,
            _ => ErrorKind::Internal,
        };
        Self::new(kind, error.to_string())
    }
}

impl From<termx_core::diagnostics::DiagnosticsError> for Failure {
    fn from(error: termx_core::diagnostics::DiagnosticsError) -> Self {
        let kind = match &error {
            termx_core::diagnostics::DiagnosticsError::Io { source, .. } => {
                Self::from(std::io::Error::from(source.kind())).kind
            }
            _ => ErrorKind::Internal,
        };
        // The message of the error itself, which redacts its path.
        Self::new(kind, error.to_string())
    }
}

/// What the frontend receives when a command fails.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommandError {
    /// Name of the command that failed.
    pub command: &'static str,
    /// What kind of failure it was. See [`ErrorKind`].
    pub kind: ErrorKind,
    /// Description of the failure, with the home directory replaced by `~`.
    pub message: String,
}

/// Result of a Tauri command.
pub type CommandResult<T> = Result<T, CommandError>;

impl CommandError {
    /// Logs the failure of `command` and returns the error for the frontend.
    ///
    /// Prefer [`logged`], which calls this for the `Err` case.
    pub fn log(command: &'static str, error: impl Into<Failure>) -> Self {
        let Failure { kind, message } = error.into();
        let message = redact::text(&message).into_owned();
        tracing::error!(target: COMMAND_TARGET, command, kind = ?kind, error = %message, "command failed");
        Self {
            command,
            kind,
            message,
        }
    }

    /// The error for a command that panicked. Nothing is logged here: the
    /// panic hook already wrote the one record, with the message, the
    /// backtrace and the name of the command.
    fn panicked(command: &'static str) -> Self {
        Self {
            command,
            kind: ErrorKind::Internal,
            message: PANIC_MESSAGE.to_owned(),
        }
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
pub fn logged<T, E: Into<Failure>>(
    command: &'static str,
    body: impl FnOnce() -> Result<T, E>,
) -> CommandResult<T> {
    // The body is not used again after a panic, so observing state it may
    // have left half-updated is not a concern here.
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        with_panic_context("command", command, body)
    }));
    match outcome {
        Ok(result) => result.map_err(|error| CommandError::log(command, error)),
        Err(_) => Err(CommandError::panicked(command)),
    }
}

/// [`logged`] for the body of an `async fn` command: also catches a panic at
/// any point of the future, so that the promise in the frontend always
/// settles.
///
/// # Errors
///
/// Returns the error of `body`, or an error saying that it panicked.
pub async fn logged_async<T, E: Into<Failure>>(
    command: &'static str,
    body: impl Future<Output = Result<T, E>>,
) -> CommandResult<T> {
    let mut body = Box::pin(body);
    let outcome = std::future::poll_fn(|context| {
        let polled = catch_unwind(AssertUnwindSafe(|| {
            with_panic_context("command", command, || body.as_mut().poll(context))
        }));
        match polled {
            Ok(Poll::Pending) => Poll::Pending,
            Ok(Poll::Ready(result)) => Poll::Ready(Some(result)),
            Err(_) => Poll::Ready(None),
        }
    })
    .await;

    match outcome {
        Some(result) => result.map_err(|error| CommandError::log(command, error)),
        None => Err(CommandError::panicked(command)),
    }
}

#[cfg(test)]
mod tests {
    use termx_core::diagnostics::tracing;

    use super::{
        COMMAND_TARGET, CommandError, ErrorKind, Failure, PANIC_MESSAGE, logged, logged_async,
    };
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
                kind: ErrorKind::Internal,
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
        assert_eq!(records[0]["fields"]["kind"], "Internal");
    }

    #[test]
    fn the_frontend_receives_the_command_the_kind_and_the_message() {
        let error = CommandError {
            command: "open_log_folder",
            kind: ErrorKind::StorageFull,
            message: "not available".to_owned(),
        };

        assert_eq!(
            serde_json::to_value(&error).unwrap(),
            serde_json::json!({
                "command": "open_log_folder",
                "kind": "storageFull",
                "message": "not available",
            })
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
                kind: ErrorKind::Internal,
                message: PANIC_MESSAGE.to_owned(),
            })
        );
        // The panic hook writes the one record of a panic, and it is not
        // installed here: nothing else is logged.
        assert!(log.finish().is_empty());
    }

    #[test]
    fn the_panic_record_of_a_command_names_the_command() {
        use termx_core::diagnostics::panic_context;

        // What the panic hook would add to its record while the body runs.
        let during = logged("resize", || Ok::<_, String>(panic_context())).unwrap();
        let during_async = tauri::async_runtime::block_on(logged_async("spawn_session", async {
            std::future::ready(()).await;
            Ok::<_, String>(panic_context())
        }))
        .unwrap();

        assert_eq!(during, [("command", "resize".to_owned())]);
        assert_eq!(during_async, [("command", "spawn_session".to_owned())]);
        assert!(
            panic_context().is_empty(),
            "the context ends with the command"
        );
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
    fn errors_say_what_kind_of_failure_they_are() {
        use termx_core::workspace::{
            LaunchFailure, LayoutError, PaneId, SessionKey, WorkspaceError,
        };

        let key = SessionKey::from_raw("s-x").unwrap();
        let kinds: Vec<(Failure, ErrorKind)> = vec![
            (
                WorkspaceError::UnknownSession(key.clone()).into(),
                ErrorKind::NotFound,
            ),
            (WorkspaceError::UnknownProject.into(), ErrorKind::NotFound),
            (
                WorkspaceError::FolderNotFound("/gone".into()).into(),
                ErrorKind::FolderMissing,
            ),
            (
                WorkspaceError::InvalidFolder.into(),
                ErrorKind::InvalidArgument,
            ),
            (
                WorkspaceError::InvalidCommand.into(),
                ErrorKind::InvalidCommand,
            ),
            (
                WorkspaceError::SessionRunning(key).into(),
                ErrorKind::SessionRunning,
            ),
            (
                WorkspaceError::Layout(LayoutError::UnknownPane(PaneId::from_raw(9))).into(),
                ErrorKind::NotFound,
            ),
            (LayoutError::TooManyPanes.into(), ErrorKind::InvalidArgument),
            (
                WorkspaceError::Launch(LaunchFailure::claude_not_found("claude")).into(),
                ErrorKind::ClaudeNotFound,
            ),
            (
                LaunchFailure::folder_missing().into(),
                ErrorKind::FolderMissing,
            ),
            (
                LaunchFailure::spawn_failed("no").into(),
                ErrorKind::SpawnFailed,
            ),
            (
                LaunchFailure::launch_unconfirmed().into(),
                ErrorKind::LaunchUnconfirmed,
            ),
            (
                std::io::Error::from(std::io::ErrorKind::StorageFull).into(),
                ErrorKind::StorageFull,
            ),
            ("a message".into(), ErrorKind::Internal),
        ];
        for (failure, kind) in kinds {
            assert_eq!(failure.kind, kind, "{failure}");
            assert!(!failure.message.is_empty());
        }
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
        assert!(log.finish().is_empty());
    }
}
