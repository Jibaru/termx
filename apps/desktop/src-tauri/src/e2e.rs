//! Test-only hooks for the end-to-end suite (`e2e/` at the repository root),
//! which drives the real application: real webview, real xterm.js, real PTY.
//!
//! This module only exists with the `e2e` cargo feature, and that feature
//! only compiles in a build with debug assertions, outside the `release`
//! profile, that sets `TERMX_ALLOW_E2E=1` (see `lib.rs` and `build.rs`), so
//! nothing here can be part of a release. It adds three things, and changes neither
//! the capabilities nor the content security policy of the application:
//!
//! - **A marker for the frontend** (`window.__TERMX_E2E__ = true`), set
//!   before the page runs. The frontend answers by exposing its terminals on
//!   that property, which is how the tests read the xterm.js buffer. Without
//!   the marker the frontend exposes nothing.
//! - **A `WebDriver` server** inside the application
//!   (`tauri-plugin-wdio-webdriver`), started only when
//!   [`WEBDRIVER_PORT_ENV_VAR`] names a port. It listens on the loopback
//!   interface. This is what lets one harness drive the application on
//!   macOS too, where no `WebDriver` exists for `WKWebView`.
//! - **The program shell sessions run** instead of the login shell
//!   ([`SHELL_ENV_VAR`]), so that the tests never depend on the shell or the
//!   dotfiles of whoever runs them.

use termx_core::diagnostics::tracing::{info, warn};
use termx_core::workspace::Launch;

/// Environment variable naming the port of the `WebDriver` server. The
/// server is not started without it.
pub const WEBDRIVER_PORT_ENV_VAR: &str = "TAURI_WEBDRIVER_PORT";

/// Environment variable naming the program the terminals run instead of the
/// login shell: the program and its arguments, one per line.
pub const SHELL_ENV_VAR: &str = "TERMX_E2E_SHELL";

/// Runs before any script of the page.
const MARKER_SCRIPT: &str = "window.__TERMX_E2E__ = true;";

/// Adds the hooks to the application being built.
pub fn install(builder: tauri::Builder<tauri::Wry>) -> tauri::Builder<tauri::Wry> {
    let marker = tauri::plugin::Builder::<tauri::Wry>::new("termx-e2e")
        .js_init_script(MARKER_SCRIPT)
        .build();
    let builder = builder.plugin(marker);

    let requested = std::env::var(WEBDRIVER_PORT_ENV_VAR).ok();
    match requested.as_deref().map(parse_port) {
        Some(Some(port)) => {
            info!(port, "end-to-end build: starting the WebDriver server");
            builder.plugin(tauri_plugin_wdio_webdriver::init_with_port(port))
        }
        Some(None) => {
            warn!("end-to-end build: the WebDriver port is not a valid port");
            builder
        }
        None => builder,
    }
}

/// A start of a shell session runs the program the environment names, if
/// it names one, in the same folder and environment. The sessions of other
/// kinds, which run a script of the workspace, are left as they are.
pub fn override_shell(mut launch: Launch) -> Launch {
    if launch.token.is_some() {
        return launch;
    }
    let Some((program, args)) = std::env::var(SHELL_ENV_VAR)
        .ok()
        .as_deref()
        .and_then(parse_shell)
    else {
        return launch;
    };
    launch.command = launch.command.with_program(program, args);
    launch
}

fn parse_port(value: &str) -> Option<u16> {
    value.parse().ok().filter(|&port| port != 0)
}

/// The program and its arguments, one per line.
fn parse_shell(value: &str) -> Option<(String, Vec<String>)> {
    let mut parts = value
        .lines()
        .filter(|part| !part.is_empty())
        .map(str::to_owned);
    let program = parts.next()?;
    Some((program, parts.collect()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_port_must_be_a_usable_port() {
        assert_eq!(parse_port("4445"), Some(4445));
        for invalid in ["", "0", "65536", "-1", "port", " 4445"] {
            assert_eq!(parse_port(invalid), None, "{invalid:?}");
        }
    }

    #[test]
    fn the_shell_is_a_program_and_its_arguments_one_per_line() {
        assert_eq!(
            parse_shell("/bin/bash\n--noprofile\n--norc"),
            Some((
                "/bin/bash".to_owned(),
                vec!["--noprofile".to_owned(), "--norc".to_owned()]
            ))
        );
        assert_eq!(parse_shell("cmd.exe"), Some(("cmd.exe".to_owned(), vec![])));
        assert_eq!(parse_shell(""), None);
        assert_eq!(parse_shell("\n"), None);
    }
}
