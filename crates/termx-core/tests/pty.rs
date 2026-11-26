//! Integration tests of the PTY layer against real pseudo-terminals.
//!
//! Most tests run the `pty-test-helper` example (see
//! `tests/helpers/pty_helper.rs`) inside the PTY, because it behaves
//! identically on every platform. Shell
//! commands are only used where the shell itself is what is being tested.
//!
//! Two rules keep the suite reliable:
//!
//! - **Nothing waits forever.** Every test body runs under [`guarded`], and
//!   every wait has a timeout, so a bug shows up as a failure and never as a
//!   hung CI job.
//! - **Content, not bytes, on Windows.** ConPTY renders the console screen
//!   instead of passing bytes through, so assertions search the visible text
//!   (escape sequences stripped). Exact byte comparisons are made on Unix,
//!   where the PTY is transparent.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use termx_core::pty::{
    ExitStatus, OUTPUT_BUFFER_CAPACITY, Pty, PtyCommand, PtyError, PtyManager, PtyOutput, PtySize,
    RecvTimeoutError,
};

/// Upper bound for any single wait. Generous, because CI machines are slow
/// (starting PowerShell alone can take several seconds).
const TIMEOUT: Duration = Duration::from_secs(60);

/// Upper bound for a whole test.
const TEST_TIMEOUT: Duration = Duration::from_secs(180);

/// Runs a test body on its own thread and fails if it does not finish in
/// time, so that a deadlock (for example in `Drop`) cannot hang the suite.
fn guarded(test: impl FnOnce() + Send + 'static) {
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
/// Cargo builds examples next to the test binaries
/// (`target/<profile>/examples`) whenever `cargo test` runs without selecting
/// targets. When a single target is selected (`cargo test --test pty`) the
/// example may be missing, in which case it is built here.
fn helper_path() -> &'static Path {
    static HELPER: OnceLock<PathBuf> = OnceLock::new();
    HELPER.get_or_init(|| {
        let test_binary = std::env::current_exe().expect("cannot locate the test binary");
        let profile_dir = test_binary
            .parent()
            .and_then(Path::parent)
            .expect("the test binary is not in target/<profile>/deps");
        let helper = profile_dir
            .join("examples")
            .join(format!("pty-test-helper{}", std::env::consts::EXE_SUFFIX));

        if !helper.is_file() {
            let mut cargo = std::process::Command::new(env!("CARGO"));
            cargo.args([
                "build",
                "--package",
                "termx-core",
                "--example",
                "pty-test-helper",
            ]);
            if profile_dir
                .file_name()
                .is_some_and(|name| name == "release")
            {
                cargo.arg("--release");
            }
            let status = cargo.status().expect("cannot run cargo");
            assert!(status.success(), "building the test helper failed");
        }
        assert!(
            helper.is_file(),
            "the test helper was not found at {}",
            helper.display()
        );
        helper
    })
}

/// A command running the test helper in the given mode.
fn helper(args: &[&str]) -> PtyCommand {
    PtyCommand::new(helper_path()).args(args)
}

/// Creates an empty directory for one test, inside Cargo's directory for
/// integration test files.
fn scratch_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("pty-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("cannot create the scratch directory");
    dir
}

/// Removes escape sequences and carriage returns, leaving the text a user
/// would see.
fn visible_text(raw: &[u8]) -> String {
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
fn numbered_lines(visible: &str, text: &str) -> BTreeSet<usize> {
    let marker = format!(":{text}");
    visible
        .match_indices(&marker)
        .filter_map(|(position, _)| {
            let digits = visible.get(position.checked_sub(7)?..position)?;
            digits.parse().ok()
        })
        .collect()
}

/// A PTY together with everything it has printed so far.
struct Session {
    pty: Pty,
    output: PtyOutput,
    raw: Vec<u8>,
}

impl Session {
    fn spawn(command: &PtyCommand) -> Self {
        Self::spawn_sized(command, PtySize::default())
    }

    fn spawn_sized(command: &PtyCommand, size: PtySize) -> Self {
        let (pty, output) = Pty::spawn(command, size).expect("failed to spawn");
        Self {
            pty,
            output,
            raw: Vec::new(),
        }
    }

    fn text(&self) -> String {
        visible_text(&self.raw)
    }

    /// Receives one chunk, waiting at most `timeout`. Returns `false` once
    /// the stream has ended.
    fn pump(&mut self, timeout: Duration) -> bool {
        match self.output.recv_timeout(timeout) {
            Ok(chunk) => {
                assert!(!chunk.is_empty(), "received an empty chunk");
                let previous = self.raw.len();
                self.raw.extend_from_slice(&chunk);
                self.answer_terminal_queries(previous);
                true
            }
            Err(RecvTimeoutError::Timeout) => true,
            Err(RecvTimeoutError::Closed) => false,
        }
    }

    /// Plays the part of the terminal emulator for the queries ConPTY sends
    /// when it starts, which it may wait on before producing any output.
    fn answer_terminal_queries(&mut self, previous: usize) {
        if !cfg!(windows) {
            return;
        }
        // (query, answer): cursor position report and primary device
        // attributes.
        for (query, answer) in [
            (&b"\x1b[6n"[..], &b"\x1b[1;1R"[..]),
            (&b"\x1b[c"[..], &b"\x1b[?1;2c"[..]),
        ] {
            // Start early enough to catch a query split across two chunks,
            // but late enough that every match contains new bytes.
            let start = previous.saturating_sub(query.len() - 1);
            let queries = self.raw[start..]
                .windows(query.len())
                .filter(|window| *window == query)
                .count();
            for _ in 0..queries {
                // The process may be gone already; that is not an error.
                let _ = self.pty.write(answer);
            }
        }
    }

    /// Reads output until `needle` is visible.
    fn wait_for(&mut self, needle: &str) {
        let deadline = Instant::now() + TIMEOUT;
        while !self.text().contains(needle) {
            let open = self.pump(Duration::from_millis(100));
            assert!(
                open || self.text().contains(needle),
                "the output ended before {needle:?} appeared; got: {:?}",
                self.text()
            );
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {needle:?}; got: {:?}",
                self.text()
            );
        }
    }

    /// Reads output until the stream ends.
    fn read_to_end(&mut self) {
        let deadline = Instant::now() + TIMEOUT;
        while self.pump(Duration::from_millis(100)) {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for the end of the output ({} bytes so far)",
                self.raw.len()
            );
        }
    }

    fn exit_status(&self) -> ExitStatus {
        self.pty
            .wait_timeout(TIMEOUT)
            .expect("timed out waiting for the process to exit")
    }

    /// Reads all the output, then returns the visible text and exit status.
    fn finish(mut self) -> (String, ExitStatus) {
        self.read_to_end();
        let status = self.exit_status();
        (self.text(), status)
    }
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

#[test]
fn spawned_command_output_is_captured() {
    guarded(|| {
        let session = Session::spawn(&helper(&["print", "hello-from-the-pty"]));
        assert!(session.pty.process_id().is_some());

        let (text, status) = session.finish();

        assert!(text.contains("hello-from-the-pty"), "got: {text:?}");
        assert!(status.success(), "{status}");
    });
}

#[test]
fn output_stream_reports_its_end() {
    guarded(|| {
        let mut session = Session::spawn(&helper(&["print", "bye"]));
        session.read_to_end();

        assert_eq!(session.output.recv(), None);
        assert_eq!(
            session.output.recv_timeout(Duration::ZERO),
            Err(RecvTimeoutError::Closed)
        );
        assert_eq!(session.output.next(), None);
    });
}

#[test]
fn utf8_output_survives_chunking() {
    guarded(|| {
        // Two, three and four byte characters, without spaces (ConPTY may
        // replace runs of spaces with cursor movements).
        const TEXT: &str = "h\u{e9}llo-w\u{f6}rld-\u{f1}-\u{65e5}\u{672c}\u{8a9e}-\u{d55c}\u{ae00}-\u{1f680}\u{1f389}-\u{2713}";
        const LINES: usize = 4_000;

        let mut session = Session::spawn(&helper(&["repeat", &LINES.to_string(), TEXT]));
        session.read_to_end();
        assert!(session.exit_status().success());

        // The reassembled bytes are valid UTF-8 even though chunk boundaries
        // are arbitrary.
        let decoded =
            std::str::from_utf8(&session.raw).expect("the reassembled output is not valid UTF-8");
        assert!(!decoded.contains('\u{fffd}'), "a character was mangled");

        let text = session.text();
        let lines = numbered_lines(&text, TEXT);
        assert_eq!(lines.len(), LINES, "some lines were lost or damaged");
        assert_eq!(lines.last(), Some(&(LINES - 1)));
        assert!(text.contains("END-OF-OUTPUT"));

        // A Unix PTY is transparent apart from turning LF into CRLF, so the
        // bytes must match exactly.
        #[cfg(unix)]
        {
            let mut expected = String::new();
            for index in 0..LINES {
                expected.push_str(&format!("{index:07}:{TEXT}\r\n"));
            }
            expected.push_str("END-OF-OUTPUT\r\n");
            assert!(session.raw == expected.as_bytes(), "the bytes were altered");
        }
    });
}

#[test]
fn large_burst_is_fully_delivered_to_a_slow_consumer() {
    guarded(|| {
        // 100 visible characters per line, 4 MB in total: several times the
        // output buffer, so the process must be throttled and resumed.
        const LINES: usize = 40_000;
        let text = "x".repeat(92);
        const { assert!(LINES * 100 > 3 * OUTPUT_BUFFER_CAPACITY) };

        let mut session = Session::spawn_sized(
            &helper(&["repeat", &LINES.to_string(), &text]),
            PtySize::new(120, 30),
        );

        // Do not read at all for a while: the buffer fills up and the
        // process blocks, since its output cannot fit anywhere.
        thread::sleep(Duration::from_millis(500));
        #[cfg(unix)]
        assert_eq!(session.pty.try_wait(), None, "the output was not throttled");

        // Then read slowly.
        let deadline = Instant::now() + 2 * TIMEOUT;
        let mut largest_chunk = 0;
        loop {
            let before = session.raw.len();
            if !session.pump(Duration::from_millis(100)) {
                break;
            }
            largest_chunk = largest_chunk.max(session.raw.len() - before);
            thread::sleep(Duration::from_millis(10));
            assert!(Instant::now() < deadline, "timed out reading the burst");
        }
        assert!(session.exit_status().success());

        // Backpressure bounds what is buffered: no chunk can be larger than
        // the buffer plus one read.
        assert!(
            largest_chunk <= OUTPUT_BUFFER_CAPACITY + termx_core::pty::READ_BUFFER_SIZE,
            "a chunk of {largest_chunk} bytes exceeds the buffer bound"
        );

        let visible = session.text();
        let lines = numbered_lines(&visible, &text);
        assert_eq!(lines.len(), LINES, "some lines were lost");
        assert_eq!(lines.first(), Some(&0));
        assert_eq!(lines.last(), Some(&(LINES - 1)));
        assert!(visible.contains("END-OF-OUTPUT"));

        #[cfg(unix)]
        assert_eq!(
            session.raw.len(),
            LINES * 102 + "END-OF-OUTPUT\r\n".len(),
            "the number of bytes delivered is wrong"
        );
    });
}

// ---------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------

#[test]
fn written_input_reaches_the_process() {
    guarded(|| {
        let mut session = Session::spawn(&helper(&["echo"]));
        session.wait_for("ready");

        session.pty.write(b"ping-123\r").unwrap();
        session.wait_for("got:ping-123");

        // Input written in several pieces is still one line.
        session.pty.write(b"sp").unwrap();
        session.pty.write(b"lit\r").unwrap();
        session.wait_for("got:split");

        session.pty.write(b"quit\r").unwrap();
        let (_, status) = session.finish();
        assert!(status.success(), "{status}");
    });
}

// ---------------------------------------------------------------------------
// Exit status
// ---------------------------------------------------------------------------

#[test]
fn exit_code_is_propagated() {
    guarded(|| {
        for code in [0u32, 1, 3, 42] {
            let mut session = Session::spawn(&helper(&["exit", &code.to_string()]));
            session.read_to_end();
            let status = session.exit_status();

            assert_eq!(status.code(), Some(code));
            assert_eq!(status.success(), code == 0);
            assert_eq!(status.signal(), None);
            // The status stays available, from every accessor.
            assert_eq!(session.pty.try_wait(), Some(status.clone()));
            assert_eq!(session.pty.wait(), status);
        }
    });
}

#[test]
fn waiting_times_out_while_the_process_runs() {
    guarded(|| {
        let mut session = Session::spawn(&helper(&["sleep"]));
        session.wait_for("ready");

        assert_eq!(session.pty.try_wait(), None);
        assert_eq!(session.pty.wait_timeout(Duration::from_millis(50)), None);
        assert_eq!(session.pty.wait_timeout(Duration::ZERO), None);
    });
}

#[cfg(unix)]
#[test]
fn death_by_signal_is_reported() {
    guarded(|| {
        let session = Session::spawn(&PtyCommand::new("/bin/sh").args(["-c", "kill -TERM $$"]));
        let status = session.exit_status();

        assert!(!status.success());
        assert_eq!(status.code(), None);
        assert!(status.signal().is_some(), "{status}");
    });
}

// ---------------------------------------------------------------------------
// Resize
// ---------------------------------------------------------------------------

/// A command that prints `ready`, then prints the terminal size as
/// `size=<rows>x<cols>` each time it reads a line.
fn size_reporter() -> PtyCommand {
    if cfg!(windows) {
        PtyCommand::new("powershell.exe").args([
            "-NoLogo",
            "-NoProfile",
            "-Command",
            "Write-Output 'ready'; \
             while ($null -ne [Console]::ReadLine()) { \
                 Write-Output ('size=' + [Console]::WindowHeight + 'x' + [Console]::WindowWidth) \
             }",
        ])
    } else {
        PtyCommand::new("/bin/sh").args([
            "-c",
            "echo ready; while read line; do set -- $(stty size); echo \"size=$1x$2\"; done",
        ])
    }
}

#[test]
fn resize_is_observed_by_the_process() {
    guarded(|| {
        let mut session = Session::spawn_sized(&size_reporter(), PtySize::new(80, 24));
        assert_eq!(session.pty.size(), PtySize::new(80, 24));
        session.wait_for("ready");

        session.pty.write(b"\r").unwrap();
        session.wait_for("size=24x80");

        session.pty.resize(PtySize::new(132, 43)).unwrap();
        assert_eq!(session.pty.size(), PtySize::new(132, 43));

        // The Windows console applies a resize asynchronously, so ask until
        // the new size is reported.
        let deadline = Instant::now() + TIMEOUT;
        while !session.text().contains("size=43x132") {
            session.pty.write(b"\r").unwrap();
            let asked = Instant::now();
            while asked.elapsed() < Duration::from_millis(500)
                && !session.text().contains("size=43x132")
            {
                assert!(
                    session.pump(Duration::from_millis(50)),
                    "the process exited"
                );
            }
            assert!(
                Instant::now() < deadline,
                "the new size was never observed; got: {:?}",
                session.text()
            );
        }

        let invalid = session.pty.resize(PtySize::new(0, 43)).unwrap_err();
        assert!(matches!(
            invalid,
            PtyError::InvalidSize { cols: 0, rows: 43 }
        ));
        assert_eq!(session.pty.size(), PtySize::new(132, 43));
    });
}

// ---------------------------------------------------------------------------
// Working directory and environment
// ---------------------------------------------------------------------------

#[test]
fn working_directory_and_environment_are_applied() {
    guarded(|| {
        let dir = scratch_dir("cwd").canonicalize().unwrap();
        let command = helper(&[
            "report",
            "TERMX_TEST_ONE",
            "TERMX_TEST_TWO",
            "TERMX_TEST_UNSET",
            "TERM",
            "COLORTERM",
            "TERM_PROGRAM",
            "TERM_PROGRAM_VERSION",
        ])
        .cwd(&dir)
        .env("TERMX_TEST_ONE", "first-value")
        .envs([("TERMX_TEST_TWO", "second-value")]);

        // Wide enough that the path is never wrapped by the Windows console.
        let mut session = Session::spawn_sized(&command, PtySize::new(500, 24));
        session.wait_for("done");
        let text = session.text();

        for expected in [
            format!("cwd={}", dir.display()),
            "TERMX_TEST_ONE=first-value".to_owned(),
            "TERMX_TEST_TWO=second-value".to_owned(),
            "TERMX_TEST_UNSET is unset".to_owned(),
            "TERM=xterm-256color".to_owned(),
            "COLORTERM=truecolor".to_owned(),
            "TERM_PROGRAM=termx".to_owned(),
            format!("TERM_PROGRAM_VERSION={}", termx_core::version()),
        ] {
            assert!(text.contains(&expected), "missing {expected:?} in {text:?}");
        }
    });
}

#[test]
fn environment_is_inherited_and_defaults_can_be_overridden() {
    guarded(|| {
        // PATH exists on every platform and is inherited from the test run.
        let command = helper(&["report", "PATH", "TERM"]).env("TERM", "dumb");
        let mut session = Session::spawn_sized(&command, PtySize::new(500, 24));
        session.wait_for("done");
        let text = session.text();

        assert!(text.contains("TERM=dumb"), "got: {text:?}");
        assert!(text.contains("PATH="), "got: {text:?}");
        assert!(!text.contains("PATH is unset"), "got: {text:?}");
    });
}

// ---------------------------------------------------------------------------
// Kill and drop
// ---------------------------------------------------------------------------

#[test]
fn kill_terminates_a_long_running_process() {
    guarded(|| {
        let mut session = Session::spawn(&helper(&["sleep"]));
        session.wait_for("ready");

        session.pty.kill().unwrap();
        let status = session.exit_status();
        assert!(!status.success(), "{status}");
        if cfg!(unix) {
            assert!(status.signal().is_some(), "{status}");
        }

        // The output stream ends, and the handle reports the process as gone.
        session.read_to_end();
        session.pty.kill().expect("killing twice is not an error");
        assert!(matches!(session.pty.write(b"x"), Err(PtyError::Exited)));
        assert!(matches!(
            session.pty.resize(PtySize::new(100, 30)),
            Err(PtyError::Exited)
        ));
    });
}

/// Reads the counter the helper's `heartbeat` mode keeps writing.
fn read_heartbeat(file: &Path) -> Option<u64> {
    std::fs::read_to_string(file).ok()?.trim().parse().ok()
}

#[test]
fn dropping_the_handle_kills_the_process() {
    guarded(|| {
        let file = scratch_dir("heartbeat").join("beat");
        let mut session = Session::spawn(&helper(&["heartbeat", &file.to_string_lossy()]));
        session.wait_for("ready");

        // The process is alive: its counter keeps increasing.
        let deadline = Instant::now() + TIMEOUT;
        while read_heartbeat(&file).is_none_or(|beat| beat < 3) {
            assert!(
                Instant::now() < deadline,
                "the helper never started beating"
            );
            thread::sleep(Duration::from_millis(10));
        }

        let Session {
            pty, mut output, ..
        } = session;
        drop(pty);

        // Dropping returns only once the process is gone, so the counter
        // must not change any more.
        let after_drop = std::fs::read_to_string(&file).unwrap();
        thread::sleep(Duration::from_millis(500));
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            after_drop,
            "the process survived its handle"
        );

        // The output stream of a dropped handle ends.
        let deadline = Instant::now() + TIMEOUT;
        loop {
            match output.recv_timeout(Duration::from_millis(100)) {
                Err(RecvTimeoutError::Closed) => break,
                _ => assert!(Instant::now() < deadline, "the output never ended"),
            }
        }
    });
}

#[test]
fn dropping_the_handle_does_not_wait_for_a_stalled_consumer() {
    guarded(|| {
        // The process produces far more than the buffer holds and nobody
        // reads: the reader thread is blocked when the handle is dropped.
        let text = "x".repeat(92);
        let (pty, mut output) =
            Pty::spawn(&helper(&["repeat", "40000", &text]), PtySize::new(120, 30)).unwrap();
        thread::sleep(Duration::from_millis(500));

        let started = Instant::now();
        drop(pty);
        assert!(
            started.elapsed() < TIMEOUT,
            "drop took {:?}",
            started.elapsed()
        );

        // What was buffered is still delivered, then the stream ends.
        let deadline = Instant::now() + TIMEOUT;
        loop {
            match output.recv_timeout(Duration::from_millis(100)) {
                Err(RecvTimeoutError::Closed) => break,
                _ => assert!(Instant::now() < deadline, "the output never ended"),
            }
        }
    });
}

#[test]
fn dropping_the_output_does_not_block_the_process() {
    guarded(|| {
        // Once the consumer goes away the output is discarded, so a process
        // printing far more than the buffer holds still runs to completion.
        let text = "x".repeat(92);
        let mut session =
            Session::spawn_sized(&helper(&["repeat", "40000", &text]), PtySize::new(120, 30));
        session.wait_for("0000000:");

        let Session { pty, output, .. } = session;
        drop(output);

        let status = pty.wait_timeout(TIMEOUT).expect("the process was blocked");
        assert!(status.success(), "{status}");
    });
}

#[cfg(unix)]
#[test]
fn background_job_holding_the_terminal_does_not_keep_the_session_open() {
    guarded(|| {
        // The job ignores the hang-up sent when the shell exits and keeps the
        // terminal open for 20 seconds, so end of file is not reported.
        let command =
            PtyCommand::new("/bin/sh").args(["-c", "(trap '' HUP; sleep 20) & echo shell-is-done"]);
        let started = Instant::now();
        let (text, status) = Session::spawn(&command).finish();

        assert!(text.contains("shell-is-done"), "got: {text:?}");
        assert!(status.success(), "{status}");
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "the session waited for the background job ({:?})",
            started.elapsed()
        );
    });
}

// ---------------------------------------------------------------------------
// Shells
// ---------------------------------------------------------------------------

#[test]
fn default_shell_starts_and_runs_commands() {
    guarded(|| {
        // An empty home directory keeps the developer's own shell
        // configuration out of the test. The empty `.zshrc` stops zsh from
        // starting its first-run wizard.
        let home = scratch_dir("home");
        std::fs::write(home.join(".zshrc"), "").unwrap();
        let mut command = PtyCommand::default_shell().cwd(&home);
        if cfg!(unix) {
            command = command.env("HOME", &home).env("ZDOTDIR", &home);
        }

        let mut session = Session::spawn(&command);
        if cfg!(windows) {
            // Wait for the PowerShell prompt (`PS C:\...>`), in case keys
            // typed while it starts are not kept.
            session.wait_for(">");
        }
        session.pty.write(b"exit 7\r").unwrap();

        let (text, status) = session.finish();
        assert_eq!(status.code(), Some(7), "{status}; output: {text:?}");
    });
}

#[cfg(unix)]
#[test]
fn login_shell_is_started_with_a_dash_in_argv0() {
    guarded(|| {
        let home = scratch_dir("login-home");
        let command = PtyCommand::login_shell("/bin/sh")
            .cwd(&home)
            .env("HOME", &home)
            .env("ENV", "");
        assert!(command.is_login_shell());

        let session = Session::spawn(&command);
        session.pty.write(b"echo \"argv0=<$0>\"; exit 5\r").unwrap();

        let (text, status) = session.finish();
        assert!(text.contains("argv0=<-sh>"), "got: {text:?}");
        assert_eq!(status.code(), Some(5), "{status}");
    });
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[test]
fn spawning_a_missing_program_fails() {
    guarded(|| {
        let error = Pty::spawn(
            &PtyCommand::new("termx-this-program-does-not-exist"),
            PtySize::default(),
        )
        .unwrap_err();

        assert!(
            matches!(&error, PtyError::Spawn { program, .. } if program == "termx-this-program-does-not-exist"),
            "unexpected error: {error}"
        );
    });
}

#[test]
fn invalid_requests_are_rejected_before_spawning() {
    guarded(|| {
        let command = helper(&["print", "never-runs"]);

        let error = Pty::spawn(&command, PtySize::new(0, 24)).unwrap_err();
        assert!(
            matches!(error, PtyError::InvalidSize { cols: 0, rows: 24 }),
            "{error}"
        );

        let missing = scratch_dir("missing-parent").join("missing");
        let error = Pty::spawn(&command.clone().cwd(&missing), PtySize::default()).unwrap_err();
        assert!(
            matches!(&error, PtyError::InvalidWorkingDirectory(dir) if *dir == missing),
            "{error}"
        );

        let error = Pty::spawn(&PtyCommand::new(""), PtySize::default()).unwrap_err();
        assert!(matches!(error, PtyError::EmptyProgram), "{error}");
    });
}

// ---------------------------------------------------------------------------
// Manager
// ---------------------------------------------------------------------------

/// Reads from `output` until `needle` is visible, answering nothing: only
/// suitable for output that does not depend on terminal queries.
fn read_until(pty: &Pty, output: &mut PtyOutput, needle: &str) -> String {
    let mut raw = Vec::new();
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let text = visible_text(&raw);
        if text.contains(needle) {
            return text;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {needle:?}; got: {text:?}"
        );
        match output.recv_timeout(Duration::from_millis(100)) {
            Ok(chunk) => {
                if cfg!(windows) && chunk.windows(4).any(|window| window == b"\x1b[6n") {
                    let _ = pty.write(b"\x1b[1;1R");
                }
                raw.extend_from_slice(&chunk);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Closed) => {
                panic!("the output ended before {needle:?}; got: {text:?}")
            }
        }
    }
}

#[test]
fn manager_drives_independent_sessions() {
    guarded(|| {
        let manager = PtyManager::new();
        let (first, mut first_output) = manager
            .spawn(&helper(&["echo"]), PtySize::default())
            .unwrap();
        let (second, mut second_output) = manager
            .spawn(&helper(&["echo"]), PtySize::default())
            .unwrap();

        assert_ne!(first, second);
        assert_eq!(manager.ids(), vec![first, second]);
        assert_eq!(manager.len(), 2);

        let first_pty = manager.get(first).unwrap();
        let second_pty = manager.get(second).unwrap();
        read_until(&first_pty, &mut first_output, "ready");
        read_until(&second_pty, &mut second_output, "ready");

        // Input goes to the session it was addressed to, and only there.
        manager.write(first, b"for-first\r").unwrap();
        manager.write(second, b"for-second\r").unwrap();
        let first_text = read_until(&first_pty, &mut first_output, "got:for-first");
        let second_text = read_until(&second_pty, &mut second_output, "got:for-second");
        assert!(!first_text.contains("for-second"), "got: {first_text:?}");
        assert!(!second_text.contains("for-first"), "got: {second_text:?}");

        manager.resize(first, PtySize::new(100, 30)).unwrap();
        assert_eq!(first_pty.size(), PtySize::new(100, 30));
        assert_eq!(second_pty.size(), PtySize::default());

        // Killing leaves the session registered, with its exit status.
        manager.kill(first).unwrap();
        let status = first_pty
            .wait_timeout(TIMEOUT)
            .expect("the process was not killed");
        assert!(!status.success());
        assert_eq!(manager.len(), 2);
        assert!(matches!(manager.write(first, b"x"), Err(PtyError::Exited)));

        // Removing the last handle of a session kills its process.
        drop(second_pty);
        drop(manager.remove(second));
        assert_eq!(manager.ids(), vec![first]);
        assert!(matches!(
            manager.write(second, b"x"),
            Err(PtyError::UnknownSession(id)) if id == second
        ));
        let deadline = Instant::now() + TIMEOUT;
        loop {
            match second_output.recv_timeout(Duration::from_millis(100)) {
                Err(RecvTimeoutError::Closed) => break,
                _ => assert!(
                    Instant::now() < deadline,
                    "the removed session kept running"
                ),
            }
        }
    });
}

#[test]
fn dropping_the_manager_kills_its_sessions() {
    guarded(|| {
        let file = scratch_dir("manager-heartbeat").join("beat");
        let manager = PtyManager::new();
        let (id, mut output) = manager
            .spawn(
                &helper(&["heartbeat", &file.to_string_lossy()]),
                PtySize::default(),
            )
            .unwrap();
        read_until(&manager.get(id).unwrap(), &mut output, "ready");
        let deadline = Instant::now() + TIMEOUT;
        while read_heartbeat(&file).is_none() {
            assert!(
                Instant::now() < deadline,
                "the helper never started beating"
            );
            thread::sleep(Duration::from_millis(10));
        }

        drop(manager);

        let after_drop = std::fs::read_to_string(&file).unwrap();
        thread::sleep(Duration::from_millis(500));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), after_drop);
    });
}
