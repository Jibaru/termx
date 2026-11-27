//! Calls the commands the way the webview does, through Tauri's IPC, on a
//! mock runtime: no window is opened.
//!
//! What this guards is the wiring that neither side can check alone: the
//! names of the commands and of their arguments as `lib/ipc.ts` sends them,
//! how identifiers and requests are decoded, and what comes back.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{INVOKE_KEY, MockRuntime, mock_builder, mock_context, noop_assets};
use tauri::webview::InvokeRequest;
use tauri::{App, WebviewWindow, WebviewWindowBuilder};
use termx_core::workspace::launch::Shell;
use termx_core::workspace::{Launcher, SessionKey, SessionState, Workspace, WorkspaceConfig};

use crate::logging::DiagnosticsState;
use crate::terminal::TerminalState;
use crate::workspace::WorkspaceState;

const TIMEOUT: Duration = Duration::from_secs(60);

/// The application without a window system, with the real commands.
struct Harness {
    // Kept alive: dropping the application shuts its state down.
    _app: App<MockRuntime>,
    webview: WebviewWindow<MockRuntime>,
    workspace: Arc<Workspace>,
}

impl Harness {
    fn new(home: &Path) -> Self {
        let mut config = WorkspaceConfig::in_memory();
        let shell = if cfg!(windows) {
            Shell::of_user()
        } else {
            Shell::new("/bin/sh")
        };
        config.launcher = Launcher::new(shell);
        let state = WorkspaceState::with_workspace(Workspace::open(config), Some(home.to_owned()));
        let workspace = Arc::clone(state.workspace());

        let app = mock_builder()
            .manage(DiagnosticsState::new())
            .manage(TerminalState::start())
            .manage(crate::quit::QuitState::default())
            .manage(state)
            .manage(crate::claude::ClaudeState::with_integration(
                termx_core::claude::Integration::new(claude_paths(home)),
            ))
            .invoke_handler(command_handlers!())
            .build(mock_context(noop_assets()))
            .expect("the mock application could not be built");
        let webview = WebviewWindowBuilder::new(&app, "main", tauri::WebviewUrl::default())
            .build()
            .expect("the mock window could not be created");
        Self {
            _app: app,
            webview,
            workspace,
        }
    }

    /// Invokes a command with the arguments `lib/ipc.ts` sends, and returns
    /// what its promise would resolve or reject with.
    fn invoke(&self, command: &str, arguments: Value) -> Result<Value, Value> {
        let request = InvokeRequest {
            cmd: command.to_owned(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: if cfg!(any(windows, target_os = "android")) {
                "http://tauri.localhost"
            } else {
                "tauri://localhost"
            }
            .parse()
            .unwrap(),
            body: InvokeBody::Json(arguments),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.to_owned(),
        };
        tauri::test::get_ipc_response(&self.webview, request)
            .map(|body| body.deserialize::<Value>().unwrap())
    }

    fn ok(&self, command: &str, arguments: Value) -> Value {
        self.invoke(command, arguments)
            .unwrap_or_else(|error| panic!("{command} failed: {error}"))
    }

    fn state_of(&self, id: &str) -> SessionState {
        self.workspace
            .session(&SessionKey::from_raw(id).unwrap())
            .expect("the session is gone")
            .state
    }

    fn wait_for_state(&self, id: &str, what: &str, condition: impl Fn(&SessionState) -> bool) {
        let deadline = Instant::now() + TIMEOUT;
        while !condition(&self.state_of(id)) {
            assert!(
                Instant::now() < deadline,
                "never {what}: {:?}",
                self.state_of(id)
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// A channel as the webview passes it: Tauri recognises the text.
/// Where the integration of the harness keeps everything: inside `home`,
/// never in the configuration of whoever runs the tests.
fn claude_paths(home: &Path) -> termx_core::claude::Paths {
    let sender = home.join("app").join(termx_core::claude::SENDER);
    let _ = std::fs::create_dir_all(sender.parent().unwrap());
    let _ = std::fs::write(&sender, b"sender");
    termx_core::claude::Paths {
        config_dir: home.join("claude-config"),
        plugin_dir: home.join("state").join("claude-plugin"),
        consent_file: Some(home.join("state").join("claude-integration.json")),
        sender,
        home: Some(home.to_owned()),
        search_path: None,
        managed_settings: Vec::new(),
    }
}

fn channel(id: u32) -> Value {
    json!(format!("__CHANNEL__:{id}"))
}

/// A command line that waits for input, for the user's shell.
fn waiting_line() -> &'static str {
    if cfg!(windows) {
        "while ($null -ne [Console]::ReadLine()) {}"
    } else {
        "while read line; do :; done"
    }
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[test]
fn a_window_subscribes_and_reads_the_workspace() {
    let home = tempfile::tempdir().unwrap();
    let app = Harness::new(home.path());

    let snapshot = app.ok("workspace_subscribe", json!({ "onEvent": channel(1) }));

    assert_eq!(snapshot["sessions"], json!([]));
    assert_eq!(snapshot["projects"], json!([]));
    assert_eq!(snapshot["recentFolders"], json!([]));
    assert_eq!(snapshot["persistence"], json!({ "mode": "memory" }));
    assert_eq!(
        snapshot["layout"],
        json!({ "root": { "type": "pane", "id": 1, "session": null }, "focused": 1, "nextId": 2 })
    );
    assert_eq!(app.ok("workspace_snapshot", json!({})), snapshot);
    assert_eq!(app.ok("workspace_refresh_git", json!({})), Value::Null);
}

#[test]
fn sessions_are_created_named_ordered_and_closed_by_their_id() {
    let home = tempfile::tempdir().unwrap();
    let project = home.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let app = Harness::new(home.path());

    // Without a folder, a session is created in the home directory.
    let shell = app.ok("session_create", json!({ "request": { "kind": "shell" } }));
    assert_eq!(shell["kind"], "shell");
    assert_eq!(shell["cwd"], text(home.path()));
    assert_eq!(shell["state"], json!({ "status": "starting" }));
    let shell_id = shell["id"].as_str().unwrap().to_owned();

    let tunnel = app.ok(
        "session_create",
        json!({ "request": {
            "kind": "background",
            "cwd": text(&project),
            "command": "ngrok http 3000",
            "name": "tunnel",
            "show": false,
        } }),
    );
    assert_eq!(tunnel["command"], "ngrok http 3000");
    assert_eq!(tunnel["title"], "tunnel");
    let tunnel_id = tunnel["id"].as_str().unwrap().to_owned();

    let sibling = app.ok("session_create_sibling", json!({ "id": tunnel_id }));
    assert_eq!(sibling["kind"], "claude");
    assert_eq!(sibling["cwd"], text(&project));
    let sibling_id = sibling["id"].as_str().unwrap().to_owned();

    app.ok(
        "session_rename",
        json!({ "id": shell_id, "name": "scratch" }),
    );
    app.ok(
        "session_report_title",
        json!({ "id": sibling_id, "title": "✳ Fix the login" }),
    );
    app.ok(
        "session_report_cwd",
        json!({ "id": shell_id, "cwd": "file://host/nowhere" }),
    );
    app.ok(
        "session_reorder",
        json!({ "id": sibling_id, "before": shell_id }),
    );
    app.ok("session_reorder", json!({ "id": shell_id, "before": null }));
    app.ok(
        "project_reorder",
        json!({ "key": tunnel["project"]["key"], "before": shell["project"]["key"] }),
    );

    let snapshot = app.ok("workspace_snapshot", json!({}));
    let titles: Vec<(&str, &str)> = snapshot["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|session| {
            (
                session["id"].as_str().unwrap(),
                session["title"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        titles,
        [
            (sibling_id.as_str(), "Fix the login"),
            (tunnel_id.as_str(), "tunnel"),
            (shell_id.as_str(), "scratch"),
        ]
    );
    assert_eq!(snapshot["projects"][0]["key"], tunnel["project"]["key"]);
    assert_eq!(
        snapshot["recentFolders"],
        json!([text(&project), text(home.path())])
    );

    assert_eq!(
        app.ok("recent_folder_forget", json!({ "folder": text(&project) })),
        json!(true)
    );
    app.ok("session_close", json!({ "id": tunnel_id }));
    // Closing it again, or telling its title late, is not a failure.
    app.ok("session_close", json!({ "id": tunnel_id }));
    app.ok(
        "session_report_title",
        json!({ "id": tunnel_id, "title": "late" }),
    );
    let left = app.ok("workspace_snapshot", json!({}));
    assert_eq!(left["sessions"].as_array().unwrap().len(), 2);
    assert_eq!(left["recentFolders"], json!([text(home.path())]));
}

#[test]
fn a_session_is_started_driven_and_closed() {
    let home = tempfile::tempdir().unwrap();
    let app = Harness::new(home.path());
    let session = app.ok(
        "session_create",
        json!({ "request": { "kind": "background", "command": waiting_line() } }),
    );
    let id = session["id"].as_str().unwrap().to_owned();

    let started = app.ok(
        "session_start",
        json!({
            "id": id,
            "request": { "mode": "fresh", "cols": 100, "rows": 30 },
            "onEvent": channel(2),
        }),
    );

    let terminal = started["terminal"].as_u64().expect("no terminal number");
    assert!(started["ackBatchBytes"].as_u64().unwrap() > 0);
    assert_eq!(started["windowsBuildNumber"].is_null(), !cfg!(windows));
    assert_eq!(app.state_of(&id), SessionState::Running);
    let running = app.ok("workspace_snapshot", json!({}));
    assert_eq!(running["sessions"][0]["terminalId"], terminal);
    assert_eq!(
        running["sessions"][0]["state"],
        json!({ "status": "running" })
    );

    // The terminal is driven by its number.
    app.ok(
        "session_write",
        json!({ "id": terminal, "data": "hello\r", "binary": false }),
    );
    app.ok(
        "session_resize",
        json!({ "id": terminal, "cols": 132, "rows": 43 }),
    );
    app.ok("session_ack", json!({ "id": terminal, "bytes": 1024 }));

    // Starting it again is a restart, in another terminal.
    let restarted = app.ok(
        "session_start",
        json!({ "id": id, "request": { "cols": 80, "rows": 24 }, "onEvent": channel(3) }),
    );
    assert_ne!(restarted["terminal"], started["terminal"]);
    assert_eq!(app.state_of(&id), SessionState::Running);

    // A running session is not removed by mistake; stopped, it is.
    let refused = app
        .invoke("session_remove", json!({ "id": id }))
        .unwrap_err();
    assert_eq!(refused["command"], "session_remove");
    assert_eq!(refused["kind"], "sessionRunning");
    app.ok("session_stop", json!({ "id": id }));
    assert_eq!(app.state_of(&id), SessionState::Dormant);
    app.ok("session_stop", json!({ "id": id }));
    let again = app.ok(
        "session_start",
        json!({ "id": id, "request": { "mode": "resume", "cols": 80, "rows": 24 }, "onEvent": channel(4) }),
    );
    assert_ne!(again["terminal"], restarted["terminal"]);
    app.ok("session_stop", json!({ "id": id }));
    app.ok("session_remove", json!({ "id": id }));
    assert_eq!(
        app.ok("workspace_snapshot", json!({}))["sessions"],
        json!([])
    );
    // Closing it now, or late calls for a terminal that is gone, are not
    // failures.
    app.ok("session_close", json!({ "id": id }));
    app.ok("session_write", json!({ "id": terminal, "data": "x" }));
    app.ok("session_ack", json!({ "id": terminal, "bytes": 1 }));
}

#[test]
fn saying_no_to_quitting_is_heard() {
    let home = tempfile::tempdir().unwrap();
    let app = Harness::new(home.path());

    // `app_quit` itself is not called: it would end the test.
    assert_eq!(app.ok("app_quit_shown", json!({})), Value::Null);
    assert_eq!(app.ok("app_quit_cancel", json!({})), Value::Null);
    // Without a menu bar there is nothing to give shortcuts to.
    assert_eq!(
        app.ok(
            "menu_set_shortcuts",
            json!({ "shortcuts": [{ "command": "splitRight", "accelerator": "Cmd+KeyD" }] }),
        ),
        Value::Null
    );
}

#[test]
fn a_running_session_is_attached_to_again_without_a_restart() {
    let home = tempfile::tempdir().unwrap();
    let app = Harness::new(home.path());
    let session = app.ok(
        "session_create",
        json!({ "request": { "kind": "background", "command": waiting_line() } }),
    );
    let id = session["id"].as_str().unwrap().to_owned();
    let started = app.ok(
        "session_start",
        json!({ "id": id, "request": { "cols": 80, "rows": 24 }, "onEvent": channel(2) }),
    );

    let attached = app.ok(
        "session_attach",
        json!({ "id": id, "cols": 100, "rows": 30, "onEvent": channel(3) }),
    );

    // The same terminal: nothing was started again.
    assert_eq!(attached["terminal"], started["terminal"]);
    assert!(attached["ackBatchBytes"].as_u64().unwrap() > 0);
    assert_eq!(app.state_of(&id), SessionState::Running);

    // A session that does not run has nothing to attach to.
    app.ok("session_stop", json!({ "id": id }));
    let refused = app
        .invoke(
            "session_attach",
            json!({ "id": id, "cols": 100, "rows": 30, "onEvent": channel(4) }),
        )
        .unwrap_err();
    assert_eq!(refused["command"], "session_attach");
    assert_eq!(refused["kind"], "notFound");
}

#[test]
fn a_session_that_ends_on_its_own_is_exited_with_its_code() {
    let home = tempfile::tempdir().unwrap();
    let app = Harness::new(home.path());
    let line = if cfg!(windows) {
        "cmd /c 'exit 6'"
    } else {
        "exit 6"
    };
    let session = app.ok(
        "session_create",
        json!({ "request": { "kind": "background", "command": line } }),
    );
    let id = session["id"].as_str().unwrap().to_owned();

    app.ok(
        "session_start",
        json!({ "id": id, "request": { "cols": 80, "rows": 24 }, "onEvent": channel(4) }),
    );

    app.wait_for_state(&id, "exited", |state| {
        matches!(state, SessionState::Exited { .. })
    });
    let ended = app.ok("workspace_snapshot", json!({}));
    assert_eq!(
        ended["sessions"][0]["state"],
        json!({ "status": "exited", "code": 6, "signal": null, "failure": null })
    );
    assert_eq!(ended["sessions"][0]["terminalId"], Value::Null);
}

#[test]
fn the_layout_is_changed_by_the_ids_of_panes_and_splits() {
    let home = tempfile::tempdir().unwrap();
    let app = Harness::new(home.path());
    let session = app.ok("session_create", json!({ "request": { "kind": "shell" } }));
    let id = session["id"].as_str().unwrap().to_owned();

    let second = app.ok("layout_split", json!({ "pane": 1, "direction": "row" }));
    let third = app.ok(
        "layout_split",
        json!({ "pane": 1, "direction": "column", "placement": "before", "session": id }),
    );
    let layout = app.ok("workspace_snapshot", json!({}))["layout"].clone();
    let split = layout["root"]["id"].as_u64().unwrap();
    assert_eq!(layout["root"]["direction"], "row");
    assert_eq!(layout["focused"], third);
    assert_eq!(layout["root"]["first"]["first"]["session"], id.as_str());

    assert_eq!(
        app.ok(
            "layout_resize_split",
            json!({ "split": split, "ratio": 0.3 })
        ),
        json!(0.3)
    );
    assert_eq!(
        app.ok("layout_resize_split", json!({ "split": split, "ratio": 5 })),
        json!(0.9)
    );
    app.ok("layout_focus_pane", json!({ "pane": second }));
    app.ok(
        "layout_show_session",
        json!({ "pane": second, "session": id }),
    );
    app.ok(
        "layout_show_session",
        json!({ "pane": second, "session": null }),
    );
    app.ok(
        "layout_move_pane",
        json!({ "pane": second, "target": third, "edge": "left" }),
    );
    app.ok("layout_close_pane", json!({ "pane": second }));
    app.ok("layout_close_pane", json!({ "pane": third }));

    let layout = app.ok("workspace_snapshot", json!({}))["layout"].clone();
    assert_eq!(
        layout["root"],
        json!({ "type": "pane", "id": 1, "session": null })
    );
    assert_eq!(layout["focused"], 1);
}

#[test]
fn a_command_that_fails_rejects_with_its_name_and_a_message() {
    let home = tempfile::tempdir().unwrap();
    let app = Harness::new(home.path());

    let unknown = app
        .invoke(
            "session_start",
            json!({ "id": "s-ghost", "request": { "cols": 80, "rows": 24 }, "onEvent": channel(5) }),
        )
        .unwrap_err();
    assert_eq!(
        unknown,
        json!({ "command": "session_start", "kind": "notFound", "message": "no session with id s-ghost" })
    );

    let gone = home.path().join("gone");
    let missing = app
        .invoke(
            "session_create",
            json!({ "request": { "kind": "shell", "cwd": text(&gone) } }),
        )
        .unwrap_err();
    assert_eq!(missing["command"], "session_create");
    assert_eq!(missing["kind"], "folderMissing");
    assert!(
        missing["message"]
            .as_str()
            .unwrap()
            .contains("does not exist"),
        "{missing}"
    );

    let refused = app
        .invoke("layout_close_pane", json!({ "pane": 99 }))
        .unwrap_err();
    assert_eq!(
        refused,
        json!({ "command": "layout_close_pane", "kind": "notFound", "message": "no pane with id 99" })
    );
    let no_command = app
        .invoke(
            "session_create",
            json!({ "request": { "kind": "background" } }),
        )
        .unwrap_err();
    assert_eq!(no_command["command"], "session_create");
    assert_eq!(no_command["kind"], "invalidCommand");

    // An identifier that is not one is refused before the command runs.
    assert!(
        app.invoke("session_close", json!({ "id": "../etc" }))
            .is_err()
    );
}

#[test]
fn the_claude_code_integration_is_previewed_installed_and_removed() {
    let home = tempfile::tempdir().unwrap();
    let app = Harness::new(home.path());

    let status = app.ok("get_claude_integration", json!({}));
    assert_eq!(status["plugin"], "notInstalled");
    assert_eq!(status["consent"], "unasked");
    assert!(status["claudeInstalled"].is_boolean());

    let preview = app.ok("preview_claude_integration", json!({}));
    let changes = preview["changes"].as_array().unwrap();
    assert!(!changes.is_empty());
    // No settings yet in this home: the note says the file is created.
    assert!(
        preview["note"]
            .as_str()
            .unwrap()
            .contains("creates settings.json")
    );
    assert!(
        changes
            .iter()
            .all(|change| change["path"].as_str().unwrap().starts_with('~'))
    );

    app.ok("set_claude_consent", json!({ "consent": "declined" }));
    assert_eq!(
        app.ok("get_claude_integration", json!({}))["consent"],
        "declined"
    );

    let (sender, changes) = std::sync::mpsc::channel();
    tauri::Listener::listen_any(&app.webview, crate::claude::CHANGED_EVENT, move |event| {
        let _ = sender.send(event.payload().to_owned());
    });
    app.ok("install_claude_integration", json!({}));
    changes.recv_timeout(TIMEOUT).unwrap();
    let status = app.ok("get_claude_integration", json!({}));
    assert_eq!(status["plugin"], "installed");
    assert_eq!(status["consent"], "accepted");
    assert!(status["version"].is_string());
    assert!(
        home.path()
            .join("claude-config")
            .join("settings.json")
            .exists()
    );

    app.ok("uninstall_claude_integration", json!({}));
    let status = app.ok("get_claude_integration", json!({}));
    assert_eq!(status["plugin"], "notInstalled");
    assert_eq!(status["consent"], "declined");

    // Settings that are not JSON are refused with a kind of their own.
    std::fs::write(
        home.path().join("claude-config").join("settings.json"),
        "{ nope",
    )
    .unwrap();
    let refused = app
        .invoke("install_claude_integration", json!({}))
        .unwrap_err();
    assert_eq!(refused["kind"], "invalidSettings");
    assert!(
        app.invoke("set_claude_consent", json!({ "consent": "maybe" }))
            .is_err()
    );
}

#[test]
fn the_attention_badge_takes_a_count() {
    let home = tempfile::tempdir().unwrap();
    let app = Harness::new(home.path());

    // The mock runtime has no Dock: what matters is that the call is read.
    assert!(
        app.ok("set_attention_badge", json!({ "count": 3 }))
            .is_boolean()
    );
    assert!(
        app.invoke("set_attention_badge", json!({ "count": -1 }))
            .is_err()
    );
}
