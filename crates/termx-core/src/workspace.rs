//! The workspace: the sessions of the application, how they are grouped
//! and laid out, and what is remembered of them between two runs.
//!
//! The [`session`](crate::session) module runs terminals: processes in
//! pseudo-terminals, identified by a number that is only valid while they
//! run. This module is the layer above. A [`Session`] here is what the user
//! sees as a row of the sidebar: it has a [`SessionKey`] that never
//! changes, a kind, a folder, a title, a project, and a process that comes
//! and goes. Nothing in this module depends on a user interface.
//!
//! # Overview
//!
//! - [`Workspace`] owns everything and is what an application talks to. It
//!   is thread-safe; every method can be called from any thread.
//! - [`Session`], [`SessionKind`], [`SessionState`]: the model of a
//!   session.
//! - [`project`]: which project a folder belongs to, and its Git branch.
//! - [`launch`]: what to run for each kind of session, through the user's
//!   shell so that it has the user's environment.
//! - [`layout`]: the split tree of panes of the main area.
//! - [`store`]: the state file, written atomically and owned by one
//!   instance.
//! - [`notify`]: change notifications, so that a frontend never polls.
//!
//! # The life of a session
//!
//! ```text
//!               create                       begin_launch
//!   (nothing) ---------> Starting <------------------------------+
//!                           |  terminal_started                  |
//!   restored from disk      v                                    |
//!   ---------> Dormant   Running --terminal_exited--> Exited ----+
//!                 |                                              |
//!                 +----------------------------------------------+
//! ```
//!
//! The workspace does not start processes itself. The application asks it
//! what to run ([`Workspace::begin_launch`]), starts that in a terminal of
//! the [`session`](crate::session) layer, and reports what became of it
//! ([`Workspace::terminal_started`], [`Workspace::terminal_exited`],
//! [`Workspace::launch_failed`]). That keeps this module free of threads
//! that read terminals, and lets it be tested without any.
//!
//! # Restoring
//!
//! Processes are not kept alive between two runs of the application. A
//! session read from the state file is [`SessionState::Dormant`]: it shows
//! in the sidebar and in its pane, and waits to be started again with
//! [`LaunchMode::Resume`] (Claude Code resumes its conversation; a shell or
//! a background command starts anew in the same folder) or
//! [`LaunchMode::Fresh`].
//!
//! # Saving
//!
//! The state file is written on the thread of the workspace, never on a
//! caller's. What the user does on purpose is saved at once: creating,
//! closing, renaming or moving a session or a project, splitting, closing
//! or moving a pane or showing a session in one, forgetting a recent
//! folder, and the conversation of a session becoming known. What changes
//! in bursts is saved once it has been left alone for [`SAVE_DEBOUNCE`],
//! and never later than [`SAVE_MAX_WAIT`] after the first change of the
//! burst: titles set by programs, activity, the Git state, the window, a
//! divider being dragged and the focus. Everything is saved on
//! [`Workspace::shutdown`].
//!
//! So a crash of termx loses at most: a change made on purpose in the
//! milliseconds before the crash, while it was being written; and the
//! changes of the last burst, from up to [`SAVE_MAX_WAIT`] before the
//! crash. A save that fails is tried again, and the frontend is told
//! ([`PersistenceMode::Failing`]).
//!
//! # Which folder counts
//!
//! A session is started in [`Session::cwd`], and restarted there. While it
//! runs, a program can report the folder it moved to (the OSC 7 escape
//! sequence, passed on with [`Workspace::report_cwd`]); the project and the
//! branch shown then follow that folder. The working directory of the
//! process itself is never inspected: that is not possible on every
//! platform, and a shell's directory is not that of the program running in
//! it. So for a program that does not report its folder, which is most of
//! them unless the shell is configured to, the project and branch are
//! those of the folder the session was started in.
//!
//! # Keeping the branch current
//!
//! Git metadata is read directly ([`project`]), so reading it again is
//! cheap. The workspace does that on threads of its own, never on a
//! caller's, and never on the thread that saves: a folder on a drive that
//! does not answer delays its own project, nothing else. A caller that
//! needs the project of a new folder waits for it for at most
//! [`GIT_DETECT_BUDGET`]; past that, the folder is shown as its own project
//! until the detection ends. The project is read again:
//!
//! - for the sessions that printed something, at most once per
//!   [`ACTIVITY_COALESCE`]: a branch changes because something ran, and
//!   what runs prints;
//! - for every session every [`GIT_POLL_INTERVAL`], which catches changes
//!   made outside termx;
//! - at once when asked ([`Workspace::refresh_git`]), for instance when the
//!   window gets the focus back.
//!
//! This was chosen over watching `.git/HEAD` for changes: a watcher costs
//! a dependency with different behaviour and limits on every platform, has
//! to watch directories rather than the file (Git replaces `HEAD` by
//! renaming), and would still miss a repository that appears later. The
//! reads above amount to a few `stat` calls per session every ten seconds.

pub mod launch;
pub mod layout;
mod model;
pub mod notify;
pub mod project;
mod service;
pub mod store;

pub use launch::{LaunchFailure, LaunchMarker, LaunchMode, LaunchToken, Launcher, MarkerFilter};
pub use layout::{Direction, Edge, Layout, LayoutError, Node, PaneId, Placement, SplitId};
pub use model::{
    InvalidSessionKey, MAX_CLAUDE_SESSION_ID_BYTES, MAX_COMMAND_BYTES, MAX_TITLE_CHARS, Session,
    SessionKey, SessionKind, SessionState, clean_title, default_title, now_ms,
};
pub use notify::{ChangeSet, Subscription};
pub use project::{GitInfo, Project, ProjectKind};
pub use service::{
    ACTIVITY_COALESCE, ActivityHandle, Delta, GIT_DETECT_BUDGET, GIT_POLL_INTERVAL,
    LAUNCH_CONFIRM_TIMEOUT, Launch, NewSession, SAVE_DEBOUNCE, SAVE_MAX_WAIT, SAVE_RETRY,
    SHUTDOWN_WAIT, Snapshot, Workspace, WorkspaceConfig, WorkspaceError, parse_reported_cwd,
};
pub use store::{PersistenceMode, WindowGeometry};

use std::path::{Path, PathBuf};

use serde::Serializer;
use serde::ser::SerializeSeq;

/// Serializes a path as text, replacing what is not Unicode instead of
/// failing: a frontend must always get its sessions.
fn lossy_path<S: Serializer>(path: &Path, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&path.to_string_lossy())
}

/// [`lossy_path`] for an optional path.
fn lossy_path_option<S: Serializer>(
    path: &Option<PathBuf>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match path {
        Some(path) => serializer.serialize_some(&*path.to_string_lossy()),
        None => serializer.serialize_none(),
    }
}

/// [`lossy_path`] for a list of paths.
fn lossy_paths<S: Serializer>(paths: &[PathBuf], serializer: S) -> Result<S::Ok, S::Error> {
    let mut sequence = serializer.serialize_seq(Some(paths.len()))?;
    for path in paths {
        sequence.serialize_element(&*path.to_string_lossy())?;
    }
    sequence.end()
}

/// [`lossy_paths`] for an optional list of paths.
fn lossy_paths_option<S: Serializer>(
    paths: &Option<Vec<PathBuf>>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match paths {
        Some(paths) => {
            let texts: Vec<_> = paths.iter().map(|path| path.to_string_lossy()).collect();
            serializer.serialize_some(&texts)
        }
        None => serializer.serialize_none(),
    }
}
