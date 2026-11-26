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
//! 1. **The home directory of the current user becomes `~`**, wherever it
//!    appears, in the spellings it is known to take: with `/`, `\`, any
//!    number of backslashes in a row (as `{:?}` and JSON print them) or mixed
//!    separators; percent-encoded in whole or in part, once or twice
//!    (`/Users/ana%20maria`, `C%3A/Users/Ana`, `%252Fhome%252Fana`); as seen
//!    from WSL (`/mnt/c/Users/Ana`); with accented letters composed or
//!    decomposed; and in any letter case on Windows and macOS, whose file
//!    systems ignore case.
//! 2. **Any other path that has the shape of a home directory keeps its
//!    shape but loses the name**: `/Users/<user>`, `/home/<user>`,
//!    `C:\Users\<user>`, `/mnt/c/Users/<user>`. This covers other accounts
//!    and the home directory of the machine a binary was built on, which
//!    shows up in backtraces. To leave things like `src/home/Home.tsx` and
//!    `/api/users/list` alone, the path must start where a path can start
//!    (the beginning of the text, or after white space, a quote, a bracket,
//!    `=`, `:`, `,`, `;` or `file://`), `/home/` and `/Users/` must be
//!    spelled exactly like that, and the Windows form needs a drive. The
//!    name ends at the next separator, quote, white space or bracket, so a
//!    name with spaces is only cut at the first space unless it is the
//!    current user's.
//! 3. **The bare name of the current user becomes `<user>`** when it stands
//!    as a whole word, in any letter case, percent-encoded or not. To avoid
//!    garbling ordinary text:
//!    - names shorter than [`MIN_USER_NAME_CHARS`] characters, generic
//!      account names and common words (`admin`, `node`, `command`, `code`,
//!      ...; see [`COMMON_NAMES`]) are never replaced outside a path;
//!    - names of fewer than [`MIN_DISTINCTIVE_NAME_CHARS`] characters are
//!      only replaced where a user name is expected: next to a path
//!      separator or an `@`, or as the value of something called `user`,
//!      `login`, `owner`, `account` or `author`;
//!    - longer names are replaced anywhere.
//!
//! # What is not
//!
//! Redaction is pattern matching, not understanding. A home directory in an
//! unusual place is only recognised for the current user, a short or common
//! user name is not replaced outside a path, a user who happens to be called
//! like an ordinary word that is not on the list will see that word replaced,
//! and any other personal data in a message is not recognised at all. The
//! rule that protects that is the one about what must never be logged in the
//! first place.

use std::borrow::Cow;
use std::path::Path;
use std::sync::OnceLock;

/// Shortest user name that is replaced when it appears outside a path.
pub const MIN_USER_NAME_CHARS: usize = 4;

/// Shortest user name that is replaced anywhere, and not only where a user
/// name is expected.
pub const MIN_DISTINCTIVE_NAME_CHARS: usize = 6;

/// What a redacted user name is replaced with.
pub const USER_PLACEHOLDER: &str = "<user>";

/// Names that are never replaced outside a path: generic accounts, and words
/// that are too common in the log of a terminal emulator.
pub const COMMON_NAMES: &[&str] = &[
    "account",
    "admin",
    "administrator",
    "agent",
    "android",
    "apple",
    "apps",
    "backend",
    "backup",
    "bash",
    "build",
    "cache",
    "cargo",
    "claude",
    "client",
    "code",
    "command",
    "config",
    "cursor",
    "daemon",
    "data",
    "debug",
    "default",
    "demo",
    "deploy",
    "desktop",
    "developer",
    "docker",
    "error",
    "example",
    "failed",
    "failure",
    "false",
    "family",
    "file",
    "files",
    "frontend",
    "github",
    "google",
    "guest",
    "home",
    "http",
    "https",
    "info",
    "input",
    "java",
    "javascript",
    "library",
    "linux",
    "local",
    "logging",
    "login",
    "logs",
    "macos",
    "main",
    "manager",
    "master",
    "message",
    "mobile",
    "mysql",
    "name",
    "network",
    "nobody",
    "node",
    "none",
    "null",
    "office",
    "operator",
    "oracle",
    "output",
    "owner",
    "panic",
    "path",
    "personal",
    "postgres",
    "private",
    "process",
    "public",
    "python",
    "react",
    "release",
    "root",
    "runner",
    "rust",
    "sample",
    "server",
    "service",
    "session",
    "shared",
    "shell",
    "source",
    "staff",
    "student",
    "support",
    "system",
    "target",
    "tauri",
    "temp",
    "terminal",
    "termx",
    "test",
    "tests",
    "thread",
    "trace",
    "true",
    "typescript",
    "ubuntu",
    "unknown",
    "user",
    "users",
    "vagrant",
    "void",
    "vscode",
    "warn",
    "wheel",
    "window",
    "windows",
    "work",
];

/// Names of things whose value is a user name.
const IDENTITY_KEYS: &[&str] = &["user", "username", "login", "owner", "account", "author"];

/// Precomposed Latin letters and what they decompose to: the letter and a
/// combining mark. macOS hands out file names decomposed, most other things
/// compose them, and a name must be found either way.
const DECOMPOSITIONS: &[(char, char, char)] = &[
    ('à', 'a', '\u{300}'),
    ('á', 'a', '\u{301}'),
    ('â', 'a', '\u{302}'),
    ('ã', 'a', '\u{303}'),
    ('ä', 'a', '\u{308}'),
    ('å', 'a', '\u{30a}'),
    ('ç', 'c', '\u{327}'),
    ('è', 'e', '\u{300}'),
    ('é', 'e', '\u{301}'),
    ('ê', 'e', '\u{302}'),
    ('ë', 'e', '\u{308}'),
    ('ì', 'i', '\u{300}'),
    ('í', 'i', '\u{301}'),
    ('î', 'i', '\u{302}'),
    ('ï', 'i', '\u{308}'),
    ('ñ', 'n', '\u{303}'),
    ('ò', 'o', '\u{300}'),
    ('ó', 'o', '\u{301}'),
    ('ô', 'o', '\u{302}'),
    ('õ', 'o', '\u{303}'),
    ('ö', 'o', '\u{308}'),
    ('ù', 'u', '\u{300}'),
    ('ú', 'u', '\u{301}'),
    ('û', 'u', '\u{302}'),
    ('ü', 'u', '\u{308}'),
    ('ý', 'y', '\u{301}'),
    ('ÿ', 'y', '\u{308}'),
    ('ć', 'c', '\u{301}'),
    ('č', 'c', '\u{30c}'),
    ('ě', 'e', '\u{30c}'),
    ('ń', 'n', '\u{301}'),
    ('ř', 'r', '\u{30c}'),
    ('ś', 's', '\u{301}'),
    ('š', 's', '\u{30c}'),
    ('ź', 'z', '\u{301}'),
    ('ž', 'z', '\u{30c}'),
];

/// A home directory, split so that it can be matched in any spelling.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Home {
    /// Lower-case drive letter, for Windows paths.
    drive: Option<u8>,
    /// The directories below the root, each in the spellings it can take.
    components: Vec<Vec<String>>,
}

/// The name of the user, as it is looked for outside paths.
#[derive(Debug, Clone, PartialEq, Eq)]
struct UserName {
    /// The spellings the name can take.
    spellings: Vec<String>,
    /// Distinctive enough to be replaced anywhere, not only where a user
    /// name is expected.
    anywhere: bool,
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
    user_names: Vec<UserName>,
    ignore_case: bool,
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
            ignore_case: !case_sensitive,
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

        let components: Vec<&str> = rest
            .split(['/', '\\'])
            .filter(|component| !component.is_empty())
            .collect();
        // A home directory that is a filesystem root would turn every path
        // into `~`, so it is ignored.
        let Some(name) = components.last() else {
            return;
        };

        let length = name.chars().count();
        let common = COMMON_NAMES
            .iter()
            .any(|common| name.eq_ignore_ascii_case(common));
        if length >= MIN_USER_NAME_CHARS && !common {
            let user_name = UserName {
                // A name is a name in any letter case.
                spellings: spellings(name, true),
                anywhere: length >= MIN_DISTINCTIVE_NAME_CHARS,
            };
            if !self.user_names.contains(&user_name) {
                self.user_names.push(user_name);
            }
        }

        let home = Home {
            drive,
            components: components
                .iter()
                .map(|component| spellings(component, self.ignore_case))
                .collect(),
        };
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
                    .and_then(|start| self.match_any(haystack, start, component))
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
            let end = match_text(haystack, index, literal.as_bytes(), self.ignore_case)?;
            whole(end).then_some(end)
        })
    }

    /// Matches any of `spellings` at `position`; the longest one wins.
    fn match_any(&self, haystack: &[u8], position: usize, spellings: &[String]) -> Option<usize> {
        spellings
            .iter()
            .filter_map(|spelling| {
                match_text(haystack, position, spelling.as_bytes(), self.ignore_case)
            })
            .max()
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
        let position = match drive {
            // Windows ignores case, and so does the path it shows to WSL.
            Some(_) => match_keyword(haystack, position, b"users", true),
            None => match_keyword(haystack, position, b"Users", false)
                .or_else(|| match_keyword(haystack, position, b"home", false)),
        }?;
        let start = match_separator(haystack, position)?;
        // Checked last because it looks backwards: is this where a path
        // starts, or the middle of one (`src/home/x`, `/api/users/x`)?
        if !starts_a_path(haystack, index, drive.is_some()) {
            return None;
        }

        let mut end = start;
        while end < haystack.len()
            && !ends_a_name(haystack[end])
            && !matches!(encoded_byte(haystack, end), Some((b'/' | b'\\', _)))
        {
            end += 1;
        }
        (end > start).then_some((start, end))
    }

    /// End of the current user's name standing as a whole word at `index`.
    fn match_user_name(&self, haystack: &[u8], index: usize) -> Option<usize> {
        // The name must start a word. A percent-encoded character before it
        // ends in hexadecimal digits but is not part of a word.
        let before = &haystack[..index];
        let inside_a_word = before.last().is_some_and(|byte| is_word_byte(*byte))
            && encoded_byte_before(before).is_none_or(is_word_byte);
        if inside_a_word {
            return None;
        }
        self.user_names.iter().find_map(|name| {
            let end = name
                .spellings
                .iter()
                .filter_map(|spelling| match_text(haystack, index, spelling.as_bytes(), true))
                .max()?;
            let whole = haystack.get(end).is_none_or(|next| !is_word_byte(*next));
            (whole && (name.anywhere || expects_a_user_name(haystack, index, end))).then_some(end)
        })
    }
}

/// The spellings of `text` to look for: composed and decomposed accents, and
/// with `any_case` also all lower-case and all upper-case. ASCII letters are
/// compared without regard to case separately, so the case variants only
/// matter for the other letters.
fn spellings(text: &str, any_case: bool) -> Vec<String> {
    let mut spellings = vec![text.to_owned()];
    let mut add = |spelling: String| {
        if !spellings.contains(&spelling) {
            spellings.push(spelling);
        }
    };
    let composed = compose(text);
    let decomposed = decompose(&composed);
    for form in [composed, decomposed] {
        if any_case {
            // Upper-casing a decomposed letter keeps its mark, so every form
            // gets both cases.
            add(form.to_lowercase());
            add(form.to_uppercase());
        }
        add(form);
    }
    spellings
}

fn decompose(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    for character in text.chars() {
        let lower = character.to_lowercase().next().unwrap_or(character);
        match DECOMPOSITIONS
            .iter()
            .find(|(composed, _, _)| *composed == lower)
        {
            Some((_, base, mark)) => {
                if character == lower {
                    output.push(*base);
                } else {
                    output.extend(base.to_uppercase());
                }
                output.push(*mark);
            }
            None => output.push(character),
        }
    }
    output
}

fn compose(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    for character in text.chars() {
        let composed = output.chars().next_back().and_then(|previous| {
            let lower = previous.to_lowercase().next().unwrap_or(previous);
            let (composed, _, _) = DECOMPOSITIONS
                .iter()
                .find(|(_, base, mark)| *base == lower && *mark == character)?;
            Some(if previous == lower {
                composed.to_string()
            } else {
                composed.to_uppercase().collect()
            })
        });
        match composed {
            Some(composed) => {
                output.pop();
                output.push_str(&composed);
            }
            None => output.push(character),
        }
    }
    output
}

/// Matches `expected` at `position`, where every byte may be written as it
/// is or percent-encoded, once or more. Returns where the match ends.
fn match_text(
    haystack: &[u8],
    mut position: usize,
    expected: &[u8],
    ignore_ascii_case: bool,
) -> Option<usize> {
    for wanted in expected {
        let found = *haystack.get(position)?;
        let same = |byte: u8| {
            if ignore_ascii_case {
                byte.eq_ignore_ascii_case(wanted)
            } else {
                byte == *wanted
            }
        };
        if same(found) {
            position += 1;
        } else if let Some((_, length)) =
            encoded_byte(haystack, position).filter(|(decoded, _)| same(*decoded))
        {
            position += length;
        } else {
            return None;
        }
    }
    Some(position)
}

/// Matches a drive at `index` and returns its lower-case letter and where the
/// rest of the path starts: `C:`, `C%3A`, or `/mnt/c` as WSL shows it.
fn match_drive(haystack: &[u8], index: usize) -> Option<(u8, usize)> {
    let rest = haystack.get(index..)?;
    match rest {
        [b'/', b'm', b'n', b't', b'/', letter, ..] if letter.is_ascii_alphabetic() => {
            Some((letter.to_ascii_lowercase(), index + 6))
        }
        [letter, b':', ..] if letter.is_ascii_alphabetic() => {
            Some((letter.to_ascii_lowercase(), index + 2))
        }
        [letter, b'%', ..] if letter.is_ascii_alphabetic() => {
            let (decoded, length) = encoded_byte(haystack, index + 1)?;
            (decoded == b':').then_some((letter.to_ascii_lowercase(), index + 1 + length))
        }
        _ => None,
    }
}

/// Matches one path separator at `position` in any spelling: `/`, one or
/// more backslashes (they double with every layer of escaping), or either of
/// them percent-encoded.
fn match_separator(haystack: &[u8], position: usize) -> Option<usize> {
    match haystack.get(position)? {
        b'/' => Some(position + 1),
        b'\\' => {
            let mut end = position;
            while haystack.get(end) == Some(&b'\\') {
                end += 1;
            }
            Some(end)
        }
        _ => match encoded_byte(haystack, position)? {
            (b'/' | b'\\', length) => Some(position + length),
            _ => None,
        },
    }
}

/// Matches `keyword` at `position`.
fn match_keyword(
    haystack: &[u8],
    position: usize,
    keyword: &[u8],
    ignore_ascii_case: bool,
) -> Option<usize> {
    let end = position + keyword.len();
    let candidate = haystack.get(position..end)?;
    let equal = if ignore_ascii_case {
        candidate.eq_ignore_ascii_case(keyword)
    } else {
        candidate == keyword
    };
    equal.then_some(end)
}

/// The byte percent-encoded at `position` and the length of its encoding:
/// `%2F`, or encoded again as `%252F`, and so on.
fn encoded_byte(haystack: &[u8], position: usize) -> Option<(u8, usize)> {
    if haystack.get(position) != Some(&b'%') {
        return None;
    }
    // Every further layer of encoding turns the `%` into `%25`.
    let mut digits = position + 1;
    while haystack.get(digits..digits + 2) == Some(b"25")
        && haystack
            .get(digits + 2..digits + 4)
            .is_some_and(|next| next.iter().all(u8::is_ascii_hexdigit))
    {
        digits += 2;
    }
    let [high, low] = haystack.get(digits..digits + 2)? else {
        return None;
    };
    let digit = |byte: &u8| char::from(*byte).to_digit(16);
    let byte = u8::try_from(digit(high)? * 16 + digit(low)?).ok()?;
    Some((byte, digits + 2 - position))
}

/// Whether a home-shaped path that begins at `index` is at a place where a
/// path begins, rather than in the middle of a longer path or a URL.
fn starts_a_path(haystack: &[u8], index: usize, has_drive: bool) -> bool {
    let Some(previous) = index.checked_sub(1).map(|before| haystack[before]) else {
        return true;
    };
    if has_drive && !haystack[index..].starts_with(b"/mnt/") {
        // `C:\Users`: anything but a letter or digit may come before the
        // drive letter, which covers `file:///C:/` and `\\?\C:\`.
        return !previous.is_ascii_alphanumeric();
    }
    previous.is_ascii_whitespace()
        || matches!(
            previous,
            b'"' | b'\'' | b'`' | b'=' | b':' | b',' | b';' | b'(' | b'[' | b'{' | b'<' | b'|'
        )
        || haystack[..index]
            .get(index.saturating_sub(7)..)
            .is_some_and(|before| before.eq_ignore_ascii_case(b"file://"))
}

/// Whether the word at `start..end` is where a user name is expected: next
/// to a path separator or an `@`, or the value of something named like a
/// user (`user=`, `"owner": "`).
fn expects_a_user_name(haystack: &[u8], start: usize, end: usize) -> bool {
    let after = haystack.get(end).copied();
    if after == Some(b'@') || match_separator(haystack, end).is_some() {
        return true;
    }
    let before = &haystack[..start];
    if before
        .last()
        .is_some_and(|previous| matches!(previous, b'@' | b'/' | b'\\'))
        || ends_with_encoded_separator(before)
    {
        return true;
    }

    // Backwards: an optional quote, `=` or `:`, an optional quote, a key.
    let is_filler = |byte: &u8| matches!(byte, b' ' | b'"' | b'\'');
    let trimmed = trim_end(before, is_filler);
    let Some((separator, key_side)) = trimmed.split_last() else {
        return false;
    };
    if !matches!(separator, b'=' | b':') {
        return false;
    }
    let key_side = trim_end(key_side, is_filler);
    let key_start = key_side
        .iter()
        .rposition(|byte| !(byte.is_ascii_alphanumeric() || *byte == b'_'))
        .map_or(0, |position| position + 1);
    let key = key_side[key_start..].to_ascii_lowercase();
    IDENTITY_KEYS
        .iter()
        .any(|known| key.ends_with(known.as_bytes()))
}

fn trim_end(bytes: &[u8], unwanted: impl Fn(&u8) -> bool) -> &[u8] {
    let end = bytes
        .iter()
        .rposition(|byte| !unwanted(byte))
        .map_or(0, |position| position + 1);
    &bytes[..end]
}

fn ends_with_encoded_separator(before: &[u8]) -> bool {
    matches!(encoded_byte_before(before), Some(b'/' | b'\\'))
}

/// The byte whose percent-encoding `before` ends with, if it ends with one.
fn encoded_byte_before(before: &[u8]) -> Option<u8> {
    // `%XX`, `%25XX`, `%2525XX`.
    (3..=7).step_by(2).find_map(|length| {
        let start = before.len().checked_sub(length)?;
        encoded_byte(before, start)
            .filter(|(_, encoded)| *encoded == length)
            .map(|(byte, _)| byte)
    })
}

/// Letters, digits, `_`, and anything that is not ASCII: what a word that
/// contains a name is made of.
fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 0x80
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
        // Where case matters, a different case is a different directory, and
        // not one that has the shape of a home directory.
        assert_eq!(unix().text("/HOME/ANA/x"), "/HOME/ANA/x");
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
        // Four letters: only where a user name is expected.
        assert_eq!(
            redactor.text("hola José, ¿qué tal?"),
            "hola José, ¿qué tal?"
        );
        assert_eq!(redactor.text("user=José;"), "user=<user>;");
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

    #[test]
    fn separators_escaped_or_encoded_more_than_once_are_recognised() {
        let redactor = windows();
        // A path that went through `{:?}` or JSON twice.
        assert_eq!(redactor.text(r"C:\\\\Users\\\\Anabel\\\\x"), r"~\\\\x");
        assert_eq!(
            redactor.text(r"C:\\\\Users\\\\Bob\\\\x"),
            r"C:\\\\Users\\\\<user>\\\\x"
        );
        // A URL inside a URL.
        assert_eq!(unix().text("next=%252Fhome%252Fana%252Fx"), "next=~%252Fx");
        assert_eq!(
            unix().text("next=%252Fhome%252Fbob%252Fx"),
            "next=%252Fhome%252F<user>%252Fx"
        );
        assert_eq!(
            redactor.text("C%253A%255CUsers%255CAnabel%255Cx"),
            "~%255Cx"
        );
    }

    #[test]
    fn the_user_name_is_found_percent_encoded_and_in_other_forms() {
        let redactor = mac();
        assert_eq!(redactor.text("user=Ana%20Maria&x=1"), "user=<user>&x=1");
        assert_eq!(redactor.text("ANA MARIA logged in"), "<user> logged in");

        let accented = Redactor::with_case_sensitivity(Some(Path::new("/Users/Jocelyné")), false);
        // Decomposed, as macOS hands out file names.
        assert_eq!(accented.text("/Users/Jocelyne\u{301}/x"), "~/x");
        assert_eq!(accented.text("hello Jocelyne\u{301}!"), "hello <user>!");
        // Upper case, composed and decomposed.
        assert_eq!(accented.text("/USERS/JOCELYNÉ/x"), "~/x");
        assert_eq!(accented.text("JOCELYNE\u{301} said"), "<user> said");

        // And the other way round: a decomposed home, composed text.
        let decomposed =
            Redactor::with_case_sensitivity(Some(Path::new("/Users/Jocelyne\u{301}")), false);
        assert_eq!(decomposed.text("/Users/Jocelyné/x"), "~/x");
    }

    #[test]
    fn things_that_only_resemble_a_home_directory_are_left_alone() {
        for redactor in [unix(), mac(), windows()] {
            for text in [
                "src/home/Home.tsx:10:2",
                "at render (tauri://localhost/src/home/Home.tsx:10:2)",
                "GET /api/users/list failed",
                "https://example.org/api/Users/42/profile",
                "crate::home::dir",
                "the users/groups table",
                "a/home/b and x/Users/y",
                "D:Users",
            ] {
                assert_eq!(redactor.text(text), text);
            }
        }
    }

    #[test]
    fn a_home_shaped_path_is_recognised_wherever_a_path_can_start() {
        let redactor = unix();
        for (text, expected) in [
            ("/home/bob/x", "/home/<user>/x"),
            ("in /home/bob/x", "in /home/<user>/x"),
            ("path=/Users/bob/x", "path=/Users/<user>/x"),
            (
                "PATH=/usr/bin:/home/bob/bin",
                "PATH=/usr/bin:/home/<user>/bin",
            ),
            ("[\"/home/bob\"]", "[\"/home/<user>\"]"),
            ("file:///home/bob/x", "file:///home/<user>/x"),
            ("FILE:///Users/bob/x", "FILE:///Users/<user>/x"),
            ("p=%2Fhome%2Fbob", "p=%2Fhome%2F<user>"),
            ("file:///C:/Users/Bob/x", "file:///C:/Users/<user>/x"),
            (r"\\?\c:\users\Bob", r"\\?\c:\users\<user>"),
            ("in /mnt/c/Users/Bob/x", "in /mnt/c/Users/<user>/x"),
        ] {
            assert_eq!(redactor.text(text), expected, "{text}");
        }
    }

    #[test]
    fn a_user_called_like_a_common_word_does_not_garble_the_log() {
        for (name, text) in [
            ("node", "node_modules/.bin/node exited: node not found"),
            ("command", "command failed: unknown command"),
            ("code", "exit code 3, see the code"),
            ("shell", "the shell exited"),
            ("claude", "claude code session started"),
        ] {
            let home = format!("/home/{name}");
            let redactor = Redactor::with_case_sensitivity(Some(Path::new(&home)), true);
            assert_eq!(redactor.text(text), text, "user {name}");
            // The home directory itself is still replaced.
            assert_eq!(redactor.text(&format!("{home}/x")), "~/x");
        }
    }

    #[test]
    fn a_short_name_is_only_replaced_where_a_user_name_is_expected() {
        let redactor = Redactor::with_case_sensitivity(Some(Path::new("/home/mara")), true);
        for text in ["mara is a word here", "samara, maracas, Mara's", "tamara"] {
            assert_eq!(redactor.text(text), text);
        }
        for (text, expected) in [
            ("user=mara", "user=<user>"),
            ("\"username\": \"Mara\"", "\"username\": \"<user>\""),
            ("owner: mara", "owner: <user>"),
            ("mara@workstation", "<user>@workstation"),
            ("ssh://git@mara", "ssh://git@<user>"),
            ("/data/mara/x", "/data/<user>/x"),
            (r"D:\work\mara", r"D:\work\<user>"),
            ("x%2Fmara%2Fy", "x%2F<user>%2Fy"),
        ] {
            assert_eq!(redactor.text(text), expected, "{text}");
        }
    }
}
