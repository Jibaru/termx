//! Sends one request to termx and reads its response, within a deadline.
//!
//! Everything happens on a thread of its own, and the caller waits for it
//! at most the time it gives: a termx that is busy, stuck or gone costs the
//! caller that time and nothing more. The thread is left behind when it
//! does not finish in time, which is fine for the short-lived programs
//! that use this (they exit right after).

use std::io::{self, BufRead, BufReader, Read, Write};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::{MAX_MESSAGE_BYTES, Response};

/// Why a request got no response.
#[derive(Debug)]
pub enum ClientError {
    /// termx could not be reached or the connection failed.
    Io(io::Error),
    /// termx did not answer within the deadline.
    TimedOut,
    /// The answer was not a response.
    Malformed,
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::TimedOut => f.write_str("termx did not answer in time"),
            Self::Malformed => f.write_str("termx sent something that is not a response"),
        }
    }
}

impl std::error::Error for ClientError {}

/// Sends `line` (one request, newline included) to the socket at
/// `address` and returns the response, or gives up after `timeout`.
///
/// # Errors
///
/// [`ClientError`] when termx cannot be reached, does not answer in time,
/// or answers something else than a response.
pub fn send(address: &str, line: Vec<u8>, timeout: Duration) -> Result<Response, ClientError> {
    let deadline = Instant::now() + timeout;
    let (sender, receiver) = mpsc::channel();
    let address = address.to_owned();
    thread::Builder::new()
        .name("termx-client".to_owned())
        .spawn(move || {
            let _ = sender.send(exchange(&address, &line, deadline));
        })
        .map_err(ClientError::Io)?;
    receiver
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .map_err(|_| ClientError::TimedOut)?
}

/// Connects, writes the request and reads the response line.
fn exchange(address: &str, line: &[u8], deadline: Instant) -> Result<Response, ClientError> {
    let mut stream = connect(address, deadline).map_err(ClientError::Io)?;
    stream.write_all(line).map_err(ClientError::Io)?;
    stream.flush().map_err(ClientError::Io)?;
    let mut reader = BufReader::new(stream.take(MAX_MESSAGE_BYTES as u64));
    let mut answer = Vec::new();
    reader.read_until(b'\n', &mut answer).map_err(|error| {
        if matches!(
            error.kind(),
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
        ) {
            ClientError::TimedOut
        } else {
            ClientError::Io(error)
        }
    })?;
    serde_json::from_slice(&answer).map_err(|_| ClientError::Malformed)
}

#[cfg(unix)]
fn connect(address: &str, deadline: Instant) -> io::Result<std::os::unix::net::UnixStream> {
    let stream = std::os::unix::net::UnixStream::connect(address)?;
    // The thread is abandoned at the deadline anyway; the timeouts only
    // keep it from lingering much longer.
    let left = deadline
        .saturating_duration_since(Instant::now())
        .max(Duration::from_millis(1));
    stream.set_read_timeout(Some(left))?;
    stream.set_write_timeout(Some(left))?;
    Ok(stream)
}

/// Opens the pipe. When every instance of the pipe is taken for a moment
/// (`ERROR_PIPE_BUSY`), tries again until the deadline.
#[cfg(windows)]
fn connect(address: &str, deadline: Instant) -> io::Result<std::fs::File> {
    /// `ERROR_PIPE_BUSY`.
    const PIPE_BUSY: i32 = 231;
    loop {
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(address)
        {
            Err(error) if error.raw_os_error() == Some(PIPE_BUSY) && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(2));
            }
            other => return other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_socket_that_does_not_exist_fails_at_once() {
        let dir = std::env::temp_dir().join("termx-no-such-socket");
        let address = if cfg!(windows) {
            r"\\.\pipe\termx-no-such-pipe-for-a-test".to_owned()
        } else {
            dir.join("none.sock").to_string_lossy().into_owned()
        };
        let started = Instant::now();

        let result = send(&address, b"{}\n".to_vec(), Duration::from_secs(5));

        assert!(matches!(result, Err(ClientError::Io(_))), "{result:?}");
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[cfg(unix)]
    #[test]
    fn a_server_that_never_answers_costs_the_timeout_and_no_more() {
        let dir = tempdir();
        let path = dir.join("silent.sock");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        // Accepts, then holds the connection without answering.
        let holder = thread::spawn(move || listener.accept().map(|(stream, _)| stream));
        let started = Instant::now();

        let result = send(
            path.to_str().unwrap(),
            b"{}\n".to_vec(),
            Duration::from_millis(200),
        );

        assert!(matches!(result, Err(ClientError::TimedOut)), "{result:?}");
        assert!(started.elapsed() < Duration::from_secs(2));
        drop(holder.join());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_response_is_read_back() {
        let dir = tempdir();
        let path = dir.join("echo.sock");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = Vec::new();
            reader.read_until(b'\n', &mut line).unwrap();
            reader
                .get_mut()
                .write_all(&Response::ok().to_line())
                .unwrap();
            line
        });

        let response = send(
            path.to_str().unwrap(),
            b"{\"op\":\"ping\"}\n".to_vec(),
            Duration::from_secs(10),
        )
        .unwrap();

        assert_eq!(response, Response::ok());
        assert_eq!(server.join().unwrap(), b"{\"op\":\"ping\"}\n");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A directory of its own, short enough for a socket path.
    #[cfg(unix)]
    fn tempdir() -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNT: AtomicU32 = AtomicU32::new(0);
        let dir = std::path::PathBuf::from("/tmp").join(format!(
            "termx-client-{}-{}",
            std::process::id(),
            COUNT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
