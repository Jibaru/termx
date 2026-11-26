//! Connects the terminal sessions of `termx-core` to the webview.
//!
//! [`TerminalState`] is the application state behind the session commands
//! (`commands::session`). It owns the [`SessionManager`], remembers which
//! window each session belongs to, so that the sessions of a window can be
//! closed with it, and kills everything when the application exits.
//!
//! # What the frontend receives
//!
//! Each session streams to one [`Channel`] created by the frontend:
//!
//! - **Output** is sent as raw bytes ([`InvokeResponseBody::Raw`]), which
//!   reach JavaScript as an `ArrayBuffer`. They are not wrapped in JSON: a
//!   JSON array of numbers would be several times larger and slow to parse.
//! - **The exit** of the process is sent as a JSON message
//!   ([`SessionEvent::Exit`]) on the same channel. A channel delivers its
//!   messages in the order they were sent, so the frontend always sees the
//!   exit after the last output, and can never miss it by subscribing too
//!   late, which a separate event could not guarantee.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde::{Deserialize, Serialize};
use tauri::ipc::{Channel, InvokeResponseBody, IpcResponse};
use termx_core::diagnostics::tracing::{error, info, warn};
use termx_core::pty::{ExitStatus, PtyCommand, PtyError, PtySize};
use termx_core::session::{SessionId, SessionManager, SessionOptions, SessionSink};

/// What the frontend asks for when it creates a session. Everything but the
/// size is optional: by default the session runs the user's login shell in
/// their home directory.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionRequest {
    /// The program to run instead of the login shell.
    #[serde(default)]
    pub program: Option<String>,
    /// Arguments of `program`. Ignored for the login shell.
    #[serde(default)]
    pub args: Vec<String>,
    /// The working directory, instead of the home directory.
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    /// Width of the terminal, in columns.
    pub cols: u16,
    /// Height of the terminal, in rows.
    pub rows: u16,
}

impl CreateSessionRequest {
    /// The options of the session this request describes.
    #[must_use]
    pub fn options(&self) -> SessionOptions {
        let mut command = match &self.program {
            Some(program) => PtyCommand::new(program).args(&self.args),
            None => PtyCommand::default_shell(),
        };
        if let Some(cwd) = &self.cwd {
            command = command.cwd(cwd);
        }
        SessionOptions::new(command).size(PtySize::new(self.cols, self.rows))
    }
}

/// A message about a session, sent on its channel as JSON. Output is not
/// one of them: it is sent as raw bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "event", rename_all = "camelCase")]
pub enum SessionEvent {
    /// The process has exited and all its output was sent.
    Exit {
        /// The exit code, unless the process was terminated by a signal or
        /// its status is unknown.
        code: Option<u32>,
        /// Description of the signal that terminated the process, if any.
        signal: Option<String>,
    },
}

impl From<&ExitStatus> for SessionEvent {
    fn from(status: &ExitStatus) -> Self {
        Self::Exit {
            code: status.code(),
            signal: status.signal().map(str::to_owned),
        }
    }
}

/// The window (by label) each running session belongs to.
type Owners = Mutex<HashMap<SessionId, String>>;

fn lock(owners: &Owners) -> MutexGuard<'_, HashMap<SessionId, String>> {
    owners.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Application state for the terminal sessions, managed by Tauri.
pub struct TerminalState {
    /// The manager, or why it could not be started.
    manager: Result<Arc<SessionManager>, String>,
    owners: Arc<Owners>,
}

impl TerminalState {
    /// Starts the session manager. A failure is logged and remembered: the
    /// application still runs, and creating a session reports the reason.
    #[must_use]
    pub fn start() -> Self {
        let manager = SessionManager::new().map(Arc::new).map_err(|source| {
            error!(error = %source, "the session manager could not be started");
            format!("terminal sessions are not available: {source}")
        });
        Self {
            manager,
            owners: Arc::default(),
        }
    }

    /// The session manager.
    ///
    /// # Errors
    ///
    /// Fails with the reason when the manager could not be started.
    pub fn manager(&self) -> Result<&Arc<SessionManager>, String> {
        self.manager.as_ref().map_err(Clone::clone)
    }

    /// Starts a session for the window `owner` that streams to `channel`.
    ///
    /// # Errors
    ///
    /// Fails when the manager is not running or the process cannot be
    /// started.
    pub fn create(
        &self,
        owner: &str,
        request: &CreateSessionRequest,
        channel: Channel<InvokeResponseBody>,
    ) -> Result<SessionId, String> {
        let manager = self.manager()?;
        let sink = ChannelSink {
            channel,
            owners: Arc::clone(&self.owners),
            delivery_failed: false,
        };
        // Held until the owner is recorded: a session that ends at once
        // waits here before it removes its entry, instead of leaving one
        // behind by removing it too early.
        let mut owners = lock(&self.owners);
        let id = manager
            .create(&request.options(), sink)
            .map_err(|source| source.to_string())?;
        owners.insert(id, owner.to_owned());
        Ok(id)
    }

    /// Closes every session of the window `owner`, killing their processes,
    /// and returns how many there were. Used when a window goes away or
    /// reloads its page: nothing is left to display those sessions.
    pub fn close_owned_by(&self, owner: &str) -> usize {
        let ids: Vec<SessionId> = {
            let mut owners = lock(&self.owners);
            let ids: Vec<SessionId> = owners
                .iter()
                .filter(|(_, label)| label.as_str() == owner)
                .map(|(&id, _)| id)
                .collect();
            for id in &ids {
                owners.remove(id);
            }
            ids
        };
        let Ok(manager) = self.manager() else {
            return 0;
        };
        // All at once: processes that resist are waited for together.
        let closed = manager.close_all(&ids);
        if closed > 0 {
            info!(closed, "closed the sessions of a window");
        }
        closed
    }

    /// Closes one session, killing its process. Returns whether it existed.
    pub fn close(&self, id: SessionId) -> bool {
        lock(&self.owners).remove(&id);
        self.manager().is_ok_and(|manager| manager.close(id))
    }

    /// Kills every session. Called when the application exits, so that no
    /// shell outlives it.
    pub fn shutdown(&self) {
        if let Ok(manager) = self.manager() {
            manager.shutdown();
        }
        lock(&self.owners).clear();
    }

    /// The window a session belongs to.
    #[cfg(test)]
    fn owner_of(&self, id: SessionId) -> Option<String> {
        lock(&self.owners).get(&id).cloned()
    }
}

/// Whether an operation failed only because the session has ended, which is
/// not an error for the frontend: it normally still sends input, resizes
/// and acknowledgements for a moment after a process exits.
#[must_use]
pub fn session_is_gone(error: &PtyError) -> bool {
    matches!(error, PtyError::UnknownSession(_) | PtyError::Exited)
}

/// Delivers what a session produces to the channel of its frontend.
struct ChannelSink {
    channel: Channel<InvokeResponseBody>,
    owners: Arc<Owners>,
    /// A failed delivery was logged already: once is enough.
    delivery_failed: bool,
}

impl ChannelSink {
    fn send(&mut self, session: SessionId, body: InvokeResponseBody) {
        if let Err(source) = self.channel.send(body)
            && !self.delivery_failed
        {
            self.delivery_failed = true;
            warn!(
                session_id = session.as_u64(),
                error = %source,
                "could not deliver the output of a session to its window"
            );
        }
    }
}

impl SessionSink for ChannelSink {
    fn output(&mut self, session: SessionId, bytes: Vec<u8>) {
        self.send(session, InvokeResponseBody::Raw(bytes));
    }

    fn exited(&mut self, session: SessionId, status: ExitStatus) {
        lock(&self.owners).remove(&session);
        info!(
            session_id = session.as_u64(),
            exit_code = status.code(),
            signalled = status.signal().is_some(),
            "session ended"
        );
        match SessionEvent::from(&status).body() {
            Ok(body) => self.send(session, body),
            Err(source) => warn!(
                session_id = session.as_u64(),
                error = %source,
                "could not encode the exit of a session"
            ),
        }
    }

    fn is_alive(&self) -> bool {
        // A channel that failed belongs to a webview that is gone.
        !self.delivery_failed
    }
}

/// The build number of Windows, which xterm.js needs to know how the
/// pseudo-console behaves (`windowsPty.buildNumber`). `None` elsewhere, and
/// when the version cannot be read.
#[must_use]
pub fn windows_build_number() -> Option<u32> {
    if !cfg!(windows) {
        return None;
    }
    match os_info::get().version() {
        os_info::Version::Semantic(_, _, build) => u32::try_from(*build).ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{self, Receiver};
    use std::time::{Duration, Instant};

    use super::*;

    /// Upper bound for any wait: generous, because CI machines are slow
    /// (starting PowerShell alone can take several seconds).
    const TIMEOUT: Duration = Duration::from_secs(60);

    /// What a frontend would receive on the channel of a session.
    #[derive(Debug, PartialEq, Eq)]
    enum Message {
        Output(Vec<u8>),
        Json(serde_json::Value),
    }

    /// A channel whose messages arrive in the returned receiver instead of
    /// in a webview.
    fn channel() -> (Channel<InvokeResponseBody>, Receiver<Message>) {
        let (sender, receiver) = mpsc::channel();
        let channel = Channel::new(move |body| {
            let message = match body {
                InvokeResponseBody::Raw(bytes) => Message::Output(bytes),
                InvokeResponseBody::Json(json) => {
                    Message::Json(serde_json::from_str(&json).expect("invalid JSON"))
                }
            };
            let _ = sender.send(message);
            Ok(())
        });
        (channel, receiver)
    }

    /// A program that prints `ready` and then waits for its input to end.
    fn waiting_program() -> CreateSessionRequest {
        let (program, args): (&str, &[&str]) = if cfg!(windows) {
            (
                "powershell.exe",
                &[
                    "-NoLogo",
                    "-NoProfile",
                    "-Command",
                    "Write-Output 'ready'; while ($null -ne [Console]::ReadLine()) {}",
                ],
            )
        } else {
            (
                "/bin/sh",
                &["-c", "echo ready; while read line; do :; done"],
            )
        };
        CreateSessionRequest {
            program: Some(program.to_owned()),
            args: args.iter().map(|&arg| arg.to_owned()).collect(),
            cols: 80,
            rows: 24,
            ..CreateSessionRequest::default()
        }
    }

    /// A program that prints `bye` and exits with code 3.
    fn exiting_program() -> CreateSessionRequest {
        let (program, args): (&str, &[&str]) = if cfg!(windows) {
            (
                "powershell.exe",
                &[
                    "-NoLogo",
                    "-NoProfile",
                    "-Command",
                    "Write-Output 'bye'; exit 3",
                ],
            )
        } else {
            ("/bin/sh", &["-c", "echo bye; exit 3"])
        };
        CreateSessionRequest {
            program: Some(program.to_owned()),
            args: args.iter().map(|&arg| arg.to_owned()).collect(),
            cols: 80,
            rows: 24,
            ..CreateSessionRequest::default()
        }
    }

    /// Receives messages until the output contains `needle`.
    fn wait_for_output(messages: &Receiver<Message>, needle: &str) {
        let mut output = Vec::new();
        while !String::from_utf8_lossy(&output).contains(needle) {
            match messages.recv_timeout(TIMEOUT) {
                Ok(Message::Output(bytes)) => output.extend_from_slice(&bytes),
                other => panic!("expected output containing {needle:?}, got {other:?}"),
            }
        }
    }

    /// Whether a process with this identifier exists.
    fn is_running(process_id: u32) -> bool {
        let pid = process_id.to_string();
        if cfg!(windows) {
            let filter = format!("PID eq {pid}");
            let output = std::process::Command::new("tasklist")
                .args(["/FI", &filter, "/FO", "CSV", "/NH"])
                .output()
                .expect("cannot run tasklist");
            String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\""))
        } else {
            std::process::Command::new("kill")
                .args(["-0", &pid])
                .stderr(std::process::Stdio::null())
                .status()
                .expect("cannot run kill")
                .success()
        }
    }

    fn wait_until_gone(process_ids: &[u32]) {
        let deadline = Instant::now() + TIMEOUT;
        while process_ids.iter().any(|&pid| is_running(pid)) {
            assert!(
                Instant::now() < deadline,
                "a process outlived its session: {process_ids:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn a_request_without_a_program_asks_for_the_login_shell() {
        let request: CreateSessionRequest =
            serde_json::from_value(serde_json::json!({ "cols": 120, "rows": 40 })).unwrap();

        let options = request.options();

        assert_eq!(options.command, PtyCommand::default_shell());
        assert_eq!(options.size, PtySize::new(120, 40));
    }

    #[test]
    fn a_request_can_name_the_program_its_arguments_and_its_directory() {
        let request: CreateSessionRequest = serde_json::from_value(serde_json::json!({
            "program": "git",
            "args": ["status", "--short"],
            "cwd": "/tmp",
            "cols": 80,
            "rows": 24,
        }))
        .unwrap();

        assert_eq!(
            request.options().command,
            PtyCommand::new("git")
                .args(["status", "--short"])
                .cwd("/tmp")
        );
    }

    #[test]
    fn the_exit_is_encoded_as_a_tagged_json_message() {
        let exit = SessionEvent::Exit {
            code: Some(3),
            signal: None,
        };

        assert_eq!(
            serde_json::to_value(&exit).unwrap(),
            serde_json::json!({ "event": "exit", "code": 3, "signal": null })
        );
    }

    #[test]
    fn output_arrives_as_raw_bytes_and_the_exit_as_the_last_message() {
        let state = TerminalState::start();
        let (channel, messages) = channel();

        let id = state.create("main", &exiting_program(), channel).unwrap();

        let mut output = Vec::new();
        let exit = loop {
            match messages.recv_timeout(TIMEOUT).expect("no exit was sent") {
                Message::Output(bytes) => output.extend_from_slice(&bytes),
                Message::Json(json) => break json,
            }
        };
        assert!(String::from_utf8_lossy(&output).contains("bye"));
        assert_eq!(
            exit,
            serde_json::json!({ "event": "exit", "code": 3, "signal": null })
        );

        // Nothing follows the exit, and the session is forgotten.
        assert!(messages.recv_timeout(Duration::from_millis(300)).is_err());
        assert_eq!(state.owner_of(id), None);
        assert!(!state.close(id));
    }

    #[test]
    fn input_resizes_and_acknowledgements_reach_the_session() {
        let state = TerminalState::start();
        let (channel, messages) = channel();
        let id = state.create("main", &waiting_program(), channel).unwrap();
        wait_for_output(&messages, "ready");
        let manager = state.manager().unwrap();

        manager.write(id, b"hello\r").unwrap();
        manager.resize(id, PtySize::new(100, 30)).unwrap();
        manager.acknowledge(id, 1024);

        assert_eq!(state.owner_of(id).as_deref(), Some("main"));
        assert!(state.close(id));
        assert_eq!(state.owner_of(id), None);
        // The session is gone, which the commands do not treat as a failure.
        assert!(session_is_gone(&manager.write(id, b"x").unwrap_err()));
        assert!(session_is_gone(
            &manager.resize(id, PtySize::new(80, 24)).unwrap_err()
        ));
        assert!(!session_is_gone(&PtyError::InvalidSize {
            cols: 0,
            rows: 0
        }));
    }

    #[test]
    fn closing_a_window_kills_its_sessions_and_only_those() {
        let state = TerminalState::start();
        let manager = state.manager().unwrap();
        let mut receivers = Vec::new();
        let mut create = |owner: &str| {
            let (channel, messages) = channel();
            let id = state.create(owner, &waiting_program(), channel).unwrap();
            wait_for_output(&messages, "ready");
            receivers.push(messages);
            (id, manager.process_id(id).expect("no process id"))
        };
        let (first, first_pid) = create("main");
        let (second, second_pid) = create("main");
        let (other, other_pid) = create("other");

        assert_eq!(state.close_owned_by("main"), 2);

        wait_until_gone(&[first_pid, second_pid]);
        assert!(is_running(other_pid), "a session of another window died");
        assert_eq!(manager.ids(), [other]);
        assert_eq!(state.owner_of(first), None);
        assert_eq!(state.owner_of(second), None);
        assert_eq!(state.owner_of(other).as_deref(), Some("other"));
        assert_eq!(state.close_owned_by("main"), 0, "nothing is left to close");

        state.shutdown();
        wait_until_gone(&[other_pid]);
    }

    #[test]
    fn shutting_down_leaves_no_shell_behind() {
        let state = TerminalState::start();
        let manager = state.manager().unwrap();
        let mut receivers = Vec::new();
        let mut process_ids = Vec::new();

        // The real thing: the user's login shell, as the application
        // starts it.
        let login_shell = CreateSessionRequest {
            cols: 80,
            rows: 24,
            ..CreateSessionRequest::default()
        };
        for request in [login_shell, waiting_program(), waiting_program()] {
            let (channel, messages) = channel();
            let id = state.create("main", &request, channel).unwrap();
            process_ids.push(manager.process_id(id).expect("no process id"));
            receivers.push(messages);
        }
        assert!(process_ids.iter().all(|&pid| is_running(pid)));

        state.shutdown();

        wait_until_gone(&process_ids);
        assert!(manager.is_empty());
        let (channel, _messages) = channel();
        let error = state
            .create("main", &waiting_program(), channel)
            .unwrap_err();
        assert!(error.contains("shut down"), "{error}");
    }

    #[test]
    fn the_windows_build_number_is_only_reported_on_windows() {
        assert_eq!(windows_build_number().is_some(), cfg!(windows));
    }
}
