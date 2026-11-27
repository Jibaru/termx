//! The wire protocol between termx and the programs that talk to it.
//!
//! termx listens on a local socket: a Unix domain socket on macOS and
//! Linux, a named pipe on Windows. Today its only clients are the hooks of
//! Claude Code (through `termx-cli hook`, see [`hook`]); the command line
//! and the MCP server of termx will use the same protocol.
//!
//! # Finding termx
//!
//! termx gives every program it starts in a terminal three environment
//! variables:
//!
//! | Variable              | Value                                              |
//! | --------------------- | -------------------------------------------------- |
//! | [`ENV_SESSION_ID`]    | The session the terminal belongs to (`s-...`).     |
//! | [`ENV_SOCKET`]        | The address of the socket of this run of termx.    |
//! | [`ENV_TICKET`]        | A secret drawn at random for this run of termx.    |
//!
//! A program without them was not started by termx, and has nothing to
//! talk to.
//!
//! # Messages
//!
//! Version [`PROTOCOL_VERSION`]. A client connects, writes one request per
//! line (a JSON object followed by `\n`), and reads one response line for
//! each, in order. A line is at most [`MAX_MESSAGE_BYTES`] bytes, newline
//! included. Every request carries the version and the ticket:
//!
//! ```json
//! {"v":1,"ticket":"<ENV_TICKET>","op":"ping"}
//! {"v":1,"ticket":"<ENV_TICKET>","op":"hook","session":"s-0123456789abcdef",
//!  "event":{"name":"PreToolUse","claudeSessionId":"0b9d...","subagent":false,
//!           "pid":4242,"at":1795716005340123}}
//! ```
//!
//! and every response says whether it worked:
//!
//! ```json
//! {"v":1,"ok":true}
//! {"v":1,"ok":true,"protocol":1,"app":"termx","version":"0.1.0"}
//! {"v":1,"ok":false,"error":{"kind":"unauthorized","message":"..."}}
//! ```
//!
//! | `op`   | Fields                       | Answer                              |
//! | ------ | ---------------------------- | ----------------------------------- |
//! | `ping` | none                         | `protocol`, `app`, `version`        |
//! | `hook` | `session`, `event` ([`HookEvent`]) | nothing more                  |
//!
//! The error kinds are those of [`ErrorKind`]. After `unauthorized`,
//! `unsupportedVersion`, `tooLarge` or `malformed`, termx closes the
//! connection. A new version of the protocol only adds operations and
//! fields; a client must ignore fields it does not know.

#![warn(missing_docs)]

pub mod client;
pub mod hook;
pub mod parent;

use serde::{Deserialize, Serialize};

/// The version of the protocol this crate speaks.
pub const PROTOCOL_VERSION: u32 = 1;

/// The longest line, request or response, newline included.
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024;

/// The session of termx a program runs in.
pub const ENV_SESSION_ID: &str = "TERMX_SESSION_ID";

/// The address of the socket of termx: a path on macOS and Linux, the name
/// of a pipe (`\\.\pipe\...`) on Windows.
pub const ENV_SOCKET: &str = "TERMX_SOCKET";

/// The secret that authenticates requests. Not named `..._TOKEN` or
/// `..._SECRET`: Claude Code can strip variables with such names from the
/// environment of its hooks (`CLAUDE_CODE_SUBPROCESS_ENV_SCRUB`), which
/// would leave the hooks unable to talk to termx.
pub const ENV_TICKET: &str = "TERMX_TICKET";

/// Every variable termx sets for the programs of its terminals.
pub const ENV_VARIABLES: [&str; 3] = [ENV_SESSION_ID, ENV_SOCKET, ENV_TICKET];

/// Length of a ticket, in hexadecimal digits (128 bits).
pub const TICKET_HEX_LEN: usize = 32;

/// A request, as it is sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Request<'a> {
    /// Always [`PROTOCOL_VERSION`].
    pub v: u32,
    /// The ticket of the run of termx.
    pub ticket: &'a str,
    /// What is asked.
    #[serde(flatten)]
    pub op: Op,
}

impl<'a> Request<'a> {
    /// A request for `op`, with `ticket`.
    #[must_use]
    pub fn new(ticket: &'a str, op: Op) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            ticket,
            op,
        }
    }

    /// The request as one line, newline included.
    #[must_use]
    pub fn to_line(&self) -> Vec<u8> {
        // Only strings and numbers: serializing cannot fail.
        let mut line = serde_json::to_vec(self).unwrap_or_default();
        line.push(b'\n');
        line
    }
}

/// The operations of the protocol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum Op {
    /// Is termx there, and which protocol does it speak?
    Ping,
    /// Claude Code ran a hook in a session of termx.
    Hook {
        /// The session of termx, from [`ENV_SESSION_ID`].
        session: String,
        /// What happened.
        event: HookEvent,
    },
}

/// What a hook of Claude Code reports: the name of the event and the few
/// fields termx needs to follow the state of the agent. Never a prompt, a
/// tool input or output, a message, a path or a transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookEvent {
    /// `hook_event_name`: `PreToolUse`, `Stop`... Letters only.
    pub name: String,
    /// `session_id`: the conversation of Claude Code, which `claude
    /// --resume` takes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_session_id: Option<String>,
    /// `notification_type`, for `Notification`: `permission_prompt`,
    /// `idle_prompt`...
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notification_type: Option<String>,
    /// `tool_name`, only when it is one of [`hook::REPORTED_TOOLS`]: the
    /// tools that ask the user something. Other tool names are not sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// `source`, for `SessionStart`: `startup`, `resume`, `clear`,
    /// `compact`, `fork`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// `reason`, for `SessionEnd`: `clear`, `resume`, `logout`,
    /// `prompt_input_exit`, `other`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The hook ran for a subagent (`agent_id` was present).
    #[serde(default)]
    pub subagent: bool,
    /// The process of Claude Code that ran the hook, when the hook could
    /// tell. See [`parent`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// When the hook started, in microseconds since 1970. Hooks run in
    /// processes of their own, so their reports can arrive out of order:
    /// this puts them back in order.
    pub at: u64,
}

/// A response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Response {
    /// Always [`PROTOCOL_VERSION`].
    pub v: u32,
    /// Whether the request was carried out.
    pub ok: bool,
    /// Why not, when it was not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<WireError>,
    /// For `ping`: the protocol termx speaks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<u32>,
    /// For `ping`: always `termx`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    /// For `ping`: the version of termx.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

impl Response {
    /// A request that was carried out.
    #[must_use]
    pub fn ok() -> Self {
        Self {
            v: PROTOCOL_VERSION,
            ok: true,
            error: None,
            protocol: None,
            app: None,
            version: None,
        }
    }

    /// The answer to `ping`.
    #[must_use]
    pub fn pong(version: &str) -> Self {
        Self {
            protocol: Some(PROTOCOL_VERSION),
            app: Some("termx".to_owned()),
            version: Some(version.to_owned()),
            ..Self::ok()
        }
    }

    /// A request that was refused.
    #[must_use]
    pub fn error(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(WireError {
                kind,
                message: message.into(),
            }),
            ..Self::ok()
        }
    }

    /// The response as one line, newline included.
    #[must_use]
    pub fn to_line(&self) -> Vec<u8> {
        let mut line = serde_json::to_vec(self).unwrap_or_default();
        line.push(b'\n');
        line
    }
}

/// Why a request was refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireError {
    /// What a client can act on.
    pub kind: ErrorKind,
    /// For a person.
    pub message: String,
}

/// The kinds of [`WireError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorKind {
    /// Not a JSON object with the fields of a request.
    Malformed,
    /// Longer than [`MAX_MESSAGE_BYTES`].
    TooLarge,
    /// A version of the protocol termx does not speak.
    UnsupportedVersion,
    /// No ticket, or not the ticket of this run of termx.
    Unauthorized,
    /// An operation termx does not know.
    UnknownOp,
    /// No session of termx has this identifier.
    UnknownSession,
    /// termx could not carry it out.
    Internal,
    /// A kind this version of the protocol does not know.
    #[serde(other)]
    Other,
}

/// Whether `ticket` is shaped like a ticket: [`TICKET_HEX_LEN`] lower-case
/// hexadecimal digits.
#[must_use]
pub fn is_valid_ticket(ticket: &str) -> bool {
    ticket.len() == TICKET_HEX_LEN
        && ticket
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Compares two tickets in a time that does not depend on where they
/// differ.
#[must_use]
pub fn tickets_match(expected: &str, given: &str) -> bool {
    let (expected, given) = (expected.as_bytes(), given.as_bytes());
    if expected.len() != given.len() {
        return false;
    }
    expected
        .iter()
        .zip(given)
        .fold(0u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

/// Whether `id` is shaped like the identifier of a session of termx: 1 to
/// 64 letters, digits, `-` or `_`.
#[must_use]
pub fn is_valid_session_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_one_line_of_json() {
        let request = Request::new(
            "t",
            Op::Hook {
                session: "s-1".to_owned(),
                event: HookEvent {
                    name: "Stop".to_owned(),
                    claude_session_id: Some("abc".to_owned()),
                    notification_type: None,
                    tool: None,
                    source: None,
                    reason: None,
                    subagent: false,
                    pid: Some(7),
                    at: 5,
                },
            },
        );

        let line = request.to_line();

        assert_eq!(line.last(), Some(&b'\n'));
        assert_eq!(line.iter().filter(|b| **b == b'\n').count(), 1);
        let json: serde_json::Value = serde_json::from_slice(&line).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "v": 1, "ticket": "t", "op": "hook", "session": "s-1",
                "event": {"name": "Stop", "claudeSessionId": "abc", "subagent": false, "pid": 7, "at": 5}
            })
        );
        assert_eq!(
            serde_json::to_value(Request::new("t", Op::Ping)).unwrap(),
            serde_json::json!({"v": 1, "ticket": "t", "op": "ping"})
        );
    }

    #[test]
    fn responses_say_whether_it_worked() {
        let json = |response: Response| serde_json::to_value(response).unwrap();

        assert_eq!(
            json(Response::ok()),
            serde_json::json!({"v": 1, "ok": true})
        );
        assert_eq!(
            json(Response::pong("1.2.3")),
            serde_json::json!({"v": 1, "ok": true, "protocol": 1, "app": "termx", "version": "1.2.3"})
        );
        assert_eq!(
            json(Response::error(ErrorKind::Unauthorized, "no")),
            serde_json::json!({"v": 1, "ok": false, "error": {"kind": "unauthorized", "message": "no"}})
        );
        // A kind from a newer termx is not an error for an older client.
        let response: Response = serde_json::from_str(
            r#"{"v":1,"ok":false,"error":{"kind":"later","message":"x"},"extra":1}"#,
        )
        .unwrap();
        assert_eq!(response.error.unwrap().kind, ErrorKind::Other);
    }

    #[test]
    fn tickets_are_checked() {
        let ticket = "0123456789abcdef0123456789abcdef";
        assert!(is_valid_ticket(ticket));
        assert!(!is_valid_ticket(&ticket.to_uppercase()));
        assert!(!is_valid_ticket(&ticket[1..]));
        assert!(tickets_match(ticket, ticket));
        assert!(!tickets_match(ticket, "0123456789abcdef0123456789abcdee"));
        assert!(!tickets_match(ticket, ""));
    }

    #[test]
    fn session_ids_are_checked() {
        assert!(is_valid_session_id("s-0123456789abcdef"));
        for bad in ["", "a b", "../x", &"x".repeat(65)] {
            assert!(!is_valid_session_id(bad), "{bad:?}");
        }
    }
}
