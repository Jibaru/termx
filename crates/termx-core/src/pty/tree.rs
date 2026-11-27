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

use std::collections::HashMap;
use std::sync::{Condvar, Mutex, Once, OnceLock};
use std::time::{Duration, Instant};

use rustix::process::{Pid, Signal, kill_process_group};

use super::lock;
use crate::diagnostics::tracing::warn;

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
/// Only for a leader that has not been reaped: its own group is then
/// certainly its own, and always included. Groups found in the list of
/// processes are included when they still have members.
pub(super) fn groups_of_live_session(leader: Pid, foreground: Option<Pid>) -> Vec<Pid> {
    let mut groups = vec![leader];
    let found = match processes() {
        // The foreground group is in the session by definition: the list
        // has it, unless it has no member left.
        Some(processes) => groups_in(&processes, leader),
        None => foreground.into_iter().collect(),
    };
    for group in found {
        if !groups.contains(&group) {
            groups.push(group);
        }
    }
    groups
}

/// The groups that `processes` shows to have members in `session`, each
/// once.
fn groups_in(processes: &[Process], session: Pid) -> Vec<Pid> {
    let mut groups = Vec::new();
    for process in processes
        .iter()
        .filter(|process| process.session == session)
    {
        if !groups.contains(&process.group) {
            groups.push(process.group);
        }
    }
    groups
}

/// Sends `signal` to every group in `groups`. Failures are ignored: a
/// group that has just emptied cannot be signalled, which is the goal.
pub(super) fn signal_groups(groups: &[Pid], signal: Signal) {
    for &group in groups {
        let _ = kill_process_group(group, signal);
    }
}

/// How often the sweeper lists the processes of the system while it has
/// sessions to look after.
const SWEEP_TICK: Duration = Duration::from_millis(20);

/// How long the sweeper keeps a session it killed whose processes are
/// still listed (a process stuck in the kernel), before it gives up.
const SWEEP_GIVE_UP: Duration = Duration::from_secs(2);

/// What the sweeper does with one session.
#[derive(Debug, Clone, Copy)]
struct Swept {
    /// When to send `SIGKILL` to what is left. `None` while the session is
    /// only watched: its processes are left alone, but followed, so that
    /// they can be ended later.
    deadline: Option<Instant>,
    /// Whether `SIGHUP` was sent already.
    hung_up: bool,
}

/// Looks after the sessions whose leader has exited and been reaped, for
/// all the terminals of the application at once: one list of the
/// processes per tick, whatever the number of sessions.
///
/// # Why a session whose leader is gone can still be signalled safely
///
/// Its identifier is not handed out again while a process belongs to it.
/// The sweeper takes a session over while its leader is still a zombie
/// or just reaped, and lists its members every [`SWEEP_TICK`]: as soon as
/// a list shows none, the session is forgotten and never signalled again.
/// For the identifier to belong to someone else between two lists, every
/// member would have to exit and the system hand out that exact
/// identifier again, within one tick.
pub(super) struct Sweeper {
    sessions: Mutex<HashMap<Pid, Swept>>,
    /// Signalled when a session is added, and when one is forgotten.
    changed: Condvar,
}

impl Sweeper {
    /// The sweeper of the application. Its thread is started the first
    /// time there is something to look after.
    pub(super) fn get() -> &'static Self {
        static SWEEPER: OnceLock<Sweeper> = OnceLock::new();
        static STARTED: Once = Once::new();
        let sweeper = SWEEPER.get_or_init(|| Self {
            sessions: Mutex::new(HashMap::new()),
            changed: Condvar::new(),
        });
        STARTED.call_once(|| {
            let started = std::thread::Builder::new()
                .name("termx-pty-sweeper".to_owned())
                .spawn(|| Self::get().run());
            if let Err(error) = started {
                warn!(error = %error, "the sweeper thread could not be started; processes left by terminals are not ended");
            }
        });
        sweeper
    }

    /// Follows the processes left in `session` by a leader that has just
    /// exited, without signalling them. Does nothing where the processes
    /// cannot be listed.
    pub(super) fn watch(&self, session: Pid) {
        if processes().is_none() {
            return;
        }
        lock(&self.sessions).entry(session).or_insert(Swept {
            deadline: None,
            hung_up: false,
        });
        self.changed.notify_all();
    }

    /// Ends what is left in `session`: `SIGHUP` now unless `hung_up`, and
    /// `SIGKILL` at `deadline` for what is still there. A session already
    /// being ended keeps the earlier deadline.
    pub(super) fn terminate(&self, session: Pid, deadline: Instant, hung_up: bool) {
        if processes().is_none() {
            return;
        }
        let mut sessions = lock(&self.sessions);
        let swept = sessions.entry(session).or_insert(Swept {
            deadline: None,
            hung_up,
        });
        swept.deadline = Some(swept.deadline.map_or(deadline, |at| at.min(deadline)));
        drop(sessions);
        self.changed.notify_all();
    }

    /// Ends what is left in `session` if the sweeper follows it, which it
    /// only does while the session has members. See
    /// [`terminate`](Self::terminate).
    pub(super) fn terminate_if_watched(&self, session: Pid, deadline: Instant) {
        let mut sessions = lock(&self.sessions);
        if let Some(swept) = sessions.get_mut(&session) {
            swept.deadline = Some(swept.deadline.map_or(deadline, |at| at.min(deadline)));
            drop(sessions);
            self.changed.notify_all();
        }
    }

    /// Ends every session the sweeper follows, as for
    /// [`terminate`](Self::terminate).
    pub(super) fn terminate_all(&self, deadline: Instant) {
        for swept in lock(&self.sessions).values_mut() {
            swept.deadline = Some(swept.deadline.map_or(deadline, |at| at.min(deadline)));
        }
        self.changed.notify_all();
    }

    /// Waits until `session` is no longer followed, or until `until`.
    /// Returns whether it is not.
    pub(super) fn wait(&self, session: Pid, until: Instant) -> bool {
        self.wait_while(until, |sessions| sessions.contains_key(&session))
    }

    /// Waits until no session that is being ended is left, or until
    /// `until`. Sessions that are only watched do not count. Returns
    /// whether none is left.
    pub(super) fn wait_all(&self, until: Instant) -> bool {
        self.wait_while(until, |sessions| {
            sessions.values().any(|swept| swept.deadline.is_some())
        })
    }

    fn wait_while(&self, until: Instant, busy: impl Fn(&HashMap<Pid, Swept>) -> bool) -> bool {
        let mut sessions = lock(&self.sessions);
        while busy(&sessions) {
            let remaining = until.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            sessions = self
                .changed
                .wait_timeout(sessions, remaining)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
        true
    }

    fn run(&self) {
        loop {
            {
                let mut sessions = lock(&self.sessions);
                while sessions.is_empty() {
                    sessions = self
                        .changed
                        .wait(sessions)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                }
            }
            self.tick(Instant::now());
            std::thread::sleep(SWEEP_TICK);
        }
    }

    /// One look at the processes of the system, for every session.
    fn tick(&self, now: Instant) {
        // Listed before the lock is taken, so that registering a session
        // never waits for the list.
        let processes = processes().unwrap_or_default();
        let mut sessions = lock(&self.sessions);
        let before = sessions.len();
        sessions.retain(|&session, swept| {
            let groups = groups_in(&processes, session);
            if groups.is_empty() {
                return false;
            }
            let Some(deadline) = swept.deadline else {
                return true;
            };
            if !swept.hung_up {
                signal_groups(&groups, Signal::HUP);
                signal_groups(&groups, Signal::CONT);
                swept.hung_up = true;
            }
            if deadline <= now {
                signal_groups(&groups, Signal::KILL);
            }
            // A process the system cannot kill at once is given a moment,
            // then left: the sweeper must not wait for ever.
            now.saturating_duration_since(deadline) < SWEEP_GIVE_UP
        });
        if sessions.len() != before {
            drop(sessions);
            self.changed.notify_all();
        }
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

        let groups = groups_of_live_session(session, Some(session));

        assert_eq!(groups[0], session);
        assert_eq!(groups.iter().filter(|&&g| g == session).count(), 1);
        if let Some(processes) = processes() {
            assert!(groups.contains(&group), "{groups:?} lacks {group:?}");
            assert!(groups_in(&processes, session).contains(&group));
        }
    }

    #[test]
    fn a_session_without_members_is_not_signalled() {
        // An identifier no process uses as a session: nothing is found, so
        // nothing would be signalled, not even a group of that number.
        let processes = vec![Process {
            group: Pid::from_raw(40).unwrap(),
            session: Pid::from_raw(30).unwrap(),
        }];
        assert!(groups_in(&processes, Pid::from_raw(41).unwrap()).is_empty());
        assert_eq!(
            groups_in(&processes, Pid::from_raw(30).unwrap()),
            [Pid::from_raw(40).unwrap()]
        );
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
