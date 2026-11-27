//! The workspace as an application uses it: sessions, order, titles,
//! layout, restoring, saving and change notifications. No process is
//! started here; `tests/workspace_launch.rs` does that.

// The helpers below are test code too: a failed `unwrap` is a failed test.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use termx_core::pty::ExitStatus;
use termx_core::workspace::launch::{CLAUDE_MISSING_EXIT_CODE, LaunchMarker, Shell, marker};
use termx_core::workspace::layout::Node;
use termx_core::workspace::project::normalize;
use termx_core::workspace::store::STATE_FILE;
use termx_core::workspace::{
    Direction, Launch, LaunchFailure, LaunchMode, Launcher, NewSession, PersistenceMode, Placement,
    ProjectKind, Session, SessionKey, SessionKind, SessionState, WindowGeometry, Workspace,
    WorkspaceConfig, WorkspaceError,
};

/// Upper bound for any wait: generous, because CI machines are slow.
const TIMEOUT: Duration = Duration::from_secs(30);

/// A long time for the things a test does not want to happen on their own.
const NEVER: Duration = Duration::from_secs(3600);

/// A shell that exists on the machine, so that the commands built are the
/// same wherever the tests run.
fn test_shell() -> Shell {
    Shell::new(if cfg!(windows) {
        "powershell.exe"
    } else {
        "/bin/sh"
    })
}

/// A configuration in which nothing happens on its own: tests ask for it.
fn quiet(state_dir: Option<&Path>) -> WorkspaceConfig {
    let mut config = match state_dir {
        Some(dir) => WorkspaceConfig::new(dir),
        None => WorkspaceConfig::in_memory(),
    };
    config.launcher = Launcher::new(test_shell());
    config.activity_coalesce = NEVER;
    config.save_debounce = NEVER;
    config.save_max_wait = NEVER;
    config.git_poll_interval = NEVER;
    config
}

fn wait_until(what: &str, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + TIMEOUT;
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A folder for sessions, in the form the workspace reports paths in.
struct Folders {
    dir: tempfile::TempDir,
}

impl Folders {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn make(&self, name: &str) -> PathBuf {
        let folder = normalize(self.dir.path()).join(name);
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    /// A folder that is a Git repository on `branch`, made by hand.
    fn repository(&self, name: &str, branch: &str) -> PathBuf {
        let folder = self.make(name);
        let git = folder.join(".git");
        std::fs::create_dir_all(git.join("objects")).unwrap();
        std::fs::create_dir_all(git.join("refs")).unwrap();
        set_branch(&folder, branch);
        folder
    }

    fn state_dir(&self) -> PathBuf {
        self.dir.path().join("state")
    }
}

fn set_branch(repository: &Path, branch: &str) {
    std::fs::write(
        repository.join(".git").join("HEAD"),
        format!("ref: refs/heads/{branch}\n"),
    )
    .unwrap();
}

fn keys(workspace: &Workspace) -> Vec<SessionKey> {
    workspace
        .snapshot()
        .sessions
        .into_iter()
        .map(|session| session.id)
        .collect()
}

fn create(workspace: &Workspace, kind: SessionKind, cwd: &Path) -> Session {
    let mut new = NewSession::new(kind, cwd);
    if kind == SessionKind::Background {
        new.command = Some("pnpm dev".to_owned());
    }
    workspace.create_session(new).unwrap()
}

/// The arguments of a command, from its debug form: `PtyCommand` does not
/// expose them.
fn described(launch: &termx_core::workspace::Launch) -> String {
    format!("{:?}", launch.command)
}

// --- Sessions --------------------------------------------------------------

#[test]
fn a_new_session_is_starting_shown_and_remembered_as_recent() {
    let folders = Folders::new();
    let repository = folders.repository("app", "main");
    let workspace = Workspace::open(quiet(None));
    let before = termx_core::workspace::now_ms();

    let session = create(&workspace, SessionKind::Claude, &repository);

    assert_eq!(session.kind, SessionKind::Claude);
    assert_eq!(session.cwd, repository);
    assert_eq!(session.live_cwd, None);
    assert_eq!(session.command, None);
    assert_eq!(session.title, "Claude Code");
    assert_eq!(
        (session.name.clone(), session.auto_title.clone()),
        (None, None)
    );
    assert_eq!(session.project.kind, ProjectKind::Git);
    assert_eq!(session.project.name, "app");
    assert_eq!(session.project.root, repository);
    assert_eq!(session.git.clone().unwrap().branch.as_deref(), Some("main"));
    assert_eq!(session.state, SessionState::Starting);
    assert_eq!(session.terminal_id, None);
    assert!(session.created_at >= before);
    assert_eq!(session.last_activity_at, session.created_at);
    assert_eq!(session.waiting_since, None);
    assert_eq!(session.claude_session_id, None);

    let snapshot = workspace.snapshot();
    assert_eq!(snapshot.sessions, std::slice::from_ref(&session));
    assert_eq!(snapshot.projects, std::slice::from_ref(&session.project));
    assert_eq!(snapshot.recent_folders, [repository]);
    assert_eq!(snapshot.persistence, PersistenceMode::Memory);
    assert_eq!(
        snapshot.layout.session_of(snapshot.layout.focused()),
        Ok(Some(&session.id))
    );
    assert_eq!(workspace.session(&session.id), Some(session));
}

#[test]
fn a_session_needs_an_existing_absolute_folder() {
    let folders = Folders::new();
    let workspace = Workspace::open(quiet(None));
    let gone = folders.make("x").join("gone");
    let file = folders.make("y").join("file.txt");
    std::fs::write(&file, b"").unwrap();

    assert_eq!(
        workspace.create_session(NewSession::new(SessionKind::Shell, &gone)),
        Err(WorkspaceError::FolderNotFound(gone))
    );
    assert_eq!(
        workspace.create_session(NewSession::new(SessionKind::Shell, &file)),
        Err(WorkspaceError::FolderNotFound(file))
    );
    assert_eq!(
        workspace.create_session(NewSession::new(SessionKind::Shell, "relative/folder")),
        Err(WorkspaceError::InvalidFolder)
    );
    assert!(workspace.snapshot().sessions.is_empty());
    assert!(workspace.recent_folders().is_empty());
}

#[test]
fn a_background_session_needs_a_command_and_the_others_have_none() {
    let folders = Folders::new();
    let folder = folders.make("app");
    let workspace = Workspace::open(quiet(None));

    for command in [
        None,
        Some(String::new()),
        Some("   ".to_owned()),
        Some("a\nb".to_owned()),
        Some("x".repeat(9000)),
    ] {
        let mut new = NewSession::new(SessionKind::Background, &folder);
        new.command = command;
        assert_eq!(
            workspace.create_session(new),
            Err(WorkspaceError::InvalidCommand)
        );
    }

    let mut new = NewSession::new(SessionKind::Background, &folder);
    new.command = Some("  ngrok http 3000  ".to_owned());
    let tunnel = workspace.create_session(new).unwrap();
    assert_eq!(tunnel.command.as_deref(), Some("ngrok http 3000"));
    assert_eq!(tunnel.title, "ngrok http 3000");

    let mut new = NewSession::new(SessionKind::Shell, &folder);
    new.command = Some("ignored".to_owned());
    new.name = Some("  my shell ".to_owned());
    let shell = workspace.create_session(new).unwrap();
    assert_eq!(shell.command, None);
    assert_eq!(shell.name.as_deref(), Some("my shell"));
    assert_eq!(shell.title, "my shell");
}

#[test]
fn a_manual_name_wins_over_automatic_titles_and_is_never_overwritten() {
    let folders = Folders::new();
    let workspace = Workspace::open(quiet(None));
    let id = create(&workspace, SessionKind::Claude, &folders.make("app")).id;
    let title = |workspace: &Workspace| workspace.session(&id).unwrap().title;

    workspace.report_title(&id, "✳ Fix the login").unwrap();
    assert_eq!(title(&workspace), "Fix the login");

    workspace
        .rename_session(&id, Some("  Login work  "))
        .unwrap();
    assert_eq!(title(&workspace), "Login work");

    // Automatic titles keep arriving; the name stays.
    workspace
        .report_title(&id, "⠐ Refactor the session")
        .unwrap();
    let session = workspace.session(&id).unwrap();
    assert_eq!(session.title, "Login work");
    assert_eq!(session.name.as_deref(), Some("Login work"));
    assert_eq!(session.auto_title.as_deref(), Some("Refactor the session"));

    // Removing the name brings the latest automatic title back.
    workspace.rename_session(&id, None).unwrap();
    assert_eq!(title(&workspace), "Refactor the session");
    workspace.rename_session(&id, Some(" \t ")).unwrap();
    assert_eq!(workspace.session(&id).unwrap().name, None);

    // A program that clears its title leaves the default.
    workspace.report_title(&id, "").unwrap();
    assert_eq!(title(&workspace), "Claude Code");

    let ghost = SessionKey::from_raw("s-ghost").unwrap();
    assert_eq!(
        workspace.rename_session(&ghost, Some("x")),
        Err(WorkspaceError::UnknownSession(ghost.clone()))
    );
    assert_eq!(
        workspace.report_title(&ghost, "x"),
        Err(WorkspaceError::UnknownSession(ghost))
    );
}

#[test]
fn the_animation_of_a_title_is_not_a_change() {
    let folders = Folders::new();
    let workspace = Workspace::open(quiet(None));
    let id = create(&workspace, SessionKind::Claude, &folders.make("app")).id;
    workspace.report_title(&id, "✳ Fix the login").unwrap();
    let subscription = workspace.subscribe();
    let revision = workspace.snapshot().revision;

    for frame in ["⠂", "⠐", "⠠", "✳", "✻"] {
        workspace
            .report_title(&id, &format!("{frame} Fix the login"))
            .unwrap();
    }

    assert_eq!(subscription.try_recv(), None);
    assert_eq!(workspace.snapshot().revision, revision);
}

#[test]
fn sessions_are_reordered_by_the_user() {
    let folders = Folders::new();
    let folder = folders.make("app");
    let workspace = Workspace::open(quiet(None));
    let a = create(&workspace, SessionKind::Shell, &folder).id;
    let b = create(&workspace, SessionKind::Shell, &folder).id;
    let c = create(&workspace, SessionKind::Shell, &folder).id;
    assert_eq!(keys(&workspace), [a.clone(), b.clone(), c.clone()]);

    workspace.reorder_session(&c, Some(&a)).unwrap();
    assert_eq!(keys(&workspace), [c.clone(), a.clone(), b.clone()]);

    workspace.reorder_session(&c, None).unwrap();
    assert_eq!(keys(&workspace), [a.clone(), b.clone(), c.clone()]);

    workspace.reorder_session(&a, Some(&c)).unwrap();
    assert_eq!(keys(&workspace), [b.clone(), a.clone(), c.clone()]);

    // Moves that move nothing are not changes.
    let subscription = workspace.subscribe();
    workspace.reorder_session(&a, Some(&a)).unwrap();
    workspace.reorder_session(&a, Some(&c)).unwrap();
    workspace.reorder_session(&c, None).unwrap();
    assert_eq!(subscription.try_recv(), None);

    let ghost = SessionKey::from_raw("s-ghost").unwrap();
    assert_eq!(
        workspace.reorder_session(&ghost, None),
        Err(WorkspaceError::UnknownSession(ghost.clone()))
    );
    assert_eq!(
        workspace.reorder_session(&a, Some(&ghost)),
        Err(WorkspaceError::UnknownSession(ghost))
    );
    assert_eq!(keys(&workspace), [b, a, c]);
}

#[test]
fn sessions_are_grouped_by_project_and_projects_are_reordered() {
    let folders = Folders::new();
    let app = folders.repository("app", "main");
    let app_src = app.join("src");
    std::fs::create_dir_all(&app_src).unwrap();
    let notes = folders.make("notes");
    let api = folders.repository("api", "main");
    let workspace = Workspace::open(quiet(None));

    let first = create(&workspace, SessionKind::Claude, &app);
    let second = create(&workspace, SessionKind::Shell, &notes);
    let third = create(&workspace, SessionKind::Claude, &app_src);
    let fourth = create(&workspace, SessionKind::Background, &api);

    // Two folders of one repository are one project.
    assert_eq!(first.project, third.project);
    assert_eq!(second.project.kind, ProjectKind::Folder);
    let names = |workspace: &Workspace| -> Vec<String> {
        workspace
            .snapshot()
            .projects
            .into_iter()
            .map(|project| project.name)
            .collect()
    };
    assert_eq!(names(&workspace), ["app", "notes", "api"]);

    workspace
        .reorder_project(&fourth.project.key, Some(&first.project.key))
        .unwrap();
    assert_eq!(names(&workspace), ["api", "app", "notes"]);
    workspace.reorder_project(&first.project.key, None).unwrap();
    assert_eq!(names(&workspace), ["api", "notes", "app"]);

    assert_eq!(
        workspace.reorder_project("no such project", None),
        Err(WorkspaceError::UnknownProject)
    );
    assert_eq!(
        workspace.reorder_project(&first.project.key, Some("no such project")),
        Err(WorkspaceError::UnknownProject)
    );

    // A project goes with its last session, and keeps its place if it
    // comes back.
    workspace.close_session(&second.id).unwrap();
    assert_eq!(names(&workspace), ["api", "app"]);
    create(&workspace, SessionKind::Shell, &notes);
    assert_eq!(names(&workspace), ["api", "notes", "app"]);
}

#[test]
fn another_claude_session_in_the_same_project_asks_nothing() {
    let folders = Folders::new();
    let repository = folders.repository("app", "main");
    let workspace = Workspace::open(quiet(None));
    let shell = create(&workspace, SessionKind::Shell, &repository);

    let sibling = workspace.create_sibling(&shell.id).unwrap();

    assert_eq!(sibling.kind, SessionKind::Claude);
    assert_eq!(sibling.cwd, repository);
    assert_eq!(sibling.project, shell.project);
    assert_ne!(sibling.id, shell.id);
    assert_eq!(keys(&workspace), [shell.id, sibling.id.clone()]);
    // It takes the focused pane.
    let layout = workspace.layout();
    assert_eq!(layout.session_of(layout.focused()), Ok(Some(&sibling.id)));

    let ghost = SessionKey::from_raw("s-ghost").unwrap();
    assert_eq!(
        workspace.create_sibling(&ghost),
        Err(WorkspaceError::UnknownSession(ghost))
    );
}

#[test]
fn closing_a_session_removes_it_and_empties_its_pane() {
    let folders = Folders::new();
    let folder = folders.make("app");
    let workspace = Workspace::open(quiet(None));
    let a = create(&workspace, SessionKind::Shell, &folder).id;
    let b = create(&workspace, SessionKind::Shell, &folder).id;
    let launch = workspace.begin_launch(&b, LaunchMode::Fresh).unwrap();
    assert!(workspace.terminal_started(&b, launch.generation, 41));

    // The terminal to close is handed back.
    assert_eq!(workspace.close_session(&b), Ok(Some(41)));

    assert_eq!(keys(&workspace), std::slice::from_ref(&a));
    assert_eq!(workspace.layout().pane_of(&b), None);
    assert_eq!(workspace.session(&b), None);
    assert_eq!(
        workspace.close_session(&b),
        Err(WorkspaceError::UnknownSession(b.clone()))
    );
    // News from its terminal is ignored.
    workspace.terminal_exited(&b, launch.generation, &ExitStatus::from_code(0));
    assert!(!workspace.terminal_started(&b, launch.generation, 42));
    assert_eq!(workspace.close_session(&a), Ok(None));
}

#[test]
fn recent_folders_are_bounded_most_recent_first_and_without_duplicates() {
    let folders = Folders::new();
    let workspace = Workspace::open(quiet(None));
    let made: Vec<PathBuf> = (0..25).map(|n| folders.make(&format!("f{n:02}"))).collect();

    for folder in &made {
        create(&workspace, SessionKind::Shell, folder);
    }
    let recent = workspace.recent_folders();
    assert_eq!(recent.len(), 20);
    assert_eq!(recent[0], made[24]);
    assert_eq!(recent[19], made[5]);

    // Using a folder again moves it to the front instead of repeating it.
    create(&workspace, SessionKind::Shell, &made[10]);
    let recent = workspace.recent_folders();
    assert_eq!(recent.len(), 20);
    assert_eq!(recent[0], made[10]);
    assert_eq!(recent.iter().filter(|f| **f == made[10]).count(), 1);

    assert!(workspace.forget_recent_folder(&made[10]));
    assert!(!workspace.forget_recent_folder(&made[10]));
    assert_eq!(workspace.recent_folders().len(), 19);
    assert_eq!(workspace.recent_folders()[0], made[24]);
}

// --- Launching and state ---------------------------------------------------

#[test]
fn a_session_goes_from_starting_to_running_to_exited() {
    let folders = Folders::new();
    let workspace = Workspace::open(quiet(None));
    let id = create(&workspace, SessionKind::Shell, &folders.make("app")).id;
    let state = |workspace: &Workspace| workspace.session(&id).unwrap().state;

    let launch = workspace.begin_launch(&id, LaunchMode::Fresh).unwrap();
    assert_eq!(state(&workspace), SessionState::Starting);
    assert_eq!(launch.previous_terminal, None);

    assert!(workspace.terminal_started(&id, launch.generation, 7));
    let session = workspace.session(&id).unwrap();
    assert_eq!(session.state, SessionState::Running);
    assert_eq!(session.terminal_id, Some(7));

    workspace.terminal_exited(&id, launch.generation, &ExitStatus::from_code(3));
    let session = workspace.session(&id).unwrap();
    assert_eq!(
        session.state,
        SessionState::Exited {
            code: Some(3),
            signal: None,
            failure: None
        }
    );
    assert_eq!(session.terminal_id, None);

    // Starting it again closes nothing, and counts as a new launch.
    let again = workspace.begin_launch(&id, LaunchMode::Fresh).unwrap();
    assert_ne!(again.generation, launch.generation);
    assert_eq!(state(&workspace), SessionState::Starting);
}

#[test]
fn a_process_that_ends_before_its_start_is_reported_stays_exited() {
    let folders = Folders::new();
    let workspace = Workspace::open(quiet(None));
    let id = create(&workspace, SessionKind::Shell, &folders.make("app")).id;
    let launch = workspace.begin_launch(&id, LaunchMode::Fresh).unwrap();

    workspace.terminal_exited(&id, launch.generation, &ExitStatus::from_code(0));
    // The launch is still the current one, so the terminal is not refused.
    assert!(workspace.terminal_started(&id, launch.generation, 9));

    let session = workspace.session(&id).unwrap();
    assert!(matches!(
        session.state,
        SessionState::Exited { code: Some(0), .. }
    ));
    assert_eq!(session.terminal_id, None);
}

#[test]
fn restarting_a_running_session_hands_back_its_terminal_and_ignores_its_news() {
    let folders = Folders::new();
    let workspace = Workspace::open(quiet(None));
    let id = create(&workspace, SessionKind::Shell, &folders.make("app")).id;
    let first = workspace.begin_launch(&id, LaunchMode::Fresh).unwrap();
    assert!(workspace.terminal_started(&id, first.generation, 1));

    let second = workspace.begin_launch(&id, LaunchMode::Fresh).unwrap();

    assert_eq!(second.previous_terminal, Some(1));
    assert_eq!(
        workspace.session(&id).unwrap().state,
        SessionState::Starting
    );
    // The old terminal dies; that is not the exit of the new launch.
    workspace.terminal_exited(&id, first.generation, &ExitStatus::from_code(1));
    assert_eq!(
        workspace.session(&id).unwrap().state,
        SessionState::Starting
    );
    assert!(!workspace.terminal_started(&id, first.generation, 5));
    workspace.launch_failed(&id, first.generation, LaunchFailure::spawn_failed("late"));
    assert_eq!(
        workspace.session(&id).unwrap().state,
        SessionState::Starting
    );

    assert!(workspace.terminal_started(&id, second.generation, 2));
    assert_eq!(workspace.session(&id).unwrap().terminal_id, Some(2));
}

#[test]
fn a_claude_that_is_not_installed_ends_the_session_with_a_typed_failure() {
    let folders = Folders::new();
    let workspace = Workspace::open(quiet(None));
    let id = create(&workspace, SessionKind::Claude, &folders.make("app")).id;
    let launch = workspace.begin_launch(&id, LaunchMode::Fresh).unwrap();
    assert!(workspace.terminal_started(&id, launch.generation, 1));
    let token = launch
        .token
        .clone()
        .expect("a Claude Code session has markers");
    let mut filter = workspace.output_filter(&id, &launch).unwrap();

    // What the script prints when the shell cannot find `claude`.
    let printed = filter.filter(
        format!(
            "profile output{}",
            marker(&token, LaunchMarker::ClaudeMissing)
        )
        .into_bytes(),
    );
    assert_eq!(printed, b"profile output");
    workspace.terminal_exited(
        &id,
        launch.generation,
        &ExitStatus::from_code(CLAUDE_MISSING_EXIT_CODE),
    );

    let SessionState::Exited {
        code,
        failure: Some(failure),
        ..
    } = workspace.session(&id).unwrap().state
    else {
        panic!("the session has no failure");
    };
    assert_eq!(code, Some(CLAUDE_MISSING_EXIT_CODE));
    assert!(matches!(failure, LaunchFailure::ClaudeNotFound { .. }));
    assert!(failure.message().contains("Claude Code is not installed"));
    // In the form the frontend receives.
    assert_eq!(
        serde_json::to_value(workspace.session(&id).unwrap().state).unwrap()["failure"]["kind"],
        "claudeNotFound"
    );
}

#[test]
fn only_the_markers_of_the_current_start_count() {
    let folders = Folders::new();
    let workspace = Workspace::open(quiet(None));
    let id = create(&workspace, SessionKind::Claude, &folders.make("app")).id;
    let first = workspace.begin_launch(&id, LaunchMode::Fresh).unwrap();
    let mut old_filter = workspace.output_filter(&id, &first).unwrap();
    let second = workspace.begin_launch(&id, LaunchMode::Fresh).unwrap();
    assert!(workspace.terminal_started(&id, second.generation, 2));
    assert_ne!(first.token, second.token);

    // The old terminal says `claude` is missing; the new one, with the
    // token of the first, that it runs it: neither counts.
    old_filter
        .filter(marker(first.token.as_ref().unwrap(), LaunchMarker::ClaudeMissing).into_bytes());
    let mut filter = workspace.output_filter(&id, &second).unwrap();
    let forged = marker(first.token.as_ref().unwrap(), LaunchMarker::Exec);
    assert_eq!(
        filter.filter(forged.clone().into_bytes()),
        forged.into_bytes()
    );
    workspace.terminal_exited(&id, second.generation, &ExitStatus::from_code(0));

    let SessionState::Exited { failure, .. } = workspace.session(&id).unwrap().state else {
        panic!("the session did not end");
    };
    assert!(
        matches!(failure, Some(LaunchFailure::LaunchUnconfirmed { .. })),
        "{failure:?}"
    );
}

#[test]
fn a_terminal_that_cannot_be_started_ends_the_session_with_the_reason() {
    let folders = Folders::new();
    let workspace = Workspace::open(quiet(None));
    let id = create(&workspace, SessionKind::Shell, &folders.make("app")).id;
    let launch = workspace.begin_launch(&id, LaunchMode::Fresh).unwrap();

    workspace.launch_failed(
        &id,
        launch.generation,
        LaunchFailure::spawn_failed("too many open files"),
    );

    assert_eq!(
        workspace.session(&id).unwrap().state,
        SessionState::Exited {
            code: None,
            signal: None,
            failure: Some(LaunchFailure::spawn_failed("too many open files")),
        }
    );
}

#[test]
fn a_session_whose_folder_is_gone_cannot_start_and_says_why() {
    let folders = Folders::new();
    let folder = folders.make("temporary");
    let workspace = Workspace::open(quiet(None));
    let id = create(&workspace, SessionKind::Claude, &folder).id;
    std::fs::remove_dir_all(&folder).unwrap();

    let refused = workspace.begin_launch(&id, LaunchMode::Resume);

    assert_eq!(
        refused,
        Err(WorkspaceError::Launch(LaunchFailure::folder_missing()))
    );
    assert_eq!(
        workspace.session(&id).unwrap().state,
        SessionState::Exited {
            code: None,
            signal: None,
            failure: Some(LaunchFailure::folder_missing()),
        }
    );
}

#[test]
fn a_running_session_whose_folder_is_gone_keeps_running_when_a_restart_is_refused() {
    let folders = Folders::new();
    let folder = folders.make("temporary");
    let workspace = Workspace::open(quiet(None));
    let id = create(&workspace, SessionKind::Shell, &folder).id;
    let launch = workspace.begin_launch(&id, LaunchMode::Fresh).unwrap();
    assert!(workspace.terminal_started(&id, launch.generation, 4));
    std::fs::remove_dir_all(&folder).unwrap();

    assert!(workspace.begin_launch(&id, LaunchMode::Fresh).is_err());

    let session = workspace.session(&id).unwrap();
    assert_eq!(session.state, SessionState::Running);
    assert_eq!(session.terminal_id, Some(4));
    // Its exit is still the exit of the current launch.
    workspace.terminal_exited(&id, launch.generation, &ExitStatus::from_code(0));
    assert!(matches!(
        workspace.session(&id).unwrap().state,
        SessionState::Exited { .. }
    ));
}

#[test]
fn a_session_whose_terminal_is_taken_away_becomes_dormant() {
    let folders = Folders::new();
    let workspace = Workspace::open(quiet(None));
    let id = create(&workspace, SessionKind::Shell, &folders.make("app")).id;
    let launch = workspace.begin_launch(&id, LaunchMode::Fresh).unwrap();
    assert!(workspace.terminal_started(&id, launch.generation, 4));

    assert!(workspace.suspend_session(&id));

    let session = workspace.session(&id).unwrap();
    assert_eq!(session.state, SessionState::Dormant);
    assert_eq!(session.terminal_id, None);
    assert!(!workspace.suspend_session(&id), "it is not active any more");
    // The terminal that was taken away has nothing more to say.
    workspace.terminal_exited(&id, launch.generation, &ExitStatus::from_code(1));
    assert_eq!(workspace.session(&id).unwrap().state, SessionState::Dormant);
}

// --- Restoring -------------------------------------------------------------

#[test]
fn sessions_come_back_dormant_with_everything_but_their_process() {
    let folders = Folders::new();
    let app = folders.repository("app", "main");
    let notes = folders.make("notes");
    let (claude, shell, background, layout, projects) = {
        let workspace = Workspace::open(quiet(Some(&folders.state_dir())));
        assert_eq!(workspace.persistence(), PersistenceMode::Primary);
        let claude = create(&workspace, SessionKind::Claude, &app);
        let shell = create(&workspace, SessionKind::Shell, &notes);
        let background = create(&workspace, SessionKind::Background, &app);
        workspace
            .rename_session(&shell.id, Some("scratch"))
            .unwrap();
        workspace
            .report_title(&claude.id, "✳ Fix the login")
            .unwrap();
        workspace
            .set_claude_session_id(&claude.id, Some("conv-123"))
            .unwrap();
        workspace
            .reorder_session(&background.id, Some(&claude.id))
            .unwrap();
        workspace
            .reorder_project(&shell.project.key, Some(&claude.project.key))
            .unwrap();
        let focused = workspace.layout().focused();
        workspace
            .split_pane(
                focused,
                Direction::Row,
                Placement::After,
                Some(claude.id.clone()),
            )
            .unwrap();
        workspace.set_window_geometry(WindowGeometry {
            x: 40.0,
            y: 60.0,
            width: 1400.0,
            height: 900.0,
            maximized: false,
        });
        // Everything is running when the application quits.
        for id in [&claude.id, &shell.id, &background.id] {
            let launch = workspace.begin_launch(id, LaunchMode::Fresh).unwrap();
            assert!(workspace.terminal_started(id, launch.generation, 1));
        }
        let snapshot = workspace.snapshot();
        workspace.shutdown();
        (
            workspace.session(&claude.id).unwrap(),
            workspace.session(&shell.id).unwrap(),
            workspace.session(&background.id).unwrap(),
            snapshot.layout,
            snapshot.projects,
        )
    };

    let workspace = Workspace::open(quiet(Some(&folders.state_dir())));

    let snapshot = workspace.snapshot();
    assert_eq!(
        snapshot
            .sessions
            .iter()
            .map(|s| s.id.clone())
            .collect::<Vec<_>>(),
        [background.id.clone(), claude.id.clone(), shell.id.clone()],
        "the order of the user is restored"
    );
    assert_eq!(snapshot.projects, projects);
    assert_eq!(snapshot.layout, layout);
    assert_eq!(snapshot.recent_folders, [app.clone(), notes.clone()]);
    assert_eq!(
        workspace
            .window_geometry()
            .map(|g| (g.x, g.y, g.width, g.height)),
        Some((40.0, 60.0, 1400.0, 900.0))
    );
    for before in [&claude, &shell, &background] {
        let after = workspace.session(&before.id).unwrap();
        // Nothing runs after a restart...
        assert_eq!(after.state, SessionState::Dormant);
        assert_eq!(after.terminal_id, None);
        // ...and everything else is as it was.
        let expected = Session {
            state: SessionState::Dormant,
            terminal_id: None,
            ..before.clone()
        };
        assert_eq!(after, expected);
    }
    assert_eq!(workspace.session(&shell.id).unwrap().title, "scratch");
    assert_eq!(
        workspace.session(&claude.id).unwrap().title,
        "Fix the login"
    );
}

#[test]
fn each_kind_resumes_in_its_own_way() {
    let folders = Folders::new();
    let app = folders.make("app");
    let ids = {
        let workspace = Workspace::open(quiet(Some(&folders.state_dir())));
        let known = create(&workspace, SessionKind::Claude, &app).id;
        let unknown = create(&workspace, SessionKind::Claude, &app).id;
        let shell = create(&workspace, SessionKind::Shell, &app).id;
        let background = create(&workspace, SessionKind::Background, &app).id;
        workspace
            .set_claude_session_id(&known, Some("0b9d6f7e-3c1a-4f3e-9a55-2f6f1f0f8a10"))
            .unwrap();
        [known, unknown, shell, background]
    };
    let workspace = Workspace::open(quiet(Some(&folders.state_dir())));
    let [known, unknown, shell, background] = &ids;
    let launcher = Launcher::new(test_shell());
    // The same command, with the token the workspace drew for that start.
    let request = |kind, command, claude_session_id, mode, launch: &Launch| {
        launcher
            .command(&termx_core::workspace::launch::LaunchRequest {
                kind,
                cwd: &app,
                command,
                claude_session_id,
                mode,
                token: launch.token.as_ref(),
            })
            .unwrap()
    };

    // Claude Code resumes the conversation it knows, by its identifier...
    let resumed = workspace.begin_launch(known, LaunchMode::Resume).unwrap();
    assert_eq!(
        resumed.command,
        request(
            SessionKind::Claude,
            None,
            Some("0b9d6f7e-3c1a-4f3e-9a55-2f6f1f0f8a10"),
            LaunchMode::Resume,
            &resumed,
        )
    );
    if cfg!(unix) {
        assert!(
            described(&resumed).contains("claude --resume 0b9d6f7e-3c1a-4f3e-9a55-2f6f1f0f8a10"),
            "{}",
            described(&resumed)
        );
    }
    // ...and otherwise the most recent one of the folder.
    let continued = workspace.begin_launch(unknown, LaunchMode::Resume).unwrap();
    assert_eq!(
        continued.command,
        request(
            SessionKind::Claude,
            None,
            None,
            LaunchMode::Resume,
            &continued
        )
    );
    if cfg!(unix) {
        assert!(described(&continued).contains("claude --continue"));
        assert!(!described(&continued).contains("--resume"));
    }
    // A restart is a new conversation, whatever is known.
    let fresh = workspace.begin_launch(known, LaunchMode::Fresh).unwrap();
    assert_eq!(
        fresh.command,
        request(SessionKind::Claude, None, None, LaunchMode::Fresh, &fresh)
    );
    assert_ne!(fresh.command, resumed.command);

    // A shell and a background command start again in the same folder,
    // whichever way they are started.
    for mode in [LaunchMode::Resume, LaunchMode::Fresh] {
        let launch = workspace.begin_launch(shell, mode).unwrap();
        assert_eq!(launch.token, None, "a shell runs no script");
        assert_eq!(
            launch.command,
            request(SessionKind::Shell, None, None, mode, &launch)
        );
        let launch = workspace.begin_launch(background, mode).unwrap();
        assert_eq!(
            launch.command,
            request(
                SessionKind::Background,
                Some("pnpm dev"),
                None,
                mode,
                &launch
            )
        );
    }
    for id in &ids {
        assert_eq!(workspace.session(id).unwrap().state, SessionState::Starting);
    }
}

#[test]
fn the_identifier_of_a_conversation_is_checked() {
    let folders = Folders::new();
    let workspace = Workspace::open(quiet(None));
    let id = create(&workspace, SessionKind::Claude, &folders.make("app")).id;

    assert_eq!(
        workspace.set_claude_session_id(&id, Some("x; rm -rf ~")),
        Err(WorkspaceError::InvalidClaudeSessionId)
    );
    assert_eq!(workspace.session(&id).unwrap().claude_session_id, None);

    workspace
        .set_claude_session_id(&id, Some("conv-1"))
        .unwrap();
    assert_eq!(
        workspace.session(&id).unwrap().claude_session_id.as_deref(),
        Some("conv-1")
    );
    workspace.set_claude_session_id(&id, None).unwrap();
    assert_eq!(workspace.session(&id).unwrap().claude_session_id, None);
}

#[test]
fn a_restored_layout_never_shows_a_session_that_is_gone() {
    let folders = Folders::new();
    let folder = folders.make("app");
    let kept = {
        let workspace = Workspace::open(quiet(Some(&folders.state_dir())));
        let kept = create(&workspace, SessionKind::Shell, &folder).id;
        let dropped = create(&workspace, SessionKind::Shell, &folder).id;
        let focused = workspace.layout().focused();
        workspace
            .split_pane(
                focused,
                Direction::Column,
                Placement::Before,
                Some(kept.clone()),
            )
            .unwrap();
        workspace.shutdown();
        // Someone edits the file by hand and removes a session.
        let path = folders.state_dir().join(STATE_FILE);
        let mut json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        json["sessions"]
            .as_array_mut()
            .unwrap()
            .retain(|session| session["id"] != dropped.as_str());
        std::fs::write(&path, json.to_string()).unwrap();
        kept
    };

    let workspace = Workspace::open(quiet(Some(&folders.state_dir())));

    let layout = workspace.layout();
    layout.check_invariants().unwrap();
    assert_eq!(layout.pane_count(), 2);
    let shown: Vec<_> = layout
        .panes()
        .into_iter()
        .filter_map(|(_, s)| s.cloned())
        .collect();
    assert_eq!(shown, [kept]);
}

// --- Saving ----------------------------------------------------------------

#[test]
fn changes_are_saved_on_their_own_after_a_pause() {
    let folders = Folders::new();
    let mut config = quiet(Some(&folders.state_dir()));
    config.save_debounce = Duration::from_millis(40);
    config.save_max_wait = Duration::from_millis(400);
    let workspace = Workspace::open(config);
    let path = workspace.state_file().unwrap();
    assert!(!path.exists(), "nothing changed yet");

    let id = create(&workspace, SessionKind::Shell, &folders.make("app")).id;

    wait_until("the state file", || path.exists());
    let saved = |name: &str| std::fs::read_to_string(&path).is_ok_and(|text| text.contains(name));
    workspace.rename_session(&id, Some("first name")).unwrap();
    wait_until("the first name to be saved", || saved("first name"));
    workspace.rename_session(&id, Some("second name")).unwrap();
    wait_until("the second name to be saved", || saved("second name"));
}

#[test]
fn a_stream_of_changes_is_still_saved_within_the_longest_wait() {
    let folders = Folders::new();
    let mut config = quiet(Some(&folders.state_dir()));
    // The pause never comes: only the longest wait can trigger the save.
    config.save_debounce = NEVER;
    config.save_max_wait = Duration::from_millis(150);
    let workspace = Workspace::open(config);
    let path = workspace.state_file().unwrap();
    let id = create(&workspace, SessionKind::Shell, &folders.make("app")).id;

    // Creating the session was saved at once; titles set by a program
    // are what comes in streams.
    wait_until("the session to be saved", || path.exists());
    let saved = |title: &str| std::fs::read_to_string(&path).is_ok_and(|text| text.contains(title));
    let deadline = Instant::now() + TIMEOUT;
    let mut n = 0;
    while !saved("title ") {
        assert!(Instant::now() < deadline, "the titles were never saved");
        n += 1;
        workspace.report_title(&id, &format!("title {n}")).unwrap();
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn nothing_is_written_while_nothing_changes() {
    let folders = Folders::new();
    let workspace = Workspace::open(quiet(Some(&folders.state_dir())));
    let id = create(&workspace, SessionKind::Shell, &folders.make("app")).id;

    // Creating a session is saved at once; flushing makes sure of it.
    workspace.flush().unwrap();
    assert!(workspace.state_file().unwrap().exists());
    assert!(!workspace.flush().unwrap(), "nothing changed since");

    // Operations that change nothing do not cause a write...
    workspace.rename_session(&id, None).unwrap();
    workspace.focus_pane(workspace.layout().focused()).unwrap();
    workspace.reorder_session(&id, None).unwrap();
    assert!(!workspace.flush().unwrap());

    // ...and the state of the process is not part of the file.
    let launch = workspace.begin_launch(&id, LaunchMode::Fresh).unwrap();
    assert!(workspace.terminal_started(&id, launch.generation, 3));
    let path = workspace.state_file().unwrap();
    let before = std::fs::read(&path).unwrap();
    workspace.flush().unwrap();
    let json = |bytes: &[u8]| {
        let mut value: serde_json::Value = serde_json::from_slice(bytes).unwrap();
        // The time of the last activity moved with the launch.
        value["sessions"][0]["lastActivityAt"] = 0.into();
        value["savedAt"] = 0.into();
        value
    };
    assert_eq!(json(&std::fs::read(&path).unwrap()), json(&before));
}

#[test]
fn the_state_file_holds_no_output_and_no_process_state() {
    let folders = Folders::new();
    let workspace = Workspace::open(quiet(Some(&folders.state_dir())));
    let id = create(&workspace, SessionKind::Background, &folders.make("app")).id;
    let launch = workspace.begin_launch(&id, LaunchMode::Fresh).unwrap();
    assert!(workspace.terminal_started(&id, launch.generation, 3));
    workspace.flush().unwrap();

    let json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(workspace.state_file().unwrap()).unwrap()).unwrap();

    assert_eq!(json["version"], 1);
    let mut top: Vec<&str> = json
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    top.sort_unstable();
    assert_eq!(
        top,
        [
            "layout",
            "projectOrder",
            "recentFolders",
            "savedAt",
            "sessions",
            "version",
            "window"
        ]
    );
    let mut fields: Vec<&str> = json["sessions"][0]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    fields.sort_unstable();
    assert_eq!(
        fields,
        [
            "autoTitle",
            "claudeSessionId",
            "command",
            "createdAt",
            "cwd",
            "git",
            "id",
            "kind",
            "lastActivityAt",
            "name",
            "project",
        ]
    );
}

#[test]
fn a_second_instance_starts_clean_and_leaves_the_state_of_the_first_alone() {
    let folders = Folders::new();
    let app = folders.make("app");
    let other = folders.make("other");
    let first = Workspace::open(quiet(Some(&folders.state_dir())));
    let session = create(&first, SessionKind::Claude, &app);
    first.flush().unwrap();

    let second = Workspace::open(quiet(Some(&folders.state_dir())));

    assert_eq!(first.persistence(), PersistenceMode::Primary);
    assert_eq!(second.persistence(), PersistenceMode::Secondary);
    let snapshot = second.snapshot();
    // It does not restore the sessions the first instance is running...
    assert!(snapshot.sessions.is_empty());
    // ...but offers the same recent folders.
    assert_eq!(snapshot.recent_folders, std::slice::from_ref(&app));
    assert_eq!(
        serde_json::to_value(&snapshot.persistence).unwrap(),
        serde_json::json!({ "mode": "secondary" })
    );

    // It works, and nothing it does reaches the file.
    create(&second, SessionKind::Shell, &other);
    assert!(!second.flush().unwrap());
    second.shutdown();
    drop(second);
    first
        .rename_session(&session.id, Some("still mine"))
        .unwrap();
    first.shutdown();
    drop(first);

    let third = Workspace::open(quiet(Some(&folders.state_dir())));
    assert_eq!(third.persistence(), PersistenceMode::Primary);
    let sessions = third.snapshot().sessions;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, session.id);
    assert_eq!(sessions[0].title, "still mine");
    assert_eq!(third.recent_folders(), [app]);
}

#[test]
fn a_corrupt_state_file_never_stops_the_workspace_from_opening() {
    let folders = Folders::new();
    std::fs::create_dir_all(folders.state_dir()).unwrap();
    std::fs::write(
        folders.state_dir().join(STATE_FILE),
        b"{ \"version\": 1, \"sess",
    )
    .unwrap();

    let workspace = Workspace::open(quiet(Some(&folders.state_dir())));

    assert_eq!(workspace.persistence(), PersistenceMode::Primary);
    assert!(workspace.snapshot().sessions.is_empty());
    let backups: Vec<String> = std::fs::read_dir(folders.state_dir())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("state.corrupt-"))
        .collect();
    assert_eq!(backups.len(), 1);
    // And it works from there.
    let id = create(&workspace, SessionKind::Shell, &folders.make("app")).id;
    workspace.flush().unwrap();
    let saved = std::fs::read_to_string(workspace.state_file().unwrap()).unwrap();
    assert!(saved.contains(id.as_str()));
}

#[test]
fn intervals_that_never_end_do_not_stop_the_workspace_from_working() {
    let folders = Folders::new();
    let mut config = quiet(Some(&folders.state_dir()));
    config.activity_coalesce = Duration::MAX;
    config.save_debounce = Duration::MAX;
    config.save_max_wait = Duration::MAX;
    config.git_poll_interval = Duration::MAX;
    let workspace = Workspace::open(config);
    let session = create(
        &workspace,
        SessionKind::Shell,
        &folders.repository("app", "main"),
    );
    workspace.activity_handle(&session.id).unwrap().touch();

    // The thread of the workspace is alive: it still answers a request.
    set_branch(&session.cwd, "still-working");
    workspace.refresh_git();
    wait_until("the new branch", || {
        workspace
            .session(&session.id)
            .unwrap()
            .git
            .and_then(|git| git.branch)
            .as_deref()
            == Some("still-working")
    });
    workspace.flush().unwrap();
    let saved = std::fs::read_to_string(workspace.state_file().unwrap()).unwrap();
    assert!(saved.contains("still-working"));
}

// --- Change notifications --------------------------------------------------

#[test]
fn a_subscriber_learns_what_changed_and_reads_the_current_values() {
    let folders = Folders::new();
    let folder = folders.make("app");
    let workspace = Workspace::open(quiet(None));
    let subscription = workspace.subscribe();
    let snapshot = workspace.snapshot();

    let a = create(&workspace, SessionKind::Shell, &folder);
    let changes = subscription.try_recv().unwrap();
    let delta = workspace.delta(&changes);

    assert!(delta.revision > snapshot.revision);
    assert_eq!(delta.sessions, std::slice::from_ref(&a));
    assert!(delta.removed.is_empty());
    assert_eq!(delta.session_order, Some(vec![a.id.clone()]));
    assert_eq!(delta.projects, Some(vec![a.project.clone()]));
    assert_eq!(delta.recent_folders, Some(vec![folder.clone()]));
    assert_eq!(delta.layout, Some(workspace.layout()));

    // A change of one session says nothing about the rest.
    workspace.rename_session(&a.id, Some("renamed")).unwrap();
    let delta = workspace.delta(&subscription.try_recv().unwrap());
    assert_eq!(delta.sessions.len(), 1);
    assert_eq!(delta.sessions[0].title, "renamed");
    assert_eq!(
        (
            delta.session_order,
            delta.projects,
            delta.layout,
            delta.recent_folders
        ),
        (None, None, None, None)
    );

    workspace.close_session(&a.id).unwrap();
    let delta = workspace.delta(&subscription.try_recv().unwrap());
    assert!(delta.sessions.is_empty());
    assert_eq!(delta.removed, [a.id]);
    assert_eq!(delta.session_order, Some(vec![]));
    assert_eq!(delta.projects, Some(vec![]));
    assert!(delta.layout.is_some());
}

#[test]
fn a_burst_of_changes_reaches_a_subscriber_as_one_notification() {
    let folders = Folders::new();
    let folder = folders.make("app");
    let workspace = Workspace::open(quiet(None));
    let a = create(&workspace, SessionKind::Shell, &folder).id;
    let subscription = workspace.subscribe();

    let b = create(&workspace, SessionKind::Shell, &folder).id;
    for n in 0..200 {
        workspace
            .rename_session(&a, Some(&format!("name {n}")))
            .unwrap();
    }
    let focused = workspace.layout().focused();
    workspace
        .split_pane(focused, Direction::Row, Placement::After, None)
        .unwrap();
    let Node::Split { id: split, .. } = *workspace.layout().root() else {
        panic!("the root is not a split");
    };
    for n in 0..100 {
        workspace
            .resize_split(split, 0.2 + f64::from(n) / 200.0)
            .unwrap();
    }
    workspace.close_session(&b).unwrap();

    let changes = subscription
        .recv_coalesced(Duration::from_millis(20))
        .unwrap();
    assert_eq!(subscription.try_recv(), None, "one notification for it all");
    let delta = workspace.delta(&changes);
    // Only the last value of each thing is sent.
    assert_eq!(delta.sessions.len(), 1);
    assert_eq!(delta.sessions[0].title, "name 199");
    assert_eq!(delta.removed, [b]);
    assert_eq!(delta.layout, Some(workspace.layout()));
    assert_eq!(delta.revision, workspace.snapshot().revision);
}

#[test]
fn subscriptions_end_when_the_workspace_shuts_down() {
    let workspace = Workspace::open(quiet(None));
    let subscription = workspace.subscribe();
    let reader = std::thread::spawn({
        let subscription = subscription.clone();
        move || subscription.recv()
    });

    workspace.shutdown();

    assert_eq!(reader.join().unwrap(), None);
    assert!(subscription.is_closed());
    // Shutting down twice is harmless.
    workspace.shutdown();
}

// --- Activity --------------------------------------------------------------

#[test]
fn output_updates_the_last_activity_at_most_once_per_interval() {
    let folders = Folders::new();
    let mut config = quiet(None);
    config.activity_coalesce = Duration::from_millis(150);
    let workspace = Workspace::open(config);
    let session = create(&workspace, SessionKind::Shell, &folders.make("app"));
    let handle = workspace.activity_handle(&session.id).unwrap();
    let subscription = workspace.subscribe();
    std::thread::sleep(Duration::from_millis(5));

    // A program printing without a pause for a while.
    let started = Instant::now();
    let mut touches = 0u64;
    while started.elapsed() < Duration::from_millis(700) {
        handle.touch();
        touches += 1;
        std::hint::spin_loop();
    }
    wait_until("the activity to be passed on", || {
        workspace.session(&session.id).unwrap().last_activity_at > session.last_activity_at
    });

    let mut notifications = 0;
    while subscription
        .recv_timeout(Duration::from_millis(400))
        .is_ok()
    {
        notifications += 1;
    }
    assert!(touches > 1000, "only {touches} touches were made");
    assert!(
        (1..=8).contains(&notifications),
        "{touches} touches caused {notifications} notifications"
    );
    let after = workspace.session(&session.id).unwrap();
    assert!(after.last_activity_at >= session.last_activity_at);
    assert!(after.last_activity_at <= termx_core::workspace::now_ms());
}

#[test]
fn an_idle_session_causes_no_notifications() {
    let folders = Folders::new();
    let mut config = quiet(None);
    config.activity_coalesce = Duration::from_millis(10);
    config.git_poll_interval = Duration::from_millis(10);
    let workspace = Workspace::open(config);
    let session = create(
        &workspace,
        SessionKind::Shell,
        &folders.repository("app", "main"),
    );
    let subscription = workspace.subscribe();

    // Many polls of Git later, nothing was said: nothing changed.
    assert!(
        subscription
            .recv_timeout(Duration::from_millis(300))
            .is_err()
    );
    assert_eq!(workspace.session(&session.id), Some(session));
}

#[test]
fn activity_is_passed_on_before_saving_on_exit() {
    let folders = Folders::new();
    let workspace = Workspace::open(quiet(Some(&folders.state_dir())));
    let session = create(&workspace, SessionKind::Shell, &folders.make("app"));
    let handle = workspace.activity_handle(&session.id).unwrap();
    std::thread::sleep(Duration::from_millis(15));

    handle.touch();
    workspace.shutdown();
    drop(workspace);

    let restored = Workspace::open(quiet(Some(&folders.state_dir())));
    let after = restored.session(&session.id).unwrap();
    assert!(
        after.last_activity_at > session.last_activity_at,
        "{} was not after {}",
        after.last_activity_at,
        session.last_activity_at
    );
    let ghost = SessionKey::from_raw("s-ghost").unwrap();
    assert!(restored.activity_handle(&ghost).is_none());
}

// --- Git and folders -------------------------------------------------------

#[test]
fn a_change_of_branch_is_noticed_on_request() {
    let folders = Folders::new();
    let repository = folders.repository("app", "main");
    let workspace = Workspace::open(quiet(None));
    let id = create(&workspace, SessionKind::Claude, &repository).id;
    let other = create(&workspace, SessionKind::Shell, &folders.make("notes"));
    let branch = |workspace: &Workspace| {
        workspace
            .session(&id)
            .unwrap()
            .git
            .and_then(|git| git.branch)
    };
    // The first reading, asked for when the workspace opened, is over.
    std::thread::sleep(Duration::from_millis(100));
    let subscription = workspace.subscribe();

    set_branch(&repository, "feature/sidebar");
    workspace.refresh_git();

    wait_until("the new branch", || {
        branch(&workspace).as_deref() == Some("feature/sidebar")
    });
    let delta = workspace.delta(&subscription.recv_timeout(TIMEOUT).unwrap());
    assert_eq!(
        delta.sessions.len(),
        1,
        "only the session whose branch changed"
    );
    assert_eq!(delta.sessions[0].id, id);
    assert_eq!(workspace.session(&other.id), Some(other));
}

#[test]
fn a_change_of_branch_is_noticed_without_asking() {
    let folders = Folders::new();
    let repository = folders.repository("app", "main");
    let mut config = quiet(None);
    config.git_poll_interval = Duration::from_millis(30);
    let workspace = Workspace::open(config);
    let id = create(&workspace, SessionKind::Claude, &repository).id;

    set_branch(&repository, "hotfix");

    wait_until("the new branch", || {
        workspace
            .session(&id)
            .unwrap()
            .git
            .and_then(|git| git.branch)
            .as_deref()
            == Some("hotfix")
    });
}

#[test]
fn a_change_of_branch_is_noticed_soon_after_the_session_prints() {
    let folders = Folders::new();
    let repository = folders.repository("app", "main");
    let mut config = quiet(None);
    config.activity_coalesce = Duration::from_millis(30);
    let workspace = Workspace::open(config);
    let id = create(&workspace, SessionKind::Shell, &repository).id;
    let handle = workspace.activity_handle(&id).unwrap();
    // The first reading, asked for when the workspace opened, is over.
    std::thread::sleep(Duration::from_millis(100));

    // `git switch` ran in the session: it printed something.
    set_branch(&repository, "after-switch");
    handle.touch();

    wait_until("the new branch", || {
        workspace
            .session(&id)
            .unwrap()
            .git
            .and_then(|git| git.branch)
            .as_deref()
            == Some("after-switch")
    });
}

#[test]
fn a_folder_that_becomes_a_repository_becomes_a_git_project() {
    let folders = Folders::new();
    let folder = folders.make("new-project");
    let workspace = Workspace::open(quiet(None));
    let session = create(&workspace, SessionKind::Shell, &folder);
    assert_eq!(session.git, None);

    // `git init`.
    folders.repository("new-project", "main");
    workspace.refresh_git();

    wait_until("the repository to be found", || {
        workspace.session(&session.id).unwrap().git.is_some()
    });
    let after = workspace.session(&session.id).unwrap();
    assert_eq!(after.project.kind, ProjectKind::Git);
    // The key is the folder either way: the session stays in its group.
    assert_eq!(after.project.key, session.project.key);
}

#[test]
fn the_folder_a_program_reports_moves_the_session_to_its_project() {
    let folders = Folders::new();
    let home = folders.make("home");
    let repository = folders.repository("app", "develop");
    let workspace = Workspace::open(quiet(None));
    let id = create(&workspace, SessionKind::Shell, &home).id;
    let launch = workspace.begin_launch(&id, LaunchMode::Fresh).unwrap();
    assert!(workspace.terminal_started(&id, launch.generation, 1));

    let url = format!(
        "file://some-host{}{}",
        if cfg!(windows) { "/" } else { "" },
        repository
            .to_string_lossy()
            .replace('\\', "/")
            .replace(' ', "%20")
    );
    assert_eq!(workspace.report_cwd(&id, &url), Ok(true));

    let session = workspace.session(&id).unwrap();
    assert_eq!(session.cwd, home, "it still restarts where it was created");
    assert_eq!(session.live_cwd.as_deref(), Some(repository.as_path()));
    assert_eq!(session.project.name, "app");
    assert_eq!(session.git.unwrap().branch.as_deref(), Some("develop"));

    // A folder that is not here, or not a folder at all, is ignored.
    for bogus in [
        "file://elsewhere/no/such/folder",
        "relative",
        "",
        "file://x",
    ] {
        assert_eq!(workspace.report_cwd(&id, bogus), Ok(false), "{bogus:?}");
    }
    assert_eq!(
        workspace.session(&id).unwrap().live_cwd.as_deref(),
        Some(repository.as_path())
    );

    // When the process ends, the session is back in its own folder.
    workspace.terminal_exited(&id, launch.generation, &ExitStatus::from_code(0));
    assert_eq!(workspace.session(&id).unwrap().live_cwd, None);
    assert_eq!(workspace.report_cwd(&id, &url), Ok(false), "nothing runs");
    workspace.refresh_git();
    wait_until("the project of the launch folder", || {
        workspace.session(&id).unwrap().project.name == "home"
    });

    let ghost = SessionKey::from_raw("s-ghost").unwrap();
    assert_eq!(
        workspace.report_cwd(&ghost, &url),
        Err(WorkspaceError::UnknownSession(ghost))
    );
}

// --- Layout ----------------------------------------------------------------

#[test]
fn layout_operations_go_through_the_workspace_and_are_validated() {
    let folders = Folders::new();
    let folder = folders.make("app");
    let workspace = Workspace::open(quiet(None));
    let a = create(&workspace, SessionKind::Shell, &folder).id;
    let b = create(&workspace, SessionKind::Shell, &folder).id;
    let first = workspace.layout().focused();
    // The newest session took the focused pane.
    assert_eq!(workspace.layout().session_of(first), Ok(Some(&b)));

    let second = workspace
        .split_pane(first, Direction::Row, Placement::After, Some(a.clone()))
        .unwrap();
    assert_eq!(workspace.layout().focused(), second);
    workspace.focus_pane(first).unwrap();
    workspace.show_session(first, Some(a.clone())).unwrap();
    assert_eq!(workspace.layout().pane_of(&a), Some(first));
    assert_eq!(workspace.layout().session_of(second), Ok(None));
    workspace
        .move_pane(second, first, termx_core::workspace::Edge::Top)
        .unwrap();
    workspace.close_pane(second).unwrap();
    assert_eq!(workspace.layout().pane_count(), 1);
    workspace.layout().check_invariants().unwrap();

    let ghost = SessionKey::from_raw("s-ghost").unwrap();
    let before = workspace.layout();
    assert_eq!(
        workspace.show_session(first, Some(ghost.clone())),
        Err(WorkspaceError::UnknownSession(ghost.clone()))
    );
    assert_eq!(
        workspace.split_pane(first, Direction::Row, Placement::After, Some(ghost.clone())),
        Err(WorkspaceError::UnknownSession(ghost))
    );
    assert!(matches!(
        workspace.close_pane(second),
        Err(WorkspaceError::Layout(_))
    ));
    assert_eq!(workspace.layout(), before);
}

#[test]
fn a_session_can_be_stopped_and_then_removed() {
    let folders = Folders::new();
    let workspace = Workspace::open(quiet(None));
    let id = create(&workspace, SessionKind::Background, &folders.make("app")).id;
    let launch = workspace.begin_launch(&id, LaunchMode::Fresh).unwrap();
    assert!(workspace.terminal_started(&id, launch.generation, 9));

    // A running session is not removed by mistake.
    assert_eq!(
        workspace.remove_session(&id),
        Err(WorkspaceError::SessionRunning(id.clone()))
    );
    assert!(workspace.session(&id).is_some());

    // Stopping keeps it, with nothing running.
    assert_eq!(workspace.stop_session(&id), Ok(Some(9)));
    let stopped = workspace.session(&id).unwrap();
    assert_eq!(stopped.state, SessionState::Dormant);
    assert_eq!(stopped.terminal_id, None);
    assert_eq!(workspace.stop_session(&id), Ok(None), "already stopped");
    // What the old terminal says afterwards is ignored.
    workspace.terminal_exited(&id, launch.generation, &ExitStatus::from_code(1));
    assert_eq!(workspace.session(&id).unwrap().state, SessionState::Dormant);

    // It can be started again, and removed once stopped.
    let again = workspace.begin_launch(&id, LaunchMode::Resume).unwrap();
    assert!(workspace.terminal_started(&id, again.generation, 10));
    assert_eq!(workspace.stop_session(&id), Ok(Some(10)));
    assert_eq!(workspace.remove_session(&id), Ok(()));
    assert!(workspace.session(&id).is_none());
    assert_eq!(
        workspace.stop_session(&id),
        Err(WorkspaceError::UnknownSession(id.clone()))
    );
}
