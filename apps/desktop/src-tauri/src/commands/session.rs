//! Commands about sessions: creating, naming, ordering, starting and
//! closing them, and driving the terminal of one that runs.
//!
//! Two identifiers are involved. A session is identified by a text that
//! never changes (`id` in the commands that take a session). While it
//! runs, its terminal is identified by a number, which changes every time
//! the session is started (`terminal` in [`StartedSession`], and the `id`
//! of [`session_write`], [`session_resize`] and [`session_ack`]).
//!
//! The output of a session and its exit do not come back through a
//! command: they are streamed on the channel given to [`session_start`].
//! See [`crate::terminal`]. What changes about a session (its state, its
//! title, its branch) reaches the frontend through the subscription of
//! [`workspace_subscribe`](super::workspace::workspace_subscribe).
//!
//! Writing, resizing or acknowledging a terminal that has ended is not an
//! error: the frontend normally still does so for a moment after a process
//! exits, before it has seen the exit. Likewise for reporting the title or
//! the folder of a session that was just closed.
//!
//! Nothing here logs what is typed or printed, nor the command of a
//! session: only identifiers, kinds and sizes.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{Runtime, State, Webview};
use termx_core::diagnostics::tracing::debug;
use termx_core::pty::{PtyError, PtySize};
use termx_core::session::{ACK_BATCH_BYTES, SessionId};
use termx_core::workspace::{
    LaunchMode, NewSession, Session, SessionKey, SessionKind, WorkspaceError,
};

use super::error::{CommandResult, ErrorKind, Failure, logged, logged_async};
use crate::terminal::{TerminalState, session_is_gone, windows_build_number};
use crate::workspace::WorkspaceState;

/// What the frontend asks for when it creates a session.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionRequest {
    /// What the session runs.
    pub kind: SessionKind,
    /// The folder it runs in. The home directory when omitted.
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    /// The command line of a background session.
    #[serde(default)]
    pub command: Option<String>,
    /// A name for the session, as if the user had renamed it.
    #[serde(default)]
    pub name: Option<String>,
    /// Whether to show the session in the focused pane. Yes when omitted.
    #[serde(default)]
    pub show: Option<bool>,
}

/// How the frontend wants a session started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartSessionRequest {
    /// `"fresh"` to start from scratch, which is also how a session is
    /// restarted, or `"resume"` to carry on: Claude Code then resumes its
    /// conversation. From scratch when omitted.
    #[serde(default)]
    pub mode: LaunchMode,
    /// Width of the terminal, in columns.
    pub cols: u16,
    /// Height of the terminal, in rows.
    pub rows: u16,
}

/// What the frontend learns about the terminal a session was started in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartedSession {
    /// Identifier of the terminal, to pass to [`session_write`],
    /// [`session_resize`] and [`session_ack`].
    pub terminal: u64,
    /// How many processed bytes to accumulate before acknowledging them
    /// with [`session_ack`].
    pub ack_batch_bytes: u64,
    /// The build number of Windows, for the `windowsPty` option of
    /// xterm.js. Absent on other systems.
    pub windows_build_number: Option<u32>,
}

/// Creates a session and returns it, in the `starting` state: start it
/// with [`session_start`] once its pane knows its size.
///
/// # Errors
///
/// Fails when the folder does not exist, or when a background session has
/// no usable command.
#[tauri::command(async)]
pub fn session_create(
    state: State<'_, WorkspaceState>,
    request: CreateSessionRequest,
) -> CommandResult<Session> {
    logged("session_create", || {
        let cwd = match request.cwd {
            Some(cwd) => cwd,
            None => state
                .default_folder()
                .ok_or(Failure::new(
                    ErrorKind::FolderMissing,
                    "the home directory is not known: choose a folder",
                ))?
                .to_owned(),
        };
        let session = state.workspace().create_session(NewSession {
            kind: request.kind,
            cwd,
            command: request.command,
            name: request.name,
            show: request.show.unwrap_or(true),
        })?;
        Ok::<_, Failure>(session)
    })
}

/// Creates another Claude Code session in the folder of the session `id`,
/// without asking anything.
///
/// # Errors
///
/// Fails when the session does not exist or its folder is gone.
#[tauri::command(async)]
pub fn session_create_sibling(
    state: State<'_, WorkspaceState>,
    id: SessionKey,
) -> CommandResult<Session> {
    logged("session_create_sibling", || {
        state.workspace().create_sibling(&id)
    })
}

/// Gives a session a name, which wins over any automatic title. `null` or
/// an empty name removes it.
///
/// # Errors
///
/// Fails when the session does not exist.
#[tauri::command]
pub fn session_rename(
    state: State<'_, WorkspaceState>,
    id: SessionKey,
    name: Option<String>,
) -> CommandResult<()> {
    logged("session_rename", || {
        state.workspace().rename_session(&id, name.as_deref())
    })
}

/// Moves a session just before the session `before` in the order of the
/// sessions, or to the end when `before` is `null`.
///
/// # Errors
///
/// Fails when either session does not exist.
#[tauri::command]
pub fn session_reorder(
    state: State<'_, WorkspaceState>,
    id: SessionKey,
    before: Option<SessionKey>,
) -> CommandResult<()> {
    logged("session_reorder", || {
        state.workspace().reorder_session(&id, before.as_ref())
    })
}

/// Moves a project just before the project `before` in the order of the
/// projects, or to the end when `before` is `null`.
///
/// # Errors
///
/// Fails when either project has no session.
#[tauri::command]
pub fn project_reorder(
    state: State<'_, WorkspaceState>,
    key: String,
    before: Option<String>,
) -> CommandResult<()> {
    logged("project_reorder", || {
        state.workspace().reorder_project(&key, before.as_deref())
    })
}

/// Starts the session `id` in a new terminal of the given size, and
/// streams its output, then its exit, to `on_event`. A session that is
/// running is restarted.
///
/// The terminal belongs to the window that started it and is closed with
/// it; the session then becomes dormant.
///
/// # Errors
///
/// Fails when the session does not exist, when its folder is gone or when
/// the process cannot be started. The session then says why in its state.
#[tauri::command(async)]
pub fn session_start<R: Runtime>(
    webview: Webview<R>,
    terminals: State<'_, TerminalState>,
    state: State<'_, WorkspaceState>,
    id: SessionKey,
    request: StartSessionRequest,
    on_event: Channel<InvokeResponseBody>,
) -> CommandResult<StartedSession> {
    logged("session_start", || {
        let terminal = terminals.start_session(
            webview.label(),
            state.workspace(),
            &id,
            request.mode,
            PtySize::new(request.cols, request.rows),
            on_event,
        )?;
        Ok::<_, Failure>(StartedSession {
            terminal: terminal.as_u64(),
            ack_batch_bytes: ACK_BATCH_BYTES,
            windows_build_number: windows_build_number(),
        })
    })
}

/// Attaches this page to the terminal a session runs in, after the page was
/// reloaded: `on_event` receives the recent output of the terminal, then
/// what follows. Nothing is started or restarted. The terminal is resized
/// to `cols` by `rows`, which makes a full-screen program redraw.
///
/// # Errors
///
/// Fails with `notFound` when the session does not exist or does not run.
#[tauri::command]
pub fn session_attach<R: Runtime>(
    webview: Webview<R>,
    terminals: State<'_, TerminalState>,
    state: State<'_, WorkspaceState>,
    id: SessionKey,
    cols: u16,
    rows: u16,
    on_event: Channel<InvokeResponseBody>,
) -> CommandResult<StartedSession> {
    logged("session_attach", || {
        let terminal = state
            .workspace()
            .session(&id)
            .and_then(|session| session.terminal_id)
            .ok_or_else(|| Failure::new(ErrorKind::NotFound, "the session does not run"))?;
        let terminal = SessionId::from_raw(terminal);
        terminals.attach(
            webview.label(),
            terminal,
            PtySize::new(cols, rows),
            on_event,
        )?;
        Ok::<_, Failure>(StartedSession {
            terminal: terminal.as_u64(),
            ack_batch_bytes: ACK_BATCH_BYTES,
            windows_build_number: windows_build_number(),
        })
    })
}

/// Closes a session for good: its process is killed and it leaves the
/// sidebar and its pane. Closing a session that does not exist does
/// nothing.
///
/// Runs on a thread that may block: a process that ignores the request to
/// terminate is waited for until it is killed by force.
///
/// # Errors
///
/// Only fails if closing the session panicked.
#[tauri::command(async)]
pub fn session_close(
    terminals: State<'_, TerminalState>,
    state: State<'_, WorkspaceState>,
    id: SessionKey,
) -> CommandResult<()> {
    logged("session_close", || {
        match state.workspace().close_session(&id) {
            Ok(Some(terminal)) => {
                terminals.close(SessionId::from_raw(terminal));
            }
            Ok(None) | Err(WorkspaceError::UnknownSession(_)) => {}
            Err(error) => return Err(error),
        }
        Ok(())
    })
}

/// Stops a session without removing it: its process is killed and it
/// becomes `dormant`, keeping its place, its name and its conversation.
/// Start it again with [`session_start`]. Stopping a session that is not
/// running does nothing.
///
/// Runs on a thread that may block, like [`session_close`].
///
/// # Errors
///
/// Fails when the session does not exist.
#[tauri::command(async)]
pub fn session_stop(
    terminals: State<'_, TerminalState>,
    state: State<'_, WorkspaceState>,
    id: SessionKey,
) -> CommandResult<()> {
    logged("session_stop", || {
        if let Some(terminal) = state.workspace().stop_session(&id)? {
            terminals.close(SessionId::from_raw(terminal));
        }
        Ok::<_, WorkspaceError>(())
    })
}

/// Removes a session that is not running: it leaves the sidebar and its
/// pane. A running session is refused, so that a process is never ended by
/// mistake: stop it first, or use [`session_close`].
///
/// # Errors
///
/// Fails when the session does not exist, or is starting or running.
#[tauri::command]
pub fn session_remove(state: State<'_, WorkspaceState>, id: SessionKey) -> CommandResult<()> {
    logged("session_remove", || state.workspace().remove_session(&id))
}

/// Records the title a program set for its terminal (OSC 0 and 2), as
/// xterm.js reports it. It becomes the title of the session unless the
/// user named it.
///
/// # Errors
///
/// Only fails if recording the title panicked.
#[tauri::command]
pub fn session_report_title(
    state: State<'_, WorkspaceState>,
    id: SessionKey,
    title: String,
) -> CommandResult<()> {
    logged("session_report_title", || {
        ignore_closed_session(state.workspace().report_title(&id, &title))
    })
}

/// Records the folder a program says it is in (OSC 7): a `file://` URL or
/// a path. The project and branch of the session follow it. A folder that
/// does not exist on this machine is ignored.
///
/// # Errors
///
/// Only fails if recording the folder panicked.
#[tauri::command(async)]
pub fn session_report_cwd(
    state: State<'_, WorkspaceState>,
    id: SessionKey,
    cwd: String,
) -> CommandResult<()> {
    logged("session_report_cwd", || {
        ignore_closed_session(state.workspace().report_cwd(&id, &cwd).map(|_| ()))
    })
}

/// Turns the failure of a report about a session that was just closed
/// into a success. See the module documentation.
fn ignore_closed_session(result: Result<(), WorkspaceError>) -> Result<(), WorkspaceError> {
    match result {
        Ok(()) | Err(WorkspaceError::UnknownSession(_)) => Ok(()),
        Err(error) => Err(error),
    }
}

/// Sends input to the terminal `id`, as if it had been typed.
///
/// `data` is text, sent as UTF-8. With `binary` set, each character of
/// `data` stands for one byte instead (its code point, 0 to 255), which is
/// how xterm.js reports the few inputs that are not text.
///
/// The frontend must wait for the result before it sends the next input of
/// the same session: that is what keeps input in order. The write runs on a
/// thread that may block, because a process that does not read its input
/// makes it wait.
///
/// # Errors
///
/// Fails when the system rejects the write.
#[tauri::command]
pub async fn session_write(
    state: State<'_, TerminalState>,
    id: u64,
    data: String,
    binary: Option<bool>,
) -> CommandResult<()> {
    logged_async("session_write", async {
        let manager = state.manager()?.clone();
        let bytes = input_bytes(data, binary.unwrap_or(false));
        let written = tauri::async_runtime::spawn_blocking(move || {
            manager.write(SessionId::from_raw(id), &bytes)
        })
        .await
        .map_err(|source| Failure::from(format!("the write did not complete: {source}")))?;
        ignore_ended_session("write", id, written)
    })
    .await
}

/// Changes the size of the terminal `id`.
///
/// # Errors
///
/// Fails when the size is invalid or the system rejects it.
#[tauri::command]
pub fn session_resize(
    state: State<'_, TerminalState>,
    id: u64,
    cols: u16,
    rows: u16,
) -> CommandResult<()> {
    logged("session_resize", || {
        state.manager()?;
        let resized = state.resize(SessionId::from_raw(id), PtySize::new(cols, rows));
        ignore_ended_session("resize", id, resized)
    })
}

/// Records that the frontend has finished processing `bytes` of the output
/// of the terminal `id`, which lets a terminal that was held back resume.
///
/// # Errors
///
/// Fails when the session manager is not running.
#[tauri::command]
pub fn session_ack(state: State<'_, TerminalState>, id: u64, bytes: u64) -> CommandResult<()> {
    logged("session_ack", || {
        state.acknowledge(SessionId::from_raw(id), bytes)
    })
}

/// The bytes to write for the input `data`. See [`session_write`].
fn input_bytes(data: String, binary: bool) -> Vec<u8> {
    if binary {
        data.chars()
            // A character above 255 is not a byte; xterm.js never sends one.
            .filter_map(|character| u8::try_from(u32::from(character)).ok())
            .collect()
    } else {
        data.into_bytes()
    }
}

/// Turns the failure of an operation on a session that has ended into a
/// success. See the module documentation.
fn ignore_ended_session(
    operation: &'static str,
    id: u64,
    result: Result<(), PtyError>,
) -> Result<(), Failure> {
    match result {
        Err(error) if session_is_gone(&error) => {
            debug!(
                session_id = id,
                operation, "ignored for a session that has ended"
            );
            Ok(())
        }
        other => other.map_err(Failure::from),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_input_is_sent_as_utf8() {
        assert_eq!(input_bytes("ls -la\r".to_owned(), false), b"ls -la\r");
        assert_eq!(
            input_bytes("\u{e9}\u{2713}".to_owned(), false),
            "\u{e9}\u{2713}".as_bytes()
        );
        assert_eq!(input_bytes("\x1b\r".to_owned(), false), b"\x1b\r");
    }

    #[test]
    fn binary_input_is_sent_byte_for_byte() {
        // A mouse report in the legacy encoding: its coordinates are bytes
        // above 127, which are not valid UTF-8 on their own.
        let report = "\x1b[M \u{a0}\u{ff}".to_owned();

        assert_eq!(input_bytes(report, true), b"\x1b[M \xa0\xff");
        assert_eq!(input_bytes("a\u{2713}b".to_owned(), true), b"ab");
    }

    #[test]
    fn operations_on_an_ended_session_succeed() {
        let gone = PtyError::UnknownSession(SessionId::from_raw(4));

        assert_eq!(ignore_ended_session("write", 4, Err(gone)), Ok(()));
        assert_eq!(
            ignore_ended_session("resize", 4, Err(PtyError::Exited)),
            Ok(())
        );
        assert_eq!(ignore_ended_session("write", 4, Ok(())), Ok(()));
    }

    #[test]
    fn other_failures_are_reported() {
        let invalid = PtyError::InvalidSize { cols: 0, rows: 24 };

        let error = ignore_ended_session("resize", 4, Err(invalid)).unwrap_err();

        assert!(error.message.contains("invalid terminal size"), "{error}");
    }

    #[test]
    fn the_started_session_is_described_in_camel_case() {
        let started = StartedSession {
            terminal: 3,
            ack_batch_bytes: 8192,
            windows_build_number: None,
        };

        assert_eq!(
            serde_json::to_value(&started).unwrap(),
            serde_json::json!({ "terminal": 3, "ackBatchBytes": 8192, "windowsBuildNumber": null })
        );
    }

    #[test]
    fn a_request_to_create_a_session_only_needs_its_kind() {
        let minimal: CreateSessionRequest =
            serde_json::from_value(serde_json::json!({ "kind": "shell" })).unwrap();
        assert_eq!(
            minimal,
            CreateSessionRequest {
                kind: SessionKind::Shell,
                cwd: None,
                command: None,
                name: None,
                show: None,
            }
        );

        let full: CreateSessionRequest = serde_json::from_value(serde_json::json!({
            "kind": "background",
            "cwd": "/work/app",
            "command": "pnpm dev",
            "name": "dev server",
            "show": false,
        }))
        .unwrap();
        assert_eq!(full.kind, SessionKind::Background);
        assert_eq!(full.cwd, Some(PathBuf::from("/work/app")));
        assert_eq!(full.command.as_deref(), Some("pnpm dev"));
        assert_eq!(full.name.as_deref(), Some("dev server"));
        assert_eq!(full.show, Some(false));

        for bad in [
            serde_json::json!({}),
            serde_json::json!({ "kind": "teleporter" }),
        ] {
            assert!(serde_json::from_value::<CreateSessionRequest>(bad).is_err());
        }
    }

    #[test]
    fn a_request_to_start_a_session_starts_from_scratch_by_default() {
        let request: StartSessionRequest =
            serde_json::from_value(serde_json::json!({ "cols": 120, "rows": 40 })).unwrap();
        assert_eq!(
            request,
            StartSessionRequest {
                mode: LaunchMode::Fresh,
                cols: 120,
                rows: 40
            }
        );

        let request: StartSessionRequest =
            serde_json::from_value(serde_json::json!({ "mode": "resume", "cols": 80, "rows": 24 }))
                .unwrap();
        assert_eq!(request.mode, LaunchMode::Resume);
    }

    #[test]
    fn reports_about_a_session_that_was_closed_are_not_failures() {
        let ghost = SessionKey::from_raw("s-ghost").unwrap();

        assert_eq!(
            ignore_closed_session(Err(WorkspaceError::UnknownSession(ghost))),
            Ok(())
        );
        assert_eq!(ignore_closed_session(Ok(())), Ok(()));
        assert!(ignore_closed_session(Err(WorkspaceError::InvalidFolder)).is_err());
    }
}
