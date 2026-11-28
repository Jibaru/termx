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
//! A settings file that is read-only, or a link to a file that does not
//! exist, is refused too. The file is written through a temporary file
//! renamed over it; just before the rename termx reads the file again, and
//! starts over once if Claude Code (or anything) changed it meanwhile. What
//! is left is the instant between that check and the rename: a write by
//! another program in that instant is lost.
//!
//! # Ownership, and giving everything back
//!
//! termx only ever removes what it added. When it installs, it records in
//! [`RECORD_FILE`] (in its state directory, next to the plugin) what was
//! there before: the whole text of the settings, whether
//! `extraKnownMarketplaces` and `enabledPlugins` existed, whether
//! `termx@termx` was already enabled; and a hash of the settings as it left
//! them. Uninstalling then:
//!
//! - when the settings are exactly as termx left them, writes back the text
//!   they had before, byte for byte (or removes the file it created);
//! - when they changed since, removes only its two members, and the
//!   objects that hold them only if termx created those objects;
//! - never touches a marketplace named `termx` that is not this directory,
//!   nor a `"termx@termx": false` the user set: those are reported as a
//!   [`Conflict`](PluginStatus::Conflict), and installing is refused;
//! - removes what Claude Code recorded of the plugin
//!   (`plugins/known_marketplaces.json`, `plugins/installed_plugins.json`,
//!   `plugins/cache/termx`) only when Claude Code recorded it for this
//!   directory.
//!
//! The plugin also declares `"defaultEnabled": false`, so that without its
//! member of `enabledPlugins` it is off even where a copy of it is left.
//!
//! # Hooks turned off by policy
//!
//! `allowManagedHooksOnly` in the managed settings of Claude Code blocks
//! the hooks of every plugin the managed settings do not force-enable, and
//! `disableAllHooks` blocks them as well. termx reads the managed settings
//! ([`Paths::managed_settings`]) and the user settings and reports an
//! installed plugin whose hooks cannot run as
//! [`Blocked`](PluginStatus::Blocked).
//!
//! # Versions
//!
//! The plugin carries the version of termx ([`plugin::version`]). A termx
//! that finds the plugin of another version, or files that are not what it
//! would write (a hook sender that moved), reports it
//! [`Outdated`](PluginStatus::Outdated); [`Integration::upgrade`] rewrites
//! the plugin directory and leaves the settings alone.
//!
//! The user-level `extraKnownMarketplaces` with a `directory` source is
//! documented for project settings; that Claude Code also registers it from
//! the user settings, without a prompt, rests on the documentation of
//! plugin loading and on a check with Claude Code 2.1.321, whose
//! `claude plugin marketplace add` and `install` write the same two
//! members.
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

/// The copy of the settings made before termx first changes them. Never
/// overwritten: it keeps the settings as they were before the first
/// install.
pub const BACKUP_SUFFIX: &str = ".termx-backup";

/// What termx found before it installed (see the
/// [module documentation](self#ownership-and-giving-everything-back)),
/// next to [`PLUGIN_DIR`].
pub const RECORD_FILE: &str = "claude-plugin-install.json";

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
    /// The managed settings of Claude Code, which can block the hooks of
    /// plugins: see [`managed_settings_files`].
    pub managed_settings: Vec<PathBuf>,
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
            managed_settings: managed_settings_files(),
        }
    }

    /// The user settings of Claude Code.
    #[must_use]
    pub fn settings(&self) -> PathBuf {
        self.config_dir.join("settings.json")
    }

    /// Where termx records what it found before installing.
    #[must_use]
    pub fn record(&self) -> PathBuf {
        self.plugin_dir
            .parent()
            .map_or_else(|| PathBuf::from(RECORD_FILE), |dir| dir.join(RECORD_FILE))
    }
}

/// The managed settings files of Claude Code on this system: the file of
/// the platform, and the files of its `managed-settings.d` directory.
///
/// | Platform | File                                                   |
/// | -------- | ------------------------------------------------------ |
/// | macOS    | `/Library/Application Support/ClaudeCode/managed-settings.json` |
/// | Linux    | `/etc/claude-code/managed-settings.json`               |
/// | Windows  | `C:\Program Files\ClaudeCode\managed-settings.json`    |
#[must_use]
pub fn managed_settings_files() -> Vec<PathBuf> {
    let dir = if cfg!(target_os = "macos") {
        PathBuf::from("/Library/Application Support/ClaudeCode")
    } else if cfg!(windows) {
        PathBuf::from(r"C:\Program Files\ClaudeCode")
    } else {
        PathBuf::from("/etc/claude-code")
    };
    let mut files = vec![dir.join("managed-settings.json")];
    if let Ok(entries) = fs::read_dir(dir.join("managed-settings.d")) {
        let mut more: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "json")
            })
            .collect();
        more.sort();
        files.extend(more);
    }
    files
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
    /// The settings declare another marketplace named `termx`, or the user
    /// turned the plugin off (`"termx@termx": false`): termx installs
    /// nothing and removes nothing, the user decides. See
    /// [`IntegrationStatus::error`].
    Conflict,
    /// Installed, but Claude Code does not run its hooks: its managed
    /// settings allow managed hooks only, or hooks are turned off. See
    /// [`IntegrationStatus::error`].
    Blocked,
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
    /// Another marketplace named `termx`, or the plugin turned off by the
    /// user. termx changes nothing.
    Conflict,
    /// Claude Code does not run the hooks of the plugin, by policy.
    Blocked,
    /// The hook sender is not next to termx (a build without it).
    SenderMissing,
    /// A file cannot be written: read-only, or not the user's.
    PermissionDenied,
    /// The disk is full.
    StorageFull,
    /// A file or folder that is needed is not there (a settings file that
    /// is a link to nothing).
    NotFound,
    /// Reading or writing a file failed otherwise.
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
        let kind = match error.kind() {
            io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem => {
                IntegrationErrorKind::PermissionDenied
            }
            io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded => {
                IntegrationErrorKind::StorageFull
            }
            io::ErrorKind::NotFound => IntegrationErrorKind::NotFound,
            _ => IntegrationErrorKind::Io,
        };
        Self::new(kind, format!("{what} {}: {error}", redact::path(path)))
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
    /// Why the status is [`Conflict`](PluginStatus::Conflict),
    /// [`Blocked`](PluginStatus::Blocked) or [`Error`](PluginStatus::Error).
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

/// What termx found before it installed. See the
/// [module documentation](self#ownership-and-giving-everything-back).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    version: u32,
    /// The plugin directory installed.
    plugin_dir: PathBuf,
    /// The configuration directory of Claude Code installed into.
    #[serde(default)]
    config_dir: Option<PathBuf>,
    /// The text of the settings before, `None` when there was no file.
    /// Only used when [`restorable`](Self::restorable).
    before: Option<String>,
    /// Whether `before` may be written back: the settings have gone from
    /// `before` to what termx wrote with nobody else changing them. Lost
    /// for good the first time termx finds them changed by someone else.
    #[serde(default)]
    restorable: bool,
    /// The hash of the settings as termx last wrote them.
    after: Option<String>,
    /// `extraKnownMarketplaces` was there.
    marketplaces_existed: bool,
    /// `enabledPlugins` was there.
    enabled_plugins_existed: bool,
    /// The marketplace of this directory was declared already.
    marketplace_before: bool,
    /// `termx@termx` was `true` already.
    enabled_before: bool,
}

/// What the settings say about the plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Found {
    marketplace: Present,
    /// `enabledPlugins["termx@termx"]`, when it is a boolean.
    enabled: Option<bool>,
}

/// What to do with the settings file.
enum Rewrite {
    Keep,
    Write(String),
    Remove,
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
            Err(error) => {
                let status = match error.kind {
                    IntegrationErrorKind::Conflict => PluginStatus::Conflict,
                    IntegrationErrorKind::Blocked => PluginStatus::Blocked,
                    _ => PluginStatus::Error,
                };
                let version = (status == PluginStatus::Blocked)
                    .then(|| plugin::installed_version(&self.paths.plugin_dir))
                    .flatten();
                (status, version, Some(error))
            }
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
        let found = self.check_settings(document.as_ref())?;
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
        let edits = found.marketplace != Present::Ours || found.enabled != Some(true);
        if document.is_some() && edits && !self.backup_path().exists() {
            changes.push(Change {
                path: self.shown(&self.backup_path()),
                description: "Copy your Claude Code settings here before changing them.".to_owned(),
            });
        }
        if found.marketplace != Present::Ours {
            changes.push(Change {
                path: self.shown(&settings),
                description: format!(
                    "Add \"termx\" to \"extraKnownMarketplaces\": a local marketplace in {}. Nothing else in the file changes.",
                    self.shown(&self.paths.plugin_dir)
                ),
            });
        }
        if found.enabled != Some(true) {
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

    /// One sentence on what [`install`](Self::install) does to the Claude
    /// Code configuration already there, for the user, matching what it
    /// writes. `None` when it would change nothing.
    ///
    /// # Errors
    ///
    /// Those of [`preview`](Self::preview).
    pub fn note(&self) -> Result<Option<String>, IntegrationError> {
        let document = self.read_settings()?;
        let found = self.check_settings(document.as_ref())?;
        let binary = self.sender_binary()?;
        if let Some(blocked) = self.blocked_by_policy(document.as_ref()) {
            return Ok(Some(format!(
                "{} termx will not change your settings while that is so.",
                blocked.message
            )));
        }
        let marketplace = found.marketplace == Present::Ours;
        let enabled = found.enabled == Some(true);
        let entries = match (marketplace, enabled) {
            (false, false) => {
                "two entries (the \"termx\" marketplace and \"termx@termx\" in enabledPlugins)"
            }
            (false, true) => "one entry (the \"termx\" marketplace in extraKnownMarketplaces)",
            _ => "one entry (\"termx@termx\" in enabledPlugins)",
        };
        let note = if marketplace && enabled {
            if self.plugin_files_current(&binary) {
                return Ok(None);
            }
            "Your Claude Code settings already enable the plugin and are not changed; termx only rewrites its own plugin folder.".to_owned()
        } else if document.is_none() {
            format!(
                "You have no Claude Code settings yet: termx creates settings.json with only {entries}."
            )
        } else {
            let backup = if self.backup_path().exists() {
                ""
            } else {
                ", after copying the file to settings.json.termx-backup"
            };
            format!(
                "Your existing settings and hooks are kept as they are: termx adds {entries} to settings.json{backup}."
            )
        };
        Ok(Some(note))
    }

    /// Installs the plugin, or brings it up to date. Doing it again changes
    /// nothing.
    ///
    /// # Errors
    ///
    /// [`IntegrationErrorKind::InvalidSettings`] and
    /// [`IntegrationErrorKind::Conflict`] before anything is written;
    /// [`IntegrationErrorKind::SenderMissing`]; the kinds of input and
    /// output errors.
    pub fn install(&self) -> Result<(), IntegrationError> {
        let document = self.read_settings()?;
        let found = self.check_settings(document.as_ref())?;
        let binary = self.sender_binary()?;
        if let Some(blocked) = self.blocked_by_policy(document.as_ref()) {
            return Err(blocked);
        }
        let current = self.settings_text()?;
        let existed = |member: &str| {
            document
                .as_ref()
                .is_some_and(|document| document.get(&[member]).is_some())
        };
        // What was there before, recorded before anything changes. A
        // record of an earlier install of this directory is kept, since it
        // knows what was there before that one; but once the settings are
        // not what termx last wrote, the text from before may never be
        // written back (it would undo the edits made since), and objects
        // there now that do not hold termx's entries are the user's.
        let record = match self.read_record() {
            Some(mut record) => {
                let edited = record.after.as_deref()
                    != current
                        .as_deref()
                        .map(|text| plugin::hash(text.as_bytes()))
                        .as_deref();
                if edited {
                    record.restorable = false;
                    record.before = None;
                    record.marketplaces_existed |=
                        existed("extraKnownMarketplaces") && found.marketplace != Present::Ours;
                    record.enabled_plugins_existed |=
                        existed("enabledPlugins") && found.enabled != Some(true);
                }
                record.config_dir = Some(self.paths.config_dir.clone());
                record
            }
            None => Record {
                version: 1,
                plugin_dir: self.paths.plugin_dir.clone(),
                config_dir: Some(self.paths.config_dir.clone()),
                before: current.clone(),
                restorable: true,
                after: None,
                marketplaces_existed: existed("extraKnownMarketplaces"),
                enabled_plugins_existed: existed("enabledPlugins"),
                marketplace_before: found.marketplace == Present::Ours,
                enabled_before: found.enabled == Some(true),
            },
        };
        self.write_record(&record)?;
        self.write_plugin(&binary)?;
        let path = self.paths.plugin_dir.to_string_lossy().into_owned();
        // The text the edit was made on: if it is not the one recorded
        // (changed in between), the record cannot give it back.
        let edited_from = std::cell::RefCell::new(None::<Option<String>>);
        self.rewrite_settings(|text| {
            *edited_from.borrow_mut() = Some(text.map(str::to_owned));
            let mut document = Document::parse(
                text.filter(|text| !text.trim().is_empty())
                    .unwrap_or("{}\n"),
            )
            .map_err(|error| self.settings_error(&error))?;
            let before = document.text();
            document
                .set(
                    &["extraKnownMarketplaces"],
                    plugin::NAME,
                    &json!({ "source": { "source": "directory", "path": path } }),
                )
                .map_err(|error| self.settings_error(&error))?;
            document
                .set(&["enabledPlugins"], plugin::PLUGIN_ID, &Value::Bool(true))
                .map_err(|error| self.settings_error(&error))?;
            let edited = document.text();
            Ok(if text.is_some() && edited == before {
                Rewrite::Keep
            } else {
                Rewrite::Write(edited)
            })
        })?;
        let mut record = record;
        if edited_from.into_inner().as_ref() != Some(&current) {
            record.restorable = false;
            record.before = None;
        }
        let after = self
            .settings_text()?
            .map(|text| plugin::hash(text.as_bytes()));
        self.write_record(&Record { after, ..record })?;
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
        match self.plugin_status() {
            Ok((PluginStatus::Outdated, _)) => {}
            // Not this termx's to change, or nothing to do.
            Ok(_) => return Ok(false),
            Err(error)
                if matches!(
                    error.kind,
                    IntegrationErrorKind::Conflict | IntegrationErrorKind::Blocked
                ) =>
            {
                return Ok(false);
            }
            Err(error) => return Err(error),
        }
        self.install()?;
        Ok(true)
    }

    /// Removes what termx installed, and only that, giving the settings
    /// back as they were: see the
    /// [module documentation](self#ownership-and-giving-everything-back).
    /// Doing it again changes nothing.
    ///
    /// # Errors
    ///
    /// [`IntegrationErrorKind::InvalidSettings`] before anything is
    /// changed, and the kinds of input and output errors.
    pub fn uninstall(&self) -> Result<(), IntegrationError> {
        let record = self.read_record();
        // Installed into another configuration (CLAUDE_CONFIG_DIR changed
        // since): that is the one to give back.
        if let Some(config_dir) = record.as_ref().and_then(|record| record.config_dir.clone())
            && !same_path(&config_dir, &self.paths.config_dir)
        {
            if !config_dir.is_dir() {
                let error = IntegrationError::new(
                    IntegrationErrorKind::NotFound,
                    format!(
                        "termx installed its plugin into the Claude Code configuration in {}, which is gone: there was nothing left to remove there.",
                        self.shown(&config_dir)
                    ),
                );
                self.remove_own_files()?;
                return Err(error);
            }
            let mut paths = self.paths.clone();
            paths.config_dir = config_dir;
            return Integration::new(paths).uninstall();
        }
        let document = self.read_settings()?;
        let found = match &document {
            Some(document) => Found {
                marketplace: self.marketplace_in(document),
                enabled: document
                    .get(&["enabledPlugins", plugin::PLUGIN_ID])
                    .and_then(|value| value.as_bool()),
            },
            None => Found {
                marketplace: Present::Absent,
                enabled: None,
            },
        };
        // Installed by termx: recorded, or pointing at its directory.
        let ours = found.marketplace != Present::Other
            && (record.is_some() || found.marketplace == Present::Ours);
        if ours {
            self.rewrite_settings(|text| {
                let Some(text) = text else {
                    return Ok(Rewrite::Keep);
                };
                // Untouched since termx wrote it: as it was before.
                if let Some(record) = &record
                    && record.restorable
                    && record.after.as_deref() == Some(plugin::hash(text.as_bytes()).as_str())
                {
                    return Ok(match &record.before {
                        Some(before) => Rewrite::Write(before.clone()),
                        None => Rewrite::Remove,
                    });
                }
                let mut document =
                    Document::parse(text).map_err(|error| self.settings_error(&error))?;
                let original = document.text();
                // Without a record, an object that holds nothing but the
                // entry of termx goes with it: termx would have created it.
                let created =
                    |existed: fn(&Record) -> bool| record.as_ref().is_none_or(|r| !existed(r));
                if found.marketplace == Present::Ours
                    && !record
                        .as_ref()
                        .is_some_and(|record| record.marketplace_before)
                {
                    document
                        .remove(
                            &["extraKnownMarketplaces"],
                            plugin::NAME,
                            created(|record| record.marketplaces_existed),
                        )
                        .map_err(|error| self.settings_error(&error))?;
                }
                // A `false` is the user's: left as it is.
                if found.enabled == Some(true)
                    && !record.as_ref().is_some_and(|record| record.enabled_before)
                {
                    document
                        .remove(
                            &["enabledPlugins"],
                            plugin::PLUGIN_ID,
                            created(|record| record.enabled_plugins_existed),
                        )
                        .map_err(|error| self.settings_error(&error))?;
                }
                let edited = document.text();
                Ok(if edited == original {
                    Rewrite::Keep
                } else {
                    Rewrite::Write(edited)
                })
            })?;
            self.forget_in_claude_state();
        }
        self.remove_own_files()?;
        info!("claude code plugin uninstalled");
        Ok(())
    }

    /// Removes the plugin directory and the record.
    fn remove_own_files(&self) -> Result<(), IntegrationError> {
        for (path, directory) in [
            (&self.paths.plugin_dir, true),
            (&self.paths.record(), false),
        ] {
            let removed = if directory {
                fs::remove_dir_all(path)
            } else {
                fs::remove_file(path)
            };
            match removed {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(IntegrationError::io("could not remove", path, &error)),
            }
        }
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
    /// The kinds of input and output errors.
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
        let found = self.check_settings(document.as_ref())?;
        let Some(document) = document else {
            return Ok((PluginStatus::NotInstalled, None));
        };
        if found.marketplace == Present::Absent || found.enabled != Some(true) {
            return Ok((PluginStatus::NotInstalled, None));
        }
        if let Some(blocked) = self.blocked_by_policy(Some(&document)) {
            return Err(blocked);
        }
        let version = plugin::installed_version(&self.paths.plugin_dir);
        let current = self
            .sender_binary()
            .is_ok_and(|binary| self.plugin_files_current(&binary));
        let status = if current {
            PluginStatus::Installed
        } else {
            PluginStatus::Outdated
        };
        Ok((status, version))
    }

    /// Why Claude Code does not run the hooks of the plugin, if it does
    /// not: see the [module documentation](self#hooks-turned-off-by-policy).
    fn blocked_by_policy(&self, user: Option<&Document>) -> Option<IntegrationError> {
        let managed: Vec<Value> = self
            .paths
            .managed_settings
            .iter()
            .filter_map(|file| fs::read(file).ok())
            .filter_map(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .collect();
        let forced = managed
            .iter()
            .any(|settings| settings["enabledPlugins"][plugin::PLUGIN_ID] == Value::Bool(true));
        let blocked = |message: &str| {
            Some(IntegrationError::new(
                IntegrationErrorKind::Blocked,
                message,
            ))
        };
        if managed
            .iter()
            .any(|settings| settings["disableAllHooks"] == Value::Bool(true))
        {
            return blocked(
                "The managed settings of Claude Code on this computer turn every hook off (disableAllHooks): the plugin is installed but cannot report anything.",
            );
        }
        if forced {
            return None;
        }
        if managed
            .iter()
            .any(|settings| settings["allowManagedHooksOnly"] == Value::Bool(true))
        {
            return blocked(
                "The managed settings of Claude Code on this computer allow managed hooks only (allowManagedHooksOnly): the plugin is installed but its hooks do not run.",
            );
        }
        if user.is_some_and(|user| user.get(&["disableAllHooks"]) == Some(Value::Bool(true))) {
            return blocked(
                "Your Claude Code settings turn hooks off (disableAllHooks): the plugin is installed but its hooks do not run.",
            );
        }
        None
    }

    /// What the settings say about the plugin. Refuses settings termx
    /// cannot change safely, and a conflict: another marketplace named
    /// `termx`, or the plugin turned off by the user.
    fn check_settings(&self, document: Option<&Document>) -> Result<Found, IntegrationError> {
        let Some(document) = document else {
            return Ok(Found {
                marketplace: Present::Absent,
                enabled: None,
            });
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
                    "{} already declares another marketplace named \"termx\": termx leaves it alone. Remove it or rename it to install the termx plugin.",
                    self.shown(&self.paths.settings())
                ),
            ));
        }
        let enabled = document.get(&["enabledPlugins", plugin::PLUGIN_ID]);
        match enabled {
            Some(Value::Bool(false)) => Err(IntegrationError::new(
                IntegrationErrorKind::Conflict,
                format!(
                    "The termx plugin is turned off in {} (\"termx@termx\": false). termx leaves that choice to you: turn it on with /plugin in Claude Code, or remove that line, then try again.",
                    self.shown(&self.paths.settings())
                ),
            )),
            Some(Value::Bool(true)) => Ok(Found {
                marketplace,
                enabled: Some(true),
            }),
            None => Ok(Found {
                marketplace,
                enabled: None,
            }),
            Some(_) => Err(IntegrationError::new(
                IntegrationErrorKind::InvalidSettings,
                format!(
                    "\"termx@termx\" in {} is not true or false: termx does not change it",
                    self.shown(&self.paths.settings())
                ),
            )),
        }
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
        let Some(text) = self.settings_text()? else {
            return Ok(None);
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

    /// The text of the settings, or `None` when there is no file.
    fn settings_text(&self) -> Result<Option<String>, IntegrationError> {
        let path = self.paths.settings();
        match fs::read_to_string(&path) {
            Ok(text) => Ok(Some(text)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if fs::symlink_metadata(&path).is_ok() {
                    return Err(self.dangling_link());
                }
                Ok(None)
            }
            Err(error) if error.kind() == io::ErrorKind::InvalidData => Err(IntegrationError::new(
                IntegrationErrorKind::InvalidSettings,
                format!("{} is not text", self.shown(&path)),
            )),
            Err(error) => Err(IntegrationError::io("could not read", &path, &error)),
        }
    }

    fn dangling_link(&self) -> IntegrationError {
        IntegrationError::new(
            IntegrationErrorKind::NotFound,
            format!(
                "{} is a link to a file that does not exist: termx does not replace it. Restore the file it points to, or remove the link.",
                self.shown(&self.paths.settings())
            ),
        )
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

    /// Rewrites the settings with what `produce` makes of their text
    /// (`None` when there is no file). Backs them up the first time,
    /// refuses a read-only file and a link to nothing, writes atomically
    /// through a link, and starts over once if the file changed just
    /// before the rename.
    fn rewrite_settings(
        &self,
        produce: impl Fn(Option<&str>) -> Result<Rewrite, IntegrationError>,
    ) -> Result<(), IntegrationError> {
        let settings = self.paths.settings();
        let is_link =
            fs::symlink_metadata(&settings).is_ok_and(|meta| meta.file_type().is_symlink());
        // A link (to a repository of dotfiles) stays a link: its target
        // is what changes.
        let target = if is_link {
            fs::canonicalize(&settings).map_err(|_| self.dangling_link())?
        } else {
            settings.clone()
        };
        for _ in 0..2 {
            let before = match fs::read(&target) {
                Ok(bytes) => Some(bytes),
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(error) => {
                    return Err(IntegrationError::io("could not read", &settings, &error));
                }
            };
            let modified = fs::metadata(&target).and_then(|meta| meta.modified()).ok();
            let text = before
                .as_deref()
                .map(|bytes| String::from_utf8_lossy(bytes).into_owned());
            let wanted = produce(text.as_deref())?;
            if matches!(wanted, Rewrite::Keep) {
                return Ok(());
            }
            if fs::metadata(&target).is_ok_and(|meta| meta.permissions().readonly()) {
                return Err(IntegrationError::new(
                    IntegrationErrorKind::PermissionDenied,
                    format!(
                        "{} is read-only: termx does not change it. Make it writable to install or remove the plugin.",
                        self.shown(&settings)
                    ),
                ));
            }
            if let Some(before) = &before {
                self.back_up(before)?;
            }
            if let Some(dir) = target.parent() {
                fs::create_dir_all(dir)
                    .map_err(|error| IntegrationError::io("could not create", dir, &error))?;
            }
            let unchanged = || {
                fs::read(&target).ok() == before
                    && fs::metadata(&target).and_then(|meta| meta.modified()).ok() == modified
            };
            let done = match &wanted {
                Rewrite::Write(text) => replace_if(&target, text.as_bytes(), unchanged)
                    .map_err(|error| IntegrationError::io("could not write", &settings, &error))?,
                Rewrite::Remove => {
                    if unchanged() {
                        match fs::remove_file(&target) {
                            Ok(()) => true,
                            Err(error) if error.kind() == io::ErrorKind::NotFound => true,
                            Err(error) => {
                                return Err(IntegrationError::io(
                                    "could not remove",
                                    &settings,
                                    &error,
                                ));
                            }
                        }
                    } else {
                        false
                    }
                }
                Rewrite::Keep => true,
            };
            if done {
                return Ok(());
            }
        }
        Err(IntegrationError::new(
            IntegrationErrorKind::Io,
            format!(
                "{} kept changing while termx edited it; try again",
                self.shown(&settings)
            ),
        ))
    }

    /// Copies the settings before termx first changes them. An existing
    /// copy is kept: it is the original.
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

    /// The record of an install of this plugin directory, if there is one.
    fn read_record(&self) -> Option<Record> {
        let bytes = fs::read(self.paths.record()).ok()?;
        let record: Record = serde_json::from_slice(&bytes).ok()?;
        (record.version == 1 && same_path(&record.plugin_dir, &self.paths.plugin_dir))
            .then_some(record)
    }

    /// Writes the record, readable by the user only: it holds the text of
    /// the settings.
    fn write_record(&self, record: &Record) -> Result<(), IntegrationError> {
        let path = self.paths.record();
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)
                .map_err(|error| IntegrationError::io("could not create", dir, &error))?;
        }
        let bytes = serde_json::to_vec_pretty(record).unwrap_or_default();
        write_atomic(&path, &bytes, None)
            .and_then(|()| make_private(&path))
            .map_err(|error| IntegrationError::io("could not write", &path, &error))
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
    /// (`plugins/` of its configuration directory), when Claude Code
    /// recorded it for this directory: the marketplace in
    /// `known_marketplaces.json`, and then the plugin in
    /// `installed_plugins.json` and its copy in `cache/termx`. Best effort:
    /// these files belong to Claude Code, and only the entries of termx are
    /// touched.
    fn forget_in_claude_state(&self) {
        let plugins = self.paths.config_dir.join("plugins");
        let known = plugins.join("known_marketplaces.json");
        let ours = self.edit_claude_state(&known, |document| {
            let ours = document
                .get(&[plugin::NAME, "source"])
                .is_some_and(|source| {
                    source["source"] == "directory"
                        && source["path"]
                            .as_str()
                            .is_some_and(|path| same_path(Path::new(path), &self.paths.plugin_dir))
                });
            ours && document.remove(&[], plugin::NAME, false).unwrap_or(false)
        });
        // The cache and the installs of the marketplace "termx" are this
        // plugin's only when the marketplace Claude Code knew was this one.
        if !ours {
            return;
        }
        self.edit_claude_state(&plugins.join("installed_plugins.json"), |document| {
            document
                .remove(&["plugins"], plugin::PLUGIN_ID, false)
                .unwrap_or(false)
        });
        let cache = plugins.join("cache").join(plugin::NAME);
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

/// Writes `bytes` over `path` like [`write_atomic`], but only if
/// `unchanged` still says so once the temporary file is ready, right
/// before the rename. Returns whether it wrote.
fn replace_if(path: &Path, bytes: &[u8], unchanged: impl Fn() -> bool) -> io::Result<bool> {
    let temporary = temporary_next_to(path);
    let result = (|| {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        if let Ok(meta) = fs::metadata(path) {
            fs::set_permissions(&temporary, meta.permissions())?;
        }
        if !unchanged() {
            return Ok(false);
        }
        fs::rename(&temporary, path)?;
        Ok(true)
    })();
    if !matches!(result, Ok(true)) {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// A name for a temporary file next to `path`.
fn temporary_next_to(path: &Path) -> PathBuf {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    dir.join(format!(".{name}.termx-{}", crate::workspace::random_hex()))
}

/// Writes `bytes` to `path` through a temporary file in the same directory,
/// renamed over it, so that a reader never sees half a file. The file gets
/// the permissions of `like`, when it exists.
fn write_atomic(path: &Path, bytes: &[u8], like: Option<&Path>) -> io::Result<()> {
    let temporary = temporary_next_to(path);
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

/// Makes `path` readable and writable by the user only (macOS, Linux).
#[cfg(unix)]
fn make_private(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn make_private(_path: &Path) -> io::Result<()> {
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_changed_just_before_the_rename_is_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, "{\"by\": \"claude\"}").unwrap();

        let wrote = replace_if(&path, b"{\"by\": \"termx\"}", || false).unwrap();

        assert!(!wrote);
        assert_eq!(fs::read_to_string(&path).unwrap(), "{\"by\": \"claude\"}");
        // No temporary file is left behind.
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
        assert!(replace_if(&path, b"{}", || true).unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), "{}");
    }
}
