//! The diagnostics bundle: one zip file to attach to a bug report.
//!
//! It contains the log files under `logs/` and a `system-info.json` with the
//! versions and the platform. Nothing else is read from the machine.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;
use time::OffsetDateTime;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use super::record::{compact_timestamp, rfc3339_millis};
use super::rotate::{LogFile, log_files};
use super::{DiagnosticsError, LogLevel, SystemInfo};

/// Name of the file inside the bundle that describes the environment.
pub const SYSTEM_INFO_FILE_NAME: &str = "system-info.json";

/// Directory inside the bundle that holds the log files.
pub const LOGS_DIR_NAME: &str = "logs";

/// What `system-info.json` says besides the [`SystemInfo`].
#[derive(Debug, Clone, Copy)]
pub struct BundleContext<'a> {
    /// Identifier of the launch that produced the bundle.
    pub run_id: &'a str,
    /// Log level in effect.
    pub log_level: LogLevel,
    /// Versions and platform.
    pub system: &'a SystemInfo,
}

#[derive(Serialize)]
struct SystemInfoFile<'a> {
    generated_at: String,
    run_id: &'a str,
    log_level: &'static str,
    #[serde(flatten)]
    system: &'a SystemInfo,
    log_files: Vec<LogFileEntry>,
}

#[derive(Serialize)]
struct LogFileEntry {
    name: String,
    bytes: u64,
}

/// Writes `termx-diagnostics-<UTC timestamp>.zip` into `dest_dir` and returns
/// its path.
///
/// # Errors
///
/// Fails when the log directory cannot be read or the bundle cannot be
/// written. A partially written bundle is removed.
pub fn export(
    log_dir: &Path,
    dest_dir: &Path,
    context: BundleContext<'_>,
) -> Result<PathBuf, DiagnosticsError> {
    let now = OffsetDateTime::now_utc();
    let logs = log_files(log_dir).map_err(|source| DiagnosticsError::io(log_dir, source))?;
    fs::create_dir_all(dest_dir).map_err(|source| DiagnosticsError::io(dest_dir, source))?;

    let path = free_path(dest_dir, &compact_timestamp(now));
    let file = File::create_new(&path).map_err(|source| DiagnosticsError::io(&path, source))?;
    if let Err(source) = write(file, &logs, context, now) {
        let _ = fs::remove_file(&path);
        return Err(DiagnosticsError::io(&path, source));
    }
    Ok(path)
}

/// A bundle path in `dir` that is not taken yet.
fn free_path(dir: &Path, stamp: &str) -> PathBuf {
    let mut path = dir.join(format!("termx-diagnostics-{stamp}.zip"));
    let mut counter = 2u32;
    while path.exists() {
        path = dir.join(format!("termx-diagnostics-{stamp}-{counter}.zip"));
        counter += 1;
    }
    path
}

fn write(
    file: File,
    logs: &[LogFile],
    context: BundleContext<'_>,
    now: OffsetDateTime,
) -> io::Result<()> {
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let mut zip = ZipWriter::new(file);
    let mut entries = Vec::with_capacity(logs.len());

    for log in logs {
        let Some(name) = log.path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        // A file can disappear between the listing and now if it is rotated
        // or pruned; the bundle is still useful without it.
        let Ok(source) = File::open(&log.path) else {
            continue;
        };
        zip.start_file(format!("{LOGS_DIR_NAME}/{name}"), options)?;
        // The active file keeps growing: stop at the size it had when listed.
        let bytes = io::copy(&mut source.take(log.size), &mut zip)?;
        entries.push(LogFileEntry {
            name: name.to_owned(),
            bytes,
        });
    }

    let info = SystemInfoFile {
        generated_at: rfc3339_millis(now),
        run_id: context.run_id,
        log_level: context.log_level.as_str(),
        system: context.system,
        log_files: entries,
    };
    zip.start_file(SYSTEM_INFO_FILE_NAME, options)?;
    serde_json::to_writer_pretty(&mut zip, &info)?;
    zip.write_all(b"\n")?;

    zip.finish()?.sync_all()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs::{self, File};
    use std::io::Read;
    use std::path::Path;

    use serde_json::Value;

    use super::{BundleContext, export};
    use crate::diagnostics::{LogLevel, SystemInfo};

    fn system() -> SystemInfo {
        SystemInfo {
            app_version: "1.2.3".to_owned(),
            core_version: "1.2.3".to_owned(),
            os: "linux".to_owned(),
            os_version: "Ubuntu 24.04".to_owned(),
            arch: "x86_64".to_owned(),
            webview_version: Some("2.46".to_owned()),
            locale: Some("en-US".to_owned()),
            debug_build: false,
        }
    }

    fn context(system: &SystemInfo) -> BundleContext<'_> {
        BundleContext {
            run_id: "5f0c2a9be771",
            log_level: LogLevel::Debug,
            system,
        }
    }

    fn read_zip(path: &Path) -> BTreeMap<String, String> {
        let mut archive = zip::ZipArchive::new(File::open(path).unwrap()).unwrap();
        (0..archive.len())
            .map(|index| {
                let mut entry = archive.by_index(index).unwrap();
                let mut content = String::new();
                entry.read_to_string(&mut content).unwrap();
                (entry.name().unwrap().into_owned(), content)
            })
            .collect()
    }

    #[test]
    fn the_bundle_holds_the_log_files_and_the_system_information() {
        let logs = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        fs::write(logs.path().join("termx.log"), "current\n").unwrap();
        fs::write(
            logs.path().join("termx-20261125T101500Z-000.log"),
            "older\n",
        )
        .unwrap();
        let system = system();

        let path = export(logs.path(), dest.path(), context(&system)).unwrap();

        let name = path.file_name().unwrap().to_str().unwrap();
        assert!(
            name.starts_with("termx-diagnostics-") && name.ends_with(".zip"),
            "{name}"
        );
        assert_eq!(path.parent().unwrap(), dest.path());

        let entries = read_zip(&path);
        assert_eq!(
            entries.keys().map(String::as_str).collect::<Vec<_>>(),
            [
                "logs/termx-20261125T101500Z-000.log",
                "logs/termx.log",
                "system-info.json"
            ]
        );
        assert_eq!(entries["logs/termx.log"], "current\n");
        assert_eq!(entries["logs/termx-20261125T101500Z-000.log"], "older\n");

        let info: Value = serde_json::from_str(&entries["system-info.json"]).unwrap();
        assert_eq!(info["app_version"], "1.2.3");
        assert_eq!(info["os_version"], "Ubuntu 24.04");
        assert_eq!(info["arch"], "x86_64");
        assert_eq!(info["webview_version"], "2.46");
        assert_eq!(info["locale"], "en-US");
        assert_eq!(info["run_id"], "5f0c2a9be771");
        assert_eq!(info["log_level"], "debug");
        assert!(info["generated_at"].as_str().unwrap().ends_with('Z'));
        assert_eq!(info["log_files"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn system_information_has_only_the_documented_keys() {
        let logs = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        let system = system();

        let path = export(logs.path(), dest.path(), context(&system)).unwrap();

        let entries = read_zip(&path);
        let info: Value = serde_json::from_str(&entries["system-info.json"]).unwrap();
        let mut keys: Vec<&str> = info
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "app_version",
                "arch",
                "core_version",
                "debug_build",
                "generated_at",
                "locale",
                "log_files",
                "log_level",
                "os",
                "os_version",
                "run_id",
                "webview_version"
            ]
        );
    }

    #[test]
    fn files_that_are_not_termx_logs_stay_out_of_the_bundle() {
        let logs = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        fs::write(logs.path().join("termx.log"), "current\n").unwrap();
        fs::write(logs.path().join("secrets.txt"), "do not export\n").unwrap();
        fs::write(logs.path().join("shell-history.log"), "do not export\n").unwrap();
        let system = system();

        let path = export(logs.path(), dest.path(), context(&system)).unwrap();

        let entries = read_zip(&path);
        assert_eq!(entries.len(), 2);
        assert!(
            entries
                .values()
                .all(|content| !content.contains("do not export"))
        );
    }

    #[test]
    fn exporting_twice_never_overwrites_a_bundle() {
        let logs = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        let system = system();

        let first = export(logs.path(), dest.path(), context(&system)).unwrap();
        let second = export(logs.path(), dest.path(), context(&system)).unwrap();

        assert_ne!(first, second);
        assert!(first.exists() && second.exists());
    }

    #[test]
    fn the_destination_directory_is_created_when_missing() {
        let logs = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        let nested = dest.path().join("a").join("b");
        let system = system();

        let path = export(logs.path(), &nested, context(&system)).unwrap();

        assert!(path.starts_with(&nested));
    }

    #[test]
    fn a_missing_log_directory_is_an_error_that_hides_the_home_directory() {
        let dest = tempfile::tempdir().unwrap();
        let missing = dest.path().join("no-such-directory");
        let system = system();

        let error = export(&missing, dest.path(), context(&system)).unwrap_err();

        assert!(error.to_string().contains("no-such-directory"), "{error}");
        assert_eq!(fs::read_dir(dest.path()).unwrap().count(), 0);
    }
}
