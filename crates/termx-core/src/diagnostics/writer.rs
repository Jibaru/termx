//! Getting records onto the disk.
//!
//! Two things matter, and they pull in opposite directions:
//!
//! - The record that explains a crash is the one written right before it, so
//!   a `WARN` or `ERROR` record must be on disk before the code that logged
//!   it goes on.
//! - Logging must never freeze the caller. The caller may be the interface
//!   thread or the thread that reads a terminal, and a disk can stall for
//!   seconds.
//!
//! So one background thread does all the writing, and callers hand their
//! records to it:
//!
//! - **`WARN` and `ERROR` records are handed over and waited for**, for at
//!   most [`URGENT_WAIT`]. Normally the thread writes the record within
//!   microseconds and the logging call returns with the record in the hands
//!   of the operating system, where it survives an abort, `process::exit` or
//!   a kill. If the disk does not answer in time the call returns anyway: the
//!   record stays queued and is written when the disk comes back, and a
//!   record says how many were written late. While the disk is stalled,
//!   further records do not wait at all. Only in that case can a warning or
//!   error be lost to a crash.
//! - **`INFO`, `DEBUG` and `TRACE` records are queued** and not waited for.
//!   The queue is bounded. When it is full the record is dropped and counted,
//!   and a record with the number of drops is written as soon as the writer
//!   catches up.
//! - The panic hook and [`LogWriter::shutdown`] write out the queue
//!   themselves and ask the operating system to flush the file to the disk.

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, TryLockError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{Map, Value};
use time::OffsetDateTime;

use super::record::{LineContext, Record};
use super::rotate::{CRASH_FILE, FIRST_ERRORS_FILE, RotatingFile, eligible_metadata};
use super::suppress::{Summary, Suppressor};

/// Target of the records the logger writes about itself.
pub const LOG_TARGET: &str = "termx::log";

/// Longest time a `WARN` or `ERROR` logging call waits for its record to be
/// written before it returns and leaves the record queued, unless
/// [`LogConfig::urgent_wait`](super::LogConfig::urgent_wait) says otherwise.
pub const URGENT_WAIT: Duration = Duration::from_millis(100);

/// `INFO` and lower records that can wait for the background thread before
/// new ones are dropped.
pub(crate) const QUEUE_CAPACITY: usize = 8 * 1024;

/// How many times [`QUEUE_CAPACITY`] the queue may hold when the extra
/// records are warnings and errors waiting for a stalled disk. Beyond that
/// they are dropped too, and counted: memory is bounded whatever happens.
const URGENT_CAPACITY_FACTOR: usize = 4;

/// Warnings and errors of a launch that are also copied to its first-errors
/// file, and the size at which the copying stops whatever their number.
pub const FIRST_ERRORS_RECORDS: u32 = 200;
const FIRST_ERRORS_BYTES: u64 = 512 * 1024;

/// How often the background thread looks for repeat counts to report when
/// nothing else wakes it.
const SUMMARY_INTERVAL: Duration = Duration::from_secs(1);

/// How long the panic hook waits for the log file when another thread is
/// writing.
const PANIC_LOCK_WAIT: Duration = Duration::from_millis(300);
const PANIC_LOCK_RETRY_DELAY: Duration = Duration::from_millis(5);

#[derive(Debug, Default)]
struct Pending {
    /// The lines to write, and whether each is a warning or an error.
    queue: VecDeque<(String, bool)>,
    /// Ticket of the last `WARN` or `ERROR` record that was queued.
    last_ticket: u64,
    /// Ticket of the last `WARN` or `ERROR` record that was written.
    written_ticket: u64,
    /// `INFO` and lower records dropped since the last report.
    dropped: u64,
    /// `WARN` and `ERROR` records dropped since the last report.
    dropped_urgent: u64,
    /// `WARN` and `ERROR` records whose caller stopped waiting, since the
    /// last report.
    deferred: u64,
    /// A caller gave up waiting and nothing was written since: the disk is
    /// stalled, and nobody else should wait for it.
    stalled: bool,
    /// The background thread is running and takes the records.
    worker_running: bool,
    /// Set by `shutdown`: from then on every record is written by its caller.
    stopped: bool,
}

/// What `take` removed from [`Pending`].
struct Batch {
    lines: VecDeque<(String, bool)>,
    last_ticket: u64,
    dropped: u64,
    dropped_urgent: u64,
    deferred: u64,
}

/// A copy of the first warnings and errors of a launch, in a file of its own
/// that the launch never deletes. However much is logged afterwards, and
/// however many files that pushes out, what went wrong first is still there.
#[derive(Debug)]
struct FirstErrors {
    path: PathBuf,
    file: Option<File>,
    records: u32,
    bytes: u64,
    /// The file could not be created, or the limits were reached.
    closed: bool,
}

impl FirstErrors {
    fn write(&mut self, line: &str) {
        if self.closed {
            return;
        }
        if self.file.is_none() {
            // Created exclusively, like every log file, so that nothing
            // planted under the name is written through.
            match OpenOptions::new()
                .append(true)
                .create_new(true)
                .open(&self.path)
            {
                Ok(file) => self.file = Some(file),
                Err(_) => {
                    self.closed = true;
                    return;
                }
            }
        }
        if let Some(file) = self.file.as_mut()
            && file.write_all(line.as_bytes()).is_ok()
        {
            self.records += 1;
            self.bytes += line.len() as u64;
        }
        if self.records >= FIRST_ERRORS_RECORDS || self.bytes >= FIRST_ERRORS_BYTES {
            self.closed = true;
            self.file = None;
        }
    }
}

/// Writes formatted lines to the log file.
#[derive(Debug)]
pub(crate) struct LogWriter {
    file: Mutex<RotatingFile>,
    pending: Mutex<Pending>,
    /// Signalled when the queue gets a record, and on shutdown.
    wake: Condvar,
    /// Signalled when records were written.
    written: Condvar,
    context: Arc<LineContext>,
    capacity: usize,
    /// Longest time a warning or error waits to be written.
    urgent_wait: Duration,
    /// Where a panic record goes when the log file cannot be had.
    crash_file: PathBuf,
    first_errors: Mutex<FirstErrors>,
    suppressor: Suppressor,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl LogWriter {
    /// A writer without a background thread: every `WARN` or `ERROR` record
    /// is written by its caller, and queued records are written along with
    /// it or by [`LogWriter::flush`].
    pub fn new(
        file: RotatingFile,
        context: Arc<LineContext>,
        capacity: usize,
        urgent_wait: Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            crash_file: file.side_file_path(CRASH_FILE),
            first_errors: Mutex::new(FirstErrors {
                path: file.side_file_path(FIRST_ERRORS_FILE),
                file: None,
                records: 0,
                bytes: 0,
                closed: false,
            }),
            suppressor: Suppressor::default(),
            file: Mutex::new(file),
            pending: Mutex::new(Pending::default()),
            wake: Condvar::new(),
            written: Condvar::new(),
            context,
            capacity,
            urgent_wait,
            worker: Mutex::new(None),
        })
    }

    /// Starts the background thread that writes the records.
    pub fn start_worker(self: &Arc<Self>) -> io::Result<()> {
        let writer = Arc::clone(self);
        lock(&self.pending).worker_running = true;
        let spawned = std::thread::Builder::new()
            .name("termx-log".to_owned())
            .spawn(move || writer.run_worker());
        match spawned {
            Ok(handle) => {
                *lock(&self.worker) = Some(handle);
                Ok(())
            }
            Err(error) => {
                lock(&self.pending).worker_running = false;
                Err(error)
            }
        }
    }

    /// Counts repeated warnings and errors; see the `suppress` module.
    pub fn suppressor(&self) -> &Suppressor {
        &self.suppressor
    }

    /// Queues the record that says how many repeats were not written.
    pub fn submit_summary(&self, summary: &Summary) {
        self.submit(self.summary_line(summary), false);
    }

    fn summary_line(&self, summary: &Summary) -> String {
        let mut fields = Map::new();
        fields.insert("repeated".to_owned(), Value::from(summary.repeated));
        fields.insert("message".to_owned(), Value::from(summary.message.as_str()));
        self.context.format(Record {
            timestamp: OffsetDateTime::now_utc(),
            level: summary.level,
            target: &summary.target,
            message: "the same record was repeated and not written again",
            span: None,
            fields,
        })
    }

    /// Queues the summaries that are due: all of them, or those whose window
    /// is over. They are not subject to the queue limit, being bounded by the
    /// number of records the suppressor tracks.
    fn queue_summaries(&self, all: bool) {
        let summaries = if all {
            self.suppressor.take_all()
        } else {
            self.suppressor.take_expired(Instant::now())
        };
        if summaries.is_empty() {
            return;
        }
        let lines: Vec<String> = summaries
            .iter()
            .map(|summary| self.summary_line(summary))
            .collect();
        let mut pending = lock(&self.pending);
        pending
            .queue
            .extend(lines.into_iter().map(|line| (line, false)));
        self.wake.notify_one();
    }

    /// Hands over one finished line.
    ///
    /// An `urgent` line has been written when this returns, unless the disk
    /// did not answer within [`URGENT_WAIT`]; then it is still queued. Other
    /// lines are queued, or dropped when the queue is full.
    pub fn submit(&self, line: String, urgent: bool) {
        let mut pending = lock(&self.pending);
        if !pending.worker_running || pending.stopped {
            if urgent {
                // Nobody else will write it: do it here.
                drop(pending);
                let mut file = lock(&self.file);
                self.write_pending(&mut file, Some(&line));
            } else if pending.stopped {
                pending.queue.push_back((line, false));
                drop(pending);
                let mut file = lock(&self.file);
                self.write_pending(&mut file, None);
            } else {
                self.enqueue(&mut pending, line);
            }
            return;
        }
        if !urgent {
            self.enqueue(&mut pending, line);
            return;
        }

        if pending.queue.len() >= self.capacity.saturating_mul(URGENT_CAPACITY_FACTOR) {
            pending.dropped_urgent = pending.dropped_urgent.saturating_add(1);
            return;
        }
        pending.last_ticket += 1;
        let ticket = pending.last_ticket;
        pending.queue.push_back((line, true));
        self.wake.notify_one();

        let deadline = Instant::now() + self.urgent_wait;
        while pending.written_ticket < ticket && pending.worker_running {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if pending.stalled || remaining.is_zero() {
                // The record stays in the queue and is written when the disk
                // answers. The caller is not held up any longer.
                pending.stalled = true;
                pending.deferred = pending.deferred.saturating_add(1);
                return;
            }
            pending = self
                .written
                .wait_timeout(pending, remaining)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        if pending.written_ticket < ticket {
            // The background thread is gone. Write what is queued here.
            drop(pending);
            let mut file = lock(&self.file);
            self.write_pending(&mut file, None);
        }
    }

    fn enqueue(&self, pending: &mut Pending, line: String) {
        if pending.queue.len() >= self.capacity {
            pending.dropped = pending.dropped.saturating_add(1);
        } else {
            pending.queue.push_back((line, false));
            self.wake.notify_one();
        }
    }

    /// Writes everything that is queued and asks the operating system to
    /// flush the file to the disk. Waits for the disk as long as it takes.
    pub fn flush(&self) {
        self.queue_summaries(true);
        let mut file = lock(&self.file);
        self.write_pending(&mut file, None);
        let _ = file.sync();
    }

    /// Writes the queue and then `line`, from a panic hook.
    ///
    /// The panic may have happened while this very thread or another one
    /// holds the file, so the lock is only tried for a bounded time, and a
    /// poisoned lock is used anyway. If the file cannot be had in that time
    /// (a stalled disk, or a panic inside the logger itself) the record goes
    /// to a separate crash file and to stderr instead: a panic is never left
    /// unrecorded.
    pub fn write_from_panic_hook(&self, line: &str) {
        // By the clock, not by a number of attempts: a sleep can take much
        // longer than asked on a busy machine.
        let deadline = Instant::now() + PANIC_LOCK_WAIT;
        loop {
            let mut file = match self.file.try_lock() {
                Ok(file) => file,
                Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
                Err(TryLockError::WouldBlock) => {
                    if Instant::now() >= deadline {
                        break;
                    }
                    std::thread::sleep(PANIC_LOCK_RETRY_DELAY);
                    continue;
                }
            };
            self.queue_summaries(true);
            self.write_pending(&mut file, Some(line));
            let _ = file.sync();
            return;
        }
        self.write_crash_file(line);
        // The default hook prints the message; this is the whole record.
        let _ = io::stderr().write_all(line.as_bytes());
    }

    /// Appends `line` to the crash file of this launch, without taking any
    /// lock. Best effort: the disk that kept the log file busy may be the
    /// same one.
    fn write_crash_file(&self, line: &str) {
        // Never write through something planted under the name.
        if self.crash_file.exists() && eligible_metadata(&self.crash_file).is_none() {
            return;
        }
        if let Ok(mut file) = OpenOptions::new()
            .append(true)
            .create(true)
            .open(&self.crash_file)
        {
            let _ = file.write_all(line.as_bytes());
            let _ = file.sync_all();
        }
    }

    /// Writes everything that is queued, stops the background thread and
    /// flushes the file to the disk. Records submitted afterwards are all
    /// written by their caller.
    pub fn shutdown(&self) {
        lock(&self.pending).stopped = true;
        self.wake.notify_all();
        if let Some(worker) = lock(&self.worker).take() {
            let _ = worker.join();
        }
        self.flush();
    }

    /// Writes, in this order: the queued records, notes about dropped and
    /// late records if there were any, and `extra`. The caller holds the
    /// file, so writers cannot interleave. Then tells the callers that wait
    /// for their record.
    fn write_pending(&self, file: &mut RotatingFile, extra: Option<&str>) {
        let batch = {
            let mut pending = lock(&self.pending);
            Batch {
                lines: std::mem::take(&mut pending.queue),
                last_ticket: pending.last_ticket,
                dropped: std::mem::take(&mut pending.dropped),
                dropped_urgent: std::mem::take(&mut pending.dropped_urgent),
                deferred: std::mem::take(&mut pending.deferred),
            }
        };

        let mut notes = Vec::new();
        if batch.deferred > 0 {
            notes.push(self.note(
                "warnings and errors were written late because the log file was busy",
                "deferred",
                batch.deferred,
                "WARN and ERROR",
            ));
        }
        if batch.dropped > 0 {
            notes.push(self.note(
                "records were dropped because the log writer could not keep up",
                "dropped",
                batch.dropped,
                "INFO and lower",
            ));
        }
        if batch.dropped_urgent > 0 {
            notes.push(self.note(
                "warnings and errors were dropped because the log file was busy for too long",
                "dropped",
                batch.dropped_urgent,
                "WARN and ERROR",
            ));
        }

        let lines = batch
            .lines
            .iter()
            .map(|(line, _)| line.as_str())
            .chain(notes.iter().map(String::as_str))
            .chain(extra);
        // A logger has nowhere to report its own failures: a record that
        // cannot be written is lost.
        let _ = file.write_batch(lines.map(str::as_bytes));

        let mut first_errors = lock(&self.first_errors);
        let urgent = batch
            .lines
            .iter()
            .filter(|(_, urgent)| *urgent)
            .map(|(line, _)| line.as_str());
        // `extra` is only ever a warning, an error or a panic.
        for line in urgent.chain(extra) {
            first_errors.write(line);
        }
        drop(first_errors);

        let mut pending = lock(&self.pending);
        pending.written_ticket = pending.written_ticket.max(batch.last_ticket);
        pending.stalled = false;
        drop(pending);
        self.written.notify_all();
    }

    fn note(&self, message: &str, counter: &str, count: u64, levels: &str) -> String {
        let mut fields = Map::new();
        fields.insert(counter.to_owned(), Value::from(count));
        fields.insert("levels".to_owned(), Value::from(levels));
        self.context.format(Record {
            timestamp: OffsetDateTime::now_utc(),
            level: "WARN",
            target: LOG_TARGET,
            message,
            span: None,
            fields,
        })
    }

    fn run_worker(&self) {
        /// Says that the thread is gone, however it ends, so that callers
        /// stop handing it records.
        struct Running<'a>(&'a LogWriter);
        impl Drop for Running<'_> {
            fn drop(&mut self) {
                lock(&self.0.pending).worker_running = false;
                self.0.written.notify_all();
            }
        }
        let _running = Running(self);

        loop {
            {
                let mut pending = lock(&self.pending);
                if pending.queue.is_empty() && !pending.stopped {
                    pending = self
                        .wake
                        .wait_timeout(pending, SUMMARY_INTERVAL)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0;
                }
                if pending.stopped {
                    return;
                }
            }
            self.queue_summaries(false);
            if lock(&self.pending).queue.is_empty() {
                continue;
            }
            let mut file = lock(&self.file);
            self.write_pending(&mut file, None);
        }
    }

    /// Keeps the log file busy for `duration`, as a disk that does not
    /// answer would, and says on `stalled` when it has started.
    #[cfg(test)]
    fn stall(&self, duration: Duration, stalled: &std::sync::mpsc::Sender<()>) {
        let _file = lock(&self.file);
        let _ = stalled.send(());
        std::thread::sleep(duration);
    }
}

/// Locks `mutex`, carrying on with the data if another thread panicked while
/// holding it: losing the log because of a panic would defeat its purpose.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::sync::Arc;

    use serde_json::Value;

    use std::time::{Duration, Instant};

    use super::{LOG_TARGET, LogWriter, URGENT_WAIT};
    use crate::diagnostics::record::LineContext;
    use crate::diagnostics::redact::Redactor;
    use crate::diagnostics::rotate::{Retention, RotatingFile, log_files};

    /// A writer with no background thread, so the tests decide when the
    /// queue is written.
    fn writer(dir: &Path, capacity: usize) -> Arc<LogWriter> {
        let run_id = "5f0c2a9be771";
        let file = RotatingFile::open(dir, Retention::default(), run_id).unwrap();
        let context = Arc::new(LineContext {
            run_id: run_id.to_owned(),
            redactor: Redactor::disabled(),
        });
        LogWriter::new(file, context, capacity, URGENT_WAIT)
    }

    fn lines(dir: &Path) -> Vec<String> {
        log_files(dir)
            .unwrap()
            .iter()
            .filter(|file| !file.name.ends_with("-errors.log"))
            .flat_map(|file| {
                fs::read_to_string(&file.path)
                    .unwrap()
                    .lines()
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    #[test]
    fn an_urgent_record_is_on_disk_when_submit_returns() {
        let dir = tempfile::tempdir().unwrap();
        let writer = writer(dir.path(), 16);

        writer.submit("error\n".to_owned(), true);

        assert_eq!(lines(dir.path()), ["error"]);
    }

    #[test]
    fn queued_records_wait_and_are_written_before_the_next_urgent_one() {
        let dir = tempfile::tempdir().unwrap();
        let writer = writer(dir.path(), 16);

        writer.submit("info 1\n".to_owned(), false);
        writer.submit("info 2\n".to_owned(), false);
        assert!(lines(dir.path()).is_empty());

        writer.submit("error\n".to_owned(), true);
        assert_eq!(lines(dir.path()), ["info 1", "info 2", "error"]);
    }

    #[test]
    fn records_over_the_capacity_are_dropped_counted_and_reported() {
        let dir = tempfile::tempdir().unwrap();
        let writer = writer(dir.path(), 3);

        for index in 0..10 {
            writer.submit(format!("info {index}\n"), false);
        }
        writer.flush();

        let lines = lines(dir.path());
        assert_eq!(lines.len(), 4, "{lines:?}");
        assert_eq!(lines[..3], ["info 0", "info 1", "info 2"]);
        let note: Value = serde_json::from_str(&lines[3]).unwrap();
        assert_eq!(note["level"], "WARN");
        assert_eq!(note["target"], LOG_TARGET);
        assert_eq!(note["fields"]["dropped"], 7);

        // The count starts again once it has been reported.
        writer.submit("info later\n".to_owned(), false);
        writer.flush();
        assert_eq!(super::tests::lines(dir.path()).len(), 5);
    }

    #[test]
    fn urgent_records_are_never_dropped_however_full_the_queue_is() {
        let dir = tempfile::tempdir().unwrap();
        let writer = writer(dir.path(), 2);

        for index in 0..100 {
            writer.submit(format!("info {index}\n"), false);
            writer.submit(format!("error {index}\n"), true);
        }

        let errors = lines(dir.path())
            .iter()
            .filter(|line| line.starts_with("error"))
            .count();
        assert_eq!(errors, 100);
        assert_eq!(lines(dir.path()).last().unwrap(), "error 99");
    }

    #[test]
    fn the_background_thread_writes_queued_records_and_stops_on_shutdown() {
        let dir = tempfile::tempdir().unwrap();
        let writer = writer(dir.path(), 1_000);
        writer.start_worker().unwrap();

        for index in 0..500 {
            writer.submit(format!("info {index}\n"), false);
        }
        writer.shutdown();

        let lines = lines(dir.path());
        assert_eq!(lines.len(), 500);
        assert_eq!(lines[499], "info 499");
    }

    #[test]
    fn records_submitted_after_shutdown_are_written_immediately() {
        let dir = tempfile::tempdir().unwrap();
        let writer = writer(dir.path(), 16);
        writer.start_worker().unwrap();
        writer.shutdown();

        writer.submit("late info\n".to_owned(), false);

        assert_eq!(lines(dir.path()), ["late info"]);
    }

    #[test]
    fn the_panic_hook_path_writes_the_queue_first() {
        let dir = tempfile::tempdir().unwrap();
        let writer = writer(dir.path(), 16);
        writer.submit("info\n".to_owned(), false);

        writer.write_from_panic_hook("panic\n");

        assert_eq!(lines(dir.path()), ["info", "panic"]);
    }

    #[test]
    fn with_the_background_thread_an_urgent_record_is_still_on_disk_on_return() {
        let dir = tempfile::tempdir().unwrap();
        // A wait that a busy test machine cannot exceed: this test is about
        // the record being there, not about how long that may take.
        let run_id = "5f0c2a9be771";
        let file = RotatingFile::open(dir.path(), Retention::default(), run_id).unwrap();
        let context = Arc::new(LineContext {
            run_id: run_id.to_owned(),
            redactor: Redactor::disabled(),
        });
        let writer = LogWriter::new(file, context, 16, Duration::from_secs(60));
        writer.start_worker().unwrap();

        for index in 0..200 {
            writer.submit(format!("info {index}\n"), false);
            writer.submit(format!("error {index}\n"), true);
            // No flush: the record has to be there already.
            assert_eq!(lines(dir.path()).last().unwrap(), &format!("error {index}"));
        }
        writer.shutdown();
    }

    #[test]
    fn a_stalled_disk_does_not_hold_up_the_caller_and_the_record_is_written_late() {
        let dir = tempfile::tempdir().unwrap();
        let writer = writer(dir.path(), 16);
        writer.start_worker().unwrap();

        std::thread::scope(|scope| {
            let (stalled, is_stalled) = std::sync::mpsc::channel();
            let (release, released) = std::sync::mpsc::channel::<()>();
            let writer = &writer;
            scope.spawn(move || {
                // The disk does not answer until the test says so. That the
                // calls below return at all is what is being tested.
                let _file = super::lock(&writer.file);
                stalled.send(()).unwrap();
                let _ = released.recv();
            });
            is_stalled.recv().unwrap();

            // The first caller waits for the bounded time, then goes on.
            let start = Instant::now();
            writer.submit("error 1\n".to_owned(), true);
            let first = start.elapsed();
            assert!(first >= URGENT_WAIT, "returned after {first:?}");

            // While the disk is known to be stalled, nobody waits at all:
            // 49 waits would take 4.9 s.
            let start = Instant::now();
            for index in 2..=50 {
                writer.submit(format!("error {index}\n"), true);
            }
            let rest = start.elapsed();
            assert!(rest < URGENT_WAIT * 20, "49 more errors took {rest:?}");

            // Nothing was written while the disk was stalled.
            release.send(()).unwrap();
        });

        // The disk is back: everything is written, in order, and counted.
        writer.shutdown();
        let lines = lines(dir.path());
        let errors: Vec<&String> = lines
            .iter()
            .filter(|line| line.starts_with("error"))
            .collect();
        assert_eq!(errors.len(), 50);
        assert_eq!(errors[0], "error 1");
        assert_eq!(errors[49], "error 50");
        let deferred: u64 = lines
            .iter()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|note| note["target"] == LOG_TARGET)
            .map(|note| note["fields"]["deferred"].as_u64().unwrap())
            .sum();
        assert_eq!(deferred, 50);
    }

    #[test]
    fn waiting_resumes_once_the_disk_answers_again() {
        let dir = tempfile::tempdir().unwrap();
        let writer = writer(dir.path(), 16);
        writer.start_worker().unwrap();

        std::thread::scope(|scope| {
            let (stalled, is_stalled) = std::sync::mpsc::channel();
            let writer = &writer;
            scope.spawn(move || writer.stall(URGENT_WAIT * 3, &stalled));
            is_stalled.recv().unwrap();
            writer.submit("late\n".to_owned(), true);
        });
        // Let the background thread write what was deferred.
        writer.flush();

        writer.submit("on time\n".to_owned(), true);
        assert_eq!(lines(dir.path()).last().unwrap(), "on time");
        writer.shutdown();
    }

    #[test]
    fn urgent_records_are_bounded_too_when_the_disk_never_answers() {
        let dir = tempfile::tempdir().unwrap();
        let writer = writer(dir.path(), 2);
        writer.start_worker().unwrap();

        std::thread::scope(|scope| {
            let (stalled, is_stalled) = std::sync::mpsc::channel();
            let writer = &writer;
            scope.spawn(move || writer.stall(URGENT_WAIT * 4, &stalled));
            is_stalled.recv().unwrap();
            for index in 0..100 {
                writer.submit(format!("error {index}\n"), true);
            }
        });
        writer.shutdown();

        let lines = lines(dir.path());
        let written = lines
            .iter()
            .filter(|line| line.starts_with("error"))
            .count() as u64;
        let dropped: u64 = lines
            .iter()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|note| note["fields"]["levels"] == "WARN and ERROR")
            .filter_map(|note| note["fields"]["dropped"].as_u64())
            .sum();
        assert!(dropped > 0);
        assert_eq!(written + dropped, 100);
    }

    #[test]
    fn a_panic_record_goes_to_the_crash_file_when_the_log_file_is_busy() {
        let dir = tempfile::tempdir().unwrap();
        let writer = writer(dir.path(), 16);

        std::thread::scope(|scope| {
            let (stalled, is_stalled) = std::sync::mpsc::channel();
            let (release, released) = std::sync::mpsc::channel::<()>();
            let writer = &writer;
            scope.spawn(move || {
                // Holds the log file until the test has looked at the disk.
                let _file = super::lock(&writer.file);
                stalled.send(()).unwrap();
                let _ = released.recv();
            });
            is_stalled.recv().unwrap();

            // Returns although the log file never becomes free...
            writer.write_from_panic_hook("the panic\n");

            // ...and the record is on disk while the file is still held.
            let crash_files: Vec<_> = log_files(dir.path())
                .unwrap()
                .into_iter()
                .filter(|file| file.name.ends_with("-crash.log"))
                .collect();
            assert_eq!(crash_files.len(), 1);
            assert_eq!(
                fs::read_to_string(&crash_files[0].path).unwrap(),
                "the panic\n"
            );
            release.send(()).unwrap();
        });
    }

    fn first_errors(dir: &Path) -> Vec<String> {
        log_files(dir)
            .unwrap()
            .iter()
            .filter(|file| file.name.ends_with("-errors.log"))
            .flat_map(|file| {
                fs::read_to_string(&file.path)
                    .unwrap()
                    .lines()
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    #[test]
    fn the_first_warnings_and_errors_are_copied_to_a_file_of_their_own() {
        let dir = tempfile::tempdir().unwrap();
        let writer = writer(dir.path(), 16);

        writer.submit("info\n".to_owned(), false);
        assert!(
            first_errors(dir.path()).is_empty(),
            "no file without an error"
        );
        writer.submit("error 0\n".to_owned(), true);
        for index in 1..(super::FIRST_ERRORS_RECORDS * 2) {
            writer.submit(format!("error {index}\n"), true);
        }
        writer.write_from_panic_hook("panic\n");

        let copied = first_errors(dir.path());
        assert_eq!(copied.len(), super::FIRST_ERRORS_RECORDS as usize);
        assert_eq!(copied[0], "error 0");
        assert!(!copied.contains(&"info".to_owned()));
    }

    #[test]
    fn the_first_errors_survive_when_the_log_itself_has_rotated_away() {
        let dir = tempfile::tempdir().unwrap();
        let run_id = "5f0c2a9be771";
        let limits = Retention {
            max_file_bytes: 1_000,
            max_total_bytes: 4_000,
            min_files_kept: 0,
            ..Retention::default()
        };
        let file = RotatingFile::open(dir.path(), limits, run_id).unwrap();
        let context = Arc::new(LineContext {
            run_id: run_id.to_owned(),
            redactor: Redactor::disabled(),
        });
        let writer = LogWriter::new(file, context, 16, URGENT_WAIT);

        writer.submit("the error that started it\n".to_owned(), true);
        for index in 0..5_000 {
            writer.submit(format!("a later, different error {index}\n"), true);
        }

        let main_log: Vec<String> = log_files(dir.path())
            .unwrap()
            .iter()
            .filter(|file| !file.name.ends_with("-errors.log"))
            .map(|file| fs::read_to_string(&file.path).unwrap())
            .collect();
        assert!(main_log.iter().all(|text| !text.contains("started it")));
        assert_eq!(first_errors(dir.path())[0], "the error that started it");
    }
}
