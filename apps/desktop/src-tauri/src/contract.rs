//! Guards the shapes the frontend and the Rust side exchange.
//!
//! `apps/desktop/src/lib/ipc.contract.json` holds one example of everything
//! that crosses the boundary: sessions in every state, a snapshot, deltas,
//! the answers of commands and the requests the frontend sends. The test
//! below builds those examples from the real types and fails when the file
//! differs; `ipc.contract.test.ts` checks the TypeScript types of
//! `lib/ipc.ts` against the same file. A change on one side that is not
//! made on the other fails one of the two.
//!
//! After changing a type on purpose, write the file again and review the
//! difference:
//!
//! ```sh
//! TERMX_UPDATE_CONTRACT=1 cargo test -p termx-desktop contract
//! ```

use std::path::PathBuf;

use serde_json::{Value, json};
use termx_core::pty::ExitStatus;
use termx_core::workspace::launch::CLAUDE_MISSING_EXIT_CODE;
use termx_core::workspace::{
    AgentState, AgentStatus, Delta, Direction, Edge, GitInfo, LaunchFailure, LaunchMode, Layout,
    PersistenceMode, Placement, Project, ProjectKind, Session, SessionKey, SessionKind,
    SessionState, Snapshot,
};

use termx_core::claude::{
    Change, Consent, IntegrationError, IntegrationErrorKind, IntegrationStatus, PluginStatus,
};

use crate::commands::claude::ClaudePreview;
use crate::commands::error::{CommandError, ErrorKind};
use crate::commands::session::{CreateSessionRequest, StartSessionRequest, StartedSession};
use crate::notify::NotificationClicked;
use crate::terminal::SessionEvent;

/// Where the contract is kept, next to the types it guards.
const CONTRACT_FILE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../src/lib/ipc.contract.json");

fn key(name: &str) -> SessionKey {
    SessionKey::from_raw(name).unwrap()
}

fn git_project() -> Project {
    Project {
        key: "/work/app".to_owned(),
        name: "app".to_owned(),
        root: PathBuf::from("/work/app"),
        kind: ProjectKind::Git,
    }
}

fn folder_project() -> Project {
    Project {
        key: "/work/notes".to_owned(),
        name: "notes".to_owned(),
        root: PathBuf::from("/work/notes"),
        kind: ProjectKind::Folder,
    }
}

/// The integration in each state the frontend shows.
fn claude_integration() -> Vec<IntegrationStatus> {
    vec![
        IntegrationStatus {
            claude_installed: true,
            plugin: PluginStatus::NotInstalled,
            version: None,
            consent: Consent::Unasked,
            error: None,
        },
        IntegrationStatus {
            claude_installed: true,
            plugin: PluginStatus::Installed,
            version: Some("0.1.0+plugin.1".to_owned()),
            consent: Consent::Accepted,
            error: None,
        },
        IntegrationStatus {
            claude_installed: true,
            plugin: PluginStatus::Outdated,
            version: Some("0.0.9+plugin.1".to_owned()),
            consent: Consent::Accepted,
            error: None,
        },
        IntegrationStatus {
            claude_installed: false,
            plugin: PluginStatus::Error,
            version: None,
            consent: Consent::Declined,
            error: Some(IntegrationError {
                kind: IntegrationErrorKind::InvalidSettings,
                message: "~/.claude/settings.json cannot be changed safely".to_owned(),
            }),
        },
    ]
}

/// A Claude Code session that runs in a linked worktree.
fn running_claude() -> Session {
    Session {
        id: key("s-claude"),
        kind: SessionKind::Claude,
        cwd: PathBuf::from("/work/app-review"),
        live_cwd: Some(PathBuf::from("/work/app-review/src")),
        command: None,
        title: "Login work".to_owned(),
        name: Some("Login work".to_owned()),
        auto_title: Some("Fix the login".to_owned()),
        project: git_project(),
        git: Some(GitInfo {
            branch: Some("review".to_owned()),
            commit: None,
            worktree: Some("app-review".to_owned()),
        }),
        state: SessionState::Running,
        terminal_id: Some(7),
        created_at: 1_795_716_000_000,
        last_activity_at: 1_795_716_005_340,
        // What the hooks of Claude Code report.
        agent: Some(AgentState::new(
            AgentStatus::WaitingForPermission,
            Some(1_795_716_005_000),
            1_795_716_005_000,
        )),
        claude_session_id: Some("0b9d6f7e-3c1a-4f3e-9a55-2f6f1f0f8a10".to_owned()),
        warning: None,
    }
}

/// A shell restored from the previous run, outside any repository.
fn dormant_shell() -> Session {
    Session {
        id: key("s-shell"),
        kind: SessionKind::Shell,
        cwd: PathBuf::from("/work/notes"),
        live_cwd: None,
        command: None,
        title: "Shell".to_owned(),
        name: None,
        auto_title: None,
        project: folder_project(),
        git: None,
        state: SessionState::Dormant,
        terminal_id: None,
        created_at: 1_795_700_000_000,
        last_activity_at: 1_795_700_100_000,
        agent: None,
        claude_session_id: None,
        warning: None,
    }
}

/// A background command that is about to start, on a detached commit.
fn starting_background() -> Session {
    Session {
        id: key("s-tunnel"),
        kind: SessionKind::Background,
        command: Some("ngrok http 3000".to_owned()),
        title: "ngrok http 3000".to_owned(),
        git: Some(GitInfo {
            branch: None,
            commit: Some("0123456".to_owned()),
            worktree: None,
        }),
        state: SessionState::Starting,
        // Its shell profile has not let the command start yet.
        warning: Some(LaunchFailure::launch_unconfirmed()),
        cwd: PathBuf::from("/work/app"),
        project: git_project(),
        ..dormant_shell()
    }
}

/// A Claude Code session that ended because `claude` is not installed.
fn failed_claude() -> Session {
    Session {
        id: key("s-failed"),
        kind: SessionKind::Claude,
        title: "Claude Code".to_owned(),
        state: SessionState::Exited {
            code: Some(CLAUDE_MISSING_EXIT_CODE),
            signal: None,
            failure: Some(LaunchFailure::claude_not_found("claude")),
        },
        ..dormant_shell()
    }
}

/// A shell that was ended by a signal.
fn killed_shell() -> Session {
    Session {
        id: key("s-killed"),
        state: SessionState::Exited {
            code: None,
            signal: Some("Hangup".to_owned()),
            failure: None,
        },
        ..dormant_shell()
    }
}

/// Two panes side by side, the second one stacked on an empty third.
fn layout() -> Layout {
    let mut layout = Layout::default();
    let first = layout.focused();
    layout.show(first, Some(key("s-claude"))).unwrap();
    let second = layout
        .split(
            first,
            Direction::Row,
            Placement::After,
            Some(key("s-shell")),
        )
        .unwrap();
    layout
        .split(second, Direction::Column, Placement::After, None)
        .unwrap();
    layout.focus(second).unwrap();
    layout
}

fn snapshot() -> Snapshot {
    Snapshot {
        revision: 42,
        sessions: vec![
            running_claude(),
            dormant_shell(),
            starting_background(),
            failed_claude(),
            killed_shell(),
        ],
        projects: vec![git_project(), folder_project()],
        layout: layout(),
        recent_folders: vec![PathBuf::from("/work/app"), PathBuf::from("/work/notes")],
        persistence: PersistenceMode::Primary,
    }
}

/// Everything that crosses the boundary, as the frontend receives or
/// sends it.
fn contract() -> Value {
    let full_delta = Delta {
        revision: 43,
        sessions: vec![running_claude()],
        removed: vec![key("s-killed")],
        session_order: Some(vec![key("s-claude"), key("s-shell")]),
        projects: Some(vec![git_project()]),
        layout: Some(layout()),
        recent_folders: Some(vec![PathBuf::from("/work/app")]),
        persistence: Some(PersistenceMode::Failing {
            reason: "No space left on device (os error 28)".to_owned(),
            since: 1_795_716_005_000,
        }),
    };
    let empty_delta = Delta {
        revision: 44,
        sessions: vec![],
        removed: vec![],
        session_order: None,
        projects: None,
        layout: None,
        recent_folders: None,
        persistence: None,
    };
    let mut missing_claude = SessionEvent::from(&ExitStatus::from_code(CLAUDE_MISSING_EXIT_CODE));
    let SessionEvent::Exit { failure, .. } = &mut missing_claude;
    *failure = Some(LaunchFailure::claude_not_found("claude"));

    let command_errors: Vec<CommandError> = [
        ErrorKind::ClaudeNotFound,
        ErrorKind::FolderMissing,
        ErrorKind::InvalidCommand,
        ErrorKind::SpawnFailed,
        ErrorKind::LaunchUnconfirmed,
        ErrorKind::StorageFull,
        ErrorKind::NotFound,
        ErrorKind::InvalidArgument,
        ErrorKind::SessionRunning,
        ErrorKind::InvalidSettings,
        ErrorKind::SettingsConflict,
        ErrorKind::SenderMissing,
        ErrorKind::Internal,
    ]
    .into_iter()
    .map(|kind| CommandError {
        command: "session_start",
        kind,
        message: "what happened, for the user".to_owned(),
    })
    .collect();

    json!({
        "comment": "Examples of what the frontend and the Rust side exchange. Written by \
                    `TERMX_UPDATE_CONTRACT=1 cargo test -p termx-desktop contract`; do not \
                    edit by hand. See src-tauri/src/contract.rs.",
        "snapshot": snapshot(),
        "deltas": [full_delta, empty_delta],
        "persistence": [
            PersistenceMode::Primary,
            PersistenceMode::Secondary,
            PersistenceMode::Disabled { reason: "the state file cannot be read".to_owned() },
            PersistenceMode::Memory,
            PersistenceMode::Failing {
                reason: "No space left on device (os error 28)".to_owned(),
                since: 1_795_716_005_000,
            },
        ],
        "launchFailures": [
            LaunchFailure::claude_not_found("claude"),
            LaunchFailure::folder_missing(),
            LaunchFailure::InvalidCommand {
                message: "This background session has no command to run.".to_owned(),
            },
            LaunchFailure::spawn_failed("too many open files"),
            LaunchFailure::launch_unconfirmed(),
        ],
        "commandErrors": command_errors,
        "claudeIntegration": claude_integration(),
        "claudePreview": ClaudePreview {
            changes: vec![
                Change {
                    path: "~/Library/Application Support/termx/claude-plugin".to_owned(),
                    description: "Write the termx plugin for Claude Code.".to_owned(),
                },
                Change {
                    path: "~/.claude/settings.json".to_owned(),
                    description: "Set \"termx@termx\" to true in \"enabledPlugins\".".to_owned(),
                },
            ],
        },
        "notificationClicked": NotificationClicked {
            session_id: "s-claude".to_owned(),
        },
        "startedSession": StartedSession {
            terminal: 7,
            ack_batch_bytes: 8192,
            windows_build_number: Some(22631),
        },
        "exitEvents": [
            SessionEvent::from(&ExitStatus::from_code(3)),
            missing_claude,
        ],
        "enums": {
            "sessionKind": [SessionKind::Claude, SessionKind::Shell, SessionKind::Background],
            "sessionStatus": ["dormant", "starting", "running", "exited"],
            "projectKind": [ProjectKind::Git, ProjectKind::Folder],
            "launchMode": [LaunchMode::Fresh, LaunchMode::Resume],
            "direction": [Direction::Row, Direction::Column],
            "placement": [Placement::Before, Placement::After],
            "edge": [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom, Edge::Center],
            "pluginStatus": [
                PluginStatus::NotInstalled,
                PluginStatus::Installed,
                PluginStatus::Outdated,
                PluginStatus::Error,
            ],
            "consent": [Consent::Unasked, Consent::Accepted, Consent::Declined],
            "agentStatus": [
                AgentStatus::Working,
                AgentStatus::WaitingForPermission,
                AgentStatus::WaitingForInput,
                AgentStatus::Done,
                AgentStatus::Unknown,
            ],
        },
        // What the frontend sends. The test checks that the Rust side reads
        // each of them as intended.
        "requests": {
            "createSession": [
                { "kind": "shell" },
                {
                    "kind": "background",
                    "cwd": "/work/app",
                    "command": "pnpm dev",
                    "name": "dev server",
                    "show": false,
                },
            ],
            "startSession": [
                { "cols": 120, "rows": 40 },
                { "mode": "resume", "cols": 80, "rows": 24 },
            ],
        },
    })
}

#[test]
fn the_contract_file_matches_the_types_of_the_rust_side() {
    let contract = contract();
    if std::env::var_os("TERMX_UPDATE_CONTRACT").is_some() {
        let mut text = serde_json::to_string_pretty(&contract).unwrap();
        text.push('\n');
        std::fs::write(CONTRACT_FILE, text).unwrap();
    }

    let text = std::fs::read_to_string(CONTRACT_FILE)
        .expect("the contract file is missing: see src-tauri/src/contract.rs");
    let on_disk: Value = serde_json::from_str(&text).expect("the contract file is not JSON");

    assert_eq!(
        on_disk, contract,
        "the shapes sent to the frontend changed: update lib/ipc.ts, then write the contract \
         again with TERMX_UPDATE_CONTRACT=1 (see src-tauri/src/contract.rs)"
    );
}

#[test]
fn the_statuses_listed_are_those_of_the_states() {
    let statuses: Vec<Value> = [
        SessionState::Dormant,
        SessionState::Starting,
        SessionState::Running,
        SessionState::Exited {
            code: None,
            signal: None,
            failure: None,
        },
    ]
    .iter()
    .map(|state| serde_json::to_value(state).unwrap()["status"].clone())
    .collect();

    assert_eq!(contract()["enums"]["sessionStatus"], Value::from(statuses));
}

#[test]
fn the_requests_of_the_frontend_are_read_as_intended() {
    let contract = contract();
    let create: Vec<CreateSessionRequest> =
        serde_json::from_value(contract["requests"]["createSession"].clone()).unwrap();
    let start: Vec<StartSessionRequest> =
        serde_json::from_value(contract["requests"]["startSession"].clone()).unwrap();

    assert_eq!(
        create,
        [
            CreateSessionRequest {
                kind: SessionKind::Shell,
                cwd: None,
                command: None,
                name: None,
                show: None,
            },
            CreateSessionRequest {
                kind: SessionKind::Background,
                cwd: Some(PathBuf::from("/work/app")),
                command: Some("pnpm dev".to_owned()),
                name: Some("dev server".to_owned()),
                show: Some(false),
            },
        ]
    );
    assert_eq!(
        start,
        [
            StartSessionRequest {
                mode: LaunchMode::Fresh,
                cols: 120,
                rows: 40
            },
            StartSessionRequest {
                mode: LaunchMode::Resume,
                cols: 80,
                rows: 24
            },
        ]
    );
}
