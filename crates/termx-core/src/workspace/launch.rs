//! What to run for each kind of session, and how.
//!
//! # Through the user's shell
//!
//! A desktop application started from the Finder, the Dock or a launcher
//! does not have the environment of a terminal: its `PATH` is the minimal
//! one of the system, without what the user's shell profile adds (Homebrew,
//! `~/.local/bin`, a Node version manager). `claude` and most development
//! tools are only found through those additions.
//!
//! So Claude Code and background commands are not started directly: the
//! user's shell starts them, after reading its profile exactly as it does
//! for a terminal.
//!
//! | Shell                          | Started as                                  |
//! | ------------------------------ | ------------------------------------------- |
//! | sh, bash, zsh, dash, ksh, fish | `<shell> -l -i -c <script>`                 |
//! | PowerShell (Windows default)   | `<shell> -NoLogo -EncodedCommand <script>`  |
//! | any other                      | `<shell> -c <script>`                       |
//!
//! `-l` makes the shell read its login files and `-i` its interactive ones,
//! which is where a `PATH` is set up, and where aliases and functions are
//! defined. PowerShell reads its profile unless told otherwise, and on
//! Windows the `PATH` of a desktop application is already the user's. The
//! script is passed to PowerShell encoded (UTF-16, Base64), which spares it
//! every quoting rule of the Windows command line.
//!
//! A [`SessionKind::Shell`] session is the login shell itself, with no
//! script.
//!
//! # When `claude` is not installed
//!
//! The script checks that the shell can find `claude` (as a program, an
//! alias or a function) before running it, and exits with
//! [`CLAUDE_NOT_FOUND_EXIT_CODE`] if it cannot. [`failure_for_exit`] turns
//! that exit into [`LaunchFailure::ClaudeNotFound`], which a frontend shows
//! instead of a dead terminal. For a shell of the "any other" family the
//! check is not made: the session ends with whatever that shell reports.
//!
//! # Privacy
//!
//! Nothing here logs. Command lines and arguments must never be logged:
//! they can hold tokens.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::model::{SessionKind, is_valid_claude_session_id};
use crate::pty::{ExitStatus, PtyCommand, default_shell};

/// The program a [`SessionKind::Claude`] session runs.
pub const CLAUDE_PROGRAM: &str = "claude";

/// The exit code of the launch script when the shell cannot find `claude`.
///
/// Shells use 127 for a command that was not found, but so does `claude`
/// itself when the interpreter it needs is missing. A code of its own tells
/// the two apart.
pub const CLAUDE_NOT_FOUND_EXIT_CODE: u32 = 113;

/// How a session is started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LaunchMode {
    /// From scratch. Claude Code starts a new conversation.
    #[default]
    Fresh,
    /// Carrying on where the session was. Claude Code resumes its
    /// conversation: `claude --resume <id>` when the identifier of the
    /// conversation is known, else `claude --continue`, which resumes the
    /// most recent conversation of the folder. Shells and background
    /// commands have nothing to resume and start as with
    /// [`Fresh`](Self::Fresh).
    Resume,
}

/// Why a session could not do its job. It is part of the state of the
/// session ([`SessionState::Exited`](super::SessionState::Exited)), with a
/// message that can be shown to the user as it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[non_exhaustive]
pub enum LaunchFailure {
    /// The shell could not find the `claude` command.
    #[error("{message}")]
    ClaudeNotFound {
        /// What to tell the user.
        message: String,
    },
    /// The folder of the session does not exist any more, or is not a
    /// folder.
    #[error("{message}")]
    FolderMissing {
        /// What to tell the user.
        message: String,
    },
    /// The session has no command to run.
    #[error("{message}")]
    InvalidCommand {
        /// What to tell the user.
        message: String,
    },
    /// The system refused to start the process.
    #[error("{message}")]
    SpawnFailed {
        /// What to tell the user.
        message: String,
    },
}

impl LaunchFailure {
    /// The failure for a `claude` that the shell could not find.
    #[must_use]
    pub fn claude_not_found(program: &str) -> Self {
        Self::ClaudeNotFound {
            message: format!(
                "Claude Code is not installed, or your shell cannot find it: the \
                 `{program}` command was not found. Install Claude Code, check that \
                 `{program}` works in a terminal, and start the session again."
            ),
        }
    }

    /// The failure for a folder that is gone.
    #[must_use]
    pub fn folder_missing() -> Self {
        Self::FolderMissing {
            message: "The folder of this session does not exist any more.".to_owned(),
        }
    }

    /// The failure for a process the system refused to start. `reason` is
    /// shown to the user: it must not contain a command line.
    #[must_use]
    pub fn spawn_failed(reason: impl std::fmt::Display) -> Self {
        Self::SpawnFailed {
            message: format!("The session could not be started: {reason}."),
        }
    }

    /// The message for the user.
    #[must_use]
    pub fn message(&self) -> &str {
        match self {
            Self::ClaudeNotFound { message }
            | Self::FolderMissing { message }
            | Self::InvalidCommand { message }
            | Self::SpawnFailed { message } => message,
        }
    }
}

/// The families of shells a script is written for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShellFamily {
    /// sh and the shells that understand its syntax: bash, zsh, dash, ksh.
    Posix,
    /// fish.
    Fish,
    /// PowerShell: `pwsh` and Windows PowerShell.
    PowerShell,
    /// Anything else. Only `-c` is assumed to work.
    Other,
}

/// The shell that starts the programs of the sessions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shell {
    program: PathBuf,
    family: ShellFamily,
}

impl Shell {
    /// The shell at `program`. Its family is told from its file name.
    #[must_use]
    pub fn new(program: impl Into<PathBuf>) -> Self {
        let program = program.into();
        let family = family_of(&program);
        Self { program, family }
    }

    /// The user's default shell, as resolved by
    /// [`default_shell`].
    #[must_use]
    pub fn of_user() -> Self {
        Self::new(default_shell())
    }

    /// The path of the shell.
    #[must_use]
    pub fn program(&self) -> &Path {
        &self.program
    }

    /// The family of the shell.
    #[must_use]
    pub fn family(&self) -> ShellFamily {
        self.family
    }

    /// The command that makes this shell run `script` with the user's
    /// environment. See the [module documentation](self).
    fn run(&self, script: &str) -> PtyCommand {
        let command = PtyCommand::new(&self.program);
        match self.family {
            ShellFamily::Posix | ShellFamily::Fish => command.args(["-l", "-i", "-c", script]),
            ShellFamily::PowerShell => {
                command.args(["-NoLogo", "-EncodedCommand", &encode_powershell(script)])
            }
            ShellFamily::Other => command.args(["-c", script]),
        }
    }
}

fn family_of(program: &Path) -> ShellFamily {
    let name = program
        .file_name()
        .unwrap_or(OsStr::new(""))
        .to_string_lossy()
        .to_ascii_lowercase();
    let name = name.strip_suffix(".exe").unwrap_or(&name);
    match name {
        "sh" | "bash" | "zsh" | "dash" | "ksh" | "ksh93" | "mksh" | "ash" => ShellFamily::Posix,
        "fish" => ShellFamily::Fish,
        "pwsh" | "powershell" | "pwsh-preview" => ShellFamily::PowerShell,
        _ => ShellFamily::Other,
    }
}

/// What is needed to start one session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaunchRequest<'a> {
    /// What the session runs.
    pub kind: SessionKind,
    /// The folder to start it in.
    pub cwd: &'a Path,
    /// The command line of a background session.
    pub command: Option<&'a str>,
    /// The identifier of the Claude Code conversation, when it is known.
    pub claude_session_id: Option<&'a str>,
    /// From scratch, or carrying on.
    pub mode: LaunchMode,
}

/// Builds the commands of the sessions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launcher {
    shell: Shell,
    claude_program: String,
}

impl Default for Launcher {
    /// The user's default shell, running `claude`.
    fn default() -> Self {
        Self::new(Shell::of_user())
    }
}

impl Launcher {
    /// A launcher that starts programs through `shell`.
    #[must_use]
    pub fn new(shell: Shell) -> Self {
        Self {
            shell,
            claude_program: CLAUDE_PROGRAM.to_owned(),
        }
    }

    /// Runs another program in place of `claude`. Meant for tests, which
    /// must not depend on Claude Code being installed, or absent.
    ///
    /// `program` is a bare command name of letters, digits, `.`, `-` and
    /// `_`; anything else is ignored, since the name is written into a
    /// shell script.
    #[must_use]
    pub fn claude_program(mut self, program: &str) -> Self {
        let valid = !program.is_empty()
            && program
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'));
        if valid {
            program.clone_into(&mut self.claude_program);
        }
        self
    }

    /// The shell programs are started through.
    #[must_use]
    pub fn shell(&self) -> &Shell {
        &self.shell
    }

    /// The command that starts the session described by `request`.
    ///
    /// # Errors
    ///
    /// [`LaunchFailure::FolderMissing`] when the folder is not an existing
    /// directory, and [`LaunchFailure::InvalidCommand`] for a background
    /// session without a command.
    pub fn command(&self, request: &LaunchRequest<'_>) -> Result<PtyCommand, LaunchFailure> {
        if !request.cwd.is_dir() {
            return Err(LaunchFailure::folder_missing());
        }
        let command = match request.kind {
            SessionKind::Shell => PtyCommand::login_shell(&self.shell.program),
            SessionKind::Claude => self.shell.run(&self.claude_script(request)),
            SessionKind::Background => {
                let line = request.command.map(str::trim).unwrap_or_default();
                if line.is_empty() {
                    return Err(LaunchFailure::InvalidCommand {
                        message: "This background session has no command to run.".to_owned(),
                    });
                }
                self.shell.run(&self.background_script(line))
            }
        };
        Ok(command.cwd(request.cwd))
    }

    /// The failure an exit of a session of this launcher stands for, if
    /// any. See [`failure_for_exit`].
    #[must_use]
    pub fn failure_for_exit(
        &self,
        kind: SessionKind,
        status: &ExitStatus,
    ) -> Option<LaunchFailure> {
        (kind == SessionKind::Claude && status.code() == Some(CLAUDE_NOT_FOUND_EXIT_CODE))
            .then(|| LaunchFailure::claude_not_found(&self.claude_program))
    }

    /// The arguments of `claude` for this request.
    fn claude_arguments(request: &LaunchRequest<'_>) -> String {
        match (request.mode, request.claude_session_id) {
            (LaunchMode::Fresh, _) => String::new(),
            // The identifier is written into a script: one that is not
            // plainly an identifier is not used.
            (LaunchMode::Resume, Some(id)) if is_valid_claude_session_id(id) => {
                format!(" --resume {id}")
            }
            (LaunchMode::Resume, _) => " --continue".to_owned(),
        }
    }

    fn claude_script(&self, request: &LaunchRequest<'_>) -> String {
        let program = &self.claude_program;
        let arguments = Self::claude_arguments(request);
        match self.shell.family {
            // A program on PATH replaces the shell (`exec`), so that no
            // extra process stays around. An alias or a function can only
            // be run by the shell itself.
            ShellFamily::Posix => format!(
                "case \"$(command -v {program} 2>/dev/null)\" in \
                 /*) exec {program}{arguments} ;; \
                 '') exit {CLAUDE_NOT_FOUND_EXIT_CODE} ;; \
                 *) {program}{arguments} ;; \
                 esac"
            ),
            ShellFamily::Fish => format!(
                "if command -sq {program}; exec {program}{arguments}; \
                 else if type -q {program}; {program}{arguments}; \
                 else; exit {CLAUDE_NOT_FOUND_EXIT_CODE}; end"
            ),
            ShellFamily::PowerShell => format!(
                "if (-not (Get-Command {program} -ErrorAction SilentlyContinue)) \
                 {{ exit {CLAUDE_NOT_FOUND_EXIT_CODE} }}\n\
                 & {program}{arguments}\n\
                 exit $LASTEXITCODE"
            ),
            ShellFamily::Other => format!("{program}{arguments}"),
        }
    }

    fn background_script(&self, line: &str) -> String {
        match self.shell.family {
            // PowerShell does not end with the exit code of the last
            // program on its own.
            ShellFamily::PowerShell => format!("{line}\nexit $LASTEXITCODE"),
            _ => line.to_owned(),
        }
    }
}

/// The failure an exit stands for, for a session started by the default
/// [`Launcher`]: a Claude Code session whose script could not find `claude`
/// ([`CLAUDE_NOT_FOUND_EXIT_CODE`]). `None` for any other exit.
#[must_use]
pub fn failure_for_exit(kind: SessionKind, status: &ExitStatus) -> Option<LaunchFailure> {
    (kind == SessionKind::Claude && status.code() == Some(CLAUDE_NOT_FOUND_EXIT_CODE))
        .then(|| LaunchFailure::claude_not_found(CLAUDE_PROGRAM))
}

/// Encodes a script for the `-EncodedCommand` option of PowerShell: its
/// UTF-16 (little endian) bytes in Base64.
fn encode_powershell(script: &str) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let group = u32::from(chunk[0]) << 16
            | u32::from(chunk.get(1).copied().unwrap_or(0)) << 8
            | u32::from(chunk.get(2).copied().unwrap_or(0));
        for position in 0..4 {
            if position <= chunk.len() {
                let index = (group >> (18 - 6 * position)) & 0x3f;
                encoded.push(char::from(ALPHABET[index as usize]));
            } else {
                encoded.push('=');
            }
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request<'a>(kind: SessionKind, cwd: &'a Path) -> LaunchRequest<'a> {
        LaunchRequest {
            kind,
            cwd,
            command: None,
            claude_session_id: None,
            mode: LaunchMode::Fresh,
        }
    }

    fn launcher(shell: &str) -> Launcher {
        Launcher::new(Shell::new(shell))
    }

    /// The arguments a command was built with, read back from its debug
    /// form: `PtyCommand` does not expose them.
    fn described(command: &PtyCommand) -> String {
        format!("{command:?}")
    }

    #[test]
    fn shells_are_told_apart_by_their_file_name() {
        for (program, family) in [
            ("/bin/zsh", ShellFamily::Posix),
            ("/bin/bash", ShellFamily::Posix),
            ("/bin/sh", ShellFamily::Posix),
            ("/usr/bin/dash", ShellFamily::Posix),
            ("/opt/homebrew/bin/fish", ShellFamily::Fish),
            ("pwsh.exe", ShellFamily::PowerShell),
            (
                r"C:\Windows\System32\WindowsPowerShell\v1.0\PowerShell.EXE",
                ShellFamily::PowerShell,
            ),
            ("/usr/local/bin/pwsh", ShellFamily::PowerShell),
            ("/bin/tcsh", ShellFamily::Other),
            ("/usr/bin/nu", ShellFamily::Other),
            ("", ShellFamily::Other),
        ] {
            // A Windows path only has a file name on Windows.
            if program.contains('\\') && !cfg!(windows) {
                continue;
            }
            assert_eq!(Shell::new(program).family(), family, "{program}");
        }
    }

    #[test]
    fn a_shell_session_is_the_login_shell_in_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = launcher("/bin/zsh");

        let command = launcher
            .command(&request(SessionKind::Shell, dir.path()))
            .unwrap();

        assert_eq!(command, PtyCommand::login_shell("/bin/zsh").cwd(dir.path()));
    }

    #[test]
    fn claude_is_started_through_the_login_shell() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = launcher("/bin/zsh");

        let command = launcher
            .command(&request(SessionKind::Claude, dir.path()))
            .unwrap();

        let script = "case \"$(command -v claude 2>/dev/null)\" in \
                      /*) exec claude ;; '') exit 113 ;; *) claude ;; esac";
        assert_eq!(
            command,
            PtyCommand::new("/bin/zsh")
                .args(["-l", "-i", "-c", script])
                .cwd(dir.path())
        );
        assert!(!command.is_login_shell());
    }

    #[test]
    fn resuming_claude_uses_the_conversation_when_it_is_known() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = launcher("/bin/bash");
        let mut resume = request(SessionKind::Claude, dir.path());
        resume.mode = LaunchMode::Resume;

        let continued = described(&launcher.command(&resume).unwrap());
        assert!(
            continued.contains("exec claude --continue ;;"),
            "{continued}"
        );
        assert!(!continued.contains("--resume"), "{continued}");

        resume.claude_session_id = Some("0b9d6f7e-3c1a-4f3e-9a55-2f6f1f0f8a10");
        let resumed = described(&launcher.command(&resume).unwrap());
        assert!(
            resumed.contains("exec claude --resume 0b9d6f7e-3c1a-4f3e-9a55-2f6f1f0f8a10 ;;"),
            "{resumed}"
        );

        // A fresh start ignores the conversation.
        resume.mode = LaunchMode::Fresh;
        let fresh = described(&launcher.command(&resume).unwrap());
        assert!(fresh.contains("exec claude ;;"), "{fresh}");
    }

    #[test]
    fn a_conversation_identifier_that_is_not_one_never_reaches_the_script() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = launcher("/bin/sh");
        let mut resume = request(SessionKind::Claude, dir.path());
        resume.mode = LaunchMode::Resume;
        resume.claude_session_id = Some("x; rm -rf ~");

        let script = described(&launcher.command(&resume).unwrap());

        assert!(!script.contains("rm -rf"), "{script}");
        assert!(script.contains("claude --continue"), "{script}");
    }

    #[test]
    fn each_family_of_shells_gets_its_own_script() {
        let dir = tempfile::tempdir().unwrap();
        let claude = request(SessionKind::Claude, dir.path());

        let fish = described(&launcher("/usr/bin/fish").command(&claude).unwrap());
        assert!(fish.contains("\"-l\", \"-i\", \"-c\""), "{fish}");
        assert!(
            fish.contains("if command -sq claude; exec claude;"),
            "{fish}"
        );
        assert!(fish.contains("exit 113; end"), "{fish}");

        let other = launcher("/bin/tcsh").command(&claude).unwrap();
        assert_eq!(
            other,
            PtyCommand::new("/bin/tcsh")
                .args(["-c", "claude"])
                .cwd(dir.path())
        );

        let powershell = launcher("pwsh").command(&claude).unwrap();
        let script = "if (-not (Get-Command claude -ErrorAction SilentlyContinue)) { exit 113 }\n\
                      & claude\nexit $LASTEXITCODE";
        assert_eq!(
            powershell,
            PtyCommand::new("pwsh")
                .args(["-NoLogo", "-EncodedCommand", &encode_powershell(script)])
                .cwd(dir.path())
        );
    }

    #[test]
    fn a_background_command_is_given_to_the_shell_as_it_is() {
        let dir = tempfile::tempdir().unwrap();
        let mut background = request(SessionKind::Background, dir.path());
        background.command = Some("  ngrok http 3000 --log \"a b\" | tee 'x y'  ");

        let command = launcher("/bin/zsh").command(&background).unwrap();

        assert_eq!(
            command,
            PtyCommand::new("/bin/zsh")
                .args([
                    "-l",
                    "-i",
                    "-c",
                    "ngrok http 3000 --log \"a b\" | tee 'x y'"
                ])
                .cwd(dir.path())
        );

        let powershell = launcher("powershell.exe").command(&background).unwrap();
        assert_eq!(
            powershell,
            PtyCommand::new("powershell.exe")
                .args([
                    "-NoLogo",
                    "-EncodedCommand",
                    &encode_powershell(
                        "ngrok http 3000 --log \"a b\" | tee 'x y'\nexit $LASTEXITCODE"
                    ),
                ])
                .cwd(dir.path())
        );
    }

    #[test]
    fn a_background_session_needs_a_command() {
        let dir = tempfile::tempdir().unwrap();
        for command in [None, Some(""), Some("  \t ")] {
            let mut background = request(SessionKind::Background, dir.path());
            background.command = command;

            let failure = launcher("/bin/sh").command(&background).unwrap_err();

            assert!(
                matches!(failure, LaunchFailure::InvalidCommand { .. }),
                "{failure:?}"
            );
        }
    }

    #[test]
    fn a_folder_that_is_gone_is_a_typed_failure_for_every_kind() {
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("gone");
        let file = dir.path().join("file");
        std::fs::write(&file, b"").unwrap();

        for kind in [
            SessionKind::Claude,
            SessionKind::Shell,
            SessionKind::Background,
        ] {
            for cwd in [&gone, &file] {
                let mut request = request(kind, cwd);
                request.command = Some("true");

                let failure = launcher("/bin/sh").command(&request).unwrap_err();

                assert_eq!(failure, LaunchFailure::folder_missing());
                assert!(!failure.message().is_empty());
            }
        }
    }

    #[test]
    fn only_the_exit_code_of_the_script_means_that_claude_is_missing() {
        let exit = |code| ExitStatus::from_code(code);
        let launcher = launcher("/bin/sh").claude_program("my-claude");

        let failure = launcher
            .failure_for_exit(SessionKind::Claude, &exit(CLAUDE_NOT_FOUND_EXIT_CODE))
            .unwrap();
        assert!(matches!(failure, LaunchFailure::ClaudeNotFound { .. }));
        assert!(failure.message().contains("`my-claude`"), "{failure}");
        assert_eq!(
            failure_for_exit(SessionKind::Claude, &exit(CLAUDE_NOT_FOUND_EXIT_CODE)),
            Some(LaunchFailure::claude_not_found("claude"))
        );

        for code in [0, 1, 127, 130] {
            assert_eq!(
                launcher.failure_for_exit(SessionKind::Claude, &exit(code)),
                None
            );
        }
        for kind in [SessionKind::Shell, SessionKind::Background] {
            assert_eq!(
                launcher.failure_for_exit(kind, &exit(CLAUDE_NOT_FOUND_EXIT_CODE)),
                None
            );
        }
    }

    #[test]
    fn the_name_of_the_claude_program_cannot_carry_a_script() {
        let launcher = launcher("/bin/sh").claude_program("x; rm -rf ~");

        assert_eq!(launcher, Launcher::new(Shell::new("/bin/sh")));
        assert_eq!(
            Launcher::new(Shell::new("/bin/sh")).claude_program(""),
            launcher
        );
    }

    #[test]
    fn failures_are_tagged_with_their_kind() {
        assert_eq!(
            serde_json::to_value(LaunchFailure::folder_missing()).unwrap(),
            serde_json::json!({
                "kind": "folderMissing",
                "message": "The folder of this session does not exist any more.",
            })
        );
        let not_found = serde_json::to_value(LaunchFailure::claude_not_found("claude")).unwrap();
        assert_eq!(not_found["kind"], "claudeNotFound");
        assert_eq!(
            serde_json::to_value(LaunchFailure::spawn_failed("no more processes")).unwrap(),
            serde_json::json!({
                "kind": "spawnFailed",
                "message": "The session could not be started: no more processes.",
            })
        );
    }

    #[test]
    fn scripts_for_powershell_are_utf16_in_base64() {
        // Known answers, as produced by
        // [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes(...)).
        assert_eq!(encode_powershell(""), "");
        assert_eq!(encode_powershell("a"), "YQA=");
        assert_eq!(encode_powershell("ab"), "YQBiAA==");
        assert_eq!(encode_powershell("dir"), "ZABpAHIA");
        assert_eq!(encode_powershell("exit 3"), "ZQB4AGkAdAAgADMA");
        assert_eq!(encode_powershell("é"), "6QA=");
    }
}
