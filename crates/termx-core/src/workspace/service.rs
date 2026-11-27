//! [`Workspace`]: the sessions, their layout, their persistence and the
//! thread that keeps them current.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::thread::{self, JoinHandle, Thread};
use std::time::{Duration, Instant};

use serde::Serialize;

use super::launch::{
    LaunchFailure, LaunchMarker, LaunchMode, LaunchRequest, LaunchToken, Launcher, MarkerFilter,
};
use super::layout::{Direction, Edge, Layout, LayoutError, PaneId, Placement, SplitId};
use super::model::{
    MAX_COMMAND_BYTES, Session, SessionKey, SessionKind, SessionState, clean_title,
    is_valid_claude_session_id, now_ms,
};
use super::notify::{ChangeHub, ChangeSet, Subscription};
use super::project::{self, Detected, Project};
use super::store::{
    MAX_RECENT_FOLDERS, PersistedState, PersistenceMode, Store, StoredSession, WindowGeometry,
};
use crate::diagnostics::tracing::{debug, info, warn};
use crate::pty::{ExitStatus, PtyCommand, lock};

/// How often, at most, the output of a session updates its
/// [`last_activity_at`](Session::last_activity_at) and notifies the
/// subscribers. Output arrives many times per second; the sidebar does not
/// need to know more than once per second.
pub const ACTIVITY_COALESCE: Duration = Duration::from_secs(1);

/// How long the workspace must be left unchanged before a change that
/// comes in bursts is saved, so that a burst costs one write: titles set by
/// programs, activity, the Git state, the window geometry, a divider being
/// dragged, the focus moving between panes. What the user does on purpose
/// is saved at once instead: see [the module documentation](super#saving).
pub const SAVE_DEBOUNCE: Duration = Duration::from_secs(1);

/// Longest a change waits to be saved, however many changes follow it.
pub const SAVE_MAX_WAIT: Duration = Duration::from_secs(5);

/// How long after a save failed it is tried again, for as long as it
/// fails. See [`PersistenceMode::Failing`].
pub const SAVE_RETRY: Duration = Duration::from_secs(5);

/// How often the Git state of every session is read again, to catch what
/// changed outside termx. See the [module documentation](super).
pub const GIT_POLL_INTERVAL: Duration = Duration::from_secs(10);

/// How long a caller waits for the project of a folder before it goes on
/// with the folder as its own project. The detection carries on in the
/// background, and the sessions of the folder are updated when it ends.
pub const GIT_DETECT_BUDGET: Duration = Duration::from_secs(2);

/// How long [`Workspace::shutdown`] waits for the thread of the workspace
/// to stop, and then for the last save, before it gives up on each.
pub const SHUTDOWN_WAIT: Duration = Duration::from_secs(5);

/// How long the script of a session has to say that it runs its command
/// before the session is given a
/// [`LaunchUnconfirmed`](LaunchFailure::LaunchUnconfirmed) warning. Long
/// enough for a slow shell profile (a Node version manager can take
/// seconds), short enough that a user staring at a bare prompt is told why.
pub const LAUNCH_CONFIRM_TIMEOUT: Duration = Duration::from_secs(15);

/// How many project keys are remembered for their order.
const MAX_PROJECT_ORDER: usize = 200;

/// Longest text accepted as a reported folder, in bytes.
const MAX_REPORTED_CWD_BYTES: usize = 4096;

/// How a [`Workspace`] is set up.
///
/// The type is `#[non_exhaustive]`: start from [`WorkspaceConfig::new`] or
/// [`WorkspaceConfig::in_memory`] and change what differs.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct WorkspaceConfig {
    /// The directory of the state file. Chosen by the application; `None`
    /// to remember nothing.
    pub state_dir: Option<PathBuf>,
    /// Builds the commands of the sessions.
    pub launcher: Launcher,
    /// See [`ACTIVITY_COALESCE`].
    pub activity_coalesce: Duration,
    /// See [`SAVE_DEBOUNCE`].
    pub save_debounce: Duration,
    /// See [`SAVE_MAX_WAIT`].
    pub save_max_wait: Duration,
    /// See [`SAVE_RETRY`].
    pub save_retry: Duration,
    /// See [`GIT_POLL_INTERVAL`].
    pub git_poll_interval: Duration,
    /// See [`GIT_DETECT_BUDGET`].
    pub git_detect_budget: Duration,
    /// See [`SHUTDOWN_WAIT`].
    pub shutdown_wait: Duration,
    /// See [`LAUNCH_CONFIRM_TIMEOUT`].
    pub launch_confirm_timeout: Duration,
    /// Finds the project of a folder: [`project::detect`], replaced by
    /// tests that need a detection that hangs.
    pub(crate) detector: fn(&Path) -> Detected,
}

impl WorkspaceConfig {
    /// A workspace that keeps its state in `state_dir`.
    #[must_use]
    pub fn new(state_dir: impl Into<PathBuf>) -> Self {
        Self {
            state_dir: Some(state_dir.into()),
            ..Self::in_memory()
        }
    }

    /// A workspace that remembers nothing.
    #[must_use]
    pub fn in_memory() -> Self {
        Self {
            state_dir: None,
            launcher: Launcher::default(),
            activity_coalesce: ACTIVITY_COALESCE,
            save_debounce: SAVE_DEBOUNCE,
            save_max_wait: SAVE_MAX_WAIT,
            save_retry: SAVE_RETRY,
            git_poll_interval: GIT_POLL_INTERVAL,
            git_detect_budget: GIT_DETECT_BUDGET,
            shutdown_wait: SHUTDOWN_WAIT,
            launch_confirm_timeout: LAUNCH_CONFIRM_TIMEOUT,
            detector: project::detect,
        }
    }
}

/// What a new session is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSession {
    /// What it runs.
    pub kind: SessionKind,
    /// The folder it runs in. Must be an existing directory.
    pub cwd: PathBuf,
    /// The command line, for [`SessionKind::Background`]. Ignored for the
    /// other kinds.
    pub command: Option<String>,
    /// A name, as if the user had renamed the session.
    pub name: Option<String>,
    /// Whether to show the session in the focused pane.
    pub show: bool,
}

impl NewSession {
    /// A session of `kind` in `cwd`, shown in the focused pane.
    #[must_use]
    pub fn new(kind: SessionKind, cwd: impl Into<PathBuf>) -> Self {
        Self {
            kind,
            cwd: cwd.into(),
            command: None,
            name: None,
            show: true,
        }
    }
}

/// What to run to start a session. See [`Workspace::begin_launch`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    /// The command to start in a terminal.
    pub command: PtyCommand,
    /// Identifies this start of the session. Pass it back with what
    /// becomes of the terminal, so that news of an earlier start is not
    /// taken for news of this one.
    pub generation: u64,
    /// The terminal the session was running in, which must now be closed.
    pub previous_terminal: Option<u64>,
    /// The token the script of this start prints its markers with. Pass
    /// the output of the terminal through
    /// [`Workspace::output_filter`], which takes them out and reports
    /// them. `None` for a shell session, which runs no script.
    pub token: Option<LaunchToken>,
}

/// Why an operation on the workspace was refused. Nothing was changed,
/// unless the documentation of the operation says otherwise.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum WorkspaceError {
    /// No session has this key.
    #[error("no session with id {0}")]
    UnknownSession(SessionKey),
    /// No project has this key.
    #[error("no such project")]
    UnknownProject,
    /// The session has a process: stop it first.
    #[error("the session {0} is running; stop it first")]
    SessionRunning(SessionKey),
    /// The folder does not exist or is not a directory.
    #[error("the folder {} does not exist or is not a folder", .0.display())]
    FolderNotFound(PathBuf),
    /// The folder is not an absolute path, or its name is not Unicode.
    #[error("the folder must be an absolute path with a Unicode name")]
    InvalidFolder,
    /// A background session was given no command, one that is too long, or
    /// one with control characters.
    #[error(
        "a background session needs a command of at most {MAX_COMMAND_BYTES} bytes, without control characters"
    )]
    InvalidCommand,
    /// The identifier of the Claude Code conversation is not one.
    #[error("the Claude Code session identifier is not valid")]
    InvalidClaudeSessionId,
    /// The layout refused the operation.
    #[error(transparent)]
    Layout(#[from] LayoutError),
    /// The session could not be started.
    #[error(transparent)]
    Launch(#[from] LaunchFailure),
}

/// Everything a frontend needs to draw the workspace.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    /// Counts the changes of the workspace. A [`Delta`] with a lower
    /// revision than a snapshot is older than it.
    pub revision: u64,
    /// The sessions, in the order the user gave them.
    pub sessions: Vec<Session>,
    /// The projects of the sessions, in the order the user gave them.
    pub projects: Vec<Project>,
    /// The layout of the main area.
    pub layout: Layout,
    /// The folders sessions were created in, most recent first.
    #[serde(serialize_with = "super::lossy_paths")]
    pub recent_folders: Vec<PathBuf>,
    /// Whether the workspace is saved, and why not when it is not. A
    /// frontend warns the user while it is
    /// [`Failing`](PersistenceMode::Failing).
    pub persistence: PersistenceMode,
}

/// The parts of the workspace that changed, with their current values.
///
/// Applying a delta is idempotent, and every part carries the state at
/// [`revision`](Self::revision), so a frontend can apply deltas as they
/// come without comparing revisions.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Delta {
    /// The revision of the workspace when the delta was read.
    pub revision: u64,
    /// The sessions that were added or changed, as they are now.
    pub sessions: Vec<Session>,
    /// The sessions that were removed.
    pub removed: Vec<SessionKey>,
    /// The keys of all the sessions in order, when sessions were added,
    /// removed or reordered.
    pub session_order: Option<Vec<SessionKey>>,
    /// All the projects in order, when they changed.
    pub projects: Option<Vec<Project>>,
    /// The layout, when it changed.
    pub layout: Option<Layout>,
    /// The recent folders, when they changed.
    #[serde(serialize_with = "super::lossy_paths_option")]
    pub recent_folders: Option<Vec<PathBuf>>,
    /// Whether the workspace is saved, when that changed.
    pub persistence: Option<PersistenceMode>,
}

/// Records that a session printed something. Cheap enough to call for
/// every batch of output: it stores a time and, at most once per
/// [`ACTIVITY_COALESCE`], wakes the thread that passes it on. It takes no
/// lock and never blocks.
#[derive(Clone)]
pub struct ActivityHandle {
    stamp: Arc<AtomicU64>,
    signals: Arc<Signals>,
}

impl ActivityHandle {
    /// Records activity now.
    pub fn touch(&self) {
        self.stamp.store(now_ms(), Ordering::Relaxed);
        if !self.signals.activity.swap(true, Ordering::AcqRel) {
            self.signals.wake();
        }
    }
}

impl std::fmt::Debug for ActivityHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActivityHandle").finish_non_exhaustive()
    }
}

/// What the other threads tell the ticker thread.
#[derive(Default)]
struct Signals {
    /// Some session recorded activity that was not passed on yet.
    activity: AtomicBool,
    /// The Git state of every session is to be read again.
    git: AtomicBool,
    stop: AtomicBool,
    /// When a save was first and last asked for, while one is due.
    save: Mutex<Option<(Instant, Instant)>>,
    /// When to try saving again after a save failed.
    retry: Mutex<Option<Instant>>,
    /// A save is due now. See [`Mutation::persist_now`].
    urgent: AtomicBool,
    ticker: OnceLock<Thread>,
}

impl Signals {
    fn wake(&self) {
        if let Some(ticker) = self.ticker.get() {
            ticker.unpark();
        }
    }

    fn request_save(&self) {
        let now = Instant::now();
        let mut save = lock(&self.save);
        *save = Some((save.map_or(now, |(first, _)| first), now));
        drop(save);
        self.wake();
    }

    /// Asks for a save as soon as the thread of the workspace wakes up.
    fn request_save_now(&self) {
        self.urgent.store(true, Ordering::Release);
        self.wake();
    }

    /// Asks for a save `delay` from now, to try again after one failed.
    fn request_save_after(&self, delay: Duration) {
        *lock(&self.retry) = Some(after(Instant::now(), delay));
        self.wake();
    }
}

struct Entry {
    session: Session,
    /// The time of the last output that was not passed on yet, or 0.
    activity: Arc<AtomicU64>,
    /// Counts the starts of the session. See [`Launch::generation`].
    generation: u64,
    /// What the script of the current start has said, while it has a
    /// process.
    launch: Option<Tracked>,
}

/// What is known of the start of a session whose script prints markers.
struct Tracked {
    /// When it was started.
    started: Instant,
    /// The script said that it runs the command.
    confirmed: bool,
    /// The script said that `claude` is missing.
    claude_missing: bool,
}

/// Passes the markers of one start of a session to the workspace. See
/// [`Workspace::output_filter`].
struct LaunchWatch {
    filter: MarkerFilter,
    shared: std::sync::Weak<Shared>,
    key: SessionKey,
    generation: u64,
}

impl crate::session::OutputFilter for LaunchWatch {
    fn filter(&mut self, bytes: Vec<u8>) -> Vec<u8> {
        let (bytes, markers) = self.filter.filter(bytes);
        if let Some(shared) = self.shared.upgrade() {
            for marker in markers {
                shared.launch_marker(&self.key, self.generation, marker);
            }
        }
        bytes
    }

    fn finish(&mut self) -> Vec<u8> {
        self.filter.finish()
    }

    fn release_at(&self) -> Option<Instant> {
        self.filter.release_at()
    }
}

struct State {
    entries: Vec<Entry>,
    project_order: Vec<String>,
    layout: Layout,
    recent_folders: Vec<PathBuf>,
    window: Option<WindowGeometry>,
    revision: u64,
    /// Starts as the mode of the store, and follows whether saving fails.
    persistence: PersistenceMode,
}

impl State {
    fn entry(&mut self, key: &SessionKey) -> Result<&mut Entry, WorkspaceError> {
        self.entries
            .iter_mut()
            .find(|entry| entry.session.id == *key)
            .ok_or_else(|| WorkspaceError::UnknownSession(key.clone()))
    }

    fn position(&self, key: &SessionKey) -> Result<usize, WorkspaceError> {
        self.entries
            .iter()
            .position(|entry| entry.session.id == *key)
            .ok_or_else(|| WorkspaceError::UnknownSession(key.clone()))
    }

    /// The projects of the sessions, in the order the user gave them.
    fn projects(&self) -> Vec<Project> {
        let mut projects: Vec<&Project> = Vec::new();
        for entry in &self.entries {
            if !projects.iter().any(|p| p.key == entry.session.project.key) {
                projects.push(&entry.session.project);
            }
        }
        let rank = |project: &Project| {
            self.project_order
                .iter()
                .position(|key| *key == project.key)
                .unwrap_or(usize::MAX)
        };
        // Stable: projects without a rank keep the order of their sessions.
        projects.sort_by_key(|project| rank(project));
        projects.into_iter().cloned().collect()
    }

    /// Gives every project in use a place in the order, new ones last.
    fn rank_new_projects(&mut self) {
        for entry in &self.entries {
            let key = &entry.session.project.key;
            if !self.project_order.contains(key) {
                self.project_order.push(key.clone());
            }
        }
        if self.project_order.len() > MAX_PROJECT_ORDER {
            let in_use: HashSet<&str> = self
                .entries
                .iter()
                .map(|entry| entry.session.project.key.as_str())
                .collect();
            let mut excess = self.project_order.len() - MAX_PROJECT_ORDER;
            self.project_order.retain(|key| {
                if excess > 0 && !in_use.contains(key.as_str()) {
                    excess -= 1;
                    false
                } else {
                    true
                }
            });
        }
    }

    fn persisted(&self) -> PersistedState {
        PersistedState {
            sessions: self
                .entries
                .iter()
                .map(|entry| {
                    let session = &entry.session;
                    StoredSession {
                        id: session.id.clone(),
                        kind: session.kind,
                        cwd: session.cwd.clone(),
                        command: session.command.clone(),
                        name: session.name.clone(),
                        auto_title: session.auto_title.clone(),
                        project: Some(session.project.clone()),
                        git: session.git.clone(),
                        created_at: session.created_at,
                        last_activity_at: session.last_activity_at,
                        claude_session_id: session.claude_session_id.clone(),
                    }
                })
                .collect(),
            project_order: self.project_order.clone(),
            layout: Some(self.layout.clone()),
            recent_folders: self.recent_folders.clone(),
            window: self.window,
        }
    }
}

/// What a change did, for [`Shared::mutate`].
#[derive(Default)]
struct Mutation {
    changes: ChangeSet,
    /// Whether something that is saved changed: it is saved after
    /// [`SAVE_DEBOUNCE`], with what changes after it.
    persist: bool,
    /// Whether something the user did on purpose changed, which is saved
    /// at once: a session was created, closed, renamed or moved, the
    /// layout changed, a conversation became known.
    persist_now: bool,
}

struct Shared {
    state: Mutex<State>,
    hub: ChangeHub,
    store: Store,
    config: WorkspaceConfig,
    signals: Arc<Signals>,
    /// The folders whose project is being detected, so that a folder on a
    /// drive that does not answer costs one waiting thread, not one more
    /// every time it is looked at.
    detecting: Mutex<HashSet<PathBuf>>,
}

impl Shared {
    /// Runs a change under the lock, then publishes what it changed and
    /// asks for a save if it needs one.
    fn mutate<R>(&self, change: impl FnOnce(&mut State, &mut Mutation) -> R) -> R {
        let mut state = lock(&self.state);
        let projects_before = state.projects();
        let mut mutation = Mutation::default();
        let result = change(&mut state, &mut mutation);

        state.rank_new_projects();
        if state.projects() != projects_before {
            mutation.changes.projects = true;
        }
        if !mutation.changes.is_empty() {
            state.revision += 1;
            mutation.changes.revision = state.revision;
            // Published under the lock, so that revisions reach the
            // subscribers in order.
            self.hub.publish(&mutation.changes);
        }
        drop(state);
        if mutation.persist_now {
            self.signals.request_save_now();
        } else if mutation.persist {
            self.signals.request_save();
        }
        result
    }

    /// Passes on the activity recorded since the last call. Returns the
    /// folders of the sessions that were active.
    fn flush_activity(&self) -> Vec<PathBuf> {
        self.mutate(|state, mutation| {
            let mut folders = Vec::new();
            for entry in &mut state.entries {
                let stamp = entry.activity.swap(0, Ordering::Relaxed);
                if stamp == 0 {
                    continue;
                }
                folders.push(entry.session.effective_cwd().to_owned());
                if stamp > entry.session.last_activity_at {
                    entry.session.last_activity_at = stamp;
                    mutation.changes.session(&entry.session.id);
                }
            }
            folders
        })
    }

    /// Reads the Git state of the sessions again: of those in `folders`,
    /// or of all of them. Returns at once: the disk is read on threads of
    /// their own, which update the sessions when they are done (see
    /// [`start_detection`](Self::start_detection)).
    fn refresh_git(self: &Arc<Self>, folders: Option<&[PathBuf]>) {
        let mut targets: Vec<PathBuf> = lock(&self.state)
            .entries
            .iter()
            .map(|entry| entry.session.effective_cwd().to_owned())
            .filter(|folder| folders.is_none_or(|folders| folders.contains(folder)))
            .collect();
        targets.sort();
        targets.dedup();
        for folder in targets {
            let _ = self.start_detection(folder);
        }
    }

    /// Starts finding the project of `folder` on a thread of its own. When
    /// it is found, the sessions shown in that folder are updated, however
    /// long it took. Returns where the result arrives, or `None` when that
    /// folder is being detected already or no thread could be started.
    fn start_detection(self: &Arc<Self>, folder: PathBuf) -> Option<mpsc::Receiver<Detected>> {
        if !lock(&self.detecting).insert(folder.clone()) {
            return None;
        }
        let (sender, receiver) = mpsc::channel();
        let shared = Arc::downgrade(self);
        let detector = self.config.detector;
        let spawned = thread::Builder::new().name("termx-git".to_owned()).spawn({
            let folder = folder.clone();
            move || {
                let detected = detector(&folder);
                if let Some(shared) = shared.upgrade() {
                    lock(&shared.detecting).remove(&folder);
                    shared.apply_detected(&folder, &detected);
                }
                let _ = sender.send(detected);
            }
        });
        if let Err(error) = spawned {
            lock(&self.detecting).remove(&folder);
            debug!(error = %error, "the project of a folder could not be detected");
            return None;
        }
        Some(receiver)
    }

    /// The project of `folder`, if it is found within
    /// [`WorkspaceConfig::git_detect_budget`]. When it is not, the
    /// detection goes on in the background (see
    /// [`start_detection`](Self::start_detection)).
    fn detect_within(self: &Arc<Self>, folder: &Path) -> Option<Detected> {
        let receiver = self.start_detection(folder.to_owned())?;
        receiver.recv_timeout(self.config.git_detect_budget).ok()
    }

    /// Shows `detected` for the sessions shown in `folder`.
    fn apply_detected(&self, folder: &Path, detected: &Detected) {
        self.mutate(|state, mutation| {
            for entry in &mut state.entries {
                let session = &mut entry.session;
                // The folder may have changed while the disk was read.
                if session.effective_cwd() != folder {
                    continue;
                }
                if session.project != detected.project || session.git != detected.git {
                    session.project = detected.project.clone();
                    session.git.clone_from(&detected.git);
                    mutation.changes.session(&session.id);
                    mutation.persist = true;
                }
            }
        });
    }

    /// Records what the script of a start said. A marker only ever
    /// improves the state: one that comes late clears the warning.
    fn launch_marker(&self, key: &SessionKey, generation: u64, marker: LaunchMarker) {
        self.mutate(|state, mutation| {
            let Ok(entry) = state.entry(key) else {
                return;
            };
            if entry.generation != generation {
                return;
            }
            let Some(launch) = &mut entry.launch else {
                return;
            };
            match marker {
                LaunchMarker::Exec => {
                    launch.confirmed = true;
                    if entry.session.warning.take().is_some() {
                        mutation.changes.session(key);
                    }
                }
                LaunchMarker::ClaudeMissing => {
                    if !launch.confirmed {
                        launch.claude_missing = true;
                    }
                }
            }
        });
    }

    /// Gives a warning to the sessions whose script has not said in time
    /// that it runs the command. Returns when the next one is due.
    fn flag_unconfirmed(&self, now: Instant) -> Option<Instant> {
        let timeout = self.config.launch_confirm_timeout;
        let waiting = lock(&self.state).entries.iter().any(|entry| {
            entry
                .launch
                .as_ref()
                .is_some_and(|launch| !launch.confirmed && !launch.claude_missing)
                && entry.session.warning.is_none()
        });
        if !waiting {
            return None;
        }
        self.mutate(|state, mutation| {
            let mut next: Option<Instant> = None;
            for entry in &mut state.entries {
                let Some(launch) = &entry.launch else {
                    continue;
                };
                if launch.confirmed || launch.claude_missing || entry.session.warning.is_some() {
                    continue;
                }
                let due = after(launch.started, timeout);
                if due > now {
                    next = Some(next.map_or(due, |next| next.min(due)));
                } else if entry.session.state.is_active() {
                    entry.session.warning = Some(LaunchFailure::launch_unconfirmed());
                    mutation.changes.session(&entry.session.id);
                }
            }
            next
        })
    }

    /// Passes the activity on and writes the state file now.
    fn flush(&self) -> io::Result<bool> {
        *lock(&self.signals.save) = None;
        self.signals.urgent.store(false, Ordering::Release);
        self.signals.activity.store(false, Ordering::Release);
        self.flush_activity();
        self.save()
    }

    /// Writes the state file now, and records whether that worked: see
    /// [`PersistenceMode::Failing`]. A save that fails is tried again after
    /// [`SAVE_RETRY`].
    fn save(&self) -> io::Result<bool> {
        let persisted = lock(&self.state).persisted();
        let result = self.store.save(&persisted);
        let failing = result.as_ref().err().map(ToString::to_string);
        let started_failing = self.mutate(|state, mutation| {
            let next = match (&state.persistence, &failing) {
                (PersistenceMode::Primary, Some(reason)) => PersistenceMode::Failing {
                    reason: reason.clone(),
                    since: now_ms(),
                },
                (PersistenceMode::Failing { since, .. }, Some(reason)) => {
                    PersistenceMode::Failing {
                        reason: reason.clone(),
                        since: *since,
                    }
                }
                (PersistenceMode::Failing { .. }, None) => PersistenceMode::Primary,
                (other, _) => other.clone(),
            };
            let started = matches!(state.persistence, PersistenceMode::Primary)
                && matches!(next, PersistenceMode::Failing { .. });
            if next != state.persistence {
                state.persistence = next;
                mutation.changes.persistence = true;
            }
            started
        });
        if let Err(error) = &result {
            // Once per episode: the retries that follow are the same event.
            if started_failing {
                warn!(error = %error, "the workspace could not be saved; trying again");
            }
            self.signals.request_save_after(self.config.save_retry);
        }
        result
    }

    fn save_logged(&self) {
        if let Ok(true) = self.save() {
            debug!("workspace saved");
        }
    }
}

/// `instant` plus `duration`, or a day later when that cannot be
/// represented: an interval configured as "never" must not be a panic.
fn after(instant: Instant, duration: Duration) -> Instant {
    instant
        .checked_add(duration)
        .or_else(|| instant.checked_add(Duration::from_secs(24 * 60 * 60)))
        .unwrap_or(instant)
}

/// The thread that passes activity on, keeps the Git state current and
/// saves, each no more often than configured.
fn run_ticker(shared: &Arc<Shared>) {
    let signals = &shared.signals;
    let config = &shared.config;
    let mut last_activity: Option<Instant> = None;
    let mut last_git: Option<Instant> = None;
    let due = |last: Option<Instant>, every: Duration, now: Instant| {
        last.is_none_or(|last| now.saturating_duration_since(last) >= every)
    };

    while !signals.stop.load(Ordering::Acquire) {
        let now = Instant::now();

        let mut active = Vec::new();
        if signals.activity.load(Ordering::Acquire)
            && due(last_activity, config.activity_coalesce, now)
        {
            signals.activity.store(false, Ordering::Release);
            active = shared.flush_activity();
            last_activity = Some(now);
        }

        let requested = signals.git.swap(false, Ordering::AcqRel);
        if requested || due(last_git, config.git_poll_interval, now) {
            shared.refresh_git(None);
            last_git = Some(now);
        } else if !active.is_empty() {
            shared.refresh_git(Some(&active));
        }

        let save_at = |request: Option<(Instant, Instant)>| {
            request.map(|(first, last)| {
                after(last, config.save_debounce).min(after(first, config.save_max_wait))
            })
        };
        let save_due = {
            let mut save = lock(&signals.save);
            let mut retry = lock(&signals.retry);
            let due = signals.urgent.swap(false, Ordering::AcqRel)
                || save_at(*save)
                    .into_iter()
                    .chain(*retry)
                    .any(|deadline| deadline <= now);
            if due {
                *save = None;
                *retry = None;
            }
            due
        };
        if save_due {
            shared.save_logged();
        }

        let unconfirmed = shared.flag_unconfirmed(now);

        // Sleep until the earliest thing that is due, or until woken.
        let mut wake = last_git.map_or(now, |last| after(last, config.git_poll_interval));
        if signals.activity.load(Ordering::Acquire) {
            wake =
                wake.min(last_activity.map_or(now, |last| after(last, config.activity_coalesce)));
        }
        if let Some(deadline) = save_at(*lock(&signals.save)) {
            wake = wake.min(deadline);
        }
        if let Some(deadline) = *lock(&signals.retry) {
            wake = wake.min(deadline);
        }
        if let Some(deadline) = unconfirmed {
            wake = wake.min(deadline);
        }
        if signals.git.load(Ordering::Acquire)
            || signals.stop.load(Ordering::Acquire)
            || signals.urgent.load(Ordering::Acquire)
        {
            continue;
        }
        let sleep = wake.saturating_duration_since(Instant::now());
        if !sleep.is_zero() {
            thread::park_timeout(sleep);
        }
    }
}

/// The sessions of the application. See the [module documentation](super).
///
/// Dropping the workspace [shuts it down](Self::shutdown).
pub struct Workspace {
    shared: Arc<Shared>,
    ticker: Mutex<Option<JoinHandle<()>>>,
}

impl Workspace {
    /// Opens the workspace: restores what the state file holds, as dormant
    /// sessions, and starts the thread that keeps it current.
    ///
    /// This never fails. When the state cannot be saved,
    /// [`persistence`](Self::persistence) says why, and the workspace works
    /// without remembering.
    #[must_use]
    pub fn open(config: WorkspaceConfig) -> Self {
        let (store, persisted) = match &config.state_dir {
            Some(dir) => Store::open(dir),
            None => (Store::in_memory(), PersistedState::default()),
        };

        let entries: Vec<Entry> = persisted
            .sessions
            .into_iter()
            .map(|stored| {
                let command = match stored.kind {
                    SessionKind::Background => stored.command,
                    SessionKind::Claude | SessionKind::Shell => None,
                };
                let mut session = Session {
                    id: stored.id,
                    kind: stored.kind,
                    // Shown at once as it was; read again right away.
                    project: stored
                        .project
                        .unwrap_or_else(|| Project::folder(&stored.cwd)),
                    cwd: stored.cwd,
                    live_cwd: None,
                    command,
                    title: String::new(),
                    name: stored.name.and_then(|name| clean_title(&name, false)),
                    auto_title: stored
                        .auto_title
                        .and_then(|title| clean_title(&title, false)),
                    git: stored.git,
                    state: SessionState::Dormant,
                    terminal_id: None,
                    created_at: stored.created_at,
                    last_activity_at: stored.last_activity_at,
                    agent: None,
                    warning: None,
                    claude_session_id: stored
                        .claude_session_id
                        .filter(|id| is_valid_claude_session_id(id)),
                };
                session.refresh_title();
                Entry {
                    session,
                    activity: Arc::default(),
                    generation: 0,
                    launch: None,
                }
            })
            .collect();

        let mut layout = persisted.layout.unwrap_or_default();
        layout.sanitize(|key| entries.iter().any(|entry| entry.session.id == *key));
        let mut state = State {
            entries,
            project_order: persisted.project_order,
            layout,
            recent_folders: persisted.recent_folders,
            window: persisted.window,
            revision: 0,
            persistence: store.mode().clone(),
        };
        state.rank_new_projects();
        info!(
            sessions = state.entries.len(),
            saved = matches!(store.mode(), PersistenceMode::Primary),
            "workspace opened"
        );

        let signals = Arc::new(Signals::default());
        // The projects and branches that were restored may be out of date.
        signals.git.store(true, Ordering::Release);
        let shared = Arc::new(Shared {
            state: Mutex::new(state),
            hub: ChangeHub::default(),
            store,
            config,
            signals,
            detecting: Mutex::default(),
        });

        let ticker = thread::Builder::new()
            .name("termx-workspace".to_owned())
            .spawn({
                let shared = Arc::clone(&shared);
                move || run_ticker(&shared)
            });
        let ticker = match ticker {
            Ok(ticker) => {
                // Set before anyone can ask for anything: a wake-up that
                // comes before the thread first sleeps is not lost.
                let _ = shared.signals.ticker.set(ticker.thread().clone());
                Some(ticker)
            }
            Err(error) => {
                warn!(error = %error, "the workspace thread could not be started; changes are only saved on exit");
                None
            }
        };
        Self {
            shared,
            ticker: Mutex::new(ticker),
        }
    }

    /// Whether the workspace is saved, and why not when it is not.
    #[must_use]
    pub fn persistence(&self) -> PersistenceMode {
        lock(&self.shared.state).persistence.clone()
    }

    /// The path of the state file, when the workspace has a directory.
    #[must_use]
    pub fn state_file(&self) -> Option<PathBuf> {
        self.shared.store.path()
    }

    /// Builds the commands of the sessions.
    #[must_use]
    pub fn launcher(&self) -> &Launcher {
        &self.shared.config.launcher
    }

    /// Everything a frontend needs to draw the workspace.
    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        let state = lock(&self.shared.state);
        Snapshot {
            revision: state.revision,
            sessions: state
                .entries
                .iter()
                .map(|entry| entry.session.clone())
                .collect(),
            projects: state.projects(),
            layout: state.layout.clone(),
            recent_folders: state.recent_folders.clone(),
            persistence: state.persistence.clone(),
        }
    }

    /// The current values of what `changes` says has changed.
    #[must_use]
    pub fn delta(&self, changes: &ChangeSet) -> Delta {
        let state = lock(&self.shared.state);
        let mut removed: Vec<SessionKey> = changes.removed.iter().cloned().collect();
        let mut sessions = Vec::new();
        for key in &changes.sessions {
            match state.entries.iter().find(|entry| entry.session.id == *key) {
                Some(entry) => sessions.push(entry.session.clone()),
                // Removed since: say so, in case that news is still on its
                // way.
                None if !removed.contains(key) => removed.push(key.clone()),
                None => {}
            }
        }
        Delta {
            revision: state.revision,
            sessions,
            removed,
            session_order: changes.session_order.then(|| {
                state
                    .entries
                    .iter()
                    .map(|entry| entry.session.id.clone())
                    .collect()
            }),
            projects: changes.projects.then(|| state.projects()),
            layout: changes.layout.then(|| state.layout.clone()),
            recent_folders: changes.recent_folders.then(|| state.recent_folders.clone()),
            persistence: changes.persistence.then(|| state.persistence.clone()),
        }
    }

    /// Subscribes to the changes of the workspace. See
    /// [`notify`](super::notify). Read [`delta`](Self::delta) for each
    /// change set received.
    ///
    /// To start a frontend without missing a change, subscribe first and
    /// take the [`snapshot`](Self::snapshot) second.
    #[must_use]
    pub fn subscribe(&self) -> Subscription {
        self.shared.hub.subscribe()
    }

    /// The session with this key, as it is now.
    #[must_use]
    pub fn session(&self, key: &SessionKey) -> Option<Session> {
        lock(&self.shared.state)
            .entries
            .iter()
            .find(|entry| entry.session.id == *key)
            .map(|entry| entry.session.clone())
    }

    /// Creates a session, in the [`Starting`](SessionState::Starting)
    /// state: start it with [`begin_launch`](Self::begin_launch). Its
    /// folder becomes the most recent of the recent folders.
    ///
    /// This reads the disk to find the project of the folder: do not call
    /// it from a thread that must stay responsive.
    ///
    /// # Errors
    ///
    /// [`WorkspaceError::InvalidFolder`], [`WorkspaceError::FolderNotFound`]
    /// and, for a background session, [`WorkspaceError::InvalidCommand`].
    pub fn create_session(&self, new: NewSession) -> Result<Session, WorkspaceError> {
        if !new.cwd.is_absolute() || new.cwd.to_str().is_none() {
            return Err(WorkspaceError::InvalidFolder);
        }
        if !new.cwd.is_dir() {
            return Err(WorkspaceError::FolderNotFound(new.cwd));
        }
        let command = match new.kind {
            SessionKind::Background => {
                let command = new.command.as_deref().map(str::trim).unwrap_or_default();
                let valid = !command.is_empty()
                    && command.len() <= MAX_COMMAND_BYTES
                    && !command.chars().any(|c| c.is_control() && c != '\t');
                if !valid {
                    return Err(WorkspaceError::InvalidCommand);
                }
                Some(command.to_owned())
            }
            SessionKind::Claude | SessionKind::Shell => None,
        };
        let detected = self
            .shared
            .detect_within(&new.cwd)
            .unwrap_or_else(|| Detected {
                project: Project::folder(&new.cwd),
                git: None,
            });
        let now = now_ms();
        let mut session = Session {
            id: SessionKey::generate(),
            kind: new.kind,
            cwd: new.cwd,
            live_cwd: None,
            command,
            title: String::new(),
            name: new.name.and_then(|name| clean_title(&name, false)),
            auto_title: None,
            project: detected.project,
            git: detected.git,
            state: SessionState::Starting,
            terminal_id: None,
            created_at: now,
            last_activity_at: now,
            agent: None,
            warning: None,
            claude_session_id: None,
        };
        session.refresh_title();
        info!(session = %session.id, kind = ?session.kind, "session created");

        Ok(self.shared.mutate(|state, mutation| {
            if new.show {
                let focused = state.layout.focused();
                // The focused pane always exists.
                let _ = state.layout.show(focused, Some(session.id.clone()));
                mutation.changes.layout = true;
            }
            state.recent_folders.retain(|folder| *folder != session.cwd);
            state.recent_folders.insert(0, session.cwd.clone());
            state.recent_folders.truncate(MAX_RECENT_FOLDERS);
            mutation.changes.recent_folders = true;
            mutation.changes.session(&session.id);
            mutation.changes.session_order = true;
            mutation.persist_now = true;
            state.entries.push(Entry {
                session: session.clone(),
                activity: Arc::default(),
                generation: 0,
                launch: None,
            });
            session
        }))
    }

    /// Creates another Claude Code session in the folder `key` was created
    /// in, without asking anything: "one more in the same project".
    ///
    /// # Errors
    ///
    /// [`WorkspaceError::UnknownSession`], and those of
    /// [`create_session`](Self::create_session).
    pub fn create_sibling(&self, key: &SessionKey) -> Result<Session, WorkspaceError> {
        let cwd = self
            .session(key)
            .ok_or_else(|| WorkspaceError::UnknownSession(key.clone()))?
            .cwd;
        self.create_session(NewSession::new(SessionKind::Claude, cwd))
    }

    /// Gives a session a name, which is shown instead of any automatic
    /// title from then on. `None`, or a name with nothing to show, removes
    /// the name: the automatic title shows again.
    ///
    /// # Errors
    ///
    /// [`WorkspaceError::UnknownSession`].
    pub fn rename_session(
        &self,
        key: &SessionKey,
        name: Option<&str>,
    ) -> Result<(), WorkspaceError> {
        let name = name.and_then(|name| clean_title(name, false));
        self.shared.mutate(|state, mutation| {
            let session = &mut state.entry(key)?.session;
            if session.name != name {
                session.name = name;
                session.refresh_title();
                mutation.changes.session(key);
                mutation.persist_now = true;
            }
            Ok(())
        })
    }

    /// Records the title a program set for its terminal (OSC 0 and 2), as
    /// observed by the frontend. It becomes the title of the session
    /// unless the user named it: a name is never overwritten.
    ///
    /// The status mark Claude Code animates in front of its title is
    /// dropped, so reporting every frame of it costs nothing. An empty
    /// title removes the automatic title.
    ///
    /// # Errors
    ///
    /// [`WorkspaceError::UnknownSession`].
    pub fn report_title(&self, key: &SessionKey, title: &str) -> Result<(), WorkspaceError> {
        let title = clean_title(title, true);
        self.shared.mutate(|state, mutation| {
            let session = &mut state.entry(key)?.session;
            if session.auto_title != title {
                session.auto_title = title;
                session.refresh_title();
                mutation.changes.session(key);
                mutation.persist = true;
            }
            Ok(())
        })
    }

    /// Records the folder a program says it is in (OSC 7), as observed by
    /// the frontend: a `file://` URL or a plain path. The project and the
    /// branch of the session follow it. A report that is not an existing
    /// local folder is ignored; returns whether it was used.
    ///
    /// This reads the disk: do not call it from a thread that must stay
    /// responsive.
    ///
    /// # Errors
    ///
    /// [`WorkspaceError::UnknownSession`].
    pub fn report_cwd(&self, key: &SessionKey, reported: &str) -> Result<bool, WorkspaceError> {
        let current = self
            .session(key)
            .ok_or_else(|| WorkspaceError::UnknownSession(key.clone()))?;
        let Some(folder) = parse_reported_cwd(reported).filter(|folder| folder.is_dir()) else {
            return Ok(false);
        };
        if current.live_cwd.as_ref() == Some(&folder) {
            return Ok(true);
        }
        // When the folder takes too long to read, its project is shown
        // once the detection ends.
        let detected = self.shared.detect_within(&folder);
        self.shared.mutate(|state, mutation| {
            let session = &mut state.entry(key)?.session;
            // Only a running program has a folder to report.
            if !session.state.is_active() {
                return Ok(false);
            }
            session.live_cwd = Some(folder);
            mutation.changes.session(key);
            if let Some(detected) = detected
                && (session.project != detected.project || session.git != detected.git)
            {
                session.project = detected.project;
                session.git = detected.git;
                mutation.persist = true;
            }
            Ok(true)
        })
    }

    /// Records the identifier Claude Code gave the conversation of a
    /// session, with which it is resumed, or forgets it.
    ///
    /// # Errors
    ///
    /// [`WorkspaceError::UnknownSession`] and
    /// [`WorkspaceError::InvalidClaudeSessionId`].
    pub fn set_claude_session_id(
        &self,
        key: &SessionKey,
        id: Option<&str>,
    ) -> Result<(), WorkspaceError> {
        if id.is_some_and(|id| !is_valid_claude_session_id(id)) {
            return Err(WorkspaceError::InvalidClaudeSessionId);
        }
        self.shared.mutate(|state, mutation| {
            let session = &mut state.entry(key)?.session;
            if session.claude_session_id.as_deref() != id {
                session.claude_session_id = id.map(str::to_owned);
                mutation.changes.session(key);
                mutation.persist_now = true;
            }
            Ok(())
        })
    }

    /// Moves a session just before the session `before` in the order of
    /// the sessions, or to the end.
    ///
    /// # Errors
    ///
    /// [`WorkspaceError::UnknownSession`] for either key.
    pub fn reorder_session(
        &self,
        key: &SessionKey,
        before: Option<&SessionKey>,
    ) -> Result<(), WorkspaceError> {
        self.shared.mutate(|state, mutation| {
            let from = state.position(key)?;
            if let Some(before) = before {
                state.position(before)?;
            }
            if before == Some(key) {
                return Ok(());
            }
            let order_before: Vec<SessionKey> =
                state.entries.iter().map(|e| e.session.id.clone()).collect();
            let entry = state.entries.remove(from);
            let to = match before {
                Some(before) => state.position(before)?,
                None => state.entries.len(),
            };
            state.entries.insert(to, entry);
            if state
                .entries
                .iter()
                .map(|e| &e.session.id)
                .ne(order_before.iter())
            {
                mutation.changes.session_order = true;
                mutation.persist_now = true;
            }
            Ok(())
        })
    }

    /// Moves a project just before the project `before` in the order of
    /// the projects, or to the end.
    ///
    /// # Errors
    ///
    /// [`WorkspaceError::UnknownProject`] for either key, when no session
    /// belongs to it.
    pub fn reorder_project(&self, key: &str, before: Option<&str>) -> Result<(), WorkspaceError> {
        self.shared.mutate(|state, mutation| {
            let in_use = |wanted: &str| state.projects().iter().any(|p| p.key == wanted);
            if !in_use(key) || before.is_some_and(|before| !in_use(before)) {
                return Err(WorkspaceError::UnknownProject);
            }
            if before == Some(key) {
                return Ok(());
            }
            state.rank_new_projects();
            state.project_order.retain(|other| other != key);
            let to = before
                .and_then(|before| state.project_order.iter().position(|k| k == before))
                .unwrap_or(state.project_order.len());
            state.project_order.insert(to, key.to_owned());
            // `mutate` notices whether the order of the projects changed.
            mutation.persist_now = true;
            Ok(())
        })
    }

    /// Stops a session without removing it: it becomes
    /// [`Dormant`](SessionState::Dormant), keeps its place, its name and
    /// its conversation, and can be started again, fresh or resuming.
    /// Returns the terminal it was running in, which the caller must close;
    /// `None` for a session that was not running, which is left as it is.
    ///
    /// # Errors
    ///
    /// [`WorkspaceError::UnknownSession`].
    pub fn stop_session(&self, key: &SessionKey) -> Result<Option<u64>, WorkspaceError> {
        let (terminal, moved) = self.shared.mutate(|state, mutation| {
            let entry = state.entry(key)?;
            if !entry.session.state.is_active() {
                return Ok::<_, WorkspaceError>((None, false));
            }
            // Whatever the old terminal still reports is not news.
            entry.generation += 1;
            entry.launch = None;
            entry.session.warning = None;
            entry.session.state = SessionState::Dormant;
            let terminal = entry.session.terminal_id.take();
            mutation.changes.session(key);
            Ok((terminal, entry.session.live_cwd.take().is_some()))
        })?;
        if moved {
            self.refresh_git();
        }
        info!(session = %key, "session stopped");
        Ok(terminal)
    }

    /// Removes a session that is not running, for good, and empties the
    /// pane that showed it. To end a running one at the same time, use
    /// [`close_session`](Self::close_session).
    ///
    /// # Errors
    ///
    /// [`WorkspaceError::UnknownSession`], and
    /// [`WorkspaceError::SessionRunning`] for a session that is starting
    /// or running, which is left as it is.
    pub fn remove_session(&self, key: &SessionKey) -> Result<(), WorkspaceError> {
        let session = self
            .session(key)
            .ok_or_else(|| WorkspaceError::UnknownSession(key.clone()))?;
        if session.state.is_active() {
            return Err(WorkspaceError::SessionRunning(key.clone()));
        }
        self.remove_if(key, |session| !session.state.is_active())
            .map(|_| ())
    }

    /// Removes a session for good, and empties the pane that showed it.
    /// Returns the terminal it was running in, which the caller must
    /// close.
    ///
    /// # Errors
    ///
    /// [`WorkspaceError::UnknownSession`].
    pub fn close_session(&self, key: &SessionKey) -> Result<Option<u64>, WorkspaceError> {
        self.remove_if(key, |_| true)
    }

    /// Removes the session `key` if `allowed` says so, under the lock.
    fn remove_if(
        &self,
        key: &SessionKey,
        allowed: impl FnOnce(&Session) -> bool,
    ) -> Result<Option<u64>, WorkspaceError> {
        let terminal = self.shared.mutate(|state, mutation| {
            let position = state.position(key)?;
            if !allowed(&state.entries[position].session) {
                return Err(WorkspaceError::SessionRunning(key.clone()));
            }
            let entry = state.entries.remove(position);
            if state.layout.remove_session(key) {
                mutation.changes.layout = true;
            }
            mutation.changes.removed.insert(key.clone());
            mutation.changes.session_order = true;
            mutation.persist_now = true;
            Ok::<_, WorkspaceError>(entry.session.terminal_id)
        })?;
        info!(session = %key, "session removed");
        Ok(terminal)
    }

    /// Prepares the start of a session and returns what to run. The
    /// session becomes [`Starting`](SessionState::Starting); report what
    /// follows with [`terminal_started`](Self::terminal_started),
    /// [`terminal_exited`](Self::terminal_exited) or
    /// [`launch_failed`](Self::launch_failed).
    ///
    /// A session that is running is restarted: close the terminal named by
    /// [`Launch::previous_terminal`].
    ///
    /// # Errors
    ///
    /// [`WorkspaceError::UnknownSession`], and [`WorkspaceError::Launch`]
    /// when there is nothing to run, typically because the folder is gone.
    /// In that case a session that was not running becomes
    /// [`Exited`](SessionState::Exited) with that failure, so that its
    /// pane can say why; one that was running is left running.
    pub fn begin_launch(
        &self,
        key: &SessionKey,
        mode: LaunchMode,
    ) -> Result<Launch, WorkspaceError> {
        let session = self
            .session(key)
            .ok_or_else(|| WorkspaceError::UnknownSession(key.clone()))?;
        let launcher = &self.shared.config.launcher;
        let token = launcher
            .uses_markers(session.kind)
            .then(LaunchToken::generate);
        // Checks that the folder exists: outside the lock.
        let command = launcher.command(&LaunchRequest {
            kind: session.kind,
            cwd: &session.cwd,
            command: session.command.as_deref(),
            claude_session_id: session.claude_session_id.as_deref(),
            mode,
            token: token.as_ref(),
        });

        let launch = self.shared.mutate(|state, mutation| {
            let entry = state.entry(key)?;
            match command {
                Ok(command) => {
                    entry.generation += 1;
                    let previous_terminal = entry.session.terminal_id.take();
                    entry.session.live_cwd = None;
                    entry.session.state = SessionState::Starting;
                    entry.session.warning = None;
                    entry.session.last_activity_at = now_ms();
                    entry.launch = token.as_ref().map(|_| Tracked {
                        started: Instant::now(),
                        confirmed: false,
                        claude_missing: false,
                    });
                    mutation.changes.session(key);
                    Ok(Launch {
                        command,
                        generation: entry.generation,
                        previous_terminal,
                        token,
                    })
                }
                Err(failure) => {
                    if entry.session.terminal_id.is_none() {
                        entry.generation += 1;
                        entry.launch = None;
                        entry.session.warning = None;
                        entry.session.state = SessionState::Exited {
                            code: None,
                            signal: None,
                            failure: Some(failure.clone()),
                        };
                        mutation.changes.session(key);
                    }
                    Err(WorkspaceError::Launch(failure))
                }
            }
        });
        // The ticker watches for a confirmation that does not come.
        self.shared.signals.wake();
        if launch.is_ok() && session.live_cwd.is_some() {
            // The project shown was that of a folder the session has left.
            self.refresh_git();
        }
        launch
    }

    /// Records that the terminal of a launch is running. Returns `false`
    /// when the launch is not the current one of the session any more (the
    /// session was closed, or started again): the caller must then close
    /// the terminal.
    pub fn terminal_started(&self, key: &SessionKey, generation: u64, terminal_id: u64) -> bool {
        self.shared.mutate(|state, mutation| {
            let Ok(entry) = state.entry(key) else {
                return false;
            };
            if entry.generation != generation {
                return false;
            }
            // A process that ended at once may have reported its exit
            // before its start: it stays exited.
            if entry.session.state == SessionState::Starting {
                entry.session.state = SessionState::Running;
                entry.session.terminal_id = Some(terminal_id);
                mutation.changes.session(key);
            }
            true
        })
    }

    /// Records that the process of a launch has exited. The session
    /// becomes [`Exited`](SessionState::Exited), with a
    /// [`LaunchFailure`] when the exit means that the program could not be
    /// found. News of a launch that is not the current one is ignored.
    pub fn terminal_exited(&self, key: &SessionKey, generation: u64, status: &ExitStatus) {
        let moved = self.shared.mutate(|state, mutation| {
            let Ok(entry) = state.entry(key) else {
                return false;
            };
            if entry.generation != generation || !entry.session.state.is_active() {
                return false;
            }
            let failure = match entry.launch.take() {
                Some(launch) if launch.claude_missing => Some(LaunchFailure::claude_not_found(
                    self.shared.config.launcher.claude_program_name(),
                )),
                // The script ended without getting to the command.
                Some(launch) if !launch.confirmed => Some(LaunchFailure::launch_unconfirmed()),
                _ => None,
            };
            entry.session.warning = None;
            entry.session.state = SessionState::Exited {
                code: status.code(),
                signal: status.signal().map(str::to_owned),
                failure,
            };
            entry.session.terminal_id = None;
            entry.session.last_activity_at = now_ms();
            mutation.changes.session(key);
            entry.session.live_cwd.take().is_some()
        });
        if moved {
            // Back in the folder it was created in: show that project.
            self.refresh_git();
        }
    }

    /// Records that the terminal of a launch could not be started. The
    /// session becomes [`Exited`](SessionState::Exited) with `failure`.
    pub fn launch_failed(&self, key: &SessionKey, generation: u64, failure: LaunchFailure) {
        self.shared.mutate(|state, mutation| {
            let Ok(entry) = state.entry(key) else {
                return;
            };
            if entry.generation != generation {
                return;
            }
            entry.session.state = SessionState::Exited {
                code: None,
                signal: None,
                failure: Some(failure),
            };
            entry.session.terminal_id = None;
            entry.session.warning = None;
            entry.launch = None;
            mutation.changes.session(key);
        });
    }

    /// Records that the terminal of a session was taken away without its
    /// process exiting on its own, as when the window that showed it
    /// reloads. The session becomes [`Dormant`](SessionState::Dormant) and
    /// can be resumed. Returns whether the session was active.
    pub fn suspend_session(&self, key: &SessionKey) -> bool {
        let (suspended, moved) = self.shared.mutate(|state, mutation| {
            let Ok(entry) = state.entry(key) else {
                return (false, false);
            };
            if !entry.session.state.is_active() {
                return (false, false);
            }
            // Whatever the old terminal still reports is not news.
            entry.generation += 1;
            entry.session.state = SessionState::Dormant;
            entry.session.terminal_id = None;
            entry.session.warning = None;
            entry.launch = None;
            mutation.changes.session(key);
            (true, entry.session.live_cwd.take().is_some())
        });
        if moved {
            self.refresh_git();
        }
        suspended
    }

    /// The filter to pass the output of the terminal of `launch` through:
    /// it takes the markers of its script out (see
    /// [`launch`](super::launch)) and records what they say in the state
    /// of the session. `None` when the launch has no markers.
    #[must_use]
    pub fn output_filter(
        &self,
        key: &SessionKey,
        launch: &Launch,
    ) -> Option<Box<dyn crate::session::OutputFilter>> {
        let token = launch.token.as_ref()?;
        Some(Box::new(LaunchWatch {
            filter: MarkerFilter::new(token),
            shared: Arc::downgrade(&self.shared),
            key: key.clone(),
            generation: launch.generation,
        }))
    }

    /// A handle that records the output of a session cheaply. `None` for
    /// an unknown session.
    #[must_use]
    pub fn activity_handle(&self, key: &SessionKey) -> Option<ActivityHandle> {
        let state = lock(&self.shared.state);
        let entry = state.entries.iter().find(|e| e.session.id == *key)?;
        Some(ActivityHandle {
            stamp: Arc::clone(&entry.activity),
            signals: Arc::clone(&self.shared.signals),
        })
    }

    /// Asks for the Git state of every session to be read again, on the
    /// thread of the workspace. Returns at once.
    pub fn refresh_git(&self) {
        self.shared.signals.git.store(true, Ordering::Release);
        self.shared.signals.wake();
    }

    /// The layout of the main area.
    #[must_use]
    pub fn layout(&self) -> Layout {
        lock(&self.shared.state).layout.clone()
    }

    /// Splits a pane. See [`Layout::split`].
    ///
    /// # Errors
    ///
    /// [`WorkspaceError::UnknownSession`] for a session that does not
    /// exist, and the errors of [`Layout::split`].
    pub fn split_pane(
        &self,
        pane: PaneId,
        direction: Direction,
        placement: Placement,
        session: Option<SessionKey>,
    ) -> Result<PaneId, WorkspaceError> {
        self.change_layout(session.as_ref(), false, |layout| {
            layout.split(pane, direction, placement, session.clone())
        })
    }

    /// Closes a pane. See [`Layout::close`].
    ///
    /// # Errors
    ///
    /// The errors of [`Layout::close`].
    pub fn close_pane(&self, pane: PaneId) -> Result<(), WorkspaceError> {
        self.change_layout(None, false, |layout| layout.close(pane))
    }

    /// Moves a pane. See [`Layout::move_pane`].
    ///
    /// # Errors
    ///
    /// The errors of [`Layout::move_pane`].
    pub fn move_pane(
        &self,
        pane: PaneId,
        target: PaneId,
        edge: Edge,
    ) -> Result<(), WorkspaceError> {
        self.change_layout(None, false, |layout| layout.move_pane(pane, target, edge))
    }

    /// Resizes a split and returns the ratio that was set. See
    /// [`Layout::resize`].
    ///
    /// # Errors
    ///
    /// The errors of [`Layout::resize`].
    pub fn resize_split(&self, split: SplitId, ratio: f64) -> Result<f64, WorkspaceError> {
        self.change_layout(None, true, |layout| layout.resize(split, ratio))
    }

    /// Gives the focus to a pane. See [`Layout::focus`].
    ///
    /// # Errors
    ///
    /// The errors of [`Layout::focus`].
    pub fn focus_pane(&self, pane: PaneId) -> Result<(), WorkspaceError> {
        self.change_layout(None, true, |layout| layout.focus(pane))
    }

    /// Shows a session in a pane, or empties the pane. See
    /// [`Layout::show`].
    ///
    /// # Errors
    ///
    /// [`WorkspaceError::UnknownSession`] for a session that does not
    /// exist, and the errors of [`Layout::show`].
    pub fn show_session(
        &self,
        pane: PaneId,
        session: Option<SessionKey>,
    ) -> Result<(), WorkspaceError> {
        self.change_layout(session.as_ref(), false, |layout| {
            layout.show(pane, session.clone())
        })
    }

    /// Changes the layout. `frequent` is for the changes that come in
    /// bursts (dragging a divider, moving the focus around), which are
    /// saved after [`SAVE_DEBOUNCE`]; the others are saved at once.
    fn change_layout<R>(
        &self,
        session: Option<&SessionKey>,
        frequent: bool,
        change: impl FnOnce(&mut Layout) -> Result<R, LayoutError>,
    ) -> Result<R, WorkspaceError> {
        self.shared.mutate(|state, mutation| {
            if let Some(session) = session {
                state.position(session)?;
            }
            let before = state.layout.clone();
            let result = change(&mut state.layout)?;
            if state.layout != before {
                mutation.changes.layout = true;
                mutation.persist = true;
                mutation.persist_now = !frequent;
            }
            Ok(result)
        })
    }

    /// The folders sessions were created in, most recent first.
    #[must_use]
    pub fn recent_folders(&self) -> Vec<PathBuf> {
        lock(&self.shared.state).recent_folders.clone()
    }

    /// Removes a folder from the recent folders. Returns whether it was
    /// one of them.
    pub fn forget_recent_folder(&self, folder: &Path) -> bool {
        self.shared.mutate(|state, mutation| {
            let before = state.recent_folders.len();
            state.recent_folders.retain(|other| other != folder);
            let removed = state.recent_folders.len() != before;
            if removed {
                mutation.changes.recent_folders = true;
                mutation.persist_now = true;
            }
            removed
        })
    }

    /// The geometry the window had when it was last recorded.
    #[must_use]
    pub fn window_geometry(&self) -> Option<WindowGeometry> {
        lock(&self.shared.state).window
    }

    /// Records the geometry of the window, to restore it at the next
    /// start. An invalid geometry is ignored. Subscribers are not told:
    /// the window knows its own geometry.
    pub fn set_window_geometry(&self, geometry: WindowGeometry) {
        if !geometry.is_valid() {
            return;
        }
        self.shared.mutate(|state, mutation| {
            if state.window != Some(geometry) {
                state.window = Some(geometry);
                mutation.persist = true;
            }
        });
    }

    /// Saves the workspace now, instead of after the usual delay. Returns
    /// whether the state file was written: it is not when nothing changed,
    /// or when the workspace is not saved at all
    /// ([`persistence`](Self::persistence)).
    ///
    /// # Errors
    ///
    /// The error of the file system. The state file is then unchanged.
    pub fn flush(&self) -> io::Result<bool> {
        self.shared.flush()
    }

    /// Stops the thread of the workspace, saves, and ends every
    /// subscription. Call it before the application exits. The workspace
    /// can still be read afterwards, but changes are no longer saved on
    /// their own. Called automatically when the workspace is dropped.
    ///
    /// This never waits longer than [`WorkspaceConfig::shutdown_wait`] for
    /// the thread to stop, nor as long again for the last save: a disk that
    /// does not answer must not keep the application from quitting. A save
    /// cut short leaves the previous state file, never a damaged one.
    pub fn shutdown(&self) {
        self.shared.signals.stop.store(true, Ordering::Release);
        self.shared.signals.wake();
        let ticker = lock(&self.ticker).take();
        if let Some(ticker) = ticker {
            let wait = self.shared.config.shutdown_wait;
            if join_within(ticker, wait) {
                let shared = Arc::clone(&self.shared);
                match run_within(wait, move || shared.flush()) {
                    Some(Ok(_)) => {}
                    Some(Err(error)) => {
                        warn!(error = %error, "the workspace could not be saved on exit");
                    }
                    None => warn!("the workspace was not saved on exit: the disk did not answer"),
                }
            } else {
                warn!(
                    "the workspace thread did not stop in time; the last changes may not be saved"
                );
            }
            info!("workspace shut down");
        }
        self.shared.hub.close();
    }
}

/// Joins `thread` if it ends within `wait`. Returns whether it did; a
/// thread that does not is left to end on its own.
fn join_within(thread: JoinHandle<()>, wait: Duration) -> bool {
    let deadline = after(Instant::now(), wait);
    while !thread.is_finished() {
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(5));
    }
    // The thread panics only if a bug makes it; nothing depends on it.
    let _ = thread.join();
    true
}

/// Runs `work` on a thread of its own and returns its result if it comes
/// within `wait`.
fn run_within<T: Send + 'static>(
    wait: Duration,
    work: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    let (sender, receiver) = mpsc::channel();
    thread::Builder::new()
        .name("termx-workspace-exit".to_owned())
        .spawn(move || {
            let _ = sender.send(work());
        })
        .ok()?;
    receiver.recv_timeout(wait).ok()
}

impl Drop for Workspace {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl std::fmt::Debug for Workspace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Workspace")
            .field("sessions", &lock(&self.shared.state).entries.len())
            .field("persistence", &lock(&self.shared.state).persistence)
            .finish_non_exhaustive()
    }
}

/// The folder named by what a program reported with OSC 7: a
/// `file://host/path` URL, percent-encoded, or a plain absolute path.
/// `None` for anything else.
///
/// The host is not compared with the name of this machine, which would
/// take a system call that is not available everywhere: a report from a
/// remote shell is only used if the same path exists here.
#[must_use]
pub fn parse_reported_cwd(reported: &str) -> Option<PathBuf> {
    if reported.len() > MAX_REPORTED_CWD_BYTES || reported.contains('\0') {
        return None;
    }
    let path = match reported.strip_prefix("file://") {
        Some(rest) => {
            // The host ends at the first slash; the path includes it.
            let path = &rest[rest.find('/')?..];
            percent_decode(path)?
        }
        None => reported.to_owned(),
    };
    // A Windows path in a URL: `/C:/Users/me`.
    let bytes = path.as_bytes();
    let path = if cfg!(windows)
        && bytes.len() >= 3
        && bytes[0] == b'/'
        && bytes[1].is_ascii_alphabetic()
        && bytes[2] == b':'
    {
        path[1..].replace('/', "\\")
    } else {
        path
    };
    let path = PathBuf::from(path);
    path.is_absolute().then_some(path)
}

/// Decodes `%XX` sequences. `None` if one is malformed or the result is
/// not UTF-8.
fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = text.get(index + 1..index + 3)?;
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reported_folders_are_parsed() {
        let cases: &[(&str, Option<&str>)] = if cfg!(windows) {
            &[
                (
                    "file:///C:/Users/me/My%20Code",
                    Some(r"C:\Users\me\My Code"),
                ),
                ("file://host/C:/x", Some(r"C:\x")),
                (r"C:\Users\me", Some(r"C:\Users\me")),
                ("relative", None),
                ("file://host", None),
            ]
        } else {
            &[
                (
                    "file://host.local/Users/me/My%20Code",
                    Some("/Users/me/My Code"),
                ),
                ("file:///tmp/a%C3%A9", Some("/tmp/a\u{e9}")),
                ("/plain/path", Some("/plain/path")),
                ("relative/path", None),
                ("file://host", None),
                ("file:///bad%2", None),
                ("file:///bad%zz", None),
                ("file:///not-utf8%ff", None),
                ("", None),
                ("/with\0nul", None),
            ]
        };
        for (reported, expected) in cases {
            assert_eq!(
                parse_reported_cwd(reported),
                expected.map(PathBuf::from),
                "{reported:?}"
            );
        }
        assert_eq!(parse_reported_cwd(&"/x".repeat(5000)), None);
    }

    /// Finds the project of a folder, except that it never answers for a
    /// folder named `hangs`: a network drive that does not respond.
    fn hanging_detector(folder: &Path) -> Detected {
        if folder.file_name().is_some_and(|name| name == "hangs") {
            thread::sleep(Duration::from_secs(3600));
        }
        project::detect(folder)
    }

    #[test]
    fn a_folder_that_does_not_answer_stalls_neither_saving_nor_quitting() {
        let dir = tempfile::tempdir().unwrap();
        let hangs = dir.path().join("hangs");
        std::fs::create_dir(&hangs).unwrap();
        let mut config = WorkspaceConfig::new(dir.path().join("state"));
        config.detector = hanging_detector;
        config.git_detect_budget = Duration::from_millis(100);
        config.git_poll_interval = Duration::from_millis(10);
        config.save_debounce = Duration::from_millis(10);
        config.shutdown_wait = Duration::from_secs(1);
        let workspace = Workspace::open(config);

        // Creating the session waits a little, then goes on with the folder
        // as its own project.
        let started = Instant::now();
        let session = workspace
            .create_session(NewSession::new(SessionKind::Shell, &hangs))
            .unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(session.project, Project::folder(&hangs));

        // The thread of the workspace keeps polling Git, and saving.
        workspace
            .rename_session(&session.id, Some("renamed"))
            .unwrap();
        let state = dir
            .path()
            .join("state")
            .join(super::super::store::STATE_FILE);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !std::fs::read_to_string(&state).is_ok_and(|text| text.contains("renamed")) {
            assert!(Instant::now() < deadline, "the workspace was never saved");
            thread::sleep(Duration::from_millis(10));
        }
        // One thread waits on the folder, however often it is looked at.
        assert_eq!(lock(&workspace.shared.detecting).len(), 1);

        let started = Instant::now();
        workspace.shutdown();
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{:?}",
            started.elapsed()
        );
    }

    #[cfg(unix)]
    #[test]
    fn saves_that_fail_are_reported_and_retried_until_they_work() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("state");
        let mut config = WorkspaceConfig::new(&state_dir);
        config.save_retry = Duration::from_millis(20);
        let workspace = Workspace::open(config);
        let subscription = workspace.subscribe();
        let session = workspace
            .create_session(NewSession::new(SessionKind::Shell, dir.path()))
            .unwrap();
        workspace.flush().unwrap();
        assert_eq!(workspace.persistence(), PersistenceMode::Primary);

        // The directory can no longer be written, as on a full disk.
        let mode =
            |bits| std::fs::set_permissions(&state_dir, std::fs::Permissions::from_mode(bits));
        mode(0o500).unwrap();
        workspace
            .rename_session(&session.id, Some("unsaved"))
            .unwrap();
        let failed = workspace.flush();
        if failed.is_ok() {
            // Running as the administrator, who may write anyway.
            mode(0o700).unwrap();
            return;
        }
        let PersistenceMode::Failing { reason, since } = workspace.persistence() else {
            panic!("not failing: {:?}", workspace.persistence());
        };
        assert!(!reason.is_empty());
        assert!(since > 0);
        assert_eq!(workspace.snapshot().persistence, workspace.persistence());
        let mut told = false;
        while let Some(changes) = subscription.try_recv() {
            told |= workspace.delta(&changes).persistence.is_some();
        }
        assert!(told, "the subscribers were not told");

        // Once the disk takes writes again, the next attempt saves.
        mode(0o700).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while workspace.persistence() != PersistenceMode::Primary {
            assert!(Instant::now() < deadline, "saving never recovered");
            thread::sleep(Duration::from_millis(10));
        }
        let text =
            std::fs::read_to_string(state_dir.join(super::super::store::STATE_FILE)).unwrap();
        assert!(text.contains("unsaved"));
    }

    #[test]
    fn what_the_user_does_is_saved_at_once_and_titles_later() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = WorkspaceConfig::new(dir.path().join("state"));
        // Without an immediate save, nothing would be written in this test.
        config.save_debounce = Duration::from_secs(3600);
        config.save_max_wait = Duration::from_secs(3600);
        let workspace = Workspace::open(config);
        let state = dir
            .path()
            .join("state")
            .join(super::super::store::STATE_FILE);
        let saved =
            |needle: &str| std::fs::read_to_string(&state).is_ok_and(|text| text.contains(needle));
        let wait_for = |needle: &str| {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !saved(needle) {
                assert!(Instant::now() < deadline, "{needle:?} was never saved");
                thread::sleep(Duration::from_millis(5));
            }
        };

        let session = workspace
            .create_session(NewSession::new(SessionKind::Shell, dir.path()))
            .unwrap();
        wait_for(session.id.as_str());
        workspace
            .rename_session(&session.id, Some("by-hand"))
            .unwrap();
        wait_for("by-hand");
        let pane = workspace.layout().focused();
        workspace
            .split_pane(pane, Direction::Column, Placement::After, None)
            .unwrap();
        wait_for("column");

        // A title set by a program changes too often to be saved each time.
        workspace.report_title(&session.id, "from-program").unwrap();
        thread::sleep(Duration::from_millis(300));
        assert!(!saved("from-program"));
        workspace.flush().unwrap();
        assert!(saved("from-program"));
    }

    #[test]
    fn a_session_without_a_name_or_a_title_shows_a_default() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = Workspace::open(WorkspaceConfig::in_memory());

        let session = workspace
            .create_session(NewSession::new(SessionKind::Claude, dir.path()))
            .unwrap();

        assert_eq!(session.title, "Claude Code");
    }
}
