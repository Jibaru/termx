//! `termx-cli hook`, run as Claude Code runs it: a process with the event
//! on its standard input.

#![allow(clippy::unwrap_used)]

use std::io::Write;
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use termx_core::socket::{Server, ServerConfig};
use termx_protocol::{ENV_SESSION_ID, ENV_SOCKET, ENV_TICKET, Op, Response};

const BINARY: &str = env!("CARGO_BIN_EXE_termx-cli");

/// What Claude Code sends a `PreToolUse` hook, with things that must never
/// leave the hook.
fn pre_tool_use() -> String {
    serde_json::json!({
        "session_id": "0b9d6f7e-3c1a-4f3e-9a55-2f6f1f0f8a10",
        "prompt_id": "p-1",
        "transcript_path": "/home/someone/.claude/projects/-home-someone-secret-project/0b9d.jsonl",
        "cwd": "/home/someone/secret-project",
        "scratchpad_dir": "/tmp/claude-1000/scratch",
        "permission_mode": "default",
        "effort": {"level": "high"},
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": "curl -H 'Authorization: Bearer sk-ant-api03-SECRETVALUE' https://internal.example",
            "description": "Call the internal API"
        },
        "tool_use_id": "toolu_01ABCDEF",
    })
    .to_string()
}

/// What Claude Code sends a `UserPromptSubmit` hook.
fn user_prompt_submit() -> String {
    serde_json::json!({
        "session_id": "0b9d6f7e-3c1a-4f3e-9a55-2f6f1f0f8a10",
        "transcript_path": "/home/someone/.claude/projects/x/0b9d.jsonl",
        "cwd": "/home/someone/secret-project",
        "permission_mode": "default",
        "hook_event_name": "UserPromptSubmit",
        "prompt": "the password is hunter2, please rotate it",
        "session_title": "Rotate the credentials",
    })
    .to_string()
}

/// Runs the hook with `input` and only the variables in `env`.
fn run_hook(env: &[(&str, &str)], input: &str) -> (Output, Duration) {
    let mut command = Command::new(BINARY);
    command
        .arg("hook")
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // What a process needs to start at all.
    for name in ["PATH", "SystemRoot", "SYSTEMROOT", "TMPDIR", "TEMP", "TMP"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command.envs(env.iter().copied());
    let started = Instant::now();
    let mut child = command.spawn().unwrap();
    // The hook may not read its input at all, and close it early.
    let _ = child.stdin.take().unwrap().write_all(input.as_bytes());
    let output = child.wait_with_output().unwrap();
    (output, started.elapsed())
}

fn assert_silent_success(output: &Output) {
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stdout.is_empty(), "stdout: {:?}", output.stdout);
    assert!(
        output.stderr.is_empty(),
        "stderr: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A server that records what it is handed, after `delay`.
fn server(
    delay: Duration,
    max_connections: usize,
) -> (Server, Arc<Mutex<Vec<Op>>>, Option<tempfile::TempDir>) {
    let ops = Arc::new(Mutex::new(Vec::new()));
    let handler = {
        let ops = Arc::clone(&ops);
        move |op: Op| {
            std::thread::sleep(delay);
            ops.lock().unwrap().push(op);
            Response::ok()
        }
    };
    let mut config = ServerConfig::default();
    config.max_connections = max_connections;
    let dir = if cfg!(unix) {
        Some(
            tempfile::Builder::new()
                .prefix("termx-cli-")
                .tempdir_in("/tmp")
                .unwrap(),
        )
    } else {
        None
    };
    config.dir = dir.as_ref().map(|dir| dir.path().join("run"));
    (Server::start(Arc::new(handler), config).unwrap(), ops, dir)
}

fn termx_env(server: &Server) -> Vec<(&'static str, String)> {
    vec![
        (ENV_SESSION_ID, "s-0123456789abcdef".to_owned()),
        (ENV_SOCKET, server.address().to_owned()),
        (ENV_TICKET, server.ticket().to_owned()),
    ]
}

fn as_refs<'a>(env: &'a [(&'static str, String)]) -> Vec<(&'static str, &'a str)> {
    env.iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect()
}

fn median(mut times: Vec<Duration>) -> Duration {
    times.sort();
    times[times.len() / 2]
}

/// The bound is generous: shared CI machines, debug builds, antivirus on
/// Windows. What matters is that it is milliseconds, not the hundreds a
/// Node or PowerShell start costs; the measurements are printed.
const MEDIAN_BOUND: Duration = Duration::from_millis(150);

#[test]
fn outside_termx_it_exits_at_once_and_says_nothing() {
    let mut times = Vec::new();
    for _ in 0..20 {
        let (output, elapsed) = run_hook(&[], &pre_tool_use());
        assert_silent_success(&output);
        times.push(elapsed);
    }
    let median = median(times.clone());
    println!(
        "hook outside termx: median {:.2} ms, max {:.2} ms over {} runs",
        median.as_secs_f64() * 1000.0,
        times.iter().max().unwrap().as_secs_f64() * 1000.0,
        times.len()
    );
    assert!(median < MEDIAN_BOUND, "median {median:?}");
}

#[test]
fn inside_termx_it_delivers_quickly_and_says_nothing() {
    let (server, ops, _dir) = server(Duration::ZERO, 8);
    let env = termx_env(&server);
    let mut times = Vec::new();
    for _ in 0..20 {
        let (output, elapsed) = run_hook(&as_refs(&env), &pre_tool_use());
        assert_silent_success(&output);
        times.push(elapsed);
    }
    let median = median(times.clone());
    println!(
        "hook inside termx (delivered): median {:.2} ms, max {:.2} ms over {} runs",
        median.as_secs_f64() * 1000.0,
        times.iter().max().unwrap().as_secs_f64() * 1000.0,
        times.len()
    );
    assert!(median < MEDIAN_BOUND, "median {median:?}");
    assert_eq!(ops.lock().unwrap().len(), 20);
}

#[test]
fn only_the_event_and_its_few_fields_reach_termx() {
    let (server, ops, _dir) = server(Duration::ZERO, 8);
    let env = termx_env(&server);

    for input in [pre_tool_use(), user_prompt_submit()] {
        let (output, _) = run_hook(&as_refs(&env), &input);
        assert_silent_success(&output);
    }

    let ops = ops.lock().unwrap();
    assert_eq!(ops.len(), 2);
    let received = serde_json::to_string(&*ops).unwrap();
    for secret in [
        "sk-ant",
        "SECRETVALUE",
        "internal.example",
        "hunter2",
        "rotate",
        "Rotate the credentials",
        "secret-project",
        "someone",
        "transcript",
        "jsonl",
        "toolu_",
        "Bash",
        "scratch",
        "p-1",
        "high",
    ] {
        assert!(
            !received.contains(secret),
            "{secret:?} reached termx: {received}"
        );
    }
    let Op::Hook { session, event } = &ops[0] else {
        panic!("not a hook: {:?}", ops[0]);
    };
    assert_eq!(session, "s-0123456789abcdef");
    assert_eq!(event.name, "PreToolUse");
    assert_eq!(
        event.claude_session_id.as_deref(),
        Some("0b9d6f7e-3c1a-4f3e-9a55-2f6f1f0f8a10")
    );
    assert!(event.at > 0);
}

#[test]
fn a_socket_that_is_gone_costs_nothing() {
    let missing = if cfg!(windows) {
        r"\\.\pipe\termx-cli-test-missing".to_owned()
    } else {
        "/tmp/termx-cli-test-missing.sock".to_owned()
    };
    let env = [
        (ENV_SESSION_ID, "s-1"),
        (ENV_SOCKET, missing.as_str()),
        (ENV_TICKET, "0123456789abcdef0123456789abcdef"),
    ];

    let (output, elapsed) = run_hook(&env, &pre_tool_use());

    assert_silent_success(&output);
    assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");
}

#[test]
fn a_slow_termx_costs_the_timeout_and_no_more() {
    // termx takes 5 s to handle the event.
    let (server, _ops, _dir) = server(Duration::from_secs(5), 8);
    let env = termx_env(&server);

    let (output, elapsed) = run_hook(&as_refs(&env), &pre_tool_use());

    assert_silent_success(&output);
    assert!(
        elapsed < termx_protocol::hook::HOOK_TIMEOUT + Duration::from_secs(2),
        "{elapsed:?}"
    );
}

#[test]
fn a_termx_with_every_place_taken_costs_the_timeout_and_no_more() {
    // One place, held by a connection that says nothing.
    let (server, _ops, _dir) = server(Duration::ZERO, 1);
    let held = hold_connection(&server);
    let env = termx_env(&server);

    let (output, elapsed) = run_hook(&as_refs(&env), &pre_tool_use());

    assert_silent_success(&output);
    assert!(
        elapsed < termx_protocol::hook::HOOK_TIMEOUT + Duration::from_secs(2),
        "{elapsed:?}"
    );
    drop(held);
}

/// A connection that says nothing, open until it is dropped.
fn hold_connection(server: &Server) -> Box<dyn std::any::Any> {
    #[cfg(unix)]
    let held = std::os::unix::net::UnixStream::connect(server.address()).unwrap();
    #[cfg(windows)]
    let held = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(server.address())
        .unwrap();
    Box::new(held)
}

#[test]
fn input_that_is_not_an_event_is_ignored_silently() {
    let (server, ops, _dir) = server(Duration::ZERO, 8);
    let env = termx_env(&server);

    for input in ["", "garbage", "{\"hook_event_name\": 5}", "[1,2,3]"] {
        let (output, _) = run_hook(&as_refs(&env), input);
        assert_silent_success(&output);
    }
    assert!(ops.lock().unwrap().is_empty());
}

#[test]
fn without_a_command_it_exits_0_and_prints_nothing_to_a_program() {
    let output = Command::new(BINARY).stdin(Stdio::null()).output().unwrap();
    assert_silent_success(&output);
}
