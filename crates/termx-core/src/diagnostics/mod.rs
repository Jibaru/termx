//! Persistent logging and diagnostics.
//!
//! When something goes wrong, termx records it in a log file on the machine
//! of the user, who can later send those files along with a bug report.
//! Nothing is ever uploaded.
//!
//! # Logging from termx code
//!
//! Use the [`tracing`] macros anywhere in the workspace. They are re-exported
//! here so that crates do not need their own dependency:
//!
//! ```
//! use termx_core::diagnostics::tracing::{error, info};
//!
//! # let session_id = 7;
//! info!(session_id, "session started");
//! # let source = std::io::Error::other("example");
//! error!(session_id, error = %source, "could not resize the terminal");
//! ```
//!
//! The application calls [`Diagnostics::init`] once at startup. Until then,
//! and in tests, the macros do nothing.
//!
//! # Privacy rules
//!
//! The log is meant to be shared, so these rules are not negotiable. Nothing
//! in the logger can recognise such data: they are enforced by review.
//!
//! - **Never log terminal output**, in whole or in part.
//! - **Never log keystrokes** or any other input sent to a session.
//! - **Never log the values of environment variables.** Names are fine.
//! - **Never log the contents of files.**
//!
//! Log sizes, counts, identifiers, durations and error kinds instead.
//!
//! What the logger does on its own, to the message and all string fields of
//! every record: it replaces the home directory with `~` and the user name
//! with `<user>` (see [`redact`] for the exact rules), so logging a path is
//! safe by default, and it replaces what looks like a secret with
//! `<redacted>` (see [`scrub`]), which is best effort and no excuse to log
//! one. Use [`redact::path`] for paths that end up anywhere else, such as an
//! error message returned to the interface.
//!
//! # Delivery
//!
//! A `WARN` or `ERROR` logging call returns when its record is written, or
//! after [`URGENT_WAIT`] if the disk does not answer, in which case the
//! record is written late. Repeats of the same warning or error are written
//! [`REPEATS_WRITTEN`] times per [`REPEAT_WINDOW`] and then counted. The
//! first [`FIRST_ERRORS_RECORDS`] warnings and errors of a launch are also
//! kept in a file that the launch never deletes. Lower levels are queued and
//! can be dropped, with a count, under extreme load.
//!
//! # Format and retention
//!
//! Each record is one JSON object on its own line, with the keys `ts`,
//! `level`, `run`, `target`, `msg`, optionally `span`, and `fields`, in that
//! order:
//!
//! ```json
//! {"ts":"2026-11-26T19:04:11.482Z","level":"ERROR","run":"5f0c2a9be771","target":"termx::command","msg":"command failed","fields":{"command":"export_diagnostics","error":"permission denied"}}
//! ```
//!
//! Every process writes its own files. A new file is started by size, and
//! files are deleted by age, count and total size according to [`Retention`]. `docs/logging.md` in the repository is the user-facing
//! description.

use std::error::Error;
use std::fmt;
use std::hash::{BuildHasher, Hasher};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tracing_subscriber::layer::SubscriberExt;

pub mod bundle;
pub mod gate;
mod layer;
mod level;
mod normalise;
mod panic;
pub mod rate_limit;
mod record;
pub mod redact;
mod rotate;
pub mod scrub;
mod suppress;
mod system;
mod writer;

pub use tracing;

pub use self::gate::{Dropped, GateConfig, RecordGate, Severity};
pub use self::level::{LogLevel, ParseLogLevelError};
pub use self::normalise::message_key;
pub use self::panic::{PANIC_TARGET, panic_context, with_panic_context};
pub use self::rate_limit::{Decision, RateLimiter};
pub use self::record::{MAX_VALUE_BYTES, TRUNCATION_MARKER};
pub use self::redact::Redactor;
pub use self::rotate::{LogFile, Retention, is_log_file_name, log_files, prune, verify_log_dir};
pub use self::suppress::{WINDOW as REPEAT_WINDOW, WRITTEN_PER_WINDOW as REPEATS_WRITTEN};
pub use self::system::SystemInfo;
pub use self::writer::{FIRST_ERRORS_RECORDS, LOG_TARGET, URGENT_WAIT};

use self::layer::JsonLinesLayer;
use self::record::LineContext;
use self::rotate::RotatingFile;
use self::writer::{LogWriter, QUEUE_CAPACITY};

/// Target of the record written at startup.
pub const STARTUP_TARGET: &str = "termx::startup";

/// Settings for [`Diagnostics::init`].
///
/// The core does not decide where logs live: the application passes the
/// directory that is right for the platform.
#[derive(Debug, Clone)]
pub struct LogConfig {
    /// Directory for the log files. Created if missing.
    pub dir: PathBuf,
    /// What is recorded. Defaults to the value of the `TERMX_LOG` environment
    /// variable, or `info`.
    pub level: LogLevel,
    /// Also print records to stderr. Meant for debug builds.
    pub stderr: bool,
    /// Limits on the size and the age of the log files.
    pub retention: Retention,
    /// Version of the application, for the startup record and the bundle.
    pub app_version: String,
    /// Version of the webview, when the application knows it.
    pub webview_version: Option<String>,
    /// Hides the home directory. Defaults to the one of the current user.
    pub redactor: Redactor,
}

impl LogConfig {
    /// The default settings for logging into `dir`.
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>, app_version: impl Into<String>) -> Self {
        Self {
            dir: dir.into(),
            level: LogLevel::from_env(),
            stderr: false,
            retention: Retention::default(),
            app_version: app_version.into(),
            webview_version: None,
            redactor: redact::redactor().clone(),
        }
    }
}

/// An error from the diagnostics module.
///
/// Paths in the message have the home directory replaced by `~`, so the
/// message can be logged and shown as it is.
#[derive(Debug)]
pub enum DiagnosticsError {
    /// A file or directory could not be read or written.
    Io {
        /// The file or directory involved.
        path: PathBuf,
        /// The underlying error.
        source: io::Error,
    },
    /// [`Diagnostics::init`] was called after a global logger was installed.
    AlreadyInitialised,
}

impl DiagnosticsError {
    pub(crate) fn io(path: &Path, source: io::Error) -> Self {
        Self::Io {
            path: path.to_owned(),
            source,
        }
    }
}

impl fmt::Display for DiagnosticsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => {
                let source = source.to_string();
                write!(f, "{}: {}", redact::path(path), redact::text(&source))
            }
            Self::AlreadyInitialised => f.write_str("logging is already initialised"),
        }
    }
}

impl Error for DiagnosticsError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::AlreadyInitialised => None,
        }
    }
}

/// Handle to the running logger.
///
/// Cheap to clone. Keep one for as long as the application runs and call
/// [`Diagnostics::shutdown`] before exiting.
#[derive(Debug, Clone)]
pub struct Diagnostics {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    dir: PathBuf,
    level: LogLevel,
    system: SystemInfo,
    context: Arc<LineContext>,
    writer: Arc<LogWriter>,
}

impl Diagnostics {
    /// Starts logging for the whole process.
    ///
    /// Installs the global `tracing` subscriber, forwards records of the
    /// `log` crate to it, installs the panic hook and writes the startup
    /// record.
    ///
    /// # Errors
    ///
    /// Fails when the log directory or file cannot be created, or when a
    /// global subscriber is already installed.
    pub fn init(config: LogConfig) -> Result<Self, DiagnosticsError> {
        let (diagnostics, dispatch) = Self::build(config)?;
        tracing::dispatcher::set_global_default(dispatch)
            .map_err(|_| DiagnosticsError::AlreadyInitialised)?;
        // Fails only when another `log` logger exists, which then keeps
        // receiving those records.
        let _ = tracing_log::LogTracer::init();
        diagnostics.install_panic_hook();
        diagnostics.log_startup();
        Ok(diagnostics)
    }

    /// Opens the log and returns the subscriber without installing anything
    /// global.
    ///
    /// [`Diagnostics::init`] is what applications call. This is for tests and
    /// for embedding: run code under the returned dispatcher with
    /// [`tracing::dispatcher::with_default`].
    ///
    /// # Errors
    ///
    /// Fails when the log directory or file cannot be created.
    pub fn build(config: LogConfig) -> Result<(Self, tracing::Dispatch), DiagnosticsError> {
        let run_id = new_run_id();
        let file = RotatingFile::open(&config.dir, config.retention, &run_id)
            .map_err(|source| DiagnosticsError::io(&config.dir, source))?;
        let context = Arc::new(LineContext {
            run_id,
            redactor: config.redactor,
        });
        let writer = LogWriter::new(file, Arc::clone(&context), QUEUE_CAPACITY);
        writer
            .start_worker()
            .map_err(|source| DiagnosticsError::io(&config.dir, source))?;

        let stderr = config
            .stderr
            .then(|| tracing_subscriber::fmt::layer().with_writer(io::stderr));
        let subscriber = tracing_subscriber::registry()
            .with(config.level.filter())
            .with(JsonLinesLayer::new(
                Arc::clone(&writer),
                Arc::clone(&context),
            ))
            .with(stderr);

        let diagnostics = Self {
            inner: Arc::new(Inner {
                dir: config.dir,
                level: config.level,
                system: SystemInfo::collect(&config.app_version, config.webview_version),
                context,
                writer,
            }),
        };
        Ok((diagnostics, tracing::Dispatch::new(subscriber)))
    }

    /// Makes every panic, on any thread, write a record to the log before
    /// the previously installed hook runs.
    ///
    /// The record has the target [`PANIC_TARGET`] and the fields `message`,
    /// `location`, `thread` and `backtrace`, plus whatever
    /// [`with_panic_context`] added. It is written even when the
    /// level is [`LogLevel::Off`].
    pub fn install_panic_hook(&self) {
        panic::install(
            Arc::clone(&self.inner.writer),
            Arc::clone(&self.inner.context),
        );
    }

    /// Writes the startup record: versions, operating system, architecture
    /// and locale.
    pub fn log_startup(&self) {
        let system = &self.inner.system;
        tracing::info!(
            target: STARTUP_TARGET,
            app_version = %system.app_version,
            core_version = %system.core_version,
            os = %system.os,
            os_version = %system.os_version,
            arch = %system.arch,
            webview_version = system.webview_version.as_deref().unwrap_or("unknown"),
            locale = system.locale.as_deref().unwrap_or("unknown"),
            debug_build = system.debug_build,
            log_level = %self.inner.level,
            "termx started"
        );
    }

    /// Directory that holds the log files.
    #[must_use]
    pub fn log_dir(&self) -> &Path {
        &self.inner.dir
    }

    /// Identifier of this launch. It is part of every record.
    #[must_use]
    pub fn run_id(&self) -> &str {
        &self.inner.context.run_id
    }

    /// The level in effect.
    #[must_use]
    pub fn level(&self) -> LogLevel {
        self.inner.level
    }

    /// Versions and platform, as written to the startup record.
    #[must_use]
    pub fn system_info(&self) -> &SystemInfo {
        &self.inner.system
    }

    /// Writes a diagnostics bundle into `dest_dir` and returns its path.
    ///
    /// See [`bundle`] for what it contains.
    ///
    /// # Errors
    ///
    /// Fails when the logs cannot be read or the bundle cannot be written.
    pub fn export_bundle(&self, dest_dir: &Path) -> Result<PathBuf, DiagnosticsError> {
        // So that the bundle has everything logged up to now.
        self.flush();
        bundle::export(
            &self.inner.dir,
            dest_dir,
            bundle::BundleContext {
                run_id: self.run_id(),
                log_level: self.inner.level,
                system: &self.inner.system,
            },
        )
    }

    /// Writes everything that is still queued and asks the operating system
    /// to flush the log file to the disk.
    ///
    /// Warnings and errors never wait in the queue, so this is only needed
    /// to be sure about `INFO` and lower records, for example before reading
    /// the log.
    pub fn flush(&self) {
        self.inner.writer.flush();
    }

    /// Flushes the log and stops the background writer. Call it once, right
    /// before the process exits.
    ///
    /// Records logged afterwards are still written, synchronously.
    pub fn shutdown(&self) {
        self.inner.writer.shutdown();
    }
}

/// A random identifier for this launch: twelve hexadecimal digits.
fn new_run_id() -> String {
    // `RandomState` is seeded from the operating system.
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u32(std::process::id());
    format!("{:012x}", hasher.finish() & 0xffff_ffff_ffff)
}

#[cfg(test)]
mod tests {
    use super::new_run_id;

    #[test]
    fn run_ids_are_twelve_hex_digits_and_differ_between_launches() {
        let first = new_run_id();
        let second = new_run_id();

        assert_eq!(first.len(), 12);
        assert!(first.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(first, second);
    }
}
