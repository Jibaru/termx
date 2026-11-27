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

/// How long a process is given by default to exit on its own when it is
/// killed, before it is killed by force. See [`PtyCommand::kill_grace`].
///
/// A shell exits within milliseconds. Programs that save their state when
/// the terminal goes away (an editor writing a swap file, Claude Code
/// flushing its session) need tens to hundreds of milliseconds, more on a
/// busy machine, and being cut short costs the user data. Three seconds
/// leaves them an order of magnitude of margin, while a program that is
/// truly stuck delays closing its terminal, or quitting the application, by
/// a pause that is still short. Terminals are killed in parallel, so that
/// pause does not grow with their number.
pub const DEFAULT_KILL_GRACE: std::time::Duration = std::time::Duration::from_secs(3);

/// Locale given to a process when the environment does not select one (Unix
/// only). See [`PtyCommand`].
pub const DEFAULT_LOCALE: &str = if cfg!(target_os = "macos") {
    "en_US.UTF-8"
} else {
    // Present on every current Linux distribution without generating locales.
    "C.UTF-8"
};

/// Environment variables that are not passed on to the processes spawned in
/// a PTY, even when the application itself was started with them.
///
/// They describe the *session the application was launched from*, not the
/// new terminal, and would mislead the programs running in it. A name ending
/// in `*` matches every variable starting with what precedes it. Extend the
/// list for one command with [`PtyCommand::env_remove`].
///
/// - **Terminal multiplexers** (`TMUX`, `TMUX_PANE`, `STY`): programs would
///   believe they run inside tmux or screen, and `tmux` would refuse to
///   start "nested" sessions.
/// - **Claude Code session state** (`CLAUDECODE`, `CLAUDE_CODE_ENTRYPOINT`,
///   `CLAUDE_CODE_SESSION_ID`, `CLAUDE_EFFORT`, `CLAUDE_PROJECT_DIR`, ...):
///   set by a running Claude Code for its own children. A Claude Code
///   started in the terminal would believe it is a child of the session
///   termx was launched from.
/// - **npm and pnpm scripts** (`npm_*`, `INIT_CWD`, `PNPM_SCRIPT_SRC_DIR`):
///   set when the application is started through a package script, as
///   during development; they change how package managers behave.
/// - **Other terminal emulators and editors** (`TERM_SESSION_ID`, `ITERM_*`,
///   `KITTY_*`, `GHOSTTY_*`, `WEZTERM_*`, `ALACRITTY_*`, `KONSOLE_*`,
///   `WT_SESSION`, the shell integration variables of VS Code, ...):
///   programs use them to detect features of a terminal that is not the one
///   they are running in, or to talk to it through a socket.
/// - **Launch context** (`__CFBundleIdentifier`, which macOS sets to the
///   bundle of the launching application, and `GPG_TTY`, which names the
///   terminal device of the launching shell and would send passphrase
///   prompts there).
///
/// # What is deliberately kept
///
/// The list contains state, never configuration. In particular these are
/// inherited, because removing them would silently change how the user's
/// tools behave:
///
/// - `ANTHROPIC_*` and `CLAUDE_CODE_OAUTH_TOKEN`: credentials and API
///   endpoints.
/// - `CLAUDE_CONFIG_DIR`: where the user keeps the Claude Code configuration.
/// - `CLAUDE_CODE_USE_BEDROCK`, `CLAUDE_CODE_USE_VERTEX`,
///   `CLAUDE_CODE_MAX_OUTPUT_TOKENS` and the other `CLAUDE_CODE_*` settings.
///   This is why Claude Code variables are listed one by one instead of
///   with a `CLAUDE_*` pattern.
/// - `NPM_TOKEN`, `NPM_CONFIG_*` and `NODE_*`: user configuration, unlike
///   the lower-case `npm_*` variables npm sets while running a script.
/// - `GIT_ASKPASS`, `EDITOR`, `SSH_AUTH_SOCK`, `TERMINFO`: possibly set by
///   an editor or a terminal, but just as possibly by the user.
pub const DEFAULT_ENV_DENYLIST: &[&str] = &[
    // Terminal multiplexers.
    "TMUX",
    "TMUX_PANE",
    "STY",
    // Claude Code session state.
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_SSE_PORT",
    "CLAUDE_CODE_REMOTE",
    "CLAUDE_AGENT_SDK_VERSION",
    "CLAUDE_PID",
    "CLAUDE_EFFORT",
    "CLAUDE_PROJECT_DIR",
    "CLAUDE_ENV_FILE",
    "CLAUDE_PLUGIN_ROOT",
    // npm and pnpm scripts.
    "npm_*",
    "INIT_CWD",
    "PNPM_SCRIPT_SRC_DIR",
    // Other terminal emulators.
    "TERM_SESSION_ID",
    "ITERM_*",
    "LC_TERMINAL",
    "LC_TERMINAL_VERSION",
    "KITTY_*",
    "GHOSTTY_*",
    "ALACRITTY_*",
    "WEZTERM_*",
    "KONSOLE_*",
    "WT_SESSION",
    "WT_PROFILE_ID",
    "VTE_VERSION",
    "WINDOWID",
    // The integrated terminal of VS Code and its forks.
    "VSCODE_INJECTION",
    "VSCODE_SHELL_INTEGRATION",
    "VSCODE_NONCE",
    "VSCODE_STABLE",
    "VSCODE_IPC_HOOK_CLI",
    "VSCODE_GIT_*",
    // Launch context.
    "__CFBundleIdentifier",
    "GPG_TTY",
];

/// A program to run inside a PTY: what to execute, where, and with which
/// environment.
///
/// # Environment
///
/// The process inherits the environment of the current process, minus the
/// variables of [`DEFAULT_ENV_DENYLIST`] and of
/// [`env_remove`](Self::env_remove), with these variables set on top of it:
///
/// | Variable               | Value                                     |
/// | ---------------------- | ----------------------------------------- |
/// | `TERM`                 | [`TERM`] (`xterm-256color`)               |
/// | `COLORTERM`            | [`COLORTERM`] (`truecolor`)               |
/// | `TERM_PROGRAM`         | [`TERM_PROGRAM`] (`termx`)                |
/// | `TERM_PROGRAM_VERSION` | [`termx_core::version()`](crate::version) |
///
/// Variables added with [`env`](Self::env) are applied last and therefore
/// override the inherited environment, the defaults above and the denylist.
///
/// On macOS and Linux, if neither `LC_ALL`, `LC_CTYPE` nor `LANG` ends up
/// set, `LANG` is set to [`DEFAULT_LOCALE`]. An application started from the
/// Finder or a desktop launcher has no locale at all, and shells would
/// otherwise start in the `C` locale and garble anything that is not ASCII.
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
    env_remove: Vec<String>,
    login_shell: bool,
    kill_grace: std::time::Duration,
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
            env_remove: Vec::new(),
            login_shell: false,
            kill_grace: DEFAULT_KILL_GRACE,
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

    /// Stops an inherited environment variable from being passed on to the
    /// process, in addition to those of [`DEFAULT_ENV_DENYLIST`].
    ///
    /// `pattern` is the name of a variable, or a prefix followed by `*` to
    /// match every variable starting with it (`"npm_*"`). Prefixes are
    /// case-sensitive; full names are too, except on Windows where the
    /// system treats them as case-insensitive.
    ///
    /// This only filters what is inherited: a variable given to
    /// [`env`](Self::env) is always set.
    #[must_use]
    pub fn env_remove(mut self, pattern: impl Into<String>) -> Self {
        self.env_remove.push(pattern.into());
        self
    }

    /// Sets how long the process is given to exit on its own when it is
    /// killed ([`Pty::kill`](super::Pty::kill), or the handle being
    /// dropped), before it is killed by force. The default is
    /// [`DEFAULT_KILL_GRACE`].
    ///
    /// On Unix the processes of the terminal are first sent `SIGHUP`, and
    /// `SIGKILL` only if they are still running when the grace period is
    /// over. A zero duration kills by force right away. Killing never
    /// blocks whatever the value; only dropping the handle waits, and only
    /// for processes that ignore the first signal.
    ///
    /// Windows has no equivalent of the polite signal: the process is
    /// always terminated immediately and this setting is ignored.
    #[must_use]
    pub fn kill_grace(mut self, grace: std::time::Duration) -> Self {
        self.kill_grace = grace;
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

    /// The grace period set with [`kill_grace`](Self::kill_grace).
    pub(super) fn kill_grace_period(&self) -> std::time::Duration {
        self.kill_grace
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

        self.apply_environment(&mut builder);
        if let Some(cwd) = &self.cwd {
            builder.cwd(cwd);
        }
        Ok(builder)
    }

    /// Turns the inherited environment held by `builder` into the one the
    /// process gets.
    fn apply_environment(&self, builder: &mut CommandBuilder) {
        let denied: Vec<String> = builder
            .iter_full_env_as_str()
            .map(|(name, _)| name)
            .filter(|name| {
                DEFAULT_ENV_DENYLIST
                    .iter()
                    .copied()
                    .chain(self.env_remove.iter().map(String::as_str))
                    .any(|pattern| matches_pattern(name, pattern))
            })
            .map(str::to_owned)
            .collect();
        for name in denied {
            builder.env_remove(name);
        }

        builder.env("TERM", TERM);
        builder.env("COLORTERM", COLORTERM);
        builder.env("TERM_PROGRAM", TERM_PROGRAM);
        builder.env("TERM_PROGRAM_VERSION", crate::version());
        for (key, value) in &self.env {
            builder.env(key, value);
        }

        if cfg!(unix) {
            let has_locale = ["LC_ALL", "LC_CTYPE", "LANG"]
                .iter()
                .any(|name| builder.get_env(name).is_some_and(|value| !value.is_empty()));
            if !has_locale {
                builder.env("LANG", DEFAULT_LOCALE);
            }
        }
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

/// Whether the variable `name` matches `pattern`: a name, or a prefix
/// followed by `*`.
fn matches_pattern(name: &str, pattern: &str) -> bool {
    match pattern.strip_suffix('*') {
        // Prefixes are compared exactly even on Windows, where names are
        // otherwise case-insensitive: `npm_*` must drop what npm sets
        // (`npm_config_...`) but keep the user's `NPM_TOKEN`.
        Some(prefix) => name.starts_with(prefix),
        None if cfg!(windows) => name.eq_ignore_ascii_case(pattern),
        None => name == pattern,
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

    /// A builder whose environment is exactly `vars`, whatever the
    /// environment of the test run is.
    fn builder_with_env(vars: &[(&str, &str)]) -> CommandBuilder {
        let mut builder = CommandBuilder::new("prog");
        builder.env_clear();
        for (name, value) in vars {
            builder.env(name, value);
        }
        builder
    }

    #[test]
    fn session_markers_of_the_parent_are_not_inherited() {
        let inherited = [
            ("TMUX", "/tmp/tmux-501/default,1,0"),
            ("TMUX_PANE", "%1"),
            ("STY", "1234.pts-0.host"),
            ("CLAUDECODE", "1"),
            ("CLAUDE_CODE_ENTRYPOINT", "cli"),
            ("CLAUDE_CODE_SESSION_ID", "abc"),
            ("CLAUDE_CODE_SSE_PORT", "1234"),
            ("npm_config_user_agent", "pnpm/11"),
            ("npm_package_name", "termx"),
            ("INIT_CWD", "/repo"),
            ("TERM_SESSION_ID", "w0t0p0"),
            ("ITERM_SESSION_ID", "w0t0p0"),
            ("WT_SESSION", "guid"),
            ("KITTY_WINDOW_ID", "1"),
            ("CLAUDE_EFFORT", "high"),
            ("CLAUDE_PROJECT_DIR", "/repo"),
            ("__CFBundleIdentifier", "com.apple.Terminal"),
            ("GPG_TTY", "/dev/ttys003"),
            ("VSCODE_INJECTION", "1"),
            ("VSCODE_GIT_IPC_HANDLE", "/tmp/vscode-git.sock"),
            ("GHOSTTY_RESOURCES_DIR", "/Applications/Ghostty.app"),
            ("ITERM_PROFILE", "Default"),
            ("WEZTERM_PANE", "0"),
            ("ALACRITTY_SOCKET", "/tmp/alacritty.sock"),
        ];
        let mut builder = builder_with_env(&inherited);
        builder.env("PATH", "/usr/bin");
        builder.env("HOME", "/home/me");
        // Configuration and credentials that merely look similar.
        let kept = [
            ("CLAUDE_CODE_USE_BEDROCK", "1"),
            ("CLAUDE_CONFIG_DIR", "/home/me/.claude"),
            ("CLAUDE_CODE_OAUTH_TOKEN", "token"),
            ("ANTHROPIC_API_KEY", "key"),
            ("ANTHROPIC_BASE_URL", "https://example.invalid"),
            ("NPM_TOKEN", "secret"),
            ("NPM_CONFIG_REGISTRY", "https://example.invalid"),
            ("GIT_ASKPASS", "/usr/bin/askpass"),
            ("VSCODE_CUSTOM_SETTING", "user"),
            ("SSH_AUTH_SOCK", "/tmp/agent"),
        ];
        for (name, value) in kept {
            builder.env(name, value);
        }

        PtyCommand::new("prog").apply_environment(&mut builder);

        for (name, _) in inherited {
            assert_eq!(env_of(&builder, name), None, "{name} was inherited");
        }
        // Ordinary variables and user settings are untouched.
        assert_eq!(env_of(&builder, "PATH"), Some("/usr/bin"));
        assert_eq!(env_of(&builder, "HOME"), Some("/home/me"));
        for (name, value) in kept {
            assert_eq!(env_of(&builder, name), Some(value), "{name} was dropped");
        }
    }

    #[test]
    fn every_default_denylist_entry_is_enforced() {
        for pattern in DEFAULT_ENV_DENYLIST {
            let name = pattern.replace('*', "example");
            let mut builder = builder_with_env(&[(&name, "value")]);

            PtyCommand::new("prog").apply_environment(&mut builder);

            assert_eq!(env_of(&builder, &name), None, "{name} was inherited");
        }
    }

    #[test]
    fn the_denylist_can_be_extended_per_command() {
        let mut builder = builder_with_env(&[
            ("SECRET_TOKEN", "x"),
            ("MYAPP_ONE", "1"),
            ("MYAPP_TWO", "2"),
            ("MYAPPLICATION", "kept"),
            ("OTHER", "kept"),
        ]);

        PtyCommand::new("prog")
            .env_remove("SECRET_TOKEN")
            .env_remove("MYAPP_*")
            .apply_environment(&mut builder);

        assert_eq!(env_of(&builder, "SECRET_TOKEN"), None);
        assert_eq!(env_of(&builder, "MYAPP_ONE"), None);
        assert_eq!(env_of(&builder, "MYAPP_TWO"), None);
        assert_eq!(env_of(&builder, "MYAPPLICATION"), Some("kept"));
        assert_eq!(env_of(&builder, "OTHER"), Some("kept"));
    }

    #[test]
    fn explicitly_set_variables_win_over_the_denylist() {
        let mut builder = builder_with_env(&[("TMUX", "inherited"), ("DROPPED", "inherited")]);

        PtyCommand::new("prog")
            .env_remove("DROPPED")
            .env("TMUX", "explicit")
            .env("DROPPED", "explicit")
            .apply_environment(&mut builder);

        assert_eq!(env_of(&builder, "TMUX"), Some("explicit"));
        assert_eq!(env_of(&builder, "DROPPED"), Some("explicit"));
    }

    #[test]
    fn patterns_match_names_and_prefixes() {
        assert!(matches_pattern("TMUX", "TMUX"));
        assert!(!matches_pattern("TMUX_PANE", "TMUX"));
        assert!(!matches_pattern("TMU", "TMUX"));
        assert!(matches_pattern("npm_config_registry", "npm_*"));
        assert!(matches_pattern("npm_", "npm_*"));
        assert!(!matches_pattern("npm", "npm_*"));
        assert!(!matches_pattern("xnpm_config", "npm_*"));
        // A multi-byte name shorter than the prefix must not panic.
        assert!(!matches_pattern("\u{e9}", "npm_*"));
        assert_eq!(matches_pattern("tmux", "TMUX"), cfg!(windows));
        assert!(!matches_pattern("NPM_TOKEN", "npm_*"));
    }

    #[cfg(unix)]
    #[test]
    fn a_utf8_locale_is_set_when_the_environment_has_none() {
        // What an application started from the Finder gets: no locale.
        let mut builder = builder_with_env(&[("PATH", "/usr/bin")]);
        PtyCommand::new("prog").apply_environment(&mut builder);
        assert_eq!(env_of(&builder, "LANG"), Some(DEFAULT_LOCALE));
        assert!(DEFAULT_LOCALE.ends_with(".UTF-8"));

        // An empty value selects no locale either.
        let mut builder = builder_with_env(&[("LANG", ""), ("LC_ALL", "")]);
        PtyCommand::new("prog").apply_environment(&mut builder);
        assert_eq!(env_of(&builder, "LANG"), Some(DEFAULT_LOCALE));
    }

    #[cfg(unix)]
    #[test]
    fn an_existing_locale_is_respected() {
        for (name, value) in [
            ("LANG", "es_PE.UTF-8"),
            ("LANG", "C"),
            ("LC_ALL", "de_DE.UTF-8"),
            ("LC_CTYPE", "UTF-8"),
        ] {
            let mut builder = builder_with_env(&[(name, value)]);
            PtyCommand::new("prog").apply_environment(&mut builder);

            assert_eq!(env_of(&builder, name), Some(value));
            if name != "LANG" {
                assert_eq!(
                    env_of(&builder, "LANG"),
                    None,
                    "LANG was added next to {name}"
                );
            }
        }

        // A locale chosen for the command counts as well.
        let mut builder = builder_with_env(&[]);
        PtyCommand::new("prog")
            .env("LC_ALL", "ja_JP.UTF-8")
            .apply_environment(&mut builder);
        assert_eq!(env_of(&builder, "LANG"), None);
    }

    #[cfg(windows)]
    #[test]
    fn no_locale_is_set_on_windows() {
        let mut builder = builder_with_env(&[]);
        PtyCommand::new("prog").apply_environment(&mut builder);
        assert_eq!(env_of(&builder, "LANG"), None);
    }

    #[test]
    fn kill_grace_defaults_to_a_generous_value_and_can_be_set() {
        use std::time::Duration;

        let command = PtyCommand::new("prog");
        assert_eq!(command.kill_grace_period(), DEFAULT_KILL_GRACE);
        assert!(DEFAULT_KILL_GRACE >= Duration::from_secs(1));
        assert_eq!(
            PtyCommand::default_shell().kill_grace_period(),
            DEFAULT_KILL_GRACE
        );

        let command = command.kill_grace(Duration::from_millis(40));
        assert_eq!(command.kill_grace_period(), Duration::from_millis(40));
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
