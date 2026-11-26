//! Recording panics.
//!
//! The hook writes the record straight to the log file, after whatever is
//! still queued, and flushes the file, so the record is on disk even when the
//! process dies right after. It then calls the hook that was installed
//! before, which keeps the default behaviour (the message on stderr, and
//! whatever the panic strategy does next).

use std::backtrace::Backtrace;
use std::panic::PanicHookInfo;
use std::sync::Arc;

use serde_json::{Map, Value};
use time::OffsetDateTime;

use super::record::{LineContext, Record};
use super::writer::LogWriter;

/// Target of the records written by the panic hook.
pub const PANIC_TARGET: &str = "termx::panic";

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
