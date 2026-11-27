//! The recent output of a terminal, kept to repaint a frontend that
//! attaches to it later: a window that reloaded its page while the
//! terminal kept running.
//!
//! [`ReplayBuffer`] keeps the last [`REPLAY_BUFFER_BYTES`] of output as
//! raw bytes, and follows the terminal modes a program sets once and
//! relies on afterwards (bracketed paste, application cursor keys, mouse
//! and focus reporting, the alternate screen, a hidden cursor), including
//! their full and soft resets. A new emulator fed
//! [`ReplayBuffer::replay`] shows the end of the scrollback and behaves as
//! the program expects. A full-screen program is then told to redraw by a
//! change of size (see the desktop crate), which paints whatever the
//! buffer no longer holds.
//!
//! **Never log the bytes**: they are the content of the user's terminal.

use std::collections::VecDeque;

/// How much recent output a terminal keeps for a frontend that attaches
/// later. A screen of Claude Code with its scrollback above is well under
/// this; a program that floods the terminal only keeps its last part.
pub const REPLAY_BUFFER_BYTES: usize = 256 * 1024;

/// How far into the kept bytes a cut is moved to start at a line, when the
/// oldest output had to be dropped.
const LINE_SEARCH_BYTES: usize = 4 * 1024;

const ESC: u8 = 0x1b;

/// The DEC private modes that are followed, with whether each is on when a
/// terminal starts, in the order they are restored:
///
/// - the alternate screen (1049, 1047, 47) first, because switching to it
///   clears what is shown;
/// - application cursor keys (1), the cursor shown (25), mouse reporting
///   (1000, 1002, 1003 and the SGR encoding 1006) and focus reporting
///   (1004);
/// - bracketed paste (2004) last.
///
/// Synchronized output (2026) is followed too, but never turned back on: a
/// replay that did so would hold the screen until the program's next
/// frame. If the kept output leaves it on, the replay turns it off. The
/// kitty keyboard flags are not followed: the emulator does not support
/// them.
const TRACKED_MODES: [(u16, bool); 11] = [
    (1049, false),
    (1047, false),
    (47, false),
    (1, false),
    (25, true),
    (1000, false),
    (1002, false),
    (1003, false),
    (1006, false),
    (1004, false),
    (2004, false),
];
const SYNCHRONIZED_OUTPUT: u16 = 2026;
/// The alternate screen modes, which a soft reset leaves alone.
const SCREEN_MODES: [u16; 3] = [1049, 1047, 47];

/// Where the mode tracker is in a control sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Parse {
    #[default]
    Ground,
    /// After ESC.
    Escape,
    /// After `ESC [`.
    Csi,
    /// After `ESC [ !`: `p` makes it a soft reset.
    CsiBang,
    /// After `ESC [ ?`, collecting the parameters.
    Private,
}

/// The recent output of one terminal and the modes it left set.
#[derive(Debug)]
pub struct ReplayBuffer {
    bytes: VecDeque<u8>,
    capacity: usize,
    /// Whether output older than what is kept was dropped.
    truncated: bool,
    /// The followed modes whose state differs from when the terminal
    /// started, with their state.
    modes: Vec<(u16, bool)>,
    parse: Parse,
    /// Parameters of the private sequence being read.
    parameters: Vec<u8>,
}

impl Default for ReplayBuffer {
    fn default() -> Self {
        Self::new(REPLAY_BUFFER_BYTES)
    }
}

impl ReplayBuffer {
    /// A buffer that keeps the last `capacity` bytes.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            bytes: VecDeque::with_capacity(capacity.min(64 * 1024)),
            capacity,
            truncated: false,
            modes: Vec::new(),
            parse: Parse::Ground,
            parameters: Vec::new(),
        }
    }

    /// Records output, as it was sent to the frontend.
    pub fn push(&mut self, output: &[u8]) {
        for &byte in output {
            self.track(byte);
        }
        if output.len() >= self.capacity {
            self.bytes.clear();
            self.bytes.extend(&output[output.len() - self.capacity..]);
            self.truncated = true;
            return;
        }
        let overflow = (self.bytes.len() + output.len()).saturating_sub(self.capacity);
        if overflow > 0 {
            self.bytes.drain(..overflow);
            self.truncated = true;
        }
        self.bytes.extend(output);
    }

    /// How many bytes are kept.
    #[must_use]
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Whether nothing was recorded yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// What a new emulator is fed: the kept output, starting at a clean
    /// place when older output was dropped (see [`clean_start`]), followed
    /// by the sequences that put the followed modes in the state the
    /// program left them in.
    #[must_use]
    pub fn replay(&self) -> Vec<u8> {
        let (front, back) = self.bytes.as_slices();
        let mut kept = Vec::with_capacity(self.bytes.len() + 64);
        kept.extend_from_slice(front);
        kept.extend_from_slice(back);
        let start = if self.truncated {
            clean_start(&kept)
        } else {
            0
        };
        let mut replay = kept.split_off(start);
        for (mode, initially) in TRACKED_MODES {
            if let Some(&(_, on)) = self.modes.iter().find(|(known, _)| *known == mode)
                && on != initially
            {
                let action = if on { 'h' } else { 'l' };
                replay.extend_from_slice(format!("\x1b[?{mode}{action}").as_bytes());
            }
        }
        if self.modes.contains(&(SYNCHRONIZED_OUTPUT, true)) {
            replay.extend_from_slice(b"\x1b[?2026l");
        }
        replay
    }

    /// Follows `ESC [ ? Pn ; ... h` and `l`, the full reset `ESC c` and the
    /// soft reset `ESC [ ! p`, across calls.
    fn track(&mut self, byte: u8) {
        self.parse = match (self.parse, byte) {
            (_, ESC) => Parse::Escape,
            (Parse::Escape, b'[') => Parse::Csi,
            (Parse::Escape, b'c') => {
                // A full reset: everything as when the terminal started.
                self.modes.clear();
                Parse::Ground
            }
            (Parse::Csi, b'?') => {
                self.parameters.clear();
                Parse::Private
            }
            (Parse::Csi, b'!') => Parse::CsiBang,
            (Parse::CsiBang, b'p') => {
                // A soft reset: the modes go back to their initial state,
                // except the screen in use.
                self.modes.retain(|(mode, _)| SCREEN_MODES.contains(mode));
                Parse::Ground
            }
            (Parse::Private, b'0'..=b'9' | b';') if self.parameters.len() < 64 => {
                self.parameters.push(byte);
                Parse::Private
            }
            (Parse::Private, b'h' | b'l') => {
                self.apply(byte == b'h');
                Parse::Ground
            }
            _ => Parse::Ground,
        };
    }

    fn apply(&mut self, set: bool) {
        let parameters = std::mem::take(&mut self.parameters);
        for mode in parameters
            .split(|&b| b == b';')
            .filter_map(|digits| std::str::from_utf8(digits).ok()?.parse::<u16>().ok())
        {
            let followed = mode == SYNCHRONIZED_OUTPUT
                || TRACKED_MODES.iter().any(|(known, _)| *known == mode);
            if !followed {
                continue;
            }
            self.modes.retain(|(known, _)| *known != mode);
            self.modes.push((mode, set));
        }
    }
}

/// Where kept output that lost its beginning is best started, so that the
/// replay never begins inside a sequence:
///
/// 1. after the first line feed near the front;
/// 2. else at the first escape near the front, where a sequence starts;
/// 3. else past what looks like the end of a control sequence cut off
///    (parameters and a final byte, such as `;5;196m`), and past the rest
///    of a UTF-8 character.
///
/// A few characters of a line that was cut anyway may be dropped; the
/// content that follows is never misread.
fn clean_start(bytes: &[u8]) -> usize {
    let window = &bytes[..bytes.len().min(LINE_SEARCH_BYTES)];
    if let Some(newline) = window.iter().position(|&b| b == b'\n') {
        return newline + 1;
    }
    if let Some(escape) = window.iter().position(|&b| b == ESC) {
        return escape;
    }
    let mut start = 0;
    // Parameter bytes, intermediate bytes, then a final byte.
    let parameters = bytes
        .iter()
        .take_while(|b| (0x30..=0x3f).contains(*b))
        .count();
    let intermediates = bytes[parameters..]
        .iter()
        .take_while(|b| (0x20..=0x2f).contains(*b))
        .count();
    if parameters + intermediates > 0
        && let Some(&last) = bytes.get(parameters + intermediates)
        && (0x40..=0x7e).contains(&last)
    {
        start = parameters + intermediates + 1;
    }
    start
        + bytes[start..]
            .iter()
            .position(|&b| b & 0b1100_0000 != 0b1000_0000)
            .unwrap_or(bytes.len() - start)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_everything_while_it_fits() {
        let mut buffer = ReplayBuffer::new(64);
        buffer.push(b"hello ");
        buffer.push(b"world");

        assert_eq!(buffer.replay(), b"hello world");
        assert_eq!(buffer.len(), 11);
    }

    #[test]
    fn keeps_the_last_bytes_and_starts_at_a_line() {
        let mut buffer = ReplayBuffer::new(16);
        buffer.push(b"first line\nsecond\nthird");

        // 16 bytes kept: "ne\nsecond\nthird"... starting after the first
        // line feed.
        assert_eq!(buffer.len(), 16);
        assert_eq!(buffer.replay(), b"second\nthird");
    }

    #[test]
    fn never_starts_inside_a_character() {
        let mut buffer = ReplayBuffer::new(4);
        // "é" is two bytes; the cut falls between them.
        buffer.push("aaé€".as_bytes());

        let replay = buffer.replay();
        assert!(std::str::from_utf8(&replay).is_ok(), "{replay:?}");
        assert_eq!(replay, "€".as_bytes());
    }

    #[test]
    fn is_bounded_whatever_is_pushed() {
        let mut buffer = ReplayBuffer::new(1024);
        for _ in 0..100 {
            buffer.push(&[b'x'; 300]);
        }
        buffer.push(&[b'y'; 5000]);

        assert_eq!(buffer.len(), 1024);
        assert!(buffer.replay().iter().all(|&b| b == b'y'));
    }

    #[test]
    fn restores_the_modes_the_program_left_set() {
        let mut buffer = ReplayBuffer::new(1024);
        buffer.push(b"\x1b[?2004h\x1b[?1;1006hprompt");
        buffer.push(b"\x1b[?1l");

        let replay = buffer.replay();
        let expected = b"\x1b[?2004h\x1b[?1;1006hprompt\x1b[?1l\x1b[?1006h\x1b[?2004h";
        assert_eq!(replay, expected.as_slice());
    }

    #[test]
    fn follows_sequences_split_across_pushes() {
        let mut buffer = ReplayBuffer::new(1024);
        buffer.push(b"\x1b[?20");
        buffer.push(b"04h");

        assert!(buffer.replay().ends_with(b"\x1b[?2004h"));

        buffer.push(b"\x1b[");
        buffer.push(b"?2004l");
        assert_eq!(buffer.replay(), b"\x1b[?2004h\x1b[?2004l");
    }

    #[test]
    fn remembers_modes_set_long_before_what_is_kept() {
        let mut buffer = ReplayBuffer::new(32);
        buffer.push(b"\x1b[?1049h");
        buffer.push(&[b'z'; 200]);

        let replay = buffer.replay();
        assert!(replay.ends_with(b"\x1b[?1049h"), "{replay:?}");
        assert!(replay.len() <= 32 + 8);
    }

    #[test]
    fn ignores_modes_it_does_not_follow() {
        let mut buffer = ReplayBuffer::new(64);
        buffer.push(b"\x1b[?12h\x1b[?7h");

        assert_eq!(buffer.replay(), b"\x1b[?12h\x1b[?7h");
    }

    #[test]
    fn a_full_reset_forgets_every_mode() {
        let mut buffer = ReplayBuffer::new(1024);
        buffer.push(b"\x1b[?1049h\x1b[?1000h\x1b[?25l\x1b[?2004h");
        buffer.push(b"\x1bc$ ");

        // Mouse reporting stays off in the plain shell after the reset.
        let replay = buffer.replay();
        assert!(replay.ends_with(b"\x1bc$ "), "{replay:?}");
    }

    #[test]
    fn a_soft_reset_forgets_every_mode_but_the_screen() {
        let mut buffer = ReplayBuffer::new(1024);
        buffer.push(b"\x1b[?1049h\x1b[?1;1000;1006h\x1b[?25l\x1b[?2004h\x1b[?1004h");
        buffer.push(b"\x1b[!p");

        let replay = buffer.replay();
        assert!(replay.ends_with(b"\x1b[!p\x1b[?1049h"), "{replay:?}");
    }

    #[test]
    fn restores_a_hidden_cursor_and_focus_reporting() {
        let mut buffer = ReplayBuffer::new(1024);
        buffer.push(b"\x1b[?25l\x1b[?1004h");
        assert!(buffer.replay().ends_with(b"\x1b[?25l\x1b[?1004h"));

        // Shown again: nothing to restore.
        buffer.push(b"\x1b[?25h\x1b[?1004l");
        assert!(buffer.replay().ends_with(b"\x1b[?1004l"));
    }

    #[test]
    fn never_leaves_synchronized_output_on() {
        let mut buffer = ReplayBuffer::new(1024);
        buffer.push(b"\x1b[?2026hhalf a frame");

        let replay = buffer.replay();
        assert!(replay.ends_with(b"half a frame\x1b[?2026l"), "{replay:?}");

        buffer.push(b"\x1b[?2026l");
        assert!(buffer.replay().ends_with(b"\x1b[?2026l"));
        assert!(!buffer.replay().ends_with(b"\x1b[?2026l\x1b[?2026l"));
    }

    #[test]
    fn never_starts_inside_a_control_sequence() {
        let mut buffer = ReplayBuffer::new(16);
        // The cut falls inside `ESC [ 38 ; 5 ; 196 m`.
        buffer.push(b"xxxxxx\x1b[38;5;196mRED plain");

        let replay = buffer.replay();
        assert_eq!(
            replay,
            b"RED plain",
            "{:?}",
            String::from_utf8_lossy(&replay)
        );
    }

    #[test]
    fn starts_at_a_sequence_when_no_line_is_near() {
        let mut buffer = ReplayBuffer::new(12);
        buffer.push(b"0123456789abc\x1b[1mBOLD");

        assert_eq!(buffer.replay(), b"\x1b[1mBOLD");
    }
}
