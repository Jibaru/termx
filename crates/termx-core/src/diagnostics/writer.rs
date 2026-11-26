//! Getting records onto the disk.
//!
//! The record that explains a crash is the one written right before it, so
//! what matters most is that a `WARN` or `ERROR` record cannot be lost:
//!
//! - **`WARN` and `ERROR` records are written synchronously.** The logging
//!   call returns after the operating system has the bytes, so the record
//!   survives an abort, `process::exit` or a kill right after it. Everything
//!   still queued is written first, to keep the file in order.
//! - **`INFO`, `DEBUG` and `TRACE` records are queued** for a background
//!   thread, so frequent logging does not wait for the disk. The queue is
//!   bounded. When it is full the record is dropped and counted, and a
//!   record with the number of drops is written as soon as the writer
//!   catches up. Only these low levels can ever be dropped.
//! - The panic hook and [`LogWriter::shutdown`] write out the queue and ask
//!   the operating system to flush the file to the disk.

use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, TryLockError};
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::{Map, Value};
use time::OffsetDateTime;

use super::record::{LineContext, Record};
use super::rotate::RotatingFile;

/// Target of the records the logger writes about itself.
pub const LOG_TARGET: &str = "termx::log";

/// Records that can wait for the background thread before new ones are
/// dropped.
pub(crate) const QUEUE_CAPACITY: usize = 8 * 1024;

/// How long the panic hook waits for the log file when another thread is
/// writing.
const PANIC_LOCK_ATTEMPTS: u32 = 100;
const PANIC_LOCK_RETRY_DELAY: Duration = Duration::from_millis(5);

#[derive(Debug, Default)]
struct Pending {
    queue: VecDeque<String>,
    /// Records dropped since the last one that was written.
    dropped: u64,
    /// Set by `shutdown`: from then on every record is written synchronously.
    stopped: bool,
}

/// Writes formatted lines to the log file.
#[derive(Debug)]
pub(crate) struct LogWriter {
    file: Mutex<RotatingFile>,
    pending: Mutex<Pending>,
    /// Signalled when the queue gets its first record, and on shutdown.
    wake: Condvar,
    context: Arc<LineContext>,
    capacity: usize,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl LogWriter {
    /// A writer without a background thread: queued records are written by
    /// the next synchronous write or [`LogWriter::flush`].
    pub fn new(file: RotatingFile, context: Arc<LineContext>, capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            file: Mutex::new(file),
            pending: Mutex::new(Pending::default()),
            wake: Condvar::new(),
            context,
            capacity,
            worker: Mutex::new(None),
        })
    }

    /// Starts the background thread that writes the queued records.
    pub fn start_worker(self: &Arc<Self>) -> io::Result<()> {
        let writer = Arc::clone(self);
        let handle = std::thread::Builder::new()
            .name("termx-log".to_owned())
            .spawn(move || writer.run_worker())?;
        *lock(&self.worker) = Some(handle);
        Ok(())
    }

    /// Hands over one finished line. `urgent` lines are on their way to the
    /// disk when this returns; the others may be queued, or dropped when the
    /// queue is full.
    pub fn submit(&self, line: String, urgent: bool) {
        if !urgent {
            let mut pending = lock(&self.pending);
            if !pending.stopped {
                if pending.queue.len() >= self.capacity {
                    pending.dropped = pending.dropped.saturating_add(1);
                } else {
                    pending.queue.push_back(line);
                    if pending.queue.len() == 1 {
                        self.wake.notify_one();
                    }
                }
                return;
            }
        }
        let mut file = lock(&self.file);
        self.write_pending(&mut file, Some(&line));
    }

    /// Writes everything that is queued and asks the operating system to
    /// flush the file to the disk.
    pub fn flush(&self) {
        let mut file = lock(&self.file);
        self.write_pending(&mut file, None);
        let _ = file.sync();
    }

    /// Writes the queue and then `line`, from a panic hook.
    ///
    /// The panic may have happened while this very thread or another one
    /// holds the file, so the lock is only tried for a bounded time, and a
    /// poisoned lock is used anyway.
    pub fn write_from_panic_hook(&self, line: &str) {
        for _ in 0..PANIC_LOCK_ATTEMPTS {
            let mut file = match self.file.try_lock() {
                Ok(file) => file,
                Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
                Err(TryLockError::WouldBlock) => {
                    std::thread::sleep(PANIC_LOCK_RETRY_DELAY);
                    continue;
                }
            };
            self.write_pending(&mut file, Some(line));
            let _ = file.sync();
            return;
        }
    }

    /// Writes everything that is queued, stops the background thread and
    /// flushes the file to the disk. Records submitted afterwards are all
    /// written synchronously.
    pub fn shutdown(&self) {
        lock(&self.pending).stopped = true;
        self.wake.notify_all();
        if let Some(worker) = lock(&self.worker).take() {
            let _ = worker.join();
        }
        self.flush();
    }

    /// Writes, in this order: the queued records, a note about dropped
    /// records if there were any, and `extra`. The caller holds the file, so
    /// writers cannot interleave.
    fn write_pending(&self, file: &mut RotatingFile, extra: Option<&str>) {
        let (queue, dropped) = {
            let mut pending = lock(&self.pending);
            (
                std::mem::take(&mut pending.queue),
                std::mem::take(&mut pending.dropped),
            )
        };
        // A logger has nowhere to report its own failures: a record that
        // cannot be written is lost.
        for line in &queue {
            let _ = file.write_record(line.as_bytes());
        }
        if dropped > 0 {
            let _ = file.write_record(self.dropped_record(dropped).as_bytes());
        }
        if let Some(line) = extra {
            let _ = file.write_record(line.as_bytes());
        }
    }

    fn dropped_record(&self, dropped: u64) -> String {
        let mut fields = Map::new();
        fields.insert("dropped".to_owned(), Value::from(dropped));
        fields.insert("levels".to_owned(), Value::from("INFO and lower"));
        self.context.format(Record {
            timestamp: OffsetDateTime::now_utc(),
            level: "WARN",
            target: LOG_TARGET,
            message: "records were dropped because the log writer could not keep up",
            span: None,
            fields,
        })
    }

    fn run_worker(&self) {
        loop {
            {
                let mut pending = lock(&self.pending);
                while pending.queue.is_empty() && !pending.stopped {
                    pending = self
                        .wake
                        .wait(pending)
                        .unwrap_or_else(PoisonError::into_inner);
                }
                if pending.stopped {
                    return;
                }
            }
            let mut file = lock(&self.file);
            self.write_pending(&mut file, None);
        }
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

    use super::{LOG_TARGET, LogWriter};
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
        LogWriter::new(file, context, capacity)
    }

    fn lines(dir: &Path) -> Vec<String> {
        log_files(dir)
            .unwrap()
            .iter()
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
}
