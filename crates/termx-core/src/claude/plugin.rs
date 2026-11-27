//! The files of the termx plugin, as termx writes them.
//!
//! The plugin lives in a local marketplace, a directory of termx:
//!
//! ```text
//! <plugin dir>/
//!   .claude-plugin/marketplace.json     the marketplace "termx", one plugin
//!   termx/
//!     .claude-plugin/plugin.json        the plugin "termx", with the version of termx
//!     hooks/hooks.json                  the hooks, which run bin/termx-cli-<hash> hook
//!     bin/termx-cli-<hash>[.exe]        a copy of the hook sender
//!     README.md
//! ```
//!
//! Claude Code loads a plugin of a local directory marketplace in place,
//! without copying it, so this directory is what runs. The hooks name the
//! copy of the hook sender by its absolute path, and run it without a shell
//! (the exec form of a hook command: `command` and `args`), so that a path
//! with spaces needs no quoting and no shell starts on any platform.
//!
//! The copy is named after a hash of its content. A hook that runs while
//! termx updates the plugin keeps its file (on Windows a running program
//! cannot be replaced), and the new hooks name the new file; the old one
//! is removed once it can be.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// The name of the marketplace, and of the plugin in it.
pub const NAME: &str = "termx";

/// How the plugin is named in `enabledPlugins`.
pub const PLUGIN_ID: &str = "termx@termx";

/// The manifest of the plugin, with `@VERSION@` for the version.
const PLUGIN_JSON: &str = include_str!("../../../../plugin/.claude-plugin/plugin.json");

/// The hooks, with `@TERMX_CLI@` for the hook sender.
const HOOKS_JSON: &str = include_str!("../../../../plugin/hooks/hooks.json");

/// What the plugin says about itself.
const README: &str = include_str!("../../../../plugin/README.md");

/// The version written into the plugin: that of termx, and of the format of
/// its files, so that a change to the plugin alone also counts as a new
/// version.
#[must_use]
pub fn version() -> String {
    format!("{}+plugin.{}", crate::version(), FORMAT)
}

/// Raised whenever what termx writes in the plugin changes.
const FORMAT: u32 = 1;

/// One file of the plugin and what it holds.
pub struct PluginFile {
    /// Relative to the plugin directory.
    pub path: PathBuf,
    /// What it holds.
    pub content: Vec<u8>,
    /// Whether it is a program.
    pub executable: bool,
}

/// The name of the copy of the hook sender whose content is `binary`.
#[must_use]
pub fn sender_name(binary: &[u8]) -> String {
    let suffix = if cfg!(windows) { ".exe" } else { "" };
    format!("termx-cli-{:016x}{suffix}", fnv1a(binary))
}

/// The files of the plugin in `dir`, with `binary` as the hook sender.
#[must_use]
pub fn files(dir: &Path, binary: &[u8]) -> Vec<PluginFile> {
    let sender = Path::new(NAME).join("bin").join(sender_name(binary));
    let sender_path = dir.join(&sender);
    let hooks = HOOKS_JSON.replace(
        "\"@TERMX_CLI@\"",
        &serde_json::to_string(&sender_path.to_string_lossy()).unwrap_or_default(),
    );
    let manifest = PLUGIN_JSON.replace("@VERSION@", &version());
    let marketplace = serde_json::json!({
        "name": NAME,
        "owner": { "name": "termx" },
        "metadata": {
            "description": "The Claude Code plugin of termx, written by termx on this machine."
        },
        "plugins": [{
            "name": NAME,
            "source": format!("./{NAME}"),
            "description": "Tells termx what each Claude Code session it hosts is doing.",
            "version": version(),
            // On only where the settings say so: removing that member
            // turns it off, even if Claude Code kept a copy.
            "defaultEnabled": false,
        }],
    });
    vec![
        PluginFile {
            path: Path::new(".claude-plugin").join("marketplace.json"),
            content: pretty(&marketplace).into_bytes(),
            executable: false,
        },
        PluginFile {
            path: Path::new(NAME).join(".claude-plugin").join("plugin.json"),
            content: manifest.into_bytes(),
            executable: false,
        },
        PluginFile {
            path: Path::new(NAME).join("hooks").join("hooks.json"),
            content: hooks.into_bytes(),
            executable: false,
        },
        PluginFile {
            path: Path::new(NAME).join("README.md"),
            content: README.as_bytes().to_vec(),
            executable: false,
        },
        PluginFile {
            path: sender,
            content: binary.to_vec(),
            executable: true,
        },
    ]
}

/// The events the hooks of the plugin are registered for.
#[must_use]
pub fn hooked_events() -> Vec<String> {
    serde_json::from_str::<Value>(HOOKS_JSON)
        .ok()
        .and_then(|hooks| {
            hooks["hooks"]
                .as_object()
                .map(|events| events.keys().cloned().collect())
        })
        .unwrap_or_default()
}

/// The version of the plugin installed in `dir`, if it has one.
#[must_use]
pub fn installed_version(dir: &Path) -> Option<String> {
    let manifest = std::fs::read(dir.join(NAME).join(".claude-plugin").join("plugin.json")).ok()?;
    let manifest: Value = serde_json::from_slice(&manifest).ok()?;
    manifest["version"].as_str().map(str::to_owned)
}

fn pretty(value: &Value) -> String {
    let mut text = serde_json::to_string_pretty(value).unwrap_or_default();
    text.push('\n');
    text
}

/// [`fnv1a`] of `bytes`, as 16 hexadecimal digits.
pub(crate) fn hash(bytes: &[u8]) -> String {
    format!("{:016x}", fnv1a(bytes))
}

/// The 64-bit FNV-1a hash: enough to tell two builds of the hook sender
/// apart, and free of dependencies.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hooks_run_the_copy_of_the_sender_without_a_shell() {
        let dir = Path::new(if cfg!(windows) {
            r"C:\Users\A B\termx"
        } else {
            "/home/a b/termx"
        });
        let files = files(dir, b"binary");
        let hooks = files
            .iter()
            .find(|file| file.path.ends_with(Path::new("hooks").join("hooks.json")))
            .unwrap();
        let hooks: Value = serde_json::from_slice(&hooks.content).unwrap();
        let sender = dir.join(NAME).join("bin").join(sender_name(b"binary"));

        let events = hooks["hooks"].as_object().unwrap();
        assert!(events.len() >= 10);
        for (event, groups) in events {
            for group in groups.as_array().unwrap() {
                for hook in group["hooks"].as_array().unwrap() {
                    assert_eq!(hook["type"], "command", "{event}");
                    assert_eq!(
                        hook["command"],
                        sender.to_string_lossy().as_ref(),
                        "{event}"
                    );
                    assert_eq!(hook["args"], serde_json::json!(["hook"]), "{event}");
                    // Never blocks Claude Code, except to be sure the end
                    // of the session is delivered before it exits.
                    assert_eq!(
                        hook["async"].as_bool().unwrap_or(false),
                        event != "SessionEnd",
                        "{event}"
                    );
                }
            }
        }
        for event in [
            "SessionStart",
            "SessionEnd",
            "UserPromptSubmit",
            "PreToolUse",
            "PostToolUse",
            "PermissionRequest",
            "Notification",
            "Stop",
            "StopFailure",
            "SubagentStop",
        ] {
            assert!(events.contains_key(event), "{event} is not hooked");
        }
    }

    #[test]
    fn the_manifests_name_the_plugin_and_its_version() {
        let files = files(Path::new("/x"), b"binary");
        let json = |end: &str| -> Value {
            let file = files.iter().find(|file| file.path.ends_with(end)).unwrap();
            serde_json::from_slice(&file.content).unwrap()
        };
        let marketplace = json("marketplace.json");
        assert_eq!(marketplace["name"], NAME);
        assert_eq!(marketplace["plugins"][0]["name"], NAME);
        assert_eq!(marketplace["plugins"][0]["source"], "./termx");
        let manifest = json("plugin.json");
        assert_eq!(manifest["name"], NAME);
        assert_eq!(manifest["version"], version());
        assert_eq!(manifest["defaultEnabled"], false);
        assert_eq!(marketplace["plugins"][0]["defaultEnabled"], false);
        assert!(files.iter().any(|file| file.executable));
    }

    #[test]
    fn senders_with_different_content_have_different_names() {
        assert_ne!(sender_name(b"one"), sender_name(b"two"));
        assert_eq!(sender_name(b"one"), sender_name(b"one"));
    }
}
