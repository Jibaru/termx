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
        state.buffer.extend_from_slice(bytes);
        drop(state);
        self.channel.readable.notify_one();
    }

    /// Marks the end of the stream. Buffered bytes can still be received.
    pub(super) fn finish(&self) {
        lock(&self.channel.state).finished = true;
        self.channel.readable.notify_all();
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
        drop(state);
        self.channel.writable.notify_all();
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
