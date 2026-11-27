//! Notifications of the system, and the badge of the application icon.
//!
//! The frontend decides when a session needs attention; this module only
//! shows it. A notification is shown with `notify-rust`, which tells which
//! ones are clicked on every platform; clicking one brings the window to
//! the front and sends it [`CLICKED_EVENT`] with the session, which the
//! frontend then shows.
//!
//! The badge (the number of sessions waiting) is shown on the Dock on macOS
//! and on launchers that support it on Linux. Windows has no such badge:
//! there it does nothing.
//!
//! Nothing here logs the title or the body of a notification: they can
//! name a project or quote a session.

use std::sync::atomic::{AtomicUsize, Ordering};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime};
use termx_core::diagnostics::tracing::{debug, warn};

/// The event the window receives when a notification is clicked.
pub const CLICKED_EVENT: &str = "notification-clicked";

/// The longest title and body that are shown, in characters.
const MAX_TEXT_CHARS: usize = 256;

/// How many notifications are watched for a click at once. Each takes a
/// thread that waits; past this, notifications are still shown, without
/// reacting to a click.
const MAX_WATCHED: usize = 16;

static WATCHED: AtomicUsize = AtomicUsize::new(0);

/// The payload of [`CLICKED_EVENT`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationClicked {
    /// The session the notification was about.
    pub session_id: String,
}

/// Shows a notification about `session_id`. Returns at once; the
/// notification is shown, and watched for a click, on a thread of its own.
pub fn show<R: Runtime>(app: &AppHandle<R>, title: &str, body: &str, session_id: String) {
    let title = shorten(title);
    let body = shorten(body);
    let app = app.clone();
    let identifier = app.config().identifier.clone();
    let watch = WATCHED.fetch_add(1, Ordering::AcqRel) < MAX_WATCHED;
    if !watch {
        WATCHED.fetch_sub(1, Ordering::AcqRel);
    }
    let spawned = std::thread::Builder::new()
        .name("termx-notification".to_owned())
        .spawn(move || {
            deliver(&app, &identifier, &title, &body, &session_id, watch);
            if watch {
                WATCHED.fetch_sub(1, Ordering::AcqRel);
            }
        });
    if let Err(error) = spawned {
        if watch {
            WATCHED.fetch_sub(1, Ordering::AcqRel);
        }
        warn!(error = %error, "could not start the thread of a notification");
    }
}

fn deliver<R: Runtime>(
    app: &AppHandle<R>,
    identifier: &str,
    title: &str,
    body: &str,
    session_id: &str,
    watch: bool,
) {
    let mut notification = notify_rust::Notification::new();
    notification.summary(title).body(body).appname("termx");
    #[cfg(target_os = "macos")]
    {
        // Only a bundled termx has its own identifier; otherwise the
        // notification is shown on behalf of the default application.
        let _ = notify_rust::set_application(identifier);
    }
    #[cfg(windows)]
    {
        notification.app_id(identifier);
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // Servers only report a click on the body for a default action.
        let _ = identifier;
        notification.action("default", "Show");
    }
    let handle = match notification.show() {
        Ok(handle) => handle,
        Err(error) => {
            warn!(error = %error, "a notification could not be shown");
            return;
        }
    };
    if !watch {
        return;
    }
    handle.wait_for_action(|action| {
        if action == "__closed" {
            return;
        }
        debug!("a notification was clicked");
        focus(app, session_id);
    });
}

/// Brings the window to the front and tells it which session to show.
fn focus<R: Runtime>(app: &AppHandle<R>, session_id: &str) {
    if let Some(window) = app.get_webview_window(crate::MAIN_WINDOW) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
    let payload = NotificationClicked {
        session_id: session_id.to_owned(),
    };
    if let Err(error) = app.emit_to(crate::MAIN_WINDOW, CLICKED_EVENT, payload) {
        warn!(error = %error, "the click on a notification could not reach the window");
    }
}

/// Shows `count` on the icon of the application, or nothing for 0.
/// Returns whether the platform shows such a badge.
pub fn set_badge<R: Runtime>(app: &AppHandle<R>, count: u32) -> bool {
    if cfg!(windows) {
        return false;
    }
    let Some(window) = app.get_webview_window(crate::MAIN_WINDOW) else {
        return false;
    };
    let count = (count > 0).then_some(i64::from(count));
    match window.set_badge_count(count) {
        Ok(()) => true,
        Err(error) => {
            debug!(error = %error, "the badge could not be set");
            false
        }
    }
}

/// `text` without control characters, cut at [`MAX_TEXT_CHARS`].
fn shorten(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .take(MAX_TEXT_CHARS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts_are_cleaned_and_cut() {
        assert_eq!(shorten("a\u{1b}[31mb\nc"), "a[31mb\nc");
        assert_eq!(shorten(&"x".repeat(1000)).chars().count(), MAX_TEXT_CHARS);
    }

    #[test]
    fn the_click_is_sent_with_the_session() {
        let payload = serde_json::to_value(NotificationClicked {
            session_id: "s-1".to_owned(),
        })
        .unwrap();
        assert_eq!(payload, serde_json::json!({ "sessionId": "s-1" }));
    }
}
