//! Pseudo-terminal (PTY) handling.
//!
//! This module spawns processes attached to a pseudo-terminal and drives
//! them: it streams their output, sends them input, resizes the terminal,
//! kills them and reports how they exited. It is built on
//! [`portable-pty`](https://docs.rs/portable-pty) and works the same on Linux
//! and macOS (POSIX PTYs) and on Windows (ConPTY).
//!
//! # Overview
//!
//! - [`PtyCommand`] describes what to run: program, arguments, working
//!   directory and environment. [`PtyCommand::default_shell`] is the user's
//!   shell, resolved by [`default_shell`].
//! - [`Pty::spawn`] starts it and returns a [`Pty`] handle and a
//!   [`PtyOutput`] stream.
//! - [`PtyManager`] is an optional registry of PTYs keyed by an opaque
//!   [`SessionId`], convenient for an application that receives its commands
//!   over IPC.
//!
//! ```no_run
//! use termx_core::pty::{Pty, PtyCommand, PtySize};
//!
//! # fn main() -> Result<(), termx_core::pty::PtyError> {
//! let command = PtyCommand::new("git").args(["status", "--short"]).cwd("/tmp");
//! let (pty, output) = Pty::spawn(&command, PtySize::new(80, 24))?;
//!
//! let mut bytes = Vec::new();
//! for chunk in output {
//!     bytes.extend_from_slice(&chunk);
//! }
//! println!("{}", String::from_utf8_lossy(&bytes));
//! println!("git {}", pty.wait());
//! # Ok(())
//! # }
//! ```
//!
//! # Output stream
//!
//! Each PTY has a **reader thread** that moves bytes from the terminal into a
//! byte buffer shared with the [`PtyOutput`], bounded to
//! [`OUTPUT_BUFFER_CAPACITY`]. The consumer pulls from it with
//! [`PtyOutput::recv`], each call returning everything buffered so far.
//!
//! This pull-based, bounded design was chosen over a callback or an unbounded
//! channel because it gives the layer above both properties it needs for
//! free:
//!
//! - **Backpressure.** When the consumer falls behind, the buffer fills up,
//!   the reader thread stops reading, the kernel buffer fills up in turn and
//!   the process blocks in `write`. A program printing gigabytes (`yes`,
//!   `cat` of a huge file) cannot exhaust memory, however slow the UI is.
//! - **Batching.** A consumer that is slower than the process receives fewer
//!   and larger chunks instead of a backlog of small ones. A UI layer can
//!   simply forward each chunk it receives, and may sleep between two
//!   `recv` calls to cap the number of messages per second.
//!
//! The consumer chooses its own thread and pace; a callback receiving the
//! bytes would instead run on the reader thread and make its latency part of
//! the reading loop.
//!
//! A consumer does not need a thread of its own per terminal, though:
//! [`PtyOutput::on_readable`] registers a notification that there is
//! something to receive, so one event loop or async runtime can drain every
//! terminal with non-blocking calls.
//!
//! Bytes are passed through **unmodified** (with one exception at the very
//! start of a Windows session, described in the platform notes below). Chunk
//! boundaries are arbitrary
//! and can fall in the middle of a UTF-8 character or of an escape sequence,
//! so chunks must be fed to a stateful decoder, never interpreted one by one.
//!
//! # Lifecycle and shutdown
//!
//! A second helper thread, the **waiter**, observes the exit of the process
//! and publishes its [`ExitStatus`] ([`Pty::wait`], [`Pty::try_wait`]). When
//! the process exits, the terminal is closed, the output stream delivers what
//! is left and ends. A session ends with the process that was spawned in it:
//! background jobs it leaves behind do not keep the stream open (their later
//! output is discarded once the terminal has been silent for a moment).
//!
//! Dropping the [`Pty`] kills the process if needed, reaps it and joins both
//! threads, so no thread, zombie process or file descriptor outlives the
//! handle. Dropping the [`PtyOutput`] only discards the output.
//!
//! Each PTY uses four file descriptors on Linux and five on macOS, which only
//! allows 256 by default. Call [`raise_open_file_limit`] when the application starts; its
//! documentation gives the resulting capacity.
//!
//! # Platform notes
//!
//! - **Enter is `\r`.** A terminal sends a carriage return for the Enter
//!   key; the terminal driver or the console translates it as needed.
//! - **ConPTY is not transparent.** On Windows the output is not the raw
//!   bytes of the process but a rendering of the console's screen: escape
//!   sequences are added, runs of spaces may become cursor movements, and
//!   text may be repainted. Compare the visible content, not the bytes.
//! - **The ConPTY start-up query is answered here.** portable-pty creates
//!   the pseudo-console in a mode where it first asks its terminal where the
//!   cursor is (`ESC [ 6 n`) and, on Windows 10, prints nothing until it is
//!   told. So that a session never stalls when no terminal emulator is
//!   attached yet, this module answers that one question itself ("top-left
//!   corner", which is where a new terminal's cursor is) and removes it from
//!   the output. Because the terminal emulator never sees the question, it
//!   cannot answer it a second time. This is the only alteration ever made
//!   to the output, it is limited to the first few bytes of a Windows
//!   session, and every later query is passed through for the terminal
//!   emulator to answer by writing to the PTY.

mod command;
mod error;
#[cfg(windows)]
mod job;
mod limits;
mod manager;
mod output;
mod process;
mod reader;
mod shell;
#[cfg(unix)]
mod tree;
mod types;

pub use command::{
    COLORTERM, DEFAULT_ENV_DENYLIST, DEFAULT_KILL_GRACE, DEFAULT_LOCALE, PtyCommand, TERM,
    TERM_PROGRAM,
};
pub use error::PtyError;
pub use limits::{OPEN_FILE_LIMIT_TARGET, raise_open_file_limit};
pub use manager::{PtyManager, SessionId};
pub use output::{OUTPUT_BUFFER_CAPACITY, PtyOutput, RecvTimeoutError};
pub use process::Pty;
pub use reader::READ_BUFFER_SIZE;
pub use shell::default_shell;
pub use types::{ExitStatus, PtySize};

use std::sync::{Mutex, MutexGuard, PoisonError};

/// Locks a mutex, ignoring poisoning.
///
/// The state guarded by the mutexes of this crate is kept consistent by
/// single assignments, so it is still valid after a panic in another thread.
/// Recovering it keeps the crate free of panics of its own.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
