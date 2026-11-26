//! Coalesces the output of a PTY into batches for the frontend.
//!
//! A process rarely prints what it has to say in one piece: a redraw arrives
//! as a burst of small reads. Forwarding every read would cost one message to
//! the frontend each, so output is collected for a few milliseconds and sent
//! as one batch (the idea of `TerminalDataBufferer` in Visual Studio Code,
//! MIT licence).
//!
//! Batches end at a clean place of the stream whenever one can be found
//! without waiting long: see [`boundary`](super::boundary). Bytes are never
//! changed, reordered or dropped.

use std::time::{Duration, Instant};

use super::boundary::Scanner;

/// How long output is collected before it is sent to the frontend.
///
/// This is the latency added to everything a session prints, including the
/// echo of a key, so it has to stay far below what can be perceived: at 4 ms
/// it is a quarter of a frame on a 60 Hz display. It is also long enough to
/// collect a whole redraw of a full-screen program, which arrives within a
/// millisecond or two, into a single batch, and it caps a session at 250
/// messages per second.
pub const BATCH_WINDOW: Duration = Duration::from_millis(4);

/// Largest batch sent to the frontend, in bytes.
///
/// Output that reaches this size is sent without waiting for the window to
/// end. It bounds the size of one message, the time the terminal emulator
/// spends on one write, and how far flow control can overshoot its
/// high-water mark. It is also the lookahead for a clean place to end a
/// batch: a sequence or a synchronized update that does not end within one
/// batch is cut.
pub const BATCH_MAX_BYTES: usize = 64 * 1024;

/// Longest time output is held back to end a batch at a clean place,
/// counted from when its first byte arrived.
///
/// When the window ends in the middle of an escape sequence or of a
/// synchronized update, the batch is held until the rest arrives. A program
/// writes these in one go, so the rest is normally a read away. If it does
/// not come within this time, which is a frame and a half at 60 Hz, the
/// batch is sent as it is: no output is ever held indefinitely.
pub const BOUNDARY_HOLD_MAX: Duration = Duration::from_millis(25);

const _: () = assert!(BATCH_WINDOW.as_millis() < BOUNDARY_HOLD_MAX.as_millis());

/// Output waiting to be sent, and the rules deciding when and where to cut
/// it into batches.
///
/// Time is passed in by the caller, which makes the behaviour deterministic.
#[derive(Debug, Default)]
pub(crate) struct Batcher {
    pending: Vec<u8>,
    /// When the first of the pending bytes arrived, or when the bytes before
    /// them were sent.
    oldest: Option<Instant>,
    /// State of the stream at the start of `pending`.
    start: Scanner,
    /// The window has ended, but the pending bytes stop at an unclean place
    /// and are being held for the rest.
    holding: bool,
}

impl Batcher {
    /// Adds output received at `now`.
    pub(crate) fn push(&mut self, now: Instant, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        if self.pending.is_empty() {
            // Taking the buffer over avoids a copy in the usual case.
            self.pending = bytes;
            self.oldest = Some(now);
        } else {
            self.pending.extend_from_slice(&bytes);
        }
    }

    /// Number of bytes waiting to be sent.
    pub(crate) fn len(&self) -> usize {
        self.pending.len()
    }

    /// Whether nothing is waiting to be sent.
    pub(crate) fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Returns the next batch if one is due at `now`.
    ///
    /// `flush` means that no more output will come, so everything is due
    /// and there is nothing to wait for. Call this again until it returns
    /// `None`: there may be more than one batch.
    pub(crate) fn next_batch(&mut self, now: Instant, flush: bool) -> Option<Vec<u8>> {
        let oldest = self.oldest?;
        let full = self.pending.len() >= BATCH_MAX_BYTES;
        let age = now.saturating_duration_since(oldest);
        if !(flush || full || age >= BATCH_WINDOW) {
            return None;
        }

        let limit = self.pending.len().min(BATCH_MAX_BYTES);
        let mut scanner = self.start;
        let boundary = scanner.last_boundary(&self.pending[..limit]);
        let cut = match boundary {
            Some(cut) => {
                if cut < limit {
                    // The scanner went past the boundary: follow the stream
                    // again, up to the boundary only, for the state there.
                    scanner = self.start;
                    scanner.last_boundary(&self.pending[..cut]);
                }
                self.start = scanner;
                cut
            }
            None if flush || full || age >= BOUNDARY_HOLD_MAX => {
                scanner.abandon();
                self.start = scanner;
                limit
            }
            None => {
                self.holding = true;
                return None;
            }
        };

        self.holding = false;
        let batch = if cut == self.pending.len() {
            self.oldest = None;
            std::mem::take(&mut self.pending)
        } else {
            let rest = self.pending.split_off(cut);
            // The age of the remaining bytes is not known exactly. Counting
            // from now delays them by at most one more window.
            self.oldest = Some(now);
            std::mem::replace(&mut self.pending, rest)
        };
        Some(batch)
    }

    /// When [`next_batch`](Self::next_batch) should be called again if no
    /// output arrives in the meantime, or `None` if nothing is pending.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        let oldest = self.oldest?;
        Some(if self.pending.len() >= BATCH_MAX_BYTES {
            oldest
        } else if self.holding {
            oldest + BOUNDARY_HOLD_MAX
        } else {
            oldest + BATCH_WINDOW
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    /// A batcher and the instant its clock starts at.
    fn batcher() -> (Batcher, Instant) {
        (Batcher::default(), Instant::now())
    }

    /// Takes every batch due at `now`.
    fn drain(batcher: &mut Batcher, now: Instant, flush: bool) -> Vec<Vec<u8>> {
        std::iter::from_fn(|| batcher.next_batch(now, flush)).collect()
    }

    #[test]
    fn nothing_is_sent_before_the_window_ends() {
        let (mut batcher, start) = batcher();
        batcher.push(start, b"one".to_vec());

        assert_eq!(batcher.deadline(), Some(start + BATCH_WINDOW));
        assert_eq!(batcher.next_batch(start, false), None);
        assert_eq!(batcher.next_batch(start + BATCH_WINDOW - MS, false), None);
        assert_eq!(batcher.len(), 3);
    }

    #[test]
    fn output_arriving_within_the_window_is_sent_as_one_batch() {
        let (mut batcher, start) = batcher();
        batcher.push(start, b"one ".to_vec());
        batcher.push(start + MS, b"two ".to_vec());
        batcher.push(start + 3 * MS, b"three".to_vec());

        // The window is counted from the first byte, not the last.
        assert_eq!(
            drain(&mut batcher, start + BATCH_WINDOW, false),
            [b"one two three".to_vec()]
        );
        assert!(batcher.is_empty());
        assert_eq!(batcher.deadline(), None);
    }

    #[test]
    fn a_new_window_starts_with_the_next_output() {
        let (mut batcher, start) = batcher();
        batcher.push(start, b"first".to_vec());
        assert_eq!(drain(&mut batcher, start + BATCH_WINDOW, false).len(), 1);

        let later = start + 100 * MS;
        batcher.push(later, b"second".to_vec());

        assert_eq!(batcher.deadline(), Some(later + BATCH_WINDOW));
        assert_eq!(batcher.next_batch(later, false), None);
        assert_eq!(
            batcher.next_batch(later + BATCH_WINDOW, false).unwrap(),
            b"second"
        );
    }

    #[test]
    fn an_empty_push_does_not_start_a_window() {
        let (mut batcher, start) = batcher();
        batcher.push(start, Vec::new());

        assert!(batcher.is_empty());
        assert_eq!(batcher.deadline(), None);
        assert_eq!(batcher.next_batch(start + BATCH_WINDOW, true), None);
    }

    #[test]
    fn a_full_batch_is_sent_without_waiting_for_the_window() {
        let (mut batcher, start) = batcher();
        batcher.push(start, vec![b'x'; BATCH_MAX_BYTES]);

        assert_eq!(batcher.deadline(), Some(start), "due at once");
        let batch = batcher.next_batch(start, false).unwrap();
        assert_eq!(batch.len(), BATCH_MAX_BYTES);
        assert!(batcher.is_empty());
    }

    #[test]
    fn output_larger_than_a_batch_is_cut_into_full_batches() {
        let (mut batcher, start) = batcher();
        let output: Vec<u8> = (0..BATCH_MAX_BYTES * 2 + 100)
            .map(|index| b'a' + (index % 26) as u8)
            .collect();
        batcher.push(start, output.clone());

        // The two full batches are due at once; the rest waits its window.
        let full = drain(&mut batcher, start, false);
        assert_eq!(
            full.iter().map(Vec::len).collect::<Vec<_>>(),
            [BATCH_MAX_BYTES, BATCH_MAX_BYTES]
        );
        assert_eq!(batcher.len(), 100);
        assert_eq!(batcher.deadline(), Some(start + BATCH_WINDOW));
        let rest = drain(&mut batcher, start + BATCH_WINDOW, false);

        let sent: Vec<u8> = full.into_iter().chain(rest).flatten().collect();
        assert_eq!(sent, output, "bytes are neither changed nor reordered");
    }

    #[test]
    fn a_batch_ends_before_an_unfinished_escape_sequence() {
        let (mut batcher, start) = batcher();
        batcher.push(start, b"text\x1b[38;2;255".to_vec());

        let now = start + BATCH_WINDOW;
        assert_eq!(drain(&mut batcher, now, false), [b"text".to_vec()]);
        assert_eq!(batcher.len(), 10, "the start of the sequence is kept");

        // The rest arrives: the sequence is sent whole with what follows.
        batcher.push(now + MS, b";0;0mred".to_vec());
        assert_eq!(
            drain(&mut batcher, now + BATCH_WINDOW, false),
            [b"\x1b[38;2;255;0;0mred".to_vec()]
        );
    }

    #[test]
    fn a_batch_that_is_only_an_unfinished_sequence_is_held_then_sent_anyway() {
        let (mut batcher, start) = batcher();
        batcher.push(start, b"\x1b]0;a title that never ends".to_vec());

        assert_eq!(batcher.next_batch(start + BATCH_WINDOW, false), None);
        assert_eq!(batcher.deadline(), Some(start + BOUNDARY_HOLD_MAX));
        assert_eq!(
            batcher.next_batch(start + BOUNDARY_HOLD_MAX - MS, false),
            None
        );

        let batch = batcher.next_batch(start + BOUNDARY_HOLD_MAX, false);
        assert_eq!(batch.unwrap(), b"\x1b]0;a title that never ends");
        assert!(batcher.is_empty());
    }

    #[test]
    fn a_held_sequence_is_sent_as_soon_as_it_is_complete() {
        let (mut batcher, start) = batcher();
        batcher.push(start, b"\x1b[38;5".to_vec());
        let now = start + BATCH_WINDOW;
        assert_eq!(batcher.next_batch(now, false), None);

        batcher.push(now + MS, b";1m".to_vec());

        // No new window: these bytes have waited long enough.
        assert_eq!(
            batcher.next_batch(now + MS, false).unwrap(),
            b"\x1b[38;5;1m"
        );
    }

    #[test]
    fn a_synchronized_update_is_sent_in_one_batch() {
        let (mut batcher, start) = batcher();
        batcher.push(start, b"prompt\x1b[?2026h\x1b[2J\x1b[Hhalf a".to_vec());

        // The window ends in the middle of the update: only what precedes
        // it is sent.
        let now = start + BATCH_WINDOW;
        assert_eq!(drain(&mut batcher, now, false), [b"prompt".to_vec()]);

        batcher.push(now + MS, b" frame\x1b[?2026l".to_vec());
        assert_eq!(
            drain(&mut batcher, now + BATCH_WINDOW, false),
            [b"\x1b[?2026h\x1b[2J\x1b[Hhalf a frame\x1b[?2026l".to_vec()]
        );
    }

    #[test]
    fn a_synchronized_update_that_never_ends_stops_being_held() {
        let (mut batcher, start) = batcher();
        batcher.push(start, b"\x1b[?2026hpart one ".to_vec());
        assert_eq!(batcher.next_batch(start + BATCH_WINDOW, false), None);

        // The hold expires: the open update is sent...
        let expired = start + BOUNDARY_HOLD_MAX;
        assert_eq!(
            batcher.next_batch(expired, false).unwrap(),
            b"\x1b[?2026hpart one "
        );

        // ...and from then on its content flows with the usual window,
        // instead of being held again and again.
        batcher.push(expired, b"part two".to_vec());
        assert_eq!(batcher.next_batch(expired + BATCH_WINDOW - MS, false), None);
        assert_eq!(
            batcher.next_batch(expired + BATCH_WINDOW, false).unwrap(),
            b"part two"
        );
    }

    #[test]
    fn a_synchronized_update_larger_than_a_batch_is_cut() {
        let (mut batcher, start) = batcher();
        let mut output = b"\x1b[?2026h".to_vec();
        output.resize(BATCH_MAX_BYTES + 10, b'x');
        output.extend_from_slice(b"\x1b[?2026ltail");
        batcher.push(start, output.clone());

        let batches = drain(&mut batcher, start, false);
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].len(), BATCH_MAX_BYTES, "not held: it is full");

        let rest = drain(&mut batcher, start + BATCH_WINDOW, false);
        assert_eq!(rest.len(), 1, "the rest of the update is not held either");
        let sent: Vec<u8> = batches.into_iter().chain(rest).flatten().collect();
        assert_eq!(sent, output);
    }

    #[test]
    fn a_long_string_sequence_does_not_slow_the_stream_down() {
        // An inline image or a clipboard write can be far larger than a
        // batch. It must flow at full speed, not one hold at a time.
        let (mut batcher, start) = batcher();
        let mut output = b"\x1b]52;c;".to_vec();
        output.resize(BATCH_MAX_BYTES * 3, b'A');
        batcher.push(start, output);

        assert_eq!(drain(&mut batcher, start, false).len(), 3);
        assert!(batcher.is_empty());

        // Still inside the same sequence, in a later, small read.
        batcher.push(start + MS, b"QUJD".to_vec());
        assert_eq!(
            batcher
                .next_batch(start + MS + BATCH_WINDOW, false)
                .unwrap(),
            b"QUJD"
        );
    }

    #[test]
    fn a_multi_byte_character_is_not_cut_in_two() {
        let (mut batcher, start) = batcher();
        let check = "✓".as_bytes();
        batcher.push(start, [b"ok ", &check[..1]].concat());

        let now = start + BATCH_WINDOW;
        assert_eq!(drain(&mut batcher, now, false), [b"ok ".to_vec()]);

        batcher.push(now, check[1..].to_vec());
        assert_eq!(
            batcher.next_batch(now + BATCH_WINDOW, false).unwrap(),
            check
        );
    }

    #[test]
    fn flushing_sends_everything_wherever_it_ends() {
        let (mut batcher, start) = batcher();
        batcher.push(start, b"last words\x1b[".to_vec());

        // No window, and no hold for a sequence that will never be finished.
        assert_eq!(
            drain(&mut batcher, start, true),
            [b"last words".to_vec(), b"\x1b[".to_vec()]
        );
        assert!(batcher.is_empty());
        assert_eq!(batcher.deadline(), None);
    }

    #[test]
    fn every_split_of_a_stream_delivers_the_same_bytes() {
        let stream = b"$ ls\r\n\x1b[1;34mdir\x1b[0m \xe2\x9c\x93\r\n\x1b[?2026h\x1b[2Jframe\x1b[?2026l\x1b]0;t\x07$ ";

        for split in 0..=stream.len() {
            let (mut batcher, start) = batcher();
            let mut sent = Vec::new();
            let (head, tail) = stream.split_at(split);

            batcher.push(start, head.to_vec());
            sent.extend(drain(&mut batcher, start + BATCH_WINDOW, false));
            batcher.push(start + BATCH_WINDOW, tail.to_vec());
            sent.extend(drain(&mut batcher, start + BATCH_WINDOW * 2, false));
            sent.extend(drain(&mut batcher, start + BATCH_WINDOW * 2, true));

            assert_eq!(sent.concat(), stream, "split at {split}");
            // Whatever the split, no batch ends inside the synchronized
            // update or inside a sequence.
            for batch in &sent {
                assert_eq!(
                    Scanner::default().last_boundary(batch),
                    Some(batch.len()),
                    "split at {split}: {:?}",
                    String::from_utf8_lossy(batch)
                );
            }
        }
    }
}
