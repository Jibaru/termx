//! The limit on open file descriptors, which caps the number of PTYs.

/// The soft limit on open file descriptors that
/// [`raise_open_file_limit`] aims for.
///
/// It is deliberately moderate rather than "as high as allowed": the limit
/// is inherited by every process spawned in a PTY, and some programs slow
/// down or misbehave with a huge limit (they loop over every possible
/// descriptor, or rely on `select`).
pub const OPEN_FILE_LIMIT_TARGET: u64 = 10_240;

/// Raises the soft limit on open file descriptors of the current process
/// towards [`OPEN_FILE_LIMIT_TARGET`], without exceeding the hard limit.
///
/// **Call this once when the application starts.** On Unix each PTY keeps
/// file descriptors open: the terminal, a duplicate used for reading and
/// writing, and the two ends of the pipe that stops the reader thread, which
/// makes four on Linux. macOS needs a fifth, which keeps the terminal open so
/// that the last output of a process is never lost. macOS also gives a
/// process started from the Finder a soft limit of only 256 descriptors,
/// which the rest of the application shares: without raising it, spawning
/// fails after roughly 50 terminals.
///
/// Resulting capacity, approximately:
///
/// | Soft limit                  | PTYs on Linux | PTYs on macOS |
/// | --------------------------- | ------------- | ------------- |
/// | 256 (macOS default)         | 60            | 50            |
/// | 1,024 (usual Linux default) | 250           | 200           |
/// | 10,240 (after this call)    | 2,500         | 2,000         |
///
/// On macOS, terminals whose descriptors are numbered 1,024 or above (beyond
/// roughly 200 PTYs) still work but lose one guarantee: their reader thread
/// can no longer be interrupted, because the `select`-based wait macOS needs
/// does not support such descriptors. Their output stream then only ends
/// once every process holding the terminal has exited.
///
/// The limit is never lowered: if it is already at or above the target, it
/// is left alone. Processes spawned afterwards inherit the new limit.
///
/// Returns the soft limit now in effect, or `None` on Windows, where handles
/// are not subject to such a limit and this function does nothing.
///
/// # Errors
///
/// Returns the error of the operating system if the limit cannot be read or
/// changed. The application can keep running; only its capacity is lower.
///
/// # Examples
///
/// ```
/// if let Err(error) = termx_core::pty::raise_open_file_limit() {
///     eprintln!("could not raise the open file limit: {error}");
/// }
/// ```
pub fn raise_open_file_limit() -> std::io::Result<Option<u64>> {
    #[cfg(unix)]
    {
        rlimit::increase_nofile_limit(OPEN_FILE_LIMIT_TARGET).map(Some)
    }
    #[cfg(not(unix))]
    {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn soft_limit_is_raised_towards_the_target_and_never_lowered() {
        let (before, hard) = rlimit::Resource::NOFILE.get().unwrap();

        let reported = raise_open_file_limit().unwrap().unwrap();

        let (after, hard_after) = rlimit::Resource::NOFILE.get().unwrap();
        assert_eq!(hard_after, hard, "the hard limit must not change");
        assert!(
            after >= before,
            "the limit was lowered: {before} -> {after}"
        );
        if before < OPEN_FILE_LIMIT_TARGET && before < hard {
            assert!(after > before, "the limit was not raised from {before}");
            assert_eq!(reported, after);
        }
        assert!(after <= hard);

        // Calling it again changes nothing.
        raise_open_file_limit().unwrap();
        assert_eq!(rlimit::Resource::NOFILE.get().unwrap().0, after);
    }

    #[cfg(windows)]
    #[test]
    fn nothing_is_done_on_windows() {
        assert_eq!(raise_open_file_limit().unwrap(), None);
    }
}
