//! Commands about the Claude Code integration: the plugin that reports
//! what each Claude Code session does, the consent of the user to install
//! it (with the event `claude-integration-changed` when any of that
//! changes), and the notifications and badge that call the user back.
//!
//! See [`termx_core::claude`] for what installing changes, and
//! [`crate::notify`] for notifications.

use serde::Deserialize;
use tauri::{AppHandle, Runtime, State};
use termx_core::claude::{
    Change, Consent, IntegrationError, IntegrationErrorKind, IntegrationStatus,
};

use super::error::{CommandResult, ErrorKind, Failure, logged};
use crate::claude::ClaudeState;

impl From<IntegrationError> for Failure {
    fn from(error: IntegrationError) -> Self {
        let kind = match error.kind {
            IntegrationErrorKind::InvalidSettings => ErrorKind::InvalidSettings,
            IntegrationErrorKind::Conflict => ErrorKind::SettingsConflict,
            IntegrationErrorKind::SenderMissing => ErrorKind::SenderMissing,
            _ => ErrorKind::Internal,
        };
        Self::new(kind, error.message)
    }
}

/// What installing would change, for the user to read before saying yes.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudePreview {
    /// One entry per change, the paths with the home directory as `~`.
    /// Empty when the plugin is installed as this termx would install it.
    pub changes: Vec<Change>,
    /// One sentence on what installing does to the configuration already
    /// there, shown as it is. Absent when nothing would change.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// What the user answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConsentAnswer {
    /// Install the plugin.
    Accepted,
    /// Do not.
    Declined,
}

/// Whether Claude Code is installed, whether the plugin is, and what the
/// user answered. Reads a few files: run off the main thread.
#[tauri::command(async)]
#[must_use]
pub fn get_claude_integration<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, ClaudeState>,
) -> IntegrationStatus {
    state.check(&app)
}

/// What installing would change.
///
/// # Errors
///
/// `invalidSettings` when the settings of Claude Code are not valid JSON,
/// `settingsConflict` when they declare another marketplace named termx,
/// `senderMissing` in a build without the hook sender.
#[tauri::command(async)]
pub fn preview_claude_integration(state: State<'_, ClaudeState>) -> CommandResult<ClaudePreview> {
    logged("preview_claude_integration", || {
        Ok::<_, IntegrationError>(ClaudePreview {
            changes: state.integration().preview()?,
            note: state.integration().note()?,
        })
    })
}

/// Installs the plugin, or brings it up to date. Records the consent of the
/// user as well: installing is saying yes.
///
/// # Errors
///
/// Those of [`preview_claude_integration`]; nothing is changed then.
#[tauri::command(async)]
pub fn install_claude_integration<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, ClaudeState>,
) -> CommandResult<()> {
    let result = logged("install_claude_integration", || {
        state.integration().install()?;
        state.integration().set_consent(Consent::Accepted)
    });
    // Also after a failure: half of it may have been done.
    state.check(&app);
    result
}

/// Removes the plugin. Records that the user does not want it any more,
/// so that it is not offered again on its own.
///
/// # Errors
///
/// `invalidSettings` when the settings of Claude Code are not valid JSON.
#[tauri::command(async)]
pub fn uninstall_claude_integration<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, ClaudeState>,
) -> CommandResult<()> {
    let result = logged("uninstall_claude_integration", || {
        state.integration().uninstall()?;
        state.integration().set_consent(Consent::Declined)
    });
    state.check(&app);
    result
}

/// Remembers what the user answered when asked. Installs nothing.
///
/// # Errors
///
/// When the answer cannot be written.
#[tauri::command(async)]
pub fn set_claude_consent<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, ClaudeState>,
    consent: ConsentAnswer,
) -> CommandResult<()> {
    let result = logged("set_claude_consent", || {
        state.integration().set_consent(match consent {
            ConsentAnswer::Accepted => Consent::Accepted,
            ConsentAnswer::Declined => Consent::Declined,
        })
    });
    state.check(&app);
    result
}

/// Shows a notification of the system about the session `session_id`.
/// Clicking it brings the window to the front and sends it the event
/// `notification-clicked` with `{ sessionId }`.
#[tauri::command]
pub fn notify<R: Runtime>(app: AppHandle<R>, title: String, body: String, session_id: String) {
    crate::notify::show(&app, &title, &body, session_id);
}

/// Shows `count` (the sessions waiting) on the icon of the application, or
/// nothing for 0. Returns whether the platform shows such a badge: the
/// Dock on macOS, some launchers on Linux; never Windows.
#[tauri::command]
pub fn set_attention_badge<R: Runtime>(app: AppHandle<R>, count: u32) -> bool {
    crate::notify::set_badge(&app, count)
}
