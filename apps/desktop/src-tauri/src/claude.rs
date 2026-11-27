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
//! It also owns the [`Integration`] behind the `claude_*` commands. When
//! the user accepted the plugin, a plugin left by another version of termx
//! is upgraded at start, on a thread of its own.

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError, Weak};

use termx_core::claude::{Integration, Paths};
use termx_core::diagnostics::tracing::{info, warn};
use termx_core::socket::{Server, ServerConfig};
use termx_core::workspace::{AgentChannel, SessionKey, Workspace, WorkspaceError};
use termx_protocol::{ErrorKind, Op, Response};

/// Application state for the Claude Code integration, managed by Tauri.
pub struct ClaudeState {
    integration: Integration,
    server: Mutex<Option<Server>>,
}

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

        let state = Self {
            integration,
            server: Mutex::new(server),
        };
        state.upgrade_in_background();
        state
    }

    /// The integration at `integration`, without a socket: for tests.
    #[must_use]
    pub fn with_integration(integration: Integration) -> Self {
        Self {
            integration,
            server: Mutex::new(None),
        }
    }

    /// The installer.
    #[must_use]
    pub fn integration(&self) -> &Integration {
        &self.integration
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

    /// Brings an installed plugin up to date, when the user accepted it.
    fn upgrade_in_background(&self) {
        if self.integration.consent() != termx_core::claude::Consent::Accepted {
            return;
        }
        let integration = self.integration.clone();
        let spawned = std::thread::Builder::new()
            .name("termx-claude-upgrade".to_owned())
            .spawn(move || match integration.upgrade() {
                Ok(true) => info!("claude code plugin upgraded"),
                Ok(false) => {}
                Err(error) => {
                    warn!(kind = ?error.kind, "the claude code plugin could not be upgraded");
                }
            });
        if let Err(error) = spawned {
            warn!(error = %error, "could not start the thread that upgrades the claude code plugin");
        }
    }
}

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
