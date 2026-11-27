//! The termx plugin of Claude Code: installing it, upgrading it, removing
//! it, and remembering whether the user wants it.
//!
//! # What is installed, and where
//!
//! The plugin ([`plugin`]) is a directory of termx, which Claude Code knows
//! as a local marketplace. Installing it does two things:
//!
//! 1. writes the plugin into [`Paths::plugin_dir`] (in the state directory
//!    of termx), with a copy of the hook sender;
//! 2. adds two members to the user settings of Claude Code,
//!    `settings.json` in [`Paths::config_dir`] (`$CLAUDE_CONFIG_DIR`, else
//!    `~/.claude`):
//!
//! ```json
//! "extraKnownMarketplaces": { "termx": { "source": { "source": "directory", "path": "<plugin dir>" } } },
//! "enabledPlugins": { "termx@termx": true }
//! ```
//!
//! That is the documented way to declare a marketplace and enable one of
//! its plugins from the settings; Claude Code loads a plugin of a local
//! directory in place, at the start of its next session. Nothing else in
//! the file changes: the edit is made in the text ([`json_edit`]), which
//! keeps the order, the formatting and every other member (the user's own
//! hooks among them), and is checked before it is written. The file is
//! copied to `settings.json.termx-backup` before termx first changes it,
//! and written atomically. A file that is not valid JSON is refused, never
//! "repaired".
//!
//! Uninstalling removes exactly those two members (and the objects they
//! were in, if they are left empty), what Claude Code recorded of this
//! plugin in its own state (`plugins/known_marketplaces.json`,
//! `plugins/installed_plugins.json`, `plugins/cache/termx`), and the plugin
//! directory. The plugin also declares `"defaultEnabled": false`, so that
//! without its member of `enabledPlugins` it is off even where a copy of it
//! is left.
//!
//! # Versions
//!
//! The plugin carries the version of termx ([`plugin::version`]). A termx
//! that finds the plugin of another version, or files that are not what it
//! would write (a hook sender that moved), reports it
//! [`Outdated`](PluginStatus::Outdated); [`Integration::upgrade`] rewrites
//! the plugin directory. The settings are only touched when they do not
//! point at the directory any more.
//!
//! # Consent
//!
//! Nothing is installed unless the user said yes. Their answer is kept in
//! [`CONSENT_FILE`], in the state directory of termx.

pub mod json_edit;
pub mod plugin;

use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use self::json_edit::{Document, EditError};
use crate::diagnostics::redact;
use crate::diagnostics::tracing::{info, warn};

/// The file, in the state directory of termx, that keeps the answer of the
/// user.
pub const CONSENT_FILE: &str = "claude-integration.json";

/// The directory of the plugin, in the state directory of termx.
pub const PLUGIN_DIR: &str = "claude-plugin";

/// The copy of the settings made before termx first changes them.
pub const BACKUP_SUFFIX: &str = ".termx-backup";

/// The name of the hook sender, next to the termx executable.
pub const SENDER: &str = if cfg!(windows) {
    "termx-cli.exe"
} else {
    "termx-cli"
};

/// Where everything is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// The configuration directory of Claude Code: `$CLAUDE_CONFIG_DIR`,
    /// else `~/.claude`.
    pub config_dir: PathBuf,
    /// Where the plugin is written.
    pub plugin_dir: PathBuf,
    /// Where the answer of the user is kept. `None` to keep it nowhere.
    pub consent_file: Option<PathBuf>,
    /// The hook sender to copy into the plugin.
    pub sender: PathBuf,
    /// The home directory, to show paths with `~` and to look for
    /// `claude`.
    pub home: Option<PathBuf>,
    /// The `PATH` to look for `claude` on.
    pub search_path: Option<OsString>,
}

impl Paths {
    /// The paths of this user: Claude Code's from the environment, termx's
    /// in `state_dir`, the hook sender next to the running executable.
    #[must_use]
    pub fn of_user(state_dir: &Path) -> Self {
        let home = std::env::home_dir();
        let config_dir = config_dir_from(std::env::var_os("CLAUDE_CONFIG_DIR"), home.as_deref());
        let sender = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join(SENDER)))
            .unwrap_or_else(|| PathBuf::from(SENDER));
        Self {
            config_dir,
            plugin_dir: state_dir.join(PLUGIN_DIR),
            consent_file: Some(state_dir.join(CONSENT_FILE)),
            sender,
            home,
            search_path: std::env::var_os("PATH"),
        }
    }

    /// The user settings of Claude Code.
    #[must_use]
    pub fn settings(&self) -> PathBuf {
        self.config_dir.join("settings.json")
    }
}

/// `$CLAUDE_CONFIG_DIR` when it is set, else `~/.claude`.
#[must_use]
pub fn config_dir_from(variable: Option<OsString>, home: Option<&Path>) -> PathBuf {
    match variable.filter(|value| !value.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => home.map_or_else(|| PathBuf::from(".claude"), |home| home.join(".claude")),
    }
}

/// What the user answered when asked whether to install the plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Consent {
    /// Not asked yet.
    Unasked,
    /// Yes.
    Accepted,
    /// No.
    Declined,
}

/// Whether the plugin is installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PluginStatus {
    /// The settings do not enable it.
    NotInstalled,
    /// Enabled, and what this termx writes.
    Installed,
    /// Enabled, from another version of termx or with files that are not
    /// what this termx writes: [`Integration::upgrade`].
    Outdated,
    /// What is installed cannot be told: see [`IntegrationStatus::error`].
    Error,
}

/// Why something could not be done, with a message for the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[error("{message}")]
pub struct IntegrationError {
    /// What a frontend can act on.
    pub kind: IntegrationErrorKind,
    /// For the user.
    pub message: String,
}

/// The kinds of [`IntegrationError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum IntegrationErrorKind {
    /// The settings of Claude Code are not valid JSON (or not an object, or
    /// a member termx needs is not an object). termx changes nothing.
    InvalidSettings,
    /// The settings already declare a marketplace named `termx` that is
    /// not this one.
    Conflict,
    /// The hook sender is not next to termx (a build without it).
    SenderMissing,
    /// Reading or writing a file failed.
    Io,
}

impl IntegrationError {
    fn new(kind: IntegrationErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    fn io(what: &str, path: &Path, error: &io::Error) -> Self {
        Self::new(
            IntegrationErrorKind::Io,
            format!("{what} {}: {error}", redact::path(path)),
        )
    }
}

/// Everything a frontend shows about the integration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationStatus {
    /// Whether Claude Code seems to be installed: `claude` is on the
    /// `PATH` or in one of the places its installers use, or its
    /// configuration directory exists.
    pub claude_installed: bool,
    /// Whether the plugin is installed.
    pub plugin: PluginStatus,
    /// The version of the plugin that is installed, if one is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// What the user answered.
    pub consent: Consent,
    /// Why the status is [`PluginStatus::Error`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<IntegrationError>,
}

/// One change installing makes, for the user to read before saying yes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    /// The file or directory, with the home directory as `~`.
    pub path: String,
    /// What changes in it.
    pub description: String,
}

/// The termx plugin of Claude Code, for one user. See the
/// [module documentation](self).
#[derive(Debug, Clone)]
pub struct Integration {
    paths: Paths,
}

impl Integration {
    /// The integration at `paths`.
    #[must_use]
    pub fn new(paths: Paths) -> Self {
        Self { paths }
    }

    /// Where everything is.
    #[must_use]
    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    /// Whether the plugin is installed, and what the user answered.
    #[must_use]
    pub fn status(&self) -> IntegrationStatus {
        let consent = self.consent();
        let claude_installed = self.claude_installed();
        let (plugin, version, error) = match self.plugin_status() {
            Ok((status, version)) => (status, version, None),
            Err(error) => (PluginStatus::Error, None, Some(error)),
        };
        IntegrationStatus {
            claude_installed,
            plugin,
            version,
            consent,
            error,
        }
    }

    /// What [`install`](Self::install) would change, for the user to read.
    /// Empty when the plugin is installed as this termx would install it.
    ///
    /// # Errors
    ///
    /// The reasons [`install`](Self::install) would fail for.
    pub fn preview(&self) -> Result<Vec<Change>, IntegrationError> {
        let settings = self.paths.settings();
        let document = self.read_settings()?;
        let (marketplace, enabled) = self.check_settings(document.as_ref())?;
        let binary = self.sender_binary()?;
        let mut changes = Vec::new();
        if !self.plugin_files_current(&binary) {
            changes.push(Change {
                path: self.shown(&self.paths.plugin_dir),
                description: format!(
                    "Write the termx plugin for Claude Code: hooks for {} events ({}), which run {} hook. Outside termx it exits at once and does nothing.",
                    plugin::hooked_events().len(),
                    plugin::hooked_events().join(", "),
                    self.shown(&self.sender_copy(&binary)),
                ),
            });
        }
        if document.is_some() && !marketplace.is_present() && !self.backup_path().exists() {
            changes.push(Change {
                path: self.shown(&self.backup_path()),
                description: "Copy your Claude Code settings here before changing them.".to_owned(),
            });
        }
        if !matches!(marketplace, Present::Ours) {
            changes.push(Change {
                path: self.shown(&settings),
                description: format!(
                    "Add \"termx\" to \"extraKnownMarketplaces\": a local marketplace in {}. Nothing else in the file changes.",
                    self.shown(&self.paths.plugin_dir)
                ),
            });
        }
        if !enabled {
            changes.push(Change {
                path: self.shown(&settings),
                description: format!(
                    "Set \"{}\" to true in \"enabledPlugins\". Nothing else in the file changes.",
                    plugin::PLUGIN_ID
                ),
            });
        }
        Ok(changes)
    }

    /// Installs the plugin, or brings it up to date. Doing it again changes
    /// nothing.
    ///
    /// # Errors
    ///
    /// [`IntegrationErrorKind::InvalidSettings`] and
    /// [`IntegrationErrorKind::Conflict`] before anything is written;
    /// [`IntegrationErrorKind::SenderMissing`];
    /// [`IntegrationErrorKind::Io`].
    pub fn install(&self) -> Result<(), IntegrationError> {
        let document = self.read_settings()?;
        self.check_settings(document.as_ref())?;
        let binary = self.sender_binary()?;
        self.write_plugin(&binary)?;
        self.edit_settings(|document| {
            let path = self.paths.plugin_dir.to_string_lossy().into_owned();
            document
                .set(
                    &["extraKnownMarketplaces"],
                    plugin::NAME,
                    &json!({ "source": { "source": "directory", "path": path } }),
                )
                .map_err(|error| self.settings_error(&error))?;
            document
                .set(&["enabledPlugins"], plugin::PLUGIN_ID, &Value::Bool(true))
                .map_err(|error| self.settings_error(&error))
        })?;
        info!(version = %plugin::version(), "claude code plugin installed");
        Ok(())
    }

    /// Rewrites the plugin when it is [`Outdated`](PluginStatus::Outdated).
    /// Returns whether it did. Never installs a plugin that is not
    /// installed: that takes the consent of the user.
    ///
    /// # Errors
    ///
    /// Those of [`install`](Self::install).
    pub fn upgrade(&self) -> Result<bool, IntegrationError> {
        if self.plugin_status()?.0 != PluginStatus::Outdated {
            return Ok(false);
        }
        self.install()?;
        Ok(true)
    }

    /// Removes the plugin: its two members of the settings, its entry in
    /// the state of Claude Code, its directory. Doing it again changes
    /// nothing.
    ///
    /// # Errors
    ///
    /// [`IntegrationErrorKind::InvalidSettings`] before anything is
    /// changed, and [`IntegrationErrorKind::Io`].
    pub fn uninstall(&self) -> Result<(), IntegrationError> {
        let document = self.read_settings()?;
        let ours = document
            .as_ref()
            .is_some_and(|document| self.marketplace_in(document) == Present::Ours);
        self.edit_settings(|document| {
            document
                .remove(&["enabledPlugins"], plugin::PLUGIN_ID, true)
                .map_err(|error| self.settings_error(&error))?;
            if ours {
                document
                    .remove(&["extraKnownMarketplaces"], plugin::NAME, true)
                    .map_err(|error| self.settings_error(&error))?;
            }
            Ok(())
        })?;
        self.forget_in_claude_state();
        match fs::remove_dir_all(&self.paths.plugin_dir) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(IntegrationError::io(
                    "could not remove",
                    &self.paths.plugin_dir,
                    &error,
                ));
            }
        }
        info!("claude code plugin uninstalled");
        Ok(())
    }

    /// What the user answered.
    #[must_use]
    pub fn consent(&self) -> Consent {
        #[derive(Deserialize)]
        struct Stored {
            consent: Consent,
        }
        self.paths
            .consent_file
            .as_ref()
            .and_then(|file| fs::read(file).ok())
            .and_then(|bytes| serde_json::from_slice::<Stored>(&bytes).ok())
            .map_or(Consent::Unasked, |stored| stored.consent)
    }

    /// Remembers what the user answered.
    ///
    /// # Errors
    ///
    /// [`IntegrationErrorKind::Io`].
    pub fn set_consent(&self, consent: Consent) -> Result<(), IntegrationError> {
        let Some(file) = &self.paths.consent_file else {
            return Ok(());
        };
        let content = json!({ "version": 1, "consent": consent, "at": crate::workspace::now_ms() });
        if let Some(dir) = file.parent() {
            fs::create_dir_all(dir)
                .map_err(|error| IntegrationError::io("could not create", dir, &error))?;
        }
        write_atomic(file, format!("{content}\n").as_bytes(), None)
            .map_err(|error| IntegrationError::io("could not write", file, &error))
    }

    /// The status of the plugin and its version.
    fn plugin_status(&self) -> Result<(PluginStatus, Option<String>), IntegrationError> {
        let document = self.read_settings()?;
        let Some(document) = document else {
            return Ok((PluginStatus::NotInstalled, None));
        };
        let enabled = document
            .get(&["enabledPlugins", plugin::PLUGIN_ID])
            .is_some_and(|value| value == Value::Bool(true));
        let marketplace = self.marketplace_in(&document);
        if !enabled || marketplace == Present::Absent {
            return Ok((PluginStatus::NotInstalled, None));
        }
        let version = plugin::installed_version(&self.paths.plugin_dir);
        let current = marketplace == Present::Ours
            && self
                .sender_binary()
                .is_ok_and(|binary| self.plugin_files_current(&binary));
        let status = if current {
            PluginStatus::Installed
        } else {
            PluginStatus::Outdated
        };
        Ok((status, version))
    }

    /// The marketplace `termx` the settings declare, and whether the plugin
    /// is enabled. Refuses settings termx cannot change safely.
    fn check_settings(
        &self,
        document: Option<&Document>,
    ) -> Result<(Present, bool), IntegrationError> {
        let Some(document) = document else {
            return Ok((Present::Absent, false));
        };
        for member in ["extraKnownMarketplaces", "enabledPlugins"] {
            if document
                .get(&[member])
                .is_some_and(|value| !value.is_object())
            {
                return Err(IntegrationError::new(
                    IntegrationErrorKind::InvalidSettings,
                    format!(
                        "\"{member}\" in {} is not an object: termx does not change it",
                        self.shown(&self.paths.settings())
                    ),
                ));
            }
        }
        let marketplace = self.marketplace_in(document);
        if marketplace == Present::Other {
            return Err(IntegrationError::new(
                IntegrationErrorKind::Conflict,
                format!(
                    "{} already declares another marketplace named \"termx\"",
                    self.shown(&self.paths.settings())
                ),
            ));
        }
        let enabled = document
            .get(&["enabledPlugins", plugin::PLUGIN_ID])
            .is_some_and(|value| value == Value::Bool(true));
        Ok((marketplace, enabled))
    }

    /// Whether the settings declare the marketplace `termx`, and whether it
    /// is this one.
    fn marketplace_in(&self, document: &Document) -> Present {
        let Some(entry) = document.get(&["extraKnownMarketplaces", plugin::NAME]) else {
            return Present::Absent;
        };
        let source = &entry["source"];
        let ours = source["source"] == "directory"
            && source["path"]
                .as_str()
                .is_some_and(|path| same_path(Path::new(path), &self.paths.plugin_dir));
        if ours { Present::Ours } else { Present::Other }
    }

    /// The settings, or `None` when there is no file.
    fn read_settings(&self) -> Result<Option<Document>, IntegrationError> {
        let path = self.paths.settings();
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) if error.kind() == io::ErrorKind::InvalidData => {
                return Err(IntegrationError::new(
                    IntegrationErrorKind::InvalidSettings,
                    format!("{} is not text", self.shown(&path)),
                ));
            }
            Err(error) => return Err(IntegrationError::io("could not read", &path, &error)),
        };
        if text.trim().is_empty() {
            return Ok(Some(
                Document::parse("{}").map_err(|error| self.settings_error(&error))?,
            ));
        }
        Document::parse(&text)
            .map(Some)
            .map_err(|error| self.settings_error(&error))
    }

    fn settings_error(&self, error: &EditError) -> IntegrationError {
        IntegrationError::new(
            IntegrationErrorKind::InvalidSettings,
            format!(
                "{} cannot be changed safely ({error}). termx changed nothing; fix the file or remove it, then try again.",
                self.shown(&self.paths.settings())
            ),
        )
    }

    /// Changes the settings with `change`, if that changes anything: backs
    /// them up the first time, writes them atomically, and does it again
    /// if Claude Code wrote them in the meantime.
    fn edit_settings(
        &self,
        change: impl Fn(&mut Document) -> Result<(), IntegrationError>,
    ) -> Result<(), IntegrationError> {
        let settings = self.paths.settings();
        // A link (to a repository of dotfiles) stays a link: its target
        // is what changes.
        let target = fs::canonicalize(&settings).unwrap_or_else(|_| settings.clone());
        for _ in 0..3 {
            let before = match fs::read(&target) {
                Ok(bytes) => Some(bytes),
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(error) => {
                    return Err(IntegrationError::io("could not read", &settings, &error));
                }
            };
            let text = before
                .as_deref()
                .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
                .filter(|text| !text.trim().is_empty())
                .unwrap_or_else(|| "{}\n".to_owned());
            let mut document =
                Document::parse(&text).map_err(|error| self.settings_error(&error))?;
            change(&mut document)?;
            let edited = document.text();
            if edited == text {
                return Ok(());
            }
            if before.is_none() && edited.trim().replace(char::is_whitespace, "") == "{}" {
                return Ok(());
            }
            if let Some(before) = &before {
                self.back_up(before)?;
            }
            if let Some(dir) = target.parent() {
                fs::create_dir_all(dir)
                    .map_err(|error| IntegrationError::io("could not create", dir, &error))?;
            }
            // Claude Code may have written the file since it was read.
            let now = fs::read(&target).ok();
            if now != before {
                continue;
            }
            return write_atomic(&target, edited.as_bytes(), Some(&target))
                .map_err(|error| IntegrationError::io("could not write", &settings, &error));
        }
        Err(IntegrationError::new(
            IntegrationErrorKind::Io,
            format!(
                "{} kept changing while termx edited it; try again",
                self.shown(&settings)
            ),
        ))
    }

    /// Copies the settings before termx first changes them.
    fn back_up(&self, content: &[u8]) -> Result<(), IntegrationError> {
        let backup = self.backup_path();
        if backup.exists() {
            return Ok(());
        }
        write_atomic(&backup, content, Some(&self.paths.settings()))
            .map_err(|error| IntegrationError::io("could not back up to", &backup, &error))
    }

    fn backup_path(&self) -> PathBuf {
        let mut name = self.paths.settings().into_os_string();
        name.push(BACKUP_SUFFIX);
        PathBuf::from(name)
    }

    /// The hook sender, read.
    fn sender_binary(&self) -> Result<Vec<u8>, IntegrationError> {
        fs::read(&self.paths.sender).map_err(|error| {
            IntegrationError::new(
                IntegrationErrorKind::SenderMissing,
                format!(
                    "the hook sender of termx is missing ({}: {error}); build it with `cargo build -p termx-cli`",
                    self.shown(&self.paths.sender)
                ),
            )
        })
    }

    /// Where the copy of `binary` goes.
    fn sender_copy(&self, binary: &[u8]) -> PathBuf {
        self.paths
            .plugin_dir
            .join(plugin::NAME)
            .join("bin")
            .join(plugin::sender_name(binary))
    }

    /// Whether the plugin directory holds what this termx writes.
    fn plugin_files_current(&self, binary: &[u8]) -> bool {
        plugin::files(&self.paths.plugin_dir, binary)
            .iter()
            .all(|file| {
                let path = self.paths.plugin_dir.join(&file.path);
                if file.executable {
                    fs::metadata(&path).is_ok_and(|meta| meta.len() == file.content.len() as u64)
                } else {
                    fs::read(&path).is_ok_and(|content| content == file.content)
                }
            })
    }

    /// Writes the plugin directory, and removes the copies of the hook
    /// sender it no longer uses.
    fn write_plugin(&self, binary: &[u8]) -> Result<(), IntegrationError> {
        let files = plugin::files(&self.paths.plugin_dir, binary);
        for file in &files {
            let path = self.paths.plugin_dir.join(&file.path);
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir)
                    .map_err(|error| IntegrationError::io("could not create", dir, &error))?;
            }
            let unchanged = if file.executable {
                fs::metadata(&path).is_ok_and(|meta| meta.len() == file.content.len() as u64)
            } else {
                fs::read(&path).is_ok_and(|content| content == file.content)
            };
            if unchanged {
                continue;
            }
            write_atomic(&path, &file.content, None)
                .map_err(|error| IntegrationError::io("could not write", &path, &error))?;
            if file.executable {
                make_executable(&path).map_err(|error| {
                    IntegrationError::io("could not set the mode of", &path, &error)
                })?;
            }
        }
        // Older copies of the hook sender stay: Claude Code may have
        // cached the hooks of an earlier version of the plugin, which name
        // them, and a hook that runs a file that is gone is an error in
        // every session. They go with the plugin when it is uninstalled.
        Ok(())
    }

    /// Removes what Claude Code recorded of the plugin in its own state
    /// (`plugins/` of its configuration directory), when it is this one:
    /// the marketplace in `known_marketplaces.json`, the plugin in
    /// `installed_plugins.json`, and its copy in `cache/termx`. Otherwise an
    /// installed copy could keep running hooks whose program is gone. Best
    /// effort: these files belong to Claude Code, and only the entries of
    /// termx are touched.
    fn forget_in_claude_state(&self) {
        let plugins = self.paths.config_dir.join("plugins");
        let known = plugins.join("known_marketplaces.json");
        let ours = self.edit_claude_state(&known, |document| {
            let ours = document
                .get(&[plugin::NAME, "source", "path"])
                .and_then(|path| path.as_str().map(PathBuf::from))
                .is_some_and(|path| same_path(&path, &self.paths.plugin_dir));
            ours && document.remove(&[], plugin::NAME, false).unwrap_or(false)
        });
        // The cache of the marketplace "termx" holds copies of this plugin
        // only when the marketplace was this one.
        let cache = plugins.join("cache").join(plugin::NAME);
        let cached_is_ours = ours || is_termx_cache(&cache.join(plugin::NAME));
        if !cached_is_ours {
            return;
        }
        self.edit_claude_state(&plugins.join("installed_plugins.json"), |document| {
            document
                .remove(&["plugins"], plugin::PLUGIN_ID, false)
                .unwrap_or(false)
        });
        if let Err(error) = fs::remove_dir_all(&cache)
            && error.kind() != io::ErrorKind::NotFound
        {
            warn!(error = %error, "the copy claude code made of the plugin could not be removed");
        }
    }

    /// Applies `change` to a JSON file of Claude Code, and writes it if it
    /// says it changed something. Returns what `change` returned.
    fn edit_claude_state(&self, file: &Path, change: impl FnOnce(&mut Document) -> bool) -> bool {
        let Ok(text) = fs::read_to_string(file) else {
            return false;
        };
        let Ok(mut document) = Document::parse(&text) else {
            return false;
        };
        if !change(&mut document) {
            return false;
        }
        if let Err(error) = write_atomic(file, document.text().as_bytes(), Some(file)) {
            warn!(error = %error, "the state of claude code could not be updated");
        }
        true
    }

    /// Whether Claude Code seems to be installed.
    fn claude_installed(&self) -> bool {
        let names: &[&str] = if cfg!(windows) {
            &["claude.exe", "claude.cmd", "claude"]
        } else {
            &["claude"]
        };
        let mut dirs: Vec<PathBuf> = self
            .paths
            .search_path
            .as_ref()
            .map(|path| std::env::split_paths(path).collect())
            .unwrap_or_default();
        if let Some(home) = &self.paths.home {
            // Where the installers of Claude Code put it, which a desktop
            // application does not have on its `PATH`.
            dirs.push(home.join(".local").join("bin"));
            dirs.push(home.join(".claude").join("local"));
            dirs.push(home.join(".npm-global").join("bin"));
            if cfg!(windows) {
                dirs.push(home.join("AppData").join("Roaming").join("npm"));
            }
        }
        if !cfg!(windows) {
            dirs.push(PathBuf::from("/opt/homebrew/bin"));
            dirs.push(PathBuf::from("/usr/local/bin"));
        }
        dirs.iter()
            .any(|dir| names.iter().any(|name| dir.join(name).is_file()))
            || self.paths.config_dir.join("settings.json").is_file()
            || self.paths.config_dir.join(".claude.json").is_file()
    }

    /// `path` as it is shown to the user: the home directory as `~`.
    fn shown(&self, path: &Path) -> String {
        if let Some(home) = &self.paths.home
            && let Ok(rest) = path.strip_prefix(home)
        {
            let separator = if cfg!(windows) { "\\" } else { "/" };
            if rest.as_os_str().is_empty() {
                return "~".to_owned();
            }
            return format!("~{separator}{}", rest.display());
        }
        path.display().to_string()
    }
}

/// Whether the settings declare the marketplace, and whose it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Present {
    Absent,
    Ours,
    Other,
}

impl Present {
    fn is_present(self) -> bool {
        self != Self::Absent
    }
}

/// Whether `dir`, a directory of the cache of Claude Code, holds copies of
/// the termx plugin: its versions have the manifest termx writes.
fn is_termx_cache(dir: &Path) -> bool {
    let Ok(versions) = fs::read_dir(dir) else {
        return false;
    };
    versions.flatten().any(|version| {
        fs::read(version.path().join(".claude-plugin").join("plugin.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .is_some_and(|manifest| {
                manifest["name"] == plugin::NAME
                    && manifest["homepage"] == "https://github.com/Jibaru/termx"
            })
    })
}

/// Whether two paths name the same directory, the second existing or not.
fn same_path(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Writes `bytes` to `path` through a temporary file in the same directory,
/// renamed over it, so that a reader never sees half a file. The file gets
/// the permissions of `like`, when it exists.
fn write_atomic(path: &Path, bytes: &[u8], like: Option<&Path>) -> io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temporary = dir.join(format!(".{name}.termx-{}", crate::workspace::random_hex()));
    let result = (|| {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        if let Some(permissions) = like
            .and_then(|like| fs::metadata(like).ok())
            .map(|meta| meta.permissions())
        {
            fs::set_permissions(&temporary, permissions)?;
        }
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(unix)]
fn make_executable(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> io::Result<()> {
    Ok(())
}
