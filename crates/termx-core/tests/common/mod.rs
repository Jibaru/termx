//! Helpers shared by the integration tests that drive real pseudo-terminals
//! (`tests/pty.rs` and `tests/session.rs`).
//!
//! Most tests run the `pty-test-helper` example (see
//! `tests/helpers/pty_helper.rs`) inside the PTY, because it behaves
//! identically on every platform.
//!
//! Two rules keep the suites reliable:
//!
//! - **Nothing waits forever.** Every test body runs under [`guarded`], and
//!   every wait has a timeout, so a bug shows up as a failure and never as a
//!   hung CI job.
//! - **Content, not bytes, on Windows.** ConPTY renders the console screen
//!   instead of passing bytes through, so assertions search the visible text
//!   (escape sequences stripped). Exact byte comparisons are made on Unix,
//!   where the PTY is transparent.

// Each test binary uses its own subset of these helpers.
#![allow(dead_code)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, mpsc};
use std::thread;
use std::time::Duration;

use termx_core::pty::PtyCommand;

/// Upper bound for any single wait. Generous, because CI machines are slow
/// (starting PowerShell alone can take several seconds).
pub const TIMEOUT: Duration = Duration::from_secs(60);

/// Upper bound for a whole test.
pub const TEST_TIMEOUT: Duration = Duration::from_secs(180);

/// Runs a test body on its own thread and fails if it does not finish in
/// time, so that a deadlock (for example in `Drop`) cannot hang the suite.
pub fn guarded(test: impl FnOnce() + Send + 'static) {
    let (done_tx, done_rx) = mpsc::channel();
    let body = thread::spawn(move || {
        test();
        let _ = done_tx.send(());
    });
    match done_rx.recv_timeout(TEST_TIMEOUT) {
        Ok(()) => body
            .join()
            .expect("the test thread panicked after finishing"),
        // The sender was dropped without sending: the body panicked.
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            if let Err(panic) = body.join() {
                std::panic::resume_unwind(panic);
            }
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            panic!("the test did not finish within {TEST_TIMEOUT:?}")
        }
    }
}

/// Path of the `pty-test-helper` example.
///
/// Cargo does not tell integration tests where it puts examples, and the
/// layout of the target directory changes between Cargo versions. So Cargo is
/// asked: building the example (a no-op when it is up to date, which it is
/// after a plain `cargo test`) with JSON messages reports the executable.
pub fn helper_path() -> &'static Path {
    static HELPER: OnceLock<PathBuf> = OnceLock::new();
    HELPER.get_or_init(|| {
        let mut cargo = std::process::Command::new(env!("CARGO"));
        cargo
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .args(["build", "--example", "pty-test-helper"])
            .arg("--message-format=json");
        if !cfg!(debug_assertions) {
            cargo.arg("--release");
        }
        let output = cargo.output().expect("cannot run cargo");
        assert!(
            output.status.success(),
            "building the test helper failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let messages = String::from_utf8_lossy(&output.stdout);
        let helper = messages
            .lines()
            .filter(|line| line.contains(r#""name":"pty-test-helper""#))
            .find_map(executable_of)
            .expect("cargo did not report where the test helper is");
        assert!(
            helper.is_file(),
            "the test helper was not found at {}",
            helper.display()
        );
        helper
    })
}

/// Extracts the `executable` field of one of Cargo's JSON messages.
pub fn executable_of(message: &str) -> Option<PathBuf> {
    let (_, rest) = message.split_once(r#""executable":""#)?;
    let mut path = String::new();
    let mut chars = rest.chars();
    loop {
        match chars.next()? {
            '"' => return Some(PathBuf::from(path)),
            // Only backslashes and quotes are escaped in a path.
            '\\' => path.push(chars.next()?),
            c => path.push(c),
        }
    }
}

/// A command running the test helper in the given mode.
pub fn helper(args: &[&str]) -> PtyCommand {
    PtyCommand::new(helper_path()).args(args)
}

/// Creates an empty directory for one test, inside Cargo's directory for
/// integration test files.
pub fn scratch_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("pty-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("cannot create the scratch directory");
    dir
}

/// Removes escape sequences and carriage returns, leaving the text a user
/// would see.
pub fn visible_text(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    let mut visible = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => match chars.next() {
                // CSI: parameters, then a final byte in `@`..=`~`.
                Some('[') => {
                    for c in chars.by_ref() {
                        if ('\x40'..='\x7e').contains(&c) {
                            break;
                        }
                    }
                }
                // OSC: terminated by BEL or by ST (`ESC \`).
                Some(']') => {
                    while let Some(c) = chars.next() {
                        if c == '\x07' {
                            break;
                        }
                        if c == '\x1b' {
                            chars.next();
                            break;
                        }
                    }
                }
                // Character set designation: one more byte.
                Some('(' | ')' | '*' | '+' | '#') => {
                    chars.next();
                }
                // Any other escape sequence is two bytes long.
                _ => {}
            },
            '\r' => {}
            c => visible.push(c),
        }
    }
    visible
}

/// Finds the numbered lines printed by the helper's `repeat` mode
/// (`<7 digits>:<text>`) and returns their numbers.
///
/// It searches the text instead of splitting it into lines, because ConPTY
/// may move the cursor instead of printing a line break.
pub fn numbered_lines(visible: &str, text: &str) -> BTreeSet<usize> {
    let marker = format!(":{text}");
    visible
        .match_indices(&marker)
        .filter_map(|(position, _)| {
            let digits = visible.get(position.checked_sub(7)?..position)?;
            digits.parse().ok()
        })
        .collect()
}

/// Reads the counter the helper's `heartbeat` mode keeps writing.
pub fn read_heartbeat(file: &Path) -> Option<u64> {
    std::fs::read_to_string(file).ok()?.trim().parse().ok()
}
