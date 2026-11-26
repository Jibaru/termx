//! The on-disk format: one JSON object per line.
//!
//! ```json
//! {"ts":"2026-11-26T19:04:11.482Z","level":"ERROR","run":"5f0c2a9be771","target":"termx_desktop::commands","msg":"command failed","fields":{"command":"export_diagnostics","error":"permission denied"}}
//! ```
//!
//! Keys always appear in this order. `span` is only present for events
//! recorded inside a span, and comes right before `fields`.

use std::fmt::Write as _;

use serde_json::{Map, Value};
use time::OffsetDateTime;

use super::redact::Redactor;

/// Longest string value kept in a record, in bytes. Longer values are cut and
/// marked, so that a single record cannot grow without bound.
pub const MAX_VALUE_BYTES: usize = 16 * 1024;

/// Marker appended to a value that was cut at [`MAX_VALUE_BYTES`].
pub const TRUNCATION_MARKER: &str = "…[truncated]";

/// What every line of one launch has in common.
#[derive(Debug, Clone)]
pub(crate) struct LineContext {
    /// Identifier of this launch of the application.
    pub run_id: String,
    pub redactor: Redactor,
}

/// One record, before it is written.
#[derive(Debug)]
pub(crate) struct Record<'a> {
    pub timestamp: OffsetDateTime,
    pub level: &'a str,
    pub target: &'a str,
    pub message: &'a str,
    /// Names of the enclosing spans, outermost first.
    pub span: Option<&'a str>,
    pub fields: Map<String, Value>,
}

impl LineContext {
    /// Serialises `record` as one line, ending in a line feed.
    ///
    /// Every string is redacted and capped here, so nothing reaches the file
    /// without passing through the [`Redactor`].
    pub fn format(&self, record: Record<'_>) -> String {
        let mut line = String::with_capacity(256);
        line.push_str("{\"ts\":");
        push_json_str(&mut line, &rfc3339_millis(record.timestamp));
        line.push_str(",\"level\":");
        push_json_str(&mut line, record.level);
        line.push_str(",\"run\":");
        push_json_str(&mut line, &self.run_id);
        line.push_str(",\"target\":");
        push_json_str(&mut line, record.target);
        line.push_str(",\"msg\":");
        push_json_str(&mut line, &self.clean(record.message));
        if let Some(span) = record.span {
            line.push_str(",\"span\":");
            push_json_str(&mut line, span);
        }
        line.push_str(",\"fields\":");
        let fields: Map<String, Value> = record
            .fields
            .into_iter()
            .map(|(key, value)| (key, self.clean_value(value)))
            .collect();
        line.push_str(&Value::Object(fields).to_string());
        line.push_str("}\n");
        line
    }

    fn clean(&self, value: &str) -> String {
        truncate(self.redactor.text(value).into_owned())
    }

    fn clean_value(&self, value: Value) -> Value {
        match value {
            Value::String(text) => Value::String(self.clean(&text)),
            Value::Array(items) => Value::Array(
                items
                    .into_iter()
                    .map(|item| self.clean_value(item))
                    .collect(),
            ),
            Value::Object(entries) => Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, self.clean_value(value)))
                    .collect(),
            ),
            other => other,
        }
    }
}

fn push_json_str(line: &mut String, value: &str) {
    line.push_str(&Value::from(value).to_string());
}

/// Cuts `value` at [`MAX_VALUE_BYTES`], on a character boundary.
fn truncate(mut value: String) -> String {
    if value.len() <= MAX_VALUE_BYTES {
        return value;
    }
    let mut end = MAX_VALUE_BYTES;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
    value.push_str(TRUNCATION_MARKER);
    value
}

/// Formats a UTC timestamp as `YYYY-MM-DDTHH:MM:SS.mmmZ`.
pub(crate) fn rfc3339_millis(timestamp: OffsetDateTime) -> String {
    let timestamp = timestamp.to_offset(time::UtcOffset::UTC);
    let mut text = String::with_capacity(24);
    // Writing to a `String` cannot fail.
    let _ = write!(
        text,
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        timestamp.year(),
        u8::from(timestamp.month()),
        timestamp.day(),
        timestamp.hour(),
        timestamp.minute(),
        timestamp.second(),
        timestamp.millisecond(),
    );
    text
}

/// Formats a UTC timestamp as `YYYYMMDDTHHMMSSZ`, for use in file names.
pub(crate) fn compact_timestamp(timestamp: OffsetDateTime) -> String {
    let timestamp = timestamp.to_offset(time::UtcOffset::UTC);
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        timestamp.year(),
        u8::from(timestamp.month()),
        timestamp.day(),
        timestamp.hour(),
        timestamp.minute(),
        timestamp.second(),
    )
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::{Map, Value, json};
    use time::OffsetDateTime;

    use super::{LineContext, MAX_VALUE_BYTES, Record, TRUNCATION_MARKER};
    use crate::diagnostics::redact::Redactor;

    fn context() -> LineContext {
        LineContext {
            run_id: "5f0c2a9be771".to_owned(),
            redactor: Redactor::with_case_sensitivity(Some(Path::new("/home/ana")), true),
        }
    }

    fn timestamp() -> OffsetDateTime {
        // 2026-11-26T19:04:11.482Z
        OffsetDateTime::from_unix_timestamp_nanos(1_795_719_851_482_000_000).unwrap()
    }

    fn fields(value: Value) -> Map<String, Value> {
        match value {
            Value::Object(map) => map,
            other => panic!("expected an object, got {other}"),
        }
    }

    #[test]
    fn a_record_is_one_json_line_with_the_documented_keys_in_order() {
        let line = context().format(Record {
            timestamp: timestamp(),
            level: "ERROR",
            target: "termx_desktop::commands",
            message: "command failed",
            span: None,
            fields: fields(json!({"command": "export_diagnostics", "attempt": 2})),
        });

        assert_eq!(
            line,
            concat!(
                r#"{"ts":"2026-11-26T19:04:11.482Z","level":"ERROR","run":"5f0c2a9be771","#,
                r#""target":"termx_desktop::commands","msg":"command failed","#,
                r#""fields":{"attempt":2,"command":"export_diagnostics"}}"#,
                "\n"
            )
        );
    }

    #[test]
    fn the_span_comes_before_the_fields() {
        let line = context().format(Record {
            timestamp: timestamp(),
            level: "INFO",
            target: "t",
            message: "m",
            span: Some("outer>inner"),
            fields: Map::new(),
        });

        assert!(line.ends_with("\"msg\":\"m\",\"span\":\"outer>inner\",\"fields\":{}}\n"));
    }

    #[test]
    fn line_breaks_and_quotes_are_escaped_so_a_record_stays_on_one_line() {
        let line = context().format(Record {
            timestamp: timestamp(),
            level: "WARN",
            target: "t",
            message: "first\nsecond \"quoted\"",
            span: None,
            fields: fields(json!({"detail": "a\r\nb"})),
        });

        assert_eq!(line.matches('\n').count(), 1);
        let parsed: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(parsed["msg"], "first\nsecond \"quoted\"");
        assert_eq!(parsed["fields"]["detail"], "a\r\nb");
    }

    #[test]
    fn the_home_directory_is_redacted_in_the_message_and_in_every_field() {
        let line = context().format(Record {
            timestamp: timestamp(),
            level: "ERROR",
            target: "t",
            message: "cannot open /home/ana/notes.txt",
            span: None,
            fields: fields(json!({"path": "/home/ana/notes.txt", "nested": ["/home/ana"]})),
        });

        assert!(!line.contains("/home/ana"), "{line}");
        let parsed: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(parsed["msg"], "cannot open ~/notes.txt");
        assert_eq!(parsed["fields"]["path"], "~/notes.txt");
        assert_eq!(parsed["fields"]["nested"][0], "~");
    }

    #[test]
    fn oversized_values_are_cut_on_a_character_boundary() {
        let long = "é".repeat(MAX_VALUE_BYTES);
        let line = context().format(Record {
            timestamp: timestamp(),
            level: "INFO",
            target: "t",
            message: &long,
            span: None,
            fields: fields(json!({ "blob": long })),
        });

        let parsed: Value = serde_json::from_str(&line).unwrap();
        for value in [&parsed["msg"], &parsed["fields"]["blob"]] {
            let value = value.as_str().unwrap();
            assert!(value.ends_with(TRUNCATION_MARKER));
            assert!(value.len() <= MAX_VALUE_BYTES + TRUNCATION_MARKER.len());
        }
    }
}
