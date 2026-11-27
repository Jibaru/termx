//! The recent output of a terminal, kept to repaint a frontend that
//! attaches to it later: a window that reloaded its page while the
//! terminal kept running.
//!
//! [`ReplayBuffer`] keeps the last [`REPLAY_BUFFER_BYTES`] of output as
//! raw bytes, and follows the terminal modes a program sets once and
//! relies on afterwards (bracketed paste, application cursor keys, mouse
//! reporting, the alternate screen). A new emulator fed
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

/// The DEC private modes that are followed, in the order they are restored.
/// The alternate screen comes first: switching to it clears what is shown.
const TRACKED_MODES: [u16; 8] = [1049, 1047, 47, 1, 1000, 1002, 1003, 1006];
/// Bracketed paste is restored last, after the mouse modes.
const BRACKETED_PASTE: u16 = 2004;

/// Where the mode tracker is in a control sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Parse {
    #[default]
    Ground,
    /// After ESC.
    Escape,
    /// After `ESC [`.
    Csi,
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
    /// The modes currently set, by number.
    modes: Vec<u16>,
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
    /// place when older output was dropped (the start of a line nearby,
    /// else the start of a character), followed by the sequences that set
    /// the modes the program left set.
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
        for mode in TRACKED_MODES
            .iter()
            .chain(std::iter::once(&BRACKETED_PASTE))
        {
            if self.modes.contains(mode) {
                replay.extend_from_slice(format!("\x1b[?{mode}h").as_bytes());
            }
        }
        replay
    }

    /// Follows `ESC [ ? Pn ; ... h` and `l` across calls.
    fn track(&mut self, byte: u8) {
        self.parse = match (self.parse, byte) {
            (_, ESC) => Parse::Escape,
            (Parse::Escape, b'[') => Parse::Csi,
            (Parse::Csi, b'?') => {
                self.parameters.clear();
                Parse::Private
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
            if !TRACKED_MODES.contains(&mode) && mode != BRACKETED_PASTE {
                continue;
            }
            self.modes.retain(|&known| known != mode);
            if set {
                self.modes.push(mode);
            }
        }
    }
}

/// Where kept output that lost its beginning is best started: after the
/// first line feed near the front, else at the first byte that starts a
/// UTF-8 character.
fn clean_start(bytes: &[u8]) -> usize {
    let window = &bytes[..bytes.len().min(LINE_SEARCH_BYTES)];
    if let Some(newline) = window.iter().position(|&b| b == b'\n') {
        return newline + 1;
    }
    bytes
        .iter()
        .position(|&b| b & 0b1100_0000 != 0b1000_0000)
        .unwrap_or(bytes.len())
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
        buffer.push(b"\x1b[?2026h\x1b[?7h");

        assert_eq!(buffer.replay(), b"\x1b[?2026h\x1b[?7h");
    }
}
