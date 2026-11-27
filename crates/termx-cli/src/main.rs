//! The command line of termx.
//!
//! For now it has one command, `termx-cli hook`, which the hooks of the
//! termx plugin for Claude Code run: it reads the event Claude Code
//! reports on its standard input and passes the little termx needs on to
//! the termx it runs in. See `termx_protocol::hook`.
//!
//! Whatever happens, `hook` prints nothing and exits 0, so that it can
//! never disturb Claude Code, inside termx or outside. Run without a
//! command, the program says how to use it, on its standard error only
//! when that is a terminal, and exits 0 as well: an older Claude Code that
//! ran it without its argument must not see a failure either.

use std::io::IsTerminal;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    match args.next().as_deref().and_then(|arg| arg.to_str()) {
        Some("hook") => {
            let _ = termx_protocol::hook::run(
                |name| std::env::var(name).ok(),
                std::io::stdin(),
                termx_protocol::parent::claude_process,
            );
            ExitCode::SUCCESS
        }
        Some("--version" | "-V") => {
            usage(&format!("termx-cli {}", env!("CARGO_PKG_VERSION")));
            ExitCode::SUCCESS
        }
        _ => {
            usage(
                "usage: termx-cli hook\n\n\
                 Reports an event of Claude Code to the termx it runs in. Run by\n\
                 the hooks of the termx plugin for Claude Code; it reads the event\n\
                 on its standard input, prints nothing and always exits 0.",
            );
            ExitCode::SUCCESS
        }
    }
}

/// Writes `text` on the standard error when a person can read it.
fn usage(text: &str) {
    use std::io::Write;

    let mut stderr = std::io::stderr();
    if stderr.is_terminal() {
        let _ = writeln!(stderr, "{text}");
    }
}
