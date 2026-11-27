//! The local socket of termx, which the hooks of Claude Code (and later the
//! command line and the MCP server of termx) talk to.
//!
//! The protocol is that of [`termx_protocol`]: one JSON request per line,
//! one response line for each. This module only listens, authenticates and
//! frames: what a request does is up to the [`Handler`] given to
//! [`Server::start`].
//!
//! # Who can connect
//!
//! Only processes of the user who runs termx, on the same machine, and
//! only with the ticket of this run ([`Server::ticket`]), which termx gives
//! to the programs of its terminals and to nothing else:
//!
//! - **macOS and Linux**: a Unix domain socket, `<pid>.sock`, with mode
//!   `0600`, in a directory of the user with mode `0700`
//!   ([`socket_dir`]). The user id of every peer is checked as well.
//!   Nothing listens on the network.
//! - **Windows**: a named pipe, `\\.\pipe\termx-<pid>-<random>`, whose
//!   access list grants access to the current user only, which refuses
//!   remote clients, and which termx must have created itself (the first
//!   instance of the name: a pipe some other program created first is not
//!   used).
//!
//! # Limits
//!
//! A line is at most [`MAX_MESSAGE_BYTES`] bytes; a longer one is answered
//! with `tooLarge` and the connection is closed. At most
//! [`MAX_CONNECTIONS`] connections are served at once; more wait for a
//! place (and the hooks give up on their own after a short time). A
//! connection that sends nothing for [`IDLE_TIMEOUT`] is closed. Input that
//! is not a request is answered with an error and never stops the server.
//!
//! # Life of the socket
//!
//! The socket exists while the [`Server`] does. [`Server::shutdown`] (or
//! dropping it) removes the socket file. A socket file left by a termx that
//! crashed is removed by the next termx that starts: the file is named
//! after the process that created it, and files of processes that are gone
//! are deleted.
//!
//! Nothing here logs what a request contains: only counts and kinds.

use std::io;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use termx_protocol::{
    ErrorKind, MAX_MESSAGE_BYTES, Op, PROTOCOL_VERSION, Response, TICKET_HEX_LEN, tickets_match,
};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{Semaphore, watch};

use crate::diagnostics::tracing::{debug, info, warn};
use crate::pty::lock;

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
pub use unix::socket_dir;

/// The most connections served at once.
pub const MAX_CONNECTIONS: usize = 32;

/// How long a connection may stay silent before it is closed.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(5);

/// What the server does with the requests it accepted: those that carry
/// the ticket and a known operation.
pub trait Handler: Send + Sync + 'static {
    /// Carries out `op` and says how it went.
    fn handle(&self, op: Op) -> Response;
}

impl<F: Fn(Op) -> Response + Send + Sync + 'static> Handler for F {
    fn handle(&self, op: Op) -> Response {
        self(op)
    }
}

/// How a [`Server`] is set up. Start from [`ServerConfig::default`].
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ServerConfig {
    /// See [`MAX_CONNECTIONS`].
    pub max_connections: usize,
    /// See [`IDLE_TIMEOUT`].
    pub idle_timeout: Duration,
    /// The directory of the socket (macOS and Linux). [`socket_dir`] when
    /// `None`.
    pub dir: Option<std::path::PathBuf>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            max_connections: MAX_CONNECTIONS,
            idle_timeout: IDLE_TIMEOUT,
            dir: None,
        }
    }
}

/// The local socket, listening on a thread of its own.
pub struct Server {
    address: String,
    ticket: String,
    stop: watch::Sender<bool>,
    thread: std::sync::Mutex<Option<JoinHandle<()>>>,
    #[cfg(unix)]
    path: std::path::PathBuf,
}

impl Server {
    /// Creates the socket and starts serving `handler` on it.
    ///
    /// # Errors
    ///
    /// Fails when the socket cannot be created: no directory for it, a
    /// directory that is not the user's own, a pipe name already taken.
    pub fn start(handler: Arc<dyn Handler>, config: ServerConfig) -> io::Result<Self> {
        let ticket = new_ticket()?;
        let (stop, stopped) = watch::channel(false);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()?;
        let shared = Arc::new(Shared {
            handler,
            ticket: ticket.clone(),
            idle_timeout: config.idle_timeout,
            permits: Arc::new(Semaphore::new(config.max_connections.max(1))),
        });

        #[cfg(unix)]
        let (listener, path) = {
            let _guard = runtime.enter();
            unix::bind(config.dir.as_deref())?
        };
        #[cfg(unix)]
        let address = path.to_string_lossy().into_owned();
        #[cfg(windows)]
        let (listener, address) = {
            let _guard = runtime.enter();
            windows::Listener::create(config.max_connections.max(1))?
        };

        let thread = thread::Builder::new()
            .name("termx-socket".to_owned())
            .spawn(move || {
                runtime.block_on(async move {
                    #[cfg(unix)]
                    unix::serve(listener, shared, stopped).await;
                    #[cfg(windows)]
                    listener.serve(shared, stopped).await;
                });
            })?;
        info!("socket listening");
        Ok(Self {
            address,
            ticket,
            stop,
            thread: std::sync::Mutex::new(Some(thread)),
            #[cfg(unix)]
            path,
        })
    }

    /// The address clients connect to: what [`termx_protocol::ENV_SOCKET`]
    /// holds.
    #[must_use]
    pub fn address(&self) -> &str {
        &self.address
    }

    /// The ticket of this run: what [`termx_protocol::ENV_TICKET`] holds.
    #[must_use]
    pub fn ticket(&self) -> &str {
        &self.ticket
    }

    /// Stops serving and removes the socket. Connections in progress are
    /// dropped. Called when the server is dropped.
    pub fn shutdown(&self) {
        let _ = self.stop.send(true);
        if let Some(thread) = lock(&self.thread).take() {
            let _ = thread.join();
            #[cfg(unix)]
            unix::remove(&self.path);
            info!("socket closed");
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl std::fmt::Debug for Server {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Server").finish_non_exhaustive()
    }
}

/// What every connection needs.
struct Shared {
    handler: Arc<dyn Handler>,
    ticket: String,
    idle_timeout: Duration,
    permits: Arc<Semaphore>,
}

/// A ticket drawn from the random generator of the system.
fn new_ticket() -> io::Result<String> {
    let mut bytes = [0u8; TICKET_HEX_LEN / 2];
    getrandom::fill(&mut bytes).map_err(|error| io::Error::other(error.to_string()))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Serves one connection until it ends, goes silent, or sends something
/// that is not worth answering further.
async fn serve_connection(stream: impl AsyncRead + AsyncWrite + Unpin, shared: &Shared) {
    let (reader, mut writer) = tokio::io::split(stream);
    let mut reader = BufReader::new(reader);
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = tokio::time::timeout(
            shared.idle_timeout,
            read_line(&mut reader, &mut line, MAX_MESSAGE_BYTES),
        )
        .await;
        let (response, close) = match read {
            // Silent for too long.
            Err(_) => return,
            Ok(Err(_) | Ok(Line::End)) => return,
            Ok(Ok(Line::TooLong)) => (
                Response::error(
                    ErrorKind::TooLarge,
                    format!("a line is at most {MAX_MESSAGE_BYTES} bytes"),
                ),
                true,
            ),
            Ok(Ok(Line::Complete)) => answer(&line, shared),
        };
        let written = tokio::time::timeout(shared.idle_timeout, async {
            writer.write_all(&response.to_line()).await?;
            writer.flush().await
        })
        .await;
        if close || !matches!(written, Ok(Ok(()))) {
            return;
        }
    }
}

/// What [`read_line`] read.
enum Line {
    /// A whole line, without its newline.
    Complete,
    /// The connection ended. Bytes without a newline before the end are
    /// dropped.
    End,
    /// The line is longer than allowed; what was read of it is dropped.
    TooLong,
}

/// Reads one line into `line`, at most `limit` bytes with its newline.
async fn read_line(
    reader: &mut (impl tokio::io::AsyncBufRead + Unpin),
    line: &mut Vec<u8>,
    limit: usize,
) -> io::Result<Line> {
    // Past the limit, the rest of the line is read and dropped, so that
    // the client is still reading when it gets its answer (closing on
    // unread input would reset the connection and lose the answer), up to
    // `DISCARD_LIMIT`.
    let mut discarded = 0usize;
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Ok(if discarded > 0 {
                Line::TooLong
            } else {
                Line::End
            });
        }
        let (taken, found) = match available.iter().position(|b| *b == b'\n') {
            Some(at) => (at + 1, true),
            None => (available.len(), false),
        };
        if discarded > 0 || line.len() + taken > limit {
            discarded += taken;
            line.clear();
            reader.consume(taken);
            if found || discarded > DISCARD_LIMIT {
                return Ok(Line::TooLong);
            }
            continue;
        }
        line.extend_from_slice(&available[..taken]);
        reader.consume(taken);
        if found {
            line.pop();
            return Ok(Line::Complete);
        }
    }
}

/// How much of a line that is too long is read and dropped before the
/// connection is closed anyway.
const DISCARD_LIMIT: usize = 16 * 1024 * 1024;

/// The response to one line, and whether to close the connection after
/// it.
fn answer(line: &[u8], shared: &Shared) -> (Response, bool) {
    let refuse = |kind, message: &str| (Response::error(kind, message), true);
    let Ok(serde_json::Value::Object(mut request)) = serde_json::from_slice(line) else {
        debug!(
            bytes = line.len(),
            "socket: a request was not a JSON object"
        );
        return refuse(ErrorKind::Malformed, "a request is a JSON object");
    };
    if request.get("v").and_then(serde_json::Value::as_u64) != Some(u64::from(PROTOCOL_VERSION)) {
        return refuse(
            ErrorKind::UnsupportedVersion,
            "this termx speaks version 1 of the protocol",
        );
    }
    let ticket_matches = request
        .get("ticket")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|ticket| tickets_match(&shared.ticket, ticket));
    if !ticket_matches {
        warn!("socket: a request without the right ticket was refused");
        return refuse(ErrorKind::Unauthorized, "the ticket is missing or wrong");
    }
    request.remove("v");
    request.remove("ticket");
    let known = matches!(
        request.get("op").and_then(serde_json::Value::as_str),
        Some("ping" | "hook")
    );
    if !known {
        return (
            Response::error(
                ErrorKind::UnknownOp,
                "this termx does not know this operation",
            ),
            false,
        );
    }
    match serde_json::from_value::<Op>(serde_json::Value::Object(request)) {
        Ok(Op::Ping) => (Response::pong(crate::version()), false),
        Ok(op) => {
            let handler = Arc::clone(&shared.handler);
            // A panic in the handler is a bug; it must not take the
            // server down with it.
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handler.handle(op))) {
                Ok(response) => (response, false),
                Err(_) => (
                    Response::error(ErrorKind::Internal, "termx could not carry it out"),
                    false,
                ),
            }
        }
        Err(_) => refuse(ErrorKind::Malformed, "the fields of the request are wrong"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared(handler: impl Handler) -> Shared {
        Shared {
            handler: Arc::new(handler),
            ticket: "0123456789abcdef0123456789abcdef".to_owned(),
            idle_timeout: IDLE_TIMEOUT,
            permits: Arc::new(Semaphore::new(1)),
        }
    }

    fn answer_to(line: &str) -> (serde_json::Value, bool) {
        let shared = shared(|_| Response::ok());
        let (response, close) = answer(line.as_bytes(), &shared);
        (serde_json::to_value(response).unwrap(), close)
    }

    const TICKET: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn requests_are_answered_according_to_what_they_are() {
        let kind = |line: &str| answer_to(line).0["error"]["kind"].clone();

        assert_eq!(kind("not json"), "malformed");
        assert_eq!(kind("[1]"), "malformed");
        assert_eq!(
            kind(r#"{"v":2,"ticket":"x","op":"ping"}"#),
            "unsupportedVersion"
        );
        assert_eq!(kind(r#"{"v":1,"op":"ping"}"#), "unauthorized");
        assert_eq!(
            kind(r#"{"v":1,"ticket":"0123456789abcdef0123456789abcdee","op":"ping"}"#),
            "unauthorized"
        );
        assert_eq!(
            kind(&format!(r#"{{"v":1,"ticket":"{TICKET}","op":"fly"}}"#)),
            "unknownOp"
        );
        assert_eq!(
            kind(&format!(r#"{{"v":1,"ticket":"{TICKET}","op":"hook"}}"#)),
            "malformed"
        );

        let (pong, close) = answer_to(&format!(r#"{{"v":1,"ticket":"{TICKET}","op":"ping"}}"#));
        assert!(!close);
        assert_eq!(pong["ok"], true);
        assert_eq!(pong["protocol"], 1);

        let (ok, close) = answer_to(&format!(
            r#"{{"v":1,"ticket":"{TICKET}","op":"hook","session":"s-1","event":{{"name":"Stop","at":1}}}}"#
        ));
        assert!(!close);
        assert_eq!(ok, serde_json::json!({"v": 1, "ok": true}));
    }

    #[test]
    fn only_refusals_that_say_nothing_more_can_be_done_close_the_connection() {
        assert!(answer_to("x").1);
        assert!(answer_to(r#"{"v":1,"op":"ping"}"#).1);
        assert!(!answer_to(&format!(r#"{{"v":1,"ticket":"{TICKET}","op":"fly"}}"#)).1);
    }

    #[test]
    fn a_handler_that_panics_is_an_internal_error() {
        let shared = shared(|_| -> Response { panic!("bug") });
        let line = format!(
            r#"{{"v":1,"ticket":"{TICKET}","op":"hook","session":"s-1","event":{{"name":"Stop","at":1}}}}"#
        );
        let (response, close) = answer(line.as_bytes(), &shared);
        assert_eq!(response.error.unwrap().kind, ErrorKind::Internal);
        assert!(!close);
    }

    #[test]
    fn tickets_are_random_and_well_formed() {
        let a = new_ticket().unwrap();
        let b = new_ticket().unwrap();
        assert_ne!(a, b);
        assert!(termx_protocol::is_valid_ticket(&a), "{a}");
    }
}
