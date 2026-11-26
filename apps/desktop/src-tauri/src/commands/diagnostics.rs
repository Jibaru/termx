//! Commands for diagnostics: receiving frontend errors, and getting the logs
//! out of the application.

use std::path::{Path, PathBuf};
use std::sync::PoisonError;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};
use termx_core::diagnostics::{Decision, Diagnostics, tracing};

use super::error::{CommandResult, logged};
use crate::logging::DiagnosticsState;

/// Target of the records that come from the frontend.
pub const FRONTEND_TARGET: &str = "termx::frontend";

/// Longest `source` kept from a frontend event, in characters.
const MAX_SOURCE_CHARS: usize = 64;
/// Longest `message` kept from a frontend event, in characters.
const MAX_MESSAGE_CHARS: usize = 2_000;
/// Longest stack trace kept from a frontend event, in characters.
const MAX_STACK_CHARS: usize = 8_000;

/// Severity of a frontend event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FrontendLevel {
    /// Something failed.
    Error,
    /// Something unexpected that the interface recovered from.
    Warn,
    /// A lifecycle event worth recording.
    Info,
}

/// Something the frontend wants recorded in the log.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrontendEvent {
    /// Severity.
    pub level: FrontendLevel,
    /// Where it was caught: `window.onerror`, `unhandledrejection`, `react`,
    /// `ipc`, or a name chosen by the caller.
    pub source: String,
    /// Description of what happened.
    pub message: String,
    /// JavaScript stack trace.
    #[serde(default)]
    pub stack: Option<String>,
    /// React component stack, for errors caught by an error boundary.
    #[serde(default)]
    pub component_stack: Option<String>,
    /// Name of the Tauri command, for failed IPC calls.
    #[serde(default)]
    pub command: Option<String>,
}

/// Where a diagnostics bundle was written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExportedBundle {
    /// Full path of the zip file.
    pub path: String,
}

/// Records an event reported by the frontend.
///
/// This is the single entry point of the frontend into the log. It is rate
/// limited, and it cannot fail: a logging problem must never cause another
/// error in the interface.
#[tauri::command]
pub fn log_frontend_event(state: State<'_, DiagnosticsState>, event: FrontendEvent) {
    record_frontend_event(&state, event);
}

/// Opens the folder that holds the log files in the file manager.
///
/// # Errors
///
/// Fails when logging is not running or the folder cannot be opened.
#[tauri::command]
pub fn open_log_folder(state: State<'_, DiagnosticsState>) -> CommandResult<()> {
    logged(
        "open_log_folder",
        running(&state).and_then(|diagnostics| show_in_file_manager(diagnostics.log_dir())),
    )
}

/// Writes a diagnostics bundle (a zip with the log files and
/// `system-info.json`) to the Downloads folder, opens that folder in the file
/// manager and returns the path of the bundle.
///
/// The destination is deliberately not an argument: the webview must not be
/// able to make the application create files at a path of its choosing.
///
/// # Errors
///
/// Fails when logging is not running, there is no Downloads folder, or the
/// bundle cannot be written.
#[tauri::command(async)]
pub fn export_diagnostics(
    app: AppHandle,
    state: State<'_, DiagnosticsState>,
) -> CommandResult<ExportedBundle> {
    let downloads = app.path().download_dir().ok();
    let bundle = logged(
        "export_diagnostics",
        export_bundle(&state, downloads.as_deref()),
    )?;

    // Showing the folder is a convenience; the path is returned either way.
    if let Some(Err(error)) = bundle.parent().map(show_in_file_manager) {
        tracing::warn!(error = %error, "could not show the diagnostics bundle in the file manager");
    }
    Ok(ExportedBundle {
        path: bundle.to_string_lossy().into_owned(),
    })
}

/// Opens `dir` in the file manager without waiting for it to close.
fn show_in_file_manager(dir: &Path) -> Result<(), String> {
    open::that_detached(dir).map_err(|error| error.to_string())
}

fn running(state: &DiagnosticsState) -> Result<&Diagnostics, String> {
    state
        .diagnostics()
        .ok_or_else(|| "logging is not running, so there are no logs".to_owned())
}

fn export_bundle(state: &DiagnosticsState, destination: Option<&Path>) -> Result<PathBuf, String> {
    let diagnostics = running(state)?;
    let destination = destination.ok_or("the Downloads folder could not be found")?;
    let bundle = diagnostics
        .export_bundle(destination)
        .map_err(|error| error.to_string())?;
    tracing::info!(path = %bundle.display(), "diagnostics bundle exported");
    Ok(bundle)
}

/// Writes `event` to the log unless the frontend is over its rate limit.
/// Returns whether it was written.
fn record_frontend_event(state: &DiagnosticsState, event: FrontendEvent) -> bool {
    let decision = state
        .frontend_limiter
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .check();
    let Decision::Allowed { suppressed } = decision else {
        return false;
    };
    if suppressed > 0 {
        tracing::warn!(
            target: FRONTEND_TARGET,
            suppressed,
            "frontend records were dropped by the rate limit"
        );
    }

    let source = cut(&event.source, MAX_SOURCE_CHARS);
    let message = cut(&event.message, MAX_MESSAGE_CHARS);
    let stack = event
        .stack
        .as_deref()
        .map(|stack| cut(stack, MAX_STACK_CHARS));
    let component_stack = event
        .component_stack
        .as_deref()
        .map(|stack| cut(stack, MAX_STACK_CHARS));
    let command = event
        .command
        .as_deref()
        .map(|command| cut(command, MAX_SOURCE_CHARS));

    // The level of a `tracing` event is fixed at the call site.
    macro_rules! emit {
        ($macro:ident) => {
            tracing::$macro!(
                target: FRONTEND_TARGET,
                source,
                stack,
                component_stack,
                command,
                "{message}"
            )
        };
    }
    match event.level {
        FrontendLevel::Error => emit!(error),
        FrontendLevel::Warn => emit!(warn),
        FrontendLevel::Info => emit!(info),
    }
    true
}

/// The first `max_chars` characters of `text`.
fn cut(text: &str, max_chars: usize) -> &str {
    match text.char_indices().nth(max_chars) {
        Some((end, _)) => &text[..end],
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use termx_core::diagnostics::tracing;

    use super::{
        FRONTEND_TARGET, FrontendEvent, FrontendLevel, MAX_MESSAGE_CHARS, MAX_STACK_CHARS, cut,
        export_bundle, record_frontend_event, running,
    };
    use crate::logging::DiagnosticsState;
    use crate::test_support::TestLog;

    fn event(message: &str) -> FrontendEvent {
        FrontendEvent {
            level: FrontendLevel::Error,
            source: "window.onerror".to_owned(),
            message: message.to_owned(),
            stack: None,
            component_stack: None,
            command: None,
        }
    }

    #[test]
    fn events_are_deserialised_from_the_frontend_payload() {
        let parsed: FrontendEvent = serde_json::from_value(serde_json::json!({
            "level": "warn",
            "source": "react",
            "message": "render failed",
            "componentStack": "at App",
        }))
        .unwrap();

        assert_eq!(parsed.level, FrontendLevel::Warn);
        assert_eq!(parsed.component_stack.as_deref(), Some("at App"));
        assert_eq!(parsed.stack, None);
        assert_eq!(parsed.command, None);
    }

    #[test]
    fn a_frontend_error_is_written_with_its_source_and_stack() {
        let log = TestLog::new();
        let state = DiagnosticsState::new();
        let mut reported = event("x is not a function");
        reported.stack = Some("at render (app.js:1:2)".to_owned());
        reported.command = Some("core_version".to_owned());

        let written = tracing::dispatcher::with_default(&log.dispatch, || {
            record_frontend_event(&state, reported)
        });

        assert!(written);
        let records = log.finish();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["level"], "ERROR");
        assert_eq!(records[0]["target"], FRONTEND_TARGET);
        assert_eq!(records[0]["msg"], "x is not a function");
        assert_eq!(records[0]["fields"]["source"], "window.onerror");
        assert_eq!(records[0]["fields"]["stack"], "at render (app.js:1:2)");
        assert_eq!(records[0]["fields"]["command"], "core_version");
        assert!(records[0]["fields"].get("component_stack").is_none());
    }

    #[test]
    fn the_level_of_the_event_is_kept() {
        let log = TestLog::new();
        let state = DiagnosticsState::new();

        tracing::dispatcher::with_default(&log.dispatch, || {
            for level in [
                FrontendLevel::Info,
                FrontendLevel::Warn,
                FrontendLevel::Error,
            ] {
                let mut reported = event("something");
                reported.level = level;
                record_frontend_event(&state, reported);
            }
        });

        let levels: Vec<_> = log
            .finish()
            .iter()
            .map(|record| record["level"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(levels, ["INFO", "WARN", "ERROR"]);
    }

    #[test]
    fn an_error_loop_in_the_frontend_cannot_flood_the_log() {
        let log = TestLog::new();
        let state = DiagnosticsState::new();

        let written = tracing::dispatcher::with_default(&log.dispatch, || {
            (0..10_000)
                .filter(|index| record_frontend_event(&state, event(&format!("loop {index}"))))
                .count()
        });

        // The burst, plus the few that the clock lets through while the loop
        // runs.
        assert!(
            (20..=30).contains(&written),
            "{written} records were written"
        );
        let records = log.finish();
        assert!(records.len() <= 60, "{} records in the log", records.len());
    }

    #[test]
    fn oversized_events_are_cut() {
        let log = TestLog::new();
        let state = DiagnosticsState::new();
        let mut reported = event(&"m".repeat(MAX_MESSAGE_CHARS * 3));
        reported.stack = Some("é".repeat(MAX_STACK_CHARS * 3));

        tracing::dispatcher::with_default(&log.dispatch, || {
            record_frontend_event(&state, reported);
        });

        let records = log.finish();
        let message = records[0]["msg"].as_str().unwrap();
        let stack = records[0]["fields"]["stack"].as_str().unwrap();
        assert_eq!(message.chars().count(), MAX_MESSAGE_CHARS);
        assert_eq!(stack.chars().count(), MAX_STACK_CHARS);
    }

    #[test]
    fn cut_keeps_whole_characters() {
        assert_eq!(cut("añb", 2), "añ");
        assert_eq!(cut("short", 10), "short");
        assert_eq!(cut("", 0), "");
    }

    #[test]
    fn the_bundle_is_exported_to_the_requested_folder_and_the_export_is_logged() {
        let log = TestLog::new();
        let destination = log.scratch_dir();
        let state = DiagnosticsState::with_diagnostics(log.diagnostics.clone());

        let bundle = tracing::dispatcher::with_default(&log.dispatch, || {
            export_bundle(&state, Some(&destination))
        })
        .unwrap();

        assert!(bundle.starts_with(&destination));
        assert!(bundle.is_file());
        let records = log.finish();
        assert_eq!(
            records.last().unwrap()["msg"],
            "diagnostics bundle exported"
        );
    }

    #[test]
    fn commands_fail_cleanly_when_logging_is_not_running() {
        let state = DiagnosticsState::new();

        assert!(running(&state).is_err());
        assert!(export_bundle(&state, None).is_err());
    }

    #[test]
    fn exporting_without_a_destination_is_an_error() {
        let log = TestLog::new();
        let state = DiagnosticsState::with_diagnostics(log.diagnostics.clone());

        assert!(export_bundle(&state, None).is_err());
    }
}
