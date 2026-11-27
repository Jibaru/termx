//! The hook sender: what `termx-cli hook` does when Claude Code runs it.
//!
//! Claude Code runs the hooks of the termx plugin for every event it
//! reports, in every session, inside termx or not: in another terminal, in
//! CI, under `claude -p`. So the sender:
//!
//! - returns at once, without reading anything, when the variables of
//!   termx ([`crate::ENV_VARIABLES`]) are missing or malformed: the hook did
//!   not run in a terminal of termx;
//! - never writes to stdout or stderr, and always exits 0, whatever goes
//!   wrong. On `UserPromptSubmit` and `SessionStart` what a hook prints is
//!   given to Claude as context, and an exit code other than 0 shows a
//!   "hook error" or, with 2, blocks what Claude was doing;
//! - gives up on termx after [`HOOK_TIMEOUT`];
//! - sends only what termx needs ([`extract`]): never the prompt, the
//!   input or output of a tool, a message, a path or the transcript.

use std::io::Read;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde::de::IgnoredAny;

use crate::{
    ENV_SESSION_ID, ENV_SOCKET, ENV_TICKET, HookEvent, Op, Request, client, is_valid_session_id,
    is_valid_ticket,
};

/// The longest wait for termx, from connecting to reading its answer. A
/// local socket answers in well under a millisecond; this only bounds what
/// a termx that is stuck can cost a hook.
pub const HOOK_TIMEOUT: Duration = Duration::from_millis(300);

/// The most of the standard input that is read. What Claude Code sends
/// after a tool ran includes the output of the tool, which can be large;
/// the fields that matter come first or last, and a payload larger than
/// this is dropped rather than read for ever.
pub const MAX_INPUT_BYTES: u64 = 64 * 1024 * 1024;

/// The tools whose name is reported: those that ask the user something,
/// which is what termx shows. Other tool names (including those of MCP
/// servers, which can name private systems) are never sent.
pub const REPORTED_TOOLS: &[&str] = &["AskUserQuestion", "ExitPlanMode"];

/// What became of one run of the hook, for the tests. The program exits 0
/// in every case.
#[derive(Debug)]
pub enum Outcome {
    /// Not in a terminal of termx: nothing was read or sent.
    OutsideTermx,
    /// The input was not the JSON of a hook.
    Unreadable,
    /// termx received the event.
    Delivered,
    /// termx refused the event, or could not be reached in time.
    Failed(String),
}

/// Where termx is, from the environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// [`ENV_SESSION_ID`].
    pub session: String,
    /// [`ENV_SOCKET`].
    pub socket: String,
    /// [`ENV_TICKET`].
    pub ticket: String,
}

impl Target {
    /// Reads the variables of termx through `variable`. `None` unless all
    /// three are there and well formed.
    pub fn from_env(variable: impl Fn(&str) -> Option<String>) -> Option<Self> {
        let session = variable(ENV_SESSION_ID).filter(|id| is_valid_session_id(id))?;
        let socket = variable(ENV_SOCKET).filter(|socket| !socket.is_empty())?;
        let ticket = variable(ENV_TICKET).filter(|ticket| is_valid_ticket(ticket))?;
        Some(Self {
            session,
            socket,
            ticket,
        })
    }
}

/// Runs the hook: reads what Claude Code sent on `input` and reports it to
/// termx. `variable` reads the environment and `pid` finds the process of
/// Claude Code (see [`crate::parent`]); both are parameters for the tests.
pub fn run(
    variable: impl Fn(&str) -> Option<String>,
    input: impl Read,
    pid: impl FnOnce() -> Option<u32>,
) -> Outcome {
    let Some(target) = Target::from_env(variable) else {
        return Outcome::OutsideTermx;
    };
    let Some(mut event) = extract(input, now_us()) else {
        return Outcome::Unreadable;
    };
    event.pid = pid();
    let line = Request::new(
        &target.ticket,
        Op::Hook {
            session: target.session.clone(),
            event,
        },
    )
    .to_line();
    match client::send(&target.socket, line, HOOK_TIMEOUT) {
        Ok(response) if response.ok => Outcome::Delivered,
        Ok(response) => Outcome::Failed(
            response
                .error
                .map_or_else(|| "refused".to_owned(), |error| error.message),
        ),
        Err(error) => Outcome::Failed(error.to_string()),
    }
}

/// The fields of the input of a hook that are read. Everything else is
/// skipped without being kept: the prompt, the input and output of tools,
/// messages, paths.
#[derive(Deserialize)]
struct RawHook {
    #[serde(default)]
    hook_event_name: Text,
    #[serde(default)]
    session_id: Text,
    #[serde(default)]
    notification_type: Text,
    #[serde(default)]
    tool_name: Text,
    #[serde(default)]
    source: Text,
    #[serde(default)]
    reason: Text,
    #[serde(default)]
    agent_id: Text,
}

/// A field that is used when it is a string, and skipped otherwise: a
/// field of another type in a future version of Claude Code must not cost
/// the whole event.
#[derive(Default)]
struct Text(Option<String>);

impl<'de> Deserialize<'de> for Text {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;

        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = Text;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("any value")
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Text, E> {
                Ok(Text(Some(value.to_owned())))
            }

            fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Text, E> {
                Ok(Text(Some(value)))
            }

            fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<Text, E> {
                Ok(Text(None))
            }

            fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<Text, E> {
                Ok(Text(None))
            }

            fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<Text, E> {
                Ok(Text(None))
            }

            fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Text, E> {
                Ok(Text(None))
            }

            fn visit_unit<E: serde::de::Error>(self) -> Result<Text, E> {
                Ok(Text(None))
            }

            fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Text, A::Error> {
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                Ok(Text(None))
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Text, A::Error> {
                while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
                Ok(Text(None))
            }
        }

        deserializer.deserialize_any(Visitor)
    }
}

/// Reads the JSON Claude Code gives a hook on its standard input and keeps
/// what termx needs, checked: `None` when it is not the input of a hook.
/// `at` is when the hook started, in microseconds since 1970.
pub fn extract(input: impl Read, at: u64) -> Option<HookEvent> {
    let mut deserializer =
        serde_json::Deserializer::from_reader(std::io::BufReader::new(input.take(MAX_INPUT_BYTES)));
    let raw = RawHook::deserialize(&mut deserializer).ok()?;
    let name = raw.hook_event_name.0.filter(|name| {
        (1..=64).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_alphabetic())
    })?;
    Some(HookEvent {
        name,
        claude_session_id: raw.session_id.0.filter(|id| is_claude_session_id(id)),
        notification_type: raw.notification_type.0.filter(|value| is_word(value)),
        tool: raw
            .tool_name
            .0
            .filter(|tool| REPORTED_TOOLS.contains(&tool.as_str())),
        source: raw.source.0.filter(|value| is_word(value)),
        reason: raw.reason.0.filter(|value| is_word(value)),
        agent: raw.agent_id.0.filter(|id| is_claude_session_id(id)),
        pid: None,
        at,
    })
}

/// Shaped like the identifier of a conversation of Claude Code: letters,
/// digits, `-` and `_`, at most 128 bytes. That is also what termx accepts
/// on the command line of `claude --resume`.
fn is_claude_session_id(id: &str) -> bool {
    (1..=128).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// A short machine word: `permission_prompt`, `startup`, `prompt_input_exit`.
fn is_word(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// Microseconds since 1970.
fn now_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TICKET: &str = "0123456789abcdef0123456789abcdef";

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        }
    }

    /// An input that fails the test if it is read.
    struct Untouchable;

    impl Read for Untouchable {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            panic!("the input was read");
        }
    }

    #[test]
    fn outside_termx_nothing_is_read() {
        let cases: &[&[(&str, &str)]] = &[
            &[],
            &[(ENV_SESSION_ID, "s-1"), (ENV_SOCKET, "/tmp/x.sock")],
            &[
                (ENV_SESSION_ID, "s-1"),
                (ENV_SOCKET, "/tmp/x.sock"),
                (ENV_TICKET, "short"),
            ],
            &[
                (ENV_SESSION_ID, "../x"),
                (ENV_SOCKET, "/tmp/x.sock"),
                (ENV_TICKET, TICKET),
            ],
            &[
                (ENV_SESSION_ID, "s-1"),
                (ENV_SOCKET, ""),
                (ENV_TICKET, TICKET),
            ],
        ];
        for vars in cases {
            let outcome = run(env(vars), Untouchable, || panic!("looked for the parent"));
            assert!(
                matches!(outcome, Outcome::OutsideTermx),
                "{vars:?}: {outcome:?}"
            );
        }
    }

    #[test]
    fn only_what_termx_needs_is_kept() {
        let input = serde_json::json!({
            "session_id": "0b9d6f7e-3c1a-4f3e-9a55-2f6f1f0f8a10",
            "transcript_path": "/Users/someone/.claude/projects/x/0b9d.jsonl",
            "cwd": "/Users/someone/secret-project",
            "permission_mode": "default",
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": "curl -H 'Authorization: Bearer sk-live-123' https://x"},
            "tool_use_id": "toolu_01",
            "prompt": "my private prompt",
        });

        let event = extract(input.to_string().as_bytes(), 42).unwrap();

        assert_eq!(
            event,
            HookEvent {
                name: "PreToolUse".to_owned(),
                claude_session_id: Some("0b9d6f7e-3c1a-4f3e-9a55-2f6f1f0f8a10".to_owned()),
                notification_type: None,
                tool: None,
                source: None,
                reason: None,
                agent: None,
                pid: None,
                at: 42,
            }
        );
        let sent = String::from_utf8(
            Request::new(
                TICKET,
                Op::Hook {
                    session: "s-1".to_owned(),
                    event,
                },
            )
            .to_line(),
        )
        .unwrap();
        for secret in [
            "sk-live",
            "secret-project",
            "private prompt",
            "transcript",
            "Bash",
            "toolu",
        ] {
            assert!(!sent.contains(secret), "{secret} was sent: {sent}");
        }
    }

    #[test]
    fn the_fields_of_each_event_are_kept() {
        let event = |json: serde_json::Value| extract(json.to_string().as_bytes(), 1).unwrap();

        let notification = event(serde_json::json!({
            "hook_event_name": "Notification", "session_id": "a",
            "message": "Claude needs your permission to use Bash", "title": "Claude Code",
            "notification_type": "permission_prompt",
        }));
        assert_eq!(
            notification.notification_type.as_deref(),
            Some("permission_prompt")
        );

        let start = event(serde_json::json!({
            "hook_event_name": "SessionStart", "session_id": "a", "source": "resume", "model": "x",
        }));
        assert_eq!(start.source.as_deref(), Some("resume"));

        let end = event(serde_json::json!({
            "hook_event_name": "SessionEnd", "session_id": "a", "reason": "prompt_input_exit",
        }));
        assert_eq!(end.reason.as_deref(), Some("prompt_input_exit"));

        let question = event(serde_json::json!({
            "hook_event_name": "PreToolUse", "session_id": "a", "tool_name": "AskUserQuestion",
            "tool_input": {"questions": [{"question": "Which one?"}]},
        }));
        assert_eq!(question.tool.as_deref(), Some("AskUserQuestion"));

        let subagent = event(serde_json::json!({
            "hook_event_name": "PostToolUse", "session_id": "a", "agent_id": "agent-1",
            "agent_type": "Explore", "tool_name": "Read", "tool_response": {"content": "x"},
        }));
        assert_eq!(subagent.agent.as_deref(), Some("agent-1"));
        assert_eq!(subagent.tool, None);
    }

    #[test]
    fn values_that_are_not_what_they_should_be_are_dropped() {
        let event = extract(
            serde_json::json!({
                "hook_event_name": "Stop",
                "session_id": "$(rm -rf ~)",
                "notification_type": "Not A Word",
                "source": "x".repeat(65),
                "reason": 5,
            })
            .to_string()
            .as_bytes(),
            1,
        );
        // A field of another type is dropped, not the event.
        let event = event.unwrap();
        assert_eq!(event.claude_session_id, None);
        assert_eq!(event.notification_type, None);
        assert_eq!(event.source, None);
        assert_eq!(event.reason, None);

        for bad in [
            &b""[..],
            b"not json",
            b"[]",
            b"{\"hook_event_name\":\"Pre Tool\"}",
            b"{\"hook_event_name\":7}",
            b"{}",
        ] {
            assert!(
                extract(bad, 1).is_none(),
                "{:?}",
                String::from_utf8_lossy(bad)
            );
        }
    }

    #[test]
    fn a_large_tool_output_is_skipped_without_being_kept() {
        let big = "x".repeat(8 * 1024 * 1024);
        let input = format!(
            r#"{{"hook_event_name":"PostToolUse","tool_response":{{"stdout":"{big}"}},"session_id":"abc"}}"#
        );

        let event = extract(input.as_bytes(), 1).unwrap();

        assert_eq!(event.name, "PostToolUse");
        assert_eq!(event.claude_session_id.as_deref(), Some("abc"));
    }

    #[test]
    fn a_socket_that_is_gone_fails_quietly_and_quickly() {
        let started = std::time::Instant::now();
        let socket = if cfg!(windows) {
            r"\\.\pipe\termx-hook-test-nothing-here"
        } else {
            "/tmp/termx-hook-test-nothing-here.sock"
        };

        let outcome = run(
            env(&[
                (ENV_SESSION_ID, "s-1"),
                (ENV_SOCKET, socket),
                (ENV_TICKET, TICKET),
            ]),
            &br#"{"hook_event_name":"Stop","session_id":"a"}"#[..],
            || Some(1),
        );

        assert!(matches!(outcome, Outcome::Failed(_)), "{outcome:?}");
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
