//! The workspace: the sessions of the application as the user sees them.
//!
//! The [`session`](crate::session) module runs terminals: processes in
//! pseudo-terminals, identified by a number that is only valid while they
//! run. This module is the layer above, and does not depend on a user
//! interface.
//!
//! - [`Session`], [`SessionKind`], [`SessionState`]: the model of a
//!   session.
//! - [`launch`]: what to run for each kind of session, through the user's
//!   shell so that it has the user's environment.
//! - [`project`]: which project a folder belongs to, and its Git branch.
//! - [`layout`]: the split tree of panes of the main area.
//! - [`store`]: the state file, written atomically and owned by one
//!   instance.
//! - [`notify`]: change notifications, so that a frontend never polls.

pub mod launch;
pub mod layout;
mod model;
pub mod notify;
pub mod project;
pub mod store;

pub use launch::{LaunchFailure, LaunchMode, Launcher};
pub use layout::{Direction, Edge, Layout, LayoutError, Node, PaneId, Placement, SplitId};
pub use model::{
    InvalidSessionKey, MAX_CLAUDE_SESSION_ID_BYTES, MAX_COMMAND_BYTES, MAX_TITLE_CHARS, Session,
    SessionKey, SessionKind, SessionState, clean_title, default_title, now_ms,
};
pub use notify::{ChangeSet, Subscription};
pub use project::{GitInfo, Project, ProjectKind};
pub use store::{PersistenceMode, WindowGeometry};

use std::path::{Path, PathBuf};

use serde::Serializer;

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
