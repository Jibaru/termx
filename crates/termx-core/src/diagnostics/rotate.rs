//! The log files: per-process files, size-based rotation and bounded
//! retention.
//!
//! Every process writes its own files, named
//! `termx-<UTC timestamp>-<run id>-<sequence>.log`, so several instances of
//! the application can share a log directory without writing over each other.
//! When the next record would take the current file past
//! [`Retention::max_file_bytes`], a new file with the next sequence number is
//! started. Nothing is ever renamed.
//!
//! Old files are deleted by age, by count and by total size according to
//! [`Retention`], whichever process wrote them. Only regular files with
//! exactly this name pattern are ever listed, exported or deleted; symbolic
//! links are ignored.

use std::collections::HashMap;
use std::fs::{self, File, Metadata, OpenOptions, TryLockError};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use same_file::Handle;
use time::OffsetDateTime;

use super::record::compact_timestamp;

const PREFIX: &str = "termx-";
const EXTENSION: &str = ".log";
const LOCK_EXTENSION: &str = ".lock";

/// Takes the place of the sequence number in the name of the file that gets
/// a panic record when the log file itself cannot be written.
pub(crate) const CRASH_FILE: &str = "crash";

/// The names that can take the place of the sequence number.
const SIDE_FILES: &[&str] = &[CRASH_FILE];

/// How far in the future a file may be dated before its date is distrusted.
const CLOCK_TOLERANCE: Duration = Duration::from_secs(60);

/// How many file names are tried when the expected one is taken.
const NAME_ATTEMPTS: u32 = 16;

/// Limits on the disk space, the number and the age of the log files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Retention {
    /// Size at which a new file is started.
    pub max_file_bytes: u64,
    /// Upper bound for all log files together, the current one included.
    pub max_total_bytes: u64,
    /// Files not written to for longer than this are deleted.
    pub max_age: Duration,
    /// Upper bound for the number of log files.
    pub max_files: usize,
    /// The newest files that are kept whatever their age, so that a wrong
    /// clock cannot wipe the log.
    pub min_files_kept: usize,
}

impl Retention {
    /// Default size at which a new file is started: 5 MiB.
    pub const DEFAULT_MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
    /// Default bound for all log files together: 25 MiB.
    pub const DEFAULT_MAX_TOTAL_BYTES: u64 = 25 * 1024 * 1024;
    /// Default maximum age of a log file: 14 days.
    pub const DEFAULT_MAX_AGE: Duration = Duration::from_secs(14 * 24 * 60 * 60);
    /// Default bound for the number of log files: 100.
    pub const DEFAULT_MAX_FILES: usize = 100;
    /// Default number of newest files kept whatever their age: 3.
    pub const DEFAULT_MIN_FILES_KEPT: usize = 3;

    /// Bytes written between two retention passes of a running process.
    fn prune_interval(&self) -> u64 {
        (self.max_file_bytes / 8).max(1)
    }
}

impl Default for Retention {
    fn default() -> Self {
        Self {
            max_file_bytes: Self::DEFAULT_MAX_FILE_BYTES,
            max_total_bytes: Self::DEFAULT_MAX_TOTAL_BYTES,
            max_age: Self::DEFAULT_MAX_AGE,
            max_files: Self::DEFAULT_MAX_FILES,
            min_files_kept: Self::DEFAULT_MIN_FILES_KEPT,
        }
    }
}

/// A log file found in the log directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogFile {
    /// Full path of the file.
    pub path: PathBuf,
    /// File name.
    pub name: String,
    /// Size in bytes.
    pub size: u64,
    /// Time of the last write.
    pub modified: SystemTime,
}

/// Returns the name of the log file number `sequence` of the launch `run_id`.
fn file_name(stamp: &str, run_id: &str, sequence: u32) -> String {
    format!("{PREFIX}{stamp}-{run_id}-{sequence:03}{EXTENSION}")
}

/// Whether `name` is exactly a name this module gives to a log file:
/// `termx-YYYYMMDDTHHMMSSZ-<12 hex digits>-<3 or more digits>.log`, or the
/// same with one of the [side file](CRASH_FILE) names in place of the digits.
#[must_use]
pub fn is_log_file_name(name: &str) -> bool {
    run_id_of(name).is_some()
}

/// The run id in a log file name, or `None` if `name` is not one.
fn run_id_of(name: &str) -> Option<&str> {
    let stem = name.strip_prefix(PREFIX)?.strip_suffix(EXTENSION)?;
    let mut parts = stem.split('-');
    let (Some(stamp), Some(run_id), Some(sequence), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return None;
    };

    let stamp = stamp.as_bytes();
    let stamp_ok = stamp.len() == 16
        && stamp[..8].iter().all(u8::is_ascii_digit)
        && stamp[8] == b'T'
        && stamp[9..15].iter().all(u8::is_ascii_digit)
        && stamp[15] == b'Z';
    let sequence_ok = sequence.len() >= 3 && sequence.bytes().all(|byte| byte.is_ascii_digit())
        || SIDE_FILES.contains(&sequence);

    (stamp_ok && is_run_id(run_id) && sequence_ok).then_some(run_id)
}

fn is_run_id(text: &str) -> bool {
    text.len() == 12
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Name of the file a running process holds a lock on to say that it is
/// alive: `termx-<run id>.lock`.
fn lock_file_name(run_id: &str) -> String {
    format!("{PREFIX}{run_id}{LOCK_EXTENSION}")
}

/// Whether the process that logs as `run_id` in `dir` is still running.
///
/// Every running process holds an exclusive lock on its lock file, and the
/// operating system releases it when the process ends, however it ends. A
/// lock file that can be locked is left over from a dead process and is
/// removed.
fn is_alive(dir: &Path, run_id: &str) -> bool {
    let path = dir.join(lock_file_name(run_id));
    if eligible_metadata(&path).is_none() {
        return false;
    }
    let Ok(file) = File::open(&path) else {
        return false;
    };
    match file.try_lock() {
        Err(TryLockError::WouldBlock) => true,
        Ok(()) => {
            // Close it before removing it, as Windows requires.
            drop(file);
            let _ = fs::remove_file(&path);
            false
        }
        // Locks are not supported here: assume the worst and keep the files.
        Err(TryLockError::Error(_)) => true,
    }
}

/// The metadata of `path` if it may be treated as one of our log files: a
/// regular file, not a link of any kind, that belongs to the current user.
///
/// - A symbolic link is not followed, so it cannot point the export or the
///   clean-up at a file elsewhere.
/// - A file with more than one hard link is refused where the platform says
///   how many there are (not on Windows): the same content is reachable under
///   another name, so it is not ours alone.
/// - A file of another user is refused where the platform has file owners
///   (not on Windows).
pub(crate) fn eligible_metadata(path: &Path) -> Option<Metadata> {
    let metadata = fs::symlink_metadata(path).ok()?;
    (metadata.file_type().is_file() && has_single_link(&metadata) && is_owned_by_us(&metadata))
        .then_some(metadata)
}

#[cfg(unix)]
fn has_single_link(metadata: &Metadata) -> bool {
    std::os::unix::fs::MetadataExt::nlink(metadata) == 1
}

#[cfg(not(unix))]
fn has_single_link(_metadata: &Metadata) -> bool {
    true
}

#[cfg(unix)]
fn is_owned_by_us(metadata: &Metadata) -> bool {
    is_owned_by(metadata, rustix::process::geteuid().as_raw())
}

#[cfg(unix)]
fn is_owned_by(metadata: &Metadata, user_id: u32) -> bool {
    std::os::unix::fs::MetadataExt::uid(metadata) == user_id
}

#[cfg(not(unix))]
fn is_owned_by_us(_metadata: &Metadata) -> bool {
    true
}

/// Checks that `dir` is a real directory of the current user.
///
/// A log directory that is a symbolic link would make termx write, export
/// and delete files wherever the link points.
///
/// # Errors
///
/// Fails when `dir` cannot be read, is a link or another kind of file, or
/// belongs to another user.
pub fn verify_log_dir(dir: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(dir)?;
    if !metadata.file_type().is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the log directory is a link or not a directory",
        ));
    }
    if !is_owned_by_us(&metadata) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "the log directory belongs to another user",
        ));
    }
    Ok(())
}

/// Lists the log files in `dir`, oldest first.
///
/// Only regular files whose name matches [`is_log_file_name`], that have a
/// single hard link and belong to the current user are returned. Symbolic
/// links are never followed, so nothing outside the directory can be exported
/// or deleted through it.
///
/// # Errors
///
/// Fails when the directory cannot be read or does not pass
/// [`verify_log_dir`].
pub fn log_files(dir: &Path) -> io::Result<Vec<LogFile>> {
    verify_log_dir(dir)?;
    let mut files = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str().filter(|name| is_log_file_name(name)) else {
            continue;
        };
        // Read from the file rather than from the directory listing, which on
        // Windows lags behind for a file that is open for writing.
        let path = entry.path();
        let Some(metadata) = eligible_metadata(&path) else {
            continue;
        };
        files.push(LogFile {
            path,
            name: name.to_owned(),
            size: metadata.len(),
            modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
        });
    }
    // By last write, so that the files instances are writing to right now
    // are the newest. Files written within the same clock tick fall back to
    // the name, which starts with the creation time and ends with the
    // sequence number.
    files.sort_by(|a, b| (a.modified, &a.name).cmp(&(b.modified, &b.name)));
    Ok(files)
}

/// Deletes the log files that `retention` no longer allows and returns the
/// paths that were removed.
///
/// `current` is the file the calling process is writing to. It is never
/// deleted, and room for it to grow to its full size is set aside. Also
/// never deleted:
///
/// - the newest [`Retention::min_files_kept`] files, not counting files dated
///   in the future, whose date cannot be trusted;
/// - the files of other processes that are still running. Each process
///   cleans up after itself, and the files of dead ones are fair game.
///
/// # Errors
///
/// Fails when the directory cannot be read. A file that cannot be deleted is
/// skipped.
pub fn prune(
    dir: &Path,
    retention: &Retention,
    now: SystemTime,
    current: Option<&Path>,
) -> io::Result<Vec<PathBuf>> {
    let mut files = log_files(dir)?;
    // A file dated in the future was written under a wrong clock. It is not
    // one of "the newest files": sort those first, where they are the first
    // to go when room is needed. The sort is stable, so the rest stay in
    // order.
    let from_the_future = |file: &LogFile| {
        file.modified
            .duration_since(now)
            .is_ok_and(|ahead| ahead > CLOCK_TOLERANCE)
    };
    files.sort_by_key(|file| !from_the_future(file));
    let first_protected = files.len().saturating_sub(retention.min_files_kept);

    let own_run = current
        .and_then(|current| current.file_name()?.to_str())
        .and_then(run_id_of);
    let mut alive: HashMap<String, bool> = HashMap::new();
    let mut belongs_to_another_live_process = |name: &str| {
        let Some(run_id) = run_id_of(name).filter(|run_id| Some(*run_id) != own_run) else {
            return false;
        };
        *alive
            .entry(run_id.to_owned())
            .or_insert_with(|| is_alive(dir, run_id))
    };

    let mut removed = Vec::new();
    // (file, may be deleted), oldest first.
    let mut kept: Vec<(LogFile, bool)> = Vec::with_capacity(files.len());
    for (index, file) in files.into_iter().enumerate() {
        let is_current = current.is_some_and(|current| current == file.path);
        let protected_as_recent = index >= first_protected && !from_the_future(&file);
        let deletable =
            !protected_as_recent && !is_current && !belongs_to_another_live_process(&file.name);
        let age = now.duration_since(file.modified).unwrap_or(Duration::ZERO);
        if deletable && age > retention.max_age && fs::remove_file(&file.path).is_ok() {
            removed.push(file.path);
        } else {
            kept.push((file, deletable));
        }
    }

    // The current file may still grow to its full size.
    let (reserved, current_size) = match current {
        Some(current) => (
            retention.max_file_bytes,
            kept.iter()
                .find(|(file, _)| file.path == current)
                .map_or(0, |(file, _)| file.size),
        ),
        None => (0, 0),
    };
    let budget = retention.max_total_bytes.saturating_sub(reserved);
    let mut others: u64 = kept.iter().map(|(file, _)| file.size).sum::<u64>() - current_size;
    let mut count = kept.len();

    for (file, deletable) in kept {
        if count <= retention.max_files && others <= budget {
            break;
        }
        if deletable && fs::remove_file(&file.path).is_ok() {
            others -= file.size;
            count -= 1;
            removed.push(file.path);
        }
    }

    Ok(removed)
}

/// The log file of this process. Starts new files and prunes as it is
/// written to.
#[derive(Debug)]
pub(crate) struct RotatingFile {
    dir: PathBuf,
    retention: Retention,
    run_id: String,
    /// Time the launch started logging, shared by the names of all its files.
    stamp: String,
    /// Sequence number of the next file to create.
    next_sequence: u32,
    /// The file being written to.
    path: PathBuf,
    file: Option<File>,
    /// Held locked for as long as this process logs, so that other instances
    /// know its files are in use and leave them alone.
    alive_lock: Option<File>,
    /// What the open file is on disk (device and inode, or the Windows
    /// equivalent), to notice when the path stops referring to it.
    identity: Option<Handle>,
    /// Size of the current file.
    written: u64,
    written_since_prune: u64,
    /// Makes starting a new file fail, to test the fallback.
    #[cfg(test)]
    pub fail_new_files: bool,
}

impl RotatingFile {
    /// Creates `dir` if needed, starts the first file of the launch `run_id`
    /// and applies the retention rules.
    pub fn open(dir: &Path, retention: Retention, run_id: &str) -> io::Result<Self> {
        let mut file = Self {
            dir: dir.to_owned(),
            retention,
            run_id: run_id.to_owned(),
            stamp: compact_timestamp(OffsetDateTime::now_utc()),
            next_sequence: 0,
            path: PathBuf::new(),
            file: None,
            alive_lock: None,
            identity: None,
            written: 0,
            written_since_prune: 0,
            #[cfg(test)]
            fail_new_files: false,
        };
        file.start_new_file()?;
        file.hold_alive_lock();
        file.prune();
        Ok(file)
    }

    /// Creates and locks the file that tells other instances this process is
    /// alive. Without it logging still works; the files are then only
    /// protected by the other retention rules.
    fn hold_alive_lock(&mut self) {
        let path = self.dir.join(lock_file_name(&self.run_id));
        let lock = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .ok()
            .filter(|file| file.try_lock().is_ok());
        self.alive_lock = lock;
    }

    /// The file being written to.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "used by the tests of this module")
    )]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Appends one complete record, starting a new file first if it would
    /// not fit.
    #[cfg(test)]
    pub fn write_record(&mut self, record: &[u8]) -> io::Result<()> {
        self.write_batch(std::iter::once(record))
    }

    /// Appends several complete records.
    ///
    /// Whether the file is still the one on disk is checked once per batch:
    /// the check opens the path, which is cheap but not free.
    pub fn write_batch<'a>(
        &mut self,
        records: impl IntoIterator<Item = &'a [u8]>,
    ) -> io::Result<()> {
        // If the file was deleted or replaced while the application runs,
        // writing on would go into a file nobody can see.
        if self.file.is_none() || !self.is_intact() {
            self.file = None;
            self.start_new_file()?;
        }

        let mut result = Ok(());
        for record in records {
            let length = record.len() as u64;
            if self.written > 0 && self.written + length > self.retention.max_file_bytes {
                self.rotate(length);
            }
            if let Some(file) = self.file.as_mut() {
                // Keep going after a failed record: the next one may fit.
                match file.write_all(record) {
                    Ok(()) => {
                        self.written += length;
                        self.written_since_prune += length;
                    }
                    Err(error) => result = Err(error),
                }
            }
        }
        if self.written_since_prune >= self.retention.prune_interval() {
            self.prune();
        }
        result
    }

    /// Whether the path of the current file still leads to the file that is
    /// open: a regular file with the same identity. It does not after the
    /// file was deleted, or replaced by a link or by another file.
    fn is_intact(&self) -> bool {
        let regular =
            fs::symlink_metadata(&self.path).is_ok_and(|metadata| metadata.file_type().is_file());
        regular && self.identity.is_some() && Handle::from_path(&self.path).ok() == self.identity
    }

    /// Path of the side file `kind` of this launch, such as [`CRASH_FILE`].
    pub fn side_file_path(&self, kind: &str) -> PathBuf {
        self.dir.join(format!(
            "{PREFIX}{}-{}-{kind}{EXTENSION}",
            self.stamp, self.run_id
        ))
    }

    /// Asks the operating system to write the file to the disk.
    pub fn sync(&mut self) -> io::Result<()> {
        match self.file.as_mut() {
            Some(file) => file.sync_all(),
            None => Ok(()),
        }
    }

    fn prune(&mut self) {
        self.written_since_prune = 0;
        // Old files are cleaned up on a best-effort basis.
        let _ = prune(
            &self.dir,
            &self.retention,
            SystemTime::now(),
            Some(&self.path),
        );
    }

    /// Starts the next file. If that is not possible (a full disk, a
    /// directory that became read-only), logging continues in the current
    /// file, which is emptied once it reaches twice its limit so that it
    /// cannot grow without bound.
    fn rotate(&mut self, incoming: u64) {
        if self.start_new_file().is_ok() {
            self.prune();
            return;
        }
        let hard_limit = self.retention.max_file_bytes.saturating_mul(2);
        if self.written + incoming > hard_limit && self.truncate().is_ok() {
            self.written = 0;
        }
    }

    /// Empties the current file. The handle used for logging is opened for
    /// appending only, which on Windows does not allow changing the length,
    /// so this goes through a second handle; appending then continues at the
    /// new end.
    fn truncate(&self) -> io::Result<()> {
        OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&self.path)
            .map(drop)
    }

    /// Creates the next file of this launch and makes it the current one.
    /// On failure the current file, if any, stays in use.
    fn start_new_file(&mut self) -> io::Result<()> {
        #[cfg(test)]
        if self.fail_new_files {
            return Err(io::Error::other("starting a new file is disabled"));
        }

        fs::create_dir_all(&self.dir)?;
        verify_log_dir(&self.dir)?;
        let mut last_error = None;
        for _ in 0..NAME_ATTEMPTS {
            let path = self
                .dir
                .join(file_name(&self.stamp, &self.run_id, self.next_sequence));
            self.next_sequence += 1;
            // `create_new` never opens something that already exists, which
            // includes a symbolic link planted under the expected name.
            match OpenOptions::new().append(true).create_new(true).open(&path) {
                Ok(file) => {
                    self.identity = file.try_clone().and_then(Handle::from_file).ok();
                    self.file = Some(file);
                    self.path = path;
                    self.written = 0;
                    return Ok(());
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    last_error = Some(error);
                }
                Err(error) => return Err(error),
            }
        }
        Err(last_error.unwrap_or_else(|| io::Error::other("no free log file name")))
    }
}

impl Drop for RotatingFile {
    fn drop(&mut self) {
        // Closing the file releases the lock; then the file can go. After a
        // crash the next instance that prunes removes it instead.
        if self.alive_lock.take().is_some() {
            let _ = fs::remove_file(self.dir.join(lock_file_name(&self.run_id)));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs::{self, File};
    use std::path::Path;
    use std::time::{Duration, SystemTime};

    use super::{Retention, RotatingFile, is_log_file_name, log_files, prune};

    const DAY: Duration = Duration::from_secs(24 * 60 * 60);
    const RUN: &str = "5f0c2a9be771";
    const OTHER_RUN: &str = "00aa11bb22cc";

    fn retention(max_file_bytes: u64, max_total_bytes: u64) -> Retention {
        Retention {
            max_file_bytes,
            max_total_bytes,
            max_age: 14 * DAY,
            max_files: 1_000,
            min_files_kept: 0,
        }
    }

    /// A valid log file name for the given day of November 2026.
    fn name(day: u32) -> String {
        format!("termx-202611{day:02}T000000Z-{RUN}-000.log")
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
            .map(|file| file.name)
            .collect()
    }

    fn total_size(dir: &Path) -> u64 {
        log_files(dir).unwrap().iter().map(|file| file.size).sum()
    }

    fn contents(dir: &Path) -> String {
        log_files(dir)
            .unwrap()
            .iter()
            .map(|file| fs::read_to_string(&file.path).unwrap())
            .collect()
    }

    #[test]
    fn the_default_limits_are_the_documented_ones() {
        let retention = Retention::default();
        assert_eq!(retention.max_file_bytes, 5 * 1024 * 1024);
        assert_eq!(retention.max_total_bytes, 25 * 1024 * 1024);
        assert_eq!(retention.max_age, 14 * DAY);
        assert_eq!(retention.max_files, 100);
        assert_eq!(retention.min_files_kept, 3);
    }

    #[test]
    fn only_names_produced_by_the_writer_are_log_files() {
        assert!(is_log_file_name(
            "termx-20261126T190411Z-5f0c2a9be771-000.log"
        ));
        assert!(is_log_file_name(
            "termx-20261126T190411Z-5f0c2a9be771-1234.log"
        ));

        for name in [
            "termx.log",
            "termx-backup.log",
            "termx-20261126T190411Z.log",
            "termx-20261126T190411Z-5f0c2a9be771.log",
            "termx-20261126T190411Z-5f0c2a9be771-00.log",
            "termx-20261126T190411Z-5f0c2a9be771-other.log",
            "termx-20261126T190411Z-5F0C2A9BE771-000.log",
            "termx-20261126T190411Z-5f0c2a9be771-000.log.bak",
            "termx-20261126X190411Z-5f0c2a9be771-000.log",
            "termx-20261126T190411Z-5f0c2a9be771-000-1.log",
            "other-20261126T190411Z-5f0c2a9be771-000.log",
            "termx-2026112éT190411Z-5f0c2a9be771-000.log",
        ] {
            assert!(!is_log_file_name(name), "{name}");
        }
    }

    #[test]
    fn opening_creates_the_directory_and_a_file_named_after_the_launch() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("nested").join("logs");

        let mut file = RotatingFile::open(&dir, Retention::default(), RUN).unwrap();
        file.write_record(b"one\n").unwrap();

        let files = log_files(&dir).unwrap();
        assert_eq!(files.len(), 1);
        assert!(
            files[0].name.ends_with(&format!("-{RUN}-000.log")),
            "{}",
            files[0].name
        );
        assert_eq!(files[0].path, file.path());
        assert_eq!(contents(&dir), "one\n");
    }

    #[test]
    fn a_new_file_is_started_before_the_current_one_exceeds_its_limit() {
        let dir = tempfile::tempdir().unwrap();
        let mut file = RotatingFile::open(dir.path(), retention(100, 10_000), RUN).unwrap();
        let record = [b'a'; 40];

        for _ in 0..5 {
            file.write_record(&record).unwrap();
        }

        let files = log_files(dir.path()).unwrap();
        assert_eq!(files.len(), 3, "{files:?}");
        assert_eq!(
            files.iter().map(|file| file.size).collect::<Vec<_>>(),
            [80, 80, 40]
        );
        assert!(files[1].name.ends_with("-001.log"));
        assert_eq!(files[2].path, file.path());
    }

    #[test]
    fn a_record_larger_than_the_limit_is_still_written_whole() {
        let dir = tempfile::tempdir().unwrap();
        let mut file = RotatingFile::open(dir.path(), retention(10, 1_000), RUN).unwrap();

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
        let mut file = RotatingFile::open(dir.path(), limits, RUN).unwrap();
        let record = [b'a'; 100];

        for _ in 0..2_000 {
            file.write_record(&record).unwrap();
            assert!(total_size(dir.path()) <= limits.max_total_bytes);
        }

        // 200 kB were written; only the newest files are left.
        let files = log_files(dir.path()).unwrap();
        assert!(files.len() <= 5, "{files:?}");
        assert_eq!(files.last().unwrap().path, file.path());
    }

    #[test]
    fn two_instances_write_separate_files_and_stay_within_the_limits_together() {
        let dir = tempfile::tempdir().unwrap();
        let limits = retention(1_000, 5_000);
        let mut first = RotatingFile::open(dir.path(), limits, RUN).unwrap();
        let mut second = RotatingFile::open(dir.path(), limits, OTHER_RUN).unwrap();
        assert_ne!(first.path(), second.path());

        for index in 0..2_000 {
            first
                .write_record(format!("first {index:>93}\n").as_bytes())
                .unwrap();
            second
                .write_record(format!("second {index:>92}\n").as_bytes())
                .unwrap();
            // Each instance may be up to one retention pass behind.
            let slack = 4 * limits.prune_interval();
            assert!(total_size(dir.path()) <= limits.max_total_bytes + slack);
        }

        // No record of one instance ended up in a file of the other, and
        // every line is whole.
        for file in log_files(dir.path()).unwrap() {
            let owner = if file.name.contains(RUN) {
                "first"
            } else {
                "second"
            };
            let text = fs::read_to_string(&file.path).unwrap();
            assert!(
                text.lines()
                    .all(|line| line.starts_with(owner) && line.len() == 99)
            );
        }
        // The newest record of both instances is still there.
        let all = contents(dir.path());
        assert!(all.contains(&format!("first {:>93}\n", 1_999)));
        assert!(all.contains(&format!("second {:>92}\n", 1_999)));
    }

    #[test]
    fn a_deleted_file_is_replaced_and_logging_continues() {
        let dir = tempfile::tempdir().unwrap();
        let mut file = RotatingFile::open(dir.path(), Retention::default(), RUN).unwrap();
        file.write_record(b"before\n").unwrap();

        fs::remove_file(file.path()).unwrap();
        file.write_record(b"after\n").unwrap();

        assert_eq!(contents(dir.path()), "after\n");
        assert!(file.path().is_file());
    }

    #[test]
    fn a_file_renamed_over_the_log_is_noticed_and_no_record_is_lost() {
        let dir = tempfile::tempdir().unwrap();
        let mut file = RotatingFile::open(dir.path(), Retention::default(), RUN).unwrap();
        file.write_record(b"one\n").unwrap();
        let original = file.path().to_owned();

        // Another regular file takes the place of the log file. The open
        // handle now refers to a file that no longer has a name.
        let intruder = dir.path().join("intruder.tmp");
        fs::write(&intruder, "").unwrap();
        fs::rename(&intruder, &original).unwrap();
        file.write_record(b"two\n").unwrap();
        file.write_record(b"three\n").unwrap();

        assert_ne!(file.path(), original, "a new file should have been started");
        assert_eq!(fs::read_to_string(file.path()).unwrap(), "two\nthree\n");
        assert_eq!(fs::read_to_string(&original).unwrap(), "");
    }

    #[test]
    fn a_file_planted_under_the_next_name_is_never_opened() {
        let dir = tempfile::tempdir().unwrap();
        let mut file = RotatingFile::open(dir.path(), retention(100, 10_000), RUN).unwrap();
        // All files of a launch share its timestamp, so the next name is
        // known in advance, to an attacker too.
        let current = file.path().to_string_lossy().into_owned();
        assert!(current.ends_with("-000.log"));
        let next = current.replace("-000.log", "-001.log");
        fs::write(&next, "planted\n").unwrap();

        for _ in 0..5 {
            file.write_record(&[b'a'; 40]).unwrap();
        }

        assert_eq!(fs::read_to_string(&next).unwrap(), "planted\n");
        assert!(file.path().to_string_lossy().ends_with("-003.log"));
    }

    #[test]
    fn a_deleted_directory_is_recreated_and_logging_continues() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("logs");
        let mut file = RotatingFile::open(&dir, Retention::default(), RUN).unwrap();
        file.write_record(b"before\n").unwrap();

        // The open handle can keep the directory alive on Windows: failing
        // to remove it there is fine, the file itself is what matters.
        let _ = fs::remove_file(file.path());
        let _ = fs::remove_dir_all(&dir);
        file.write_record(b"after\n").unwrap();

        assert_eq!(contents(&dir), "after\n");
    }

    #[test]
    fn the_file_is_emptied_when_no_new_file_can_be_started() {
        let dir = tempfile::tempdir().unwrap();
        let mut file = RotatingFile::open(dir.path(), retention(100, 10_000), RUN).unwrap();
        file.fail_new_files = true;

        for index in 0..1_000u32 {
            file.write_record(format!("{index:>9}\n").as_bytes())
                .unwrap();
            assert!(
                total_size(dir.path()) <= 200,
                "the file grew past twice its limit"
            );
        }

        // Logging went on in the same file, and the newest record is there.
        assert_eq!(log_files(dir.path()).unwrap().len(), 1);
        assert!(contents(dir.path()).ends_with("      999\n"));
    }

    #[test]
    fn pruning_deletes_files_older_than_the_maximum_age() {
        let dir = tempfile::tempdir().unwrap();
        create(dir.path(), &name(1), 10, 30 * DAY);
        create(dir.path(), &name(2), 10, 15 * DAY);
        create(dir.path(), &name(3), 10, 13 * DAY);
        create(dir.path(), &name(4), 10, DAY);

        let removed = prune(dir.path(), &retention(100, 10_000), SystemTime::now(), None).unwrap();

        assert_eq!(removed.len(), 2);
        assert_eq!(names(dir.path()), [name(3), name(4)]);
    }

    #[test]
    fn the_newest_files_survive_a_clock_that_jumped_forward() {
        let dir = tempfile::tempdir().unwrap();
        for day in 1..=6 {
            create(dir.path(), &name(day), 10, DAY);
        }
        let mut limits = retention(100, 10_000);
        limits.min_files_kept = 3;

        // A year in the future, every file looks expired.
        let future = SystemTime::now() + 365 * DAY;
        prune(dir.path(), &limits, future, None).unwrap();

        assert_eq!(names(dir.path()), [name(4), name(5), name(6)]);
    }

    #[test]
    fn files_dated_in_the_future_do_not_count_as_the_newest() {
        let dir = tempfile::tempdir().unwrap();
        // Three files written while the clock was a month ahead...
        for day in 1..=3 {
            create(dir.path(), &name(day), 10, Duration::ZERO);
            File::options()
                .write(true)
                .open(dir.path().join(name(day)))
                .unwrap()
                .set_modified(SystemTime::now() + 30 * DAY)
                .unwrap();
        }
        // ...and four real ones, all past the maximum age.
        for day in 4..=7 {
            create(dir.path(), &name(day), 10, (40 - day) * DAY);
        }
        let mut limits = retention(100, 10_000);
        limits.min_files_kept = 3;

        prune(dir.path(), &limits, SystemTime::now(), None).unwrap();

        // The three newest real files are the ones that are kept.
        let left = names(dir.path());
        for day in 5..=7 {
            assert!(left.contains(&name(day)), "{left:?}");
        }
        assert!(!left.contains(&name(4)));
    }

    #[test]
    fn a_busy_instance_does_not_prune_the_files_of_an_idle_live_one() {
        let dir = tempfile::tempdir().unwrap();
        let limits = retention(1_000, 3_000);
        let mut idle = RotatingFile::open(dir.path(), limits, OTHER_RUN).unwrap();
        idle.write_record(b"the idle instance was here\n").unwrap();
        let idle_file = idle.path().to_owned();
        // Make it the oldest file by far, as after a long idle period.
        File::options()
            .write(true)
            .open(&idle_file)
            .unwrap()
            .set_modified(SystemTime::now() - 30 * DAY)
            .unwrap();

        let mut busy = RotatingFile::open(dir.path(), limits, RUN).unwrap();
        for _ in 0..500 {
            busy.write_record(&[b'b'; 100]).unwrap();
        }

        assert!(
            idle_file.is_file(),
            "the file of a live instance was pruned"
        );
        idle.write_record(b"and still is\n").unwrap();
        assert_eq!(idle.path(), idle_file);
        assert!(
            fs::read_to_string(&idle_file)
                .unwrap()
                .starts_with("the idle instance was here\n")
        );

        // Once it is gone, its files are ordinary old files.
        drop(idle);
        File::options()
            .write(true)
            .open(&idle_file)
            .unwrap()
            .set_modified(SystemTime::now() - 30 * DAY)
            .unwrap();
        prune(dir.path(), &limits, SystemTime::now(), Some(busy.path())).unwrap();
        assert!(!idle_file.exists());
    }

    #[test]
    fn the_files_of_a_dead_instance_are_pruned_and_its_lock_file_removed() {
        let dir = tempfile::tempdir().unwrap();
        let dead = format!("termx-20261101T000000Z-{OTHER_RUN}-000.log");
        create(dir.path(), &dead, 10, 30 * DAY);
        // The lock file of a process that died without cleaning up.
        let lock = dir.path().join(format!("termx-{OTHER_RUN}.lock"));
        fs::write(&lock, "").unwrap();

        prune(dir.path(), &retention(100, 10_000), SystemTime::now(), None).unwrap();

        assert!(names(dir.path()).is_empty());
        assert!(!lock.exists());
    }

    #[test]
    fn the_alive_lock_file_is_removed_when_logging_ends() {
        let dir = tempfile::tempdir().unwrap();
        let lock = dir.path().join(format!("termx-{RUN}.lock"));

        let file = RotatingFile::open(dir.path(), Retention::default(), RUN).unwrap();
        assert!(lock.is_file());
        drop(file);

        assert!(!lock.exists());
    }

    #[test]
    fn the_current_file_is_never_deleted() {
        let dir = tempfile::tempdir().unwrap();
        create(dir.path(), &name(1), 500, 100 * DAY);
        create(dir.path(), &name(2), 500, 100 * DAY);
        let current = dir.path().join(name(1));

        prune(
            dir.path(),
            &retention(10, 10),
            SystemTime::now(),
            Some(&current),
        )
        .unwrap();

        assert_eq!(names(dir.path()), [name(1)]);
    }

    #[test]
    fn pruning_deletes_the_oldest_files_until_the_total_fits() {
        let dir = tempfile::tempdir().unwrap();
        for day in 1..=4 {
            create(dir.path(), &name(day), 100, DAY);
        }
        create(dir.path(), &name(5), 50, Duration::ZERO);
        let current = dir.path().join(name(5));

        // 300 bytes in total, of which 100 are reserved for the current file.
        prune(
            dir.path(),
            &retention(100, 300),
            SystemTime::now(),
            Some(&current),
        )
        .unwrap();

        assert_eq!(names(dir.path()), [name(3), name(4), name(5)]);
    }

    #[test]
    fn pruning_deletes_the_oldest_files_beyond_the_maximum_count() {
        let dir = tempfile::tempdir().unwrap();
        for day in 1..=6 {
            create(dir.path(), &name(day), 10, DAY);
        }
        let mut limits = retention(100, 10_000);
        limits.max_files = 2;

        prune(dir.path(), &limits, SystemTime::now(), None).unwrap();

        assert_eq!(names(dir.path()), [name(5), name(6)]);
    }

    #[test]
    fn files_that_are_not_termx_logs_are_never_touched() {
        let dir = tempfile::tempdir().unwrap();
        create(dir.path(), "notes.txt", 500, 100 * DAY);
        create(dir.path(), "other.log", 500, 100 * DAY);
        create(dir.path(), "termx.log", 500, 100 * DAY);
        create(dir.path(), "termx-backup.log", 500, 100 * DAY);
        fs::create_dir(dir.path().join(name(1))).unwrap();

        let removed = prune(dir.path(), &retention(10, 10), SystemTime::now(), None).unwrap();

        assert!(removed.is_empty());
        assert!(names(dir.path()).is_empty());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 5);
    }

    #[cfg(unix)]
    #[test]
    fn a_hard_link_to_another_file_is_not_a_log_file() {
        let dir = tempfile::tempdir().unwrap();
        let private = dir.path().join("private.txt");
        fs::write(&private, "private").unwrap();
        fs::hard_link(&private, dir.path().join(name(1))).unwrap();
        create(dir.path(), &name(2), 10, DAY);

        assert_eq!(names(dir.path()), [name(2)]);
        prune(dir.path(), &retention(1, 1), SystemTime::now(), None).unwrap();
        assert_eq!(fs::read_to_string(&private).unwrap(), "private");
        assert!(dir.path().join(name(1)).exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_file_of_another_user_is_not_ours() {
        let dir = tempfile::tempdir().unwrap();
        create(dir.path(), &name(1), 10, DAY);
        let metadata = fs::symlink_metadata(dir.path().join(name(1))).unwrap();
        let us = rustix::process::geteuid().as_raw();

        assert!(super::is_owned_by(&metadata, us));
        assert!(!super::is_owned_by(&metadata, us.wrapping_add(1)));
    }

    #[cfg(unix)]
    #[test]
    fn a_log_directory_that_is_a_link_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let elsewhere = root.path().join("elsewhere");
        fs::create_dir(&elsewhere).unwrap();
        create(&elsewhere, &name(1), 10, DAY);
        let link = root.path().join("logs");
        std::os::unix::fs::symlink(&elsewhere, &link).unwrap();

        assert!(log_files(&link).is_err());
        assert!(RotatingFile::open(&link, Retention::default(), RUN).is_err());
        assert_eq!(
            fs::read_dir(&elsewhere).unwrap().count(),
            1,
            "nothing was written there"
        );
        // The real directory is fine.
        assert_eq!(names(&elsewhere), [name(1)]);
    }

    #[cfg(unix)]
    #[test]
    fn symbolic_links_are_neither_listed_nor_followed() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("private.txt");
        fs::write(&target, "private").unwrap();
        std::os::unix::fs::symlink(&target, dir.path().join(name(1))).unwrap();

        assert!(names(dir.path()).is_empty());
        let removed = prune(dir.path(), &retention(1, 1), SystemTime::now(), None).unwrap();
        assert!(removed.is_empty());
        assert_eq!(fs::read_to_string(&target).unwrap(), "private");
    }

    #[cfg(unix)]
    #[test]
    fn a_symbolic_link_under_the_expected_name_is_not_written_through() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("victim.txt");
        fs::write(&target, "untouched").unwrap();
        let mut file = RotatingFile::open(dir.path(), Retention::default(), RUN).unwrap();

        // Replace the current file with a link to a file elsewhere.
        fs::remove_file(file.path()).unwrap();
        std::os::unix::fs::symlink(&target, file.path()).unwrap();
        file.write_record(b"record\n").unwrap();

        assert_eq!(fs::read_to_string(&target).unwrap(), "untouched");
        assert_eq!(contents(dir.path()), "record\n");
    }

    #[test]
    fn old_files_are_pruned_when_the_log_is_opened() {
        let dir = tempfile::tempdir().unwrap();
        create(dir.path(), &name(1), 10, 30 * DAY);
        let mut limits = retention(100, 10_000);
        limits.min_files_kept = 1;

        let file = RotatingFile::open(dir.path(), limits, RUN).unwrap();

        assert_eq!(log_files(dir.path()).unwrap().len(), 1);
        assert!(file.path().is_file());
    }
}
