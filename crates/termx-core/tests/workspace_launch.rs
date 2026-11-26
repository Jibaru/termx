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
use termx_core::workspace::launch::{CLAUDE_NOT_FOUND_EXIT_CODE, LaunchRequest, Shell};
use termx_core::workspace::{
    LaunchFailure, LaunchMode, Launcher, NewSession, SessionKey, SessionKind, SessionState,
    Workspace, WorkspaceConfig,
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
            // An exit code of the program itself is not a failure of termx.
            assert_eq!(launcher.failure_for_exit(SessionKind::Claude, &exit), None);
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

#[test]
fn a_claude_that_is_not_installed_is_a_typed_failure_not_a_dead_terminal() {
    guarded(|| {
        let setup = Setup::new();
        for shell in shells() {
            let launcher = Launcher::new(shell.clone()).claude_program(Setup::MISSING);

            let command = launcher
                .command(&setup.request(SessionKind::Claude))
                .unwrap();
            let (output, exit) = run(setup.isolate(command));

            assert_eq!(
                exit.code(),
                Some(CLAUDE_NOT_FOUND_EXIT_CODE),
                "{shell:?} printed {output:?}"
            );
            let failure = launcher
                .failure_for_exit(SessionKind::Claude, &exit)
                .expect("the exit was not recognised");
            assert!(matches!(failure, LaunchFailure::ClaudeNotFound { .. }));
            assert!(failure.message().contains(Setup::MISSING));
        }
    });
}

#[test]
fn a_session_whose_claude_is_missing_ends_exited_with_the_failure() {
    guarded(|| {
        let setup = Setup::new();
        let shell = shells().remove(0);
        let mut config = WorkspaceConfig::in_memory();
        config.launcher = Launcher::new(shell).claude_program(Setup::MISSING);
        let workspace = Arc::new(Workspace::open(config));
        let manager = SessionManager::new().unwrap();
        let session = workspace
            .create_session(NewSession::new(SessionKind::Claude, setup.project()))
            .unwrap();
        assert_eq!(session.state, SessionState::Starting);

        // What the application does to start a session.
        let launch = workspace
            .begin_launch(&session.id, LaunchMode::Fresh)
            .unwrap();
        let recorder = Recorder {
            report_to: Some((
                Arc::clone(&workspace),
                session.id.clone(),
                launch.generation,
            )),
            ..Recorder::default()
        };
        let terminal = manager
            .create(
                &SessionOptions::new(setup.isolate(launch.command)),
                recorder.clone(),
            )
            .unwrap();
        assert!(workspace.terminal_started(&session.id, launch.generation, terminal.as_u64()));
        recorder.wait_for_exit();

        let after = workspace.session(&session.id).unwrap();
        let SessionState::Exited {
            code,
            failure: Some(failure),
            ..
        } = after.state
        else {
            panic!("the session did not end with a failure: {:?}", after.state);
        };
        assert_eq!(code, Some(CLAUDE_NOT_FOUND_EXIT_CODE));
        assert!(matches!(failure, LaunchFailure::ClaudeNotFound { .. }));
        assert_eq!(after.terminal_id, None);
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
            assert_eq!(
                launcher.failure_for_exit(SessionKind::Background, &exit),
                None
            );
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
