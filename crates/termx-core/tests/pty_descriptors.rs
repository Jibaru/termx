//! File descriptor accounting of the PTY layer.
//!
//! These checks count the descriptors of the whole process, so they live in
//! their own test binary, as a single test: nothing else may open or close
//! descriptors while they run.

#![cfg(unix)]

use std::time::{Duration, Instant};

use termx_core::pty::{Pty, PtyCommand, PtyOutput, PtySize, raise_open_file_limit};

/// Number of descriptors a PTY is documented to use: one more on macOS,
/// where the terminal is kept open on our side so that no output is lost.
const DESCRIPTORS_PER_PTY: usize = if cfg!(target_os = "macos") { 5 } else { 4 };

fn open_descriptors() -> usize {
    std::fs::read_dir("/dev/fd")
        .expect("cannot list the open file descriptors")
        .count()
}

fn spawn_sleeper() -> (Pty, PtyOutput) {
    let command = PtyCommand::new("/bin/sh").args(["-c", "sleep 60"]);
    Pty::spawn(&command, PtySize::default()).expect("failed to spawn")
}

#[test]
fn descriptors_per_pty_are_bounded_and_released() {
    let limit = raise_open_file_limit()
        .expect("cannot raise the open file limit")
        .expect("Unix has an open file limit");
    let baseline = open_descriptors();

    // More terminals than the macOS default limit of 256 descriptors allows
    // (spawn number 50 used to fail there).
    const PTYS: usize = 80;
    assert!(
        limit as usize > baseline + PTYS * DESCRIPTORS_PER_PTY,
        "the limit of {limit} descriptors is too low for this test"
    );
    let sessions: Vec<(Pty, PtyOutput)> = (0..PTYS).map(|_| spawn_sleeper()).collect();

    let used = open_descriptors() - baseline;
    assert!(
        used <= PTYS * DESCRIPTORS_PER_PTY,
        "{PTYS} terminals use {used} descriptors, more than {DESCRIPTORS_PER_PTY} each"
    );

    // Dropping the handles releases every descriptor: nothing leaks.
    let started = Instant::now();
    drop(sessions);
    assert!(
        started.elapsed() < Duration::from_secs(60),
        "dropping took {:?}",
        started.elapsed()
    );
    assert_eq!(open_descriptors(), baseline, "descriptors were leaked");
}
