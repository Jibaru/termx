//! The state file: what termx remembers between two runs.
//!
//! One JSON file, [`STATE_FILE`], in a directory chosen by the application.
//! It holds the sessions, the order of the projects, the layout, the recent
//! folders and the geometry of the window. It never holds what was printed
//! in a terminal: scrollback is not persisted.
//!
//! # Writing
//!
//! A save never leaves a half-written file behind. The new content goes to
//! a temporary file next to the state file, is flushed to the disk
//! (`fsync`), and replaces the state file with one `rename`, which is
//! atomic; the directory is then flushed as well where the system allows
//! it. A crash at any point leaves either the old file or the new one, and
//! at worst a temporary file that the next run removes.
//!
//! # Two instances of termx
//!
//! The first instance to open the directory takes a lock
//! ([`LOCK_FILE`], an advisory lock the system releases when the process
//! ends, however it ends) and becomes the [`Primary`](PersistenceMode::Primary):
//! it restores the state and saves it. An instance that finds the lock
//! taken is a [`Secondary`](PersistenceMode::Secondary): it starts without
//! sessions, only borrows the recent folders, and **never writes**.
//!
//! A lock was chosen over letting the last writer win because the state is
//! not mergeable: two instances that both restored the same sessions would
//! both offer to resume the same Claude Code conversation, and whichever
//! quit last would erase the sessions of the other. With the lock there is
//! one owner and no lost update; the price is that what is opened in a
//! second instance is not there after a restart.
//!
//! # A file that cannot be used
//!
//! Whatever is in the file, loading never fails and never panics, and
//! nothing is thrown away without a copy:
//!
//! | File                                  | What happens                                   |
//! | ------------------------------------- | ---------------------------------------------- |
//! | Missing                               | Clean start.                                   |
//! | Not JSON, or without a version        | Moved to `state.corrupt-<time>.json`.          |
//! | From a newer termx (version too high) | Copied to `state.future-v<n>-<time>.json`.     |
//! | From an older termx                   | Copied to `state.v<n>-<time>.json`, migrated.  |
//! | Valid but with entries termx rejects  | Copied to `state.partial-<time>.json`.         |
//! | Cannot be read, or the copy fails     | Left alone: nothing is saved during this run.  |
//!
//! Each of these logs one warning. The [`MAX_BACKUPS`] newest copies are
//! kept.
//!
//! # Versions
//!
//! The file has a `version`. [`CURRENT_VERSION`] is what this build
//! writes. To change the format, raise it and add a function to
//! `MIGRATIONS` that turns a file of the previous version into the new
//! one.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::layout::Layout;
use super::model::{SessionKey, SessionKind, now_ms};
use super::project::{GitInfo, Project};
use crate::diagnostics::tracing::{debug, warn};
use crate::pty::lock;

/// Name of the state file.
pub const STATE_FILE: &str = "state.json";

/// Name of the file whose lock marks the instance that owns the state.
pub const LOCK_FILE: &str = "state.lock";

/// The version of the format this build reads and writes.
pub const CURRENT_VERSION: u32 = 1;

/// How many backup copies of state files are kept.
pub const MAX_BACKUPS: usize = 5;

/// How many recent folders are remembered.
pub const MAX_RECENT_FOLDERS: usize = 20;

/// How many sessions are restored from a file. A file with more was not
/// written by termx; the rest is ignored (and the file backed up).
pub const MAX_SESSIONS: usize = 1000;

/// Size above which a state file is not read at all, in bytes.
const MAX_STATE_BYTES: u64 = 16 * 1024 * 1024;

/// Turns a file of one version into the next one.
type Migration = fn(Value) -> Result<Value, String>;

/// The migrations, in order: the function at index `n` reads version `n + 1`
/// and produces version `n + 2`. Empty while version 1 is current.
const MIGRATIONS: &[Migration] = &[];

/// Whether the state is saved, and why not when it is not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "mode", rename_all = "camelCase")]
#[non_exhaustive]
pub enum PersistenceMode {
    /// This instance owns the state file: it restored it and saves it.
    Primary,
    /// Another instance of termx owns the state file. Nothing is saved.
    Secondary,
    /// Nothing is saved, because the state file could not be read or
    /// backed up. It is left as it is.
    Disabled {
        /// Why, for the user.
        reason: String,
    },
    /// The workspace was opened without a directory. Nothing is saved.
    Memory,
}

/// The position and size of the window, in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowGeometry {
    /// Horizontal position of the top-left corner.
    pub x: f64,
    /// Vertical position of the top-left corner.
    pub y: f64,
    /// Width of the content of the window.
    pub width: f64,
    /// Height of the content of the window.
    pub height: f64,
    /// Whether the window was maximized.
    #[serde(default)]
    pub maximized: bool,
}

impl WindowGeometry {
    /// Whether every number is finite and the size is positive.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        [self.x, self.y, self.width, self.height]
            .iter()
            .all(|value| value.is_finite())
            && self.width >= 1.0
            && self.height >= 1.0
    }
}

/// What is remembered of one session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredSession {
    /// The identifier of the session.
    pub id: SessionKey,
    /// What the session runs.
    pub kind: SessionKind,
    /// The folder the session is started in.
    pub cwd: PathBuf,
    /// The command line of a background session.
    #[serde(default)]
    pub command: Option<String>,
    /// The name the user gave.
    #[serde(default)]
    pub name: Option<String>,
    /// The last title the program set.
    #[serde(default)]
    pub auto_title: Option<String>,
    /// The project the session was last grouped under, so that the sidebar
    /// looks the same at once after a restart.
    #[serde(default)]
    pub project: Option<Project>,
    /// The branch and worktree last seen.
    #[serde(default)]
    pub git: Option<GitInfo>,
    /// When the session was created.
    #[serde(default)]
    pub created_at: u64,
    /// When the session was last active.
    #[serde(default)]
    pub last_activity_at: u64,
    /// The identifier of the Claude Code conversation, when it is known.
    #[serde(default)]
    pub claude_session_id: Option<String>,
}

/// Everything that is remembered between two runs.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PersistedState {
    /// The sessions, in the order the user gave them.
    pub sessions: Vec<StoredSession>,
    /// The keys of the projects, in the order the user gave them.
    pub project_order: Vec<String>,
    /// The layout of the main area. `None` in a file that has none.
    pub layout: Option<Layout>,
    /// The folders sessions were created in, most recent first.
    pub recent_folders: Vec<PathBuf>,
    /// The geometry of the window.
    pub window: Option<WindowGeometry>,
}

/// The layout of the file. Entries are read one by one, so that one bad
/// entry does not cost the others.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FileOut<'a> {
    version: u32,
    saved_at: u64,
    sessions: &'a [StoredSession],
    project_order: &'a [String],
    layout: &'a Option<Layout>,
    recent_folders: Vec<String>,
    window: &'a Option<WindowGeometry>,
}

/// The state file of one directory, and the right to write it.
#[derive(Debug)]
pub struct Store {
    dir: Option<PathBuf>,
    mode: PersistenceMode,
    /// Holds the lock of a primary instance for as long as it lives.
    _lock: Option<File>,
    /// The content last written or read, to skip saves that change nothing.
    last_written: Mutex<Option<Vec<u8>>>,
}

impl Store {
    /// A store that remembers nothing.
    #[must_use]
    pub fn in_memory() -> Self {
        Self {
            dir: None,
            mode: PersistenceMode::Memory,
            _lock: None,
            last_written: Mutex::new(None),
        }
    }

    /// Opens the state of `dir`, creating the directory if needed, and
    /// returns what was remembered. See the [module documentation](self)
    /// for what happens to a file that cannot be used. Never fails: the
    /// [`mode`](Self::mode) tells whether the state will be saved.
    #[must_use]
    pub fn open(dir: &Path) -> (Self, PersistedState) {
        Self::open_with(dir, MIGRATIONS, CURRENT_VERSION)
    }

    fn open_with(dir: &Path, migrations: &[Migration], current: u32) -> (Self, PersistedState) {
        let disabled = |reason: String| Self {
            dir: Some(dir.to_owned()),
            mode: PersistenceMode::Disabled { reason },
            _lock: None,
            last_written: Mutex::new(None),
        };

        if let Err(error) = fs::create_dir_all(dir) {
            warn!(dir = %dir.display(), error = %error, "the state directory cannot be created; nothing will be saved");
            return (
                disabled(format!("the state directory cannot be created: {error}")),
                PersistedState::default(),
            );
        }

        let lock = match acquire_lock(&dir.join(LOCK_FILE)) {
            LockOutcome::Acquired(file) => Some(file),
            LockOutcome::Unsupported => None,
            LockOutcome::Taken => {
                warn!(
                    dir = %dir.display(),
                    "another instance of termx owns the state; this one will not save"
                );
                // Only the recent folders are borrowed, read-only.
                let borrowed = fs::read(dir.join(STATE_FILE))
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                    .map(|value| parse(&value).0)
                    .unwrap_or_default();
                let store = Self {
                    dir: Some(dir.to_owned()),
                    mode: PersistenceMode::Secondary,
                    _lock: None,
                    last_written: Mutex::new(None),
                };
                return (
                    store,
                    PersistedState {
                        recent_folders: borrowed.recent_folders,
                        ..PersistedState::default()
                    },
                );
            }
        };

        remove_stale_temporaries(dir);
        match load(dir, migrations, current) {
            Ok((state, bytes)) => {
                prune_backups(dir);
                let store = Self {
                    dir: Some(dir.to_owned()),
                    mode: PersistenceMode::Primary,
                    _lock: lock,
                    last_written: Mutex::new(bytes),
                };
                (store, state)
            }
            Err(reason) => (disabled(reason), PersistedState::default()),
        }
    }

    /// Whether the state is saved.
    #[must_use]
    pub fn mode(&self) -> &PersistenceMode {
        &self.mode
    }

    /// The path of the state file, when there is a directory.
    #[must_use]
    pub fn path(&self) -> Option<PathBuf> {
        self.dir.as_ref().map(|dir| dir.join(STATE_FILE))
    }

    /// Saves `state`, atomically. Does nothing, successfully, unless this
    /// store is the [`Primary`](PersistenceMode::Primary), or when the
    /// state is what the file already holds. Returns whether the file was
    /// written.
    ///
    /// # Errors
    ///
    /// The error of the file system. The state file is then unchanged.
    pub fn save(&self, state: &PersistedState) -> io::Result<bool> {
        let (PersistenceMode::Primary, Some(dir)) = (&self.mode, &self.dir) else {
            return Ok(false);
        };
        let comparable = encode(state, 0);
        let mut last_written = lock(&self.last_written);
        if last_written.as_ref() == Some(&comparable) {
            return Ok(false);
        }
        write_atomic(&dir.join(STATE_FILE), &encode(state, now_ms()))?;
        *last_written = Some(comparable);
        Ok(true)
    }
}

/// The bytes of a state file. `saved_at` is the only part that does not
/// depend on `state`.
fn encode(state: &PersistedState, saved_at: u64) -> Vec<u8> {
    let file = FileOut {
        version: CURRENT_VERSION,
        saved_at,
        sessions: &state.sessions,
        project_order: &state.project_order,
        layout: &state.layout,
        recent_folders: state
            .recent_folders
            .iter()
            .map(|folder| folder.to_string_lossy().into_owned())
            .collect(),
        window: &state.window,
    };
    let mut bytes = serde_json::to_vec_pretty(&file).unwrap_or_else(|error| {
        // Every part is plain data: this cannot happen, and an empty state
        // would be worse than the previous file.
        warn!(error = %error, "the state could not be encoded");
        Vec::new()
    });
    bytes.push(b'\n');
    bytes
}

enum LockOutcome {
    Acquired(File),
    Taken,
    /// The file system cannot lock: carry on as the only instance.
    Unsupported,
}

fn acquire_lock(path: &Path) -> LockOutcome {
    let file = match OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
    {
        Ok(file) => file,
        Err(error) => {
            warn!(error = %error, "the lock of the state cannot be opened; assuming a single instance");
            return LockOutcome::Unsupported;
        }
    };
    match file.try_lock() {
        Ok(()) => LockOutcome::Acquired(file),
        Err(fs::TryLockError::WouldBlock) => LockOutcome::Taken,
        Err(fs::TryLockError::Error(error)) => {
            warn!(error = %error, "the state cannot be locked; assuming a single instance");
            LockOutcome::Unsupported
        }
    }
}

/// Reads the state file of `dir`. On success returns the state and, when
/// the file was used as it is, the comparable form of its content.
///
/// # Errors
///
/// A reason, for the user, why nothing may be saved during this run.
fn load(
    dir: &Path,
    migrations: &[Migration],
    current: u32,
) -> Result<(PersistedState, Option<Vec<u8>>), String> {
    let path = dir.join(STATE_FILE);
    let bytes = match read_bounded(&path) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return Ok((PersistedState::default(), None)),
        Err(error) => {
            warn!(error = %error, "the state file cannot be read; it is left alone and nothing will be saved");
            return Err(format!("the state file cannot be read: {error}"));
        }
    };

    let parsed = serde_json::from_slice::<Value>(&bytes).ok();
    let version = parsed
        .as_ref()
        .and_then(|value| value.get("version"))
        .and_then(Value::as_u64);
    let (Some(mut value), Some(version)) = (parsed, version) else {
        set_aside(&path, "corrupt", true)?;
        warn!("the state file was not valid; it was set aside and termx starts clean");
        return Ok((PersistedState::default(), None));
    };

    if version > u64::from(current) {
        set_aside(&path, &format!("future-v{version}"), false)?;
        warn!(
            version,
            supported = current,
            "the state file was written by a newer termx; a copy was kept and termx starts clean"
        );
        return Ok((PersistedState::default(), None));
    }

    let version = u32::try_from(version).unwrap_or(current);
    if version < current {
        set_aside(&path, &format!("v{version}"), false)?;
        for step in version.max(1)..current {
            let migration = usize::try_from(step - 1)
                .ok()
                .and_then(|index| migrations.get(index));
            let migrated = match migration {
                Some(migration) => migration(value),
                None => Err(format!("no migration from version {step}")),
            };
            value = match migrated {
                Ok(value) => value,
                Err(error) => {
                    warn!(
                        from = step,
                        error = %error,
                        "the state file could not be migrated; a copy was kept and termx starts clean"
                    );
                    return Ok((PersistedState::default(), None));
                }
            };
        }
        debug!(from = version, to = current, "the state file was migrated");
    }

    let (state, rejected) = parse(&value);
    if rejected > 0 {
        set_aside(&path, "partial", false)?;
        warn!(
            rejected,
            "some entries of the state file were not valid; a copy was kept and they were dropped"
        );
    }
    let comparable = (rejected == 0 && version == current).then(|| encode(&state, 0));
    Ok((state, comparable))
}

/// Reads a file of at most [`MAX_STATE_BYTES`]. `Ok(None)` if it does not
/// exist.
fn read_bounded(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs::metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
        Ok(metadata) if metadata.len() > MAX_STATE_BYTES => {
            return Err(io::Error::other("the file is too large to be a state file"));
        }
        Ok(_) => {}
    }
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Reads what it can of a file of the current version. Returns the state
/// and how many entries were rejected.
fn parse(value: &Value) -> (PersistedState, usize) {
    let mut rejected = 0;
    let mut state = PersistedState::default();

    let mut seen = std::collections::HashSet::new();
    if let Some(sessions) = value.get("sessions").and_then(Value::as_array) {
        for entry in sessions {
            match serde_json::from_value::<StoredSession>(entry.clone()) {
                Ok(session)
                    if state.sessions.len() < MAX_SESSIONS
                        && session.cwd.is_absolute()
                        && seen.insert(session.id.clone()) =>
                {
                    state.sessions.push(session);
                }
                _ => rejected += 1,
            }
        }
    } else if value.get("sessions").is_some_and(|v| !v.is_null()) {
        rejected += 1;
    }

    match value.get("projectOrder") {
        None | Some(Value::Null) => {}
        Some(order) => match serde_json::from_value::<Vec<String>>(order.clone()) {
            Ok(order) => state.project_order = order,
            Err(_) => rejected += 1,
        },
    }

    match value.get("layout") {
        None | Some(Value::Null) => {}
        Some(layout) => match serde_json::from_value::<Layout>(layout.clone()) {
            Ok(mut layout) => {
                if layout.sanitize(|session| seen.contains(session)) {
                    rejected += 1;
                }
                state.layout = Some(layout);
            }
            Err(_) => rejected += 1,
        },
    }

    match value.get("recentFolders") {
        None | Some(Value::Null) => {}
        Some(folders) => match serde_json::from_value::<Vec<PathBuf>>(folders.clone()) {
            Ok(folders) => {
                for folder in folders {
                    if folder.is_absolute()
                        && !state.recent_folders.contains(&folder)
                        && state.recent_folders.len() < MAX_RECENT_FOLDERS
                    {
                        state.recent_folders.push(folder);
                    }
                }
            }
            Err(_) => rejected += 1,
        },
    }

    match value.get("window") {
        None | Some(Value::Null) => {}
        Some(window) => match serde_json::from_value::<WindowGeometry>(window.clone()) {
            Ok(window) if window.is_valid() => state.window = Some(window),
            _ => rejected += 1,
        },
    }

    (state, rejected)
}

/// Keeps a copy of the state file under a name that says why, moving the
/// file itself when `moved`.
///
/// # Errors
///
/// A reason why nothing may be saved: without a copy, the file must not be
/// overwritten.
fn set_aside(path: &Path, label: &str, moved: bool) -> Result<(), String> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let name = format!(
        "state.{label}-{:013}-{}.json",
        now_ms(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let backup = path.with_file_name(name);
    let result = if moved {
        fs::rename(path, &backup)
    } else {
        fs::copy(path, &backup).map(|_| ())
    };
    result.map_err(|error| {
        warn!(error = %error, "the state file could not be backed up; it is left alone and nothing will be saved");
        format!("the state file could not be backed up: {error}")
    })
}

/// Whether `name` is a backup made by [`set_aside`]. Returns the time in
/// its name, which orders the backups.
fn backup_time(name: &str) -> Option<u64> {
    let rest = name.strip_prefix("state.")?.strip_suffix(".json")?;
    let mut parts = rest.rsplitn(3, '-');
    let _counter: u64 = parts.next()?.parse().ok()?;
    let time: u64 = parts.next()?.parse().ok()?;
    let label = parts.next()?;
    let known = label == "corrupt"
        || label == "partial"
        || label.strip_prefix("future-v").is_some_and(is_number)
        || label.strip_prefix('v').is_some_and(is_number);
    known.then_some(time)
}

fn is_number(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

/// Deletes all but the [`MAX_BACKUPS`] newest backups.
fn prune_backups(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut backups: Vec<(u64, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let time = backup_time(&entry.file_name().to_string_lossy())?;
            Some((time, entry.path()))
        })
        .collect();
    backups.sort();
    let excess = backups.len().saturating_sub(MAX_BACKUPS);
    for (_, path) in backups.into_iter().take(excess) {
        let _ = fs::remove_file(path);
    }
}

/// Prefix of the temporary files of [`write_atomic`].
fn temporary_prefix() -> String {
    format!("{STATE_FILE}.tmp-")
}

/// Removes what a save that was interrupted left behind. Only called by
/// the instance that holds the lock, so no save can be in progress.
fn remove_stale_temporaries(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let prefix = temporary_prefix();
    for entry in entries.filter_map(Result::ok) {
        if entry.file_name().to_string_lossy().starts_with(&prefix) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// The points of a save at which a test can make it fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// Half of the content is in the temporary file.
    PartlyWritten,
    /// All of it is, and was flushed to the disk.
    Synced,
}

/// Replaces `path` by a file holding `bytes`, atomically. See the
/// [module documentation](self#writing).
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_atomic_with(path, bytes, &mut |_| Ok(()))
}

/// [`write_atomic`], calling `checkpoint` at each [`Stage`]. An error from
/// it aborts the save the way a crash would: nothing is cleaned up.
fn write_atomic_with(
    path: &Path,
    bytes: &[u8],
    checkpoint: &mut dyn FnMut(Stage) -> io::Result<()>,
) -> io::Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let temporary = path.with_file_name(format!(
        "{}{}-{}",
        temporary_prefix(),
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));

    let written = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // The file names folders and commands: it is the user's alone.
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        let (first, second) = bytes.split_at(bytes.len() / 2);
        file.write_all(first)?;
        checkpoint(Stage::PartlyWritten).map_err(Crash::wrap)?;
        file.write_all(second)?;
        file.sync_all()?;
        checkpoint(Stage::Synced).map_err(Crash::wrap)?;
        drop(file);
        rename_replacing(&temporary, path)
    })();

    match written {
        Ok(()) => {
            sync_dir(path);
            Ok(())
        }
        Err(error) => {
            if !Crash::is(&error) {
                let _ = fs::remove_file(&temporary);
            }
            Err(error)
        }
    }
}

/// Marks an error as a simulated crash, after which nothing is cleaned up.
#[derive(Debug)]
struct Crash(io::Error);

impl Crash {
    fn wrap(error: io::Error) -> io::Error {
        io::Error::other(Self(error))
    }

    fn is(error: &io::Error) -> bool {
        error.get_ref().is_some_and(|inner| inner.is::<Self>())
    }
}

impl std::fmt::Display for Crash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for Crash {}

/// Renames `from` over `to`. On Windows the rename fails while another
/// program (an antivirus, an indexer) has the target open, which passes
/// within moments: it is tried a few times.
fn rename_replacing(from: &Path, to: &Path) -> io::Result<()> {
    let attempts = if cfg!(windows) { 10 } else { 1 };
    let mut result = fs::rename(from, to);
    for _ in 1..attempts {
        match &result {
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                std::thread::sleep(std::time::Duration::from_millis(20));
                result = fs::rename(from, to);
            }
            _ => break,
        }
    }
    result
}

/// Flushes the directory entry of `path`, so that the rename itself
/// survives a power failure. Not possible on Windows, and best effort
/// elsewhere: the file is complete either way.
fn sync_dir(path: &Path) {
    #[cfg(unix)]
    if let Some(dir) = path.parent()
        && let Ok(dir) = File::open(dir)
    {
        let _ = dir.sync_all();
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::layout::{Direction, Placement};
    use crate::workspace::project::ProjectKind;

    fn absolute(name: &str) -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(format!(r"C:\work\{name}"))
        } else {
            PathBuf::from(format!("/work/{name}"))
        }
    }

    fn session(id: &str, kind: SessionKind) -> StoredSession {
        StoredSession {
            id: SessionKey::from_raw(id).unwrap(),
            kind,
            cwd: absolute(id),
            command: (kind == SessionKind::Background).then(|| "pnpm dev".to_owned()),
            name: Some(format!("name of {id}")),
            auto_title: Some("a title".to_owned()),
            project: Some(Project {
                key: absolute(id).to_string_lossy().into_owned(),
                name: id.to_owned(),
                root: absolute(id),
                kind: ProjectKind::Git,
            }),
            git: Some(GitInfo {
                branch: Some("main".to_owned()),
                commit: None,
                worktree: Some("wt".to_owned()),
            }),
            created_at: 1_000,
            last_activity_at: 2_000,
            claude_session_id: (kind == SessionKind::Claude).then(|| "abc-123".to_owned()),
        }
    }

    fn sample() -> PersistedState {
        let mut layout = Layout::default();
        let first = layout.focused();
        layout
            .show(first, Some(SessionKey::from_raw("s-a").unwrap()))
            .unwrap();
        layout
            .split(
                first,
                Direction::Row,
                Placement::After,
                Some(SessionKey::from_raw("s-b").unwrap()),
            )
            .unwrap();
        PersistedState {
            sessions: vec![
                session("s-a", SessionKind::Claude),
                session("s-b", SessionKind::Shell),
                session("s-c", SessionKind::Background),
            ],
            project_order: vec!["two".to_owned(), "one".to_owned()],
            layout: Some(layout),
            recent_folders: vec![absolute("recent-1"), absolute("recent-2")],
            window: Some(WindowGeometry {
                x: 10.0,
                y: 20.5,
                width: 1200.0,
                height: 800.0,
                maximized: true,
            }),
        }
    }

    fn backups(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| backup_time(name).is_some())
            .collect();
        names.sort();
        names
    }

    fn files(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_missing_file_is_a_clean_start() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("not").join("there").join("yet");

        let (store, state) = Store::open(&nested);

        assert_eq!(state, PersistedState::default());
        assert_eq!(*store.mode(), PersistenceMode::Primary);
        assert_eq!(store.path(), Some(nested.join(STATE_FILE)));
        assert_eq!(files(&nested), [LOCK_FILE]);
    }

    #[test]
    fn the_state_survives_a_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let state = sample();
        {
            let (store, _) = Store::open(dir.path());
            assert!(store.save(&state).unwrap());
            // Nothing changed: nothing is written.
            assert!(!store.save(&state).unwrap());
        }

        let (store, read) = Store::open(dir.path());

        assert_eq!(read, state);
        assert!(backups(dir.path()).is_empty());
        // What was read is what is on disk: saving it again writes nothing.
        assert!(!store.save(&read).unwrap());
        let text = fs::read_to_string(dir.path().join(STATE_FILE)).unwrap();
        let json: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(json["version"], CURRENT_VERSION);
        assert!(json["savedAt"].as_u64().unwrap() > 0);
        assert!(text.ends_with('\n'));
    }

    #[cfg(unix)]
    #[test]
    fn the_state_file_is_private_to_the_user() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let (store, _) = Store::open(dir.path());
        store.save(&sample()).unwrap();

        let mode = fs::metadata(dir.path().join(STATE_FILE))
            .unwrap()
            .permissions()
            .mode();

        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn a_store_without_a_directory_saves_nothing() {
        let store = Store::in_memory();

        assert_eq!(*store.mode(), PersistenceMode::Memory);
        assert_eq!(store.path(), None);
        assert!(!store.save(&sample()).unwrap());
    }

    #[test]
    fn a_crash_in_the_middle_of_a_save_leaves_the_previous_state() {
        for stage in [Stage::PartlyWritten, Stage::Synced] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join(STATE_FILE);
            let old = sample();
            {
                let (store, _) = Store::open(dir.path());
                store.save(&old).unwrap();
            }
            let before = fs::read(&path).unwrap();
            let mut new = sample();
            new.sessions.clear();

            let crashed = write_atomic_with(&path, &encode(&new, 1), &mut |at| {
                if at == stage {
                    Err(io::Error::other("power failure"))
                } else {
                    Ok(())
                }
            });

            assert!(crashed.is_err());
            assert_eq!(fs::read(&path).unwrap(), before, "{stage:?}");
            // The temporary file is what a crash leaves behind...
            let leftovers: Vec<String> = files(dir.path())
                .into_iter()
                .filter(|name| name.starts_with(&temporary_prefix()))
                .collect();
            assert_eq!(leftovers.len(), 1, "{stage:?}");
            if stage == Stage::PartlyWritten {
                let partial = fs::read(dir.path().join(&leftovers[0])).unwrap();
                assert!(serde_json::from_slice::<Value>(&partial).is_err());
            }

            // ...and the next run reads the old state and cleans up.
            let (store, read) = Store::open(dir.path());
            assert_eq!(read, old, "{stage:?}");
            assert_eq!(*store.mode(), PersistenceMode::Primary);
            assert_eq!(files(dir.path()), [STATE_FILE, LOCK_FILE], "{stage:?}");
        }
    }

    #[test]
    fn a_save_that_fails_cleans_up_and_keeps_the_previous_state() {
        let dir = tempfile::tempdir().unwrap();
        let (store, _) = Store::open(dir.path());
        store.save(&sample()).unwrap();
        let path = dir.path().join(STATE_FILE);
        let before = fs::read(&path).unwrap();
        // A directory in the way makes the rename fail.
        let blocked = dir.path().join("blocked");
        fs::create_dir_all(blocked.join(STATE_FILE).join("child")).unwrap();

        let failed = write_atomic(&blocked.join(STATE_FILE), b"{}");

        assert!(failed.is_err());
        assert_eq!(files(&blocked), [STATE_FILE], "the temporary file is gone");
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    #[test]
    fn a_corrupt_file_is_set_aside_and_termx_starts_clean() {
        for content in [
            &b"{ \"version\": 1, \"sessions\": [ { \"id\""[..],
            b"",
            b"\xff\xfe\x00garbage",
            b"[1, 2, 3]",
            b"{}",
            b"{ \"version\": \"one\" }",
            b"{ \"version\": -1 }",
            b"null",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join(STATE_FILE);
            fs::write(&path, content).unwrap();

            let (store, state) = Store::open(dir.path());

            assert_eq!(state, PersistedState::default());
            assert_eq!(*store.mode(), PersistenceMode::Primary);
            assert!(!path.exists(), "the corrupt file was moved away");
            let kept = backups(dir.path());
            assert_eq!(kept.len(), 1, "{content:?}");
            assert!(kept[0].starts_with("state.corrupt-"), "{kept:?}");
            assert_eq!(fs::read(dir.path().join(&kept[0])).unwrap(), content);

            // The clean state can be saved, and the backup stays.
            assert!(store.save(&sample()).unwrap());
            drop(store);
            assert_eq!(Store::open(dir.path()).1, sample());
            assert_eq!(backups(dir.path()), kept);
        }
    }

    #[test]
    fn a_file_from_a_newer_termx_is_kept_and_not_used() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(STATE_FILE);
        let content = br#"{ "version": 99, "sessions": [], "somethingNew": true }"#;
        fs::write(&path, content).unwrap();

        let (store, state) = Store::open(dir.path());

        assert_eq!(state, PersistedState::default());
        assert_eq!(*store.mode(), PersistenceMode::Primary);
        let kept = backups(dir.path());
        assert_eq!(kept.len(), 1);
        assert!(kept[0].starts_with("state.future-v99-"), "{kept:?}");
        assert_eq!(fs::read(dir.path().join(&kept[0])).unwrap(), content);
        // The file itself is untouched until something is saved.
        assert_eq!(fs::read(&path).unwrap(), content);
    }

    #[test]
    fn invalid_entries_are_dropped_and_the_file_is_backed_up() {
        let dir = tempfile::tempdir().unwrap();
        let good = serde_json::to_value(session("s-good", SessionKind::Shell)).unwrap();
        let mut relative = serde_json::to_value(session("s-rel", SessionKind::Shell)).unwrap();
        relative["cwd"] = "relative/path".into();
        let mut unknown_kind = serde_json::to_value(session("s-kind", SessionKind::Shell)).unwrap();
        unknown_kind["kind"] = "teleporter".into();
        let content = serde_json::json!({
            "version": 1,
            "sessions": [good, good, relative, unknown_kind, { "id": "../x" }, 7],
            "projectOrder": "not a list",
            "layout": { "root": { "type": "pane", "id": 1, "session": "s-gone" }, "focused": 1, "nextId": 2 },
            "recentFolders": [absolute("a"), "relative", absolute("a")],
            "window": { "x": 0, "y": 0, "width": -5, "height": 10 },
            "unknownField": { "kept": false },
        });
        fs::write(dir.path().join(STATE_FILE), content.to_string()).unwrap();

        let (store, state) = Store::open(dir.path());

        assert_eq!(*store.mode(), PersistenceMode::Primary);
        assert_eq!(state.sessions, [session("s-good", SessionKind::Shell)]);
        assert!(state.project_order.is_empty());
        assert_eq!(state.layout, Some(Layout::default()));
        assert_eq!(state.recent_folders, [absolute("a")]);
        assert_eq!(state.window, None);
        let kept = backups(dir.path());
        assert_eq!(kept.len(), 1);
        assert!(kept[0].starts_with("state.partial-"), "{kept:?}");
    }

    #[test]
    fn a_file_with_only_a_version_is_an_empty_state() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(STATE_FILE), br#"{ "version": 1 }"#).unwrap();

        let (_store, state) = Store::open(dir.path());

        assert_eq!(state, PersistedState::default());
        assert!(backups(dir.path()).is_empty());
    }

    #[test]
    fn an_older_file_is_backed_up_and_migrated() {
        // A pretend version 3 whose migrations rename a field and move the
        // recent folders.
        fn one_to_two(mut value: Value) -> Result<Value, String> {
            let object = value.as_object_mut().ok_or("not an object")?;
            let folders = object.remove("recent").ok_or("no recent folders")?;
            object.insert("recentFolders".to_owned(), folders);
            object.insert("version".to_owned(), 2.into());
            Ok(value)
        }
        fn two_to_three(mut value: Value) -> Result<Value, String> {
            let object = value.as_object_mut().ok_or("not an object")?;
            let order = object.remove("projects").unwrap_or(Value::Null);
            object.insert("projectOrder".to_owned(), order);
            object.insert("version".to_owned(), 3.into());
            Ok(value)
        }
        let migrations: &[Migration] = &[one_to_two, two_to_three];
        let dir = tempfile::tempdir().unwrap();
        let old = serde_json::json!({
            "version": 1,
            "recent": [absolute("old")],
            "projects": ["p"],
        });
        fs::write(dir.path().join(STATE_FILE), old.to_string()).unwrap();

        let (store, state) = Store::open_with(dir.path(), migrations, 3);

        assert_eq!(*store.mode(), PersistenceMode::Primary);
        assert_eq!(state.recent_folders, [absolute("old")]);
        assert_eq!(state.project_order, ["p"]);
        let kept = backups(dir.path());
        assert_eq!(kept.len(), 1);
        assert!(kept[0].starts_with("state.v1-"), "{kept:?}");
        let backup: Value =
            serde_json::from_slice(&fs::read(dir.path().join(&kept[0])).unwrap()).unwrap();
        assert_eq!(backup, old);
    }

    #[test]
    fn a_migration_that_fails_keeps_the_file_and_starts_clean() {
        fn broken(_: Value) -> Result<Value, String> {
            Err("cannot make sense of it".to_owned())
        }
        let dir = tempfile::tempdir().unwrap();
        let old = br#"{ "version": 1, "recentFolders": [] }"#;
        fs::write(dir.path().join(STATE_FILE), old).unwrap();

        for migrations in [&[broken as Migration][..], &[]] {
            let (store, state) = Store::open_with(dir.path(), migrations, 2);

            assert_eq!(state, PersistedState::default());
            assert_eq!(*store.mode(), PersistenceMode::Primary);
        }
        let kept = backups(dir.path());
        assert_eq!(kept.len(), 2);
        assert_eq!(fs::read(dir.path().join(&kept[0])).unwrap(), old);
    }

    #[test]
    fn only_the_newest_backups_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        for time in 0..(MAX_BACKUPS + 4) {
            fs::write(
                dir.path().join(format!("state.corrupt-{time:013}-0.json")),
                b"x",
            )
            .unwrap();
        }
        fs::write(dir.path().join("state.notes.json"), b"mine").unwrap();
        fs::write(dir.path().join("unrelated.txt"), b"mine").unwrap();

        let (_store, _) = Store::open(dir.path());

        let kept = backups(dir.path());
        assert_eq!(kept.len(), MAX_BACKUPS);
        assert_eq!(kept[0], "state.corrupt-0000000000004-0.json");
        assert!(dir.path().join("state.notes.json").exists());
        assert!(dir.path().join("unrelated.txt").exists());
    }

    #[test]
    fn backup_names_are_recognised() {
        for name in [
            "state.corrupt-1795716005340-0.json",
            "state.partial-1795716005340-12.json",
            "state.v1-1795716005340-0.json",
            "state.future-v99-1795716005340-3.json",
        ] {
            assert_eq!(backup_time(name), Some(1_795_716_005_340), "{name}");
        }
        for name in [
            "state.json",
            "state.lock",
            "state.json.tmp-12-0",
            "state.notes-1-0.json",
            "state.v-1-0.json",
            "state.corrupt-x-0.json",
            "other.corrupt-1-0.json",
        ] {
            assert_eq!(backup_time(name), None, "{name}");
        }
    }

    #[test]
    fn a_second_instance_never_writes() {
        let dir = tempfile::tempdir().unwrap();
        let (first, _) = Store::open(dir.path());
        first.save(&sample()).unwrap();
        let before = fs::read(dir.path().join(STATE_FILE)).unwrap();

        let (second, borrowed) = Store::open(dir.path());

        assert_eq!(*first.mode(), PersistenceMode::Primary);
        assert_eq!(*second.mode(), PersistenceMode::Secondary);
        // It only learns the recent folders.
        assert_eq!(
            borrowed,
            PersistedState {
                recent_folders: sample().recent_folders,
                ..PersistedState::default()
            }
        );
        let mut other = sample();
        other.sessions.clear();
        assert!(!second.save(&other).unwrap());
        assert_eq!(fs::read(dir.path().join(STATE_FILE)).unwrap(), before);

        // The first one still saves, and once it is gone the next instance
        // owns the state again.
        let mut changed = sample();
        changed.project_order.clear();
        assert!(first.save(&changed).unwrap());
        drop(first);
        drop(second);
        let (third, read) = Store::open(dir.path());
        assert_eq!(*third.mode(), PersistenceMode::Primary);
        assert_eq!(read, changed);
    }

    #[test]
    fn a_second_instance_does_not_touch_a_corrupt_file() {
        let dir = tempfile::tempdir().unwrap();
        let (_first, _) = Store::open(dir.path());
        fs::write(dir.path().join(STATE_FILE), b"not json").unwrap();

        let (second, state) = Store::open(dir.path());

        assert_eq!(*second.mode(), PersistenceMode::Secondary);
        assert_eq!(state, PersistedState::default());
        assert_eq!(fs::read(dir.path().join(STATE_FILE)).unwrap(), b"not json");
        assert!(backups(dir.path()).is_empty());
    }

    #[test]
    fn concurrent_writers_never_produce_a_torn_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(STATE_FILE);
        let states: Vec<Vec<u8>> = (0..4)
            .map(|n| {
                let mut state = sample();
                state.project_order = vec![format!("writer-{n}"); 200];
                encode(&state, n)
            })
            .collect();
        write_atomic(&path, &states[0]).unwrap();

        std::thread::scope(|scope| {
            for bytes in &states {
                let path = &path;
                scope.spawn(move || {
                    for _ in 0..25 {
                        // On Windows a rename can lose against a reader or
                        // another rename; the file is whole either way.
                        let _ = write_atomic(path, bytes);
                    }
                });
            }
            let states = &states;
            let path = &path;
            scope.spawn(move || {
                for _ in 0..200 {
                    if let Ok(bytes) = fs::read(path) {
                        assert!(states.contains(&bytes), "a torn state file was read");
                    }
                }
            });
        });

        assert!(states.contains(&fs::read(&path).unwrap()));
        assert_eq!(files(dir.path()), [STATE_FILE]);
    }

    #[test]
    fn a_file_that_cannot_be_backed_up_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        // A directory where the state file should be: it can neither be
        // read as a file nor replaced.
        fs::create_dir(dir.path().join(STATE_FILE)).unwrap();

        let (store, state) = Store::open(dir.path());

        assert_eq!(state, PersistedState::default());
        assert!(
            matches!(store.mode(), PersistenceMode::Disabled { .. }),
            "{:?}",
            store.mode()
        );
        assert!(!store.save(&sample()).unwrap());
        assert!(dir.path().join(STATE_FILE).is_dir());
    }

    #[test]
    fn window_geometry_is_checked() {
        let good = WindowGeometry {
            x: -1200.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
            maximized: false,
        };

        assert!(good.is_valid());
        assert!(!WindowGeometry { width: 0.0, ..good }.is_valid());
        assert!(
            !WindowGeometry {
                height: f64::NAN,
                ..good
            }
            .is_valid()
        );
        assert!(
            !WindowGeometry {
                x: f64::INFINITY,
                ..good
            }
            .is_valid()
        );
    }

    #[test]
    fn the_mode_is_tagged_for_the_frontend() {
        assert_eq!(
            serde_json::to_value(PersistenceMode::Primary).unwrap(),
            serde_json::json!({ "mode": "primary" })
        );
        assert_eq!(
            serde_json::to_value(PersistenceMode::Disabled {
                reason: "why".to_owned()
            })
            .unwrap(),
            serde_json::json!({ "mode": "disabled", "reason": "why" })
        );
    }
}
