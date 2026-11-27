//! The data of a session: what the sidebar shows and what is remembered
//! between two runs of the application.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::launch::LaunchFailure;
use super::project::{GitInfo, Project};

/// Longest manual name and automatic title that is kept, in characters. A
/// longer one is cut.
pub const MAX_TITLE_CHARS: usize = 256;

/// Longest command line of a background process, in bytes.
pub const MAX_COMMAND_BYTES: usize = 8 * 1024;

/// Longest identifier of a Claude Code session, in bytes.
pub const MAX_CLAUDE_SESSION_ID_BYTES: usize = 128;

/// The identifier of a session, which stays the same for its whole life,
/// including across restarts of its process and of the application.
///
/// It is distinct from the [`SessionId`](crate::session::SessionId) of the
/// terminal layer, which identifies one running process and changes every
/// time the session is started.
///
/// A key is a short text of letters, digits, `-` and `_`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SessionKey(String);

impl SessionKey {
    /// Creates a key that no other session has: `s-` followed by 16
    /// hexadecimal digits drawn at random.
    #[must_use]
    pub fn generate() -> Self {
        Self(format!("s-{}", random_hex()))
    }

    /// Accepts a key received from outside: a frontend or a file.
    ///
    /// # Errors
    ///
    /// [`InvalidSessionKey`] unless `raw` is 1 to 64 letters, digits, `-`
    /// or `_`.
    pub fn from_raw(raw: impl Into<String>) -> Result<Self, InvalidSessionKey> {
        let raw = raw.into();
        let valid = (1..=64).contains(&raw.len())
            && raw
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        if valid {
            Ok(Self(raw))
        } else {
            Err(InvalidSessionKey)
        }
    }

    /// The key as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SessionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for SessionKey {
    type Error = InvalidSessionKey;

    fn try_from(raw: String) -> Result<Self, Self::Error> {
        Self::from_raw(raw)
    }
}

impl From<SessionKey> for String {
    fn from(key: SessionKey) -> Self {
        key.0
    }
}

/// A text that is not a valid [`SessionKey`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("a session identifier is 1 to 64 letters, digits, '-' or '_'")]
pub struct InvalidSessionKey;

/// What a session runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionKind {
    /// Claude Code (`claude`), started in the folder of the session.
    Claude,
    /// The user's login shell.
    Shell,
    /// A named long-running command, such as a tunnel or a development
    /// server. Shown in its own section of the sidebar.
    Background,
}

/// Where the process of a session is in its life.
///
/// This is serialized with a `status` tag. It is only ever about the
/// process: what the agent in it is doing goes in [`Session::agent`], and
/// is never added here. The enum is `#[non_exhaustive]` all the same, and a
/// frontend treats a status it does not know as
/// [`Running`](Self::Running).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase")]
#[non_exhaustive]
pub enum SessionState {
    /// Nothing runs: the session was restored from the previous run of the
    /// application, or stopped. It can be resumed or restarted. A restored
    /// session is never started on its own: only when the user asks.
    Dormant,
    /// The session was created or its start was requested, and its process
    /// is not running yet.
    Starting,
    /// The process is running.
    Running,
    /// The process has ended, or could not be started.
    Exited {
        /// The exit code. `None` when a signal ended the process, when the
        /// status is unknown, and when the process never started.
        code: Option<u32>,
        /// Description of the signal that ended the process, if any.
        signal: Option<String>,
        /// Why the session could not do its job, when termx knows: the
        /// program was not found, the folder is gone. `None` for an
        /// ordinary exit.
        failure: Option<LaunchFailure>,
    },
}

impl SessionState {
    /// Whether a process is running or about to.
    #[must_use]
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Starting | Self::Running)
    }
}

/// One session: a row of the sidebar.
///
/// Times are milliseconds since the Unix epoch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    /// Identifies the session for its whole life.
    pub id: SessionKey,
    /// What the session runs.
    pub kind: SessionKind,
    /// The folder the session was created in. Its process is started, and
    /// restarted, there.
    #[serde(serialize_with = "super::lossy_path")]
    pub cwd: PathBuf,
    /// The folder the program last reported to be in (OSC 7), while it
    /// runs. `None` when it never reported one, which is the usual case:
    /// few shells do without being configured to.
    #[serde(serialize_with = "super::lossy_path_option")]
    pub live_cwd: Option<PathBuf>,
    /// The command line of a [`SessionKind::Background`] session. `None`
    /// for the other kinds.
    pub command: Option<String>,
    /// What to show as the title: [`name`](Self::name) if there is one,
    /// else [`auto_title`](Self::auto_title), else a default for the kind.
    pub title: String,
    /// The name the user gave. It always wins over the automatic title.
    pub name: Option<String>,
    /// The title the program set for its terminal (OSC 0 and 2).
    pub auto_title: Option<String>,
    /// The project the session is grouped under: the one of
    /// [`live_cwd`](Self::live_cwd) when it is known, else of
    /// [`cwd`](Self::cwd).
    pub project: Project,
    /// The branch and worktree of that folder. `None` outside a repository.
    pub git: Option<GitInfo>,
    /// Where the process is in its life.
    pub state: SessionState,
    /// The identifier of the running terminal, to send it input, resize it
    /// and acknowledge its output. `None` unless a process is running.
    pub terminal_id: Option<u64>,
    /// When the session was created.
    pub created_at: u64,
    /// When the session last printed something, or was last started.
    pub last_activity_at: u64,
    /// What the agent of a Claude Code session is doing, as its hooks
    /// report it. Always `None` for now: see [`AgentState`].
    pub agent: Option<AgentState>,
    /// The identifier Claude Code gave its conversation, to resume it with
    /// `claude --resume`. `None` until it is known.
    ///
    /// It stays here rather than in [`agent`](Self::agent): it belongs to
    /// the session, is saved with it, and is needed to resume a dormant
    /// session, which has no agent running.
    pub claude_session_id: Option<String>,
    /// Something the user should know about the session while it runs,
    /// with a message for them. For now only
    /// [`LaunchUnconfirmed`](LaunchFailure::LaunchUnconfirmed): its shell
    /// did not get to the command. Cleared when the command starts after
    /// all, and when the session starts again.
    pub warning: Option<LaunchFailure>,
}

/// What the agent of a session is doing.
///
/// Reserved for the version that installs hooks in Claude Code: until then
/// [`Session::agent`] is always `None`. That version only sets it (and the
/// conversation, with
/// [`Workspace::set_claude_session_id`](super::Workspace::set_claude_session_id)):
/// it adds fields and statuses, never changes these, and never touches
/// [`SessionState`]. A frontend shows nothing about the agent while it is
/// `null`. A status it does not know is shown as unknown: neutral, neither
/// busy nor asking for attention, since guessing either way misleads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct AgentState {
    /// What the agent does.
    pub status: AgentStatus,
    /// Since when it waits for the user, while its status is one of the
    /// `WaitingFor...` ones. Milliseconds since 1970.
    pub waiting_since: Option<u64>,
}

/// What the agent of a session does. See [`AgentState`].
///
/// What it waits for is part of the status, rather than a field next to
/// it: a frontend needs both to decide how loudly to ask for attention,
/// and one value cannot be half set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum AgentStatus {
    /// It works on what it was asked.
    Working,
    /// It waits for the user to allow something it wants to do (a
    /// command, an edit). The session cannot go on without them.
    WaitingForPermission,
    /// It waits for the user's next message or answer to a question.
    WaitingForInput,
    /// It finished what it was asked and has nothing to do.
    Done,
}

impl AgentState {
    /// An agent in `status`, waiting since `waiting_since`.
    #[must_use]
    pub fn new(status: AgentStatus, waiting_since: Option<u64>) -> Self {
        Self {
            status,
            waiting_since,
        }
    }
}

impl Session {
    /// The folder whose project and branch are shown.
    #[must_use]
    pub fn effective_cwd(&self) -> &Path {
        self.live_cwd.as_deref().unwrap_or(&self.cwd)
    }

    /// Recomputes [`title`](Self::title) from [`name`](Self::name) and
    /// [`auto_title`](Self::auto_title). Call it after changing either:
    /// the fields are public, and nothing else keeps the title in step.
    pub fn refresh_title(&mut self) {
        self.title = self
            .name
            .clone()
            .or_else(|| self.auto_title.clone())
            .unwrap_or_else(|| default_title(self.kind, self.command.as_deref()));
    }
}

/// The title of a session that has neither a name nor an automatic title.
#[must_use]
pub fn default_title(kind: SessionKind, command: Option<&str>) -> String {
    match kind {
        SessionKind::Claude => "Claude Code".to_owned(),
        SessionKind::Shell => "Shell".to_owned(),
        SessionKind::Background => command
            .and_then(|command| clean_title(command, false))
            .unwrap_or_else(|| "Background process".to_owned()),
    }
}

/// Makes a title or a name fit to be shown: control characters and
/// invisible formatting characters ([`is_invisible_format`]) are removed,
/// white space is collapsed and the result is cut at
/// [`MAX_TITLE_CHARS`]. With `strip_status`, the glyph Claude Code puts in
/// front of its title to show that it is working is removed as well, so
/// that its animation does not count as a change of title.
///
/// Returns `None` when nothing is left.
#[must_use]
pub fn clean_title(raw: &str, strip_status: bool) -> Option<String> {
    let mut text = String::with_capacity(raw.len().min(MAX_TITLE_CHARS * 4));
    let mut pending_space = false;
    for c in raw.chars() {
        if c.is_whitespace() {
            pending_space = !text.is_empty();
        } else if !c.is_control() && !is_invisible_format(c) {
            if pending_space {
                text.push(' ');
                pending_space = false;
            }
            text.push(c);
        }
        if text.chars().count() >= MAX_TITLE_CHARS * 2 {
            break;
        }
    }
    let mut rest = text.as_str();
    if strip_status {
        let mut chars = rest.chars();
        if let (Some(first), Some(' ')) = (chars.next(), chars.next())
            && is_status_glyph(first)
        {
            rest = chars.as_str().trim_start();
        }
    }
    let cut: String = rest.chars().take(MAX_TITLE_CHARS).collect();
    let cut = cut.trim_end();
    (!cut.is_empty()).then(|| cut.to_owned())
}

/// Whether `c` is an invisible character that changes how the text around
/// it is shown: the bidirectional embeddings, overrides, isolates and
/// marks, zero-width spaces and word joiners, the byte order mark, the
/// soft hyphen, the Hangul fillers (blank letters that show as nothing) and
/// the interlinear annotation marks. A program could use them to make its title look like
/// another, or to reverse what the sidebar shows after it. The zero-width
/// joiner and non-joiner stay: emoji sequences and several scripts need
/// them.
fn is_invisible_format(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{061C}'
            | '\u{180E}'
            | '\u{200B}'
            | '\u{200E}'
            | '\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FEFF}'
            | '\u{115F}'
            | '\u{1160}'
            | '\u{3164}'
            | '\u{FFA0}'
            | '\u{FFF9}'..='\u{FFFB}'
    )
}

/// Whether `c` is one of the marks Claude Code animates in front of its
/// title: Braille patterns for its spinner, and asterisk-like stars.
fn is_status_glyph(c: char) -> bool {
    matches!(c, '\u{2800}'..='\u{28FF}')
        || matches!(
            c,
            '*' | '·' | '✢' | '✳' | '✶' | '✻' | '✽' | '∗' | '⏺' | '●' | '○'
        )
}

/// 16 hexadecimal digits that no other call returns, drawn at random.
pub(crate) fn random_hex() -> String {
    use std::hash::{BuildHasher, Hasher};

    static COUNTER: AtomicU64 = AtomicU64::new(0);
    // `RandomState` is seeded from the operating system.
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    hasher.write_u64(now_ms());
    hasher.write_u32(std::process::id());
    format!("{:016x}", hasher.finish())
}

/// The current time, in milliseconds since the Unix epoch.
#[must_use]
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

/// Checks the identifier of a Claude Code conversation before it is used
/// on a command line: letters, digits, `-` and `_` only.
#[must_use]
pub fn is_valid_claude_session_id(id: &str) -> bool {
    (1..=MAX_CLAUDE_SESSION_ID_BYTES).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_keys_are_valid_and_distinct() {
        let keys: std::collections::HashSet<SessionKey> =
            (0..1000).map(|_| SessionKey::generate()).collect();

        assert_eq!(keys.len(), 1000);
        for key in keys {
            assert_eq!(key.as_str().len(), 18);
            assert_eq!(SessionKey::from_raw(key.as_str()), Ok(key));
        }
    }

    #[test]
    fn keys_from_outside_are_checked() {
        for bad in ["", "a b", "../x", "s\u{e9}", &"x".repeat(65), "a/b", "a\"b"] {
            assert_eq!(SessionKey::from_raw(bad), Err(InvalidSessionKey), "{bad:?}");
            let json = serde_json::to_string(bad).unwrap();
            assert!(serde_json::from_str::<SessionKey>(&json).is_err());
        }
        let key: SessionKey = serde_json::from_str("\"s-1_A\"").unwrap();
        assert_eq!(key.as_str(), "s-1_A");
        assert_eq!(serde_json::to_string(&key).unwrap(), "\"s-1_A\"");
    }

    #[test]
    fn states_are_tagged_with_their_status() {
        let json = |state: &SessionState| serde_json::to_value(state).unwrap();

        assert_eq!(
            json(&SessionState::Dormant),
            serde_json::json!({ "status": "dormant" })
        );
        assert_eq!(
            json(&SessionState::Starting),
            serde_json::json!({ "status": "starting" })
        );
        assert_eq!(
            json(&SessionState::Running),
            serde_json::json!({ "status": "running" })
        );
        assert_eq!(
            json(&SessionState::Exited {
                code: Some(2),
                signal: None,
                failure: None
            }),
            serde_json::json!({ "status": "exited", "code": 2, "signal": null, "failure": null })
        );
        assert!(SessionState::Starting.is_active());
        assert!(SessionState::Running.is_active());
        assert!(!SessionState::Dormant.is_active());
    }

    #[test]
    fn an_agent_says_what_it_waits_for() {
        let json = |status| serde_json::to_value(AgentState::new(status, Some(5))).unwrap();

        assert_eq!(
            json(AgentStatus::WaitingForPermission),
            serde_json::json!({ "status": "waitingForPermission", "waitingSince": 5 })
        );
        assert_eq!(
            json(AgentStatus::WaitingForInput)["status"],
            "waitingForInput"
        );
        assert_eq!(json(AgentStatus::Working)["status"], "working");
        assert_eq!(json(AgentStatus::Done)["status"], "done");
    }

    #[test]
    fn titles_are_cleaned() {
        assert_eq!(
            clean_title("  hello   world \n", false).unwrap(),
            "hello world"
        );
        assert_eq!(clean_title("a\x1b[31mb\x07c", false).unwrap(), "a[31mbc");
        assert_eq!(clean_title(" \t\n", false), None);
        assert_eq!(clean_title("", true), None);
        assert_eq!(
            clean_title(&"x".repeat(1000), false)
                .unwrap()
                .chars()
                .count(),
            MAX_TITLE_CHARS
        );
        assert_eq!(
            clean_title(&"é".repeat(1000), false)
                .unwrap()
                .chars()
                .count(),
            MAX_TITLE_CHARS
        );
    }

    #[test]
    fn titles_cannot_reorder_or_hide_text() {
        // A right-to-left override would show "gpj.exe" as "exe.jpg".
        assert_eq!(
            clean_title("invoice\u{202E}gpj.exe", false).unwrap(),
            "invoicegpj.exe"
        );
        for c in [
            '\u{202A}', '\u{202B}', '\u{202C}', '\u{202D}', '\u{2066}', '\u{2067}', '\u{2068}',
            '\u{2069}', '\u{200B}', '\u{200E}', '\u{200F}', '\u{061C}', '\u{FEFF}', '\u{2060}',
            '\u{00AD}', '\u{115F}', '\u{1160}', '\u{3164}', '\u{FFA0}', '\u{FFF9}', '\u{FFFA}',
            '\u{FFFB}',
        ] {
            assert_eq!(
                clean_title(&format!("a{c}b"), false).unwrap(),
                "ab",
                "{c:?}"
            );
        }
        assert_eq!(clean_title("\u{202E}\u{2066}", false), None);
        // What emoji and scripts need is kept.
        assert_eq!(
            clean_title("👩\u{200D}💻 work", false).unwrap(),
            "👩\u{200D}💻 work"
        );
        assert_eq!(
            clean_title("می\u{200C}خواهم", false).unwrap(),
            "می\u{200C}خواهم"
        );
        // A title made only of blank Hangul letters is no title.
        assert_eq!(clean_title("\u{3164}\u{3164}", false), None);
        // Hangul itself is untouched.
        assert_eq!(clean_title("한국어", false).unwrap(), "한국어");
        // Text in a right-to-left script itself is untouched.
        assert_eq!(clean_title("שלום עולם", false).unwrap(), "שלום עולם");
    }

    #[test]
    fn the_status_glyph_of_claude_code_is_not_part_of_the_title() {
        for raw in [
            "✳ Fix the login",
            "⠐ Fix the login",
            "⠂  Fix the login",
            "* Fix the login",
        ] {
            assert_eq!(clean_title(raw, true).unwrap(), "Fix the login", "{raw:?}");
        }
        // Only a mark followed by a space, and only when asked.
        assert_eq!(clean_title("✳Fix", true).unwrap(), "✳Fix");
        assert_eq!(clean_title("✳ Fix", false).unwrap(), "✳ Fix");
        assert_eq!(clean_title("a b", true).unwrap(), "a b");
        assert_eq!(clean_title("✳ ", true).unwrap(), "✳");
    }

    #[test]
    fn default_titles_depend_on_the_kind() {
        assert_eq!(default_title(SessionKind::Claude, None), "Claude Code");
        assert_eq!(default_title(SessionKind::Shell, None), "Shell");
        assert_eq!(
            default_title(SessionKind::Background, Some("pnpm  dev")),
            "pnpm dev"
        );
        assert_eq!(
            default_title(SessionKind::Background, None),
            "Background process"
        );
    }

    #[test]
    fn claude_session_ids_are_checked() {
        assert!(is_valid_claude_session_id(
            "0b9d6f7e-3c1a-4f3e-9a55-2f6f1f0f8a10"
        ));
        for bad in ["", "a b", "x;rm -rf", "$(id)", "a'b", &"a".repeat(129)] {
            assert!(!is_valid_claude_session_id(bad), "{bad:?}");
        }
    }
}
