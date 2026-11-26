//! A process running inside a PTY.

use std::io::Write;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use portable_pty::{MasterPty, PtyPair, native_pty_system};

use super::output::{self, OutputSink, PtyOutput};
use super::{ExitStatus, PtyCommand, PtyError, PtySize, lock, reader};

type Master = Box<dyn MasterPty + Send>;
type Writer = Box<dyn Write + Send>;
type Child = Box<dyn portable_pty::Child + Send + Sync>;

/// A process running inside a pseudo-terminal.
///
/// Created with [`Pty::spawn`], which also returns the [`PtyOutput`] carrying
/// what the process prints. The handle is used to send input, resize the
/// terminal, kill the process and observe its exit. Every method takes
/// `&self` and the type is `Send + Sync`, so it can be shared between threads
/// in an [`Arc`].
///
/// Dropping the handle kills the process if it is still running, reaps it and
/// joins the helper threads: nothing is left behind.
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
/// soon as the process is reaped. Killing must therefore be serialised with
/// reaping: both happen under this mutex, and the waiter thread polls instead
/// of blocking in `wait`.
#[cfg(unix)]
struct ChildControl {
    child: Arc<Mutex<Child>>,
    /// Wakes the waiter thread up for an immediate check.
    poke: std::sync::mpsc::Sender<()>,
}

/// On Windows a process is addressed by a handle, which stays valid after the
/// process exits. The waiter thread owns the child and blocks until it exits,
/// while this independent handle is used to terminate it.
#[cfg(windows)]
struct ChildControl {
    killer: Mutex<Box<dyn portable_pty::ChildKiller + Send + Sync>>,
}

/// Shortest and longest pause between two checks of whether the process has
/// exited (Unix only). The pause starts short, doubles up to the maximum
/// while nothing happens, and is reset whenever the reader thread reaches end
/// of file or the process is killed. In the usual case an exit is therefore
/// noticed immediately, and an idle session costs four cheap wake-ups per
/// second.
#[cfg(unix)]
const EXIT_POLL_MIN: Duration = Duration::from_millis(1);
#[cfg(unix)]
const EXIT_POLL_MAX: Duration = Duration::from_millis(250);

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
        let size = size.validate()?;
        let builder = command.to_builder()?;

        let PtyPair { slave, master } = native_pty_system()
            .openpty(size.to_native())
            .map_err(|error| PtyError::Open(format!("{error:#}")))?;
        let child = slave
            .spawn_command(builder)
            .map_err(|error| PtyError::Spawn {
                program: command.program().to_string_lossy().into_owned(),
                message: format!("{error:#}"),
            })?;
        // The process has its own handles to the terminal. Ours must be
        // closed, or end of file would never be reported on Unix.
        drop(slave);

        Self::attach(master, child, size)
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
    ) -> Result<(Self, PtyOutput), PtyError> {
        let streams = match Streams::open(master.as_ref()) {
            Ok(streams) => streams,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };

        let (sink, output) = output::channel();
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
            let child = Arc::new(Mutex::new(child));
            let reader_task = {
                let (sink, poke) = (sink.clone(), poke.clone());
                let (master, stop) = (streams.master, streams.stop_reader);
                move || {
                    reader::pump_until_stopped(&master, &stop, &sink);
                    sink.finish();
                    // End of file usually means the process is gone.
                    let _ = poke.send(());
                }
            };
            let waiter_task = {
                let (child, shared) = (Arc::clone(&child), Arc::clone(&shared));
                move || shared.finish(poll_exit(&child, &pokes))
            };
            (ChildControl { child, poke }, reader_task, waiter_task)
        };

        #[cfg(windows)]
        let (control, reader_task, waiter_task) = {
            let control = ChildControl {
                killer: Mutex::new(child.clone_killer()),
            };
            let reader_task = {
                let sink = sink.clone();
                let reader = streams.reader;
                move || {
                    reader::pump_blocking(reader, &sink);
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
    /// On Unix the process is first sent `SIGHUP`, as when a terminal window
    /// is closed, and `SIGKILL` if it is still alive after a short grace
    /// period (at most 250 ms, during which this call blocks). On Windows
    /// the process is terminated immediately, and the remaining processes
    /// attached to the console are terminated when the console is closed.
    ///
    /// Killing is asynchronous: use [`wait`](Self::wait) or
    /// [`wait_timeout`](Self::wait_timeout) to observe the exit.
    ///
    /// # Errors
    ///
    /// [`PtyError::Kill`] if the system refuses to signal the process.
    pub fn kill(&self) -> Result<(), PtyError> {
        if self.try_wait().is_some() {
            return Ok(());
        }
        #[cfg(unix)]
        {
            let mut child = lock(&self.control.child);
            // Holding the lock, the process cannot be reaped by the waiter
            // thread. Checking here guarantees that the signal goes to our
            // child and never to an unrelated process reusing its id.
            let result = match child.try_wait() {
                Ok(None) => child.kill().map_err(PtyError::Kill),
                Ok(Some(_)) | Err(_) => Ok(()),
            };
            drop(child);
            let _ = self.control.poke.send(());
            result
        }
        #[cfg(windows)]
        {
            // The result is deliberately ignored: portable-pty 0.9 reports a
            // successful `TerminateProcess` as an error and a failed one as
            // success. The only failure mode is a process that has already
            // exited, which is not an error for this method.
            let _ = lock(&self.control.killer).kill();
            Ok(())
        }
    }

    /// Returns the exit status if the process has exited, without blocking.
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
        let deadline = Instant::now().checked_add(timeout);
        let mut exit = lock(&self.shared.exit);
        loop {
            if let Some(status) = exit.as_ref() {
                return Some(status.clone());
            }
            let Some(deadline) = deadline else {
                // The timeout is too large to be represented: wait forever.
                drop(exit);
                return Some(self.wait());
            };
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return None;
            }
            exit = self
                .shared
                .exited
                .wait_timeout(exit, remaining)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
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

        let killed = self.kill().is_ok();
        self.shared.close();

        match self.waiter.take() {
            // If the process could not be killed (it may run as another
            // user), the waiter thread is left to finish on its own when the
            // process eventually exits, rather than blocking here forever.
            Some(waiter) if killed => {
                let _ = waiter.join();
            }
            Some(_) => {}
            // Spawning failed half-way and there is no waiter thread: reap
            // the process here so that it does not stay behind as a zombie.
            None =>
            {
                #[cfg(unix)]
                if killed {
                    let _ = lock(&self.control.child).wait();
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
            // keeps a PTY down to four descriptors (see `limits`), and the
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

/// Waits for the child to exit by polling it, and returns its status.
#[cfg(unix)]
fn poll_exit(child: &Mutex<Child>, pokes: &std::sync::mpsc::Receiver<()>) -> ExitStatus {
    use std::sync::mpsc::RecvTimeoutError;

    let mut pause = EXIT_POLL_MIN;
    loop {
        // The lock is released before pausing, so that `kill` can run.
        let polled = lock(child).try_wait();
        match polled {
            Ok(Some(status)) => return status.into(),
            Ok(None) => {}
            // Something else reaped the process, for example a host
            // application ignoring `SIGCHLD`. Its status is lost.
            Err(_) => return ExitStatus::unknown(),
        }
        match pokes.recv_timeout(pause) {
            Ok(()) => pause = EXIT_POLL_MIN,
            Err(RecvTimeoutError::Timeout) => pause = (pause * 2).min(EXIT_POLL_MAX),
            // Only when the handle was dropped without being able to kill the
            // process: keep polling at the slowest pace.
            Err(RecvTimeoutError::Disconnected) => {
                thread::sleep(EXIT_POLL_MAX);
                pause = EXIT_POLL_MAX;
            }
        }
    }
}
