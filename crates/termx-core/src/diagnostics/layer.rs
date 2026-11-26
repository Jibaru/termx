//! The `tracing` layer that turns events into log lines.

use std::fmt;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;
use std::time::Instant;

use serde_json::{Map, Value};
use time::OffsetDateTime;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_log::NormalizeEvent;
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::registry::LookupSpan;

use super::normalise::message_key;
use super::record::{LineContext, Record};
use super::suppress::Verdict;
use super::writer::LogWriter;

/// Turns every event into one JSON line and hands it to the writer.
pub(crate) struct JsonLinesLayer {
    writer: Arc<LogWriter>,
    context: Arc<LineContext>,
}

impl JsonLinesLayer {
    pub fn new(writer: Arc<LogWriter>, context: Arc<LineContext>) -> Self {
        Self { writer, context }
    }
}

impl<S> Layer<S> for JsonLinesLayer
where
    S: Subscriber + for<'span> LookupSpan<'span>,
{
    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        // Records that come from the `log` crate carry their real target in
        // fields; `normalized_metadata` recovers it.
        let normalized = event.normalized_metadata();
        let metadata = normalized.as_ref().unwrap_or_else(|| event.metadata());

        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);

        // Warnings and errors are waited for until they are written, and
        // their repeats are counted instead of written; see the `writer` and
        // `suppress` modules.
        let urgent = *metadata.level() <= Level::WARN;
        if urgent {
            let mut hasher = DefaultHasher::new();
            metadata.callsite().hash(&mut hasher);
            message_key(&visitor.message).hash(&mut hasher);
            let verdict = self.writer.suppressor().check(
                hasher.finish(),
                metadata.level().as_str(),
                metadata.target(),
                &visitor.message,
                Instant::now(),
            );
            match verdict {
                Verdict::Suppress => return,
                Verdict::Write(Some(summary)) => self.writer.submit_summary(&summary),
                Verdict::Write(None) => {}
            }
        }

        let span = ctx.event_scope(event).map(|scope| {
            scope
                .from_root()
                .map(|span| span.name())
                .collect::<Vec<_>>()
                .join(">")
        });

        let line = self.context.format(Record {
            timestamp: OffsetDateTime::now_utc(),
            level: metadata.level().as_str(),
            target: metadata.target(),
            message: &visitor.message,
            span: span.as_deref(),
            fields: visitor.fields,
        });
        self.writer.submit(line, urgent);
    }
}

/// Collects the fields of an event.
#[derive(Default)]
struct FieldVisitor {
    message: String,
    fields: Map<String, Value>,
}

impl FieldVisitor {
    fn insert(&mut self, field: &Field, value: Value) {
        let name = field.name();
        if name == "message" {
            self.message = match value {
                Value::String(text) => text,
                other => other.to_string(),
            };
        } else if !name.starts_with("log.") {
            // `log.*` fields are bookkeeping added by tracing-log.
            self.fields.insert(name.to_owned(), value);
        }
    }
}

impl Visit for FieldVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.insert(field, Value::String(format!("{value:?}")));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.insert(field, Value::String(value.to_owned()));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.insert(field, Value::Bool(value));
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.insert(field, Value::from(value));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.insert(field, Value::from(value));
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        // Not-a-number and the infinities have no JSON representation.
        let value = serde_json::Number::from_f64(value)
            .map_or_else(|| Value::String(value.to_string()), Value::Number);
        self.insert(field, value);
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        let mut text = value.to_string();
        let mut source = value.source();
        while let Some(cause) = source {
            text.push_str(": ");
            text.push_str(&cause.to_string());
            source = cause.source();
        }
        self.insert(field, Value::String(text));
    }
}
