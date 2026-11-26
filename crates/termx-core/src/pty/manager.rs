//! A registry of running PTYs.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use super::{Pty, PtyCommand, PtyError, PtyOutput, PtySize, lock};

/// Opaque identifier of a PTY registered in a [`PtyManager`].
///
/// Identifiers are unique within one manager and are never reused. The raw
/// value is only exposed so that the identifier can cross a serialisation
/// boundary, such as the IPC between the application and its user interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(u64);

impl SessionId {
    /// Rebuilds an identifier from the value returned by
    /// [`as_u64`](Self::as_u64).
    #[must_use]
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    /// The raw value of the identifier.
    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// A thread-safe registry of running PTYs, keyed by [`SessionId`].
///
/// It only owns the PTYs. Everything else the application knows about a
/// session (title, status, ...) is deliberately kept out of it.
///
/// Removing a PTY from the manager, or dropping the manager, drops the PTY
/// and therefore kills its process, unless a handle obtained from
/// [`get`](Self::get) is still alive.
///
/// # Examples
///
/// ```no_run
/// use termx_core::pty::{PtyCommand, PtyManager, PtySize};
///
/// # fn main() -> Result<(), termx_core::pty::PtyError> {
/// let manager = PtyManager::new();
/// let (id, output) = manager.spawn(&PtyCommand::default_shell(), PtySize::default())?;
///
/// manager.write(id, b"ls\r")?;
/// manager.resize(id, PtySize::new(100, 30))?;
/// # drop(output);
/// manager.remove(id);
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Default)]
pub struct PtyManager {
    next_id: AtomicU64,
    sessions: Mutex<HashMap<SessionId, Arc<Pty>>>,
}

impl PtyManager {
    /// Creates an empty manager.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Spawns `command` in a new PTY and registers it.
    ///
    /// # Errors
    ///
    /// The same as [`Pty::spawn`]. Nothing is registered on error.
    pub fn spawn(
        &self,
        command: &PtyCommand,
        size: PtySize,
    ) -> Result<(SessionId, PtyOutput), PtyError> {
        let (pty, output) = Pty::spawn(command, size)?;
        let id = SessionId(self.next_id.fetch_add(1, Ordering::Relaxed) + 1);
        lock(&self.sessions).insert(id, Arc::new(pty));
        Ok((id, output))
    }

    /// Returns a handle to a registered PTY, for the operations that have no
    /// shortcut on the manager (such as waiting for the exit status).
    #[must_use]
    pub fn get(&self, id: SessionId) -> Option<Arc<Pty>> {
        lock(&self.sessions).get(&id).cloned()
    }

    /// Sends input to a PTY. See [`Pty::write`].
    ///
    /// # Errors
    ///
    /// [`PtyError::UnknownSession`] if `id` is not registered, or the error
    /// of [`Pty::write`].
    pub fn write(&self, id: SessionId, data: &[u8]) -> Result<(), PtyError> {
        self.require(id)?.write(data)
    }

    /// Resizes a PTY. See [`Pty::resize`].
    ///
    /// # Errors
    ///
    /// [`PtyError::UnknownSession`] if `id` is not registered, or the error
    /// of [`Pty::resize`].
    pub fn resize(&self, id: SessionId, size: PtySize) -> Result<(), PtyError> {
        self.require(id)?.resize(size)
    }

    /// Kills the process of a PTY, which stays registered so that its exit
    /// status can still be read. See [`Pty::kill`].
    ///
    /// # Errors
    ///
    /// [`PtyError::UnknownSession`] if `id` is not registered, or the error
    /// of [`Pty::kill`].
    pub fn kill(&self, id: SessionId) -> Result<(), PtyError> {
        self.require(id)?.kill()
    }

    /// Unregisters a PTY and returns it.
    ///
    /// Dropping the returned handle kills the process if it is still running.
    pub fn remove(&self, id: SessionId) -> Option<Arc<Pty>> {
        lock(&self.sessions).remove(&id)
    }

    /// The identifiers of the registered PTYs, in creation order.
    #[must_use]
    pub fn ids(&self) -> Vec<SessionId> {
        let mut ids: Vec<SessionId> = lock(&self.sessions).keys().copied().collect();
        ids.sort_unstable();
        ids
    }

    /// The number of registered PTYs.
    #[must_use]
    pub fn len(&self) -> usize {
        lock(&self.sessions).len()
    }

    /// Whether no PTY is registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Looks a PTY up, releasing the registry lock before returning so that a
    /// slow operation on one PTY never blocks the others.
    fn require(&self, id: SessionId) -> Result<Arc<Pty>, PtyError> {
        self.get(id).ok_or(PtyError::UnknownSession(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_id_round_trips_through_its_raw_value() {
        let id = SessionId::from_raw(42);

        assert_eq!(id.as_u64(), 42);
        assert_eq!(SessionId::from_raw(id.as_u64()), id);
        assert_eq!(id.to_string(), "42");
    }

    #[test]
    fn operations_on_an_unknown_session_fail() {
        let manager = PtyManager::new();
        let id = SessionId::from_raw(7);

        assert!(manager.is_empty());
        assert!(manager.ids().is_empty());
        assert!(manager.get(id).is_none());
        assert!(manager.remove(id).is_none());
        for error in [
            manager.write(id, b"x").unwrap_err(),
            manager.resize(id, PtySize::default()).unwrap_err(),
            manager.kill(id).unwrap_err(),
        ] {
            assert!(
                matches!(error, PtyError::UnknownSession(unknown) if unknown == id),
                "unexpected error: {error}"
            );
            assert_eq!(error.to_string(), "no terminal session with id 7");
        }
    }

    #[test]
    fn a_failed_spawn_registers_nothing() {
        let manager = PtyManager::new();

        let error = manager
            .spawn(&PtyCommand::new(""), PtySize::default())
            .unwrap_err();

        assert!(matches!(error, PtyError::EmptyProgram), "{error}");
        assert_eq!(manager.len(), 0);
    }
}
