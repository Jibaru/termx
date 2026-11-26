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
#[cfg(any(unix, test))]
pub(super) fn pump_blocking(reader: impl Read, sink: &OutputSink) {
    pump_blocking_with(reader, |bytes| sink.push(bytes));
}

/// Like [`pump_blocking`], handing each read to `forward`.
fn pump_blocking_with(mut reader: impl Read, mut forward: impl FnMut(&[u8])) {
    let mut buffer = vec![0u8; READ_BUFFER_SIZE];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => forward(&buffer[..n]),
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
}

/// Reads until end of file like [`pump_blocking`], taking care of the cursor
/// position query the Windows console sends when it starts.
///
/// `answer` is called once, when the query is seen, and must write
/// [`CURSOR_POSITION_REPORT`] to the PTY. See [`StartupQueryFilter`].
#[cfg(windows)]
pub(super) fn pump_console(reader: impl Read, sink: &OutputSink, mut answer: impl FnMut()) {
    let mut filter = StartupQueryFilter::default();
    pump_blocking_with(reader, |bytes| {
        let (output, query_seen) = filter.filter(bytes);
        if query_seen {
            answer();
        }
        if !output.is_empty() {
            sink.push(&output);
        }
    });
    let rest = filter.finish();
    if !rest.is_empty() {
        sink.push(&rest);
    }
}

/// The query the Windows console sends when it starts: "where is the
/// cursor?" (DSR 6).
#[cfg(any(windows, test))]
const CURSOR_POSITION_QUERY: &[u8] = b"\x1b[6n";

/// The answer to [`CURSOR_POSITION_QUERY`] for a terminal whose cursor is in
/// the top-left corner, which is where a new terminal starts.
#[cfg(any(windows, test))]
pub(super) const CURSOR_POSITION_REPORT: &[u8] = b"\x1b[1;1R";

/// How far into the output the start-up query is looked for. The console
/// sends it before anything the process prints, after at most a few mode
/// settings.
#[cfg(any(windows, test))]
const STARTUP_SCAN_LIMIT: usize = 256;

/// Removes the start-up cursor position query from the output of a Windows
/// pseudo-console, so that this module can answer it itself.
///
/// portable-pty creates the pseudo-console with `PSEUDOCONSOLE_INHERIT_CURSOR`.
/// The console then asks its terminal for the cursor position before doing
/// anything else and, on Windows 10, produces no output at all until it gets
/// the answer. A session nobody is looking at yet, or a consumer that is not
/// a terminal emulator, would stall forever.
///
/// The query is therefore answered here, immediately and unconditionally,
/// and removed from the stream. Removing it is what guarantees that it is
/// never answered twice: a terminal emulator attached to the output never
/// sees the question. Answering "top-left corner" is correct because the
/// question is asked before any output, when a new terminal's cursor has not
/// moved yet.
///
/// Only the first query within the first [`STARTUP_SCAN_LIMIT`] bytes is
/// treated this way. Everything else, including later queries made by the
/// programs running in the terminal, is passed through untouched for the
/// terminal emulator to answer.
#[cfg(any(windows, test))]
#[derive(Default)]
struct StartupQueryFilter {
    /// Bytes delivered so far while looking for the query.
    scanned: usize,
    /// Trailing bytes that may be the beginning of the query, held back
    /// until the next read tells.
    held: Vec<u8>,
    done: bool,
}

#[cfg(any(windows, test))]
impl StartupQueryFilter {
    /// Returns the bytes to deliver for one read, and whether the query was
    /// found in it.
    fn filter<'a>(&mut self, bytes: &'a [u8]) -> (std::borrow::Cow<'a, [u8]>, bool) {
        use std::borrow::Cow;

        if self.done {
            return (Cow::Borrowed(bytes), false);
        }

        let mut data = std::mem::take(&mut self.held);
        data.extend_from_slice(bytes);

        // Only a query starting inside the start-up window is the console's.
        let window_left = STARTUP_SCAN_LIMIT - self.scanned;
        if let Some(position) = data
            .windows(CURSOR_POSITION_QUERY.len())
            .position(|window| window == CURSOR_POSITION_QUERY)
            .filter(|&position| position < window_left)
        {
            data.drain(position..position + CURSOR_POSITION_QUERY.len());
            self.done = true;
            return (Cow::Owned(data), true);
        }

        // Keep back a tail that could be the start of a query cut in two.
        let tail = (1..CURSOR_POSITION_QUERY.len())
            .rev()
            .find(|&length| data.ends_with(&CURSOR_POSITION_QUERY[..length]))
            .unwrap_or(0);
        let scanned = self.scanned + data.len() - tail;
        if scanned >= STARTUP_SCAN_LIMIT {
            // Past the start-up window: nothing is held back any more.
            self.done = true;
            return (Cow::Owned(data), false);
        }
        self.scanned = scanned;
        self.held = data.split_off(data.len() - tail);
        (Cow::Owned(data), false)
    }

    /// Returns the bytes still held back, at the end of the stream.
    fn finish(&mut self) -> Vec<u8> {
        self.done = true;
        std::mem::take(&mut self.held)
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

    /// Feeds `chunks` to a new filter and returns everything delivered and
    /// how many times the query was reported.
    fn run_filter(chunks: &[&[u8]]) -> (Vec<u8>, usize) {
        let mut filter = StartupQueryFilter::default();
        let mut delivered = Vec::new();
        let mut queries = 0;
        for chunk in chunks {
            let (output, query_seen) = filter.filter(chunk);
            delivered.extend_from_slice(&output);
            queries += usize::from(query_seen);
        }
        delivered.extend_from_slice(&filter.finish());
        (delivered, queries)
    }

    #[test]
    fn startup_query_is_removed_and_reported_once() {
        let (delivered, queries) = run_filter(&[b"\x1b[?9001h\x1b[6n\x1b[?1004hhello"]);

        assert_eq!(delivered, b"\x1b[?9001h\x1b[?1004hhello");
        assert_eq!(queries, 1);
    }

    #[test]
    fn startup_query_is_found_wherever_the_reads_cut_it() {
        let stream = b"\x1b[?25l\x1b[6n\x1b[2Jprompt> ";
        let expected = b"\x1b[?25l\x1b[2Jprompt> ";

        // Every way of cutting the stream in two, then byte by byte.
        for cut in 0..=stream.len() {
            let (delivered, queries) = run_filter(&[&stream[..cut], &stream[cut..]]);
            assert_eq!(delivered, expected, "cut at {cut}");
            assert_eq!(queries, 1, "cut at {cut}");
        }
        let bytes: Vec<&[u8]> = stream.chunks(1).collect();
        let (delivered, queries) = run_filter(&bytes);
        assert_eq!(delivered, expected);
        assert_eq!(queries, 1);
    }

    #[test]
    fn only_the_first_query_belongs_to_the_console() {
        // A later query comes from a program and is for the terminal
        // emulator to answer: it must be delivered.
        let (delivered, queries) = run_filter(&[b"\x1b[6n", b"$ \x1b[6n", b"\x1b[6n"]);

        assert_eq!(delivered, b"$ \x1b[6n\x1b[6n");
        assert_eq!(queries, 1);
    }

    #[test]
    fn output_without_a_startup_query_is_untouched() {
        // Sequences that merely start like the query, with awkward cuts.
        let chunks: [&[u8]; 6] = [
            b"\x1b",
            b"[6",
            b"m colour \x1b[",
            b"60n \x1b",
            b"[",
            b"0m end\x1b[6",
        ];
        let (delivered, queries) = run_filter(&chunks);

        assert_eq!(delivered, chunks.concat());
        assert_eq!(queries, 0);
    }

    #[test]
    fn queries_after_the_startup_window_are_untouched() {
        let mut stream = vec![b'x'; STARTUP_SCAN_LIMIT];
        stream.extend_from_slice(CURSOR_POSITION_QUERY);
        stream.extend_from_slice(b" tail");

        for size in [1, 3, 7, 64, stream.len()] {
            let chunks: Vec<&[u8]> = stream.chunks(size).collect();
            let (delivered, queries) = run_filter(&chunks);
            assert_eq!(delivered, stream, "chunks of {size}");
            assert_eq!(queries, 0, "chunks of {size}");
        }
    }

    #[test]
    fn filter_is_transparent_once_done() {
        let mut filter = StartupQueryFilter::default();
        assert!(filter.filter(b"\x1b[6n").1);

        // No copy and no holding back afterwards, even for a lone escape.
        let (output, query_seen) = filter.filter(b"text\x1b");
        assert!(matches!(output, std::borrow::Cow::Borrowed(b"text\x1b")));
        assert!(!query_seen);
        assert!(filter.finish().is_empty());
    }

    #[test]
    fn cursor_position_report_is_well_formed() {
        assert_eq!(CURSOR_POSITION_REPORT, b"\x1b[1;1R");
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
