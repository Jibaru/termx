//! Launching each kind of session for real: through a real shell, in a
//! real pseudo-terminal.
//!
//! Claude Code itself is never run. A stand-in program with a name nobody
//! has installed plays its part, so that the tests neither depend on Claude
//! Code being installed nor start it when it is.
//!
//! On Unix the shell is given a home directory made for the test, whose
//! profile is the only thing that puts the stand-in on `PATH`: exactly the
//! situation of a desktop application, which does not have the `PATH` of
//! the user's terminal. On Windows there is no such profile mechanism to
//! rely on, and the stand-in is put on the `PATH` of the session.

// The helpers below are test code too: a failed `unwrap` is a failed test.
#![allow(clippy::unwrap_used)]

mod common;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Instant;

use common::{TIMEOUT, guarded, visible_text};
use termx_core::pty::{ExitStatus, PtyCommand};
use termx_core::session::{SessionId, SessionManager, SessionOptions, SessionSink};
use termx_core::workspace::launch::{CLAUDE_MISSING_EXIT_CODE, LaunchRequest, Shell};
use termx_core::workspace::{
    LaunchFailure, LaunchMode, Launcher, NewSession, Session, SessionKey, SessionKind,
    SessionState, Workspace, WorkspaceConfig,
};

/// What a session printed and how it ended.
#[derive(Default)]
struct Seen {
    output: Vec<u8>,
    exit: Option<ExitStatus>,
}

/// A sink that records, and tells a workspace about the exit when it was
/// given one, as the application does.
#[derive(Clone, Default)]
struct Recorder {
    seen: Arc<(Mutex<Seen>, Condvar)>,
    report_to: Option<(Arc<Workspace>, SessionKey, u64)>,
}

impl Recorder {
    fn wait_for_exit(&self) -> (String, ExitStatus) {
        let deadline = Instant::now() + TIMEOUT;
        let mut seen = self.seen.0.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(exit) = seen.exit.clone() {
                return (visible_text(&seen.output), exit);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero(),
                "the session did not end; it printed: {:?}",
                visible_text(&seen.output)
            );
            seen = self
                .seen
                .1
                .wait_timeout(seen, remaining)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    fn wait_for_output(&self, needle: &str) {
        let deadline = Instant::now() + TIMEOUT;
        let mut seen = self.seen.0.lock().unwrap_or_else(PoisonError::into_inner);
        while !visible_text(&seen.output).contains(needle) {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero() && seen.exit.is_none(),
                "{needle:?} was not printed; exit: {:?}; printed: {:?}",
                seen.exit,
                visible_text(&seen.output)
            );
            seen = self
                .seen
                .1
                .wait_timeout(seen, remaining)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

impl SessionSink for Recorder {
    fn output(&mut self, _session: SessionId, bytes: Vec<u8>) {
        let mut seen = self.seen.0.lock().unwrap_or_else(PoisonError::into_inner);
        seen.output.extend_from_slice(&bytes);
        self.seen.1.notify_all();
    }

    fn exited(&mut self, _session: SessionId, status: ExitStatus) {
        if let Some((workspace, key, generation)) = &self.report_to {
            workspace.terminal_exited(key, *generation, &status);
        }
        let mut seen = self.seen.0.lock().unwrap_or_else(PoisonError::into_inner);
        seen.exit = Some(status);
        self.seen.1.notify_all();
    }
}

/// Where the tests run their sessions: a folder to work in and, apart from
/// it, what stands in for the user's setup.
struct Setup {
    dir: tempfile::TempDir,
}

impl Setup {
    fn new() -> Self {
        let setup = Self {
            dir: tempfile::tempdir().unwrap(),
        };
        for folder in [setup.home(), setup.bin(), setup.project()] {
            std::fs::create_dir_all(folder).unwrap();
        }
        if cfg!(unix) {
            // Read by the shells when they start as a login shell, which is
            // how sessions start them. The interactive files exist so that
            // no shell offers to create them.
            let profile = format!(
                "export PATH=\"{}:$PATH\"\nexport TERMX_FROM_PROFILE=yes\nalias {}='{} via-alias'\n",
                setup.bin().display(),
                Self::ALIAS,
                setup.bin().join(Self::STAND_IN).display(),
            );
            for file in [".profile", ".bash_profile", ".zprofile"] {
                std::fs::write(setup.home().join(file), &profile).unwrap();
            }
            for file in [".zshrc", ".bashrc"] {
                std::fs::write(setup.home().join(file), "").unwrap();
            }
        }
        setup
    }

    /// The name of the program that stands in for `claude`.
    const STAND_IN: &str = "termx-test-stand-in-for-claude";
    /// An alias for it, which only the shell can resolve.
    const ALIAS: &str = "termx-test-alias-for-claude";
    /// A name that nothing answers to.
    const MISSING: &str = "termx-test-claude-that-is-not-installed";

    fn home(&self) -> PathBuf {
        self.dir.path().join("home")
    }

    fn bin(&self) -> PathBuf {
        self.dir.path().join("bin")
    }

    fn project(&self) -> PathBuf {
        self.dir.path().join("project")
    }

    /// Installs the stand-in: it prints its arguments and where it runs,
    /// and exits with code 7.
    fn install_stand_in(&self) {
        if cfg!(windows) {
            std::fs::write(
                self.bin().join(format!("{}.cmd", Self::STAND_IN)),
                "@echo off\r\necho stand-in-ran:%*\r\nexit /b 7\r\n",
            )
            .unwrap();
        } else {
            let path = self.bin().join(Self::STAND_IN);
            std::fs::write(
                &path,
                "#!/bin/sh\necho \"stand-in-ran:$*\"\necho \"cwd=$(pwd -P)\"\nexit 7\n",
            )
            .unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
    }

    /// Gives a command the user's setup of the test instead of that of
    /// whoever runs the tests.
    fn isolate(&self, command: PtyCommand) -> PtyCommand {
        if cfg!(windows) {
            let mut path = std::ffi::OsString::from(self.bin());
            path.push(";");
            path.push(std::env::var_os("PATH").unwrap_or_default());
            command.env("PATH", path)
        } else {
            command
                .env("HOME", self.home())
                .env_remove("ZDOTDIR")
                .env_remove("ENV")
                .env_remove("BASH_ENV")
        }
    }

    fn request(&self, kind: SessionKind) -> LaunchRequest<'_> {
        // Leaked so that the request can borrow it: a test runs once.
        let cwd: &'static Path = Box::leak(self.project().into_boxed_path());
        LaunchRequest {
            kind,
            cwd,
            command: None,
            claude_session_id: None,
            mode: LaunchMode::Fresh,
            token: None,
        }
    }
}

/// The shells to test with: every common one the machine has.
fn shells() -> Vec<Shell> {
    if cfg!(windows) {
        return vec![Shell::of_user()];
    }
    ["/bin/sh", "/bin/bash", "/bin/zsh"]
        .into_iter()
        .filter(|shell| Path::new(shell).is_file())
        .map(Shell::new)
        .collect()
}

/// Runs a command in a terminal until it ends.
fn run(command: PtyCommand) -> (String, ExitStatus) {
    let manager = SessionManager::new().unwrap();
    let recorder = Recorder::default();
    manager
        .create(&SessionOptions::new(command), recorder.clone())
        .unwrap();
    recorder.wait_for_exit()
}

#[test]
fn claude_is_found_through_the_profile_of_the_shell() {
    guarded(|| {
        let setup = Setup::new();
        setup.install_stand_in();
        assert!(!shells().is_empty());
        for shell in shells() {
            let launcher = Launcher::new(shell.clone()).claude_program(Setup::STAND_IN);

            let command = launcher
                .command(&setup.request(SessionKind::Claude))
                .unwrap();
            let (output, exit) = run(setup.isolate(command));

            assert!(
                output.contains("stand-in-ran:"),
                "{shell:?} printed {output:?}"
            );
            assert_eq!(exit.code(), Some(7), "{shell:?} printed {output:?}");
            if cfg!(unix) {
                let project = std::fs::canonicalize(setup.project()).unwrap();
                assert!(
                    output.contains(&format!("cwd={}", project.display())),
                    "{shell:?} printed {output:?}"
                );
            }
        }
    });
}

#[test]
fn resuming_passes_the_conversation_or_asks_for_the_latest() {
    guarded(|| {
        let setup = Setup::new();
        setup.install_stand_in();
        for shell in shells() {
            let launcher = Launcher::new(shell.clone()).claude_program(Setup::STAND_IN);
            let mut request = setup.request(SessionKind::Claude);
            request.mode = LaunchMode::Resume;

            let (output, exit) = run(setup.isolate(launcher.command(&request).unwrap()));
            assert!(
                output.contains("stand-in-ran:--continue"),
                "{shell:?} printed {output:?}"
            );
            assert_eq!(exit.code(), Some(7));

            request.claude_session_id = Some("0b9d6f7e-3c1a-4f3e-9a55-2f6f1f0f8a10");
            let (output, _) = run(setup.isolate(launcher.command(&request).unwrap()));
            assert!(
                output.contains("stand-in-ran:--resume 0b9d6f7e-3c1a-4f3e-9a55-2f6f1f0f8a10"),
                "{shell:?} printed {output:?}"
            );
        }
    });
}

#[cfg(unix)]
#[test]
fn a_claude_that_is_an_alias_is_run_by_the_shell() {
    guarded(|| {
        let setup = Setup::new();
        setup.install_stand_in();
        for shell in shells() {
            let launcher = Launcher::new(shell.clone()).claude_program(Setup::ALIAS);

            let command = launcher
                .command(&setup.request(SessionKind::Claude))
                .unwrap();
            let (output, exit) = run(setup.isolate(command));

            assert!(
                output.contains("stand-in-ran:via-alias"),
                "{shell:?} printed {output:?}"
            );
            assert_eq!(exit.code(), Some(7), "{shell:?} printed {output:?}");
        }
    });
}

/// A workspace whose sessions run `claude_program` through `shell`.
fn workspace_for(shell: Shell, claude_program: &str) -> Arc<Workspace> {
    let mut config = WorkspaceConfig::in_memory();
    config.launcher = Launcher::new(shell).claude_program(claude_program);
    Arc::new(Workspace::open(config))
}

/// Starts a session of `kind` as the application does: through the
/// workspace, with its output filtered by it. Returns the manager, the
/// terminal and what records the output.
fn start(
    workspace: &Arc<Workspace>,
    setup: &Setup,
    kind: SessionKind,
    command: Option<&str>,
) -> (SessionManager, SessionId, Recorder, SessionKey) {
    let mut new = NewSession::new(kind, setup.project());
    new.command = command.map(str::to_owned);
    let session = workspace.create_session(new).unwrap();
    let launch = workspace
        .begin_launch(&session.id, LaunchMode::Fresh)
        .unwrap();
    let recorder = Recorder {
        report_to: Some((Arc::clone(workspace), session.id.clone(), launch.generation)),
        ..Recorder::default()
    };
    let manager = SessionManager::new().unwrap();
    let filter = workspace.output_filter(&session.id, &launch);
    let terminal = manager
        .create_filtered(
            &SessionOptions::new(setup.isolate(launch.command)),
            filter,
            recorder.clone(),
        )
        .unwrap();
    assert!(workspace.terminal_started(&session.id, launch.generation, terminal.as_u64()));
    (manager, terminal, recorder, session.id)
}

/// Runs a session until it ends, as the application does. Returns what it
/// printed, how its process ended and the session afterwards.
fn run_session(
    workspace: &Arc<Workspace>,
    setup: &Setup,
    kind: SessionKind,
    command: Option<&str>,
) -> (String, ExitStatus, Session) {
    let (_manager, _terminal, recorder, key) = start(workspace, setup, kind, command);
    let (output, exit) = recorder.wait_for_exit();
    (output, exit, workspace.session(&key).unwrap())
}

/// The failure an ended session reports.
fn failure_of(session: &Session) -> Option<LaunchFailure> {
    match &session.state {
        SessionState::Exited { failure, .. } => failure.clone(),
        other => panic!("the session did not end: {other:?}"),
    }
}

#[test]
fn a_claude_that_is_not_installed_is_a_typed_failure_not_a_dead_terminal() {
    guarded(|| {
        let setup = Setup::new();
        for shell in shells() {
            let workspace = workspace_for(shell.clone(), Setup::MISSING);

            let (output, exit, session) =
                run_session(&workspace, &setup, SessionKind::Claude, None);

            assert_eq!(
                exit.code(),
                Some(CLAUDE_MISSING_EXIT_CODE),
                "{shell:?} printed {output:?}"
            );
            let failure = failure_of(&session).expect("the failure was not recognised");
            assert!(matches!(failure, LaunchFailure::ClaudeNotFound { .. }));
            assert!(failure.message().contains(Setup::MISSING));
            assert_eq!(session.terminal_id, None);
            // The markers never reach the terminal.
            assert!(!output.contains("7703"), "{shell:?} printed {output:?}");
        }
    });
}

#[test]
fn a_program_that_exits_like_a_missing_claude_is_not_taken_for_one() {
    guarded(|| {
        let setup = Setup::new();
        setup.install_stand_in();
        for shell in shells() {
            let workspace = workspace_for(shell.clone(), Setup::STAND_IN);

            let (output, exit, session) =
                run_session(&workspace, &setup, SessionKind::Claude, None);

            assert!(
                output.contains("stand-in-ran:"),
                "{shell:?} printed {output:?}"
            );
            assert!(!output.contains("7703"), "{shell:?} printed {output:?}");
            assert_eq!(exit.code(), Some(7), "{shell:?} printed {output:?}");
            // The program ran and chose its exit code: not a failure.
            assert_eq!(failure_of(&session), None, "{shell:?} printed {output:?}");
        }

        // The code that used to mean "not installed", from the program.
        let name = "termx-test-claude-that-exits-113";
        if cfg!(windows) {
            std::fs::write(
                setup.bin().join(format!("{name}.cmd")),
                "@echo off\r\necho ran-and-exited\r\nexit /b 113\r\n",
            )
            .unwrap();
        } else {
            let path = setup.bin().join(name);
            std::fs::write(&path, "#!/bin/sh\necho ran-and-exited\nexit 113\n").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        for shell in shells() {
            let workspace = workspace_for(shell.clone(), name);

            let (output, exit, session) =
                run_session(&workspace, &setup, SessionKind::Claude, None);

            assert!(
                output.contains("ran-and-exited"),
                "{shell:?} printed {output:?}"
            );
            assert_eq!(exit.code(), Some(113), "{shell:?}");
            assert_eq!(failure_of(&session), None, "{shell:?}");
        }
    });
}

#[cfg(unix)]
#[test]
fn a_profile_that_replaces_the_shell_is_reported() {
    guarded(|| {
        if !Path::new("/bin/zsh").is_file() || !Path::new("/bin/bash").is_file() {
            return;
        }
        let setup = Setup::new();
        setup.install_stand_in();
        // The command never runs: zsh is replaced by a bare bash.
        std::fs::write(setup.home().join(".zshrc"), "exec /bin/bash --norc -i\n").unwrap();
        let mut config = WorkspaceConfig::in_memory();
        config.launcher = Launcher::new(Shell::new("/bin/zsh")).claude_program(Setup::STAND_IN);
        config.launch_confirm_timeout = std::time::Duration::from_millis(500);
        let workspace = Arc::new(Workspace::open(config));

        let (manager, terminal, recorder, key) =
            start(&workspace, &setup, SessionKind::Claude, None);

        let deadline = Instant::now() + TIMEOUT;
        loop {
            let session = workspace.session(&key).unwrap();
            if let Some(warning) = session.warning {
                assert!(matches!(warning, LaunchFailure::LaunchUnconfirmed { .. }));
                assert_eq!(session.state, SessionState::Running);
                break;
            }
            assert!(Instant::now() < deadline, "no warning: {session:?}");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        manager.write(terminal, b"exit 0\r").unwrap();
        let (output, _) = recorder.wait_for_exit();
        assert!(!output.contains("stand-in-ran"), "{output:?}");
        let session = workspace.session(&key).unwrap();
        assert!(
            matches!(
                failure_of(&session),
                Some(LaunchFailure::LaunchUnconfirmed { .. })
            ),
            "{session:?}"
        );
        assert_eq!(session.warning, None);
    });
}

#[test]
fn a_session_that_starts_its_command_is_confirmed() {
    guarded(|| {
        let setup = Setup::new();
        let shell = shells().remove(0);
        let mut config = WorkspaceConfig::in_memory();
        config.launcher = Launcher::new(shell);
        config.launch_confirm_timeout = std::time::Duration::from_millis(200);
        let workspace = Arc::new(Workspace::open(config));
        let line = if cfg!(windows) {
            "Write-Output 'ready'; Start-Sleep 2"
        } else {
            "echo ready; sleep 2"
        };

        let (_manager, _terminal, recorder, key) =
            start(&workspace, &setup, SessionKind::Background, Some(line));
        recorder.wait_for_output("ready");
        // Well past the timeout, the session still has no warning.
        std::thread::sleep(std::time::Duration::from_millis(600));
        assert_eq!(workspace.session(&key).unwrap().warning, None);

        let (output, _) = recorder.wait_for_exit();
        assert!(!output.contains("7703"), "{output:?}");
        assert_eq!(failure_of(&workspace.session(&key).unwrap()), None);
    });
}

#[test]
fn a_background_command_runs_with_the_environment_of_the_shell() {
    guarded(|| {
        let setup = Setup::new();
        for shell in shells() {
            let launcher = Launcher::new(shell.clone());
            let mut request = setup.request(SessionKind::Background);
            // Quotes, a variable and two commands: the line is for the
            // shell to interpret, not for termx.
            request.command = Some(if cfg!(windows) {
                "Write-Output \"bg:$('o' + 'k') done\"; cmd /c \"exit 5\""
            } else {
                "echo \"bg:$TERMX_FROM_PROFILE done\"; (exit 5)"
            });

            let (output, exit) = run(setup.isolate(launcher.command(&request).unwrap()));

            let expected = if cfg!(windows) {
                "bg:ok done"
            } else {
                "bg:yes done"
            };
            assert!(output.contains(expected), "{shell:?} printed {output:?}");
            assert_eq!(exit.code(), Some(5), "{shell:?} printed {output:?}");
        }
    });
}

#[test]
fn a_shell_session_is_an_interactive_shell_in_the_folder() {
    guarded(|| {
        let setup = Setup::new();
        let shell = shells().remove(0);
        let launcher = Launcher::new(shell.clone());
        let command = launcher
            .command(&setup.request(SessionKind::Shell))
            .unwrap();
        let manager = SessionManager::new().unwrap();
        let recorder = Recorder::default();
        let id = manager
            .create(
                &SessionOptions::new(setup.isolate(command)),
                recorder.clone(),
            )
            .unwrap();

        // It waits for commands, and runs them.
        let marker = if cfg!(windows) {
            "Write-Output ('shell-' + 'ready')\r"
        } else {
            "echo shell-\"ready\"\r"
        };
        manager.write(id, marker.as_bytes()).unwrap();
        recorder.wait_for_output("shell-ready");
        manager.write(id, b"exit 4\r").unwrap();

        let (_, exit) = recorder.wait_for_exit();
        assert_eq!(exit.code(), Some(4), "{shell:?}");
    });
}

/// The identifiers of the running processes whose command line is exactly
/// `sleep <seconds>`, found with `pgrep`. Processes that have exited and
/// wait to be reaped do not count.
#[cfg(unix)]
fn sleeping(seconds: u64) -> Vec<u32> {
    let output = std::process::Command::new("pgrep")
        .args(["-f", &format!("^sleep {seconds}$")])
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().parse().ok())
        .filter(|pid: &u32| {
            let state = std::process::Command::new("ps")
                .args(["-o", "stat=", "-p", &pid.to_string()])
                .output()
                .unwrap();
            let state = String::from_utf8_lossy(&state.stdout);
            !state.trim().is_empty() && !state.trim_start().starts_with('Z')
        })
        .collect()
}

/// Background commands that leave processes behind when only the shell is
/// signalled: a program that ignores `SIGHUP` in the foreground, in a
/// subshell, behind `nohup`, in the shell itself, and a job in a process
/// group of its own.
#[cfg(unix)]
fn stubborn_commands(base: u64) -> Vec<(u64, String)> {
    let line = |offset: u64, pattern: &str| {
        let seconds = base + offset;
        (seconds, pattern.replace("N", &seconds.to_string()))
    };
    vec![
        line(1, "sh -c 'trap \"\" HUP; sleep N'; echo done"),
        line(2, "(trap '' HUP; sleep N) & wait"),
        line(3, "nohup sleep N >/dev/null 2>&1 & wait"),
        line(4, "trap '' HUP TERM; sleep N; echo x"),
        line(5, "sleep N & sleep N"),
        line(6, "sleep N | cat"),
    ]
}

/// Starts the stubborn commands through `shell`, waits until each one has
/// its `sleep` running, ends them with `end`, and checks that nothing is
/// left.
#[cfg(unix)]
fn nothing_outlives(shell: &Shell, base: u64, end: impl FnOnce(&SessionManager, &[SessionId])) {
    use std::time::Duration;

    let setup = Setup::new();
    let launcher = Launcher::new(shell.clone());
    let manager = SessionManager::new().unwrap();
    let commands = stubborn_commands(base);
    let mut ids = Vec::new();
    for (_, line) in &commands {
        let mut request = setup.request(SessionKind::Background);
        request.command = Some(line);
        let command = setup
            .isolate(launcher.command(&request).unwrap())
            .kill_grace(Duration::from_millis(500));
        ids.push(
            manager
                .create(&SessionOptions::new(command), Recorder::default())
                .unwrap(),
        );
    }
    let deadline = Instant::now() + TIMEOUT;
    for (seconds, line) in &commands {
        while sleeping(*seconds).is_empty() {
            assert!(Instant::now() < deadline, "{line:?} never started");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    let started = Instant::now();
    end(&manager, &ids);
    let took = started.elapsed();

    // What was killed may still need a moment to be reaped by the system.
    let deadline = Instant::now() + Duration::from_secs(2);
    for (seconds, line) in &commands {
        while !sleeping(*seconds).is_empty() {
            assert!(
                Instant::now() < deadline,
                "{shell:?}: {line:?} left {:?} running",
                sleeping(*seconds)
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    assert!(took < Duration::from_secs(10), "ending took {took:?}");
}

/// The shells whose job control and signal handling differ the most.
#[cfg(unix)]
fn job_control_shells() -> Vec<Shell> {
    ["/bin/zsh", "/bin/bash", "/bin/sh"]
        .into_iter()
        .filter(|shell| Path::new(shell).is_file())
        .map(Shell::new)
        .collect()
}

#[cfg(unix)]
#[test]
fn closing_a_session_ends_everything_it_started() {
    guarded(|| {
        for (index, shell) in job_control_shells().iter().enumerate() {
            let base = 9_871_000 + 100 * index as u64;
            nothing_outlives(shell, base, |manager, ids| {
                assert_eq!(manager.close_all(ids), ids.len());
            });
        }
    });
}

#[cfg(unix)]
#[test]
fn shutting_down_ends_everything_the_sessions_started() {
    guarded(|| {
        for (index, shell) in job_control_shells().iter().enumerate() {
            let base = 9_872_000 + 100 * index as u64;
            nothing_outlives(shell, base, |manager, _| manager.shutdown());
        }
    });
}

/// Writes an executable shell script at `path`.
#[cfg(unix)]
fn write_script(path: &Path, text: &str) {
    use std::os::unix::fs::PermissionsExt;

    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(unix)]
#[test]
fn a_shell_termx_does_not_know_lends_its_path_to_sh() {
    guarded(|| {
        let setup = Setup::new();
        setup.install_stand_in();
        // A shell of another syntax: it only puts the stand-in on its own
        // `PATH`, and understands `-c`.
        let elsewhere = setup.dir.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::rename(
            setup.bin().join(Setup::STAND_IN),
            elsewhere.join(Setup::STAND_IN),
        )
        .unwrap();
        let shell = setup.dir.path().join("termx-test-unknown-shell");
        write_script(
            &shell,
            &format!(
                "#!/bin/sh\necho 'welcome to an odd shell'\nPATH=\"{}:$PATH\"; export PATH\nexec /bin/sh \"$@\"\n",
                elsewhere.display()
            ),
        );
        let workspace = workspace_for(Shell::new(&shell), Setup::STAND_IN);

        let (output, exit, session) = run_session(&workspace, &setup, SessionKind::Claude, None);

        assert!(output.contains("stand-in-ran:"), "printed {output:?}");
        assert_eq!(exit.code(), Some(7));
        assert_eq!(failure_of(&session), None);

        // One that cannot tell: `/bin/sh` reads the login files, which put
        // `bin` on `PATH` in these tests.
        setup.install_stand_in();
        let mute = setup.dir.path().join("termx-test-mute-shell");
        write_script(&mute, "#!/bin/sh\nexit 1\n");
        let workspace = workspace_for(Shell::new(&mute), Setup::STAND_IN);

        let (output, exit, session) = run_session(&workspace, &setup, SessionKind::Claude, None);

        assert!(output.contains("stand-in-ran:"), "printed {output:?}");
        assert_eq!(exit.code(), Some(7));
        assert_eq!(failure_of(&session), None);
    });
}
