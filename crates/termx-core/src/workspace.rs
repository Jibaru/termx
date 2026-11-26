//! The workspace: the sessions of the application as the user sees them.
//!
//! The [`session`](crate::session) module runs terminals: processes in
//! pseudo-terminals, identified by a number that is only valid while they
//! run. This module is the layer above, and does not depend on a user
//! interface.
//!
//! - [`project`]: which project a folder belongs to, and its Git branch.

pub mod project;

pub use project::{GitInfo, Project, ProjectKind};

use std::path::Path;

use serde::Serializer;

/// Serializes a path as text, replacing what is not Unicode instead of
/// failing: a frontend must always get its sessions.
fn lossy_path<S: Serializer>(path: &Path, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&path.to_string_lossy())
}
