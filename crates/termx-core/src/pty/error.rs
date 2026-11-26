//! The error type of the PTY layer.

use std::io;
use std::path::PathBuf;

/// Everything that can go wrong while spawning or driving a PTY.
///
/// The enum is `#[non_exhaustive]`: new variants may be added without a
/// breaking change, so always keep a wildcard arm when matching on it.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PtyError {
    /// The command has an empty program name.
    #[error("the program to run is empty")]
    EmptyProgram,

    /// A login shell was given extra arguments.
    ///
    /// A login shell is started by prefixing its `argv[0]` with `-`, which
    /// leaves no portable way of passing it further arguments.
    #[error("a login shell cannot be given arguments")]
    LoginShellArguments,

    /// The requested terminal size cannot be represented by the platform.
    #[error("invalid terminal size {cols}x{rows}: both dimensions must be between 1 and {max}", max = super::PtySize::MAX_DIMENSION)]
    InvalidSize {
        /// Requested number of columns.
        cols: u16,
        /// Requested number of rows.
        rows: u16,
    },

    /// The working directory does not exist or is not a directory.
    #[error("working directory {} does not exist or is not a directory", .0.display())]
    InvalidWorkingDirectory(PathBuf),

    /// The operating system refused to create a pseudo-terminal.
    #[error("failed to open a pseudo-terminal: {0}")]
    Open(String),

    /// The program could not be started.
    #[error("failed to spawn {program:?}: {message}")]
    Spawn {
        /// The program that was being started.
        program: String,
        /// The reason reported by the operating system.
        message: String,
    },

    /// A helper thread or one of the PTY streams could not be set up.
    #[error("failed to set up the pseudo-terminal streams: {0}")]
    Setup(String),

    /// Writing input to the PTY failed.
    #[error("failed to write to the pseudo-terminal")]
    Write(#[source] io::Error),

    /// Resizing the PTY failed.
    #[error("failed to resize the pseudo-terminal: {0}")]
    Resize(String),

    /// The process could not be killed.
    #[error("failed to kill the process")]
    Kill(#[source] io::Error),

    /// The operation needs a running process, but it has already exited.
    #[error("the process has already exited")]
    Exited,
}
