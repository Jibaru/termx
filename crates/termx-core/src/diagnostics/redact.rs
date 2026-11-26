//! Redaction of personal data before it reaches the log.
//!
//! The personal detail termx comes across by accident is the name of the
//! user, mostly as part of the home directory. Every record written by the
//! [`diagnostics`](super) module passes through a [`Redactor`], so call sites
//! do not have to remember. The helpers here are for values that leave
//! through another door, such as an error message shown in the interface.
//!
//! # What is replaced
//!
//! 1. **The home directory of the current user becomes `~`**, in any of the
//!    spellings it is known to take: with `/`, `\`, doubled backslashes (as
//!    printed by `{:?}`) or mixed separators; percent-encoded in whole or in
//!    part, as in URLs (`/Users/ana%20maria`, `C%3A/Users/Ana`); as seen
//!    from WSL (`/mnt/c/Users/Ana`); and in any letter case on Windows and
//!    macOS, whose file systems ignore case.
//! 2. **Any other path that has the shape of a home directory keeps its
//!    shape but loses the name**: `/Users/<user>`, `/home/<user>`,
//!    `C:\Users\<user>`, `/mnt/c/Users/<user>`. This covers other accounts
//!    and the home directory of the machine a binary was built on, which
//!    shows up in backtraces. The name ends at the next separator, quote,
//!    whitespace or bracket, so a name with spaces is only cut at the first
//!    space unless it is the current user's.
//! 3. **The bare name of the current user becomes `<user>`** when it stands
//!    as a whole word, in any letter case. To avoid mangling ordinary text
//!    this only applies to names of at least [`MIN_USER_NAME_CHARS`]
//!    characters that are not generic account names or common log words
//!    (`admin`, `user`, `root`, `error`, ...).
//!
//! # What is not
//!
//! Redaction is pattern matching, not understanding. A home directory in an
//! unusual place is only recognised for the current user, a short or generic
//! user name is not replaced outside a path, and any other personal data in
//! a message is not recognised at all. The rule that protects that is the
//! one about what must never be logged in the first place.

use std::borrow::Cow;
use std::path::Path;
use std::sync::OnceLock;

/// Shortest user name that is replaced when it appears outside a path.
pub const MIN_USER_NAME_CHARS: usize = 4;

/// What a redacted user name is replaced with.
pub const USER_PLACEHOLDER: &str = "<user>";

/// Names that are never replaced outside a path: generic accounts, and words
/// that are too common in logs.
const COMMON_NAMES: &[&str] = &[
    "admin",
    "administrator",
    "debug",
    "default",
    "error",
    "false",
    "guest",
    "home",
    "info",
    "local",
    "main",
    "none",
    "null",
    "owner",
    "public",
    "root",
    "runner",
    "shared",
    "system",
    "termx",
    "test",
    "tests",
    "trace",
    "true",
    "ubuntu",
    "unknown",
    "user",
    "users",
    "warn",
];

/// A home directory, split so that it can be matched in any spelling.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Home {
    /// Lower-case drive letter, for Windows paths.
    drive: Option<u8>,
    /// The directories below the root.
    components: Vec<String>,
}

/// Replaces the home directory and the name of the user.
///
/// See the [module documentation](self) for the rules.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Redactor {
    homes: Vec<Home>,
    /// Home directories that could not be split, matched as plain text.
    literals: Vec<String>,
    /// Names replaced when they appear as a whole word.
    user_names: Vec<String>,
    ignore_ascii_case: bool,
    /// Whether rule 2 (home-shaped paths of anyone) is applied.
    any_user_dirs: bool,
}

impl Redactor {
    /// A redactor for the given home directory, comparing paths the way the
    /// current platform does (ignoring case on Windows and macOS).
    #[must_use]
    pub fn new(home: Option<&Path>) -> Self {
        let case_sensitive = !cfg!(any(windows, target_os = "macos"));
        Self::with_case_sensitivity(home, case_sensitive)
    }

    /// A redactor for the given home directory with an explicit choice of
    /// case sensitivity.
    #[must_use]
    pub fn with_case_sensitivity(home: Option<&Path>, case_sensitive: bool) -> Self {
        let mut redactor = Self {
            ignore_ascii_case: !case_sensitive,
            any_user_dirs: true,
            ..Self::default()
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
    /// canonical spelling when that differs (symbolic links).
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
        let Some(text) = home.to_str() else {
            return;
        };
        // The verbatim prefix that `canonicalize` adds on Windows.
        let text = text.strip_prefix(r"\\?\").unwrap_or(text);
        let text = text.trim_end_matches(['/', '\\']);
        let bytes = text.as_bytes();

        let (drive, rest) =
            if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
                (Some(bytes[0].to_ascii_lowercase()), &text[2..])
            } else {
                (None, text)
            };
        if !rest.starts_with(['/', '\\']) {
            // Not an absolute path we know how to split (a network share,
            // for example): match it as it is, if it is long enough to be
            // safe.
            if text.len() >= 4 && !self.literals.iter().any(|known| known == text) {
                self.literals.push(text.to_owned());
            }
            return;
        }

        let components: Vec<String> = rest
            .split(['/', '\\'])
            .filter(|component| !component.is_empty())
            .map(str::to_owned)
            .collect();
        // A home directory that is a filesystem root would turn every path
        // into `~`, so it is ignored.
        let Some(name) = components.last() else {
            return;
        };

        let common = COMMON_NAMES
            .iter()
            .any(|common| name.eq_ignore_ascii_case(common));
        if name.chars().count() >= MIN_USER_NAME_CHARS
            && !common
            && !self.user_names.iter().any(|known| known == name)
        {
            self.user_names.push(name.clone());
        }

        let home = Home { drive, components };
        if !self.homes.contains(&home) {
            self.homes.push(home);
            // Longest first, so that a home inside another one wins.
            self.homes
                .sort_by_key(|home| std::cmp::Reverse(home.components.len()));
        }
    }

    /// Returns `input` with the home directory and the user name replaced.
    #[must_use]
    pub fn text<'a>(&self, input: &'a str) -> Cow<'a, str> {
        if *self == Self::default() {
            return Cow::Borrowed(input);
        }

        let bytes = input.as_bytes();
        let mut output: Option<String> = None;
        let mut copied = 0;
        let mut index = 0;
        while index < bytes.len() {
            // (start of the part to replace, its end, the replacement)
            let found = if let Some(end) = self.match_home(bytes, index) {
                Some((index, end, "~"))
            } else if let Some((start, end)) = self.match_user_dir(bytes, index) {
                Some((start, end, USER_PLACEHOLDER))
            } else {
                self.match_user_name(bytes, index)
                    .map(|end| (index, end, USER_PLACEHOLDER))
            };

            if let Some((start, end, replacement)) = found {
                let output = output.get_or_insert_with(|| String::with_capacity(input.len()));
                // Matches start and end next to ASCII bytes or whole
                // components, so these are character boundaries.
                output.push_str(&input[copied..start]);
                output.push_str(replacement);
                index = end;
                copied = end;
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

    /// End of the current user's home directory starting at `index`, if any.
    fn match_home(&self, haystack: &[u8], index: usize) -> Option<usize> {
        // The match must end where the directory name ends, which includes
        // a percent-encoded separator.
        let whole = |end: usize| {
            haystack
                .get(end)
                .is_none_or(|next| !continues_a_name(*next))
                || match_separator(haystack, end).is_some()
        };

        for home in &self.homes {
            let Some(mut position) = (match home.drive {
                Some(letter) => match_drive(haystack, index).filter(|(found, _)| *found == letter),
                None => Some((0, index)),
            })
            .map(|(_, position)| position) else {
                continue;
            };
            let matched = home.components.iter().all(|component| {
                let Some(after) = match_separator(haystack, position)
                    .and_then(|start| self.match_component(haystack, start, component.as_bytes()))
                else {
                    return false;
                };
                position = after;
                true
            });
            if matched && whole(position) {
                return Some(position);
            }
        }

        self.literals.iter().find_map(|literal| {
            let end = self.match_component(haystack, index, literal.as_bytes())?;
            whole(end).then_some(end)
        })
    }

    /// Matches `component` at `position`, where every byte may be written as
    /// it is or percent-encoded.
    fn match_component(
        &self,
        haystack: &[u8],
        mut position: usize,
        component: &[u8],
    ) -> Option<usize> {
        for expected in component {
            let found = *haystack.get(position)?;
            let equal = if self.ignore_ascii_case {
                found.eq_ignore_ascii_case(expected)
            } else {
                found == *expected
            };
            if equal {
                position += 1;
            } else if found == b'%' && percent_byte(haystack, position) == Some(*expected) {
                position += 3;
            } else {
                return None;
            }
        }
        Some(position)
    }

    /// The name in a path shaped like a home directory starting at `index`:
    /// `/Users/<name>`, `/home/<name>`, `C:\Users\<name>` and friends.
    fn match_user_dir(&self, haystack: &[u8], index: usize) -> Option<(usize, usize)> {
        if !self.any_user_dirs {
            return None;
        }
        let drive = match_drive(haystack, index);
        let position = drive.map_or(index, |(_, position)| position);
        let position = match_separator(haystack, position)?;
        let position = match_keyword(haystack, position, b"users").or_else(|| {
            // `/home` is not where Windows keeps its users.
            drive
                .is_none()
                .then(|| match_keyword(haystack, position, b"home"))
                .flatten()
        })?;
        let start = match_separator(haystack, position)?;

        let mut end = start;
        while end < haystack.len()
            && !ends_a_name(haystack[end])
            && !matches!(percent_byte(haystack, end), Some(b'/' | b'\\'))
        {
            end += 1;
        }
        (end > start).then_some((start, end))
    }

    /// End of the current user's name standing as a whole word at `index`.
    fn match_user_name(&self, haystack: &[u8], index: usize) -> Option<usize> {
        let is_word = |byte: &u8| byte.is_ascii_alphanumeric() || *byte == b'_' || *byte >= 0x80;
        if index > 0 && is_word(&haystack[index - 1]) {
            return None;
        }
        self.user_names.iter().find_map(|name| {
            let end = index + name.len();
            let candidate = haystack.get(index..end)?;
            let whole = haystack.get(end).is_none_or(|next| !is_word(next));
            (candidate.eq_ignore_ascii_case(name.as_bytes()) && whole).then_some(end)
        })
    }
}

/// Matches a drive at `index` and returns its lower-case letter and where the
/// rest of the path starts: `C:`, `C%3A`, or `/mnt/c` as WSL shows it.
fn match_drive(haystack: &[u8], index: usize) -> Option<(u8, usize)> {
    let rest = haystack.get(index..)?;
    match rest {
        [letter, b':', ..] if letter.is_ascii_alphabetic() => {
            Some((letter.to_ascii_lowercase(), index + 2))
        }
        [letter, b'%', b'3', b'A' | b'a', ..] if letter.is_ascii_alphabetic() => {
            Some((letter.to_ascii_lowercase(), index + 4))
        }
        [b'/', b'm', b'n', b't', b'/', letter, ..] if letter.is_ascii_alphabetic() => {
            Some((letter.to_ascii_lowercase(), index + 6))
        }
        _ => None,
    }
}

/// Matches one path separator at `position` in any spelling: `/`, `\`, a
/// doubled backslash, or either of them percent-encoded.
fn match_separator(haystack: &[u8], position: usize) -> Option<usize> {
    match haystack.get(position..)? {
        [b'\\', b'\\', ..] => Some(position + 2),
        [b'/' | b'\\', ..] => Some(position + 1),
        _ => matches!(percent_byte(haystack, position), Some(b'/' | b'\\')).then_some(position + 3),
    }
}

/// Matches the lower-case `keyword` at `position`, ignoring case.
fn match_keyword(haystack: &[u8], position: usize, keyword: &[u8]) -> Option<usize> {
    let end = position + keyword.len();
    haystack
        .get(position..end)?
        .eq_ignore_ascii_case(keyword)
        .then_some(end)
}

/// The byte written as `%XX` at `position`, if there is one.
fn percent_byte(haystack: &[u8], position: usize) -> Option<u8> {
    let [b'%', high, low] = haystack.get(position..position + 3)? else {
        return None;
    };
    let digit = |byte: &u8| char::from(*byte).to_digit(16);
    u8::try_from(digit(high)? * 16 + digit(low)?).ok()
}

/// Whether `byte` right after a matched home directory means that the last
/// component goes on, so it is a different directory (`/home/anabel`).
fn continues_a_name(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'%') || byte >= 0x80
}

/// Whether `byte` cannot be part of the user name in a home-shaped path.
fn ends_a_name(byte: u8) -> bool {
    byte.is_ascii_whitespace()
        || byte.is_ascii_control()
        || matches!(
            byte,
            b'/' | b'\\'
                | b'"'
                | b'\''
                | b'`'
                | b':'
                | b';'
                | b','
                | b'<'
                | b'>'
                | b'|'
                | b'*'
                | b'?'
                | b'='
                | b'('
                | b')'
                | b'['
                | b']'
                | b'{'
                | b'}'
        )
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

/// Returns `input` with the home directory and the name of the current user
/// replaced wherever they appear.
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

    fn mac() -> Redactor {
        Redactor::with_case_sensitivity(Some(Path::new("/Users/Ana Maria")), false)
    }

    fn windows() -> Redactor {
        Redactor::with_case_sensitivity(Some(Path::new(r"C:\Users\Anabel")), false)
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
    fn text_without_anything_personal_is_borrowed_unchanged() {
        let redacted = unix().text("/var/log/syslog — nothing to see, C: is a drive");
        assert!(matches!(redacted, Cow::Borrowed(_)));
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
    fn a_name_with_spaces_is_matched_whole_and_percent_encoded() {
        let redactor = mac();
        assert_eq!(redactor.text("/Users/Ana Maria/x"), "~/x");
        assert_eq!(
            redactor.text("file:///Users/Ana%20Maria/app.js"),
            "file://~/app.js"
        );
        // Encoded more than necessary, and in the other hex case.
        assert_eq!(redactor.text("/Users/%41na%20Maria/x"), "~/x");
        assert_eq!(redactor.text("%2fUsers%2FAna%20Maria%2Fx"), "~%2Fx");
    }

    #[test]
    fn case_is_ignored_where_the_file_system_ignores_it() {
        assert_eq!(mac().text("/users/ana maria/x"), "~/x");
        assert_eq!(mac().text("/USERS/ANA%20MARIA"), "~");
        assert_eq!(windows().text(r"c:\users\anabel\file.txt"), r"~\file.txt");
        // Where case matters, a different case is a different directory. It
        // still has the shape of a home directory, so the name goes.
        assert_eq!(unix().text("/HOME/ANA/x"), "/HOME/<user>/x");
    }

    #[test]
    fn windows_paths_match_with_any_separator() {
        let redactor = windows();
        assert_eq!(
            redactor.text(r"C:\Users\Anabel\AppData\Local"),
            r"~\AppData\Local"
        );
        assert_eq!(redactor.text("C:/Users/Anabel/x"), "~/x");
        assert_eq!(redactor.text(r"C:/Users\Anabel/x"), "~/x");
        // The spelling produced by `{:?}` on a path.
        assert_eq!(redactor.text(r#""C:\\Users\\Anabel\\x""#), r#""~\\x""#);
        // In a URL, with and without the drive colon encoded.
        assert_eq!(
            redactor.text("file:///C:/Users/Anabel/app.js"),
            "file:///~/app.js"
        );
        assert_eq!(
            redactor.text("file:///C%3A/Users/Anabel/app.js"),
            "file:///~/app.js"
        );
        // As WSL shows the same directory.
        assert_eq!(redactor.text("/mnt/c/Users/Anabel/x"), "~/x");
        // The verbatim form returned by `canonicalize`.
        assert_eq!(redactor.text(r"\\?\C:\Users\Anabel\x"), r"\\?\~\x");
    }

    #[test]
    fn the_verbatim_prefix_of_a_canonical_home_is_ignored() {
        let redactor =
            Redactor::with_case_sensitivity(Some(Path::new(r"\\?\C:\Users\Anabel")), false);
        assert_eq!(redactor, windows());
    }

    #[test]
    fn non_ascii_names_are_matched_raw_and_encoded() {
        let redactor = Redactor::with_case_sensitivity(Some(Path::new("/home/José")), true);
        assert_eq!(redactor.text("/home/José/x"), "~/x");
        assert_eq!(redactor.text("/home/Jos%C3%A9/x"), "~/x");
        assert_eq!(redactor.text("/home/Joséphine/x"), "/home/<user>/x");
        assert_eq!(
            redactor.text("hola José, ¿qué tal?"),
            "hola <user>, ¿qué tal?"
        );
    }

    #[test]
    fn home_shaped_paths_of_other_users_lose_the_name() {
        let redactor = unix();
        assert_eq!(redactor.text("/home/anabel/file"), "/home/<user>/file");
        assert_eq!(redactor.text("/home/ana-work/file"), "/home/<user>/file");
        assert_eq!(redactor.text("/home/ana%20maria/x"), "/home/<user>/x");
        assert_eq!(
            redactor.text("/Users/builder/.cargo/registry"),
            "/Users/<user>/.cargo/registry"
        );
        assert_eq!(redactor.text(r"C:\Users\Bob\x"), r"C:\Users\<user>\x");
        assert_eq!(redactor.text(r"D:\\Users\\Bob\\x"), r"D:\\Users\\<user>\\x");
        assert_eq!(redactor.text("C%3A/Users/Bob/x"), "C%3A/Users/<user>/x");
        assert_eq!(redactor.text("/mnt/d/Users/Bob"), "/mnt/d/Users/<user>");
        assert_eq!(
            redactor.text("at (/Users/bob), then"),
            "at (/Users/<user>), then"
        );
        assert_eq!(
            redactor.text("open '/home/bob': denied"),
            "open '/home/<user>': denied"
        );
    }

    #[test]
    fn a_backtrace_from_another_machine_does_not_keep_its_home_directory() {
        let frame = "   4: termx::run\n             at /Users/builder/src/termx/main.rs:12:5\n";
        assert_eq!(
            unix().text(frame),
            "   4: termx::run\n             at /Users/<user>/src/termx/main.rs:12:5\n"
        );
    }

    #[test]
    fn redacting_twice_changes_nothing_more() {
        let redactor = windows();
        let once = redactor
            .text(r"C:\Users\Anabel\a and C:\Users\Bob\b by anabel")
            .into_owned();
        assert_eq!(once, r"~\a and C:\Users\<user>\b by <user>");
        assert_eq!(redactor.text(&once), once);
    }

    #[test]
    fn a_distinctive_user_name_is_replaced_as_a_whole_word() {
        let redactor = windows();
        assert_eq!(redactor.text("owner is anabel."), "owner is <user>.");
        assert_eq!(redactor.text("ANABEL@host"), "<user>@host");
        assert_eq!(redactor.text("user=Anabel;"), "user=<user>;");
        // Inside another word it is not the user name.
        assert_eq!(
            redactor.text("anabella and manabel"),
            "anabella and manabel"
        );
        assert_eq!(redactor.text("anabel_2"), "anabel_2");
    }

    #[test]
    fn short_and_generic_user_names_are_only_replaced_inside_paths() {
        // Three letters: too likely to be part of ordinary text.
        assert_eq!(unix().text("ana wrote banana"), "ana wrote banana");

        for name in ["admin", "user", "root", "runner", "Administrator", "error"] {
            let home = format!("/home/{name}");
            let redactor = Redactor::with_case_sensitivity(Some(Path::new(&home)), true);
            let text = format!("{name}: an {name} happened in /home/{name}/x");
            assert_eq!(
                redactor.text(&text),
                format!("{name}: an {name} happened in ~/x")
            );
        }
    }

    #[test]
    fn a_home_directory_in_an_unusual_place_is_matched_for_the_current_user() {
        let redactor = Redactor::with_case_sensitivity(Some(Path::new("/var/home/carmen")), true);
        assert_eq!(redactor.text("/var/home/carmen/x"), "~/x");
        assert_eq!(redactor.text("/data/carmen/x"), "/data/<user>/x");
    }

    #[test]
    fn a_root_or_missing_home_directory_only_leaves_the_generic_rule() {
        for home in [Some(Path::new("/")), Some(Path::new("")), None] {
            let redactor = Redactor::with_case_sensitivity(home, true);
            assert_eq!(redactor.text("/etc/hosts"), "/etc/hosts");
            assert_eq!(redactor.text("/home/bob/x"), "/home/<user>/x");
        }
    }

    #[test]
    fn a_disabled_redactor_changes_nothing() {
        assert_eq!(Redactor::disabled().text("/home/bob/x"), "/home/bob/x");
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
