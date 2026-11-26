//! Recording panics.
//!
//! The hook writes the record straight to the log file, after whatever is
//! still queued, and flushes the file, so the record is on disk even when the
//! process dies right after. It then calls the hook that was installed
//! before, which keeps the default behaviour (the message on stderr, and
//! whatever the panic strategy does next).

use std::backtrace::Backtrace;
use std::cell::RefCell;
use std::panic::PanicHookInfo;
use std::sync::Arc;

use serde_json::{Map, Value};
use time::OffsetDateTime;

use super::record::{LineContext, Record};
use super::writer::LogWriter;

/// Target of the records written by the panic hook.
pub const PANIC_TARGET: &str = "termx::panic";

thread_local! {
    /// What the code running on this thread is doing, for the panic record.
    static CONTEXT: RefCell<Vec<(&'static str, String)>> = const { RefCell::new(Vec::new()) };
}

/// Runs `body` and, if it panics, adds the field `key` with `value` to the
/// panic record.
///
/// This is how a panic record says what was going on, such as which command
/// was running, so that one record tells the whole story instead of a second
/// one being logged where the panic is caught. Contexts nest; an inner one
/// with the same key wins.
///
/// # Examples
///
/// ```
/// use termx_core::diagnostics::with_panic_context;
///
/// let answer = with_panic_context("command", "resize", || 42);
/// assert_eq!(answer, 42);
/// ```
pub fn with_panic_context<T>(key: &'static str, value: &str, body: impl FnOnce() -> T) -> T {
    /// Removes the entry again, also when `body` unwinds.
    struct Entry;
    impl Drop for Entry {
        fn drop(&mut self) {
            let _ = CONTEXT.try_with(|context| {
                if let Ok(mut context) = context.try_borrow_mut() {
                    context.pop();
                }
            });
        }
    }

    let _ = CONTEXT.try_with(|context| {
        if let Ok(mut context) = context.try_borrow_mut() {
            context.push((key, value.to_owned()));
        }
    });
    let _entry = Entry;
    body()
}

/// What [`with_panic_context`] has set on the current thread, outermost
/// first.
#[must_use]
pub fn panic_context() -> Vec<(&'static str, String)> {
    CONTEXT
        .try_with(|context| context.try_borrow().map(|context| context.clone()).ok())
        .ok()
        .flatten()
        .unwrap_or_default()
}

pub(crate) fn install(writer: Arc<LogWriter>, context: Arc<LineContext>) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let line = context.format(record(info));
        writer.write_from_panic_hook(&line);
        previous(info);
    }));
}

fn record(info: &PanicHookInfo<'_>) -> Record<'static> {
    let message = info
        .payload_as_str()
        .unwrap_or("panic with a non-string payload");

    let mut fields = Map::new();
    // First, so that the fields of the record itself cannot be overwritten.
    for (key, value) in panic_context() {
        fields.insert(key.to_owned(), Value::from(value));
    }
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
