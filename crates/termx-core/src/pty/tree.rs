//! Everything a terminal started, so that closing it ends all of it.
//!
//! # Unix
//!
//! The process of a terminal is the leader of a session of its own (it is
//! started with `setsid`), and everything it starts belongs to that session
//! unless it leaves on purpose (`setsid`, as a daemon does). Signalling only
//! the leader is not enough: a shell with job control puts each job in a
//! process group of its own, a program can ignore `SIGHUP` (`nohup`,
//! `trap '' HUP`), and those outlive the shell that started them.
//!
//! So a terminal is ended by signalling every process group of its session:
//! `SIGHUP` first, as when a terminal window is closed, and `SIGKILL` for
//! whatever is still there after the grace period. The groups are found by
//! listing the processes of the system and keeping those whose session is
//! the terminal's ([`groups_of_session`]): `/proc` on Linux, the process
//! table on macOS. On other systems, where no such list is read, the group
//! of the leader and the foreground group of the terminal are signalled.
//!
//! # Process identifiers
//!
//! A process identifier is reused once the process is gone, so signalling
//! a stale one could hit an unrelated process. What keeps this safe:
//!
//! - The leader is signalled only while it has not been reaped, under the
//!   same lock as the reaping (see `process.rs`).
//! - A group or session identifier is not handed out again while any
//!   process still belongs to that group or session, on Linux and on
//!   macOS. The groups that are signalled are those the list of processes
//!   has just shown to have members in the terminal's session.
//!
//! What is left is the moment between reading the list and sending the
//! signal, during which the last member of a group would have to exit and
//! its identifier be handed out again: the system hands identifiers out in
//! increasing order and wraps around after tens of thousands at the least,
//! so that does not happen within microseconds in practice.
//!
//! # Windows
//!
//! See `job.rs`.

use rustix::process::{Pid, Signal, kill_process_group};

/// One process of the system, as far as ending a terminal is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Process {
    group: Pid,
    session: Pid,
}

/// The process groups of the session whose leader is `leader`, plus
/// `foreground` (the foreground group of its terminal) when it is known.
/// The group of the leader comes first, and every group appears once.
///
/// Only groups that still have members are returned, except the leader's
/// own, which is always there: signalling a group that is gone fails
/// harmlessly.
pub(super) fn groups_of_session(leader: Pid, foreground: Option<Pid>) -> Vec<Pid> {
    let mut groups = vec![leader];
    let mut add = |group: Pid| {
        if !groups.contains(&group) {
            groups.push(group);
        }
    };
    match processes() {
        // The foreground group is in the session by definition: the list
        // has it, unless it has no member left.
        Some(processes) => processes
            .iter()
            .filter(|process| process.session == leader)
            .for_each(|process| add(process.group)),
        None => {
            if let Some(foreground) = foreground {
                add(foreground);
            }
        }
    }
    groups
}

/// Whether any process that has not exited still belongs to the session of
/// `leader`.
///
/// Where the processes of the system cannot be listed, this only knows
/// about the group of the leader.
pub(super) fn session_has_members(leader: Pid) -> bool {
    match processes() {
        Some(processes) => processes.iter().any(|process| process.session == leader),
        None => rustix::process::test_kill_process_group(leader).is_ok(),
    }
}

/// Sends `signal` to every group in `groups`. Failures are ignored: a
/// group that has just emptied cannot be signalled, which is the goal.
pub(super) fn signal_groups(groups: &[Pid], signal: Signal) {
    for &group in groups {
        let _ = kill_process_group(group, signal);
    }
}

/// The processes of the system that have not exited, or `None` where they
/// cannot be listed.
#[cfg(target_os = "linux")]
fn processes() -> Option<Vec<Process>> {
    let entries = std::fs::read_dir("/proc").ok()?;
    Some(
        entries
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.bytes().all(|b| b.is_ascii_digit()))
            })
            .filter_map(|entry| {
                parse_stat(&std::fs::read_to_string(entry.path().join("stat")).ok()?)
            })
            .collect(),
    )
}

/// Reads the group and the session of a process from its `/proc/<pid>/stat`
/// line: `pid (name) state ppid pgrp session ...`. The name may hold
/// spaces and parentheses, so the fields are counted from the last `)`.
/// `None` for a process that has exited and waits to be reaped (`Z`, `X`).
#[cfg(any(target_os = "linux", test))]
fn parse_stat(stat: &str) -> Option<Process> {
    let rest = &stat[stat.rfind(')')? + 1..];
    let mut fields = rest.split_ascii_whitespace();
    let state = fields.next()?;
    if matches!(state, "Z" | "X" | "x") {
        return None;
    }
    let _parent = fields.next()?;
    let group = Pid::from_raw(fields.next()?.parse().ok()?)?;
    let session = Pid::from_raw(fields.next()?.parse().ok()?)?;
    Some(Process { group, session })
}

/// The processes of the system that have not exited, or `None` where they
/// cannot be listed.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn processes() -> Option<Vec<Process>> {
    use std::ffi::{c_int, c_void};

    use rustix::process::{getpgid, getsid};

    // Asked twice: once for the number, once for the list, with room for
    // processes started in between.
    // SAFETY: a null buffer of size 0 is the documented way of asking
    // `proc_listallpids` for the number of processes; nothing is written.
    let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    let count = usize::try_from(count).ok()?;
    let mut pids: Vec<c_int> = vec![0; count + 64];
    let size = c_int::try_from(pids.len() * std::mem::size_of::<c_int>()).ok()?;
    // SAFETY: the buffer is valid for `size` bytes and holds `c_int`s,
    // which is what `proc_listallpids` writes; it writes at most `size`
    // bytes and returns how many identifiers it wrote.
    let written = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast::<c_void>(), size) };
    let written = usize::try_from(written).ok()?.min(pids.len());
    pids.truncate(written);
    // A process that has exited is not found by `getsid` any more, even
    // before it is reaped.
    Some(
        pids.into_iter()
            .filter_map(Pid::from_raw)
            .filter_map(|pid| {
                Some(Process {
                    session: getsid(Some(pid)).ok()?,
                    group: getpgid(Some(pid)).ok()?,
                })
            })
            .collect(),
    )
}

/// The processes of the system that have not exited, or `None` where they
/// cannot be listed.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn processes() -> Option<Vec<Process>> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    use rustix::process::{getpgid, getsid};

    #[test]
    fn this_process_is_in_the_list_of_its_session() {
        let session = getsid(None).unwrap();
        let group = getpgid(None).unwrap();

        let groups = groups_of_session(session, Some(session));

        assert_eq!(groups[0], session);
        assert_eq!(groups.iter().filter(|&&g| g == session).count(), 1);
        if processes().is_some() {
            assert!(groups.contains(&group), "{groups:?} lacks {group:?}");
            assert!(session_has_members(session));
        }
    }

    #[test]
    fn stat_lines_are_read_from_the_end_of_the_name() {
        let pid = |raw| Pid::from_raw(raw).unwrap();
        assert_eq!(
            parse_stat("42 (a b) c)) S 1 40 30 0 -1"),
            Some(Process {
                group: pid(40),
                session: pid(30)
            })
        );
        assert_eq!(parse_stat("42 (gone) Z 1 40 30 0 -1"), None);
        assert_eq!(parse_stat("garbage"), None);
    }
}
