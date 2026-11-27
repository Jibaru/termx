//! Wires the Claude Code integration of `termx-core` into the application:
//! the local socket the hooks report to, and the plugin installer.
//!
//! [`ClaudeState`] starts the socket ([`termx_core::socket`]) when the
//! application starts, hands what the hooks report to the workspace, and
//! gives the workspace the address and the ticket of the socket, which every
//! session started from then on receives in its environment. Without a
//! socket (it could not be created), termx works as before, without agent
//! states.
//!
//! It also owns the [`Integration`] behind the commands of the
//! integration ([`crate::commands::claude`]). Once the application runs,
//! [`ClaudeState::watch`] upgrades a plugin left by another version of
//! termx (when the user accepted it) and looks again every
//! [`CHECK_INTERVAL`] for changes made elsewhere.

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError, Weak};

use tauri::{AppHandle, Emitter, Manager, Runtime};
use termx_core::claude::{Integration, IntegrationStatus, Paths};
use termx_core::diagnostics::tracing::{info, warn};
use termx_core::socket::{Server, ServerConfig};
use termx_core::workspace::{AgentChannel, SessionKey, Workspace, WorkspaceError};
use termx_protocol::{ErrorKind, Op, Response};

/// Application state for the Claude Code integration, managed by Tauri.
pub struct ClaudeState {
    integration: Integration,
    server: Mutex<Option<Server>>,
    /// The status the window was last given, to tell it when it changes.
    last: Mutex<Option<IntegrationStatus>>,
}

/// The event the window receives, without a payload, whenever the plugin is installed, upgraded or removed, the answer of the
/// user changes, or a check finds that something else changed them (the
/// settings edited by hand).
pub const CHANGED_EVENT: &str = "claude-integration-changed";

impl ClaudeState {
    /// Starts the socket for `workspace`, and prepares the installer with
    /// the plugin in `state_dir`. Never fails: a socket that cannot be
    /// created is logged, and the sessions then get no agent state.
    #[must_use]
    pub fn start(workspace: &Arc<Workspace>, state_dir: Option<&Path>) -> Self {
        let fallback = std::env::temp_dir().join("termx");
        let mut paths = Paths::of_user(state_dir.unwrap_or(&fallback));
        if state_dir.is_none() {
            paths.consent_file = None;
        }
        let integration = Integration::new(paths);

        let weak = Arc::downgrade(workspace);
        let server = match Server::start(
            Arc::new(move |op| handle(&weak, op)),
            ServerConfig::default(),
        ) {
            Ok(server) => {
                workspace.set_agent_channel(Some(AgentChannel {
                    socket: server.address().to_owned(),
                    ticket: server.ticket().to_owned(),
                }));
                Some(server)
            }
            Err(error) => {
                warn!(error = %error, "the socket for claude code could not be created; sessions get no agent state");
                None
            }
        };

        Self {
            last: Mutex::new(Some(integration.status())),
            integration,
            server: Mutex::new(server),
        }
    }

    /// The integration at `integration`, without a socket: for tests.
    #[must_use]
    pub fn with_integration(integration: Integration) -> Self {
        Self {
            last: Mutex::new(Some(integration.status())),
            integration,
            server: Mutex::new(None),
        }
    }

    /// The installer.
    #[must_use]
    pub fn integration(&self) -> &Integration {
        &self.integration
    }

    /// The status now. When it differs from the one last read (or the one
    /// at start), the window is told with [`CHANGED_EVENT`].
    pub fn check<R: Runtime>(&self, app: &AppHandle<R>) -> IntegrationStatus {
        let status = self.integration.status();
        let previous = self
            .last
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .replace(status.clone());
        if previous.is_some_and(|previous| previous != status)
            && let Err(error) = app.emit(CHANGED_EVENT, ())
        {
            warn!(error = %error, "the window could not be told that the claude code integration changed");
        }
        status
    }

    /// Closes the socket. Called when the application exits.
    pub fn shutdown(&self) {
        let server = self
            .server
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        drop(server);
    }

    /// Watches the integration, on a thread of its own, once the
    /// application runs: brings an installed plugin up to date when the
    /// user accepted it, then checks every [`CHECK_INTERVAL`] whether
    /// something else changed it (the settings edited by hand, Claude Code
    /// installed), telling the window with [`CHANGED_EVENT`].
    pub fn watch<R: Runtime>(app: &AppHandle<R>) {
        let app = app.clone();
        let spawned = std::thread::Builder::new()
            .name("termx-claude-watch".to_owned())
            .spawn(move || {
                let Some(state) = app.try_state::<Self>() else {
                    return;
                };
                if state.integration.consent() == termx_core::claude::Consent::Accepted {
                    match state.integration.upgrade() {
                        Ok(true) => info!("claude code plugin upgraded"),
                        Ok(false) => {}
                        Err(error) => {
                            warn!(kind = ?error.kind, "the claude code plugin could not be upgraded");
                        }
                    }
                }
                loop {
                    state.check(&app);
                    std::thread::sleep(CHECK_INTERVAL);
                }
            });
        if let Err(error) = spawned {
            warn!(error = %error, "could not start the thread that watches the claude code integration");
        }
    }
}

/// How often [`ClaudeState::watch`] looks at the integration again.
pub const CHECK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// What the socket does with a request: a report of a hook goes to the
/// workspace.
fn handle(workspace: &Weak<Workspace>, op: Op) -> Response {
    let Some(workspace) = workspace.upgrade() else {
        return Response::error(ErrorKind::Internal, "termx is quitting");
    };
    match op {
        Op::Ping => Response::pong(termx_core::version()),
        Op::Hook { session, event } => {
            let Ok(key) = SessionKey::from_raw(session) else {
                return Response::error(ErrorKind::UnknownSession, "no such session");
            };
            match workspace.report_hook(&key, &event) {
                Ok(_) => Response::ok(),
                Err(WorkspaceError::UnknownSession(_)) => {
                    Response::error(ErrorKind::UnknownSession, "no such session")
                }
                Err(_) => Response::error(ErrorKind::Internal, "the report could not be applied"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use termx_core::workspace::{
        AgentStatus, LaunchMode, NewSession, SessionKind, WorkspaceConfig,
    };
    use termx_protocol::{HookEvent, Request, client};

    use super::*;

    #[test]
    fn a_hook_reaches_the_session_through_the_socket() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = Arc::new(Workspace::open(WorkspaceConfig::in_memory()));
        let state = ClaudeState::start(&workspace, Some(dir.path()));
        let session = workspace
            .create_session(NewSession::new(SessionKind::Claude, dir.path()))
            .unwrap();
        let launch = workspace
            .begin_launch(&session.id, LaunchMode::Fresh)
            .unwrap();
        workspace.terminal_started(&session.id, launch.generation, 1);
        let socket = launch
            .command
            .env_value(termx_protocol::ENV_SOCKET)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let ticket = launch
            .command
            .env_value(termx_protocol::ENV_TICKET)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let event = HookEvent {
            name: "PermissionRequest".to_owned(),
            claude_session_id: Some("c-1".to_owned()),
            notification_type: None,
            tool: None,
            source: None,
            reason: None,
            agent: None,
            pid: None,
            at: termx_core::workspace::now_ms() * 1000,
        };

        let response = client::send(
            &socket,
            Request::new(
                &ticket,
                Op::Hook {
                    session: session.id.to_string(),
                    event: event.clone(),
                },
            )
            .to_line(),
            std::time::Duration::from_secs(10),
        )
        .unwrap();

        assert!(response.ok, "{response:?}");
        let agent = workspace.session(&session.id).unwrap().agent.unwrap();
        assert_eq!(agent.status, AgentStatus::WaitingForPermission);
        // An unknown session is said to be one.
        let unknown = client::send(
            &socket,
            Request::new(
                &ticket,
                Op::Hook {
                    session: "s-nobody".to_owned(),
                    event,
                },
            )
            .to_line(),
            std::time::Duration::from_secs(10),
        )
        .unwrap();
        assert_eq!(unknown.error.unwrap().kind, ErrorKind::UnknownSession);
        state.shutdown();
    }
}
