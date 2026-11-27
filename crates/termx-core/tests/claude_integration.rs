//! Installing, upgrading and removing the termx plugin of Claude Code, in a
//! configuration directory made for each test (never the user's own), next
//! to hooks the user already has.

#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use termx_core::claude::{
    BACKUP_SUFFIX, Consent, Integration, IntegrationErrorKind, Paths, PluginStatus, plugin,
};

/// A user's settings with a hook on every event (a desktop mascot), in the
/// user's own formatting.
const MASCOT_SETTINGS: &str = r#"{
  "model": "opus",
  "permissions": {
    "allow": ["Bash(git status)"]
  },
  "hooks": {
    "SessionStart": [{ "hooks": [{ "type": "command", "command": "mascot wake" }] }],
    "UserPromptSubmit": [{ "hooks": [{ "type": "command", "command": "mascot listen" }] }],
    "PreToolUse": [{ "matcher": "*", "hooks": [{ "type": "command", "command": "mascot work" }] }],
    "PostToolUse": [{ "matcher": "*", "hooks": [{ "type": "command", "command": "mascot work" }] }],
    "Notification": [{ "hooks": [{ "type": "command", "command": "mascot wave" }] }],
    "Stop": [{ "hooks": [{ "type": "command", "command": "mascot sleep" }] }],
    "SessionEnd": [{ "hooks": [{ "type": "command", "command": "mascot bye" }] }]
  },
  "theme": "dark"
}
"#;

struct Setup {
    _home: tempfile::TempDir,
    home: PathBuf,
    integration: Integration,
}

impl Setup {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().to_owned();
        let sender = root.join("app").join(termx_core::claude::SENDER);
        fs::create_dir_all(sender.parent().unwrap()).unwrap();
        fs::write(&sender, b"the hook sender of this build").unwrap();
        let integration = Integration::new(Paths {
            config_dir: root.join("claude-config"),
            plugin_dir: root.join("termx-state").join("claude-plugin"),
            consent_file: Some(root.join("termx-state").join("claude-integration.json")),
            sender,
            home: Some(root.clone()),
            search_path: None,
        });
        Self {
            _home: home,
            home: root,
            integration,
        }
    }

    fn with_settings(text: &str) -> Self {
        let setup = Self::new();
        fs::create_dir_all(&setup.integration.paths().config_dir).unwrap();
        fs::write(setup.settings(), text).unwrap();
        setup
    }

    fn settings(&self) -> PathBuf {
        self.integration.paths().settings()
    }

    fn settings_text(&self) -> String {
        fs::read_to_string(self.settings()).unwrap()
    }

    fn settings_value(&self) -> Value {
        serde_json::from_str(&self.settings_text()).unwrap()
    }

    fn plugin_dir(&self) -> &Path {
        &self.integration.paths().plugin_dir
    }

    fn backup(&self) -> PathBuf {
        let mut name = self.settings().into_os_string();
        name.push(BACKUP_SUFFIX);
        PathBuf::from(name)
    }
}

#[test]
fn installing_adds_two_members_and_leaves_the_users_hooks_alone() {
    let setup = Setup::with_settings(MASCOT_SETTINGS);
    let before: Value = serde_json::from_str(MASCOT_SETTINGS).unwrap();

    setup.integration.install().unwrap();

    let after = setup.settings_value();
    let mut expected = before.clone();
    expected["extraKnownMarketplaces"] = json!({
        "termx": { "source": { "source": "directory", "path": setup.plugin_dir().to_string_lossy() } }
    });
    expected["enabledPlugins"] = json!({ "termx@termx": true });
    assert_eq!(after, expected);
    // The user's text is untouched up to where the new members start.
    let text = setup.settings_text();
    let kept = &MASCOT_SETTINGS[..MASCOT_SETTINGS.rfind("\n}").unwrap()];
    assert!(text.starts_with(kept), "{text}");
    // Backed up as it was.
    assert_eq!(fs::read_to_string(setup.backup()).unwrap(), MASCOT_SETTINGS);
    // The plugin is there, and names the copy of the hook sender.
    let hooks: Value = serde_json::from_slice(
        &fs::read(
            setup
                .plugin_dir()
                .join("termx")
                .join("hooks")
                .join("hooks.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let command = hooks["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        Path::new(&command).starts_with(setup.plugin_dir()),
        "{command}"
    );
    assert_eq!(
        fs::read(&command).unwrap(),
        b"the hook sender of this build"
    );
    let status = setup.integration.status();
    assert_eq!(status.plugin, PluginStatus::Installed);
    assert_eq!(status.version.as_deref(), Some(plugin::version().as_str()));
}

#[test]
fn installing_twice_changes_nothing_more() {
    let setup = Setup::with_settings(MASCOT_SETTINGS);
    setup.integration.install().unwrap();
    let first = setup.settings_text();
    let backup = fs::read(setup.backup()).unwrap();

    setup.integration.install().unwrap();

    assert_eq!(setup.settings_text(), first);
    assert_eq!(fs::read(setup.backup()).unwrap(), backup);
    assert!(setup.integration.preview().unwrap().is_empty());
    assert_eq!(setup.integration.note().unwrap(), None);
}

#[test]
fn uninstalling_gives_back_the_settings_byte_for_byte() {
    let setup = Setup::with_settings(MASCOT_SETTINGS);
    setup.integration.install().unwrap();

    setup.integration.uninstall().unwrap();

    assert_eq!(setup.settings_text(), MASCOT_SETTINGS);
    assert!(!setup.plugin_dir().exists());
    assert_eq!(
        setup.integration.status().plugin,
        PluginStatus::NotInstalled
    );
    // Again: nothing to do.
    setup.integration.uninstall().unwrap();
    assert_eq!(setup.settings_text(), MASCOT_SETTINGS);
}

#[test]
fn a_user_without_settings_gets_a_file_with_only_the_plugin() {
    let setup = Setup::new();
    assert_eq!(
        setup.integration.status().plugin,
        PluginStatus::NotInstalled
    );

    setup.integration.install().unwrap();

    assert_eq!(
        setup.settings_value(),
        json!({
            "extraKnownMarketplaces": {
                "termx": { "source": { "source": "directory", "path": setup.plugin_dir().to_string_lossy() } }
            },
            "enabledPlugins": { "termx@termx": true }
        })
    );
    // Nothing to back up.
    assert!(!setup.backup().exists());
    setup.integration.uninstall().unwrap();
    assert_eq!(setup.settings_value(), json!({}));
}

#[test]
fn settings_that_are_not_valid_json_are_refused_and_left_alone() {
    for broken in [
        "{ \"model\": \"opus\", }",
        "// comment\n{}",
        "[1, 2]",
        "{\"enabledPlugins\": [\"x\"]}",
    ] {
        let setup = Setup::with_settings(broken);

        let error = setup.integration.install().unwrap_err();

        assert_eq!(
            error.kind,
            IntegrationErrorKind::InvalidSettings,
            "{broken}"
        );
        assert!(error.message.contains("settings.json"), "{}", error.message);
        assert_eq!(setup.settings_text(), broken);
        assert!(!setup.backup().exists());
        assert!(!setup.plugin_dir().exists());
        assert!(setup.integration.preview().is_err());
        // Uninstalling never "repairs" the file either.
        let _ = setup.integration.uninstall();
        assert_eq!(setup.settings_text(), broken);
    }
}

#[test]
fn settings_that_cannot_be_read_make_the_status_an_error() {
    let setup = Setup::with_settings("{ not json");

    let status = setup.integration.status();

    assert_eq!(status.plugin, PluginStatus::Error);
    assert_eq!(
        status.error.unwrap().kind,
        IntegrationErrorKind::InvalidSettings
    );
    assert!(setup.integration.uninstall().is_err());
    assert_eq!(setup.settings_text(), "{ not json");
}

#[test]
fn another_marketplace_named_termx_is_not_taken_over() {
    let theirs = r#"{"extraKnownMarketplaces": {"termx": {"source": {"source": "github", "repo": "someone/termx"}}}}"#;
    let setup = Setup::with_settings(theirs);

    let error = setup.integration.install().unwrap_err();

    assert_eq!(error.kind, IntegrationErrorKind::Conflict);
    assert_eq!(setup.settings_text(), theirs);
    // Uninstalling leaves it alone too.
    setup.integration.uninstall().unwrap();
    assert_eq!(setup.settings_text(), theirs);
}

#[test]
fn a_plugin_of_another_version_is_outdated_and_upgraded_without_touching_the_settings() {
    let setup = Setup::with_settings(MASCOT_SETTINGS);
    setup.integration.install().unwrap();
    let settings = setup.settings_text();
    // What an older termx wrote: another version, another hook sender.
    let manifest = setup
        .plugin_dir()
        .join("termx")
        .join(".claude-plugin")
        .join("plugin.json");
    let older = fs::read_to_string(&manifest)
        .unwrap()
        .replace(&plugin::version(), "0.0.1+plugin.0");
    fs::write(&manifest, older).unwrap();
    fs::write(&setup.integration.paths().sender, b"a newer hook sender").unwrap();

    let status = setup.integration.status();
    assert_eq!(status.plugin, PluginStatus::Outdated);
    assert_eq!(status.version.as_deref(), Some("0.0.1+plugin.0"));

    assert!(setup.integration.upgrade().unwrap());

    assert_eq!(setup.integration.status().plugin, PluginStatus::Installed);
    assert_eq!(setup.settings_text(), settings);
    // The hooks run the new sender; the old one stays for the copies of
    // the old hooks Claude Code may have cached.
    let hooks: Value = serde_json::from_slice(
        &fs::read(
            setup
                .plugin_dir()
                .join("termx")
                .join("hooks")
                .join("hooks.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let command = hooks["hooks"]["Stop"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(fs::read(command).unwrap(), b"a newer hook sender");
    let mut copies: Vec<_> = fs::read_dir(setup.plugin_dir().join("termx").join("bin"))
        .unwrap()
        .flatten()
        .map(|entry| fs::read(entry.path()).unwrap())
        .collect();
    copies.sort();
    assert_eq!(
        copies,
        vec![
            b"a newer hook sender".to_vec(),
            b"the hook sender of this build".to_vec()
        ]
    );
    // Up to date: nothing to do.
    assert!(!setup.integration.upgrade().unwrap());
}

#[test]
fn upgrading_never_installs_what_the_user_did_not_install() {
    let setup = Setup::with_settings(MASCOT_SETTINGS);

    assert!(!setup.integration.upgrade().unwrap());

    assert_eq!(setup.settings_text(), MASCOT_SETTINGS);
    assert!(!setup.plugin_dir().exists());
}

#[test]
fn the_preview_says_exactly_what_will_change_with_home_as_a_tilde() {
    let setup = Setup::with_settings(MASCOT_SETTINGS);

    let changes = setup.integration.preview().unwrap();

    let paths: Vec<&str> = changes.iter().map(|change| change.path.as_str()).collect();
    let sep = std::path::MAIN_SEPARATOR;
    assert_eq!(
        paths,
        [
            format!("~{sep}termx-state{sep}claude-plugin"),
            format!("~{sep}claude-config{sep}settings.json{BACKUP_SUFFIX}"),
            format!("~{sep}claude-config{sep}settings.json"),
            format!("~{sep}claude-config{sep}settings.json"),
        ]
    );
    let text = serde_json::to_string(&changes).unwrap();
    assert!(text.contains("extraKnownMarketplaces"));
    assert!(text.contains("termx@termx"));
    assert!(text.contains("PreToolUse"));
    assert!(!text.contains(&*setup.home.to_string_lossy()), "{text}");
    assert_eq!(
        setup.integration.note().unwrap().as_deref(),
        Some(
            "Your existing settings and hooks are kept as they are: termx adds two entries (the \"termx\" marketplace and \"termx@termx\" in enabledPlugins) to settings.json, after copying the file to settings.json.termx-backup."
        )
    );
    // Previewing changes nothing.
    assert_eq!(setup.settings_text(), MASCOT_SETTINGS);
    assert!(!setup.plugin_dir().exists());
}

#[test]
fn the_answer_of_the_user_is_remembered() {
    let setup = Setup::new();
    assert_eq!(setup.integration.consent(), Consent::Unasked);

    setup.integration.set_consent(Consent::Declined).unwrap();
    assert_eq!(setup.integration.consent(), Consent::Declined);
    assert_eq!(setup.integration.status().consent, Consent::Declined);

    setup.integration.set_consent(Consent::Accepted).unwrap();
    let again = Integration::new(setup.integration.paths().clone());
    assert_eq!(again.consent(), Consent::Accepted);
}

#[test]
fn without_a_hook_sender_nothing_is_installed() {
    let setup = Setup::with_settings(MASCOT_SETTINGS);
    fs::remove_file(&setup.integration.paths().sender).unwrap();

    let error = setup.integration.install().unwrap_err();

    assert_eq!(error.kind, IntegrationErrorKind::SenderMissing);
    assert_eq!(setup.settings_text(), MASCOT_SETTINGS);
}

#[test]
fn what_claude_code_recorded_of_the_plugin_is_removed_with_it() {
    let setup = Setup::with_settings(MASCOT_SETTINGS);
    setup.integration.install().unwrap();
    // What `claude plugin marketplace add` and `claude plugin install`
    // record (Claude Code 2.1.321), next to entries of other plugins.
    let state = setup.integration.paths().config_dir.join("plugins");
    let cached = state
        .join("cache")
        .join("termx")
        .join("termx")
        .join("0.0.0-plugin.1");
    fs::create_dir_all(cached.join(".claude-plugin")).unwrap();
    fs::copy(
        setup
            .plugin_dir()
            .join("termx")
            .join(".claude-plugin")
            .join("plugin.json"),
        cached.join(".claude-plugin").join("plugin.json"),
    )
    .unwrap();
    let other_cache = state.join("cache").join("claude-plugins-official");
    fs::create_dir_all(&other_cache).unwrap();
    let official =
        json!({"source": {"source": "github", "repo": "anthropics/claude-plugins-official"}});
    let known = json!({
        "claude-plugins-official": official,
        "termx": {"source": {"source": "directory", "path": setup.plugin_dir().to_string_lossy()}, "installLocation": "x"}
    });
    fs::write(
        state.join("known_marketplaces.json"),
        serde_json::to_string_pretty(&known).unwrap(),
    )
    .unwrap();
    let installed = json!({"version": 2, "plugins": {
        "termx@termx": [{"scope": "user", "installPath": cached.to_string_lossy(), "version": "0.0.0+plugin.1"}],
        "other@claude-plugins-official": [{"scope": "user"}]
    }});
    fs::write(
        state.join("installed_plugins.json"),
        serde_json::to_string_pretty(&installed).unwrap(),
    )
    .unwrap();

    setup.integration.uninstall().unwrap();

    let read = |name: &str| -> Value {
        serde_json::from_str(&fs::read_to_string(state.join(name)).unwrap()).unwrap()
    };
    assert_eq!(
        read("known_marketplaces.json"),
        json!({"claude-plugins-official": official})
    );
    assert_eq!(
        read("installed_plugins.json"),
        json!({"version": 2, "plugins": {"other@claude-plugins-official": [{"scope": "user"}]}})
    );
    assert!(!state.join("cache").join("termx").exists());
    assert!(other_cache.exists());
    assert_eq!(setup.settings_text(), MASCOT_SETTINGS);
}

#[cfg(unix)]
#[test]
fn settings_kept_elsewhere_through_a_link_stay_a_link() {
    let setup = Setup::new();
    let dotfiles = setup.home.join("dotfiles");
    fs::create_dir_all(&dotfiles).unwrap();
    fs::write(dotfiles.join("claude-settings.json"), MASCOT_SETTINGS).unwrap();
    fs::create_dir_all(&setup.integration.paths().config_dir).unwrap();
    std::os::unix::fs::symlink(dotfiles.join("claude-settings.json"), setup.settings()).unwrap();

    setup.integration.install().unwrap();

    assert!(
        fs::symlink_metadata(setup.settings())
            .unwrap()
            .file_type()
            .is_symlink()
    );
    let target: Value =
        serde_json::from_str(&fs::read_to_string(dotfiles.join("claude-settings.json")).unwrap())
            .unwrap();
    assert_eq!(target["enabledPlugins"]["termx@termx"], true);
    setup.integration.uninstall().unwrap();
    assert_eq!(
        fs::read_to_string(dotfiles.join("claude-settings.json")).unwrap(),
        MASCOT_SETTINGS
    );
}

#[test]
fn claude_code_is_found_on_the_path_or_by_its_configuration() {
    let setup = Setup::new();
    assert!(!setup.integration.status().claude_installed);

    let bin = setup.home.join("bin");
    fs::create_dir_all(&bin).unwrap();
    fs::write(
        bin.join(if cfg!(windows) {
            "claude.exe"
        } else {
            "claude"
        }),
        b"",
    )
    .unwrap();
    let mut paths = setup.integration.paths().clone();
    paths.search_path = Some(bin.into_os_string());
    assert!(Integration::new(paths).status().claude_installed);

    let configured = Setup::with_settings("{}");
    assert!(configured.integration.status().claude_installed);
}

#[test]
fn the_configuration_directory_follows_claude_config_dir() {
    let home = Path::new("/home/someone");
    assert_eq!(
        termx_core::claude::config_dir_from(None, Some(home)),
        home.join(".claude")
    );
    assert_eq!(
        termx_core::claude::config_dir_from(Some("/elsewhere".into()), Some(home)),
        PathBuf::from("/elsewhere")
    );
    assert_eq!(
        termx_core::claude::config_dir_from(Some("".into()), Some(home)),
        home.join(".claude")
    );
}
