//! Flow control between a session and its frontend.
//!
//! The frontend acknowledges the output it has finished processing. While
//! too much output is unacknowledged, the session stops taking output from
//! its PTY, so the buffers behind it fill up and the process blocks in
//! `write`, exactly as it would in front of any slow terminal.
//!
//! The model is the one of Visual Studio Code's integrated terminal (MIT
//! licence, `src/vs/platform/terminal/common/terminal.ts`,
//! `FlowControlConstants`): a high-water mark that pauses, a low-water mark
//! that resumes, and acknowledgements sent in batches. Only the idea is
//! shared; the values are termx's own.

/// Number of unacknowledged bytes above which a session stops taking output
/// from its PTY.
///
/// This is how much output can be on its way to, or queued in, the terminal
/// emulator of one session. It is a few screens of dense output: enough for
/// the emulator to always have work queued while it is the bottleneck, and
/// small enough to keep short what is still printed after Ctrl+C, since
/// everything already buffered comes out after the process was interrupted.
/// The value is the one Orca (MIT licence) uses for the same purpose.
pub const HIGH_WATERMARK_BYTES: u64 = 256 * 1024;

/// Number of unacknowledged bytes below which a paused session resumes.
///
/// Well below the high-water mark, so that a session does not flip between
/// the two states on every acknowledgement, and well above zero, so that new
/// output is already on its way when the emulator finishes what it has.
pub const LOW_WATERMARK_BYTES: u64 = 32 * 1024;

/// How many processed bytes a frontend should accumulate before sending an
/// acknowledgement.
///
/// Acknowledging every batch would add a message per batch in the other
/// direction; this keeps it to one per 8 KiB, and to none at all for the
/// trickle of output of an interactive shell. A frontend must not use a
/// larger value than [`LOW_WATERMARK_BYTES`]: what it keeps to itself is
/// never acknowledged, so a paused session could then never get below the
/// low-water mark and would stay paused for ever.
pub const ACK_BATCH_BYTES: u64 = 8 * 1024;

const _: () = assert!(ACK_BATCH_BYTES <= LOW_WATERMARK_BYTES);
const _: () = assert!(LOW_WATERMARK_BYTES < HIGH_WATERMARK_BYTES);

/// The flow-control state of one session.
#[derive(Debug, Default)]
pub(crate) struct FlowControl {
    unacknowledged: u64,
    paused: bool,
}

impl FlowControl {
    /// Records that `bytes` were handed to the frontend.
    pub(crate) fn sent(&mut self, bytes: usize) {
        self.unacknowledged = self.unacknowledged.saturating_add(bytes as u64);
        if self.unacknowledged > HIGH_WATERMARK_BYTES {
            self.paused = true;
        }
    }

    /// Records that the frontend has processed `bytes`. Acknowledging more
    /// than was sent is harmless.
    pub(crate) fn acknowledge(&mut self, bytes: u64) {
        self.unacknowledged = self.unacknowledged.saturating_sub(bytes);
        if self.unacknowledged < LOW_WATERMARK_BYTES {
            self.paused = false;
        }
    }

    /// Gives up on the acknowledgement of everything sent so far, as if it
    /// had been acknowledged. See the stall watchdog of the pump.
    pub(crate) fn write_off(&mut self) {
        self.unacknowledged = 0;
        self.paused = false;
    }

    /// How many more bytes can be handed to the frontend before the session
    /// is paused. Zero while it is paused.
    pub(crate) fn credit(&self) -> usize {
        if self.paused {
            return 0;
        }
        let credit = (HIGH_WATERMARK_BYTES + 1).saturating_sub(self.unacknowledged);
        usize::try_from(credit).unwrap_or(usize::MAX)
    }

    /// Whether the session must leave its output where it is for now.
    pub(crate) fn is_paused(&self) -> bool {
        self.paused
    }

    /// Bytes handed to the frontend and not acknowledged yet.
    pub(crate) fn unacknowledged(&self) -> u64 {
        self.unacknowledged
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HIGH: usize = HIGH_WATERMARK_BYTES as usize;

    #[test]
    fn output_flows_until_the_high_water_mark_is_exceeded() {
        let mut flow = FlowControl::default();

        flow.sent(HIGH);
        assert!(!flow.is_paused(), "at the mark is still fine");

        flow.sent(1);
        assert!(flow.is_paused());
        assert_eq!(flow.unacknowledged(), HIGH_WATERMARK_BYTES + 1);
    }

    #[test]
    fn credit_is_what_can_still_be_sent_before_pausing() {
        let mut flow = FlowControl::default();
        assert_eq!(flow.credit(), HIGH + 1);

        flow.sent(HIGH - 10);
        assert_eq!(flow.credit(), 11);

        // Sending exactly the credit is what pauses the session.
        flow.sent(11);
        assert!(flow.is_paused());
        assert_eq!(flow.credit(), 0);

        // Still paused between the marks: no credit either.
        flow.acknowledge(HIGH_WATERMARK_BYTES / 2);
        assert_eq!(flow.credit(), 0);
    }

    #[test]
    fn a_paused_session_resumes_only_below_the_low_water_mark() {
        let mut flow = FlowControl::default();
        flow.sent(HIGH + 1);

        // Acknowledging most of it is not enough...
        flow.acknowledge(HIGH_WATERMARK_BYTES + 1 - LOW_WATERMARK_BYTES);
        assert_eq!(flow.unacknowledged(), LOW_WATERMARK_BYTES);
        assert!(flow.is_paused(), "at the low-water mark is still paused");

        // ...one more byte is.
        flow.acknowledge(1);
        assert!(!flow.is_paused());
    }

    #[test]
    fn a_session_between_the_marks_keeps_the_state_it_was_in() {
        let mut flow = FlowControl::default();

        // Coming from below: flowing.
        flow.sent(HIGH / 2);
        assert!(!flow.is_paused());

        // Coming from above: paused.
        flow.sent(HIGH);
        assert!(flow.is_paused());
        flow.acknowledge(HIGH_WATERMARK_BYTES);
        assert_eq!(flow.unacknowledged(), HIGH_WATERMARK_BYTES / 2);
        assert!(flow.is_paused());
    }

    #[test]
    fn acknowledging_more_than_was_sent_is_harmless() {
        let mut flow = FlowControl::default();
        flow.sent(100);

        flow.acknowledge(u64::MAX);

        assert_eq!(flow.unacknowledged(), 0);
        assert!(!flow.is_paused());
        flow.sent(HIGH + 1);
        assert!(flow.is_paused(), "the count did not wrap around");
    }

    #[test]
    fn a_frontend_that_batches_its_acknowledgements_cannot_wedge_a_session() {
        let mut flow = FlowControl::default();
        flow.sent(HIGH + 1);

        // The frontend has processed everything, but keeps the remainder
        // that does not fill a batch to itself.
        let processed = HIGH_WATERMARK_BYTES + 1;
        let acknowledged = processed - processed % ACK_BATCH_BYTES;
        flow.acknowledge(acknowledged);

        assert!(flow.unacknowledged() < ACK_BATCH_BYTES);
        assert!(!flow.is_paused());
    }
}
