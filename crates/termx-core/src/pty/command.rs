//! Description of the program to run inside a PTY.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use portable_pty::CommandBuilder;

use super::PtyError;
use super::shell::default_shell;

/// Value of `TERM` given to every process. xterm.js, which renders the
/// terminal, emulates xterm with 256 colours.
pub const TERM: &str = "xterm-256color";

/// Value of `COLORTERM` given to every process, advertising 24-bit colour.
pub const COLORTERM: &str = "truecolor";

/// Value of `TERM_PROGRAM` given to every process, so that programs can tell
/// they are running inside termx.
pub const TERM_PROGRAM: &str = "termx";

/// A program to run inside a PTY: what to execute, where, and with which
/// environment.
///
/// The process inherits the environment of the current process, with these
/// variables set on top of it:
///
/// | Variable               | Value                                     |
/// | ---------------------- | ----------------------------------------- |
/// | `TERM`                 | [`TERM`] (`xterm-256color`)               |
/// | `COLORTERM`            | [`COLORTERM`] (`truecolor`)               |
/// | `TERM_PROGRAM`         | [`TERM_PROGRAM`] (`termx`)                |
/// | `TERM_PROGRAM_VERSION` | [`termx_core::version()`](crate::version) |
///
/// Variables added with [`env`](Self::env) are applied last and therefore
/// override both the inherited environment and the defaults above.
///
/// # Examples
///
/// ```
/// use termx_core::pty::PtyCommand;
///
/// let command = PtyCommand::new("git")
///     .args(["log", "--oneline"])
///     .cwd("/tmp")
///     .env("GIT_PAGER", "cat");
/// assert_eq!(command.program(), "git");
///
/// // The user's shell, started as a login shell on macOS and Linux.
/// let shell = PtyCommand::default_shell();
/// assert!(!shell.program().is_empty());
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtyCommand {
    program: OsString,
    args: Vec<OsString>,
    cwd: Option<PathBuf>,
    env: Vec<(OsString, OsString)>,
    login_shell: bool,
}

impl PtyCommand {
    /// Creates a command that runs `program`.
    ///
    /// A bare name such as `"git"` is looked up on `PATH`.
    #[must_use]
    pub fn new(program: impl AsRef<OsStr>) -> Self {
        Self {
            program: program.as_ref().to_owned(),
            args: Vec::new(),
            cwd: None,
            env: Vec::new(),
            login_shell: false,
        }
    }

    /// Creates a command that runs `shell` as a login shell.
    ///
    /// On macOS and Linux the shell is started the way `login(1)` and other
    /// terminal emulators do it: `argv[0]` is the shell's file name prefixed
    /// with `-` (for example `-zsh`), which makes it read the user's profile.
    /// `shell` must be the absolute path of an executable, and the command
    /// cannot take arguments ([`PtyError::LoginShellArguments`]).
    ///
    /// Windows has no notion of a login shell, so there this is the same as
    /// [`new`](Self::new).
    #[must_use]
    pub fn login_shell(shell: impl AsRef<OsStr>) -> Self {
        Self {
            login_shell: cfg!(unix),
            ..Self::new(shell)
        }
    }

    /// Creates a command that runs the user's default shell, as resolved by
    /// [`default_shell`], as a login shell.
    #[must_use]
    pub fn default_shell() -> Self {
        Self::login_shell(default_shell())
    }

    /// Appends one argument.
    #[must_use]
    pub fn arg(mut self, arg: impl AsRef<OsStr>) -> Self {
        self.args.push(arg.as_ref().to_owned());
        self
    }

    /// Appends several arguments.
    #[must_use]
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.args
            .extend(args.into_iter().map(|arg| arg.as_ref().to_owned()));
        self
    }

    /// Sets the working directory of the process.
    ///
    /// When no directory is set, the process starts in the user's home
    /// directory, which is what a new terminal is expected to do (a desktop
    /// application's own working directory is rarely meaningful).
    #[must_use]
    pub fn cwd(mut self, dir: impl AsRef<Path>) -> Self {
        self.cwd = Some(dir.as_ref().to_owned());
        self
    }

    /// Sets an environment variable for the process, overriding any inherited
    /// or default value.
    #[must_use]
    pub fn env(mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> Self {
        self.env
            .push((key.as_ref().to_owned(), value.as_ref().to_owned()));
        self
    }

    /// Sets several environment variables. See [`env`](Self::env).
    #[must_use]
    pub fn envs<I, K, V>(mut self, vars: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        self.env.extend(
            vars.into_iter()
                .map(|(key, value)| (key.as_ref().to_owned(), value.as_ref().to_owned())),
        );
        self
    }

    /// The program this command runs.
    #[must_use]
    pub fn program(&self) -> &OsStr {
        &self.program
    }

    /// Whether the program is started as a login shell. Always `false` on
    /// Windows.
    #[must_use]
    pub fn is_login_shell(&self) -> bool {
        self.login_shell
    }

    /// Validates the command and converts it to a portable-pty builder.
    pub(super) fn to_builder(&self) -> Result<CommandBuilder, PtyError> {
        if self.program.is_empty() {
            return Err(PtyError::EmptyProgram);
        }
        if let Some(cwd) = &self.cwd
            && !cwd.is_dir()
        {
            // portable-pty would silently fall back to the home directory.
            return Err(PtyError::InvalidWorkingDirectory(cwd.clone()));
        }

        let mut builder = if self.login_shell {
            self.login_shell_builder()?
        } else {
            let mut builder = CommandBuilder::new(&self.program);
            builder.args(&self.args);
            builder
        };

        builder.env("TERM", TERM);
        builder.env("COLORTERM", COLORTERM);
        builder.env("TERM_PROGRAM", TERM_PROGRAM);
        builder.env("TERM_PROGRAM_VERSION", crate::version());
        for (key, value) in &self.env {
            builder.env(key, value);
        }
        if let Some(cwd) = &self.cwd {
            builder.cwd(cwd);
        }
        Ok(builder)
    }

    /// Builds the command for a login shell.
    ///
    /// portable-pty starts its "default program" as a login shell, taking the
    /// program from `SHELL`, and quietly substitutes another shell when that
    /// one is not executable. The check below turns that into an error.
    fn login_shell_builder(&self) -> Result<CommandBuilder, PtyError> {
        if !self.args.is_empty() {
            return Err(PtyError::LoginShellArguments);
        }
        let shell = Path::new(&self.program);
        if !shell.is_absolute() || !is_executable_file(shell) {
            return Err(PtyError::Spawn {
                program: self.program.to_string_lossy().into_owned(),
                message: "a login shell must be the absolute path of an executable file".to_owned(),
            });
        }
        let mut builder = CommandBuilder::new_default_prog();
        builder.env("SHELL", &self.program);
        Ok(builder)
    }
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    path.metadata()
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of<'a>(builder: &'a CommandBuilder, key: &str) -> Option<&'a str> {
        builder.get_env(key).and_then(OsStr::to_str)
    }

    #[test]
    fn builder_receives_program_and_arguments() {
        let builder = PtyCommand::new("prog")
            .arg("one")
            .args(["two", "three"])
            .to_builder()
            .unwrap();

        assert_eq!(builder.get_argv(), &["prog", "one", "two", "three"]);
    }

    #[test]
    fn terminal_identification_variables_are_set() {
        let builder = PtyCommand::new("prog").to_builder().unwrap();

        assert_eq!(env_of(&builder, "TERM"), Some("xterm-256color"));
        assert_eq!(env_of(&builder, "COLORTERM"), Some("truecolor"));
        assert_eq!(env_of(&builder, "TERM_PROGRAM"), Some("termx"));
        assert_eq!(
            env_of(&builder, "TERM_PROGRAM_VERSION"),
            Some(crate::version())
        );
    }

    #[test]
    fn extra_variables_override_the_defaults_and_later_ones_win() {
        let builder = PtyCommand::new("prog")
            .env("TERM", "dumb")
            .envs([("TERMX_TEST", "first"), ("TERMX_OTHER", "x")])
            .env("TERMX_TEST", "second")
            .to_builder()
            .unwrap();

        assert_eq!(env_of(&builder, "TERM"), Some("dumb"));
        assert_eq!(env_of(&builder, "TERMX_TEST"), Some("second"));
        assert_eq!(env_of(&builder, "TERMX_OTHER"), Some("x"));
        assert_eq!(env_of(&builder, "COLORTERM"), Some("truecolor"));
    }

    #[test]
    fn working_directory_is_passed_through() {
        let dir = std::env::temp_dir();
        let builder = PtyCommand::new("prog").cwd(&dir).to_builder().unwrap();

        assert_eq!(builder.get_cwd(), Some(&dir.into_os_string()));
    }

    #[test]
    fn missing_working_directory_is_an_error() {
        let dir = std::env::temp_dir().join("termx-this-directory-does-not-exist");
        let error = PtyCommand::new("prog").cwd(&dir).to_builder().unwrap_err();

        assert!(
            matches!(&error, PtyError::InvalidWorkingDirectory(path) if *path == dir),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn empty_program_is_an_error() {
        let error = PtyCommand::new("").to_builder().unwrap_err();
        assert!(matches!(error, PtyError::EmptyProgram), "{error}");
    }

    #[test]
    fn default_shell_command_runs_the_default_shell() {
        let command = PtyCommand::default_shell();

        assert_eq!(command.program(), default_shell().as_os_str());
        assert_eq!(command.is_login_shell(), cfg!(unix));
        assert!(command.to_builder().is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn login_shell_is_started_through_argv0() {
        let builder = PtyCommand::login_shell("/bin/sh").to_builder().unwrap();

        // An empty argv is how portable-pty is told to start `$SHELL` with a
        // dash-prefixed argv[0].
        assert!(builder.is_default_prog());
        assert_eq!(env_of(&builder, "SHELL"), Some("/bin/sh"));
    }

    #[cfg(unix)]
    #[test]
    fn login_shell_rejects_arguments() {
        let error = PtyCommand::login_shell("/bin/sh")
            .arg("-c")
            .to_builder()
            .unwrap_err();

        assert!(matches!(error, PtyError::LoginShellArguments), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn login_shell_must_be_an_absolute_executable() {
        for shell in ["sh", "/termx/does/not/exist", "/etc/hosts", "/bin"] {
            let error = PtyCommand::login_shell(shell).to_builder().unwrap_err();
            assert!(
                matches!(&error, PtyError::Spawn { program, .. } if program == shell),
                "unexpected error for {shell}: {error}"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn login_shell_is_a_plain_command_on_windows() {
        let command = PtyCommand::login_shell("cmd.exe").arg("/c").arg("ver");

        assert!(!command.is_login_shell());
        assert_eq!(command.to_builder().unwrap().get_argv().len(), 3);
    }
}
