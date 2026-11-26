//! Integration tests of the session layer: real processes in real
//! pseudo-terminals, observed through a [`SessionSink`] as a frontend would.
//!
//! The rules of `tests/common` apply: nothing waits forever, and on Windows
//! the visible text is compared, not the bytes.

mod common;

use std::path::Path;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use common::{TIMEOUT, guarded, helper, numbered_lines, read_heartbeat, scratch_dir, visible_text};
use termx_core::pty::{ExitStatus, PtyCommand, PtyError, PtySize};
use termx_core::session::{
    ACK_BATCH_BYTES, BATCH_MAX_BYTES, HIGH_WATERMARK_BYTES, SessionId, SessionManager,
    SessionOptions, SessionSink,
};

/// What a [`Frontend`] was told so far.
#[derive(Default)]
struct Received {
    raw: Vec<u8>,
    batches: usize,
    largest_batch: usize,
    empty_batches: usize,
    exit: Option<ExitStatus>,
    /// Calls made after the exit was reported, which must never happen.
    calls_after_exit: usize,
    /// The session each call was made for.
    sessions: Vec<SessionId>,
}

/// A stand-in for the frontend: a sink that records what it receives and
/// lets a test wait for it. Clones share the same record.
#[derive(Clone, Default)]
struct Frontend(Arc<(Mutex<Received>, Condvar)>);

impl Frontend {
    fn received(&self) -> MutexGuard<'_, Received> {
        self.0.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Blocks until `condition` holds for what was received, and returns
    /// what it returned.
    fn wait<T>(&self, what: &str, condition: impl Fn(&Received) -> Option<T>) -> T {
        let deadline = Instant::now() + TIMEOUT;
        let mut received = self.received();
        loop {
            if let Some(value) = condition(&received) {
                return value;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero(),
                "timed out waiting for {what}; exit: {:?}; got: {:?}",
                received.exit,
                visible_text(&received.raw)
            );
            received = self
                .0
                .1
                .wait_timeout(received, remaining)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Waits until `needle` is visible in the output.
    fn wait_for(&self, needle: &str) {
        self.wait(&format!("{needle:?}"), |received| {
            visible_text(&received.raw).contains(needle).then_some(())
        });
    }

    /// Waits for the exit of the session and returns its status.
    fn wait_for_exit(&self) -> ExitStatus {
        self.wait("the exit", |received| received.exit.clone())
    }

    fn text(&self) -> String {
        visible_text(&self.received().raw)
    }

    fn len(&self) -> usize {
        self.received().raw.len()
    }
}

impl SessionSink for Frontend {
    fn output(&mut self, session: SessionId, bytes: Vec<u8>) {
        let mut received = self.received();
        if received.exit.is_some() {
            received.calls_after_exit += 1;
        }
        received.batches += 1;
        received.largest_batch = received.largest_batch.max(bytes.len());
        received.empty_batches += usize::from(bytes.is_empty());
        received.raw.extend_from_slice(&bytes);
        received.sessions.push(session);
        drop(received);
        self.0.1.notify_all();
    }

    fn exited(&mut self, session: SessionId, status: ExitStatus) {
        let mut received = self.received();
        if received.exit.is_some() {
            received.calls_after_exit += 1;
        }
        received.exit = Some(status);
        received.sessions.push(session);
        drop(received);
        self.0.1.notify_all();
    }
}

/// Starts a session running the test helper in the given mode.
fn start(manager: &SessionManager, args: &[&str]) -> (SessionId, Frontend) {
    start_command(manager, helper(args), PtySize::new(120, 30))
}

fn start_command(
    manager: &SessionManager,
    command: PtyCommand,
    size: PtySize,
) -> (SessionId, Frontend) {
    let frontend = Frontend::default();
    let id = manager
        .create(&SessionOptions::new(command).size(size), frontend.clone())
        .expect("failed to create the session");
    (id, frontend)
}

/// Waits until `condition` holds, failing the test if it never does.
fn wait_until(what: &str, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + TIMEOUT;
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        thread::sleep(Duration::from_millis(5));
    }
}

/// Waits until the helper's `heartbeat` mode is visibly running.
fn wait_for_heartbeat(file: &Path) {
    wait_until("the helper starts beating", || {
        read_heartbeat(file).is_some_and(|beat| beat >= 2)
    });
}

/// Asserts that none of the heartbeat files changes any more.
fn assert_no_heartbeat(files: &[std::path::PathBuf]) {
    let read_all = || -> Vec<String> {
        files
            .iter()
            .map(|file| std::fs::read_to_string(file).unwrap_or_default())
            .collect()
    };
    let before = read_all();
    thread::sleep(Duration::from_millis(500));
    assert_eq!(read_all(), before, "a process is still running");
}

// ---------------------------------------------------------------------------
// One session, end to end
// ---------------------------------------------------------------------------

#[test]
fn input_reaches_the_process_and_its_answers_reach_the_sink() {
    guarded(|| {
        let manager = SessionManager::new().unwrap();
        let (id, frontend) = start(&manager, &["echo"]);
        assert_eq!(manager.ids(), [id]);
        assert_eq!(manager.len(), 1);
        assert!(manager.process_id(id).is_some());

        frontend.wait_for("ready");
        manager.write(id, b"hello\r").unwrap();
        frontend.wait_for("got:hello");
        manager.acknowledge(id, frontend.len() as u64);

        // Input is bytes, whatever they encode. (The Windows console
        // converts what it is sent, so this is checked on Unix.)
        if cfg!(unix) {
            manager
                .write(id, "caf\u{e9}-\u{2713}\r".as_bytes())
                .unwrap();
            frontend.wait_for("got:caf\u{e9}-\u{2713}");
        }

        manager.write(id, b"quit\r").unwrap();
        let status = frontend.wait_for_exit();
        assert!(status.success(), "{status}");

        // The session that ended is forgotten.
        wait_until("the session is removed", || manager.is_empty());
        assert!(matches!(
            manager.write(id, b"x"),
            Err(PtyError::UnknownSession(unknown)) if unknown == id
        ));
        assert!(!manager.close(id), "there is nothing left to close");

        let received = frontend.received();
        assert_eq!(received.calls_after_exit, 0);
        assert_eq!(received.empty_batches, 0);
        assert!(received.sessions.iter().all(|&session| session == id));
    });
}

#[test]
fn output_passes_through_unmodified() {
    guarded(|| {
        const LINES: usize = 2_000;
        // Without spaces: ConPTY may replace runs of spaces with cursor
        // movements.
        let text = "\u{2713}-caf\u{e9}-\u{1f680}-plain";
        let manager = SessionManager::new().unwrap();
        let (_, frontend) = start(&manager, &["repeat", &LINES.to_string(), text]);

        let status = frontend.wait_for_exit();
        assert!(status.success(), "{status}");

        let received = frontend.received();
        let visible = visible_text(&received.raw);
        assert_eq!(numbered_lines(&visible, text).len(), LINES);
        assert!(visible.contains("END-OF-OUTPUT"));
        // On Unix the PTY is transparent, so the bytes can be compared.
        if cfg!(unix) {
            let mut expected = Vec::new();
            for index in 0..LINES {
                expected.extend_from_slice(format!("{index:07}:{text}\r\n").as_bytes());
            }
            expected.extend_from_slice(b"END-OF-OUTPUT\r\n");
            assert!(received.raw == expected, "the bytes were altered");
        }
        // Small reads were put together: far fewer batches than lines.
        // (ConPTY paces its own output, so this is checked on Unix.)
        if cfg!(unix) {
            assert!(
                received.batches < LINES / 4,
                "{} batches for {LINES} lines",
                received.batches
            );
        }
    });
}

#[test]
fn the_exit_code_is_reported_after_the_last_output() {
    guarded(|| {
        let manager = SessionManager::new().unwrap();

        let (_, printed) = start(&manager, &["print", "last-words"]);
        let status = printed.wait_for_exit();
        assert!(status.success(), "{status}");
        // The exit is the last thing the sink hears, and the output that
        // came before it is complete.
        assert!(printed.text().contains("last-words"), "{}", printed.text());
        assert_eq!(printed.received().calls_after_exit, 0);

        let (_, failed) = start(&manager, &["exit", "3"]);
        let status = failed.wait_for_exit();
        assert_eq!(status.code(), Some(3), "{status}");
        assert!(!status.success());

        wait_until("both sessions are removed", || manager.is_empty());
    });
}

#[test]
fn the_default_session_is_the_login_shell() {
    guarded(|| {
        // An empty home directory keeps the developer's own shell
        // configuration out of the test. The empty `.zshrc` stops zsh from
        // starting its first-run wizard.
        let home = scratch_dir("session-home");
        std::fs::write(home.join(".zshrc"), "").unwrap();
        let mut options = SessionOptions::default();
        assert_eq!(options.command, PtyCommand::default_shell());
        options.command = options.command.cwd(&home);
        if cfg!(unix) {
            options.command = options.command.env("HOME", &home).env("ZDOTDIR", &home);
        }

        let manager = SessionManager::new().unwrap();
        let frontend = Frontend::default();
        let id = manager.create(&options, frontend.clone()).unwrap();
        if cfg!(windows) {
            // Wait for the PowerShell prompt (`PS C:\...>`), in case keys
            // typed while it starts are not kept.
            frontend.wait_for(">");
        }
        manager.write(id, b"exit 7\r").unwrap();

        let status = frontend.wait_for_exit();
        assert_eq!(status.code(), Some(7), "{status}; {:?}", frontend.text());
    });
}

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
fn a_session_starts_at_the_requested_size_and_can_be_resized() {
    guarded(|| {
        let manager = SessionManager::new().unwrap();
        let (id, frontend) = start_command(&manager, size_reporter(), PtySize::new(90, 28));
        frontend.wait_for("ready");

        manager.write(id, b"\r").unwrap();
        frontend.wait_for("size=28x90");

        manager.resize(id, PtySize::new(132, 43)).unwrap();
        // The Windows console applies a resize asynchronously, so ask until
        // the new size is reported.
        let deadline = Instant::now() + TIMEOUT;
        while !frontend.text().contains("size=43x132") {
            manager.write(id, b"\r").unwrap();
            thread::sleep(Duration::from_millis(200));
            assert!(
                Instant::now() < deadline,
                "the new size was never observed; got: {:?}",
                frontend.text()
            );
        }

        let invalid = manager.resize(id, PtySize::new(0, 43)).unwrap_err();
        assert!(matches!(
            invalid,
            PtyError::InvalidSize { cols: 0, rows: 43 }
        ));
        assert!(manager.close(id));
    });
}

// ---------------------------------------------------------------------------
// Flow control
// ---------------------------------------------------------------------------

#[test]
fn a_frontend_that_never_acknowledges_throttles_the_process() {
    guarded(|| {
        // 100 visible characters per line, 6 MB in total: far more than
        // everything between the process and the frontend can hold, so the
        // process can only finish if it is first throttled, then resumed.
        const LINES: usize = 60_000;
        let text = "x".repeat(92);
        let manager = SessionManager::new().unwrap();
        let (id, frontend) = start(&manager, &["repeat", &LINES.to_string(), &text]);

        // The frontend acknowledges nothing. Output stops once the
        // high-water mark is exceeded...
        frontend.wait("the high-water mark", |received| {
            (received.raw.len() as u64 > HIGH_WATERMARK_BYTES).then_some(())
        });
        thread::sleep(Duration::from_millis(500));
        let stalled_at = frontend.len();
        thread::sleep(Duration::from_millis(500));
        assert_eq!(frontend.len(), stalled_at, "output kept flowing");
        // ...by at most one batch, which bounds what the frontend holds.
        assert!(
            stalled_at as u64 <= HIGH_WATERMARK_BYTES + BATCH_MAX_BYTES as u64,
            "{stalled_at} bytes were sent without any acknowledgement"
        );
        // The process is blocked, not finished: its session is still there
        // and the end of its output is nowhere near.
        assert_eq!(manager.ids(), [id]);
        assert_eq!(frontend.received().exit, None);
        assert!(!frontend.text().contains("END-OF-OUTPUT"));

        // Input and resizes still work while the output is stalled.
        manager.resize(id, PtySize::new(100, 40)).unwrap();

        // The frontend catches up, acknowledging in batches as a real one
        // does: everything arrives.
        let mut acknowledged = 0;
        let status = loop {
            let (processed, exit) = {
                let received = frontend.received();
                (received.raw.len() as u64, received.exit.clone())
            };
            let due = (processed - acknowledged) / ACK_BATCH_BYTES * ACK_BATCH_BYTES;
            if due > 0 {
                manager.acknowledge(id, due);
                acknowledged += due;
            }
            if let Some(status) = exit {
                break status;
            }
            thread::sleep(Duration::from_millis(1));
        };
        assert!(status.success(), "{status}");

        let received = frontend.received();
        let visible = visible_text(&received.raw);
        let lines = numbered_lines(&visible, &text);
        assert_eq!(lines.len(), LINES, "some lines were lost");
        assert_eq!(lines.first(), Some(&0));
        assert_eq!(lines.last(), Some(&(LINES - 1)));
        assert!(visible.contains("END-OF-OUTPUT"));
        assert!(
            received.largest_batch <= BATCH_MAX_BYTES,
            "a batch of {} bytes exceeds the limit",
            received.largest_batch
        );
        assert_eq!(received.calls_after_exit, 0);
    });
}

/// Interrupting a program that floods the terminal must take effect
/// quickly: what still arrives afterwards is what was buffered between the
/// process and the frontend, so those buffers have to be small.
#[cfg(unix)]
#[test]
fn little_output_follows_the_interruption_of_a_flood() {
    guarded(|| {
        // A frontend that keeps up with 5 MB per second, far less than the
        // program prints: every buffer on the way is full.
        const FRONTEND_BYTES_PER_TICK: u64 = 50_000;
        const TICK: Duration = Duration::from_millis(10);

        let manager = Arc::new(SessionManager::new().unwrap());
        // Would print 10 GB if nothing stopped it.
        let (id, frontend) = start(&manager, &["repeat", "100000000", &"x".repeat(92)]);

        let acknowledging = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let acker = thread::spawn({
            let (manager, frontend) = (Arc::clone(&manager), frontend.clone());
            let acknowledging = Arc::clone(&acknowledging);
            move || {
                let mut acknowledged = 0;
                while acknowledging.load(std::sync::atomic::Ordering::SeqCst) {
                    let processed = frontend.len() as u64;
                    let due = (processed - acknowledged).min(FRONTEND_BYTES_PER_TICK);
                    manager.acknowledge(id, due);
                    acknowledged += due;
                    thread::sleep(TICK);
                }
            }
        });

        // Let the flood reach its steady state, then press Ctrl+C.
        frontend.wait("a few megabytes", |received| {
            (received.raw.len() > 3 * 1024 * 1024).then_some(())
        });
        let before = frontend.len();
        manager.write(id, b"\x03").unwrap();

        let status = frontend.wait_for_exit();
        acknowledging.store(false, std::sync::atomic::Ordering::SeqCst);
        acker.join().unwrap();
        assert!(status.signal().is_some(), "{status}");

        let after = frontend.len() - before;
        println!("{after} bytes were delivered after the interruption");
        assert!(
            after as u64 <= HIGH_WATERMARK_BYTES,
            "{after} bytes were still delivered after Ctrl+C"
        );
    });
}

#[test]
fn a_stalled_session_does_not_affect_another_one() {
    guarded(|| {
        let manager = SessionManager::new().unwrap();
        let text = "y".repeat(92);
        let (stalled, flood) = start(&manager, &["repeat", "60000", &text]);
        flood.wait("the high-water mark", |received| {
            (received.raw.len() as u64 > HIGH_WATERMARK_BYTES).then_some(())
        });

        let (id, other) = start(&manager, &["echo"]);
        other.wait_for("ready");
        manager.write(id, b"still-alive\r").unwrap();
        other.wait_for("got:still-alive");

        // Closing the stalled session does not need its output drained.
        let closing = Instant::now();
        assert!(manager.close(stalled));
        assert!(
            closing.elapsed() < Duration::from_secs(10),
            "closing took {:?}",
            closing.elapsed()
        );
        assert_eq!(flood.received().exit, None);
    });
}

// ---------------------------------------------------------------------------
// Closing and shutting down
// ---------------------------------------------------------------------------

#[test]
fn closing_a_session_kills_its_process_and_silences_its_sink() {
    guarded(|| {
        let file = scratch_dir("session-close").join("beat");
        let manager = SessionManager::new().unwrap();
        let (id, frontend) = start(&manager, &["heartbeat", &file.to_string_lossy()]);
        frontend.wait_for("ready");
        wait_for_heartbeat(&file);

        assert!(manager.close(id));

        // Closing returns only once the process is gone.
        assert_no_heartbeat(std::slice::from_ref(&file));
        assert!(manager.is_empty());
        // The application closed it: no exit is reported, and the sink was
        // dropped, so this handle is the only one left.
        assert_eq!(frontend.received().exit, None);
        assert_eq!(Arc::strong_count(&frontend.0), 1, "the sink was kept");
        assert!(!manager.close(id), "closing twice is harmless");
    });
}

#[test]
fn shutting_down_kills_every_session() {
    guarded(|| {
        const SESSIONS: usize = 12;
        termx_core::pty::raise_open_file_limit().expect("cannot raise the open file limit");
        let dir = scratch_dir("session-shutdown");
        let manager = SessionManager::new().unwrap();

        let mut files = Vec::new();
        let mut frontends = Vec::new();
        for index in 0..SESSIONS {
            let file = dir.join(format!("beat-{index}"));
            let (_, frontend) = start(&manager, &["heartbeat", &file.to_string_lossy()]);
            files.push(file);
            frontends.push(frontend);
        }
        // One more that is stalled by flow control while it is killed.
        let (_, flood) = start(&manager, &["repeat", "60000", &"z".repeat(92)]);
        flood.wait("the high-water mark", |received| {
            (received.raw.len() as u64 > HIGH_WATERMARK_BYTES).then_some(())
        });
        for file in &files {
            wait_for_heartbeat(file);
        }
        assert_eq!(manager.len(), SESSIONS + 1);

        let started = Instant::now();
        manager.shutdown();
        let elapsed = started.elapsed();

        // No process survives, and none was reported as having exited:
        // shutting down is the application's own doing.
        assert_no_heartbeat(&files);
        assert!(manager.is_empty());
        for frontend in frontends.iter().chain([&flood]) {
            assert_eq!(frontend.received().exit, None);
            assert_eq!(Arc::strong_count(&frontend.0), 1, "a sink was kept");
        }
        // The processes are killed together, not one grace period each.
        assert!(
            elapsed < Duration::from_secs(20),
            "shutting down took {elapsed:?}"
        );

        let error = manager
            .create(
                &SessionOptions::new(helper(&["sleep"])),
                Frontend::default(),
            )
            .unwrap_err();
        assert!(matches!(error, PtyError::ManagerShutDown), "{error}");
    });
}

/// How long the processes of [`stubborn`] sessions resist being closed.
#[cfg(unix)]
const STUBBORN_GRACE: Duration = Duration::from_secs(1);

/// Starts sessions whose processes ignore the hang-up signal a close sends,
/// and are only stopped by the forced kill at the end of their grace period.
/// Returns their identifiers and process identifiers.
#[cfg(unix)]
fn stubborn(manager: &SessionManager, count: usize) -> (Vec<SessionId>, Vec<u32>) {
    let command = PtyCommand::new("/bin/sh")
        .args(["-c", "trap '' HUP; echo ready; while :; do sleep 1; done"])
        .kill_grace(STUBBORN_GRACE);
    let mut ids = Vec::new();
    let mut frontends = Vec::new();
    for _ in 0..count {
        let (id, frontend) = start_command(manager, command.clone(), PtySize::default());
        ids.push(id);
        frontends.push(frontend);
    }
    // The trap is in place once the shell has printed.
    for frontend in &frontends {
        frontend.wait_for("ready");
    }
    let process_ids = ids
        .iter()
        .map(|&id| manager.process_id(id).expect("no process id"))
        .collect();
    (ids, process_ids)
}

/// Whether a process with this identifier exists.
#[cfg(unix)]
fn is_running(process_id: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &process_id.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .expect("cannot run kill")
        .success()
}

#[cfg(unix)]
fn survivors(process_ids: &[u32]) -> Vec<u32> {
    process_ids
        .iter()
        .copied()
        .filter(|&process_id| is_running(process_id))
        .collect()
}

#[cfg(unix)]
#[test]
fn shutting_down_kills_processes_that_ignore_the_hang_up_together() {
    guarded(|| {
        const SESSIONS: usize = 30;
        termx_core::pty::raise_open_file_limit().expect("cannot raise the open file limit");
        let manager = SessionManager::new().unwrap();
        let (_, process_ids) = stubborn(&manager, SESSIONS);
        assert_eq!(survivors(&process_ids).len(), SESSIONS);

        let started = Instant::now();
        manager.shutdown();
        let elapsed = started.elapsed();
        println!("shut {SESSIONS} stubborn sessions down in {elapsed:?}");

        assert_eq!(
            survivors(&process_ids),
            Vec::<u32>::new(),
            "processes survived"
        );
        // They resisted for their whole grace period, but all at once: one
        // grace period, not thirty.
        assert!(elapsed >= STUBBORN_GRACE, "killed early: {elapsed:?}");
        assert!(
            elapsed < STUBBORN_GRACE * 3,
            "shutting down took {elapsed:?}"
        );
    });
}

#[cfg(unix)]
#[test]
fn closing_several_sessions_takes_one_grace_period_in_total() {
    guarded(|| {
        const SESSIONS: usize = 6;
        let manager = SessionManager::new().unwrap();
        let (ids, process_ids) = stubborn(&manager, SESSIONS);
        let (bystander, _frontend) = start(&manager, &["sleep"]);

        let started = Instant::now();
        let closed = manager.close_all(&ids);
        let elapsed = started.elapsed();

        assert_eq!(closed, SESSIONS);
        // Closing returns only once the processes are gone.
        assert_eq!(
            survivors(&process_ids),
            Vec::<u32>::new(),
            "processes survived"
        );
        assert!(
            elapsed < STUBBORN_GRACE * 3,
            "closing {SESSIONS} sessions took {elapsed:?}"
        );
        assert_eq!(manager.ids(), [bystander], "another session was closed");
        assert_eq!(manager.close_all(&ids), 0, "closing twice is harmless");
    });
}

#[cfg(unix)]
#[test]
fn a_session_being_closed_stays_registered_until_its_process_is_gone() {
    guarded(|| {
        let manager = Arc::new(SessionManager::new().unwrap());
        let (ids, process_ids) = stubborn(&manager, 1);

        let closing = thread::spawn({
            let manager = Arc::clone(&manager);
            let id = ids[0];
            move || manager.close(id)
        });
        // The close is waiting for the process, which resists.
        thread::sleep(STUBBORN_GRACE / 4);
        assert_eq!(survivors(&process_ids), process_ids);
        assert_eq!(manager.ids(), ids, "forgotten while its process is alive");

        assert!(closing.join().unwrap());
        assert!(manager.is_empty());
        assert_eq!(survivors(&process_ids), Vec::<u32>::new());
    });
}

#[cfg(unix)]
#[test]
fn shutting_down_waits_for_a_close_that_is_in_progress() {
    guarded(|| {
        // What happens when a window is closed and the application exits
        // right after: the close runs on its own thread, and the exit must
        // not overtake it, or the process would outlive the application.
        let manager = Arc::new(SessionManager::new().unwrap());
        let (ids, process_ids) = stubborn(&manager, 3);

        let closing = thread::spawn({
            let manager = Arc::clone(&manager);
            move || manager.close_all(&ids)
        });
        thread::sleep(STUBBORN_GRACE / 4);
        assert_eq!(survivors(&process_ids), process_ids);

        manager.shutdown();

        // No waiting here: the processes are gone when shutdown returns.
        assert_eq!(
            survivors(&process_ids),
            Vec::<u32>::new(),
            "processes survived"
        );
        closing.join().unwrap();
    });
}

#[test]
fn dropping_the_manager_kills_its_sessions() {
    guarded(|| {
        let file = scratch_dir("session-drop").join("beat");
        let manager = SessionManager::new().unwrap();
        let (_, frontend) = start(&manager, &["heartbeat", &file.to_string_lossy()]);
        frontend.wait_for("ready");
        wait_for_heartbeat(&file);

        drop(manager);

        assert_no_heartbeat(&[file]);
        assert_eq!(frontend.received().exit, None);
    });
}

// ---------------------------------------------------------------------------
// Many sessions
// ---------------------------------------------------------------------------

#[test]
fn dozens_of_sessions_run_side_by_side() {
    guarded(|| {
        const SESSIONS: usize = 40;
        termx_core::pty::raise_open_file_limit().expect("cannot raise the open file limit");
        let manager = Arc::new(SessionManager::new().unwrap());

        // Created from several threads at once, as when a window restores
        // its sessions.
        let creators: Vec<_> = (0..SESSIONS)
            .map(|index| {
                let manager = Arc::clone(&manager);
                thread::spawn(move || {
                    let marker = format!("session-{index}-marker");
                    let (id, frontend) = start(&manager, &["print", &marker]);
                    (id, marker, frontend)
                })
            })
            .collect();
        let sessions: Vec<_> = creators
            .into_iter()
            .map(|creator| creator.join().expect("a creator panicked"))
            .collect();

        let mut ids: Vec<SessionId> = sessions.iter().map(|(id, ..)| *id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), SESSIONS, "identifiers are unique");

        for (id, marker, frontend) in &sessions {
            let status = frontend.wait_for_exit();
            assert!(status.success(), "{marker}: {status}");
            let received = frontend.received();
            let text = visible_text(&received.raw);
            assert!(text.contains(marker.as_str()), "{marker}: got {text:?}");
            // Nothing of another session leaked into this one.
            assert_eq!(text.matches("-marker").count(), 1, "{marker}: {text:?}");
            assert!(received.sessions.iter().all(|session| session == id));
        }
        wait_until("every session is removed", || manager.is_empty());
    });
}
