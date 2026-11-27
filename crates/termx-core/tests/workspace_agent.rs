//! What the hooks of Claude Code report, as the workspace keeps it: the
//! agent state of a session, its conversation, and what survives a
//! restart.

#![allow(clippy::unwrap_used)]

use termx_core::pty::ExitStatus;
use termx_core::workspace::{
    AgentChannel, AgentStatus, HookOutcome, LaunchMode, NewSession, Session, SessionKey,
    SessionKind, Workspace, WorkspaceConfig, WorkspaceError,
};
use termx_protocol::{ENV_SESSION_ID, ENV_SOCKET, ENV_TICKET, HookEvent};

const CONVERSATION: &str = "0b9d6f7e-3c1a-4f3e-9a55-2f6f1f0f8a10";

fn event(name: &str, at_ms: u64) -> HookEvent {
    HookEvent {
        name: name.to_owned(),
        claude_session_id: Some(CONVERSATION.to_owned()),
        notification_type: None,
        tool: None,
        source: None,
        reason: None,
        agent: None,
        pid: None,
        at: at_ms * 1000,
    }
}

fn start_event(source: &str, id: &str, at_ms: u64) -> HookEvent {
    HookEvent {
        source: Some(source.to_owned()),
        claude_session_id: Some(id.to_owned()),
        ..event("SessionStart", at_ms)
    }
}

/// A running session of `kind`, started as the application starts one.
fn running(workspace: &Workspace, kind: SessionKind, dir: &std::path::Path) -> (Session, u64) {
    let session = workspace
        .create_session(NewSession::new(kind, dir))
        .unwrap();
    let launch = workspace
        .begin_launch(&session.id, LaunchMode::Fresh)
        .unwrap();
    assert!(workspace.terminal_started(&session.id, launch.generation, 1));
    (session, launch.generation)
}

fn agent_status(workspace: &Workspace, key: &SessionKey) -> Option<AgentStatus> {
    workspace
        .session(key)
        .unwrap()
        .agent
        .map(|agent| agent.status)
}

/// A time that hooks of a test can use: just now, so that it is not
/// taken for a report from the future.
fn now() -> u64 {
    termx_core::workspace::now_ms()
}

#[test]
fn a_turn_shows_in_the_session_and_its_changes_are_published() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = Workspace::open(WorkspaceConfig::in_memory());
    let (session, _) = running(&workspace, SessionKind::Claude, dir.path());
    let subscription = workspace.subscribe();
    let t = now();

    let mut seen = Vec::new();
    for event in [
        start_event("startup", CONVERSATION, t),
        event("UserPromptSubmit", t + 1),
        event("PermissionRequest", t + 2),
        event("PostToolUse", t + 3),
        event("Stop", t + 4),
    ] {
        assert_eq!(
            workspace.report_hook(&session.id, &event).unwrap(),
            HookOutcome::Applied,
            "{}",
            event.name
        );
        seen.push(agent_status(&workspace, &session.id).unwrap());
    }

    assert_eq!(
        seen,
        [
            AgentStatus::Unknown,
            AgentStatus::Working,
            AgentStatus::WaitingForPermission,
            AgentStatus::Working,
            AgentStatus::Done,
        ]
    );
    let agent = workspace.session(&session.id).unwrap().agent.unwrap();
    assert!(agent.waiting_since.is_some());
    assert!(!agent.stale);
    // Subscribers heard of it.
    let mut published = false;
    while let Some(changes) = subscription.try_recv() {
        published |= workspace
            .delta(&changes)
            .sessions
            .iter()
            .any(|changed| changed.id == session.id && changed.agent.is_some());
    }
    assert!(published);
    // A repeat changes nothing.
    assert_eq!(
        workspace
            .report_hook(&session.id, &event("Stop", t + 4))
            .unwrap(),
        HookOutcome::Ignored
    );
}

#[test]
fn the_conversation_is_saved_and_resumed_but_the_agent_state_is_not() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    let key = {
        let workspace = Workspace::open(WorkspaceConfig::new(&state));
        let (session, _) = running(&workspace, SessionKind::Claude, dir.path());
        let t = now();
        workspace
            .report_hook(&session.id, &start_event("startup", CONVERSATION, t))
            .unwrap();
        workspace
            .report_hook(&session.id, &event("UserPromptSubmit", t + 1))
            .unwrap();
        assert_eq!(
            workspace
                .session(&session.id)
                .unwrap()
                .claude_session_id
                .as_deref(),
            Some(CONVERSATION)
        );
        workspace.shutdown();
        session.id
    };

    let workspace = Workspace::open(WorkspaceConfig::new(&state));
    let restored = workspace.session(&key).unwrap();
    assert_eq!(restored.claude_session_id.as_deref(), Some(CONVERSATION));
    assert_eq!(restored.agent, None);
    // Resuming it runs `claude --resume <conversation>`.
    let launch = workspace.begin_launch(&key, LaunchMode::Resume).unwrap();
    let described = format!("{:?}", launch.command);
    assert!(described.contains(CONVERSATION), "{described}");
    // What was saved holds nothing of the agent state.
    let saved = std::fs::read_to_string(state.join("state.json")).unwrap();
    for word in ["agent", "working", "waitingSince"] {
        assert!(!saved.contains(word), "{word} was saved: {saved}");
    }
}

#[test]
fn clear_and_resume_change_the_conversation() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = Workspace::open(WorkspaceConfig::in_memory());
    let (session, _) = running(&workspace, SessionKind::Claude, dir.path());
    let t = now();
    let conversation =
        |workspace: &Workspace| workspace.session(&session.id).unwrap().claude_session_id;

    workspace
        .report_hook(&session.id, &start_event("startup", CONVERSATION, t))
        .unwrap();
    workspace
        .report_hook(&session.id, &event("SessionEnd", t + 1))
        .unwrap();
    workspace
        .report_hook(&session.id, &start_event("clear", "after-clear", t + 2))
        .unwrap();
    assert_eq!(conversation(&workspace).as_deref(), Some("after-clear"));

    workspace
        .report_hook(&session.id, &start_event("resume", "an-older-one", t + 3))
        .unwrap();
    assert_eq!(conversation(&workspace).as_deref(), Some("an-older-one"));
}

#[test]
fn the_agent_goes_away_with_the_terminal() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = Workspace::open(WorkspaceConfig::in_memory());
    let (session, generation) = running(&workspace, SessionKind::Claude, dir.path());
    let t = now();
    workspace
        .report_hook(&session.id, &event("UserPromptSubmit", t))
        .unwrap();
    assert_eq!(
        agent_status(&workspace, &session.id),
        Some(AgentStatus::Working)
    );

    // Claude Code exits without a final hook: so does its terminal.
    workspace.terminal_exited(&session.id, generation, &ExitStatus::from_code(0));

    assert_eq!(agent_status(&workspace, &session.id), None);
    // A late report of the process that is gone changes nothing.
    assert_eq!(
        workspace
            .report_hook(&session.id, &event("PreToolUse", t + 1))
            .unwrap(),
        HookOutcome::Ignored
    );
    assert_eq!(agent_status(&workspace, &session.id), None);
}

#[test]
fn stopping_or_restarting_forgets_the_agent() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = Workspace::open(WorkspaceConfig::in_memory());
    let (session, _) = running(&workspace, SessionKind::Shell, dir.path());
    let t = now();
    workspace
        .report_hook(&session.id, &event("Stop", t))
        .unwrap();
    assert_eq!(
        agent_status(&workspace, &session.id),
        Some(AgentStatus::Done)
    );

    workspace
        .begin_launch(&session.id, LaunchMode::Fresh)
        .unwrap();
    assert_eq!(agent_status(&workspace, &session.id), None);

    let (other, _) = running(&workspace, SessionKind::Shell, dir.path());
    workspace.report_hook(&other.id, &event("Stop", t)).unwrap();
    workspace.stop_session(&other.id).unwrap();
    assert_eq!(agent_status(&workspace, &other.id), None);
}

#[test]
fn a_report_for_a_session_that_does_not_exist_is_an_error() {
    let workspace = Workspace::open(WorkspaceConfig::in_memory());
    let key = SessionKey::from_raw("s-nobody").unwrap();

    let result = workspace.report_hook(&key, &event("Stop", now()));

    assert!(
        matches!(result, Err(WorkspaceError::UnknownSession(_))),
        "{result:?}"
    );
}

#[test]
fn sessions_are_told_where_to_report_once_there_is_a_channel() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = Workspace::open(WorkspaceConfig::in_memory());
    let session = workspace
        .create_session(NewSession::new(SessionKind::Shell, dir.path()))
        .unwrap();

    let launch = workspace
        .begin_launch(&session.id, LaunchMode::Fresh)
        .unwrap();
    assert_eq!(launch.command.env_value(ENV_SESSION_ID), None);

    workspace.set_agent_channel(Some(AgentChannel {
        socket: "/tmp/termx-test/1.sock".to_owned(),
        ticket: "0123456789abcdef0123456789abcdef".to_owned(),
    }));
    for kind in [SessionKind::Shell, SessionKind::Claude] {
        let session = workspace
            .create_session(NewSession::new(kind, dir.path()))
            .unwrap();
        let launch = workspace
            .begin_launch(&session.id, LaunchMode::Fresh)
            .unwrap();
        let value = |name: &str| {
            launch
                .command
                .env_value(name)
                .map(|value| value.to_string_lossy().into_owned())
        };
        assert_eq!(value(ENV_SESSION_ID).as_deref(), Some(session.id.as_str()));
        assert_eq!(value(ENV_SOCKET).as_deref(), Some("/tmp/termx-test/1.sock"));
        assert_eq!(
            value(ENV_TICKET).as_deref(),
            Some("0123456789abcdef0123456789abcdef")
        );
    }
}
