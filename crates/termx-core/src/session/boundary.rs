//! Finds the places where a stream of terminal output can be cut without
//! separating bytes that belong together.
//!
//! The terminal emulator copes with a cut anywhere: its parser keeps its
//! state between two writes. Cutting at a boundary is still worth it, because
//! the emulator may paint the screen between two writes. A batch that stops
//! halfway through a redraw shows a torn frame, which is exactly what a
//! program asks to avoid when it brackets its redraw in a synchronized update
//! (DEC private mode 2026).
//!
//! [`Scanner`] is a small state machine that follows the output just closely
//! enough to know where sequences start and end. It never changes a byte, and
//! it does not validate anything: unknown or malformed input simply returns
//! it to the ground state.

/// Longest parameter string of a control sequence that is inspected to
/// recognise the synchronized update mode. Longer ones are still followed to
/// their end, only not interpreted.
const MAX_INSPECTED_PARAMETERS: usize = 32;

/// The DEC private mode that brackets a synchronized update.
const SYNCHRONIZED_UPDATE_MODE: &[u8] = b"2026";

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;
/// CAN and SUB abort the sequence in progress.
const CAN: u8 = 0x18;
const SUB: u8 = 0x1a;

/// Where the scanner is in the grammar of escape sequences.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum State {
    /// Plain text: no sequence in progress.
    #[default]
    Ground,
    /// Inside a multi-byte UTF-8 character, with this many bytes to come.
    Utf8(u8),
    /// After ESC, and after its intermediate bytes if any.
    Escape,
    /// Inside a control sequence (`ESC [`), before its final byte.
    Csi,
    /// Inside a string sequence (OSC, DCS, APC, PM or SOS).
    String {
        /// Whether BEL ends it, which only holds for an OSC.
        bel_terminates: bool,
    },
    /// After an ESC inside a string sequence: a `\` ends the string.
    StringEscape,
}

/// Follows a stream of terminal output and reports where it can be cut.
///
/// The value is small and `Copy`, so a caller can keep the state at the start
/// of its buffer and scan the buffer again from there.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Scanner {
    state: State,
    /// A synchronized update was started and has not ended.
    in_synchronized_update: bool,
    /// The sequence in progress was already cut in two by a forced cut, so
    /// there is nothing left to protect until it ends.
    sequence_abandoned: bool,
    /// The parameter bytes of the control sequence in progress.
    parameters: [u8; MAX_INSPECTED_PARAMETERS],
    parameters_len: usize,
    /// The control sequence in progress has more parameter bytes than are
    /// kept, so it is not interpreted.
    parameters_overflowed: bool,
}

impl Scanner {
    /// Advances over `bytes` and returns the largest `n` in `1..=bytes.len()`
    /// such that cutting the stream after the first `n` bytes is clean:
    /// between two characters, outside any escape sequence and outside any
    /// synchronized update. Returns `None` when there is no such place.
    pub(crate) fn last_boundary(&mut self, bytes: &[u8]) -> Option<usize> {
        let mut boundary = None;
        for (index, &byte) in bytes.iter().enumerate() {
            self.advance(byte);
            if self.at_boundary() {
                boundary = Some(index + 1);
            }
        }
        boundary
    }

    /// Records that the stream was cut right here although this is not a
    /// boundary. What was being protected is already broken, so holding more
    /// output back for it would only add delay: the rest of the sequence in
    /// progress, and of the synchronized update in progress, may be cut
    /// anywhere.
    pub(crate) fn abandon(&mut self) {
        self.in_synchronized_update = false;
        if self.state != State::Ground {
            self.sequence_abandoned = true;
        }
    }

    /// Whether the stream can be cut cleanly at the current position.
    fn at_boundary(&self) -> bool {
        !self.in_synchronized_update && (self.state == State::Ground || self.sequence_abandoned)
    }

    fn enter(&mut self, state: State) {
        self.state = state;
        if state == State::Ground {
            self.sequence_abandoned = false;
        }
    }

    fn advance(&mut self, byte: u8) {
        match self.state {
            State::Ground => self.ground(byte),
            State::Utf8(remaining) => {
                if (0x80..=0xbf).contains(&byte) {
                    self.enter(match remaining {
                        0 | 1 => State::Ground,
                        more => State::Utf8(more - 1),
                    });
                } else {
                    // Not a continuation byte: the character was malformed,
                    // and this byte starts whatever comes next.
                    self.enter(State::Ground);
                    self.ground(byte);
                }
            }
            State::Escape => match byte {
                b'[' => {
                    self.parameters_len = 0;
                    self.parameters_overflowed = false;
                    self.enter(State::Csi);
                }
                b']' => self.enter(State::String {
                    bel_terminates: true,
                }),
                b'P' | b'X' | b'^' | b'_' => self.enter(State::String {
                    bel_terminates: false,
                }),
                // Intermediate bytes: the final byte is still to come.
                0x20..=0x2f => {}
                ESC => {}
                _ => self.enter(State::Ground),
            },
            State::Csi => match byte {
                // Parameter and intermediate bytes.
                0x20..=0x3f => {
                    if self.parameters_len < MAX_INSPECTED_PARAMETERS {
                        self.parameters[self.parameters_len] = byte;
                        self.parameters_len += 1;
                    } else {
                        self.parameters_overflowed = true;
                    }
                }
                // The final byte.
                0x40..=0x7e => {
                    self.finish_control_sequence(byte);
                    self.enter(State::Ground);
                }
                ESC => self.enter(State::Escape),
                CAN | SUB => self.enter(State::Ground),
                // Other control characters act without ending the sequence.
                _ => {}
            },
            State::String { bel_terminates } => match byte {
                ESC => self.state = State::StringEscape,
                BEL if bel_terminates => self.enter(State::Ground),
                CAN | SUB => self.enter(State::Ground),
                _ => {}
            },
            State::StringEscape => {
                if byte == b'\\' {
                    self.enter(State::Ground);
                } else {
                    // The ESC ended the string and starts a new sequence,
                    // of which this byte is the second.
                    self.state = State::Escape;
                    self.advance(byte);
                }
            }
        }
    }

    fn ground(&mut self, byte: u8) {
        match byte {
            ESC => self.state = State::Escape,
            0xc2..=0xdf => self.state = State::Utf8(1),
            0xe0..=0xef => self.state = State::Utf8(2),
            0xf0..=0xf4 => self.state = State::Utf8(3),
            _ => {}
        }
    }

    /// Notes the start or the end of a synchronized update: `CSI ? 2026 h`
    /// and `CSI ? 2026 l`, possibly setting other modes in the same sequence.
    fn finish_control_sequence(&mut self, final_byte: u8) {
        let enable = match final_byte {
            b'h' => true,
            b'l' => false,
            _ => return,
        };
        if self.parameters_overflowed {
            return;
        }
        let Some(modes) = self.parameters[..self.parameters_len].strip_prefix(b"?") else {
            return;
        };
        if modes
            .split(|&byte| byte == b';')
            .any(|mode| mode == SYNCHRONIZED_UPDATE_MODE)
        {
            self.in_synchronized_update = enable;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The last boundary of `bytes` for a scanner starting in the ground
    /// state.
    fn boundary(bytes: &[u8]) -> Option<usize> {
        Scanner::default().last_boundary(bytes)
    }

    #[test]
    fn plain_text_can_be_cut_anywhere() {
        assert_eq!(boundary(b"hello"), Some(5));
        assert_eq!(boundary(b"line one\r\nline two\r\n"), Some(20));
        assert_eq!(boundary(b""), None);
    }

    #[test]
    fn a_complete_sequence_is_not_a_reason_to_hold_anything() {
        assert_eq!(boundary(b"\x1b[31mred\x1b[0m"), Some(12));
        assert_eq!(boundary(b"\x1b]0;title\x07"), Some(10));
        assert_eq!(boundary(b"\x1b]8;;https://example.com\x1b\\link"), Some(30));
        assert_eq!(boundary(b"\x1bPq#0;2;0;0;0\x1b\\"), Some(15));
        assert_eq!(boundary(b"\x1b(B"), Some(3));
        assert_eq!(boundary(b"\x1b7\x1b8\x1bM"), Some(6));
    }

    #[test]
    fn an_unfinished_sequence_is_cut_before_its_escape() {
        for (bytes, expected) in [
            (&b"text\x1b"[..], Some(4)),
            (b"text\x1b[", Some(4)),
            (b"text\x1b[38;2;10;20", Some(4)),
            (b"text\x1b]0;half a title", Some(4)),
            (b"text\x1b]0;title\x1b", Some(4)),
            (b"text\x1bP1$r", Some(4)),
            (b"text\x1b(", Some(4)),
            (b"\x1b[1", None),
        ] {
            assert_eq!(boundary(bytes), expected, "{bytes:?}");
        }
    }

    #[test]
    fn a_sequence_split_over_two_scans_is_followed_across_them() {
        let mut scanner = Scanner::default();

        assert_eq!(scanner.last_boundary(b"ab\x1b[38;5"), Some(2));
        // The same scanner continues where it stopped: the first three
        // bytes finish the sequence.
        assert_eq!(scanner.last_boundary(b";1mcd"), Some(5));
    }

    #[test]
    fn every_split_of_a_sequence_gives_the_same_boundaries() {
        let stream = b"a\x1b[1;31mb\x1b]0;t\x07c\x1b[?2026hd\x1b[2Je\x1b[?2026lf\xe2\x9c\x93g";
        let mut whole = Scanner::default();
        let expected: Vec<bool> = stream
            .iter()
            .map(|&byte| {
                whole.advance(byte);
                whole.at_boundary()
            })
            .collect();

        for split in 0..=stream.len() {
            let mut scanner = Scanner::default();
            let (head, tail) = stream.split_at(split);
            let head_boundary = scanner.last_boundary(head);
            let tail_boundary = scanner.last_boundary(tail);

            let last = |range: std::ops::Range<usize>| {
                range
                    .clone()
                    .rev()
                    .find(|&index| expected[index])
                    .map(|index| index + 1 - range.start)
            };
            assert_eq!(head_boundary, last(0..split), "head, split at {split}");
            assert_eq!(
                tail_boundary,
                last(split..stream.len()),
                "tail, split at {split}"
            );
        }
    }

    #[test]
    fn a_multi_byte_character_is_not_cut_in_two() {
        // "é" is two bytes, "✓" three, "😀" four.
        assert_eq!(boundary("é".as_bytes()), Some(2));
        assert_eq!(boundary(&"aé".as_bytes()[..2]), Some(1));
        assert_eq!(boundary(&"a✓".as_bytes()[..3]), Some(1));
        assert_eq!(boundary(&"a😀".as_bytes()[..4]), Some(1));
        assert_eq!(boundary("a😀".as_bytes()), Some(5));
    }

    #[test]
    fn malformed_utf8_does_not_hold_anything_back() {
        // A lead byte followed by something that is not a continuation.
        assert_eq!(boundary(b"\xe2ab"), Some(3));
        // A lone continuation byte and an invalid byte.
        assert_eq!(boundary(b"\x80\xff"), Some(2));
        // A lead byte followed by an escape sequence.
        assert_eq!(boundary(b"\xf0\x1b[0m"), Some(5));
    }

    #[test]
    fn a_synchronized_update_is_kept_whole() {
        let begin = b"\x1b[?2026h";
        let end = b"\x1b[?2026l";
        let mut frame = Vec::new();
        frame.extend_from_slice(b"before");
        frame.extend_from_slice(begin);
        frame.extend_from_slice(b"\x1b[2J\x1b[Hframe");

        // Open: the only clean cut is before the update started.
        assert_eq!(boundary(&frame), Some(6));

        frame.extend_from_slice(end);
        assert_eq!(boundary(&frame), Some(frame.len()));

        frame.extend_from_slice(b"after");
        assert_eq!(boundary(&frame), Some(frame.len()));
    }

    #[test]
    fn the_synchronized_update_mode_is_found_among_other_modes() {
        assert_eq!(boundary(b"a\x1b[?25;2026hframe"), Some(1));
        assert_eq!(boundary(b"a\x1b[?2026;25hframe\x1b[?25;2026l"), Some(28));
        // Other modes, and the same number as an ANSI mode, are not it.
        assert_eq!(boundary(b"a\x1b[?2027hb"), Some(10));
        assert_eq!(boundary(b"a\x1b[?202hb"), Some(9));
        assert_eq!(boundary(b"a\x1b[2026hb"), Some(9));
    }

    #[test]
    fn a_control_sequence_with_too_many_parameters_is_followed_but_not_read() {
        let mut bytes = b"a\x1b[?".to_vec();
        bytes.extend(std::iter::repeat_n(b'1', MAX_INSPECTED_PARAMETERS * 2));
        bytes.extend_from_slice(b";2026h");
        let end_of_sequence = bytes.len();
        bytes.extend_from_slice(b"tail");

        // Not held as a synchronized update, and not lost track of either.
        assert_eq!(boundary(&bytes[..end_of_sequence - 1]), Some(1));
        assert_eq!(boundary(&bytes), Some(bytes.len()));
    }

    #[test]
    fn cancel_and_substitute_abort_a_sequence() {
        assert_eq!(boundary(b"\x1b[12\x18text"), Some(9));
        assert_eq!(boundary(b"\x1b]0;title\x1atext"), Some(14));
    }

    #[test]
    fn an_escape_inside_a_string_starts_a_new_sequence() {
        // The OSC is never terminated; the CSI that interrupts it is.
        assert_eq!(boundary(b"\x1b]0;title\x1b[0mtext"), Some(17));
    }

    #[test]
    fn bel_only_ends_an_operating_system_command() {
        assert_eq!(boundary(b"\x1b]0;t\x07x"), Some(7));
        assert_eq!(boundary(b"a\x1bPdata\x07x"), Some(1));
    }

    #[test]
    fn after_a_forced_cut_the_rest_of_the_sequence_can_be_cut_anywhere() {
        let mut scanner = Scanner::default();
        assert_eq!(scanner.last_boundary(b"\x1b]52;c;QUJD"), None);

        scanner.abandon();

        assert_eq!(scanner.last_boundary(b"REVG"), Some(4));
        // Once the string ends, sequences are protected again.
        assert_eq!(scanner.last_boundary(b"\x07ok\x1b[3"), Some(3));
    }

    #[test]
    fn after_a_forced_cut_the_rest_of_the_synchronized_update_can_be_cut_anywhere() {
        let mut scanner = Scanner::default();
        assert_eq!(scanner.last_boundary(b"\x1b[?2026hpart one"), None);

        scanner.abandon();

        assert_eq!(scanner.last_boundary(b"part two"), Some(8));
        // The end of the abandoned update is harmless, and the next update
        // is protected again.
        assert_eq!(
            scanner.last_boundary(b"\x1b[?2026lx\x1b[?2026hnext"),
            Some(9)
        );
    }
}
