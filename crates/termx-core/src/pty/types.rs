//! Small value types shared by the PTY layer.

use super::PtyError;

/// Size of a terminal in character cells.
///
/// # Examples
///
/// ```
/// use termx_core::pty::PtySize;
///
/// let size = PtySize::new(120, 40);
/// assert_eq!((size.cols, size.rows), (120, 40));
/// assert_eq!(PtySize::default(), PtySize::new(80, 24));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PtySize {
    /// Number of columns (width).
    pub cols: u16,
    /// Number of rows (height).
    pub rows: u16,
}

impl PtySize {
    /// Largest value accepted for either dimension.
    ///
    /// The Windows pseudo-console stores sizes in signed 16-bit integers, so
    /// the same limit is enforced everywhere to keep behaviour identical
    /// across platforms.
    pub const MAX_DIMENSION: u16 = i16::MAX as u16;

    /// Creates a size of `cols` columns by `rows` rows.
    #[must_use]
    pub const fn new(cols: u16, rows: u16) -> Self {
        Self { cols, rows }
    }

    /// Checks that both dimensions are within `1..=MAX_DIMENSION`.
    pub(super) fn validate(self) -> Result<Self, PtyError> {
        let valid = |n: u16| (1..=Self::MAX_DIMENSION).contains(&n);
        if valid(self.cols) && valid(self.rows) {
            Ok(self)
        } else {
            Err(PtyError::InvalidSize {
                cols: self.cols,
                rows: self.rows,
            })
        }
    }

    pub(super) fn to_native(self) -> portable_pty::PtySize {
        portable_pty::PtySize {
            rows: self.rows,
            cols: self.cols,
            pixel_width: 0,
            pixel_height: 0,
        }
    }
}

impl Default for PtySize {
    /// The classic 80x24 terminal.
    fn default() -> Self {
        Self::new(80, 24)
    }
}

/// How a process running in a PTY ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitStatus {
    code: Option<u32>,
    signal: Option<String>,
}

impl ExitStatus {
    /// Status used when the operating system could not report how the
    /// process ended (for example because something else reaped it).
    pub(super) const fn unknown() -> Self {
        Self {
            code: None,
            signal: None,
        }
    }

    /// Returns `true` if the process exited with code 0.
    #[must_use]
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }

    /// The exit code of the process.
    ///
    /// `None` if the process was terminated by a signal (Unix) or if the
    /// operating system could not report its status.
    #[must_use]
    pub fn code(&self) -> Option<u32> {
        self.code
    }

    /// Description of the signal that terminated the process, as reported by
    /// the operating system (for example `"Killed: 9"` or `"Hangup"`).
    ///
    /// Always `None` on Windows, where a killed process reports an exit code
    /// instead.
    #[must_use]
    pub fn signal(&self) -> Option<&str> {
        self.signal.as_deref()
    }
}

impl From<portable_pty::ExitStatus> for ExitStatus {
    fn from(status: portable_pty::ExitStatus) -> Self {
        match status.signal() {
            Some(signal) => Self {
                code: None,
                signal: Some(signal.to_owned()),
            },
            None => Self {
                code: Some(status.exit_code()),
                signal: None,
            },
        }
    }
}

impl std::fmt::Display for ExitStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (&self.signal, self.code) {
            (Some(signal), _) => write!(f, "terminated by signal ({signal})"),
            (None, Some(code)) => write!(f, "exited with code {code}"),
            (None, None) => write!(f, "exited with an unknown status"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_size_is_80_by_24() {
        assert_eq!(PtySize::default(), PtySize { cols: 80, rows: 24 });
    }

    #[test]
    fn zero_and_oversized_dimensions_are_rejected() {
        for (cols, rows) in [(0, 24), (80, 0), (0, 0), (40_000, 24), (80, 40_000)] {
            let error = PtySize::new(cols, rows).validate().unwrap_err();
            assert!(
                matches!(error, PtyError::InvalidSize { cols: c, rows: r } if c == cols && r == rows),
                "unexpected error for {cols}x{rows}: {error}"
            );
        }
        assert!(PtySize::new(1, 1).validate().is_ok());
        assert!(
            PtySize::new(PtySize::MAX_DIMENSION, PtySize::MAX_DIMENSION)
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn exit_status_from_an_exit_code() {
        let ok = ExitStatus::from(portable_pty::ExitStatus::with_exit_code(0));
        assert!(ok.success());
        assert_eq!(ok.code(), Some(0));
        assert_eq!(ok.signal(), None);
        assert_eq!(ok.to_string(), "exited with code 0");

        let failed = ExitStatus::from(portable_pty::ExitStatus::with_exit_code(3));
        assert!(!failed.success());
        assert_eq!(failed.code(), Some(3));
    }

    #[test]
    fn exit_status_from_a_signal_has_no_code() {
        let killed = ExitStatus::from(portable_pty::ExitStatus::with_signal("Killed"));
        assert!(!killed.success());
        assert_eq!(killed.code(), None);
        assert_eq!(killed.signal(), Some("Killed"));
        assert_eq!(killed.to_string(), "terminated by signal (Killed)");
    }

    #[test]
    fn unknown_exit_status_is_not_a_success() {
        let unknown = ExitStatus::unknown();
        assert!(!unknown.success());
        assert_eq!(unknown.code(), None);
        assert_eq!(unknown.signal(), None);
    }
}
