//! The registry of running sessions.

use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::driver::{self, Event};
use super::pump::{PtySource, Pump};
use super::{OutputFilter, SessionId, SessionSink};
use crate::diagnostics::tracing::{debug, info, warn};
use crate::pty::{DEFAULT_KILL_GRACE, Pty, PtyCommand, PtyError, PtyManager, PtySize, lock};

/// Longest time [`SessionManager::shutdown`] waits for the closes that are
/// in progress on other threads when it is called.
///
/// Such a close has already asked its processes to terminate, and they are
/// killed by force when their grace period is over, so the wait normally
/// ends within [`DEFAULT_KILL_GRACE`]. Twice that leaves room for a busy
/// machine without letting a close that is stuck hold up the exit of the
/// application for ever.
pub const SHUTDOWN_CLOSE_WAIT: Duration = DEFAULT_KILL_GRACE.saturating_mul(2);

/// Counts the closes in progress, so that a shutdown can wait for them.
#[derive(Default)]
struct ClosesInProgress {
    count: Mutex<usize>,
    finished: Condvar,
}

impl ClosesInProgress {
    /// Registers a close for as long as the returned guard lives.
    fn begin(&self) -> CloseInProgress<'_> {
        *lock(&self.count) += 1;
        CloseInProgress(self)
    }

    /// Waits until no close is in progress, for at most `timeout`. Returns
    /// how many still were when it gave up.
    fn wait(&self, timeout: Duration) -> usize {
        let deadline = Instant::now().checked_add(timeout);
        let mut count = lock(&self.count);
        while *count > 0 {
            let remaining = deadline.map_or(Duration::MAX, |d| {
                d.saturating_duration_since(Instant::now())
            });
            if remaining.is_zero() {
                break;
            }
            count = self
                .finished
                .wait_timeout(count, remaining)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
        *count
    }
}

struct CloseInProgress<'a>(&'a ClosesInProgress);

impl Drop for CloseInProgress<'_> {
    fn drop(&mut self) {
        *lock(&self.0.count) -= 1;
        self.0.finished.notify_all();
    }
}

/// What a new session runs, and in a terminal of which size.
///
/// The default is the user's login shell
/// ([`PtyCommand::default_shell`]), started in their home directory, in a
/// terminal of 80 columns by 24 rows.
///
/// The type is `#[non_exhaustive]`: start from [`SessionOptions::default`]
/// or [`SessionOptions::new`] and change what differs.
///
/// # Examples
///
/// ```
/// use termx_core::pty::{PtyCommand, PtySize};
/// use termx_core::session::SessionOptions;
///
/// let shell = SessionOptions::default().size(PtySize::new(120, 40));
/// assert!(shell.command.is_login_shell() || cfg!(windows));
///
/// let git = SessionOptions::new(PtyCommand::new("git").arg("status").cwd("/tmp"));
/// assert_eq!(git.size, PtySize::default());
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SessionOptions {
    /// The program to run, its arguments, working directory and environment.
    pub command: PtyCommand,
    /// The initial size of the terminal.
    pub size: PtySize,
}

impl SessionOptions {
    /// Options for a session running `command` in a terminal of the default
    /// size.
    #[must_use]
    pub fn new(command: PtyCommand) -> Self {
        Self {
            command,
            size: PtySize::default(),
        }
    }

    /// Sets the initial size of the terminal.
    #[must_use]
    pub fn size(mut self, size: PtySize) -> Self {
        self.size = size;
        self
    }
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self::new(PtyCommand::default_shell())
    }
}

/// A thread-safe registry of running sessions, keyed by [`SessionId`].
///
/// See the [module documentation](super) for how output travels from a
/// session to its [`SessionSink`]. Identifiers are unique within one manager
/// and never reused.
///
/// Dropping the manager [shuts it down](Self::shutdown).
pub struct SessionManager {
    /// Shared with the driver thread, which removes the sessions that end.
    ptys: Arc<PtyManager>,
    events: Sender<Event>,
    driver: Mutex<Option<JoinHandle<()>>>,
    closes: ClosesInProgress,
}

impl SessionManager {
    /// Creates an empty manager and starts its driver thread.
    ///
    /// # Errors
    ///
    /// [`PtyError::Setup`] if the thread cannot be started.
    pub fn new() -> Result<Self, PtyError> {
        let ptys = Arc::new(PtyManager::new());
        let (events, receiver) = driver::channel();
        let thread = thread::Builder::new()
            .name("termx-session-driver".to_owned())
            .spawn({
                let ptys = Arc::clone(&ptys);
                move || {
                    driver::run(&receiver, |id| {
                        // The process has exited: dropping the terminal
                        // only releases what it still holds.
                        drop(ptys.remove(id));
                        debug!(session_id = id.as_u64(), "session removed after its exit");
                    });
                }
            })
            .map_err(|error| {
                PtyError::Setup(format!("cannot start the session driver thread: {error}"))
            })?;
        Ok(Self {
            ptys,
            events,
            driver: Mutex::new(Some(thread)),
            closes: ClosesInProgress::default(),
        })
    }

    /// Starts a session and returns its identifier.
    ///
    /// The output of the process, and eventually its exit, are delivered to
    /// `sink`. Output may reach the sink before this function returns.
    ///
    /// # Errors
    ///
    /// The same as [`Pty::spawn`](crate::pty::Pty::spawn), and
    /// [`PtyError::ManagerShutDown`] once [`shutdown`](Self::shutdown) has
    /// been called. Nothing is left running on error, and the sink is
    /// dropped.
    pub fn create(
        &self,
        options: &SessionOptions,
        sink: impl SessionSink,
    ) -> Result<SessionId, PtyError> {
        self.create_filtered(options, None, sink)
    }

    /// Like [`create`](Self::create), with the output passed through
    /// `filter` before anything else sees it.
    ///
    /// # Errors
    ///
    /// Those of [`create`](Self::create).
    pub fn create_filtered(
        &self,
        options: &SessionOptions,
        filter: Option<Box<dyn OutputFilter>>,
        sink: impl SessionSink,
    ) -> Result<SessionId, PtyError> {
        let (id, mut output) = self.ptys.spawn(&options.command, options.size)?;
        // Gone already if the manager was shut down in the meantime.
        let pty = self.ptys.get(id).ok_or(PtyError::ManagerShutDown)?;

        // The callback only carries what is needed to wake the driver up.
        // In particular it does not hold the terminal, which would keep it
        // alive for ever (see `PtyOutput::on_readable`).
        output.on_readable({
            let events = self.events.clone();
            move || {
                let _ = events.send(Event::Readable(id));
            }
        });
        let source = PtySource::new(output, Arc::downgrade(&pty));
        let process_id = pty.process_id();
        drop(pty);

        let pump = Pump::new(id, Box::new(source), Box::new(sink)).with_filter(filter);
        if self.events.send(Event::Add(id, Box::new(pump))).is_err() {
            // The driver thread is gone: the manager was shut down.
            self.kill_and_remove(id);
            return Err(PtyError::ManagerShutDown);
        }

        info!(
            session_id = id.as_u64(),
            program = %options.command.program().to_string_lossy(),
            login_shell = options.command.is_login_shell(),
            cols = options.size.cols,
            rows = options.size.rows,
            process_id,
            "session started"
        );
        Ok(id)
    }

    /// Sends input to a session, as if it had been typed. See
    /// [`Pty::write`](crate::pty::Pty::write).
    ///
    /// This blocks while the process is not reading its input and the
    /// system's input buffer is full, so do not call it from a thread that
    /// must stay responsive. Concurrent writes to the same session are each
    /// written whole, in an unspecified order: a caller for which the order
    /// matters must not start a write before the previous one has returned.
    ///
    /// # Errors
    ///
    /// [`PtyError::UnknownSession`] if the session does not exist or has
    /// ended, or the error of [`Pty::write`](crate::pty::Pty::write).
    pub fn write(&self, id: SessionId, data: &[u8]) -> Result<(), PtyError> {
        self.ptys.write(id, data)
    }

    /// Changes the size of the terminal of a session. Never blocks on the
    /// process. See [`Pty::resize`](crate::pty::Pty::resize).
    ///
    /// # Errors
    ///
    /// [`PtyError::UnknownSession`] if the session does not exist or has
    /// ended, or the error of [`Pty::resize`](crate::pty::Pty::resize).
    pub fn resize(&self, id: SessionId, size: PtySize) -> Result<(), PtyError> {
        self.ptys.resize(id, size)
    }

    /// Records that the frontend has finished processing `bytes` of the
    /// output of a session, which may let a paused session resume. See the
    /// [module documentation](super#the-path-of-the-output).
    ///
    /// Never blocks. An acknowledgement for a session that does not exist
    /// is ignored: the frontend is normally still processing output when a
    /// session ends.
    pub fn acknowledge(&self, id: SessionId, bytes: u64) {
        let _ = self.events.send(Event::Acknowledge(id, bytes));
    }

    /// Ends a session: its process is killed and its sink is dropped,
    /// without [`SessionSink::exited`] being called. Returns whether the
    /// session existed.
    ///
    /// Once this returns, the sink is not called again and the process is
    /// gone. The process is asked to terminate first and killed by force if
    /// it has not exited after its grace period
    /// ([`PtyCommand::kill_grace`]), so this returns at once for a process
    /// that exits when asked and after the grace period for one that does
    /// not. Use [`close_all`](Self::close_all) for several sessions.
    pub fn close(&self, id: SessionId) -> bool {
        self.close_all(&[id]) == 1
    }

    /// Ends several sessions at once, as [`close`](Self::close) does for
    /// one, and returns how many of them existed.
    ///
    /// Every process is asked to terminate before any of them is waited
    /// for, so closing many sessions takes about as long as closing the
    /// slowest one, not the sum of them all.
    ///
    /// A session stays registered until its process is gone. If the
    /// manager is shut down while this is waiting, for instance because the
    /// application exits, the shutdown waits for this call: no process is
    /// left behind half-killed.
    pub fn close_all(&self, ids: &[SessionId]) -> usize {
        let _in_progress = self.closes.begin();

        // The pumps go first, and with them the output streams, so that no
        // more output is delivered and a process that is busy printing can
        // exit.
        let (done_tx, done_rx) = mpsc::channel();
        let pumps = ids
            .iter()
            .filter(|&&id| {
                self.events
                    .send(Event::Remove(id, Some(done_tx.clone())))
                    .is_ok()
            })
            .count();
        drop(done_tx);
        for _ in 0..pumps {
            // An error means that the driver is gone, and the pumps with it.
            if done_rx.recv().is_err() {
                break;
            }
        }

        let sessions: Vec<(SessionId, Arc<Pty>)> = ids
            .iter()
            .filter_map(|&id| Some((id, self.ptys.get(id)?)))
            .collect();
        // Killing never blocks: every process is signalled before the
        // first one is waited for.
        let signalled: Vec<bool> = sessions.iter().map(|(_, pty)| pty.kill().is_ok()).collect();
        for ((id, pty), signalled) in sessions.iter().zip(signalled) {
            // A process that could not be signalled may never exit: waiting
            // for it would block for ever.
            if signalled {
                pty.wait();
            }
            // Only now, with the process gone, is the session forgotten.
            drop(self.ptys.remove(*id));
            info!(session_id = id.as_u64(), "session closed");
        }
        sessions.len()
    }

    /// Ends every session and closes the manager for good: from then on
    /// [`create`](Self::create) fails with [`PtyError::ManagerShutDown`].
    ///
    /// No sink is called once this returns, and none is told about an exit.
    /// Every process is asked to terminate before any of them is waited
    /// for, so this takes about as long as the slowest of them, not the sum
    /// (see [`PtyManager::shutdown`]). Closes in progress on other threads
    /// are waited for as well, for at most [`SHUTDOWN_CLOSE_WAIT`]. Call it
    /// before the application exits: no process started here outlives it.
    /// Called automatically when the manager is dropped.
    pub fn shutdown(&self) {
        let _ = self.events.send(Event::Shutdown);
        let driver = lock(&self.driver).take();
        if let Some(driver) = driver {
            // The thread only panics if a sink does; there is nothing to do
            // about it here, and the sessions are killed below either way.
            let _ = driver.join();
            info!(sessions = self.ptys.len(), "session manager shut down");
        }
        self.ptys.shutdown();

        // A session being closed on another thread was signalled, but its
        // process may still be within its grace period, and it is that
        // thread which holds it now. Returning before it is done would let
        // the application exit with the process alive.
        let unfinished = self.closes.wait(SHUTDOWN_CLOSE_WAIT);
        if unfinished > 0 {
            warn!(
                unfinished,
                "gave up waiting for sessions that are being closed"
            );
        }
    }

    /// The operating system's identifier of the process of a session, if
    /// the session exists and the identifier is known.
    #[must_use]
    pub fn process_id(&self, id: SessionId) -> Option<u32> {
        self.ptys.get(id)?.process_id()
    }

    /// The identifiers of the running sessions, in creation order.
    #[must_use]
    pub fn ids(&self) -> Vec<SessionId> {
        self.ptys.ids()
    }

    /// The number of running sessions.
    #[must_use]
    pub fn len(&self) -> usize {
        self.ptys.len()
    }

    /// Whether no session is running.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ptys.is_empty()
    }

    /// Kills the process of a session and waits for it to be gone. Returns
    /// whether the session was registered.
    fn kill_and_remove(&self, id: SessionId) -> bool {
        let Some(pty) = self.ptys.remove(id) else {
            return false;
        };
        // Dropping the handle kills too; asking first keeps the kill from
        // waiting behind another holder of the handle, such as a write in
        // progress.
        let _ = pty.kill();
        drop(pty);
        true
    }
}

impl Drop for SessionManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl std::fmt::Debug for SessionManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionManager")
            .field("sessions", &self.ptys.ids())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::pump::testing::RecordingSink;

    #[test]
    fn default_options_run_the_login_shell_in_a_default_sized_terminal() {
        let options = SessionOptions::default();

        assert_eq!(options.command, PtyCommand::default_shell());
        assert_eq!(options.size, PtySize::new(80, 24));
        assert_eq!(
            SessionOptions::default().size(PtySize::new(132, 43)).size,
            PtySize::new(132, 43)
        );
    }

    #[test]
    fn operations_on_an_unknown_session_fail_or_are_ignored() {
        let manager = SessionManager::new().unwrap();
        let id = SessionId::from_raw(7);

        assert!(manager.is_empty());
        assert!(manager.ids().is_empty());
        assert_eq!(manager.process_id(id), None);
        for error in [
            manager.write(id, b"x").unwrap_err(),
            manager.resize(id, PtySize::default()).unwrap_err(),
        ] {
            assert!(
                matches!(error, PtyError::UnknownSession(unknown) if unknown == id),
                "unexpected error: {error}"
            );
        }
        manager.acknowledge(id, 1024);
        assert!(!manager.close(id));
    }

    #[test]
    fn a_failed_create_registers_nothing() {
        let manager = SessionManager::new().unwrap();
        let sink = RecordingSink::default();

        let error = manager
            .create(&SessionOptions::new(PtyCommand::new("")), sink.clone())
            .unwrap_err();

        assert!(matches!(error, PtyError::EmptyProgram), "{error}");
        assert!(manager.is_empty());
        assert!(sink.deliveries().is_empty());
    }

    #[test]
    fn nothing_can_be_created_after_shutdown() {
        let manager = SessionManager::new().unwrap();
        manager.shutdown();

        let error = manager
            .create(&SessionOptions::default(), RecordingSink::default())
            .unwrap_err();

        assert!(matches!(error, PtyError::ManagerShutDown), "{error}");
        assert!(manager.is_empty());
        // The rest is harmless, as is shutting down again.
        manager.acknowledge(SessionId::from_raw(1), 1);
        assert!(!manager.close(SessionId::from_raw(1)));
        manager.shutdown();
    }
}
