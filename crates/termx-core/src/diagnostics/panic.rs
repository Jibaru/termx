//! Recording panics.
//!
//! The hook writes the record straight to the log file instead of going
//! through the background writer, so the record is on disk even when the
//! process dies right after. It then calls the hook that was installed
//! before, which keeps the default behaviour (the message on stderr, and
//! whatever the panic strategy does next).

use std::backtrace::Backtrace;
use std::panic::PanicHookInfo;
use std::sync::{Arc, TryLockError};
use std::time::Duration;

use serde_json::{Map, Value};
use time::OffsetDateTime;

use super::SharedFile;
use super::record::{LineContext, Record};

/// Target of the records written by the panic hook.
pub const PANIC_TARGET: &str = "termx::panic";

/// How long the hook waits for the log file when another thread is writing.
const LOCK_ATTEMPTS: u32 = 100;
const LOCK_RETRY_DELAY: Duration = Duration::from_millis(5);

pub(crate) fn install(file: SharedFile, context: Arc<LineContext>) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let line = context.format(record(info));
        write_now(&file, line.as_bytes());
        previous(info);
    }));
}

fn record(info: &PanicHookInfo<'_>) -> Record<'static> {
    let message = info
        .payload_as_str()
        .unwrap_or("panic with a non-string payload");

    let mut fields = Map::new();
    fields.insert("message".to_owned(), Value::from(message));
    if let Some(location) = info.location() {
        fields.insert("location".to_owned(), Value::from(location.to_string()));
    }
    let thread = std::thread::current();
    fields.insert(
        "thread".to_owned(),
        Value::from(thread.name().unwrap_or("<unnamed>")),
    );
    fields.insert(
        "backtrace".to_owned(),
        Value::from(Backtrace::force_capture().to_string()),
    );

    Record {
        timestamp: OffsetDateTime::now_utc(),
        level: "ERROR",
        target: PANIC_TARGET,
        message: "panic",
        span: None,
        fields,
    }
}

/// Writes `line` to the log file from the panicking thread.
///
/// The panic may have happened while this very thread or another one holds
/// the file, so the lock is only tried for a bounded time, and a poisoned
/// lock is used anyway.
fn write_now(file: &SharedFile, line: &[u8]) {
    for _ in 0..LOCK_ATTEMPTS {
        let mut guard = match file.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => {
                std::thread::sleep(LOCK_RETRY_DELAY);
                continue;
            }
        };
        let _ = guard.write_record(line);
        let _ = guard.flush();
        return;
    }
}
