//! Terminal sessions.
//!
//! A session is one running terminal: a process attached to a PTY, whose
//! output is delivered to a frontend. This module is the layer between the
//! [PTY](crate::pty) and whatever displays the terminal. It knows nothing
//! about the frontend: output goes to a [`SessionSink`], which the
//! application implements on top of its own transport.
//!
//! # Overview
//!
//! - [`SessionManager`] is the registry of the running sessions, keyed by
//!   [`SessionId`]. It creates sessions, forwards input and resizes, takes
//!   the acknowledgements of the frontend, closes sessions and shuts
//!   everything down.
//! - [`SessionOptions`] says what a new session runs. The default is the
//!   user's login shell in their home directory.
//! - [`SessionSink`] receives the output of a session in batches and, at the
//!   end, how its process exited.
//!
//! ```no_run
//! use termx_core::pty::ExitStatus;
//! use termx_core::session::{SessionId, SessionManager, SessionOptions, SessionSink};
//!
//! struct Printer;
//!
//! impl SessionSink for Printer {
//!     fn output(&mut self, _session: SessionId, bytes: Vec<u8>) {
//!         print!("{}", String::from_utf8_lossy(&bytes));
//!     }
//!
//!     fn exited(&mut self, session: SessionId, status: ExitStatus) {
//!         println!("session {session} {status}");
//!     }
//! }
//!
//! # fn main() -> Result<(), termx_core::pty::PtyError> {
//! let sessions = SessionManager::new()?;
//! let id = sessions.create(&SessionOptions::default(), Printer)?;
//!
//! sessions.write(id, b"echo hello\r")?;
//! // A real frontend acknowledges what it has displayed; see below.
//! sessions.acknowledge(id, 1024);
//! sessions.close(id);
//! # Ok(())
//! # }
//! ```
//!
//! # The path of the output
//!
//! ```text
//! process -> PTY -> reader thread -> bounded buffer -> pump -> SessionSink
//!                                       (64 KiB)      batches   frontend
//!                                                        ^         |
//!                                                        +-- acks -+
//! ```
//!
//! One **driver thread** per manager runs the pumps of all its sessions. A
//! PTY tells it when there is output to take
//! ([`PtyOutput::on_readable`](crate::pty::PtyOutput::on_readable)), so a
//! session needs no thread of its own beyond the two of its PTY, and an idle
//! session costs nothing.
//!
//! **Batching.** Output is collected for [`BATCH_WINDOW`] and sent as one
//! batch of at most [`BATCH_MAX_BYTES`]. A batch ends at a clean place of the
//! stream when one can be found without waiting longer than
//! [`BOUNDARY_HOLD_MAX`]: not inside a UTF-8 character, not inside an escape
//! sequence, and not inside a synchronized update (DEC private mode 2026),
//! so that a frontend which paints between two batches never shows half a
//! frame. This is best effort: a sequence or an update larger than a batch,
//! or slower than the hold, is cut. A terminal emulator copes with that,
//! because its parser keeps its state between writes.
//!
//! Bytes are passed through **unmodified**, in order, and none is dropped.
//!
//! **Flow control.** The frontend calls [`SessionManager::acknowledge`] with
//! the number of bytes it has finished processing (for xterm.js: in the
//! callback of `write`, not on receipt). While more than
//! [`HIGH_WATERMARK_BYTES`] are unacknowledged, the pump takes nothing from
//! the PTY: the bounded buffer fills up, the reader thread stops reading, the
//! kernel's buffer fills up and the process blocks in `write`. The pump
//! resumes once fewer than [`LOW_WATERMARK_BYTES`] are unacknowledged. A
//! frontend should batch its acknowledgements by [`ACK_BATCH_BYTES`].
//!
//! A frontend that never acknowledges therefore stalls its session and
//! nothing else. The memory a session can hold is bounded whatever the
//! process prints: about [`HIGH_WATERMARK_BYTES`] plus one batch in flight to
//! the frontend, at most one batch in the pump, and the buffer of the PTY,
//! which a session shrinks to [`PTY_BUFFER_BYTES`].
//!
//! **When acknowledgements stop.** A session that stays paused for
//! [`STALL_TIMEOUT`] without a single acknowledgement, while its sink is
//! alive, is not left stuck for ever: one warning is logged and the
//! outstanding bytes are written off, which lets one more high-water mark of
//! output through. This repeats at most [`STALL_WRITE_OFFS_MAX`] times in a
//! row; any acknowledgement starts the count again. A frontend that lost an
//! acknowledgement therefore recovers by itself, a frontend that is slow is
//! fed at a trickle, and one that is truly dead costs a bounded amount of
//! memory before its session stops for good.
//!
//! The pump never takes more from the PTY than it may pass on. That keeps
//! the output still to come after the user interrupts a program that floods
//! the terminal to about what was already on its way, instead of megabytes
//! parked in buffers.
//!
//! # The end of a session
//!
//! When the process exits, the remaining output is delivered, then
//! [`SessionSink::exited`] is called with the exit status, and the session
//! is removed from the manager: its identifier is unknown from then on.
//! Since the frontend may be behind, the exit is only reported once the
//! last output could be handed over under flow control.
//!
//! [`SessionManager::close`] ends a session on the application's initiative:
//! the process is killed and the sink is dropped without being told about an
//! exit. [`SessionManager::shutdown`] does that for every session.

mod batch;
mod boundary;
mod driver;
mod flow;
mod manager;
mod pump;

pub use batch::{BATCH_MAX_BYTES, BATCH_WINDOW, BOUNDARY_HOLD_MAX};
pub use flow::{ACK_BATCH_BYTES, HIGH_WATERMARK_BYTES, LOW_WATERMARK_BYTES};
pub use manager::{SHUTDOWN_CLOSE_WAIT, SessionManager, SessionOptions};
pub use pump::{PTY_BUFFER_BYTES, STALL_TIMEOUT, STALL_WRITE_OFFS_MAX};

use crate::pty::ExitStatus;
pub use crate::pty::SessionId;

/// Receives what a session produces: its output and, at the end, how its
/// process exited.
///
/// Both methods are called on the manager's driver thread, which serves
/// every session. They must return quickly and must not block: hand the
/// data over to the frontend's transport and return. They must not call
/// back into the [`SessionManager`] either.
///
/// The sink is dropped when the session ends, is closed, or when the manager
/// is shut down.
///
/// **Never log the bytes**: they are the content of the user's terminal.
pub trait SessionSink: Send + 'static {
    /// A batch of output, to be written to the terminal emulator as it is.
    ///
    /// Batches arrive in order. A batch is never empty and never larger than
    /// [`BATCH_MAX_BYTES`]. Acknowledge its length with
    /// [`SessionManager::acknowledge`] once the frontend has processed it.
    fn output(&mut self, session: SessionId, bytes: Vec<u8>);

    /// The process of the session has exited and all its output was
    /// delivered. Called at most once, and nothing is called after it.
    ///
    /// It is not called for a session that was closed or shut down.
    fn exited(&mut self, session: SessionId, status: ExitStatus);

    /// Whether output can still reach the frontend. A sink that knows its
    /// transport is gone returns `false`, which stops the session from
    /// resuming on its own when acknowledgements stop coming (see
    /// [`STALL_TIMEOUT`]): there is nobody to resume for.
    fn is_alive(&self) -> bool {
        true
    }
}
