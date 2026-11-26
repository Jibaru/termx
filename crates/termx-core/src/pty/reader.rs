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
    use std::io::{ErrorKind, PipeReader, Read};
    use std::os::fd::AsRawFd;
    use std::time::Duration;

    use filedescriptor::{FileDescriptor, POLLIN, poll, pollfd};

    use super::{OutputSink, READ_BUFFER_SIZE, pump_blocking};

    /// Once the process has exited, how long the PTY may stay silent before
    /// the stream is ended even though something still holds the terminal
    /// open. See [`pump_until_stopped`].
    const DRAIN_IDLE_TIMEOUT: Duration = Duration::from_millis(100);

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
    ///   the normal case, or until the PTY has been silent for
    ///   [`DRAIN_IDLE_TIMEOUT`], so that everything the process wrote before
    ///   exiting is delivered.
    pub(in crate::pty) fn pump_until_stopped(
        mut master: FileDescriptor,
        stop: &PipeReader,
        sink: &OutputSink,
    ) {
        let mut buffer = vec![0u8; READ_BUFFER_SIZE];
        let mut stopping = false;
        loop {
            if sink.is_shut_down() {
                return;
            }

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
            // watched, with the idle timeout.
            let (watched, timeout) = if stopping {
                (&mut fds[..1], Some(DRAIN_IDLE_TIMEOUT))
            } else {
                (&mut fds[..], None)
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

            let was_stopping = stopping;
            stopping = stopping || fds[1].revents != 0;

            // Any event on the PTY (data, hang-up or error) is settled by
            // reading from it.
            if fds[0].revents != 0 {
                match master.read(&mut buffer) {
                    Ok(0) => return,
                    Ok(n) => sink.push(&buffer[..n]),
                    Err(error) if error.kind() == ErrorKind::Interrupted => {}
                    Err(_) => return,
                }
            } else if was_stopping {
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
