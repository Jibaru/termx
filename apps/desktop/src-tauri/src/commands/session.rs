//! Commands driving the terminal sessions: create, write, resize,
//! acknowledge and close.
//!
//! The output of a session and its exit do not come back through a command:
//! they are streamed on the channel given to [`session_create`]. See
//! [`crate::terminal`].
//!
//! Writing, resizing or acknowledging a session that has ended is not an
//! error: the frontend normally still does so for a moment after a process
//! exits, before it has seen the exit.
//!
//! Nothing here logs what is typed or printed, only identifiers and sizes.

use serde::Serialize;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{State, Webview};
use termx_core::diagnostics::tracing::debug;
use termx_core::pty::{PtyError, PtySize};
use termx_core::session::{ACK_BATCH_BYTES, SessionId};

use super::error::{CommandResult, logged, logged_async};
use crate::terminal::{CreateSessionRequest, TerminalState, session_is_gone, windows_build_number};

/// What the frontend learns about the session it created.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedSession {
    /// Identifier to pass to the other session commands.
    pub id: u64,
    /// How many processed bytes to accumulate before acknowledging them
    /// with [`session_ack`].
    pub ack_batch_bytes: u64,
    /// The build number of Windows, for the `windowsPty` option of
    /// xterm.js. Absent on other systems.
    pub windows_build_number: Option<u32>,
}

/// Starts a session and streams its output, then its exit, to `on_event`.
///
/// The session belongs to the window that created it and is closed with it.
///
/// # Errors
///
/// Fails when the process cannot be started.
#[tauri::command(async)]
pub fn session_create(
    webview: Webview,
    state: State<'_, TerminalState>,
    request: CreateSessionRequest,
    on_event: Channel<InvokeResponseBody>,
) -> CommandResult<CreatedSession> {
    logged("session_create", || {
        let id = state.create(webview.label(), &request, on_event)?;
        Ok::<_, String>(CreatedSession {
            id: id.as_u64(),
            ack_batch_bytes: ACK_BATCH_BYTES,
            windows_build_number: windows_build_number(),
        })
    })
}

/// Sends input to a session, as if it had been typed.
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
        .map_err(|source| format!("the write did not complete: {source}"))?;
        ignore_ended_session("write", id, written)
    })
    .await
}

/// Changes the size of the terminal of a session.
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
        let resized = state
            .manager()?
            .resize(SessionId::from_raw(id), PtySize::new(cols, rows));
        ignore_ended_session("resize", id, resized)
    })
}

/// Records that the frontend has finished processing `bytes` of the output
/// of a session, which lets a session that was held back resume.
///
/// # Errors
///
/// Fails when the session manager is not running.
#[tauri::command]
pub fn session_ack(state: State<'_, TerminalState>, id: u64, bytes: u64) -> CommandResult<()> {
    logged("session_ack", || {
        state.manager()?.acknowledge(SessionId::from_raw(id), bytes);
        Ok::<_, String>(())
    })
}

/// Closes a session, killing its process. Closing a session that has
/// already ended does nothing.
///
/// Runs on a thread that may block: a process that ignores the request to
/// terminate is waited for until it is killed by force.
///
/// # Errors
///
/// Only fails if closing the session panicked.
#[tauri::command(async)]
pub fn session_close(state: State<'_, TerminalState>, id: u64) -> CommandResult<()> {
    logged("session_close", || {
        state.close(SessionId::from_raw(id));
        Ok::<_, String>(())
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
) -> Result<(), String> {
    match result {
        Err(error) if session_is_gone(&error) => {
            debug!(
                session_id = id,
                operation, "ignored for a session that has ended"
            );
            Ok(())
        }
        other => other.map_err(|error| error.to_string()),
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

        assert!(error.contains("invalid terminal size"), "{error}");
    }

    #[test]
    fn the_created_session_is_described_in_camel_case() {
        let created = CreatedSession {
            id: 3,
            ack_batch_bytes: 8192,
            windows_build_number: None,
        };

        assert_eq!(
            serde_json::to_value(&created).unwrap(),
            serde_json::json!({ "id": 3, "ackBatchBytes": 8192, "windowsBuildNumber": null })
        );
    }
}
