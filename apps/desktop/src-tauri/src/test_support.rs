//! Helpers shared by the unit tests of this crate.

use serde_json::Value;
use termx_core::diagnostics::tracing::Dispatch;
use termx_core::diagnostics::{Diagnostics, LogConfig, LogLevel, log_files};

/// A logger writing to a temporary directory. Run the code under test with
/// `tracing::dispatcher::with_default(&log.dispatch, ...)`.
pub struct TestLog {
    pub diagnostics: Diagnostics,
    pub dispatch: Dispatch,
    dir: tempfile::TempDir,
}

impl TestLog {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut config = LogConfig::new(dir.path().join("logs"), "0.0.0-test");
        config.level = LogLevel::Info;
        let (diagnostics, dispatch) = Diagnostics::build(config).unwrap();
        Self {
            diagnostics,
            dispatch,
            dir,
        }
    }

    /// A directory next to the logs, for files written by the test.
    pub fn scratch_dir(&self) -> std::path::PathBuf {
        self.dir.path().join("scratch")
    }

    /// Flushes the log and returns its records, in order.
    pub fn finish(&self) -> Vec<Value> {
        self.diagnostics.shutdown();
        let mut records = Vec::new();
        for file in log_files(self.diagnostics.log_dir()).unwrap() {
            let text = std::fs::read_to_string(&file.path).unwrap();
            records.extend(
                text.lines()
                    .map(|line| serde_json::from_str::<Value>(line).unwrap()),
            );
        }
        records
    }
}
