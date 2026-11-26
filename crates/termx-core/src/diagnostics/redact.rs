//! Redaction of personal data before it reaches the log.
//!
//! The only personal detail termx can come across by accident is the name of
//! the user, which is part of the home directory. Every record written by the
//! [`diagnostics`](super) module passes through a [`Redactor`] that replaces
//! the home directory with `~`, so call sites do not have to remember. The
//! helpers here are for values that leave through another door, such as an
//! error message shown in the interface.

use std::borrow::Cow;
use std::path::Path;
use std::sync::OnceLock;

/// Replaces the home directory of the user with `~`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Redactor {
    /// Spellings of the home directory, longest first.
    needles: Vec<String>,
    ignore_ascii_case: bool,
}

impl Redactor {
    /// A redactor for the given home directory, comparing paths the way the
    /// current platform does (ignoring case on Windows).
    #[must_use]
    pub fn new(home: Option<&Path>) -> Self {
        Self::with_case_sensitivity(home, !cfg!(windows))
    }

    /// A redactor for the given home directory with an explicit choice of
    /// case sensitivity.
    #[must_use]
    pub fn with_case_sensitivity(home: Option<&Path>, case_sensitive: bool) -> Self {
        let mut redactor = Self {
            needles: Vec::new(),
            ignore_ascii_case: !case_sensitive,
        };
        if let Some(home) = home {
            redactor.add_home(home);
        }
        redactor
    }

    /// A redactor that changes nothing.
    #[must_use]
    pub fn disabled() -> Self {
        Self::default()
    }

    /// A redactor for the home directory of the current user, including its
    /// canonical spelling when that differs (symbolic links, `\\?\` prefixes).
    #[must_use]
    pub fn for_current_user() -> Self {
        let home = std::env::home_dir();
        let mut redactor = Self::new(home.as_deref());
        if let Some(canonical) = home.and_then(|home| home.canonicalize().ok()) {
            redactor.add_home(&canonical);
        }
        redactor
    }

    fn add_home(&mut self, home: &Path) {
        // A home directory that is a filesystem root (or empty) would turn
        // every path into `~`, so it is ignored.
        if home.parent().is_none() {
            return;
        }
        let Some(home) = home.to_str() else {
            return;
        };
        let home = home.trim_end_matches(['/', '\\']);
        if home.len() < 2 {
            return;
        }

        let mut spellings = vec![home.to_owned()];
        if home.contains('\\') {
            // The same path as it appears in URLs and in tools that prefer
            // forward slashes, and as it appears in `{:?}` output.
            spellings.push(home.replace('\\', "/"));
            spellings.push(home.replace('\\', "\\\\"));
        }
        for spelling in spellings {
            if !self.needles.contains(&spelling) {
                self.needles.push(spelling);
            }
        }
        self.needles
            .sort_by_key(|needle| std::cmp::Reverse(needle.len()));
    }

    /// Replaces every occurrence of the home directory in `input` with `~`.
    ///
    /// The home directory is only replaced when it is followed by something
    /// that cannot be part of the same file name, so `/home/ana` is left
    /// alone inside `/home/anabel`.
    #[must_use]
    pub fn text<'a>(&self, input: &'a str) -> Cow<'a, str> {
        if self.needles.is_empty() {
            return Cow::Borrowed(input);
        }

        let bytes = input.as_bytes();
        let mut output: Option<String> = None;
        let mut copied = 0;
        let mut index = 0;
        while index < bytes.len() {
            if let Some(length) = self.match_at(bytes, index) {
                let output = output.get_or_insert_with(|| String::with_capacity(input.len()));
                output.push_str(&input[copied..index]);
                output.push('~');
                index += length;
                copied = index;
            } else {
                index += 1;
            }
        }

        match output {
            None => Cow::Borrowed(input),
            Some(mut output) => {
                output.push_str(&input[copied..]);
                Cow::Owned(output)
            }
        }
    }

    /// Returns `path` as text with the home directory replaced by `~`.
    ///
    /// Use this for every path that is logged or put in an error message.
    #[must_use]
    pub fn path(&self, path: &Path) -> String {
        self.text(&path.to_string_lossy()).into_owned()
    }

    /// Length of the home directory spelling that starts at `index`, if any.
    fn match_at(&self, haystack: &[u8], index: usize) -> Option<usize> {
        let rest = &haystack[index..];
        self.needles.iter().find_map(|needle| {
            let needle = needle.as_bytes();
            let candidate = rest.get(..needle.len())?;
            let equal = if self.ignore_ascii_case {
                candidate.eq_ignore_ascii_case(needle)
            } else {
                candidate == needle
            };
            let at_boundary = rest
                .get(needle.len())
                .is_none_or(|next| !(next.is_ascii_alphanumeric() || matches!(next, b'_' | b'-')));
            (equal && at_boundary).then_some(needle.len())
        })
    }
}

/// The redactor for the current user, created on first use.
pub fn redactor() -> &'static Redactor {
    static REDACTOR: OnceLock<Redactor> = OnceLock::new();
    REDACTOR.get_or_init(Redactor::for_current_user)
}

/// Returns `path` as text with the home directory of the current user
/// replaced by `~`.
///
/// # Examples
///
/// ```
/// let home = std::env::home_dir().unwrap_or_default();
/// let shown = termx_core::diagnostics::redact::path(home.join("project"));
/// assert!(!shown.is_empty());
/// ```
#[must_use]
pub fn path(path: impl AsRef<Path>) -> String {
    redactor().path(path.as_ref())
}

/// Returns `input` with the home directory of the current user replaced by
/// `~` wherever it appears.
#[must_use]
pub fn text(input: &str) -> Cow<'_, str> {
    redactor().text(input)
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::path::Path;

    use super::Redactor;

    fn unix() -> Redactor {
        Redactor::with_case_sensitivity(Some(Path::new("/home/ana")), true)
    }

    fn windows() -> Redactor {
        Redactor::with_case_sensitivity(Some(Path::new(r"C:\Users\Ana")), false)
    }

    #[test]
    fn the_home_directory_is_replaced_by_a_tilde() {
        assert_eq!(unix().text("/home/ana"), "~");
        assert_eq!(unix().text("/home/ana/projects/termx"), "~/projects/termx");
        assert_eq!(
            unix().path(Path::new("/home/ana/.config/termx")),
            "~/.config/termx"
        );
    }

    #[test]
    fn every_occurrence_inside_a_message_is_replaced() {
        let message = "cannot copy /home/ana/a.txt to '/home/ana/b.txt': denied (/home/ana).";
        assert_eq!(
            unix().text(message),
            "cannot copy ~/a.txt to '~/b.txt': denied (~)."
        );
    }

    #[test]
    fn file_urls_are_redacted() {
        assert_eq!(
            unix().text("file:///home/ana/x.js:10:2"),
            "file://~/x.js:10:2"
        );
    }

    #[test]
    fn other_directories_with_the_same_prefix_are_left_alone() {
        assert_eq!(unix().text("/home/anabel/file"), "/home/anabel/file");
        assert_eq!(unix().text("/home/ana-work/file"), "/home/ana-work/file");
        assert_eq!(unix().text("/home/ana_2"), "/home/ana_2");
    }

    #[test]
    fn text_without_the_home_directory_is_borrowed_unchanged() {
        let redacted = unix().text("/var/log/syslog — nothing to see");
        assert!(matches!(redacted, Cow::Borrowed(_)));
        assert_eq!(redacted, "/var/log/syslog — nothing to see");
    }

    #[test]
    fn non_ascii_text_around_the_home_directory_survives() {
        assert_eq!(unix().text("ñandú → /home/ana/café ✓"), "ñandú → ~/café ✓");
    }

    #[test]
    fn a_trailing_separator_on_the_home_directory_is_ignored() {
        let redactor = Redactor::with_case_sensitivity(Some(Path::new("/home/ana/")), true);
        assert_eq!(redactor.text("/home/ana/x"), "~/x");
    }

    #[test]
    fn windows_paths_match_in_any_case_and_with_any_separator() {
        let redactor = windows();
        assert_eq!(
            redactor.text(r"C:\Users\Ana\AppData\Local"),
            r"~\AppData\Local"
        );
        assert_eq!(redactor.text(r"c:\users\ana\file.txt"), r"~\file.txt");
        assert_eq!(
            redactor.text("file:///C:/Users/Ana/app.js"),
            "file:///~/app.js"
        );
        // The spelling produced by `{:?}` on a path.
        assert_eq!(redactor.text(r#""C:\\Users\\Ana\\x""#), r#""~\\x""#);
        assert_eq!(redactor.text(r"C:\Users\Anabel"), r"C:\Users\Anabel");
    }

    #[test]
    fn case_matters_when_the_platform_says_so() {
        assert_eq!(unix().text("/HOME/ANA/x"), "/HOME/ANA/x");
    }

    #[test]
    fn a_root_or_missing_home_directory_disables_redaction() {
        for home in [Some(Path::new("/")), Some(Path::new("")), None] {
            let redactor = Redactor::with_case_sensitivity(home, true);
            assert_eq!(redactor, Redactor::disabled());
            assert_eq!(redactor.text("/etc/hosts"), "/etc/hosts");
        }
    }

    #[test]
    fn the_current_user_redactor_hides_the_real_home_directory() {
        let Some(home) = std::env::home_dir() else {
            return;
        };
        if home.parent().is_none() {
            return;
        }
        let shown = super::path(home.join("termx-redaction-test"));
        assert!(shown.starts_with('~'), "{shown:?} should start with ~");
        assert!(!shown.contains(&*home.to_string_lossy()));
    }
}
