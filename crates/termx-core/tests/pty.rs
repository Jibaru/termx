//! Integration tests of the PTY layer against real pseudo-terminals.
//!
//! The helpers and the rules that keep the suite reliable are in
//! `tests/common`: tests run the `pty-test-helper` example inside the PTY
//! (shell commands are only used where the shell itself is what is being
//! tested), nothing waits forever, and on Windows the visible text is
//! compared, not the bytes.
//!
//! The tests never answer the queries the Windows console sends when it
//! starts: the whole suite doubles as a check that output flows when no
//! terminal emulator is attached.

mod common;

use std::path::PathBuf;
use std::sync::{Arc, Barrier, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use common::{TIMEOUT, guarded, helper, numbered_lines, read_heartbeat, scratch_dir, visible_text};
use termx_core::pty::{
    ExitStatus, OUTPUT_BUFFER_CAPACITY, Pty, PtyCommand, PtyError, PtyManager, PtyOutput, PtySize,
    RecvTimeoutError,
};

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
                self.raw.extend_from_slice(&chunk);
                true
            }
            Err(RecvTimeoutError::Timeout) => true,
            Err(RecvTimeoutError::Closed) => false,
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
fn output_can_be_consumed_without_a_blocking_thread() {
    guarded(|| {
        // One loop serves two terminals, woken up by their notifications and
        // never blocking on either of them.
        let (events_tx, events_rx) = mpsc::channel::<usize>();
        let mut terminals: Vec<(Pty, PtyOutput, Vec<u8>, bool)> =
            ["first-terminal", "second-terminal"]
                .iter()
                .enumerate()
                .map(|(index, text)| {
                    let (pty, mut output) =
                        Pty::spawn(&helper(&["repeat", "500", text]), PtySize::default()).unwrap();
                    let events = events_tx.clone();
                    output.on_readable(move || {
                        let _ = events.send(index);
                    });
                    (pty, output, Vec::new(), false)
                })
                .collect();

        while terminals.iter().any(|(_, _, _, ended)| !ended) {
            let index = events_rx
                .recv_timeout(TIMEOUT)
                .expect("no notification although a stream is still open");
            let (_, output, raw, ended) = &mut terminals[index];
            loop {
                match output.recv_timeout(Duration::ZERO) {
                    Ok(chunk) => raw.extend_from_slice(&chunk),
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Closed) => {
                        *ended = true;
                        break;
                    }
                }
            }
        }

        for ((pty, _, raw, _), text) in terminals.iter().zip(["first-terminal", "second-terminal"])
        {
            assert!(pty.wait_timeout(TIMEOUT).unwrap().success());
            let visible = visible_text(raw);
            assert_eq!(numbered_lines(&visible, text).len(), 500, "lines were lost");
            assert!(visible.contains("END-OF-OUTPUT"));
        }
    });
}

#[test]
fn panicking_readable_callback_does_not_wedge_the_session() {
    guarded(|| {
        // More output than the buffer holds: if the reader thread died with
        // the callback, the process would block and never exit.
        let text = "x".repeat(92);
        let (pty, mut output) =
            Pty::spawn(&helper(&["repeat", "40000", &text]), PtySize::new(120, 30)).unwrap();
        output.on_readable(|| panic!("deliberate panic in a test callback"));

        let mut raw = Vec::new();
        let deadline = Instant::now() + TIMEOUT;
        loop {
            match output.recv_timeout(Duration::from_millis(100)) {
                Ok(chunk) => raw.extend_from_slice(&chunk),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Closed) => break,
            }
            assert!(Instant::now() < deadline, "the stream never ended");
        }

        assert!(visible_text(&raw).contains("END-OF-OUTPUT"));
        let status = pty.try_wait().expect("the exit status was never published");
        assert!(status.success(), "{status}");
    });
}

#[test]
fn dropping_the_handle_does_not_run_the_readable_callback() {
    guarded(|| {
        // The callback needs a lock that the dropping thread holds. Calling
        // it while the handle is dropped would deadlock.
        let gate = Arc::new(Mutex::new(()));
        let (pty, mut output) = Pty::spawn(&helper(&["sleep"]), PtySize::default()).unwrap();
        read_until(&mut output, "ready");
        output.on_readable({
            let gate = Arc::clone(&gate);
            move || drop(gate.lock())
        });

        let held = gate.lock().unwrap();
        drop(pty);
        drop(held);

        assert_eq!(output.recv(), None);
    });
}

#[test]
fn readable_callback_holding_the_handle_is_released_when_the_stream_ends() {
    guarded(|| {
        // The pitfall the documentation warns about: the callback keeps the
        // handle alive, and the handle keeps the callback. The cycle is
        // broken when the stream ends.
        let (pty, mut output) = Pty::spawn(&helper(&["print", "bye"]), PtySize::default()).unwrap();
        let pty = Arc::new(pty);
        output.on_readable({
            let pty = Arc::clone(&pty);
            move || drop(pty.try_wait())
        });

        let deadline = Instant::now() + TIMEOUT;
        loop {
            match output.recv_timeout(Duration::from_millis(100)) {
                Err(RecvTimeoutError::Closed) => break,
                _ => assert!(Instant::now() < deadline, "the stream never ended"),
            }
        }

        // The reader thread releases the callback just after announcing the
        // end, so give it a moment.
        while Arc::strong_count(&pty) > 1 {
            assert!(
                Instant::now() < deadline,
                "the callback still holds the handle"
            );
            thread::sleep(Duration::from_millis(10));
        }
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

/// Whether the pseudo-console of this version of Windows sends the cursor
/// position query when it starts, observed on a PTY that bypasses
/// `termx_core` so that nothing answers or removes it.
#[cfg(windows)]
fn console_sends_startup_query() -> bool {
    use std::io::Read;

    let pair = portable_pty::native_pty_system()
        .openpty(portable_pty::PtySize::default())
        .expect("cannot open a raw pseudo-console");
    let mut command = portable_pty::CommandBuilder::new(common::helper_path());
    command.arg("sleep");
    let mut child = pair
        .slave
        .spawn_command(command)
        .expect("cannot spawn in the raw pseudo-console");
    let mut reader = pair
        .master
        .try_clone_reader()
        .expect("cannot read the console");

    let (chunks_tx, chunks_rx) = mpsc::channel();
    let reading = thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        while let Ok(n) = reader.read(&mut buffer) {
            if n == 0 || chunks_tx.send(buffer[..n].to_vec()).is_err() {
                break;
            }
        }
    });

    // The query comes first, so a few seconds of output are plenty.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut raw = Vec::new();
    let found = loop {
        if raw.windows(4).any(|window| window == b"\x1b[6n") {
            break true;
        }
        match chunks_rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(chunk) => raw.extend_from_slice(&chunk),
            Err(_) => break false,
        }
    };

    let _ = child.kill();
    let _ = child.wait();
    // Closing the console ends the read.
    drop(pair);
    drop(chunks_rx);
    let _ = reading.join();
    found
}

#[cfg(windows)]
#[test]
fn console_startup_query_is_answered_without_a_terminal() {
    use std::io::Write;

    guarded(|| {
        let console_asks = console_sends_startup_query();

        // Nobody plays the terminal emulator here: nothing is ever written
        // to the PTY. The output must flow all the same.
        let started = Instant::now();
        let mut session = Session::spawn(&helper(&["print", "nobody-answers"]));
        session.wait_for("nobody-answers");
        let first_output = started.elapsed();
        session.read_to_end();
        assert!(session.exit_status().success());

        // The question was answered by the PTY layer, so it must not reach
        // the consumer, whose terminal emulator would answer it again.
        assert!(
            !session.raw.windows(4).any(|window| window == b"\x1b[6n"),
            "the start-up query was delivered: {:?}",
            String::from_utf8_lossy(&session.raw)
        );
        assert!(
            first_output < Duration::from_secs(20),
            "the output stalled for {first_output:?}"
        );

        // Written straight to stderr so that it shows in the CI log: whether
        // this console asks decides how much the assertions above prove.
        let _ = writeln!(
            std::io::stderr(),
            "note: this console sends the start-up query: {console_asks}; \
             first output without a terminal after {first_output:?}"
        );
    });
}

// ---------------------------------------------------------------------------
// Concurrency
// ---------------------------------------------------------------------------

#[test]
fn many_threads_can_spawn_at_the_same_time() {
    guarded(|| {
        // Restoring many sessions at start-up spawns them in parallel, which
        // used to fail at random on macOS.
        const THREADS: usize = 48;
        const ROUNDS: usize = 3;
        termx_core::pty::raise_open_file_limit().expect("cannot raise the open file limit");

        for round in 0..ROUNDS {
            let barrier = Arc::new(Barrier::new(THREADS));
            let spawners: Vec<_> = (0..THREADS)
                .map(|index| {
                    let barrier = Arc::clone(&barrier);
                    thread::spawn(move || {
                        let marker = format!("spawned-{round}-{index}");
                        let command = helper(&["print", &marker]);
                        barrier.wait();
                        let (pty, output) = Pty::spawn(&command, PtySize::default())
                            .map_err(|error| format!("{marker}: {error}"))?;
                        let mut session = Session {
                            pty,
                            output,
                            raw: Vec::new(),
                        };
                        session.read_to_end();
                        let status = session.exit_status();
                        if status.success() && session.text().contains(&marker) {
                            Ok(())
                        } else {
                            Err(format!("{marker}: {status}; got {:?}", session.text()))
                        }
                    })
                })
                .collect();

            let failures: Vec<String> = spawners
                .into_iter()
                .filter_map(|spawner| spawner.join().expect("a spawner panicked").err())
                .collect();
            assert!(
                failures.is_empty(),
                "{} of {THREADS} concurrent spawns failed: {failures:#?}",
                failures.len()
            );
        }
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
fn exit_status_is_available_when_the_output_ends() {
    guarded(|| {
        // The end of the stream and the exit status come from two threads;
        // the stream must not win the race.
        for round in 0..100 {
            let mut session = Session::spawn(&helper(&["exit", "9"]));
            session.read_to_end();

            let status = session.pty.try_wait();
            assert_eq!(
                status.as_ref().and_then(ExitStatus::code),
                Some(9),
                "round {round}: the stream ended before the exit status was known"
            );
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

#[test]
fn denied_variables_are_not_inherited() {
    guarded(|| {
        // Cargo sets these two for the test run, so they are inherited
        // unless denied. The session markers are only set when the suite
        // itself runs under tmux, Claude Code or a package script, but must
        // be absent either way.
        let command = helper(&[
            "report",
            "CARGO_PKG_NAME",
            "CARGO_PKG_VERSION",
            "CARGO_MANIFEST_DIR",
            "TMUX",
            "CLAUDECODE",
            "npm_config_user_agent",
        ])
        .env_remove("CARGO_PKG_*")
        .env("CARGO_PKG_VERSION", "set-explicitly");
        let mut session = Session::spawn_sized(&command, PtySize::new(500, 24));
        session.wait_for("done");
        let text = session.text();

        for expected in [
            "CARGO_PKG_NAME is unset",
            "CARGO_PKG_VERSION=set-explicitly",
            "CARGO_MANIFEST_DIR=",
            "TMUX is unset",
            "CLAUDECODE is unset",
            "npm_config_user_agent is unset",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in {text:?}");
        }

        // Without the extra pattern the variable is inherited.
        let mut session = Session::spawn_sized(
            &helper(&["report", "CARGO_PKG_NAME"]),
            PtySize::new(500, 24),
        );
        session.wait_for("done");
        assert!(session.text().contains("CARGO_PKG_NAME=termx-core"));
    });
}

#[cfg(unix)]
#[test]
fn process_gets_a_utf8_locale_when_none_is_inherited() {
    guarded(|| {
        let command = helper(&["report", "LANG", "LC_ALL", "LC_CTYPE"])
            .env_remove("LANG")
            .env_remove("LC_*");
        let mut session = Session::spawn(&command);
        session.wait_for("done");
        let text = session.text();

        let expected = format!("LANG={}", termx_core::pty::DEFAULT_LOCALE);
        assert!(text.contains(&expected), "missing {expected:?} in {text:?}");
        assert!(text.contains("LC_ALL is unset"), "got: {text:?}");
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

/// A command that prints `ready` and then runs until it is forcibly killed.
/// On Unix it ignores the hang-up signal a polite kill sends, and is given a
/// short grace period so that the tests using it stay quick.
fn stubborn_process() -> PtyCommand {
    let command = if cfg!(unix) {
        PtyCommand::new("/bin/sh")
            .args(["-c", "trap '' HUP; echo ready; while :; do sleep 1; done"])
    } else {
        helper(&["sleep"])
    };
    command.kill_grace(Duration::from_millis(250))
}

#[cfg(unix)]
#[test]
fn process_cleaning_up_after_a_kill_is_not_cut_short() {
    guarded(|| {
        // On hang-up this process takes half a second to "save its state",
        // then exits with a code of its own. A grace period of 250 ms used
        // to kill it by force in the middle.
        let command = PtyCommand::new("/bin/sh").args([
            "-c",
            "trap 'sleep 0.5; exit 3' HUP; echo ready; while :; do sleep 0.1; done",
        ]);
        let mut session = Session::spawn(&command);
        session.wait_for("ready");

        session.pty.kill().unwrap();
        let status = session.exit_status();

        assert_eq!(
            status.code(),
            Some(3),
            "the clean-up was interrupted: {status}"
        );
    });
}

#[cfg(unix)]
#[test]
fn kill_grace_period_is_configurable() {
    guarded(|| {
        // A process ignoring the hang-up lives for the grace period...
        let patient = stubborn_process().kill_grace(Duration::from_millis(1500));
        let mut session = Session::spawn(&patient);
        session.wait_for("ready");
        session.pty.kill().unwrap();
        assert_eq!(
            session.pty.wait_timeout(Duration::from_millis(700)),
            None,
            "the process was killed by force before its grace period was over"
        );
        let status = session.exit_status();
        assert!(
            status
                .signal()
                .is_some_and(|signal| signal.contains("Kill")),
            "{status}"
        );

        // ...and not at all without one.
        let impatient = stubborn_process().kill_grace(Duration::ZERO);
        let mut session = Session::spawn(&impatient);
        session.wait_for("ready");
        let started = Instant::now();
        session.pty.kill().unwrap();
        let status = session.exit_status();
        assert!(
            status
                .signal()
                .is_some_and(|signal| signal.contains("Kill")),
            "{status}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "a forced kill took {:?}",
            started.elapsed()
        );
    });
}

#[test]
fn kill_returns_at_once_even_when_the_process_resists() {
    guarded(|| {
        const PROCESSES: usize = 20;
        let mut sessions: Vec<Session> = (0..PROCESSES)
            .map(|_| Session::spawn(&stubborn_process()))
            .collect();
        for session in &mut sessions {
            session.wait_for("ready");
        }

        // Killing used to block for the whole grace period of each process:
        // five seconds for these twenty.
        let started = Instant::now();
        for session in &sessions {
            session.pty.kill().unwrap();
        }
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_secs(2),
            "killing {PROCESSES} processes blocked for {elapsed:?}"
        );

        // The processes are still terminated, forcibly if they have to be.
        for session in &sessions {
            let status = session.exit_status();
            assert!(!status.success(), "{status}");
            if cfg!(unix) {
                let signal = status.signal().unwrap_or_default();
                assert!(signal.contains("Kill"), "expected SIGKILL, got {status}");
            }
        }
    });
}

#[test]
fn shutting_down_many_sessions_does_not_add_up() {
    guarded(|| {
        const SESSIONS: usize = 40;
        termx_core::pty::raise_open_file_limit().expect("cannot raise the open file limit");
        let manager = PtyManager::new();
        for _ in 0..SESSIONS {
            let (_, mut output) = manager
                .spawn(&stubborn_process(), PtySize::default())
                .unwrap();
            read_until(&mut output, "ready");
        }
        assert_eq!(manager.len(), SESSIONS);

        // One after the other, these would take a grace period each: ten
        // seconds on Unix. Signalled together they take about one.
        let started = Instant::now();
        manager.shutdown();
        let elapsed = started.elapsed();

        assert!(manager.is_empty());
        let limit = Duration::from_secs(if cfg!(unix) { 5 } else { 30 });
        assert!(
            elapsed < limit,
            "shutting {SESSIONS} sessions down took {elapsed:?}"
        );
        println!("shut {SESSIONS} sessions down in {elapsed:?}");
    });
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

/// Whether a Windows process with this identifier exists.
#[cfg(windows)]
fn windows_process_exists(pid: u32) -> bool {
    let filter = format!("PID eq {pid}");
    let output = std::process::Command::new("tasklist")
        .args(["/FI", &filter, "/FO", "CSV", "/NH"])
        .output()
        .expect("cannot run tasklist");
    String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\""))
}

#[cfg(windows)]
#[test]
fn killing_ends_what_the_process_started_outside_its_console() {
    guarded(|| {
        // `Start-Process` gives the grandchild a console of its own: closing
        // the pseudo-console alone does not end it.
        let command = PtyCommand::new("powershell.exe").args([
            "-NoLogo",
            "-NoProfile",
            "-Command",
            "$p = Start-Process -PassThru -WindowStyle Hidden powershell.exe \
             -ArgumentList '-NoProfile','-Command','Start-Sleep 300'; \
             Write-Output \"grandchild:$($p.Id):\"; Start-Sleep 300",
        ]);
        let mut session = Session::spawn(&command);
        let grandchild = |text: &str| -> Option<u32> {
            let rest = text.split("grandchild:").nth(1)?;
            let (pid, _) = rest.split_once(':')?;
            pid.trim().parse().ok()
        };
        let deadline = Instant::now() + TIMEOUT;
        let pid = loop {
            if let Some(pid) = grandchild(&session.text()) {
                break pid;
            }
            assert!(
                session.pump(Duration::from_millis(100)),
                "{:?}",
                session.text()
            );
            assert!(
                Instant::now() < deadline,
                "no process id in {:?}",
                session.text()
            );
        };
        assert!(windows_process_exists(pid), "the grandchild never ran");

        session.pty.kill().unwrap();
        let _ = session.exit_status();

        let deadline = Instant::now() + TIMEOUT;
        while windows_process_exists(pid) {
            assert!(
                Instant::now() < deadline,
                "the grandchild outlived the kill"
            );
            thread::sleep(Duration::from_millis(100));
        }
    });
}

/// The running processes whose command line is exactly `sleep <seconds>`.
#[cfg(unix)]
fn sleeping(seconds: u64) -> Vec<u32> {
    let output = std::process::Command::new("pgrep")
        .args(["-f", &format!("^sleep {seconds}$")])
        .output()
        .expect("cannot run pgrep");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().parse().ok())
        .filter(|pid: &u32| {
            let state = std::process::Command::new("ps")
                .args(["-o", "stat=", "-p", &pid.to_string()])
                .output()
                .expect("cannot run ps");
            let state = String::from_utf8_lossy(&state.stdout);
            !state.trim().is_empty() && !state.trim_start().starts_with('Z')
        })
        .collect()
}

/// Starts a shell that leaves two jobs behind, one behind `nohup` and one
/// ignoring `SIGHUP`, and exits. Returns the handle once it has exited.
#[cfg(unix)]
fn shell_leaving_jobs(first: u64, second: u64) -> Pty {
    let line = format!(
        "nohup sleep {first} >/dev/null 2>&1 & (trap '' HUP; sleep {second}) & sleep 0.3; exit"
    );
    let command = PtyCommand::new("/bin/sh")
        .args(["-c", &line])
        .kill_grace(Duration::from_millis(300));
    let (pty, output) = Pty::spawn(&command, PtySize::default()).expect("failed to spawn");
    drop(output);
    pty.wait_timeout(TIMEOUT).expect("the shell did not exit");
    let deadline = Instant::now() + TIMEOUT;
    while sleeping(first).is_empty() || sleeping(second).is_empty() {
        assert!(Instant::now() < deadline, "the jobs never started");
        thread::sleep(Duration::from_millis(20));
    }
    pty
}

#[cfg(unix)]
fn wait_until_none_sleeping(seconds: &[u64]) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while seconds.iter().any(|&s| !sleeping(s).is_empty()) {
        assert!(Instant::now() < deadline, "jobs outlived their terminal");
        thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(unix)]
#[test]
fn killing_a_terminal_whose_shell_exited_ends_what_it_left() {
    guarded(|| {
        let pty = shell_leaving_jobs(9_874_001, 9_874_002);
        // As in other terminals, the jobs outlive the shell...
        thread::sleep(Duration::from_millis(300));
        assert!(!sleeping(9_874_001).is_empty());

        // ...until the terminal is closed.
        pty.kill().expect("cannot kill");
        drop(pty);

        assert!(sleeping(9_874_001).is_empty(), "nohup job survived");
        assert!(sleeping(9_874_002).is_empty(), "trapped job survived");
    });
}

#[cfg(unix)]
#[test]
fn quitting_ends_what_terminals_that_ended_left() {
    guarded(|| {
        // Dropped without being killed, as when a shell ends on its own.
        drop(shell_leaving_jobs(9_874_011, 9_874_012));
        thread::sleep(Duration::from_millis(300));
        assert!(!sleeping(9_874_011).is_empty());

        assert!(termx_core::pty::end_leftover_processes(TIMEOUT));

        wait_until_none_sleeping(&[9_874_011, 9_874_012]);
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

#[cfg(unix)]
#[test]
fn background_job_that_keeps_printing_does_not_keep_the_session_open() {
    guarded(|| {
        // The job survives the shell and prints every 20 ms for 20 seconds,
        // so the terminal is never silent for long.
        let command = PtyCommand::new("/bin/sh").args([
            "-c",
            "(trap '' HUP; i=0; while [ $i -lt 1000 ]; do echo spam; sleep 0.02; i=$((i+1)); done) & \
             sleep 0.2; echo shell-is-done",
        ]);
        let started = Instant::now();
        let (text, status) = Session::spawn(&command).finish();

        assert!(text.contains("shell-is-done"), "got: {text:?}");
        assert!(status.success(), "{status}");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the session followed the background job for {:?}",
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

/// Reads from `output` until `needle` is visible.
fn read_until(output: &mut PtyOutput, needle: &str) -> String {
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
            Ok(chunk) => raw.extend_from_slice(&chunk),
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
        read_until(&mut first_output, "ready");
        read_until(&mut second_output, "ready");

        // Input goes to the session it was addressed to, and only there.
        manager.write(first, b"for-first\r").unwrap();
        manager.write(second, b"for-second\r").unwrap();
        let first_text = read_until(&mut first_output, "got:for-first");
        let second_text = read_until(&mut second_output, "got:for-second");
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
fn spawns_racing_a_shutdown_leave_nothing_behind() {
    guarded(|| {
        termx_core::pty::raise_open_file_limit().expect("cannot raise the open file limit");
        let dir = scratch_dir("shutdown-race");
        let manager = Arc::new(PtyManager::new());

        // Several threads keep spawning while the manager is shut down.
        let spawners: Vec<_> = (0..8)
            .map(|thread_index| {
                let manager = Arc::clone(&manager);
                let dir = dir.clone();
                thread::spawn(move || {
                    let mut heartbeats = Vec::new();
                    for spawn_index in 0..200 {
                        let file = dir.join(format!("beat-{thread_index}-{spawn_index}"));
                        let command = helper(&["heartbeat", &file.to_string_lossy()]);
                        match manager.spawn(&command, PtySize::default()) {
                            Ok(_) => heartbeats.push(file),
                            Err(PtyError::ManagerShutDown) => {
                                heartbeats.push(file);
                                return Ok(heartbeats);
                            }
                            Err(error) => return Err(error.to_string()),
                        }
                    }
                    Err("the manager was never shut down".to_owned())
                })
            })
            .collect();

        thread::sleep(Duration::from_millis(150));
        manager.shutdown();

        let mut heartbeats = Vec::new();
        for spawner in spawners {
            heartbeats.extend(spawner.join().expect("a spawner panicked").unwrap());
        }

        // Nothing was registered behind the shutdown's back...
        assert!(
            manager.is_empty(),
            "{} sessions were left registered",
            manager.len()
        );
        // ...and every process that was started is dead: no heartbeat moves.
        let read_all = |files: &[PathBuf]| -> Vec<Option<String>> {
            files
                .iter()
                .map(|file| std::fs::read_to_string(file).ok())
                .collect()
        };
        let before = read_all(&heartbeats);
        thread::sleep(Duration::from_millis(500));
        assert_eq!(
            read_all(&heartbeats),
            before,
            "a process outlived the shutdown"
        );
    });
}

#[test]
fn dropping_the_manager_kills_its_sessions() {
    guarded(|| {
        let file = scratch_dir("manager-heartbeat").join("beat");
        let manager = PtyManager::new();
        let (_, mut output) = manager
            .spawn(
                &helper(&["heartbeat", &file.to_string_lossy()]),
                PtySize::default(),
            )
            .unwrap();
        read_until(&mut output, "ready");
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
