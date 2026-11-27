//! The local socket: who may talk to it, what it does with what it gets,
//! and what it leaves behind.

#![allow(clippy::unwrap_used)]

use std::io::{BufRead, BufReader, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use termx_core::socket::{Server, ServerConfig};
use termx_protocol::{ErrorKind, HookEvent, Op, Request, Response, client};

const TIMEOUT: Duration = Duration::from_secs(10);

/// A server that records the operations it is handed.
struct Recorded {
    server: Server,
    ops: Arc<Mutex<Vec<Op>>>,
    _dir: Option<tempfile::TempDir>,
}

/// A directory short enough for a socket path, removed afterwards.
#[cfg(unix)]
fn socket_dir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("termx-sock-")
        .tempdir_in("/tmp")
        .unwrap()
}

fn start() -> Recorded {
    start_with(|config| config)
}

fn start_with(configure: impl FnOnce(ServerConfig) -> ServerConfig) -> Recorded {
    let ops = Arc::new(Mutex::new(Vec::new()));
    let handler = {
        let ops = Arc::clone(&ops);
        move |op: Op| {
            ops.lock().unwrap().push(op);
            Response::ok()
        }
    };
    #[cfg(unix)]
    let dir = Some(socket_dir());
    #[cfg(not(unix))]
    let dir: Option<tempfile::TempDir> = None;
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut config = ServerConfig::default();
    #[cfg(unix)]
    {
        config.dir = dir.as_ref().map(|dir| dir.path().join("run"));
    }
    let server = Server::start(Arc::new(handler), configure(config)).unwrap();
    Recorded {
        server,
        ops,
        _dir: dir,
    }
}

fn hook(session: &str, name: &str) -> Op {
    Op::Hook {
        session: session.to_owned(),
        event: HookEvent {
            name: name.to_owned(),
            claude_session_id: Some("c-1".to_owned()),
            notification_type: None,
            tool: None,
            source: None,
            reason: None,
            agent: None,
            pid: None,
            at: 1,
        },
    }
}

fn send(server: &Server, line: Vec<u8>) -> Response {
    client::send(server.address(), line, TIMEOUT).unwrap()
}

/// A raw connection, to send what a well-behaved client never would.
#[cfg(unix)]
fn raw(server: &Server) -> std::os::unix::net::UnixStream {
    let stream = std::os::unix::net::UnixStream::connect(server.address()).unwrap();
    stream.set_read_timeout(Some(TIMEOUT)).unwrap();
    stream
}

/// Opens the pipe, waiting while every instance is taken for a moment
/// (`ERROR_PIPE_BUSY`), as `termx_protocol::client` does.
#[cfg(windows)]
fn raw(server: &Server) -> std::fs::File {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(server.address())
        {
            Err(error) if error.raw_os_error() == Some(231) && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(2));
            }
            other => return other.unwrap(),
        }
    }
}

#[test]
fn a_request_with_the_ticket_is_handed_on() {
    let recorded = start();
    let server = &recorded.server;

    let response = send(
        server,
        Request::new(server.ticket(), hook("s-1", "Stop")).to_line(),
    );

    assert_eq!(response, Response::ok());
    assert_eq!(*recorded.ops.lock().unwrap(), vec![hook("s-1", "Stop")]);
    let pong = send(server, Request::new(server.ticket(), Op::Ping).to_line());
    assert_eq!(pong.protocol, Some(1));
    assert_eq!(pong.app.as_deref(), Some("termx"));
}

#[test]
fn a_request_without_the_right_ticket_is_refused_and_not_handed_on() {
    let recorded = start();
    let server = &recorded.server;
    let wrong = "f".repeat(32);

    for line in [
        Request::new(&wrong, hook("s-1", "Stop")).to_line(),
        Request::new("", hook("s-1", "Stop")).to_line(),
        b"{\"v\":1,\"op\":\"hook\",\"session\":\"s-1\",\"event\":{\"name\":\"Stop\",\"at\":1}}\n"
            .to_vec(),
    ] {
        let response = send(server, line);
        assert_eq!(response.error.unwrap().kind, ErrorKind::Unauthorized);
    }
    assert!(recorded.ops.lock().unwrap().is_empty());
}

#[test]
fn malformed_and_oversized_input_is_answered_and_the_server_carries_on() {
    let recorded = start();
    let server = &recorded.server;

    for (line, kind) in [
        (b"not json\n".to_vec(), ErrorKind::Malformed),
        (b"\xff\xfe\x00\n".to_vec(), ErrorKind::Malformed),
        (b"{\"v\":9}\n".to_vec(), ErrorKind::UnsupportedVersion),
        (
            {
                let mut line = vec![b'a'; termx_protocol::MAX_MESSAGE_BYTES + 10];
                line.push(b'\n');
                line
            },
            ErrorKind::TooLarge,
        ),
    ] {
        let response = send(server, line);
        assert_eq!(response.error.unwrap().kind, kind);
    }

    // A connection that sends half a line and goes away costs nothing.
    let mut half = raw(server);
    half.write_all(b"{\"v\":1,").unwrap();
    drop(half);

    // Still serving.
    let response = send(server, Request::new(server.ticket(), Op::Ping).to_line());
    assert!(response.ok);
    assert!(recorded.ops.lock().unwrap().is_empty());
}

#[test]
fn several_requests_share_a_connection_in_order() {
    let recorded = start();
    let server = &recorded.server;
    let mut stream = raw(server);
    for name in ["UserPromptSubmit", "PreToolUse", "Stop"] {
        stream
            .write_all(&Request::new(server.ticket(), hook("s-1", name)).to_line())
            .unwrap();
    }
    let mut reader = BufReader::new(stream);
    for _ in 0..3 {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let response: Response = serde_json::from_str(&line).unwrap();
        assert!(response.ok, "{line}");
    }
    let names: Vec<String> = recorded
        .ops
        .lock()
        .unwrap()
        .iter()
        .map(|op| match op {
            Op::Hook { event, .. } => event.name.clone(),
            Op::Ping => "ping".to_owned(),
        })
        .collect();
    assert_eq!(names, ["UserPromptSubmit", "PreToolUse", "Stop"]);
}

#[test]
fn many_senders_at_once_are_all_served() {
    let recorded = start_with(|mut config| {
        config.max_connections = 4;
        config
    });
    let server = Arc::new(recorded.server);
    let delivered = Arc::new(AtomicUsize::new(0));
    let senders: Vec<_> = (0..64)
        .map(|n| {
            let server = Arc::clone(&server);
            let delivered = Arc::clone(&delivered);
            std::thread::spawn(move || {
                let line = Request::new(server.ticket(), hook(&format!("s-{n}"), "Stop")).to_line();
                if client::send(server.address(), line, Duration::from_secs(30))
                    .is_ok_and(|response| response.ok)
                {
                    delivered.fetch_add(1, Ordering::Relaxed);
                }
            })
        })
        .collect();
    for sender in senders {
        sender.join().unwrap();
    }
    assert_eq!(delivered.load(Ordering::Relaxed), 64);
    assert_eq!(recorded.ops.lock().unwrap().len(), 64);
}

#[test]
fn a_silent_connection_is_closed_and_does_not_hold_its_place() {
    let recorded = start_with(|mut config| {
        config.max_connections = 1;
        config.idle_timeout = Duration::from_millis(200);
        config
    });
    let server = &recorded.server;
    let silent = raw(server);

    // The only place is taken until the silent one is closed.
    let started = Instant::now();
    let response = client::send(
        server.address(),
        Request::new(server.ticket(), Op::Ping).to_line(),
        TIMEOUT,
    )
    .unwrap();
    assert!(response.ok);
    assert!(started.elapsed() < Duration::from_secs(5));
    drop(silent);
}

#[test]
fn idle_connections_do_not_keep_a_hook_out() {
    let recorded = start();
    let server = &recorded.server;
    // Another process of the user opens 64 connections and sends nothing.
    let idle: Vec<_> = (0..64).map(|_| raw(server)).collect();

    let started = Instant::now();
    let response = client::send(
        server.address(),
        Request::new(server.ticket(), hook("s-1", "Stop")).to_line(),
        termx_protocol::hook::HOOK_TIMEOUT,
    )
    .unwrap();

    assert!(response.ok);
    assert!(started.elapsed() < termx_protocol::hook::HOOK_TIMEOUT);
    assert_eq!(recorded.ops.lock().unwrap().len(), 1);
    drop(idle);
}

#[test]
fn shutting_down_stops_serving() {
    let recorded = start();
    let address = recorded.server.address().to_owned();
    let ticket = recorded.server.ticket().to_owned();

    recorded.server.shutdown();

    let result = client::send(
        &address,
        Request::new(&ticket, Op::Ping).to_line(),
        Duration::from_secs(2),
    );
    assert!(result.is_err(), "{result:?}");
}

#[test]
fn every_run_has_its_own_ticket_and_address() {
    let first = start();
    let second = start();
    assert_ne!(first.server.ticket(), second.server.ticket());
    assert_ne!(first.server.address(), second.server.address());
    // One run's ticket is worthless at the other.
    let response = send(
        &second.server,
        Request::new(first.server.ticket(), Op::Ping).to_line(),
    );
    assert_eq!(response.error.unwrap().kind, ErrorKind::Unauthorized);
}

#[cfg(unix)]
mod unix {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    use super::*;

    #[test]
    fn only_the_user_can_reach_the_socket() {
        let recorded = start();
        let path = std::path::Path::new(recorded.server.address());

        let socket = std::fs::symlink_metadata(path).unwrap();
        assert_eq!(socket.mode() & 0o777, 0o600);
        let dir = std::fs::symlink_metadata(path.parent().unwrap()).unwrap();
        assert_eq!(dir.mode() & 0o777, 0o700);
        assert_eq!(dir.uid(), rustix_uid());
    }

    #[test]
    fn a_directory_others_can_enter_is_closed_first() {
        let base = socket_dir();
        let dir = base.path().join("open");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).unwrap();
        let mut config = ServerConfig::default();
        config.dir = Some(dir.clone());

        let server = Server::start(Arc::new(|_| Response::ok()), config).unwrap();

        assert_eq!(
            std::fs::metadata(&dir).unwrap().mode() & 0o777,
            0o700,
            "the directory was left open"
        );
        drop(server);
    }

    #[test]
    fn the_socket_file_is_removed_on_shutdown() {
        let recorded = start();
        let path = std::path::PathBuf::from(recorded.server.address());
        assert!(path.exists());

        drop(recorded.server);

        assert!(!path.exists());
    }

    #[test]
    fn sockets_left_by_a_termx_that_crashed_are_removed() {
        let base = socket_dir();
        let dir = base.path().join("run");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        // A process number that cannot be running, and this very process
        // (a socket left by an earlier process with the same number).
        let dead = dir.join(format!("{}.sock", i32::MAX - 1));
        let same = dir.join(format!("{}.sock", std::process::id()));
        let other = dir.join("notes.txt");
        for path in [&dead, &same] {
            drop(std::os::unix::net::UnixListener::bind(path).unwrap());
        }
        std::fs::write(&other, "kept").unwrap();
        let mut config = ServerConfig::default();
        config.dir = Some(dir.clone());

        let server = Server::start(Arc::new(|_| Response::ok()), config).unwrap();

        assert!(!dead.exists(), "the stale socket is still there");
        assert!(other.exists(), "a file that is not a socket was removed");
        assert_eq!(std::path::Path::new(server.address()), same);
        let response = send(&server, Request::new(server.ticket(), Op::Ping).to_line());
        assert!(response.ok);
    }

    fn rustix_uid() -> u32 {
        // The owner of a file this process creates.
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::metadata(file.path()).unwrap().uid()
    }
}

#[cfg(windows)]
mod windows {
    use super::*;

    #[test]
    fn the_pipe_is_named_for_this_run_and_cannot_be_taken_over() {
        let recorded = start();
        let address = recorded.server.address();
        assert!(address.starts_with(r"\\.\pipe\termx-"), "{address}");

        // A second server on the same name is refused: termx only uses a
        // pipe it created first.
        let mut options = tokio::net::windows::named_pipe::ServerOptions::new();
        options.first_pipe_instance(true);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        assert!(options.create(address).is_err());
    }
}
