//! Connects the terminal sessions of `termx-core` to the webview.
//!
//! [`TerminalState`] is the application state behind the session commands
//! (`commands::session`). It owns the [`SessionManager`], remembers which
//! window each terminal belongs to, so that the terminals of a window can be
//! closed with it, and kills everything when the application exits.
//!
//! # Terminals and sessions
//!
//! A *terminal* here is one running process in a pseudo-terminal,
//! identified by a number ([`SessionId`]) for as long as it runs. A
//! *session* is what the user sees in the sidebar, identified by a
//! [`SessionKey`] for its whole life (`termx_core::workspace`). A session
//! gets a new terminal every time it is started;
//! [`TerminalState::start_session`] is where the two meet: it asks the
//! workspace what to run, runs it, and tells the workspace what became of
//! it.
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
//!
//! Nothing here logs what a session runs beyond the path of its shell:
//! never a command line, an argument or an environment value.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde::Serialize;
use tauri::ipc::{Channel, InvokeResponseBody, IpcResponse};
use termx_core::diagnostics::tracing::{error, info, warn};
use termx_core::pty::{ExitStatus, PtyError, PtySize};
use termx_core::session::{OutputFilter, SessionId, SessionManager, SessionOptions, SessionSink};
use termx_core::workspace::{
    ActivityHandle, LaunchFailure, LaunchMode, SessionKey, SessionState, Workspace,
};

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
        /// Why the session could not do its job, when termx knows: for
        /// instance that `claude` is not installed. Its message is for the
        /// user. Absent for an ordinary exit.
        #[serde(skip_serializing_if = "Option::is_none")]
        failure: Option<LaunchFailure>,
    },
}

impl From<&ExitStatus> for SessionEvent {
    fn from(status: &ExitStatus) -> Self {
        Self::Exit {
            code: status.code(),
            signal: status.signal().map(str::to_owned),
            failure: None,
        }
    }
}

/// The session of the workspace a terminal runs for.
#[derive(Clone)]
struct SessionLink {
    workspace: Arc<Workspace>,
    key: SessionKey,
    /// Which start of the session this terminal is. See
    /// [`termx_core::workspace::Launch::generation`].
    generation: u64,
}

/// What is remembered of a running terminal.
struct Owner {
    /// The label of the window the terminal belongs to.
    window: String,
    /// The session it runs for, unless it is a bare terminal.
    session: Option<SessionLink>,
}

/// The running terminals.
type Owners = Mutex<HashMap<SessionId, Owner>>;

fn lock(owners: &Owners) -> MutexGuard<'_, HashMap<SessionId, Owner>> {
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

    /// Starts a bare terminal for the window `owner`, running `options`
    /// and streaming to `channel`. It belongs to no session of the
    /// workspace.
    ///
    /// # Errors
    ///
    /// Fails when the manager is not running or the process cannot be
    /// started.
    pub fn create(
        &self,
        owner: &str,
        options: &SessionOptions,
        channel: Channel<InvokeResponseBody>,
    ) -> Result<SessionId, String> {
        self.spawn(owner, options, None, channel, None)
    }

    /// Starts the session `key` of `workspace` in a new terminal of the
    /// given size, for the window `owner`, streaming to `channel`.
    ///
    /// A session that is already running is restarted: its terminal is
    /// closed first. The workspace is told what becomes of the terminal,
    /// so the state of the session follows on its own.
    ///
    /// This blocks while a previous terminal is being closed: call it from
    /// a thread that may.
    ///
    /// # Errors
    ///
    /// Fails when the manager is not running, when the session does not
    /// exist, when there is nothing to run (its folder is gone) or when
    /// the process cannot be started. Except in the first two cases the
    /// session is left [`Exited`](SessionState::Exited) with the reason.
    pub fn start_session(
        &self,
        owner: &str,
        workspace: &Arc<Workspace>,
        key: &SessionKey,
        mode: LaunchMode,
        size: PtySize,
        channel: Channel<InvokeResponseBody>,
    ) -> Result<SessionId, String> {
        self.manager()?;
        let launch = workspace
            .begin_launch(key, mode)
            .map_err(|source| source.to_string())?;
        if let Some(previous) = launch.previous_terminal {
            self.close(SessionId::from_raw(previous));
        }
        let link = SessionLink {
            workspace: Arc::clone(workspace),
            key: key.clone(),
            generation: launch.generation,
        };
        // Takes the markers of the launch script out of the output, and
        // tells the workspace what they say.
        let filter = workspace.output_filter(key, &launch);
        let options = SessionOptions::new(launch.command).size(size);
        let id = match self.spawn(owner, &options, filter, channel, Some(link)) {
            Ok(id) => id,
            Err(reason) => {
                workspace.launch_failed(
                    key,
                    launch.generation,
                    LaunchFailure::spawn_failed(reason.trim_end_matches('.')),
                );
                return Err(reason);
            }
        };
        if !workspace.terminal_started(key, launch.generation, id.as_u64()) {
            // Closed or started again in the meantime: nobody wants this
            // terminal any more.
            self.close(id);
            return Err("the session was closed while it was starting".to_owned());
        }
        Ok(id)
    }

    fn spawn(
        &self,
        owner: &str,
        options: &SessionOptions,
        filter: Option<Box<dyn OutputFilter>>,
        channel: Channel<InvokeResponseBody>,
        session: Option<SessionLink>,
    ) -> Result<SessionId, String> {
        let manager = self.manager()?;
        let activity = session
            .as_ref()
            .and_then(|link| link.workspace.activity_handle(&link.key));
        let sink = ChannelSink {
            channel,
            owners: Arc::clone(&self.owners),
            delivery_failed: false,
            session: session.clone(),
            activity,
        };
        // Held until the owner is recorded: a session that ends at once
        // waits here before it removes its entry, instead of leaving one
        // behind by removing it too early.
        let mut owners = lock(&self.owners);
        let id = manager
            .create_filtered(options, filter, sink)
            .map_err(|source| source.to_string())?;
        owners.insert(
            id,
            Owner {
                window: owner.to_owned(),
                session,
            },
        );
        Ok(id)
    }

    /// Closes every terminal of the window `owner`, killing their
    /// processes, and returns how many there were. Used when a window goes
    /// away or reloads its page: nothing is left to display them. The
    /// sessions they ran for become dormant, and can be resumed.
    ///
    /// This blocks while processes that resist are killed. A caller that
    /// must not block does the two halves itself:
    /// [`detach_owned_by`](Self::detach_owned_by) at once, and
    /// [`close_detached`](Self::close_detached) on another thread.
    pub fn close_owned_by(&self, owner: &str) -> usize {
        let terminals = self.detach_owned_by(owner);
        self.close_detached(&terminals)
    }

    /// Takes the terminals of the window `owner` away from it, and returns
    /// them to be closed with [`close_detached`](Self::close_detached).
    /// Their sessions become dormant at once. Never blocks on a process.
    ///
    /// Doing this part immediately is what keeps a page that reloads from
    /// losing the terminals it starts right after: only those that existed
    /// when the old page went away are closed.
    #[must_use]
    pub fn detach_owned_by(&self, owner: &str) -> Vec<SessionId> {
        let mut owners = lock(&self.owners);
        let ids: Vec<SessionId> = owners
            .iter()
            .filter(|(_, terminal)| terminal.window == owner)
            .map(|(&id, _)| id)
            .collect();
        let links: Vec<SessionLink> = ids
            .iter()
            .filter_map(|id| owners.remove(id)?.session)
            .collect();
        drop(owners);
        for link in links {
            link.workspace.suspend_session(&link.key);
        }
        ids
    }

    /// Kills the processes of terminals that were detached, all at once:
    /// those that resist are waited for together. Returns how many there
    /// were.
    pub fn close_detached(&self, terminals: &[SessionId]) -> usize {
        let Ok(manager) = self.manager() else {
            return 0;
        };
        let closed = manager.close_all(terminals);
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

    /// The window a terminal belongs to.
    #[cfg(test)]
    fn owner_of(&self, id: SessionId) -> Option<String> {
        lock(&self.owners)
            .get(&id)
            .map(|terminal| terminal.window.clone())
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
    /// The session the terminal runs for, to tell the workspace its exit.
    session: Option<SessionLink>,
    /// Records that the session printed something.
    activity: Option<ActivityHandle>,
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
        if let Some(activity) = &self.activity {
            // A time and a flag: nothing that could hold the output up.
            activity.touch();
        }
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
        let mut event = SessionEvent::from(&status);
        if let Some(link) = &self.session {
            link.workspace
                .terminal_exited(&link.key, link.generation, &status);
            // What the workspace made of the exit, for the terminal to say.
            if let (
                SessionEvent::Exit { failure, .. },
                Some(SessionState::Exited {
                    failure: reason, ..
                }),
            ) = (
                &mut event,
                link.workspace.session(&link.key).map(|found| found.state),
            ) {
                *failure = reason;
            }
        }
        match event.body() {
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

    use termx_core::pty::PtyCommand;
    use termx_core::workspace::launch::{CLAUDE_MISSING_EXIT_CODE, Shell};
    use termx_core::workspace::{Launcher, NewSession, SessionKind, WorkspaceConfig};

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
    fn waiting_program() -> SessionOptions {
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
        SessionOptions::new(PtyCommand::new(program).args(args))
    }

    /// A program that prints `bye` and exits with code 3.
    fn exiting_program() -> SessionOptions {
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
        SessionOptions::new(PtyCommand::new(program).args(args))
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
    fn the_exit_is_encoded_as_a_tagged_json_message() {
        let exit = SessionEvent::Exit {
            code: Some(3),
            signal: None,
            failure: None,
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
        let login_shell = SessionOptions::default();
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

    /// A workspace that remembers nothing and whose sessions run through
    /// the shell every machine has.
    fn workspace(claude_program: &str) -> Arc<Workspace> {
        let mut config = WorkspaceConfig::in_memory();
        let shell = if cfg!(windows) {
            Shell::of_user()
        } else {
            Shell::new("/bin/sh")
        };
        config.launcher = Launcher::new(shell).claude_program(claude_program);
        Arc::new(Workspace::open(config))
    }

    fn wait_for_state(
        workspace: &Workspace,
        key: &SessionKey,
        what: &str,
        condition: impl Fn(&SessionState) -> bool,
    ) -> SessionState {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let state = workspace.session(key).expect("the session is gone").state;
            if condition(&state) {
                return state;
            }
            assert!(Instant::now() < deadline, "never {what}: {state:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// A command line that prints `ready` and waits, in the user's shell.
    fn waiting_line() -> &'static str {
        if cfg!(windows) {
            "Write-Output 'ready'; while ($null -ne [Console]::ReadLine()) {}"
        } else {
            "echo ready; while read line; do :; done"
        }
    }

    #[test]
    fn a_session_of_the_workspace_follows_its_terminal() {
        let dir = tempfile::tempdir().unwrap();
        let state = TerminalState::start();
        let workspace = workspace("claude");
        let mut new = NewSession::new(SessionKind::Background, dir.path());
        new.command = Some(waiting_line().to_owned());
        let session = workspace.create_session(new).unwrap();
        let (channel, messages) = channel();

        let id = state
            .start_session(
                "main",
                &workspace,
                &session.id,
                LaunchMode::Fresh,
                PtySize::new(80, 24),
                channel,
            )
            .unwrap();

        wait_for_output(&messages, "ready");
        let running = workspace.session(&session.id).unwrap();
        assert_eq!(running.state, SessionState::Running);
        assert_eq!(running.terminal_id, Some(id.as_u64()));
        assert_eq!(state.owner_of(id).as_deref(), Some("main"));

        // Its window goes away: the process is killed, the session stays
        // and can be resumed.
        let process_id = state.manager().unwrap().process_id(id).unwrap();
        assert_eq!(state.close_owned_by("main"), 1);
        wait_until_gone(&[process_id]);
        let after = workspace.session(&session.id).unwrap();
        assert_eq!(after.state, SessionState::Dormant);
        assert_eq!(after.terminal_id, None);
    }

    #[test]
    fn output_counts_as_activity_and_the_exit_ends_the_session() {
        let dir = tempfile::tempdir().unwrap();
        let state = TerminalState::start();
        let workspace = workspace("claude");
        let mut new = NewSession::new(SessionKind::Background, dir.path());
        new.command = Some(
            if cfg!(windows) {
                "Start-Sleep -Milliseconds 300; Write-Output 'bye'; cmd /c 'exit 3'"
            } else {
                "sleep 0.3; echo bye; exit 3"
            }
            .to_owned(),
        );
        let session = workspace.create_session(new).unwrap();
        let (channel, messages) = channel();

        state
            .start_session(
                "main",
                &workspace,
                &session.id,
                LaunchMode::Fresh,
                PtySize::new(80, 24),
                channel,
            )
            .unwrap();

        let exit = loop {
            match messages.recv_timeout(TIMEOUT).expect("no exit was sent") {
                Message::Output(_) => {}
                Message::Json(json) => break json,
            }
        };
        assert_eq!(
            exit,
            serde_json::json!({ "event": "exit", "code": 3, "signal": null })
        );
        let ended = wait_for_state(&workspace, &session.id, "exited", |state| {
            matches!(state, SessionState::Exited { .. })
        });
        assert_eq!(
            ended,
            SessionState::Exited {
                code: Some(3),
                signal: None,
                failure: None
            }
        );
        // What it printed moved its last activity, once the workspace
        // passed it on.
        workspace.flush().unwrap();
        assert!(workspace.session(&session.id).unwrap().last_activity_at > session.created_at);
    }

    #[test]
    fn a_missing_claude_is_told_to_the_terminal_and_to_the_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let state = TerminalState::start();
        let workspace = workspace("termx-test-claude-that-is-not-installed");
        let session = workspace
            .create_session(NewSession::new(SessionKind::Claude, dir.path()))
            .unwrap();
        let (channel, messages) = channel();

        state
            .start_session(
                "main",
                &workspace,
                &session.id,
                LaunchMode::Fresh,
                PtySize::new(80, 24),
                channel,
            )
            .unwrap();

        let exit = loop {
            match messages.recv_timeout(TIMEOUT).expect("no exit was sent") {
                Message::Output(_) => {}
                Message::Json(json) => break json,
            }
        };
        assert_eq!(exit["event"], "exit");
        assert_eq!(exit["code"], CLAUDE_MISSING_EXIT_CODE);
        assert_eq!(exit["failure"]["kind"], "claudeNotFound");
        assert!(
            exit["failure"]["message"]
                .as_str()
                .unwrap()
                .contains("Claude Code is not installed"),
            "{exit}"
        );
        let SessionState::Exited { failure, .. } = workspace.session(&session.id).unwrap().state
        else {
            panic!("the session did not end");
        };
        assert!(matches!(
            failure,
            Some(LaunchFailure::ClaudeNotFound { .. })
        ));
    }

    #[test]
    fn restarting_a_session_replaces_its_terminal() {
        let dir = tempfile::tempdir().unwrap();
        let state = TerminalState::start();
        let workspace = workspace("claude");
        let mut new = NewSession::new(SessionKind::Background, dir.path());
        new.command = Some(waiting_line().to_owned());
        let session = workspace.create_session(new).unwrap();
        let start = |mode| {
            let (channel, messages) = channel();
            let id = state
                .start_session(
                    "main",
                    &workspace,
                    &session.id,
                    mode,
                    PtySize::new(80, 24),
                    channel,
                )
                .unwrap();
            wait_for_output(&messages, "ready");
            (id, messages)
        };
        let (first, _first_messages) = start(LaunchMode::Fresh);
        let first_pid = state.manager().unwrap().process_id(first).unwrap();

        let (second, _second_messages) = start(LaunchMode::Resume);

        assert_ne!(first, second);
        wait_until_gone(&[first_pid]);
        let running = workspace.session(&session.id).unwrap();
        assert_eq!(running.state, SessionState::Running);
        assert_eq!(running.terminal_id, Some(second.as_u64()));
        assert_eq!(state.manager().unwrap().ids(), [second]);
        state.shutdown();
    }

    #[test]
    fn a_session_that_cannot_start_says_why_and_leaves_nothing_running() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("short-lived");
        std::fs::create_dir(&folder).unwrap();
        let state = TerminalState::start();
        let workspace = workspace("claude");
        let session = workspace
            .create_session(NewSession::new(SessionKind::Shell, &folder))
            .unwrap();
        std::fs::remove_dir(&folder).unwrap();
        let (unused, _messages) = channel();

        let error = state
            .start_session(
                "main",
                &workspace,
                &session.id,
                LaunchMode::Resume,
                PtySize::new(80, 24),
                unused,
            )
            .unwrap_err();

        assert!(error.contains("does not exist any more"), "{error}");
        assert!(state.manager().unwrap().is_empty());
        assert_eq!(
            workspace.session(&session.id).unwrap().state,
            SessionState::Exited {
                code: None,
                signal: None,
                failure: Some(LaunchFailure::folder_missing()),
            }
        );

        // An unknown session is an error too, and changes nothing.
        let (unused, _messages) = channel();
        let ghost = SessionKey::from_raw("s-ghost").unwrap();
        let error = state
            .start_session(
                "main",
                &workspace,
                &ghost,
                LaunchMode::Fresh,
                PtySize::new(80, 24),
                unused,
            )
            .unwrap_err();
        assert!(error.contains("no session"), "{error}");
    }

    #[test]
    fn the_windows_build_number_is_only_reported_on_windows() {
        assert_eq!(windows_build_number().is_some(), cfg!(windows));
    }
}
