//! Core library of the termx terminal emulator.
//!
//! This crate holds everything that does not depend on a user interface:
//! pseudo-terminal handling, session management and configuration. It must
//! never depend on Tauri or on any other UI toolkit, so that it can be tested
//! and reused on its own.
//!
//! # Layout
//!
//! - [`pty`]: spawning and driving pseudo-terminals.
//! - [`session`]: the model of a running terminal session.
//! - [`config`]: user configuration.
//! - [`diagnostics`]: persistent logging and the diagnostics bundle.

#![warn(missing_docs)]

pub mod config;
pub mod diagnostics;
pub mod pty;
pub mod session;

/// Returns the version of the termx core library.
///
/// The value comes from the crate manifest at compile time, so it always
/// matches the version the binary was built from.
///
/// # Examples
///
/// ```
/// assert!(!termx_core::version().is_empty());
/// ```
#[must_use]
pub const fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::version;

    #[test]
    fn version_matches_the_crate_manifest() {
        assert_eq!(version(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn version_is_a_semver_triple() {
        let core = version().split(['-', '+']).next().unwrap();
        let parts: Vec<&str> = core.split('.').collect();

        assert_eq!(parts.len(), 3, "expected MAJOR.MINOR.PATCH in {core:?}");
        for part in parts {
            part.parse::<u64>().unwrap();
        }
    }
}
