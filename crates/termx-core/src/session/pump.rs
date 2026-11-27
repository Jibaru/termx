//! The output pump of one session: takes output from the PTY, batches it,
//! hands the batches to the sink under flow control and, at the end, reports
//! how the process exited.
//!
//! A [`Pump`] is a state machine that never blocks and never reads a clock:
//! whoever drives it calls [`Pump::poll`] with the current time whenever
//! something may have changed, and [`Pump::deadline`] says when to call it
//! again if nothing does. That makes it testable with a scripted source, a
//! recording sink and made-up instants.

use std::sync::Weak;
use std::time::{Duration, Instant};

use super::batch::{BATCH_MAX_BYTES, Batcher};
use super::flow::FlowControl;
use super::{OutputFilter, SessionId, SessionSink};
use crate::diagnostics::tracing::warn;
use crate::pty::{ExitStatus, Pty, PtyOutput, RecvTimeoutError};

/// How often the exit status is looked for when the output of a process has
/// ended and its exit status is not known yet.
///
/// The status is normally published before the output ends, so this only
/// matters for a process that closed its terminal and kept running.
pub(crate) const EXIT_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How long a session may stay paused without receiving any
/// acknowledgement before the bytes it is waiting for are written off.
///
/// A healthy frontend acknowledges within milliseconds, and a busy one
/// within a fraction of a second, so five seconds without a single
/// acknowledgement means that something is wrong: an acknowledgement was
/// lost, or the frontend is stuck. Waiting for ever would freeze the
/// terminal with no way out for the user.
pub const STALL_TIMEOUT: Duration = Duration::from_secs(5);

/// How many times in a row the outstanding bytes of a session are written
/// off without any acknowledgement in between.
///
/// Each write-off lets about one high-water mark of output through, so a
/// frontend that never acknowledges again receives a trickle (some 50 KiB
/// per second) for a minute, about 3 MiB in all, and then nothing: a
/// frontend that is dead although its channel looks alive must not be sent
/// output without limit.
pub const STALL_WRITE_OFFS_MAX: u32 = 12;

/// Capacity given to the output buffer of the PTY of a session, in bytes.
///
/// The PTY layer buffers a megabyte by default, for a consumer that reads
/// when it pleases. A session reads as soon as the frontend can take more,
/// so a large buffer would only hold output that is printed long after the
/// user pressed Ctrl+C. One batch is enough to never keep the pump waiting.
pub const PTY_BUFFER_BYTES: usize = BATCH_MAX_BYTES;

/// What a non-blocking read of the output returned.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Received {
    /// Some output.
    Data(Vec<u8>),
    /// Nothing for now.
    Empty,
    /// The output has ended.
    Closed,
}

/// Where a pump takes its output from. Implemented by a PTY, and by scripted
/// sources in tests.
pub(crate) trait Source: Send {
    /// Takes at most `max` bytes of the output available now, without
    /// blocking. What is not taken stays in the source.
    fn try_recv(&mut self, max: usize) -> Received;

    /// The exit status of the process, once it is known.
    fn exit_status(&mut self) -> Option<ExitStatus>;
}

/// The output of a real PTY.
pub(crate) struct PtySource {
    output: PtyOutput,
    /// Weak, so that the pump never keeps a terminal alive that its owner
    /// has dropped in order to kill it.
    pty: Weak<Pty>,
}

impl PtySource {
    pub(crate) fn new(mut output: PtyOutput, pty: Weak<Pty>) -> Self {
        output.set_capacity(PTY_BUFFER_BYTES);
        Self { output, pty }
    }
}

impl Source for PtySource {
    fn try_recv(&mut self, max: usize) -> Received {
        match self.output.try_recv_up_to(max) {
            Ok(bytes) => Received::Data(bytes),
            Err(RecvTimeoutError::Timeout) => Received::Empty,
            Err(RecvTimeoutError::Closed) => Received::Closed,
        }
    }

    fn exit_status(&mut self) -> Option<ExitStatus> {
        self.pty.upgrade()?.try_wait()
    }
}

/// What [`Pump::poll`] left the pump in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PumpState {
    /// The session is running, or has output left to deliver.
    Running,
    /// Everything was delivered and the exit was reported: the pump is done
    /// and must not be polled again.
    Finished,
}

/// The output pump of one session.
pub(crate) struct Pump {
    id: SessionId,
    source: Box<dyn Source>,
    sink: Box<dyn SessionSink>,
    batcher: Batcher,
    flow: FlowControl,
    /// The source reported the end of the output.
    source_closed: bool,
    /// When to look for the exit status again, once the output has ended
    /// without it.
    exit_check: Option<Instant>,
    /// Since when the session has been paused without an acknowledgement.
    stalled_since: Option<Instant>,
    /// Write-offs since the last acknowledgement.
    write_offs: u32,
    /// What the output goes through first.
    filter: Option<Box<dyn OutputFilter>>,
}

impl Pump {
    pub(crate) fn new(id: SessionId, source: Box<dyn Source>, sink: Box<dyn SessionSink>) -> Self {
        Self {
            id,
            source,
            sink,
            batcher: Batcher::default(),
            flow: FlowControl::default(),
            source_closed: false,
            exit_check: None,
            stalled_since: None,
            write_offs: 0,
            filter: None,
        }
    }

    /// Passes the output through `filter` before anything else.
    pub(crate) fn with_filter(mut self, filter: Option<Box<dyn OutputFilter>>) -> Self {
        self.filter = filter;
        self
    }

    /// Records that the frontend has processed `bytes` of output at `now`.
    /// Call [`poll`](Self::poll) afterwards: the session may have resumed.
    pub(crate) fn acknowledge(&mut self, now: Instant, bytes: u64) {
        self.flow.acknowledge(bytes);
        if bytes > 0 {
            // The frontend is alive: the watchdog starts over.
            self.write_offs = 0;
            self.stalled_since = self.flow.is_paused().then_some(now);
        }
    }

    /// The stall watchdog: resumes a session that has been paused for
    /// [`STALL_TIMEOUT`] without any acknowledgement, by writing off what
    /// it was waiting for. See the module documentation of `session`.
    fn resume_if_stalled(&mut self, now: Instant) {
        let Some(since) = self.stalled_since else {
            return;
        };
        if !self.flow.is_paused()
            || now.saturating_duration_since(since) < STALL_TIMEOUT
            || self.write_offs >= STALL_WRITE_OFFS_MAX
            || !self.sink.is_alive()
        {
            return;
        }
        if self.write_offs == 0 {
            // Once per stall: the write-offs that follow are the same event.
            warn!(
                session_id = self.id.as_u64(),
                unacknowledged = self.flow.unacknowledged(),
                "the frontend stopped acknowledging output; delivering slowly without it"
            );
        }
        self.write_offs += 1;
        self.flow.write_off();
        self.stalled_since = None;
    }

    /// Whether the watchdog can still resume this session by itself.
    fn can_resume_by_itself(&self) -> bool {
        self.write_offs < STALL_WRITE_OFFS_MAX && self.sink.is_alive()
    }

    /// Moves as much output as is allowed and due at `now`.
    pub(crate) fn poll(&mut self, now: Instant) -> PumpState {
        self.resume_if_stalled(now);
        let state = self.pump(now);
        if !self.flow.is_paused() {
            self.stalled_since = None;
        } else if self.stalled_since.is_none() {
            self.stalled_since = Some(now);
        }
        state
    }

    fn pump(&mut self, now: Instant) -> PumpState {
        // What a filter kept back for too long goes out as it is.
        if let Some(filter) = &mut self.filter
            && filter.release_at().is_some_and(|at| at <= now)
        {
            let held = filter.release();
            if !held.is_empty() {
                self.batcher.push(now, held);
            }
        }
        loop {
            let mut progressed = false;

            // Take output from the PTY, but never more than can be passed
            // on: at most what fills one batch, and at most what the
            // frontend may still be sent before the session is paused.
            // Everything else stays in the PTY's own small buffer, where it
            // throttles the process, and where little of it is left to
            // print once the process is interrupted.
            while !self.source_closed && !self.flow.is_paused() {
                let room = BATCH_MAX_BYTES
                    .saturating_sub(self.batcher.len())
                    .min(self.flow.credit());
                if room == 0 {
                    break;
                }
                match self.source.try_recv(room) {
                    Received::Data(bytes) => {
                        let bytes = match &mut self.filter {
                            Some(filter) => filter.filter(bytes),
                            None => bytes,
                        };
                        if !bytes.is_empty() {
                            self.batcher.push(now, bytes);
                        }
                        progressed = true;
                    }
                    Received::Empty => break,
                    Received::Closed => {
                        if let Some(filter) = &mut self.filter {
                            let held = filter.finish();
                            if !held.is_empty() {
                                self.batcher.push(now, held);
                            }
                        }
                        self.source_closed = true;
                        progressed = true;
                    }
                }
            }

            while !self.flow.is_paused() {
                let Some(batch) = self.batcher.next_batch(now, self.source_closed) else {
                    break;
                };
                self.flow.sent(batch.len());
                self.sink.output(self.id, batch);
                progressed = true;
            }

            if !progressed {
                break;
            }
        }

        if !(self.source_closed && self.batcher.is_empty()) {
            return PumpState::Running;
        }
        match self.source.exit_status() {
            Some(status) => {
                self.sink.exited(self.id, status);
                PumpState::Finished
            }
            None => {
                self.exit_check = Some(now + EXIT_POLL_INTERVAL);
                PumpState::Running
            }
        }
    }

    /// When [`poll`](Self::poll) must be called again even if nothing
    /// happens, or `None` if only an event can make it progress: new output
    /// or an acknowledgement.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        if self.source_closed && self.batcher.is_empty() {
            return self.exit_check;
        }
        if self.flow.is_paused() {
            // An acknowledgement lets the pending output out, or the
            // watchdog when none comes.
            return self
                .stalled_since
                .filter(|_| self.can_resume_by_itself())
                .map(|since| since + STALL_TIMEOUT);
        }
        let release = self.filter.as_ref().and_then(|filter| filter.release_at());
        match (self.batcher.deadline(), release) {
            (Some(batch), Some(release)) => Some(batch.min(release)),
            (batch, release) => batch.or(release),
        }
    }
}

#[cfg(test)]
pub(crate) mod testing {
    //! Scripted sources and recording sinks, shared by the tests of the pump
    //! and of the driver.

    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex, PoisonError};

    use super::{Received, Source};
    use crate::pty::ExitStatus;
    use crate::session::{SessionId, SessionSink};

    /// An exit status with the given code.
    pub(crate) fn exit_code(code: u32) -> ExitStatus {
        ExitStatus::from(portable_pty::ExitStatus::with_exit_code(code))
    }

    #[derive(Default)]
    struct Script {
        chunks: VecDeque<Vec<u8>>,
        closed: bool,
        exit: Option<ExitStatus>,
        reads: usize,
        largest_request: usize,
    }

    /// A source fed by the test. Clones share the same script.
    #[derive(Clone, Default)]
    pub(crate) struct ScriptedSource(Arc<Mutex<Script>>);

    impl ScriptedSource {
        fn script(&self) -> std::sync::MutexGuard<'_, Script> {
            self.0.lock().unwrap_or_else(PoisonError::into_inner)
        }

        /// Makes `bytes` available as one read.
        pub(crate) fn push(&self, bytes: impl Into<Vec<u8>>) {
            self.script().chunks.push_back(bytes.into());
        }

        /// Ends the output once what was pushed has been read.
        pub(crate) fn close(&self) {
            self.script().closed = true;
        }

        /// Publishes the exit status.
        pub(crate) fn exit(&self, status: ExitStatus) {
            self.script().exit = Some(status);
        }

        /// Bytes pushed and not read yet.
        pub(crate) fn unread(&self) -> usize {
            self.script().chunks.iter().map(Vec::len).sum()
        }

        /// The most bytes a single read ever asked for.
        pub(crate) fn largest_request(&self) -> usize {
            self.script().largest_request
        }

        /// Number of reads that returned output.
        pub(crate) fn reads(&self) -> usize {
            self.script().reads
        }
    }

    impl Source for ScriptedSource {
        fn try_recv(&mut self, max: usize) -> Received {
            let mut script = self.script();
            script.largest_request = script.largest_request.max(max);
            match script.chunks.pop_front() {
                Some(mut chunk) => {
                    script.reads += 1;
                    if chunk.len() > max {
                        let rest = chunk.split_off(max);
                        script.chunks.push_front(rest);
                    }
                    Received::Data(chunk)
                }
                None if script.closed => Received::Closed,
                None => Received::Empty,
            }
        }

        fn exit_status(&mut self) -> Option<ExitStatus> {
            self.script().exit.clone()
        }
    }

    /// What a sink was told, in order.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) enum Delivery {
        Output(SessionId, Vec<u8>),
        Exited(SessionId, ExitStatus),
    }

    /// A sink that records what it is told. Clones share the same record.
    #[derive(Clone, Default)]
    pub(crate) struct RecordingSink(
        Arc<Mutex<Vec<Delivery>>>,
        Arc<std::sync::atomic::AtomicBool>,
    );

    impl RecordingSink {
        /// Makes the sink report that its frontend is gone.
        pub(crate) fn disconnect(&self) {
            self.1.store(true, std::sync::atomic::Ordering::SeqCst);
        }

        pub(crate) fn deliveries(&self) -> Vec<Delivery> {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }

        /// The batches of output received so far.
        pub(crate) fn batches(&self) -> Vec<Vec<u8>> {
            self.deliveries()
                .into_iter()
                .filter_map(|delivery| match delivery {
                    Delivery::Output(_, bytes) => Some(bytes),
                    Delivery::Exited(..) => None,
                })
                .collect()
        }

        /// Every byte of output received so far.
        pub(crate) fn output(&self) -> Vec<u8> {
            self.batches().concat()
        }

        /// The exit status, if the exit was reported.
        pub(crate) fn exit(&self) -> Option<ExitStatus> {
            self.deliveries()
                .into_iter()
                .find_map(|delivery| match delivery {
                    Delivery::Exited(_, status) => Some(status),
                    Delivery::Output(..) => None,
                })
        }
    }

    impl SessionSink for RecordingSink {
        fn output(&mut self, session: SessionId, bytes: Vec<u8>) {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(Delivery::Output(session, bytes));
        }

        fn exited(&mut self, session: SessionId, status: ExitStatus) {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(Delivery::Exited(session, status));
        }

        fn is_alive(&self) -> bool {
            !self.1.load(std::sync::atomic::Ordering::SeqCst)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{Delivery, RecordingSink, ScriptedSource, exit_code};
    use super::*;
    use crate::session::batch::{BATCH_WINDOW, BOUNDARY_HOLD_MAX};
    use crate::session::flow::{ACK_BATCH_BYTES, HIGH_WATERMARK_BYTES, LOW_WATERMARK_BYTES};

    const ID: SessionId = SessionId::from_raw(7);
    const MS: Duration = Duration::from_millis(1);

    struct Fixture {
        pump: Pump,
        source: ScriptedSource,
        sink: RecordingSink,
        /// The made-up clock: tests add to it.
        now: Instant,
    }

    fn fixture() -> Fixture {
        let source = ScriptedSource::default();
        let sink = RecordingSink::default();
        Fixture {
            pump: Pump::new(ID, Box::new(source.clone()), Box::new(sink.clone())),
            source,
            sink,
            now: Instant::now(),
        }
    }

    impl Fixture {
        fn poll(&mut self) -> PumpState {
            self.pump.poll(self.now)
        }

        /// Moves the clock to the pump's deadline and polls, as the driver
        /// does. Panics if the pump has no deadline.
        fn poll_at_deadline(&mut self) -> PumpState {
            self.now = self.pump.deadline().expect("the pump has no deadline");
            self.poll()
        }

        /// Acknowledges like a frontend that has processed everything it
        /// was sent and batches its acknowledgements.
        fn acknowledge_all_processed(&mut self, acknowledged: &mut u64) {
            let processed = self.sink.output().len() as u64;
            let due = (processed - *acknowledged) / ACK_BATCH_BYTES * ACK_BATCH_BYTES;
            self.pump.acknowledge(self.now, due);
            *acknowledged += due;
        }
    }

    #[test]
    fn output_is_delivered_after_the_window_as_one_batch() {
        let mut f = fixture();
        f.source.push("hello ");
        f.source.push("world");

        assert_eq!(f.poll(), PumpState::Running);
        assert!(f.sink.batches().is_empty(), "the window has not ended");
        assert_eq!(f.pump.deadline(), Some(f.now + BATCH_WINDOW));

        f.poll_at_deadline();
        assert_eq!(f.sink.batches(), [b"hello world".to_vec()]);
        assert_eq!(f.pump.deadline(), None, "idle: only an event wakes it");
    }

    /// Removes every `#`, and keeps a trailing `!` back until it knows
    /// what follows.
    struct NoHashes {
        held: Vec<u8>,
    }

    impl OutputFilter for NoHashes {
        fn filter(&mut self, bytes: Vec<u8>) -> Vec<u8> {
            let mut all = std::mem::take(&mut self.held);
            all.extend(bytes.into_iter().filter(|&b| b != b'#'));
            if all.last() == Some(&b'!') {
                self.held = all.split_off(all.len() - 1);
            }
            all
        }

        fn finish(&mut self) -> Vec<u8> {
            std::mem::take(&mut self.held)
        }
    }

    #[test]
    fn a_filter_sees_the_output_before_it_is_batched_and_counted() {
        let mut f = fixture();
        f.pump = Pump::new(ID, Box::new(f.source.clone()), Box::new(f.sink.clone()))
            .with_filter(Some(Box::new(NoHashes { held: Vec::new() })));
        f.source.push("##a#b!");
        f.poll();
        f.poll_at_deadline();
        assert_eq!(f.sink.output(), b"ab");

        // What was held back comes out when the output ends.
        f.source.close();
        f.source.exit(exit_code(0));
        assert_eq!(f.poll(), PumpState::Finished);
        assert_eq!(f.sink.output(), b"ab!");
        assert_eq!(f.sink.exit(), Some(exit_code(0)));
    }

    /// Holds a trailing ESC back until `at`.
    struct HoldsEscape {
        held: Vec<u8>,
        at: Instant,
        holding: bool,
    }

    impl OutputFilter for HoldsEscape {
        fn filter(&mut self, mut bytes: Vec<u8>) -> Vec<u8> {
            if bytes.last() == Some(&0x1b) {
                self.held = bytes.split_off(bytes.len() - 1);
                self.holding = true;
            }
            bytes
        }

        fn release_at(&self) -> Option<Instant> {
            self.holding.then_some(self.at)
        }

        fn release(&mut self) -> Vec<u8> {
            self.holding = false;
            std::mem::take(&mut self.held)
        }
    }

    #[test]
    fn what_a_filter_holds_back_goes_out_when_its_time_comes() {
        let mut f = fixture();
        let at = f.now + 3 * MS;
        f.pump = Pump::new(ID, Box::new(f.source.clone()), Box::new(f.sink.clone())).with_filter(
            Some(Box::new(HoldsEscape {
                held: Vec::new(),
                at,
                holding: false,
            })),
        );
        f.source.push("Password: \x1b");
        f.poll();
        // The pump wakes up for the held byte even if nothing else comes.
        assert!(f.pump.deadline().is_some_and(|deadline| deadline <= at));
        while f.pump.deadline().is_some() {
            f.poll_at_deadline();
        }
        assert_eq!(f.sink.output(), b"Password: \x1b");
        assert_eq!(f.pump.deadline(), None);
    }

    #[test]
    fn output_arriving_during_the_window_joins_the_batch() {
        let mut f = fixture();
        f.source.push("a");
        f.poll();

        f.now += 2 * MS;
        f.source.push("b");
        f.poll();
        assert!(f.sink.batches().is_empty());

        f.poll_at_deadline();
        assert_eq!(f.sink.batches(), [b"ab".to_vec()]);
    }

    #[test]
    fn bytes_pass_through_unmodified() {
        let mut f = fixture();
        let bytes: Vec<u8> = (0..=255).cycle().take(10_000).collect();
        f.source.push(bytes.clone());

        f.poll();
        f.now += BOUNDARY_HOLD_MAX;
        f.poll();

        assert_eq!(f.sink.output(), bytes);
    }

    #[test]
    fn a_sequence_split_between_two_reads_is_delivered_whole() {
        let mut f = fixture();
        f.source.push("plain\x1b[38;2;1");
        f.poll();
        f.poll_at_deadline();
        assert_eq!(f.sink.batches(), [b"plain".to_vec()]);

        f.source.push(";2;3mcolour");
        f.poll();
        f.poll_at_deadline();
        assert_eq!(
            f.sink.batches(),
            [b"plain".to_vec(), b"\x1b[38;2;1;2;3mcolour".to_vec()]
        );
    }

    #[test]
    fn a_synchronized_update_split_between_reads_is_delivered_whole() {
        let mut f = fixture();
        f.source.push("\x1b[?2026h\x1b[Hfirst half ");
        f.poll();
        f.poll_at_deadline();
        assert!(f.sink.batches().is_empty(), "held for the rest");
        assert_eq!(
            f.pump.deadline(),
            Some(f.now - BATCH_WINDOW + BOUNDARY_HOLD_MAX)
        );

        f.now += MS;
        f.source.push("second half\x1b[?2026l");
        f.poll();
        assert_eq!(
            f.sink.batches(),
            [b"\x1b[?2026h\x1b[Hfirst half second half\x1b[?2026l".to_vec()]
        );
    }

    #[test]
    fn held_output_is_delivered_when_the_hold_expires() {
        let mut f = fixture();
        f.source.push("\x1b[?2026hnever closed");
        f.poll();

        f.poll_at_deadline();
        assert!(f.sink.batches().is_empty());
        f.poll_at_deadline();

        assert_eq!(f.sink.output(), b"\x1b[?2026hnever closed");
        assert_eq!(f.pump.deadline(), None);
    }

    #[test]
    fn the_exit_is_reported_after_the_last_output() {
        let mut f = fixture();
        f.source.push("bye\r\n");
        f.source.close();
        f.source.exit(exit_code(3));

        // The end of the output flushes without waiting for the window.
        assert_eq!(f.poll(), PumpState::Finished);
        assert_eq!(
            f.sink.deliveries(),
            [
                Delivery::Output(ID, b"bye\r\n".to_vec()),
                Delivery::Exited(ID, exit_code(3)),
            ]
        );
    }

    #[test]
    fn a_session_without_output_still_reports_its_exit() {
        let mut f = fixture();
        f.source.close();
        f.source.exit(exit_code(0));

        assert_eq!(f.poll(), PumpState::Finished);
        assert_eq!(f.sink.deliveries(), [Delivery::Exited(ID, exit_code(0))]);
    }

    #[test]
    fn an_exit_status_that_comes_late_is_waited_for_without_blocking() {
        let mut f = fixture();
        f.source.push("detached");
        f.source.close();

        assert_eq!(f.poll(), PumpState::Running);
        assert_eq!(f.sink.output(), b"detached");
        assert_eq!(f.sink.exit(), None);
        assert_eq!(f.pump.deadline(), Some(f.now + EXIT_POLL_INTERVAL));

        // Still unknown at the next check: checked again later.
        assert_eq!(f.poll_at_deadline(), PumpState::Running);
        assert_eq!(f.pump.deadline(), Some(f.now + EXIT_POLL_INTERVAL));

        f.source.exit(exit_code(9));
        assert_eq!(f.poll_at_deadline(), PumpState::Finished);
        assert_eq!(f.sink.exit(), Some(exit_code(9)));
    }

    #[test]
    fn a_frontend_that_never_acknowledges_stops_the_pump() {
        let mut f = fixture();
        let chunk = vec![b'x'; BATCH_MAX_BYTES];
        // Far more than the frontend is allowed to have outstanding.
        let chunks = 64;
        for _ in 0..chunks {
            f.source.push(chunk.clone());
        }

        // Polled for a while, but for less than the stall watchdog allows.
        for _ in 0..100 {
            f.poll();
            f.now += BOUNDARY_HOLD_MAX;
        }

        let sent = f.sink.output().len();
        assert!(
            sent as u64 > HIGH_WATERMARK_BYTES,
            "the pump stopped too early: {sent} bytes"
        );
        assert!(
            sent as u64 <= HIGH_WATERMARK_BYTES + BATCH_MAX_BYTES as u64,
            "the pump overshot the high-water mark by more than a batch: {sent} bytes"
        );
        // What was not sent is still in the source, where it throttles the
        // process.
        assert_eq!(f.source.unread(), chunks * BATCH_MAX_BYTES - sent);
        // The only thing left to wait for is the watchdog: no spinning.
        let paused_at = f.pump.deadline().expect("no watchdog deadline") - STALL_TIMEOUT;
        assert!(paused_at <= f.now);

        // A frontend that is gone is never resumed for: nothing moves,
        // however long it lasts.
        f.sink.disconnect();
        assert_eq!(f.pump.deadline(), None, "a paused pump must not spin");
        let reads = f.source.reads();
        f.now += Duration::from_secs(3600);
        f.poll();
        assert_eq!(f.sink.output().len(), sent);
        assert_eq!(f.source.reads(), reads);
    }

    /// Fills a pump until it pauses and returns how much it sent.
    fn pause(f: &mut Fixture) -> usize {
        for _ in 0..256 {
            f.source.push(vec![b'x'; BATCH_MAX_BYTES]);
        }
        f.poll();
        f.now += BATCH_WINDOW;
        f.poll();
        assert!(f.sink.output().len() as u64 > HIGH_WATERMARK_BYTES);
        f.sink.output().len()
    }

    #[test]
    fn a_stalled_session_resumes_slowly_when_no_acknowledgement_comes() {
        let mut f = fixture();
        let sent = pause(&mut f);

        // Nothing until the timeout...
        f.now += STALL_TIMEOUT - MS * 10;
        f.poll();
        assert_eq!(f.sink.output().len(), sent);

        // ...then about one more high-water mark, and a new wait.
        f.poll_at_deadline();
        f.now += BATCH_WINDOW;
        f.poll();
        let resumed = f.sink.output().len() - sent;
        assert!(resumed as u64 > HIGH_WATERMARK_BYTES, "{resumed} bytes");
        assert!(resumed as u64 <= HIGH_WATERMARK_BYTES + BATCH_MAX_BYTES as u64);
        f.now += STALL_TIMEOUT / 2;
        f.poll();
        assert_eq!(f.sink.output().len(), sent + resumed, "it is a trickle");
    }

    #[test]
    fn a_frontend_that_never_comes_back_is_given_up_on() {
        let mut f = fixture();
        let sent = pause(&mut f);

        // An hour of silence.
        for _ in 0..3600 {
            f.now += Duration::from_secs(1);
            f.poll();
        }

        // The trickle was bounded, and the pump no longer wakes up for it.
        let trickled = (f.sink.output().len() - sent) as u64;
        let most =
            u64::from(STALL_WRITE_OFFS_MAX) * (HIGH_WATERMARK_BYTES + BATCH_MAX_BYTES as u64);
        assert!(trickled > 0);
        assert!(
            trickled <= most,
            "{trickled} bytes were sent unacknowledged"
        );
        assert_eq!(f.pump.deadline(), None);
        assert!(f.source.unread() > 0, "the process is still held back");
    }

    #[test]
    fn an_acknowledgement_ends_the_stall_and_resets_the_watchdog() {
        let mut f = fixture();
        pause(&mut f);
        // Written off almost as often as allowed...
        for _ in 0..STALL_WRITE_OFFS_MAX - 1 {
            f.now += STALL_TIMEOUT;
            f.poll();
            f.now += BATCH_WINDOW;
            f.poll();
        }
        let before = f.sink.output().len();

        // ...when the frontend comes back to life.
        f.pump.acknowledge(f.now, u64::MAX);
        f.poll();
        f.now += BATCH_WINDOW;
        f.poll();
        assert!(f.sink.output().len() > before, "normal flow resumed");

        // Had the count not started over, this second stall would be the
        // last write-off; instead the watchdog is fully armed again.
        for _ in 0..3 {
            f.now += STALL_TIMEOUT;
            f.poll();
            f.now += BATCH_WINDOW;
            f.poll();
        }
        assert!(f.pump.deadline().is_some(), "the watchdog gave up early");
    }

    #[test]
    fn an_acknowledgement_that_does_not_resume_restarts_the_timeout() {
        let mut f = fixture();
        let sent = pause(&mut f);

        f.now += STALL_TIMEOUT - MS;
        // Not enough to get below the low-water mark, but a sign of life.
        f.pump.acknowledge(f.now, 1);
        f.poll();
        f.now += STALL_TIMEOUT - MS;
        f.poll();

        assert_eq!(f.sink.output().len(), sent, "written off too early");
        assert_eq!(f.pump.deadline(), Some(f.now + MS));
    }

    #[test]
    fn the_pump_never_takes_more_than_it_can_pass_on() {
        let mut f = fixture();
        // One huge read is available, as when a process floods the
        // terminal: several megabytes, far above the high-water mark.
        let flood = 4 * 1024 * 1024;
        f.source.push(vec![b'x'; flood]);

        for _ in 0..100 {
            f.poll();
            f.now += BOUNDARY_HOLD_MAX;
        }

        // The frontend acknowledged nothing, so the pump paused. What it
        // took from the source is what it sent, to the byte: nothing is
        // parked in the pump, where it would be printed after a Ctrl+C.
        let sent = f.sink.output().len();
        assert_eq!(flood - f.source.unread(), sent);
        assert!(sent as u64 > HIGH_WATERMARK_BYTES);
        assert!(sent as u64 <= HIGH_WATERMARK_BYTES + BATCH_MAX_BYTES as u64);
        assert!(f.source.largest_request() <= BATCH_MAX_BYTES);
    }

    #[test]
    fn acknowledgements_resume_a_paused_pump_below_the_low_water_mark() {
        let mut f = fixture();
        for _ in 0..16 {
            f.source.push(vec![b'x'; BATCH_MAX_BYTES]);
        }
        f.poll();
        let sent = f.sink.output().len() as u64;
        assert!(sent > HIGH_WATERMARK_BYTES);

        // Not far enough below: still paused.
        f.pump.acknowledge(f.now, sent - LOW_WATERMARK_BYTES);
        f.poll();
        assert_eq!(f.sink.output().len() as u64, sent);

        f.pump.acknowledge(f.now, 1);
        f.poll();
        assert!(f.sink.output().len() as u64 > sent, "the pump resumed");
    }

    #[test]
    fn a_slow_frontend_receives_everything_in_order() {
        let mut f = fixture();
        let total = 4 * 1024 * 1024;
        let expected: Vec<u8> = (0..total).map(|index| (index % 251) as u8).collect();
        for chunk in expected.chunks(50_000) {
            f.source.push(chunk);
        }
        f.source.close();
        f.source.exit(exit_code(0));

        let mut acknowledged = 0;
        let mut state = PumpState::Running;
        let mut most_outstanding = 0;
        for _ in 0..10_000 {
            state = f.poll();
            if state == PumpState::Finished {
                break;
            }
            most_outstanding = most_outstanding.max(f.sink.output().len() as u64 - acknowledged);
            f.acknowledge_all_processed(&mut acknowledged);
            f.now += MS;
        }

        assert_eq!(state, PumpState::Finished);
        assert_eq!(f.sink.output(), expected);
        assert_eq!(f.sink.exit(), Some(exit_code(0)));
        assert!(
            most_outstanding <= HIGH_WATERMARK_BYTES + BATCH_MAX_BYTES as u64,
            "{most_outstanding} bytes were outstanding at once"
        );
        assert!(
            f.sink.batches().iter().all(|b| b.len() <= BATCH_MAX_BYTES),
            "a batch is larger than the limit"
        );
    }

    #[test]
    fn the_exit_waits_for_a_paused_frontend_to_take_the_last_output() {
        let mut f = fixture();
        for _ in 0..8 {
            f.source.push(vec![b'x'; BATCH_MAX_BYTES]);
        }
        f.source.close();
        f.source.exit(exit_code(0));

        assert_eq!(f.poll(), PumpState::Running);
        assert_eq!(f.sink.exit(), None, "output is still waiting");
        // Nothing to do but wait for an acknowledgement, or the watchdog.
        assert_eq!(f.pump.deadline(), Some(f.now + STALL_TIMEOUT));

        f.pump.acknowledge(f.now, u64::MAX);
        assert_eq!(f.poll(), PumpState::Finished);
        assert_eq!(f.sink.output().len(), 8 * BATCH_MAX_BYTES);
        assert!(matches!(
            f.sink.deliveries().last(),
            Some(Delivery::Exited(..))
        ));
    }
}
