//! The log files: size-based rotation and bounded retention.
//!
//! Records are appended to `termx.log`. When the next record would take the
//! file past [`Retention::max_file_bytes`], the file is renamed to
//! `termx-<UTC timestamp>-<counter>.log` and a new `termx.log` is started. Rotated
//! files are deleted when they are older than [`Retention::max_age`], and the
//! oldest ones are deleted until everything fits in
//! [`Retention::max_total_bytes`].

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use time::OffsetDateTime;

use super::record::compact_timestamp;

/// Name of the file that is currently written to.
pub const ACTIVE_FILE_NAME: &str = "termx.log";

const ROTATED_PREFIX: &str = "termx-";
const EXTENSION: &str = ".log";

/// Limits on the disk space and the age of the log files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Retention {
    /// Size at which the active file is rotated.
    pub max_file_bytes: u64,
    /// Upper bound for all log files together, the active one included.
    pub max_total_bytes: u64,
    /// Rotated files older than this are deleted.
    pub max_age: Duration,
}

impl Retention {
    /// Default size at which the active file is rotated: 5 MiB.
    pub const DEFAULT_MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
    /// Default bound for all log files together: 25 MiB.
    pub const DEFAULT_MAX_TOTAL_BYTES: u64 = 25 * 1024 * 1024;
    /// Default maximum age of a rotated file: 14 days.
    pub const DEFAULT_MAX_AGE: Duration = Duration::from_secs(14 * 24 * 60 * 60);

    /// Space left for rotated files once room for a full active file has
    /// been set aside.
    const fn rotated_budget(&self) -> u64 {
        self.max_total_bytes.saturating_sub(self.max_file_bytes)
    }
}

impl Default for Retention {
    fn default() -> Self {
        Self {
            max_file_bytes: Self::DEFAULT_MAX_FILE_BYTES,
            max_total_bytes: Self::DEFAULT_MAX_TOTAL_BYTES,
            max_age: Self::DEFAULT_MAX_AGE,
        }
    }
}

/// A log file found in the log directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogFile {
    /// Full path of the file.
    pub path: PathBuf,
    /// Size in bytes.
    pub size: u64,
    /// Time of the last write.
    pub modified: SystemTime,
    /// Whether this is the file currently written to.
    pub active: bool,
}

/// Lists the log files in `dir`, oldest first; the active file comes last.
///
/// Files that are not termx logs are ignored, so nothing else is ever
/// deleted from or exported out of the directory.
///
/// # Errors
///
/// Fails when the directory cannot be read.
pub fn log_files(dir: &Path) -> io::Result<Vec<LogFile>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let active = name == ACTIVE_FILE_NAME;
        let rotated = name.starts_with(ROTATED_PREFIX) && name.ends_with(EXTENSION);
        if !(active || rotated) {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        files.push(LogFile {
            path: entry.path(),
            size: metadata.len(),
            modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            active,
        });
    }
    // Rotated file names embed their timestamp, so the name breaks ties
    // between files modified within the same clock tick.
    files.sort_by(|a, b| (a.active, a.modified, &a.path).cmp(&(b.active, b.modified, &b.path)));
    Ok(files)
}

/// Deletes the rotated files that `retention` no longer allows and returns
/// the paths that were removed.
///
/// The active file is never deleted.
///
/// # Errors
///
/// Fails when the directory cannot be read. A file that cannot be deleted is
/// skipped.
pub fn prune(dir: &Path, retention: &Retention, now: SystemTime) -> io::Result<Vec<PathBuf>> {
    let files = log_files(dir)?;
    let mut removed = Vec::new();
    let mut kept = Vec::new();

    for file in files.into_iter().filter(|file| !file.active) {
        let age = now.duration_since(file.modified).unwrap_or(Duration::ZERO);
        if age > retention.max_age {
            if fs::remove_file(&file.path).is_ok() {
                removed.push(file.path);
            }
        } else {
            kept.push(file);
        }
    }

    let mut total: u64 = kept.iter().map(|file| file.size).sum();
    // `kept` is ordered oldest first.
    for file in kept {
        if total <= retention.rotated_budget() {
            break;
        }
        if fs::remove_file(&file.path).is_ok() {
            total -= file.size;
            removed.push(file.path);
        }
    }

    Ok(removed)
}

/// The active log file. Rotates and prunes as it is written to.
#[derive(Debug)]
pub(crate) struct RotatingFile {
    dir: PathBuf,
    retention: Retention,
    file: Option<File>,
    /// Size of the active file.
    written: u64,
}

impl RotatingFile {
    /// Creates `dir` if needed, applies the retention rules and opens the
    /// active file for appending.
    pub fn open(dir: &Path, retention: Retention) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let mut file = Self {
            dir: dir.to_owned(),
            retention,
            file: None,
            written: 0,
        };
        file.reopen()?;
        // Old files are cleaned up on a best-effort basis.
        let _ = prune(&file.dir, &file.retention, SystemTime::now());
        Ok(file)
    }

    /// Appends one complete record, rotating first if it would not fit.
    pub fn write_record(&mut self, record: &[u8]) -> io::Result<()> {
        let length = record.len() as u64;
        if self.written > 0 && self.written + length > self.retention.max_file_bytes {
            self.rotate();
        }
        if self.file.is_none() {
            self.reopen()?;
        }
        if let Some(file) = self.file.as_mut() {
            file.write_all(record)?;
            self.written += length;
        }
        Ok(())
    }

    pub fn flush(&mut self) -> io::Result<()> {
        match self.file.as_mut() {
            Some(file) => file.flush(),
            None => Ok(()),
        }
    }

    fn active_path(&self) -> PathBuf {
        self.dir.join(ACTIVE_FILE_NAME)
    }

    fn reopen(&mut self) -> io::Result<()> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.active_path())?;
        self.written = file.metadata()?.len();
        self.file = Some(file);
        Ok(())
    }

    /// Moves the active file aside. If that fails (another program may be
    /// holding the file), logging continues in the same file and rotation is
    /// attempted again with the next record.
    fn rotate(&mut self) {
        // The handle must be closed before the file can be renamed on
        // Windows.
        self.file = None;
        if fs::rename(self.active_path(), self.rotated_path()).is_ok() {
            self.written = 0;
            let _ = prune(&self.dir, &self.retention, SystemTime::now());
        }
    }

    /// A name for the file being rotated that is not taken yet. The counter
    /// keeps files rotated within the same second in order.
    fn rotated_path(&self) -> PathBuf {
        let stamp = compact_timestamp(OffsetDateTime::now_utc());
        let mut counter = 0u32;
        loop {
            let name = format!("{ROTATED_PREFIX}{stamp}-{counter:03}{EXTENSION}");
            let path = self.dir.join(name);
            if !path.exists() {
                return path;
            }
            counter += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs::{self, File};
    use std::path::Path;
    use std::time::{Duration, SystemTime};

    use super::{ACTIVE_FILE_NAME, Retention, RotatingFile, log_files, prune};

    const DAY: Duration = Duration::from_secs(24 * 60 * 60);

    fn retention(max_file_bytes: u64, max_total_bytes: u64) -> Retention {
        Retention {
            max_file_bytes,
            max_total_bytes,
            max_age: 14 * DAY,
        }
    }

    fn create(dir: &Path, name: &str, size: usize, age: Duration) {
        let path = dir.join(name);
        fs::write(&path, vec![b'x'; size]).unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(SystemTime::now() - age)
            .unwrap();
    }

    fn names(dir: &Path) -> Vec<String> {
        log_files(dir)
            .unwrap()
            .into_iter()
            .map(|file| {
                file.path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }

    fn total_size(dir: &Path) -> u64 {
        log_files(dir).unwrap().iter().map(|file| file.size).sum()
    }

    #[test]
    fn the_default_limits_are_the_documented_ones() {
        let retention = Retention::default();
        assert_eq!(retention.max_file_bytes, 5 * 1024 * 1024);
        assert_eq!(retention.max_total_bytes, 25 * 1024 * 1024);
        assert_eq!(retention.max_age, 14 * DAY);
    }

    #[test]
    fn opening_creates_the_directory_and_the_active_file() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("nested").join("logs");

        let mut file = RotatingFile::open(&dir, Retention::default()).unwrap();
        file.write_record(b"one\n").unwrap();

        assert_eq!(
            fs::read_to_string(dir.join(ACTIVE_FILE_NAME)).unwrap(),
            "one\n"
        );
    }

    #[test]
    fn records_are_appended_to_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(ACTIVE_FILE_NAME), "old\n").unwrap();

        let mut file = RotatingFile::open(dir.path(), Retention::default()).unwrap();
        file.write_record(b"new\n").unwrap();

        assert_eq!(
            fs::read_to_string(dir.path().join(ACTIVE_FILE_NAME)).unwrap(),
            "old\nnew\n"
        );
    }

    #[test]
    fn the_active_file_is_rotated_before_it_exceeds_its_limit() {
        let dir = tempfile::tempdir().unwrap();
        let mut file = RotatingFile::open(dir.path(), retention(100, 10_000)).unwrap();
        let record = [b'a'; 40];

        for _ in 0..5 {
            file.write_record(&record).unwrap();
        }

        let files = log_files(dir.path()).unwrap();
        assert_eq!(files.len(), 3, "{files:?}");
        assert!(files.iter().all(|file| file.size <= 100));
        assert!(files.last().unwrap().active);
        assert_eq!(files.iter().filter(|file| !file.active).count(), 2);
        assert_eq!(total_size(dir.path()), 200);
    }

    #[test]
    fn a_record_larger_than_the_limit_is_still_written_whole() {
        let dir = tempfile::tempdir().unwrap();
        let mut file = RotatingFile::open(dir.path(), retention(10, 1_000)).unwrap();

        file.write_record(&[b'a'; 25]).unwrap();
        file.write_record(&[b'b'; 25]).unwrap();

        let files = log_files(dir.path()).unwrap();
        assert_eq!(files.len(), 2);
        assert!(files.iter().all(|file| file.size == 25));
    }

    #[test]
    fn the_total_size_stays_bounded_however_much_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let limits = retention(1_000, 5_000);
        let mut file = RotatingFile::open(dir.path(), limits).unwrap();
        let record = [b'a'; 100];

        for _ in 0..2_000 {
            file.write_record(&record).unwrap();
            assert!(total_size(dir.path()) <= limits.max_total_bytes);
        }

        // 200 kB were written; only the newest files are left.
        let files = log_files(dir.path()).unwrap();
        assert!(files.len() <= 5, "{files:?}");
        assert!(files.last().unwrap().active);
    }

    #[test]
    fn pruning_deletes_rotated_files_older_than_the_maximum_age() {
        let dir = tempfile::tempdir().unwrap();
        create(dir.path(), "termx-20260101T000000Z.log", 10, 30 * DAY);
        create(dir.path(), "termx-20261101T000000Z.log", 10, 15 * DAY);
        create(dir.path(), "termx-20261120T000000Z.log", 10, 13 * DAY);
        create(dir.path(), ACTIVE_FILE_NAME, 10, 40 * DAY);

        let removed = prune(dir.path(), &retention(100, 10_000), SystemTime::now()).unwrap();

        assert_eq!(removed.len(), 2);
        assert_eq!(
            names(dir.path()),
            ["termx-20261120T000000Z.log", ACTIVE_FILE_NAME]
        );
    }

    #[test]
    fn pruning_deletes_the_oldest_files_until_the_total_fits() {
        let dir = tempfile::tempdir().unwrap();
        create(dir.path(), "termx-20261101T000000Z.log", 100, 4 * DAY);
        create(dir.path(), "termx-20261102T000000Z.log", 100, 3 * DAY);
        create(dir.path(), "termx-20261103T000000Z.log", 100, 2 * DAY);
        create(dir.path(), "termx-20261104T000000Z.log", 100, DAY);
        create(dir.path(), ACTIVE_FILE_NAME, 50, Duration::ZERO);

        // 300 bytes in total, of which 100 are reserved for the active file.
        prune(dir.path(), &retention(100, 300), SystemTime::now()).unwrap();

        assert_eq!(
            names(dir.path()),
            [
                "termx-20261103T000000Z.log",
                "termx-20261104T000000Z.log",
                ACTIVE_FILE_NAME
            ]
        );
    }

    #[test]
    fn files_that_are_not_termx_logs_are_never_touched() {
        let dir = tempfile::tempdir().unwrap();
        create(dir.path(), "notes.txt", 500, 100 * DAY);
        create(dir.path(), "other.log", 500, 100 * DAY);
        create(dir.path(), "termx-backup.zip", 500, 100 * DAY);
        fs::create_dir(dir.path().join("termx-directory.log")).unwrap();

        let removed = prune(dir.path(), &retention(10, 10), SystemTime::now()).unwrap();

        assert!(removed.is_empty());
        assert!(names(dir.path()).is_empty());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 4);
    }

    #[test]
    fn old_files_are_pruned_when_the_log_is_opened() {
        let dir = tempfile::tempdir().unwrap();
        create(dir.path(), "termx-20260101T000000Z.log", 10, 30 * DAY);

        let _file = RotatingFile::open(dir.path(), Retention::default()).unwrap();

        assert_eq!(names(dir.path()), [ACTIVE_FILE_NAME]);
    }
}
