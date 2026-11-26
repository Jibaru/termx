//! The thread that drives the output pumps of every session.
//!
//! One thread serves all the sessions of a manager: a session costs no
//! thread of its own beyond the two its PTY already has. The thread sleeps
//! until something happens (a PTY has output, the frontend acknowledged some,
//! a session was added or removed) or until the earliest deadline of a pump,
//! and then polls the pumps concerned. Pumps never block, so one busy
//! session cannot hold up the others for longer than it takes to hand over a
//! few batches.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::Instant;

use super::SessionId;
use super::pump::{Pump, PumpState};

/// Something the driver thread has to react to.
pub(crate) enum Event {
    /// A new session: start pumping its output.
    Add(SessionId, Box<Pump>),
    /// The PTY of this session has output, or its output has ended.
    Readable(SessionId),
    /// The frontend has processed this many bytes of this session.
    Acknowledge(SessionId, u64),
    /// The session was closed: forget it without reporting an exit. The
    /// sender, if any, is told once the pump has been dropped.
    Remove(SessionId, Option<Sender<()>>),
    /// Stop the thread, dropping every pump.
    Shutdown,
}

/// Runs until [`Event::Shutdown`] is received or every sender is gone.
///
/// `finished` is called, on this thread, when a pump has delivered
/// everything and reported the exit of its session.
pub(crate) fn run(events: &Receiver<Event>, mut finished: impl FnMut(SessionId)) {
    let mut pumps: HashMap<SessionId, Box<Pump>> = HashMap::new();
    // Reused between iterations.
    let mut due: Vec<SessionId> = Vec::new();

    loop {
        let deadline = pumps.values().filter_map(|pump| pump.deadline()).min();
        let event = match deadline {
            None => events.recv().map_err(|_| RecvTimeoutError::Disconnected),
            Some(deadline) => {
                events.recv_timeout(deadline.saturating_duration_since(Instant::now()))
            }
        };

        let now = Instant::now();
        due.clear();
        match event {
            Ok(Event::Add(id, pump)) => {
                pumps.insert(id, pump);
                due.push(id);
            }
            Ok(Event::Readable(id)) => due.push(id),
            Ok(Event::Acknowledge(id, bytes)) => {
                if let Some(pump) = pumps.get_mut(&id) {
                    pump.acknowledge(bytes);
                    due.push(id);
                }
            }
            Ok(Event::Remove(id, done)) => {
                drop(pumps.remove(&id));
                if let Some(done) = done {
                    let _ = done.send(());
                }
            }
            Ok(Event::Shutdown) | Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {}
        }

        // Deadlines are checked on every iteration, not only after a
        // timeout: a steady stream of events must not starve them.
        due.extend(
            pumps
                .iter()
                .filter(|(_, pump)| pump.deadline().is_some_and(|deadline| deadline <= now))
                .map(|(&id, _)| id),
        );
        due.sort_unstable();
        due.dedup();

        for &id in &due {
            // An event may name a session that is gone, or not added yet:
            // a PTY can be readable before its pump reaches this thread,
            // and adding a pump polls it anyway.
            let Some(pump) = pumps.get_mut(&id) else {
                continue;
            };
            if pump.poll(now) == PumpState::Finished {
                pumps.remove(&id);
                finished(id);
            }
        }
    }
}

/// Creates the channel a driver thread is fed through.
pub(crate) fn channel() -> (Sender<Event>, Receiver<Event>) {
    mpsc::channel()
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::Sender;
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    use super::*;
    use crate::session::batch::BATCH_MAX_BYTES;
    use crate::session::flow::HIGH_WATERMARK_BYTES;
    use crate::session::pump::testing::{RecordingSink, ScriptedSource, exit_code};

    /// Upper bound for any wait: generous, because CI machines are slow.
    const TIMEOUT: Duration = Duration::from_secs(30);

    struct Driver {
        events: Sender<Event>,
        finished: mpsc::Receiver<SessionId>,
        thread: Option<JoinHandle<()>>,
    }

    impl Driver {
        fn start() -> Self {
            let (events, receiver) = channel();
            let (finished_tx, finished) = mpsc::channel();
            let thread = thread::spawn(move || {
                run(&receiver, |id| {
                    let _ = finished_tx.send(id);
                });
            });
            Self {
                events,
                finished,
                thread: Some(thread),
            }
        }

        /// Adds a session and returns the handles to script and observe it.
        fn add(&self, raw_id: u64) -> (SessionId, ScriptedSource, RecordingSink) {
            let id = SessionId::from_raw(raw_id);
            let source = ScriptedSource::default();
            let sink = RecordingSink::default();
            let pump = Pump::new(id, Box::new(source.clone()), Box::new(sink.clone()));
            self.events.send(Event::Add(id, Box::new(pump))).unwrap();
            (id, source, sink)
        }

        fn send(&self, event: Event) {
            self.events.send(event).unwrap();
        }

        fn stop(mut self) {
            self.send(Event::Shutdown);
            self.thread.take().unwrap().join().unwrap();
        }
    }

    /// Waits until `condition` holds, failing the test if it never does.
    fn wait_until(what: &str, condition: impl Fn() -> bool) {
        let start = Instant::now();
        while !condition() {
            assert!(start.elapsed() < TIMEOUT, "timed out waiting until {what}");
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn output_is_delivered_without_any_further_event() {
        let driver = Driver::start();
        let (id, source, sink) = driver.add(1);

        source.push("hello");
        driver.send(Event::Readable(id));

        // Nothing else is sent: the driver wakes itself up when the
        // batching window ends.
        wait_until("the output is delivered", || sink.output() == b"hello");
        driver.stop();
    }

    #[test]
    fn a_session_is_polled_when_it_is_added() {
        let driver = Driver::start();
        let id = SessionId::from_raw(1);
        let source = ScriptedSource::default();
        let sink = RecordingSink::default();
        // The output and its notification come before the pump, as when a
        // process prints at once.
        source.push("early");
        driver.send(Event::Readable(id));
        let pump = Pump::new(id, Box::new(source.clone()), Box::new(sink.clone()));
        driver.send(Event::Add(id, Box::new(pump)));

        wait_until("the early output is delivered", || {
            sink.output() == b"early"
        });
        driver.stop();
    }

    #[test]
    fn a_finished_session_is_reported_and_forgotten() {
        let driver = Driver::start();
        let (id, source, sink) = driver.add(4);

        source.push("bye");
        source.close();
        source.exit(exit_code(2));
        driver.send(Event::Readable(id));

        assert_eq!(driver.finished.recv_timeout(TIMEOUT).unwrap(), id);
        assert_eq!(sink.output(), b"bye");
        assert_eq!(sink.exit(), Some(exit_code(2)));

        // Events for it are ignored from then on.
        driver.send(Event::Readable(id));
        driver.send(Event::Acknowledge(id, 10));
        driver.stop();
    }

    #[test]
    fn a_late_exit_status_is_picked_up_by_the_driver_itself() {
        let driver = Driver::start();
        let (id, source, sink) = driver.add(1);
        source.push("detached");
        source.close();
        driver.send(Event::Readable(id));
        // The end of the output is what flushes it without a window.
        wait_until("the end of the output is seen", || {
            sink.output() == b"detached"
        });
        assert_eq!(sink.exit(), None);

        // No event announces the status: the driver looks again by itself.
        source.exit(exit_code(5));

        assert_eq!(driver.finished.recv_timeout(TIMEOUT).unwrap(), id);
        assert_eq!(sink.exit(), Some(exit_code(5)));
        driver.stop();
    }

    #[test]
    fn a_removed_session_gets_no_more_output_and_no_exit() {
        let driver = Driver::start();
        let (id, source, sink) = driver.add(1);
        let (other, other_source, other_sink) = driver.add(2);

        let (done_tx, done_rx) = mpsc::channel();
        driver.send(Event::Remove(id, Some(done_tx)));
        done_rx.recv_timeout(TIMEOUT).unwrap();
        source.push("too late");
        source.close();
        source.exit(exit_code(0));
        driver.send(Event::Readable(id));

        // The other session proves the driver went past those events.
        other_source.push("still here");
        other_source.close();
        other_source.exit(exit_code(0));
        driver.send(Event::Readable(other));
        assert_eq!(driver.finished.recv_timeout(TIMEOUT).unwrap(), other);
        assert_eq!(other_sink.output(), b"still here");

        assert!(sink.deliveries().is_empty());
        assert!(driver.finished.try_recv().is_err());
        driver.stop();
    }

    #[test]
    fn a_paused_session_does_not_hold_the_others_up() {
        let driver = Driver::start();
        let (stuck, stuck_source, stuck_sink) = driver.add(1);
        let (lively, lively_source, lively_sink) = driver.add(2);

        // Never acknowledged: this session stalls above its high-water mark.
        for _ in 0..32 {
            stuck_source.push(vec![b'x'; BATCH_MAX_BYTES]);
        }
        driver.send(Event::Readable(stuck));
        wait_until("the first session is paused", || {
            stuck_sink.output().len() as u64 > HIGH_WATERMARK_BYTES
        });

        lively_source.push("unaffected");
        driver.send(Event::Readable(lively));
        wait_until("the second session delivers", || {
            lively_sink.output() == b"unaffected"
        });

        let stalled_at = stuck_sink.output().len();
        assert!(stalled_at < 32 * BATCH_MAX_BYTES);

        // An acknowledgement gets the first one going again.
        driver.send(Event::Acknowledge(stuck, u64::MAX));
        wait_until("the first session resumes", || {
            stuck_sink.output().len() > stalled_at
        });
        driver.stop();
    }

    #[test]
    fn many_sessions_are_served_by_the_one_thread() {
        let driver = Driver::start();
        let sessions: Vec<_> = (1..=64).map(|raw| driver.add(raw)).collect();

        for (id, source, _) in &sessions {
            source.push(format!("output of {id}"));
            source.close();
            source.exit(exit_code(0));
            driver.send(Event::Readable(*id));
        }

        let mut finished: Vec<SessionId> = (0..sessions.len())
            .map(|_| driver.finished.recv_timeout(TIMEOUT).unwrap())
            .collect();
        finished.sort_unstable();
        let expected: Vec<SessionId> = sessions.iter().map(|(id, ..)| *id).collect();
        assert_eq!(finished, expected);
        for (id, _, sink) in &sessions {
            assert_eq!(sink.output(), format!("output of {id}").into_bytes());
        }
        driver.stop();
    }

    #[test]
    fn the_thread_stops_when_every_sender_is_gone() {
        let (events, receiver) = channel();
        let thread = thread::spawn(move || run(&receiver, |_| {}));

        drop(events);

        thread.join().unwrap();
    }
}
