//! A process running inside a PTY.

use std::io::Write;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use portable_pty::{MasterPty, PtyPair, native_pty_system};

use super::output::{self, OutputSink, PtyOutput};
#[cfg(unix)]
use super::tree;
use super::{ExitStatus, PtyCommand, PtyError, PtySize, lock, reader};
#[cfg(windows)]
use crate::diagnostics::tracing::warn;

type Master = Box<dyn MasterPty + Send>;
type Writer = Box<dyn Write + Send>;
type Child = Box<dyn portable_pty::Child + Send + Sync>;

/// Our own handle to the process side of the terminal, kept open by the
/// reader thread on macOS until it has finished reading; `None` elsewhere.
///
/// On macOS a process that exits while the terminal still holds output
/// nobody has read waits for it to be read, but only for about 600 ms. After
/// that the output is thrown away, unless something else still has the
/// terminal open, in which case the process keeps waiting. A reader thread
/// that the system schedules late (measured under heavy load) or a consumer
/// that is slow to take the output would therefore lose the last output of
/// the process. Keeping the terminal open on our side makes the exit wait
/// for as long as it takes: backpressure instead of loss.
///
/// End of file is still reported as soon as the process exits, because macOS
/// revokes the terminal of a session whose leader is gone. It costs a fifth
/// file descriptor per PTY on macOS.
type TerminalKeeper = Option<Box<dyn portable_pty::SlavePty + Send>>;

/// A process running inside a pseudo-terminal.
///
/// Created with [`Pty::spawn`], which also returns the [`PtyOutput`] carrying
/// what the process prints. The handle is used to send input, resize the
/// terminal, kill the process and observe its exit. Every method takes
/// `&self` and the type is `Send + Sync`, so it can be shared between threads
/// in an [`Arc`].
///
/// Dropping the handle kills the process if it is still running, reaps it and
/// joins the helper threads: nothing is left behind. This waits for the
/// process to be gone, which is immediate for one that exits when asked but
/// takes the whole [grace period](PtyCommand::kill_grace) for one that does
/// not. Where that matters, call [`kill`](Self::kill) first, which never
/// blocks, and drop the handle later or on another thread;
/// [`PtyManager::shutdown`](super::PtyManager::shutdown) does this for all
/// its terminals at once.
///
/// # Examples
///
/// ```no_run
/// use termx_core::pty::{Pty, PtyCommand, PtySize};
///
/// # fn main() -> Result<(), termx_core::pty::PtyError> {
/// let (pty, output) = Pty::spawn(&PtyCommand::default_shell(), PtySize::new(120, 40))?;
///
/// pty.write(b"echo hello\r")?;
/// pty.resize(PtySize::new(100, 30))?;
///
/// // Typically on a dedicated thread: forward the output to the UI.
/// for chunk in output {
///     print!("{}", String::from_utf8_lossy(&chunk));
/// }
/// println!("the shell {}", pty.wait());
/// # Ok(())
/// # }
/// ```
pub struct Pty {
    shared: Arc<Shared>,
    sink: OutputSink,
    control: ChildControl,
    process_id: Option<u32>,
    reader: Option<JoinHandle<()>>,
    waiter: Option<JoinHandle<()>>,
}

/// State shared between the handle and its helper threads.
struct Shared {
    exit: Mutex<Option<ExitStatus>>,
    exited: Condvar,
    terminal: Mutex<Terminal>,
    /// Separate from `terminal` so that a write blocked on a process that
    /// does not read its input never delays a resize.
    writer: Mutex<Option<Writer>>,
    /// Write end of the pipe that stops the reader thread; closing it is the
    /// signal.
    #[cfg(unix)]
    stop_reader: Mutex<Option<std::io::PipeWriter>>,
}

struct Terminal {
    /// `None` once the process has exited and the terminal was closed.
    master: Option<Master>,
    size: PtySize,
}

/// What is needed to kill the process, which differs per platform because of
/// how each one lets the exit of a process be observed.
///
/// On Unix a process is addressed by its id, which the system may reuse as
/// soon as the process is reaped. Signalling must therefore be serialised
/// with reaping: both happen under this mutex, and the waiter thread polls
/// instead of blocking in `wait`.
#[cfg(unix)]
struct ChildControl {
    state: Arc<Mutex<ChildState>>,
    /// Wakes the waiter thread up for an immediate check.
    poke: std::sync::mpsc::Sender<()>,
}

#[cfg(unix)]
struct ChildState {
    child: std::process::Child,
    /// The terminal, to ask it for its foreground process group.
    master: Arc<std::fs::File>,
    /// How long the processes get to exit after `SIGHUP` before `SIGKILL`.
    kill_grace: Duration,
    /// Set by `kill`: when the waiter thread must send `SIGKILL` to what is
    /// still alive of the session.
    force_kill_at: Option<Instant>,
    /// See [`PtyCommand::end_leftovers_on_exit`].
    end_leftovers_on_exit: bool,
    /// Whether the child has exited and its session was handed to the
    /// sweeper (see [`hand_over`]).
    handed_over: bool,
    /// Set by `kill` once the child has exited: when what it left must be
    /// killed by force.
    leftovers_deadline: Option<Instant>,
}

#[cfg(unix)]
impl ChildState {
    /// The child, which leads the session of the terminal: its identifier
    /// is also that of the session and of its own process group.
    fn leader(&self) -> Option<rustix::process::Pid> {
        rustix::process::Pid::from_raw(i32::try_from(self.child.id()).ok()?)
    }

    /// The process groups of the session, the child's first. Only called
    /// while the child has not been reaped (see [`ChildControl`]).
    fn groups(&self) -> Vec<rustix::process::Pid> {
        let Some(leader) = self.leader() else {
            return Vec::new();
        };
        let foreground = rustix::termios::tcgetpgrp(&*self.master).ok();
        tree::groups_of_live_session(leader, foreground)
    }

    /// Sends `SIGHUP` to every process group of the session, as closing a
    /// terminal window does, and `SIGCONT` so that a stopped job sees it.
    /// The error is that of signalling the group of the child, which is
    /// the one that must succeed.
    fn hang_up(&self) -> Result<(), PtyError> {
        use rustix::process::{Signal, kill_process_group};

        let groups = self.groups();
        let leader = groups
            .first()
            .copied()
            .ok_or_else(|| PtyError::Kill(std::io::Error::other("invalid process id")))?;
        kill_process_group(leader, Signal::HUP).map_err(|error| PtyError::Kill(error.into()))?;
        tree::signal_groups(&groups[1..], Signal::HUP);
        tree::signal_groups(&groups, Signal::CONT);
        Ok(())
    }

    /// Ends what the child left in its session, now that it has exited:
    /// `SIGHUP`, and `SIGKILL` after the grace period. Asked once; the
    /// sweeper does it, whether the session was handed over already or is
    /// about to be.
    fn end_leftovers(&mut self) {
        if self.leftovers_deadline.is_some() {
            return;
        }
        let deadline = after_grace(self.kill_grace);
        self.leftovers_deadline = Some(deadline);
        if self.handed_over
            && let Some(session) = self.leader()
        {
            tree::Sweeper::get().terminate_if_watched(session, deadline);
        }
    }

    /// Sends `SIGKILL` to the child and to every process group of its
    /// session.
    fn kill_session(&mut self) {
        let groups = self.groups();
        let _ = self.child.kill();
        tree::signal_groups(&groups, rustix::process::Signal::KILL);
    }
}

/// What becomes of the processes a child left in its session once it has
/// exited (Unix only). Decided under the lock, right after the child was
/// reaped, so that the session is handed to the [`tree::Sweeper`] while
/// its identifier is certainly still the terminal's.
#[cfg(unix)]
fn hand_over(state: &mut ChildState, forced: bool) -> Option<rustix::process::Pid> {
    state.handed_over = true;
    let session = state.leader()?;
    let sweeper = tree::Sweeper::get();
    if let Some(deadline) = state.leftovers_deadline {
        // Killed in the moment between its exit and now.
        sweeper.terminate(session, deadline, false);
        return None;
    }
    match state.force_kill_at.or(forced.then(Instant::now)) {
        // Killed: what is left was sent `SIGHUP` with the child, and gets
        // `SIGKILL` at the same deadline. Dropping the handle waits for it.
        Some(deadline) => {
            sweeper.terminate(session, deadline, true);
            Some(session)
        }
        None if state.end_leftovers_on_exit => {
            sweeper.terminate(session, after_grace(state.kill_grace), false);
            None
        }
        None => {
            sweeper.watch(session);
            None
        }
    }
}

/// Now plus `grace`, or a day from now if that cannot be represented.
#[cfg(unix)]
fn after_grace(grace: Duration) -> Instant {
    let now = Instant::now();
    now.checked_add(grace)
        .or_else(|| now.checked_add(Duration::from_secs(24 * 60 * 60)))
        .unwrap_or(now)
}

/// How long dropping a handle waits for the processes its child left,
/// beyond their grace period (Unix only).
#[cfg(unix)]
const LEFTOVER_WAIT_MARGIN: Duration = Duration::from_secs(3);

/// On Windows a process is addressed by a handle, which stays valid after the
/// process exits. The waiter thread owns the child and blocks until it exits,
/// while this independent handle is used to terminate it.
///
/// Everything the process starts belongs to `job` as well (see `job.rs`).
#[cfg(windows)]
struct ChildControl {
    killer: Mutex<Box<dyn portable_pty::ChildKiller + Send + Sync>>,
    /// `None` if the system refused to create it: then only the process
    /// and what is attached to its console are ended.
    job: Option<super::job::Job>,
}

/// How long the end of the output stream is held back for the exit status
/// to be published, once the terminal reports end of file (Unix only).
#[cfg(unix)]
const STREAM_END_GRACE: Duration = Duration::from_secs(1);

/// Shortest and longest pause between two checks of whether the process has
/// exited (Unix only).
///
/// The waiter thread does not rely on this polling in the cases that matter:
/// it is woken up at once when the reader thread reaches end of file, which
/// is what a process exiting normally causes, and when the process is killed.
/// The pause starts short so that those wake-ups, and short-lived commands,
/// are followed by a few quick checks, then doubles up to the maximum.
///
/// The slow polling that remains is a backstop for the one case nothing
/// announces: the process exits while something else, such as a background
/// job it started, keeps the terminal open. It costs one `waitpid` per second
/// and per terminal, and bounds the delay before such an exit is noticed to
/// one second. Blocking in `wait` instead would remove it, but reaping could
/// then no longer be serialised with `kill` (see [`ChildControl`]).
#[cfg(unix)]
const EXIT_POLL_MIN: Duration = Duration::from_millis(1);
#[cfg(unix)]
const EXIT_POLL_MAX: Duration = Duration::from_secs(1);

impl Pty {
    /// Opens a pseudo-terminal of the given size and starts `command` in it.
    ///
    /// Returns the handle to the process together with its output stream.
    ///
    /// # Errors
    ///
    /// - [`PtyError::EmptyProgram`], [`PtyError::LoginShellArguments`],
    ///   [`PtyError::InvalidWorkingDirectory`] or [`PtyError::InvalidSize`]
    ///   if the request is invalid; nothing is started in that case.
    /// - [`PtyError::Open`] if the system cannot create a pseudo-terminal.
    /// - [`PtyError::Spawn`] if the program cannot be started, for example
    ///   because it does not exist.
    /// - [`PtyError::Setup`] if the streams or helper threads cannot be
    ///   created; the process is killed before returning.
    pub fn spawn(command: &PtyCommand, size: PtySize) -> Result<(Self, PtyOutput), PtyError> {
        Self::spawn_with_reader_delay(command, size, Duration::ZERO)
    }

    /// Implements [`spawn`](Self::spawn). `reader_delay` holds the reader
    /// thread back before its first read, which lets a test reproduce a
    /// reader thread that the system is slow to schedule.
    fn spawn_with_reader_delay(
        command: &PtyCommand,
        size: PtySize,
        reader_delay: Duration,
    ) -> Result<(Self, PtyOutput), PtyError> {
        let size = size.validate()?;
        let builder = command.to_builder()?;

        let PtyPair { slave, master } = open_pty(size)?;
        let child = slave
            .spawn_command(builder)
            .map_err(|error| PtyError::Spawn {
                program: command.program().to_string_lossy().into_owned(),
                message: format!("{error:#}"),
            })?;
        // The process has its own handles to the terminal. Ours is closed,
        // or end of file would never be reported on Linux. macOS is the
        // opposite: see `TerminalKeeper`.
        #[cfg(target_os = "macos")]
        let keeper: TerminalKeeper = Some(slave);
        #[cfg(not(target_os = "macos"))]
        let keeper: TerminalKeeper = {
            drop(slave);
            None
        };

        Self::attach(
            master,
            child,
            size,
            command.kill_grace_period(),
            command.ends_leftovers_on_exit(),
            reader_delay,
            keeper,
        )
    }

    /// Wires a freshly spawned child up to the helper threads.
    ///
    /// From the moment the child exists, every error path must kill and reap
    /// it. That is done by building the `Pty` first and letting its `Drop`
    /// implementation clean up.
    fn attach(
        master: Master,
        mut child: Child,
        size: PtySize,
        kill_grace: Duration,
        end_leftovers_on_exit: bool,
        reader_delay: Duration,
        keeper: TerminalKeeper,
    ) -> Result<(Self, PtyOutput), PtyError> {
        // Windows terminates a process at once: there is no grace period,
        // its console needs no keeper, and the job ends what is left.
        #[cfg(windows)]
        let _ = (kill_grace, keeper, end_leftovers_on_exit);

        let streams = match Streams::open(master.as_ref()) {
            Ok(streams) => streams,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };

        // On Unix the child is a plain `std::process::Child`. Having the
        // concrete type gives access to an immediate `SIGKILL`, where the
        // trait object only offers a blocking "hang up, wait, then kill".
        #[cfg(unix)]
        let child = {
            let child: Box<dyn portable_pty::Child> = child;
            match child.downcast::<std::process::Child>() {
                Ok(child) => *child,
                Err(mut child) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(PtyError::Setup(
                        "the pseudo-terminal returned an unexpected kind of process".to_owned(),
                    ));
                }
            }
        };

        let (sink, output) = output::channel();
        #[cfg(unix)]
        let process_id = Some(child.id());
        #[cfg(windows)]
        let process_id = child.process_id();
        let shared = Arc::new(Shared {
            exit: Mutex::new(None),
            exited: Condvar::new(),
            terminal: Mutex::new(Terminal {
                master: Some(master),
                size,
            }),
            writer: Mutex::new(Some(streams.writer)),
            #[cfg(unix)]
            stop_reader: Mutex::new(Some(streams.stop_writer)),
        });

        #[cfg(unix)]
        let (control, reader_task, waiter_task) = {
            let (poke, pokes) = std::sync::mpsc::channel();
            let state = Arc::new(Mutex::new(ChildState {
                child,
                master: Arc::clone(&streams.master),
                kill_grace,
                force_kill_at: None,
                end_leftovers_on_exit,
                handed_over: false,
                leftovers_deadline: None,
            }));
            let reader_task = {
                let (sink, poke, shared) = (sink.clone(), poke.clone(), Arc::clone(&shared));
                let (master, stop) = (streams.master, streams.stop_reader);
                move || {
                    thread::sleep(reader_delay);
                    reader::pump_until_stopped(&master, &stop, &sink);
                    // Everything that will be delivered has been read: the
                    // terminal no longer needs to be kept open.
                    drop(keeper);
                    // End of file usually means the process is gone: have the
                    // waiter thread check now, and give it a moment to publish
                    // the exit status before the stream ends, so that a
                    // consumer reaching the end of the stream finds the
                    // status. The wait is bounded because a process may close
                    // its terminal and keep running.
                    let _ = poke.send(());
                    if !sink.is_shut_down()
                        && let Some(deadline) = Instant::now().checked_add(STREAM_END_GRACE)
                    {
                        let _ = shared.wait_until(deadline);
                    }
                    sink.finish();
                }
            };
            let waiter_task = {
                let (state, shared) = (Arc::clone(&state), Arc::clone(&shared));
                move || {
                    let (status, killed) = poll_exit(&state, &pokes);
                    shared.finish(status);
                    // A terminal that was killed is only gone once what it
                    // started is: dropping the handle joins this thread.
                    if let Some(session) = killed {
                        let grace = lock(&state).kill_grace;
                        tree::Sweeper::get()
                            .wait(session, after_grace(grace + LEFTOVER_WAIT_MARGIN));
                    }
                }
            };
            (ChildControl { state, poke }, reader_task, waiter_task)
        };

        #[cfg(windows)]
        let (control, reader_task, waiter_task) = {
            let job = child.as_raw_handle().and_then(|process| {
                super::job::Job::containing(process)
                    .map_err(|error| {
                        warn!(error = %error, "a terminal could not be put in a job; what it starts may outlive it");
                    })
                    .ok()
            });
            let control = ChildControl {
                killer: Mutex::new(child.clone_killer()),
                job,
            };
            let reader_task = {
                let (sink, shared) = (sink.clone(), Arc::clone(&shared));
                let reader = streams.reader;
                move || {
                    thread::sleep(reader_delay);
                    // The console's start-up question is answered here, so
                    // that output flows even when no terminal emulator is
                    // attached to answer it (see `StartupQueryFilter`).
                    reader::pump_console(reader, &sink, || {
                        if let Some(writer) = lock(&shared.writer).as_mut() {
                            let _ = writer
                                .write_all(reader::CURSOR_POSITION_REPORT)
                                .and_then(|()| writer.flush());
                        }
                    });
                    sink.finish();
                }
            };
            let waiter_task = {
                let shared = Arc::clone(&shared);
                move || {
                    let status = child
                        .wait()
                        .map_or_else(|_| ExitStatus::unknown(), ExitStatus::from);
                    shared.finish(status);
                }
            };
            (control, reader_task, waiter_task)
        };

        let mut pty = Self {
            shared,
            sink,
            control,
            process_id,
            reader: None,
            waiter: None,
        };
        pty.reader = Some(spawn_thread("termx-pty-reader", reader_task)?);
        pty.waiter = Some(spawn_thread("termx-pty-waiter", waiter_task)?);
        Ok((pty, output))
    }

    /// Sends input to the process, as if it had been typed.
    ///
    /// The bytes are passed through unmodified. Note that the Enter key of a
    /// terminal sends a carriage return (`\r`), not a line feed.
    ///
    /// This blocks while the process is not reading its input and the
    /// system's input buffer is full.
    ///
    /// # Errors
    ///
    /// [`PtyError::Exited`] if the process has exited, and
    /// [`PtyError::Write`] if the system rejects the write, which can happen
    /// in the instant between the process exiting and its exit being
    /// observed.
    pub fn write(&self, data: &[u8]) -> Result<(), PtyError> {
        self.ensure_running()?;
        let mut writer = lock(&self.shared.writer);
        let writer = writer.as_mut().ok_or(PtyError::Exited)?;
        writer
            .write_all(data)
            .and_then(|()| writer.flush())
            .map_err(PtyError::Write)
    }

    /// Changes the size of the terminal and notifies the process (`SIGWINCH`
    /// on Unix, a console resize event on Windows).
    ///
    /// # Errors
    ///
    /// [`PtyError::InvalidSize`] if a dimension is zero or too large,
    /// [`PtyError::Exited`] if the process has exited, and
    /// [`PtyError::Resize`] if the system rejects the new size.
    pub fn resize(&self, size: PtySize) -> Result<(), PtyError> {
        let size = size.validate()?;
        self.ensure_running()?;
        let mut terminal = lock(&self.shared.terminal);
        let master = terminal.master.as_ref().ok_or(PtyError::Exited)?;
        master
            .resize(size.to_native())
            .map_err(|error| PtyError::Resize(format!("{error:#}")))?;
        terminal.size = size;
        Ok(())
    }

    /// The size the terminal was created with or last resized to.
    #[must_use]
    pub fn size(&self) -> PtySize {
        lock(&self.shared.terminal).size
    }

    /// The operating system's identifier of the process, if known.
    #[must_use]
    pub fn process_id(&self) -> Option<u32> {
        self.process_id
    }

    /// Kills the process. Does nothing if it has already exited.
    ///
    /// This never blocks: it only requests the termination, so it is safe to
    /// call from an asynchronous context. Use [`wait`](Self::wait) or
    /// [`wait_timeout`](Self::wait_timeout) to observe the exit.
    ///
    /// Everything the process started is ended with it, not only the
    /// process itself. On Unix every process group of its session is sent
    /// `SIGHUP`, as when a terminal window is closed, which lets programs
    /// save their state and exit on their own. Whatever is still alive
    /// after the grace period of the command ([`PtyCommand::kill_grace`],
    /// three seconds by default), including processes that ignore
    /// `SIGHUP`, is sent `SIGKILL` in the background. On Windows the
    /// process and everything it started are terminated immediately. A
    /// process that leaves on purpose (a daemon that starts a session of
    /// its own on Unix, or breaks away from the job on Windows) is not
    /// followed.
    ///
    /// On macOS a process, even a killed one, only finishes exiting once the
    /// output it has already printed has been read from the terminal. If the
    /// consumer of the [`PtyOutput`] has stopped receiving while its buffer
    /// is full, the exit is observed when it resumes, when the `PtyOutput`
    /// is dropped, or when this handle is dropped.
    ///
    /// # Errors
    ///
    /// [`PtyError::Kill`] if the system refuses to signal the process.
    pub fn kill(&self) -> Result<(), PtyError> {
        self.kill_inner(true)
    }

    /// [`kill`](Self::kill). Without `leftovers`, a process that has exited
    /// already is left alone with what it left: that is how dropping the
    /// handle of a terminal that ended on its own behaves.
    fn kill_inner(&self, leftovers: bool) -> Result<(), PtyError> {
        #[cfg(windows)]
        let _ = leftovers;
        #[cfg(windows)]
        if self.try_wait().is_some() {
            return Ok(());
        }
        #[cfg(unix)]
        {
            let mut state = lock(&self.control.state);
            // Holding the lock, the process cannot be reaped by the waiter
            // thread. Checking here guarantees that the signal goes to our
            // child and never to an unrelated process reusing its id.
            let result = match state.child.try_wait() {
                // A second call neither repeats the signal nor postpones the
                // forced kill.
                Ok(None) if state.force_kill_at.is_none() => {
                    let sent = state.hang_up();
                    if sent.is_ok() {
                        state.force_kill_at = Instant::now().checked_add(state.kill_grace);
                    }
                    sent
                }
                Ok(None) => Ok(()),
                // It has exited: what it left in its session may still be
                // there, and is ended too.
                Ok(Some(_)) | Err(_) => {
                    if leftovers {
                        state.end_leftovers();
                    }
                    Ok(())
                }
            };
            drop(state);
            // The waiter thread takes it from here: it notices the exit, or
            // forces it once the grace period is over.
            let _ = self.control.poke.send(());
            result
        }
        #[cfg(windows)]
        {
            // The result is deliberately ignored: portable-pty 0.9 reports a
            // successful `TerminateProcess` as an error and a failed one as
            // success. The only failure mode is a process that has already
            // exited, which is not an error for this method.
            if let Some(job) = &self.control.job {
                job.terminate();
            }
            let _ = lock(&self.control.killer).kill();
            Ok(())
        }
    }

    /// Returns the exit status if the process has exited, without blocking.
    ///
    /// Once the [`PtyOutput`] has ended, this returns the status right away
    /// in every ordinary case. The one exception is a process that closes
    /// its terminal and keeps running: its output ends while it is alive.
    /// Code that needs the status should therefore call
    /// [`wait`](Self::wait) or [`wait_timeout`](Self::wait_timeout) rather
    /// than assume it is there.
    #[must_use]
    pub fn try_wait(&self) -> Option<ExitStatus> {
        lock(&self.shared.exit).clone()
    }

    /// Blocks until the process exits and returns its exit status.
    ///
    /// The exit status is available as soon as the process ends, possibly
    /// before the [`PtyOutput`] has delivered its last bytes. It can be
    /// called any number of times, from any thread.
    pub fn wait(&self) -> ExitStatus {
        let mut exit = lock(&self.shared.exit);
        loop {
            if let Some(status) = exit.as_ref() {
                return status.clone();
            }
            exit = self
                .shared
                .exited
                .wait(exit)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    /// Like [`wait`](Self::wait), but gives up after `timeout` and returns
    /// `None` if the process is still running.
    #[must_use]
    pub fn wait_timeout(&self, timeout: Duration) -> Option<ExitStatus> {
        match Instant::now().checked_add(timeout) {
            Some(deadline) => self.shared.wait_until(deadline),
            // The timeout is too large to be represented: wait forever.
            None => Some(self.wait()),
        }
    }
}

impl Pty {
    /// Fails with [`PtyError::Exited`] once the exit status is published, so
    /// that the outcome of an operation does not depend on how far the
    /// waiter thread got in closing the terminal.
    fn ensure_running(&self) -> Result<(), PtyError> {
        match self.try_wait() {
            Some(_) => Err(PtyError::Exited),
            None => Ok(()),
        }
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        // From now on output is discarded, so that the reader thread cannot
        // be stuck on a consumer that is slow or gone. On Windows it has to
        // keep draining for the console to close.
        self.sink.shut_down();

        // A terminal that ended on its own keeps what it left, as in other
        // terminals; one that was killed is waited for, leftovers included.
        let killed = self.kill_inner(false).is_ok();
        self.shared.close();

        match self.waiter.take() {
            // If the process could not be killed (it may run as another
            // user), the waiter thread is left to finish on its own when the
            // process eventually exits, rather than blocking here forever.
            Some(waiter) if killed => {
                let _ = waiter.join();
                #[cfg(unix)]
                {
                    let state = lock(&self.control.state);
                    let ending = state.leftovers_deadline.zip(state.leader());
                    drop(state);
                    if let Some((deadline, session)) = ending {
                        tree::Sweeper::get().wait(session, deadline + LEFTOVER_WAIT_MARGIN);
                    }
                }
            }
            Some(_) => {}
            // Spawning failed half-way and there is no waiter thread: reap
            // the process here so that it does not stay behind as a zombie.
            None =>
            {
                #[cfg(unix)]
                if killed {
                    let mut state = lock(&self.control.state);
                    let _ = state.child.kill();
                    let _ = state.child.wait();
                }
            }
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

impl std::fmt::Debug for Pty {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pty")
            .field("process_id", &self.process_id)
            .field("size", &self.size())
            .field("exit_status", &self.try_wait())
            .finish_non_exhaustive()
    }
}

impl Shared {
    /// Waits for the exit status until `deadline`.
    fn wait_until(&self, deadline: Instant) -> Option<ExitStatus> {
        let mut exit = lock(&self.exit);
        loop {
            if let Some(status) = exit.as_ref() {
                return Some(status.clone());
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return None;
            }
            exit = self
                .exited
                .wait_timeout(exit, remaining)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
    }

    /// Publishes the exit status, then closes the terminal.
    fn finish(&self, status: ExitStatus) {
        *lock(&self.exit) = Some(status);
        self.exited.notify_all();
        self.close();
    }

    /// Closes our side of the terminal and tells the reader thread to stop.
    ///
    /// On Windows, closing the pseudo-console is what makes the reader reach
    /// end of file, after the console has flushed its remaining output; a
    /// process exiting is not enough. Older versions of Windows block in that
    /// call until the output has been read, so it must happen outside of any
    /// lock and while the reader thread is still running.
    fn close(&self) {
        #[cfg(unix)]
        drop(lock(&self.stop_reader).take());

        let master = lock(&self.terminal).master.take();
        drop(master);

        let writer = lock(&self.writer).take();
        drop(writer);
    }
}

/// How many times creating a pseudo-terminal is attempted, and the pause
/// before the second attempt (doubled before each further one).
const OPEN_ATTEMPTS: u32 = 5;
const OPEN_RETRY_PAUSE: Duration = Duration::from_millis(5);

/// Creates a pseudo-terminal, one caller at a time.
///
/// `openpty(3)` is not safe to call from several threads at once on macOS:
/// concurrent calls fail at random ("Unknown error: -6"), which used to break
/// a third of the spawns when many sessions were started in parallel. The
/// call is quick, so a process-wide lock costs nothing; the retries cover a
/// transient failure caused by something outside this module.
fn open_pty(size: PtySize) -> Result<PtyPair, PtyError> {
    static OPENING: Mutex<()> = Mutex::new(());

    let mut pause = OPEN_RETRY_PAUSE;
    let mut attempt = 1;
    loop {
        let result = {
            let _guard = lock(&OPENING);
            native_pty_system().openpty(size.to_native())
        };
        match result {
            Ok(pair) => return Ok(pair),
            Err(error) if attempt == OPEN_ATTEMPTS => {
                return Err(PtyError::Open(format!("{error:#}")));
            }
            Err(_) => {
                thread::sleep(pause);
                pause *= 2;
                attempt += 1;
            }
        }
    }
}

/// The streams of a new terminal, opened before any thread is started so that
/// a failure can be handled in one place.
struct Streams {
    writer: Writer,
    /// The descriptor the reader thread reads from, shared with `writer`.
    #[cfg(unix)]
    master: Arc<std::fs::File>,
    #[cfg(unix)]
    stop_reader: std::io::PipeReader,
    #[cfg(unix)]
    stop_writer: std::io::PipeWriter,
    #[cfg(windows)]
    reader: Box<dyn std::io::Read + Send>,
}

impl Streams {
    fn open(master: &(dyn MasterPty + Send)) -> Result<Self, PtyError> {
        let setup = |error: &dyn std::fmt::Display| PtyError::Setup(error.to_string());

        #[cfg(unix)]
        {
            let fd = master
                .as_raw_fd()
                .ok_or_else(|| setup(&"the pseudo-terminal has no file descriptor"))?;
            // One owned duplicate of the master serves both directions: a
            // `&File` can be read and written from different threads. This
            // saves a descriptor per PTY (see `limits`), and the
            // reader thread never depends on the lifetime of the master.
            let master = filedescriptor::FileDescriptor::dup(&fd)
                .and_then(|duplicate| duplicate.as_file())
                .map(Arc::new)
                .map_err(|error| setup(&error))?;
            let (stop_reader, stop_writer) = std::io::pipe().map_err(|error| setup(&error))?;
            Ok(Self {
                writer: Box::new(SharedFile(Arc::clone(&master))),
                master,
                stop_reader,
                stop_writer,
            })
        }
        #[cfg(windows)]
        {
            let writer = master
                .take_writer()
                .map_err(|error| setup(&format!("{error:#}")))?;
            let reader = master
                .try_clone_reader()
                .map_err(|error| setup(&format!("{error:#}")))?;
            Ok(Self { writer, reader })
        }
    }
}

/// Write half of the duplicated master descriptor.
#[cfg(unix)]
struct SharedFile(Arc<std::fs::File>);

#[cfg(unix)]
impl Write for SharedFile {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        (&*self.0).write(buffer)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn spawn_thread(
    name: &str,
    task: impl FnOnce() + Send + 'static,
) -> Result<JoinHandle<()>, PtyError> {
    thread::Builder::new()
        .name(name.to_owned())
        .spawn(task)
        .map_err(|error| PtyError::Setup(format!("cannot start the {name} thread: {error}")))
}

/// Waits for the child to exit by polling it, and returns its status. Also
/// carries out the forced kill requested by [`Pty::kill`].
///
/// When the process was killed, also returns its session, whose leftovers
/// the [`tree::Sweeper`] is ending.
#[cfg(unix)]
fn poll_exit(
    state: &Mutex<ChildState>,
    pokes: &std::sync::mpsc::Receiver<()>,
) -> (ExitStatus, Option<rustix::process::Pid>) {
    use std::sync::mpsc::RecvTimeoutError;

    let mut pause = EXIT_POLL_MIN;
    // Set once the grace period is over and `SIGKILL` was sent.
    let mut forced = false;
    loop {
        // The lock is released before pausing, so that `kill` can run.
        let until_forced_kill = {
            let mut state = lock(state);
            match state.child.try_wait() {
                Ok(Some(status)) => {
                    return (
                        portable_pty::ExitStatus::from(status).into(),
                        hand_over(&mut state, forced),
                    );
                }
                Ok(None) => {}
                // Something else reaped the process, for example a host
                // application ignoring `SIGCHLD`. Its status is lost.
                Err(_) => return (ExitStatus::unknown(), hand_over(&mut state, forced)),
            }
            let now = Instant::now();
            if state.force_kill_at.is_some_and(|at| at <= now) {
                // The grace period is over: `SIGKILL`. The process was just
                // seen alive under the lock, so its id is still ours.
                state.kill_session();
                state.force_kill_at = None;
                forced = true;
                pause = EXIT_POLL_MIN;
            }
            state
                .force_kill_at
                .map(|at| at.saturating_duration_since(now))
        };

        let timeout = until_forced_kill.map_or(pause, |until| until.min(pause));
        match pokes.recv_timeout(timeout) {
            Ok(()) => pause = EXIT_POLL_MIN,
            Err(RecvTimeoutError::Timeout) => pause = (pause * 2).min(EXIT_POLL_MAX),
            // Only when the handle was dropped without being able to signal
            // the process: keep polling at the slowest pace.
            Err(RecvTimeoutError::Disconnected) => {
                thread::sleep(timeout);
                pause = EXIT_POLL_MAX;
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn output_of_a_process_that_exits_before_it_is_read_is_not_lost() {
        // The process prints and exits long before the reader thread gets to
        // read, as happens for real when the machine is very busy. One
        // second is beyond the time macOS keeps unread output for.
        let command = PtyCommand::new("/bin/echo").arg("printed-before-reading");
        let (pty, output) =
            Pty::spawn_with_reader_delay(&command, PtySize::default(), Duration::from_secs(1))
                .unwrap();

        let bytes: Vec<u8> = output.flatten().collect();
        let text = String::from_utf8_lossy(&bytes);

        assert!(text.contains("printed-before-reading"), "got: {text:?}");
        assert!(pty.wait().success());
    }
}
