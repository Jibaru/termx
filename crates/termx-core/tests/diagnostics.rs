//! End-to-end tests of the diagnostics module through its public API.

// `allow-unwrap-in-tests` only covers `#[test]` functions, not their helpers.
#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use termx_core::diagnostics::tracing::{self, debug, error, info, info_span, warn};
use termx_core::diagnostics::{
    Diagnostics, LogConfig, LogLevel, PANIC_TARGET, Redactor, Retention, STARTUP_TARGET, log_files,
};

/// Set for the child process of the panic test: the directory to log into.
const PANIC_CHILD_DIR: &str = "TERMX_TEST_PANIC_LOG_DIR";

fn config(dir: &Path) -> LogConfig {
    LogConfig {
        dir: dir.to_owned(),
        level: LogLevel::Info,
        stderr: false,
        retention: Retention::default(),
        app_version: "1.2.3".to_owned(),
        webview_version: Some("605.1.15".to_owned()),
        redactor: Redactor::with_case_sensitivity(Some(Path::new("/home/ana")), true),
    }
}

/// Runs `body` with logging going to `config.dir`, then returns the records
/// that were written, in order.
fn logged(config: LogConfig, body: impl FnOnce(&Diagnostics)) -> Vec<Value> {
    let dir = config.dir.clone();
    let (diagnostics, dispatch) = Diagnostics::build(config).unwrap();
    tracing::dispatcher::with_default(&dispatch, || body(&diagnostics));
    diagnostics.shutdown();
    read_records(&dir)
}

/// Everything in the log directory, oldest file first.
fn read_text(dir: &Path) -> String {
    log_files(dir)
        .unwrap()
        .iter()
        .map(|file| fs::read_to_string(&file.path).unwrap())
        .collect()
}

fn read_records(dir: &Path) -> Vec<Value> {
    let mut records = Vec::new();
    for file in log_files(dir).unwrap() {
        let text = fs::read_to_string(&file.path).unwrap();
        assert!(text.is_empty() || text.ends_with('\n'), "{text:?}");
        for line in text.lines() {
            let record: Value = serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("not a JSON line ({error}): {line}"));
            records.push(record);
        }
    }
    records
}

#[test]
fn logging_creates_the_file_and_writes_one_json_object_per_line() {
    let dir = tempfile::tempdir().unwrap();
    let log_dir = dir.path().join("logs");

    let records = logged(config(&log_dir), |_| {
        info!(session_id = 7, "session started");
        error!(command = "resize", error = %"no such session", attempts = 2, ok = false, "command failed");
    });

    assert_eq!(log_files(&log_dir).unwrap().len(), 1);
    assert_eq!(records.len(), 2);

    let first = &records[0];
    assert_eq!(first["level"], "INFO");
    assert_eq!(first["msg"], "session started");
    assert_eq!(first["target"], "diagnostics");
    assert_eq!(first["fields"]["session_id"], 7);

    let second = &records[1];
    assert_eq!(second["level"], "ERROR");
    assert_eq!(second["msg"], "command failed");
    assert_eq!(second["fields"]["command"], "resize");
    assert_eq!(second["fields"]["error"], "no such session");
    assert_eq!(second["fields"]["attempts"], 2);
    assert_eq!(second["fields"]["ok"], false);
}

#[test]
fn every_record_has_the_documented_keys_in_the_documented_order() {
    let dir = tempfile::tempdir().unwrap();

    let records = logged(config(dir.path()), |_| warn!(code = 3, "careful"));
    let text = read_text(dir.path());

    let positions: Vec<usize> = [
        "\"ts\":",
        "\"level\":",
        "\"run\":",
        "\"target\":",
        "\"msg\":",
        "\"fields\":",
    ]
    .iter()
    .map(|key| {
        text.find(key)
            .unwrap_or_else(|| panic!("{key} missing in {text}"))
    })
    .collect();
    assert!(positions.is_sorted(), "{text}");

    let timestamp = records[0]["ts"].as_str().unwrap();
    assert_eq!(
        timestamp.len(),
        "2026-11-26T19:04:11.482Z".len(),
        "{timestamp}"
    );
    assert!(timestamp.ends_with('Z') && timestamp.as_bytes()[10] == b'T');
}

#[test]
fn all_records_of_a_launch_share_its_run_id() {
    let dir = tempfile::tempdir().unwrap();
    let mut run_id = String::new();

    let records = logged(config(dir.path()), |diagnostics| {
        run_id = diagnostics.run_id().to_owned();
        info!("one");
        info!("two");
    });

    assert_eq!(run_id.len(), 12);
    assert!(
        records
            .iter()
            .all(|record| record["run"] == run_id.as_str())
    );

    let second_launch = logged(config(dir.path()), |_| info!("three"));
    assert_eq!(
        second_launch.len(),
        3,
        "the log is appended to across launches"
    );
    assert_ne!(second_launch[2]["run"], run_id.as_str());
}

#[test]
fn the_level_decides_what_is_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let emit = |_: &Diagnostics| {
        debug!("detail");
        info!("normal");
        error!("broken");
    };

    let at_info = logged(config(&dir.path().join("info")), emit);
    assert_eq!(at_info.len(), 2);

    let mut verbose = config(&dir.path().join("debug"));
    verbose.level = LogLevel::Debug;
    assert_eq!(logged(verbose, emit).len(), 3);

    let mut quiet = config(&dir.path().join("error"));
    quiet.level = LogLevel::Error;
    let at_error = logged(quiet, emit);
    assert_eq!(at_error.len(), 1);
    assert_eq!(at_error[0]["msg"], "broken");

    let mut off = config(&dir.path().join("off"));
    off.level = LogLevel::Off;
    assert!(logged(off, emit).is_empty());
}

#[test]
fn the_home_directory_never_reaches_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = PathBuf::from("/home/ana/projects/termx/config.toml");

    logged(config(dir.path()), |_| {
        error!(path = %path.display(), debug_path = ?path, "cannot read /home/ana/.config/termx");
    });

    let text = read_text(dir.path());
    assert!(!text.contains("/home/ana"), "{text}");
    assert!(text.contains("~/projects/termx/config.toml"), "{text}");
    assert!(text.contains("cannot read ~/.config/termx"), "{text}");
}

#[test]
fn span_names_are_recorded_with_the_event() {
    let dir = tempfile::tempdir().unwrap();

    let records = logged(config(dir.path()), |_| {
        let _outer = info_span!("startup").entered();
        let _inner = info_span!("restore_sessions").entered();
        info!("restored");
    });

    assert_eq!(records[0]["span"], "startup>restore_sessions");
}

#[test]
fn the_startup_record_describes_the_application_and_the_platform() {
    let dir = tempfile::tempdir().unwrap();

    let records = logged(config(dir.path()), Diagnostics::log_startup);

    let record = &records[0];
    assert_eq!(record["target"], STARTUP_TARGET);
    assert_eq!(record["level"], "INFO");
    let fields = &record["fields"];
    assert_eq!(fields["app_version"], "1.2.3");
    assert_eq!(fields["core_version"], termx_core::version());
    assert_eq!(fields["os"], std::env::consts::OS);
    assert_eq!(fields["arch"], std::env::consts::ARCH);
    assert_eq!(fields["webview_version"], "605.1.15");
    assert_eq!(fields["log_level"], "info");
    assert!(!fields["os_version"].as_str().unwrap().is_empty());
    assert!(fields["locale"].is_string());
}

#[test]
fn rotation_keeps_the_log_within_its_limits_while_logging() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = config(dir.path());
    config.retention = Retention {
        max_file_bytes: 4 * 1024,
        max_total_bytes: 16 * 1024,
        min_files_kept: 0,
        ..Retention::default()
    };

    let records = logged(config, |_| {
        for index in 0..2_000 {
            info!(
                index,
                "a record that is long enough to fill the files quickly"
            );
        }
    });

    let files = log_files(dir.path()).unwrap();
    assert!(files.len() > 1, "the log should have rotated");
    assert!(files.iter().all(|file| file.size <= 4 * 1024), "{files:?}");
    assert!(files.iter().map(|file| file.size).sum::<u64>() <= 16 * 1024);
    // The newest records survive, complete and in order.
    assert_eq!(records.last().unwrap()["fields"]["index"], 1_999);
    let indexes: Vec<u64> = records
        .iter()
        .map(|record| record["fields"]["index"].as_u64().unwrap())
        .collect();
    assert!(indexes.is_sorted());
}

#[test]
fn the_bundle_exported_from_a_live_logger_contains_its_records() {
    let dir = tempfile::tempdir().unwrap();
    let log_dir = dir.path().join("logs");
    let dest = dir.path().join("out");
    let (diagnostics, dispatch) = Diagnostics::build(config(&log_dir)).unwrap();
    tracing::dispatcher::with_default(&dispatch, || error!("something broke"));
    diagnostics.shutdown();

    let bundle = diagnostics.export_bundle(&dest).unwrap();

    let mut archive = zip::ZipArchive::new(fs::File::open(&bundle).unwrap()).unwrap();
    assert!(archive.by_name("system-info.json").is_ok());
    let name = format!("logs/{}", log_files(&log_dir).unwrap()[0].name);
    let log = std::io::read_to_string(archive.by_name(&name).unwrap()).unwrap();
    assert!(log.contains("something broke"));
}

#[test]
fn a_panic_on_any_thread_is_recorded_and_the_default_behaviour_is_kept() {
    // The child half: started by the parent half below with the variable
    // set. It installs the real global logger and panics, which makes this
    // test fail in the child process only.
    if let Some(dir) = std::env::var_os(PANIC_CHILD_DIR) {
        let mut config = config(Path::new(&dir));
        // The panic record does not depend on the level.
        config.level = LogLevel::Off;
        let _diagnostics = Diagnostics::init(config).unwrap();

        let worker = std::thread::Builder::new()
            .name("pty-reader".to_owned())
            .spawn(|| panic!("worker failed in /home/ana/project"))
            .unwrap();
        assert!(worker.join().is_err());
        panic!("main failed with code {}", 42);
    }

    let dir = tempfile::tempdir().unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "a_panic_on_any_thread_is_recorded_and_the_default_behaviour_is_kept",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(PANIC_CHILD_DIR, dir.path())
        .env("RUST_BACKTRACE", "0")
        .output()
        .unwrap();

    // The default hook still ran: the process failed and said why on stderr.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "the child should fail: {stderr}");
    assert!(stderr.contains("worker failed in"), "{stderr}");
    assert!(stderr.contains("main failed with code 42"), "{stderr}");

    let records = read_records(dir.path());
    let panics: Vec<&Value> = records
        .iter()
        .filter(|record| record["target"] == PANIC_TARGET)
        .collect();
    assert_eq!(panics.len(), 2, "{records:?}");

    let worker = &panics[0];
    assert_eq!(worker["level"], "ERROR");
    assert_eq!(worker["msg"], "panic");
    assert_eq!(worker["fields"]["message"], "worker failed in ~/project");
    assert_eq!(worker["fields"]["thread"], "pty-reader");
    let location = worker["fields"]["location"].as_str().unwrap();
    assert!(location.contains("diagnostics.rs:"), "{location}");
    assert!(!worker["fields"]["backtrace"].as_str().unwrap().is_empty());

    let main = &panics[1];
    assert_eq!(main["fields"]["message"], "main failed with code 42");
    assert_eq!(main["run"], worker["run"]);
}
