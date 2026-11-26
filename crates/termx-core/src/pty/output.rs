//! The output stream of a PTY: a bounded byte buffer between the reader
//! thread and the consumer.

use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use super::lock;

/// Maximum number of bytes of output buffered for a consumer that is not
/// keeping up, before the reader thread stops reading from the PTY.
///
/// Once the reader stops, the kernel's own PTY buffer fills up and the child
/// process blocks in `write`, which is the backpressure a terminal is expected
/// to apply. The buffer may exceed this limit by at most one read
/// ([`READ_BUFFER_SIZE`](super::READ_BUFFER_SIZE) bytes).
pub const OUTPUT_BUFFER_CAPACITY: usize = 1024 * 1024;

type ReadyCallback = Arc<dyn Fn() + Send + Sync>;

#[derive(Default)]
struct State {
    /// Bytes read from the PTY and not yet taken by the consumer.
    buffer: Vec<u8>,
    /// The reader thread is done: no more bytes will ever be pushed.
    finished: bool,
    /// The [`PtyOutput`] was dropped: nobody will take the bytes.
    receiver_dropped: bool,
    /// The [`Pty`](super::Pty) is being dropped: stop as soon as possible.
    shut_down: bool,
    /// Called when there is something new to receive.
    on_readable: Option<ReadyCallback>,
}

#[derive(Default)]
struct Channel {
    state: Mutex<State>,
    /// Signalled when bytes are pushed or the stream finishes.
    readable: Condvar,
    /// Signalled when the buffer is emptied or pushing becomes pointless.
    writable: Condvar,
}

/// Creates a connected sink (reader thread side) and output (consumer side).
pub(super) fn channel() -> (OutputSink, PtyOutput) {
    let channel = Arc::new(Channel::default());
    (
        OutputSink {
            channel: Arc::clone(&channel),
        },
        PtyOutput { channel },
    )
}

/// Producer side of the output stream, owned by the reader thread (and by the
/// [`Pty`](super::Pty), to shut it down).
#[derive(Clone)]
pub(super) struct OutputSink {
    channel: Arc<Channel>,
}

impl OutputSink {
    /// Appends `bytes` to the buffer, blocking while the buffer is full.
    ///
    /// The bytes are silently dropped when nobody can receive them any more,
    /// so that the reader thread keeps draining the PTY and the child process
    /// never blocks on a consumer that went away.
    pub(super) fn push(&self, bytes: &[u8]) {
        let mut state = lock(&self.channel.state);
        while state.buffer.len() >= OUTPUT_BUFFER_CAPACITY
            && !state.receiver_dropped
            && !state.shut_down
        {
            state = self
                .channel
                .writable
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        if state.receiver_dropped || state.shut_down {
            return;
        }
        // The callback is edge-triggered: it fires when the buffer stops
        // being empty, not for every push into a buffer the consumer has
        // already been told about.
        let callback = if state.buffer.is_empty() && !bytes.is_empty() {
            state.on_readable.clone()
        } else {
            None
        };
        state.buffer.extend_from_slice(bytes);
        drop(state);
        self.channel.readable.notify_one();
        if let Some(callback) = callback {
            self.notify(&callback);
        }
    }

    /// Calls the consumer's callback, without the lock so that it may use
    /// the output.
    ///
    /// The callback is foreign code running on the reader thread. If it
    /// panicked through here, the thread would die without ever ending the
    /// stream: the consumer would wait forever and the process would block
    /// once the buffer is full. The panic is contained instead, and the
    /// callback is unregistered, since it can no longer be trusted.
    fn notify(&self, callback: &ReadyCallback) {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback()));
        if outcome.is_err() {
            let broken = lock(&self.channel.state).on_readable.take();
            // Dropped outside the lock: its captures may do anything.
            drop(broken);
        }
    }

    /// Marks the end of the stream. Buffered bytes can still be received.
    pub(super) fn finish(&self) {
        let (callback, shut_down) = {
            let mut state = lock(&self.channel.state);
            state.finished = true;
            (state.on_readable.clone(), state.shut_down)
        };
        self.channel.readable.notify_all();
        // Not called when the `Pty` is being dropped: the dropping thread is
        // waiting for this one, so a slow callback would delay it and one
        // needing a lock held by that thread would deadlock.
        if let Some(callback) = callback
            && !shut_down
        {
            self.notify(&callback);
        }
    }

    /// Tells the reader thread to stop forwarding output and to stop as soon
    /// as it can. Called when the [`Pty`](super::Pty) is dropped.
    pub(super) fn shut_down(&self) {
        lock(&self.channel.state).shut_down = true;
        self.channel.writable.notify_all();
    }

    /// Whether [`shut_down`](Self::shut_down) was called. Only the Unix
    /// reader loop can be interrupted, so only it asks.
    #[cfg(any(unix, test))]
    pub(super) fn is_shut_down(&self) -> bool {
        lock(&self.channel.state).shut_down
    }
}

/// Why [`PtyOutput::recv_timeout`] returned without output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RecvTimeoutError {
    /// No output arrived before the timeout. The stream is still open.
    #[error("timed out waiting for terminal output")]
    Timeout,
    /// The stream ended: every byte has been received and no more will come.
    #[error("the terminal output stream has ended")]
    Closed,
}

/// The output of a process running in a PTY, as a stream of byte chunks.
///
/// Obtained from [`Pty::spawn`](super::Pty::spawn). See the
/// [module documentation](super#output-stream) for the design.
///
/// The bytes are exactly what the process wrote to its terminal, in order and
/// unmodified. **Chunk boundaries are arbitrary**: a chunk may end in the
/// middle of a UTF-8 character or of an escape sequence, so never decode or
/// parse a chunk on its own; feed the chunks to a stateful decoder (xterm.js
/// does this for you).
///
/// Each call returns *everything* buffered since the previous one, so a slow
/// consumer automatically receives fewer, larger chunks.
///
/// The stream ends when the process has exited and its remaining output has
/// been delivered, or when the [`Pty`](super::Pty) is dropped. The exit
/// status is published before the stream ends, so it can be read with
/// [`Pty::wait`](super::Pty::wait) as soon as `recv` returns `None` (see
/// [`Pty::try_wait`](super::Pty::try_wait) for the one exception). Dropping
/// the `PtyOutput` does not affect the process: its output is then discarded.
///
/// `PtyOutput` is also an [`Iterator`] over the chunks.
pub struct PtyOutput {
    channel: Arc<Channel>,
}

impl PtyOutput {
    /// Blocks until output is available and returns it, or returns `None`
    /// once the stream has ended.
    pub fn recv(&mut self) -> Option<Vec<u8>> {
        self.recv_until(None).ok()
    }

    /// Like [`recv`](Self::recv), but gives up after `timeout`.
    ///
    /// A zero timeout makes this a non-blocking poll.
    ///
    /// # Errors
    ///
    /// [`RecvTimeoutError::Timeout`] if no output arrived in time, and
    /// [`RecvTimeoutError::Closed`] once the stream has ended.
    pub fn recv_timeout(&mut self, timeout: Duration) -> Result<Vec<u8>, RecvTimeoutError> {
        // A timeout too large to be represented means "wait forever".
        self.recv_until(Instant::now().checked_add(timeout))
    }

    /// Registers a callback invoked whenever there is something new to
    /// receive, as an alternative to dedicating a thread to blocking in
    /// [`recv`](Self::recv).
    ///
    /// The callback is called each time output arrives while the buffer is
    /// empty, and once when the stream ends. If output is already waiting
    /// (or the stream has already ended) it is called immediately. After a
    /// call, drain the stream with `recv_timeout(Duration::ZERO)` until it
    /// reports [`Timeout`](RecvTimeoutError::Timeout) or
    /// [`Closed`](RecvTimeoutError::Closed); the callback will not fire
    /// again before the buffer has been emptied.
    ///
    /// The callback runs on the PTY's reader thread (or on the calling
    /// thread for the immediate call), so it must return quickly and must
    /// not block: typically it only wakes up an event loop or an async
    /// task, which then does the draining. Backpressure is unaffected: the
    /// output stays in the bounded buffer until it is received.
    ///
    /// Registering a callback replaces the previous one.
    ///
    /// The end of the stream is **not** announced when it is caused by the
    /// [`Pty`](super::Pty) being dropped, because the dropping thread is
    /// waiting for the reader thread; whoever drops the `Pty` knows the
    /// session is over.
    ///
    /// If the callback panics, the panic is contained so that the session
    /// keeps working, and the callback is unregistered: it is never called
    /// again. The stream must then be read with [`recv`](Self::recv) or
    /// after registering a new callback.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::sync::mpsc;
    /// use std::time::Duration;
    /// use termx_core::pty::{Pty, PtyCommand, PtySize, RecvTimeoutError};
    ///
    /// # fn main() -> Result<(), termx_core::pty::PtyError> {
    /// let (pty, mut output) = Pty::spawn(&PtyCommand::default_shell(), PtySize::default())?;
    ///
    /// // One event loop can serve every terminal: no thread per session.
    /// let (events, event_loop) = mpsc::channel();
    /// output.on_readable(move || {
    ///     let _ = events.send("terminal 1 is readable");
    /// });
    ///
    /// for _event in event_loop {
    ///     loop {
    ///         match output.recv_timeout(Duration::ZERO) {
    ///             Ok(chunk) => { /* forward the chunk to the UI */ }
    ///             Err(RecvTimeoutError::Timeout) => break,
    ///             Err(RecvTimeoutError::Closed) => return Ok(()),
    ///         }
    ///     }
    /// }
    /// # drop(pty);
    /// # Ok(())
    /// # }
    /// ```
    pub fn on_readable(&mut self, callback: impl Fn() + Send + Sync + 'static) {
        let callback: ReadyCallback = Arc::new(callback);
        let ready_now = {
            let mut state = lock(&self.channel.state);
            state.on_readable = Some(Arc::clone(&callback));
            !state.buffer.is_empty() || state.finished
        };
        if ready_now {
            callback();
        }
    }

    /// Waits for output until `deadline`, or forever if it is `None`.
    fn recv_until(&mut self, deadline: Option<Instant>) -> Result<Vec<u8>, RecvTimeoutError> {
        let mut state = lock(&self.channel.state);
        loop {
            if !state.buffer.is_empty() {
                let chunk = std::mem::take(&mut state.buffer);
                drop(state);
                self.channel.writable.notify_one();
                return Ok(chunk);
            }
            if state.finished {
                return Err(RecvTimeoutError::Closed);
            }
            state = match deadline {
                None => self
                    .channel
                    .readable
                    .wait(state)
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
                Some(deadline) => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return Err(RecvTimeoutError::Timeout);
                    }
                    self.channel
                        .readable
                        .wait_timeout(state, remaining)
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .0
                }
            };
        }
    }
}

impl Iterator for PtyOutput {
    type Item = Vec<u8>;

    fn next(&mut self) -> Option<Self::Item> {
        self.recv()
    }
}

impl Drop for PtyOutput {
    fn drop(&mut self) {
        let mut state = lock(&self.channel.state);
        state.receiver_dropped = true;
        state.buffer = Vec::new();
        let callback = state.on_readable.take();
        drop(state);
        self.channel.writable.notify_all();
        drop(callback);
    }
}

impl std::fmt::Debug for PtyOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = lock(&self.channel.state);
        f.debug_struct("PtyOutput")
            .field("buffered", &state.buffer.len())
            .field("finished", &state.finished)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::thread;

    const SOON: Duration = Duration::from_secs(10);

    #[test]
    fn pushed_bytes_are_coalesced_into_one_chunk() {
        let (sink, mut output) = channel();
        sink.push(b"hello ");
        sink.push(b"world");

        assert_eq!(output.recv_timeout(SOON).unwrap(), b"hello world");
    }

    #[test]
    fn bytes_are_not_altered() {
        let (sink, mut output) = channel();
        let bytes: Vec<u8> = (0..=255).collect();
        sink.push(&bytes);

        assert_eq!(output.recv_timeout(SOON).unwrap(), bytes);
    }

    #[test]
    fn recv_times_out_while_the_stream_is_open() {
        let (_sink, mut output) = channel();

        assert_eq!(
            output.recv_timeout(Duration::ZERO),
            Err(RecvTimeoutError::Timeout)
        );
        assert_eq!(
            output.recv_timeout(Duration::from_millis(20)),
            Err(RecvTimeoutError::Timeout)
        );
    }

    #[test]
    fn buffered_bytes_are_delivered_before_the_end_of_the_stream() {
        let (sink, mut output) = channel();
        sink.push(b"last words");
        sink.finish();

        assert_eq!(output.recv().unwrap(), b"last words");
        assert_eq!(output.recv(), None);
        assert_eq!(output.recv_timeout(SOON), Err(RecvTimeoutError::Closed));
        assert_eq!(output.next(), None);
    }

    #[test]
    fn recv_wakes_up_when_bytes_arrive() {
        let (sink, mut output) = channel();
        let producer = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            sink.push(b"late");
            sink.finish();
        });

        assert_eq!(output.recv().unwrap(), b"late");
        assert_eq!(output.recv(), None);
        producer.join().unwrap();
    }

    #[test]
    fn a_huge_timeout_waits_instead_of_overflowing() {
        let (sink, mut output) = channel();
        sink.push(b"x");

        assert_eq!(output.recv_timeout(Duration::MAX).unwrap(), b"x");
    }

    #[test]
    fn push_blocks_while_the_buffer_is_full_and_resumes_when_it_is_emptied() {
        let (sink, mut output) = channel();
        let (progress_tx, progress_rx) = mpsc::channel();
        let block = vec![7u8; OUTPUT_BUFFER_CAPACITY];

        let producer = thread::spawn({
            let block = block.clone();
            move || {
                sink.push(&block);
                progress_tx.send("first").unwrap();
                sink.push(b"second");
                progress_tx.send("second").unwrap();
                sink.finish();
            }
        });

        assert_eq!(progress_rx.recv_timeout(SOON).unwrap(), "first");
        assert!(
            progress_rx
                .recv_timeout(Duration::from_millis(200))
                .is_err(),
            "the second push must wait for the consumer"
        );

        assert_eq!(output.recv_timeout(SOON).unwrap(), block);
        assert_eq!(progress_rx.recv_timeout(SOON).unwrap(), "second");
        assert_eq!(output.recv_timeout(SOON).unwrap(), b"second");
        assert_eq!(output.recv(), None);
        producer.join().unwrap();
    }

    /// Registers a callback counting its calls.
    fn count_calls(output: &mut PtyOutput) -> Arc<std::sync::atomic::AtomicUsize> {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        output.on_readable({
            let calls = Arc::clone(&calls);
            move || {
                calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        });
        calls
    }

    #[test]
    fn readable_callback_fires_when_the_buffer_stops_being_empty() {
        use std::sync::atomic::Ordering::SeqCst;

        let (sink, mut output) = channel();
        let calls = count_calls(&mut output);
        assert_eq!(calls.load(SeqCst), 0, "nothing to receive yet");

        sink.push(b"one");
        assert_eq!(calls.load(SeqCst), 1);
        // More output for a consumer that was already notified: no new call.
        sink.push(b"two");
        assert_eq!(calls.load(SeqCst), 1);

        assert_eq!(output.recv_timeout(Duration::ZERO).unwrap(), b"onetwo");
        assert_eq!(
            output.recv_timeout(Duration::ZERO),
            Err(RecvTimeoutError::Timeout)
        );

        // The buffer was emptied, so the next output notifies again.
        sink.push(b"three");
        assert_eq!(calls.load(SeqCst), 2);

        // The end of the stream notifies too.
        sink.finish();
        assert_eq!(calls.load(SeqCst), 3);
        assert_eq!(output.recv_timeout(Duration::ZERO).unwrap(), b"three");
        assert_eq!(
            output.recv_timeout(Duration::ZERO),
            Err(RecvTimeoutError::Closed)
        );
    }

    #[test]
    fn readable_callback_fires_immediately_when_output_is_waiting() {
        use std::sync::atomic::Ordering::SeqCst;

        let (sink, mut output) = channel();
        sink.push(b"early");
        assert_eq!(count_calls(&mut output).load(SeqCst), 1);

        let (sink, mut output) = channel();
        sink.finish();
        assert_eq!(count_calls(&mut output).load(SeqCst), 1);
    }

    #[test]
    fn panicking_callback_does_not_wedge_the_stream() {
        use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};

        let (sink, mut output) = channel();
        let calls = Arc::new(AtomicUsize::new(0));
        output.on_readable({
            let calls = Arc::clone(&calls);
            move || {
                calls.fetch_add(1, SeqCst);
                panic!("deliberate panic in a test callback");
            }
        });

        // The panic does not reach the producer (the reader thread)...
        sink.push(b"one");
        assert_eq!(calls.load(SeqCst), 1);
        assert_eq!(output.recv_timeout(Duration::ZERO).unwrap(), b"one");

        // ...the callback is not called again...
        sink.push(b"two");
        sink.finish();
        assert_eq!(calls.load(SeqCst), 1);
        assert_eq!(Arc::strong_count(&calls), 1, "the callback was kept");

        // ...and the stream still delivers everything and ends.
        assert_eq!(output.recv().unwrap(), b"two");
        assert_eq!(output.recv(), None);

        // A new callback can be registered afterwards.
        let (sink, mut output) = channel();
        output.on_readable(|| panic!("deliberate panic in a test callback"));
        sink.push(b"x");
        let calls = count_calls(&mut output);
        assert_eq!(calls.load(SeqCst), 1, "output is waiting");
    }

    #[test]
    fn callback_panicking_at_the_end_of_the_stream_is_contained() {
        let (sink, mut output) = channel();
        output.on_readable(|| panic!("deliberate panic in a test callback"));

        sink.finish();

        assert_eq!(output.recv(), None);
    }

    #[test]
    fn callback_is_not_called_once_shut_down() {
        use std::sync::atomic::Ordering::SeqCst;

        let (sink, mut output) = channel();
        let calls = count_calls(&mut output);

        // What dropping a `Pty` does: shut down, then the reader finishes.
        sink.shut_down();
        sink.push(b"discarded");
        sink.finish();

        assert_eq!(calls.load(SeqCst), 0);
        assert_eq!(output.recv(), None);
    }

    #[test]
    fn readable_callback_may_use_the_channel_without_deadlocking() {
        // The callback runs outside the buffer lock: a consumer may be
        // receiving on another thread while it runs.
        let (sink, mut output) = channel();
        let (wake_tx, wake_rx) = mpsc::channel();
        output.on_readable(move || {
            let _ = wake_tx.send(());
        });

        let consumer = thread::spawn(move || {
            let mut received = Vec::new();
            for () in wake_rx {
                loop {
                    match output.recv_timeout(Duration::ZERO) {
                        Ok(chunk) => received.extend_from_slice(&chunk),
                        Err(RecvTimeoutError::Timeout) => break,
                        Err(RecvTimeoutError::Closed) => return received,
                    }
                }
            }
            received
        });

        let mut expected = Vec::new();
        for index in 0..2_000u32 {
            let bytes = index.to_le_bytes();
            sink.push(&bytes);
            expected.extend_from_slice(&bytes);
        }
        sink.finish();

        assert_eq!(consumer.join().unwrap(), expected);
    }

    /// Fills the buffer, starts a push that must block, then runs `unblock`
    /// and checks that the push returns.
    fn assert_unblocks_a_full_push(unblock: impl FnOnce(&OutputSink, PtyOutput)) {
        let (sink, output) = channel();
        let (done_tx, done_rx) = mpsc::channel();
        sink.push(&vec![0u8; OUTPUT_BUFFER_CAPACITY]);

        let producer = thread::spawn({
            let sink = sink.clone();
            move || {
                sink.push(b"blocked");
                done_tx.send(()).unwrap();
            }
        });
        assert!(done_rx.recv_timeout(Duration::from_millis(200)).is_err());

        unblock(&sink, output);

        done_rx.recv_timeout(SOON).unwrap();
        producer.join().unwrap();
    }

    #[test]
    fn dropping_the_output_unblocks_the_producer() {
        assert_unblocks_a_full_push(|sink, output| {
            drop(output);
            // Nothing is buffered for a consumer that no longer exists.
            sink.push(b"discarded");
            assert!(lock(&sink.channel.state).buffer.is_empty());
        });
    }

    #[test]
    fn shutting_down_unblocks_the_producer_and_keeps_buffered_bytes() {
        assert_unblocks_a_full_push(|sink, mut output| {
            assert!(!sink.is_shut_down());
            sink.shut_down();
            assert!(sink.is_shut_down());

            sink.finish();
            assert_eq!(output.recv().unwrap().len(), OUTPUT_BUFFER_CAPACITY);
            assert_eq!(output.recv(), None);
        });
    }
}
