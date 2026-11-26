//! The loops run by the reader thread, moving bytes from the PTY to the
//! output buffer.

use std::io::{ErrorKind, Read};

use super::output::OutputSink;

/// Size of the buffer used for each read from the PTY, and therefore the
/// largest amount of output moved to the output buffer in one step.
pub const READ_BUFFER_SIZE: usize = 64 * 1024;

/// Reads until end of file, with plain blocking reads.
///
/// Any read error other than an interruption ends the stream: a PTY reports
/// the terminal going away as an error (`EIO` on Linux, a broken pipe on
/// Windows) rather than as a zero-length read.
pub(super) fn pump_blocking(mut reader: impl Read, sink: &OutputSink) {
    let mut buffer = vec![0u8; READ_BUFFER_SIZE];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => sink.push(&buffer[..n]),
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
}

#[cfg(unix)]
pub(super) use unix::pump_until_stopped;

#[cfg(unix)]
mod unix {
    use std::fs::File;
    use std::io::{ErrorKind, PipeReader, Read};
    use std::os::fd::AsRawFd;
    use std::time::{Duration, Instant};

    use filedescriptor::{POLLIN, poll, pollfd};

    use super::{OutputSink, READ_BUFFER_SIZE, pump_blocking};
    use crate::pty::OUTPUT_BUFFER_CAPACITY;

    /// Once the process has exited, how long the PTY may stay silent before
    /// the stream is ended even though something still holds the terminal
    /// open. See [`pump_until_stopped`].
    const DRAIN_IDLE_TIMEOUT: Duration = Duration::from_millis(100);

    /// Once the process has exited, the longest the stream is kept open for
    /// something that keeps printing, not counting the time spent waiting
    /// for the consumer.
    const DRAIN_TIME_LIMIT: Duration = Duration::from_secs(1);

    /// Once the process has exited, the most output still delivered. What
    /// the process itself wrote before exiting is at most what the kernel
    /// buffers for a terminal, which is well below this on every system.
    const DRAIN_BYTE_LIMIT: usize = OUTPUT_BUFFER_CAPACITY;

    /// Reads from the PTY `master` until end of file or until told to stop
    /// through the `stop` pipe.
    ///
    /// End of file is only reported once *every* process holding the terminal
    /// open is gone. A background job left behind by a shell keeps it open
    /// indefinitely, so a plain blocking read could never be interrupted.
    /// Instead this waits on the PTY and on a pipe whose write end is closed
    /// when the process exits or the [`Pty`](crate::pty::Pty) is dropped:
    ///
    /// - on drop (the sink is shut down), it returns immediately;
    /// - on exit, it keeps reading until end of file, which is immediate in
    ///   the normal case, so that everything the process wrote before
    ///   exiting is delivered. When something else keeps the terminal open,
    ///   it stops once the PTY has been silent for [`DRAIN_IDLE_TIMEOUT`].
    ///   If that something never stops printing, it stops after
    ///   [`DRAIN_TIME_LIMIT`] or [`DRAIN_BYTE_LIMIT`], whichever comes first:
    ///   the end of the stream always follows the exit within a bounded
    ///   time.
    pub(in crate::pty) fn pump_until_stopped(
        mut master: &File,
        stop: &PipeReader,
        sink: &OutputSink,
    ) {
        let mut buffer = vec![0u8; READ_BUFFER_SIZE];
        // Set when told to stop: the deadline and the bytes left to deliver.
        let mut drain: Option<(Instant, usize)> = None;
        loop {
            if sink.is_shut_down() {
                return;
            }
            let timeout = match drain {
                None => None,
                Some((deadline, _)) => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return;
                    }
                    Some(remaining.min(DRAIN_IDLE_TIMEOUT))
                }
            };

            let mut fds = [
                pollfd {
                    fd: master.as_raw_fd(),
                    events: POLLIN,
                    revents: 0,
                },
                pollfd {
                    fd: stop.as_raw_fd(),
                    events: POLLIN,
                    revents: 0,
                },
            ];
            // Once stopping, the closed pipe is always ready: only the PTY is
            // watched, with a timeout.
            let watched = if drain.is_some() {
                &mut fds[..1]
            } else {
                &mut fds[..]
            };
            match poll(watched, timeout) {
                Ok(_) => {}
                Err(error) if is_interrupted(&error) => continue,
                // Waiting is not possible (for example the descriptor number
                // is too large for the `select`-based implementation used on
                // macOS). Degrade to blocking reads, which still end at end
                // of file.
                Err(_) => return pump_blocking(master, sink),
            }

            let was_draining = drain.is_some();
            if !was_draining && fds[1].revents != 0 {
                drain = Some((Instant::now() + DRAIN_TIME_LIMIT, DRAIN_BYTE_LIMIT));
            }

            // Any event on the PTY (data, hang-up or error) is settled by
            // reading from it.
            if fds[0].revents != 0 {
                let n = match master.read(&mut buffer) {
                    Ok(0) => return,
                    Ok(n) => n,
                    Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                    Err(_) => return,
                };
                let pushing = Instant::now();
                sink.push(&buffer[..n]);
                if let Some((deadline, bytes_left)) = &mut drain {
                    // A slow consumer must not eat into the time limit, or
                    // the last output of the process could be cut off.
                    *deadline += pushing.elapsed();
                    *bytes_left = bytes_left.saturating_sub(n);
                    if *bytes_left == 0 {
                        return;
                    }
                }
            } else if was_draining {
                // The idle timeout elapsed without any output.
                return;
            }
        }
    }

    fn is_interrupted(error: &filedescriptor::Error) -> bool {
        match error {
            filedescriptor::Error::Poll(error) | filedescriptor::Error::Io(error) => {
                error.kind() == ErrorKind::Interrupted
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pty::output::channel;
    use std::io;

    /// Hands out its data in tiny reads, with an interruption and finally an
    /// error, like a PTY whose terminal went away.
    struct Choppy {
        data: Vec<u8>,
        position: usize,
        interrupted: bool,
    }

    impl Read for Choppy {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if !self.interrupted {
                self.interrupted = true;
                return Err(ErrorKind::Interrupted.into());
            }
            if self.position == self.data.len() {
                return Err(io::Error::other("the terminal went away"));
            }
            let n = 3.min(self.data.len() - self.position).min(buffer.len());
            buffer[..n].copy_from_slice(&self.data[self.position..self.position + n]);
            self.position += n;
            Ok(n)
        }
    }

    #[test]
    fn blocking_pump_forwards_every_byte_until_the_read_fails() {
        let text = "multi-byte: \u{e9}\u{65e5}\u{672c}\u{1f680} \x1b[31mred\x1b[0m";
        let (sink, mut output) = channel();

        pump_blocking(
            Choppy {
                data: text.as_bytes().to_vec(),
                position: 0,
                interrupted: false,
            },
            &sink,
        );
        sink.finish();

        // Reads of 3 bytes split the multi-byte characters; reassembling the
        // chunks must give back the exact bytes.
        let received: Vec<u8> = output.by_ref().flatten().collect();
        assert_eq!(String::from_utf8(received).unwrap(), text);
    }

    #[test]
    fn blocking_pump_stops_at_end_of_file() {
        let (sink, mut output) = channel();

        pump_blocking(&b"abc"[..], &sink);
        sink.finish();

        assert_eq!(output.recv().unwrap(), b"abc");
        assert_eq!(output.recv(), None);
    }
}
