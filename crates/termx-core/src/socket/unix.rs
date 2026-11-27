//! The socket on macOS and Linux: a Unix domain socket in a directory of
//! the user.

use std::fs;
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::net::UnixListener;
use tokio::sync::watch;

use super::{Shared, serve_connection};
use crate::diagnostics::tracing::{debug, warn};

/// The longest socket path that every system accepts (`sun_path` holds 104
/// bytes on macOS, 108 on Linux, terminator included).
const MAX_SOCKET_PATH: usize = 100;

/// The directory of the socket:
///
/// | Platform | Directory                                              |
/// | -------- | ------------------------------------------------------ |
/// | Linux    | `$XDG_RUNTIME_DIR/termx`, else `/tmp/termx-<uid>`       |
/// | macOS    | `$TMPDIR/termx` (a folder of the user), else `/tmp/termx-<uid>` |
///
/// The directory is created with mode `0700`, and only used if it belongs
/// to the user and nobody else can enter it.
#[must_use]
pub fn socket_dir() -> PathBuf {
    let variable = if cfg!(target_os = "macos") {
        "TMPDIR"
    } else {
        "XDG_RUNTIME_DIR"
    };
    std::env::var_os(variable)
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .map_or_else(fallback_dir, |dir| dir.join("termx"))
}

/// `/tmp/termx-<uid>`.
fn fallback_dir() -> PathBuf {
    PathBuf::from("/tmp").join(format!("termx-{}", rustix::process::geteuid().as_raw()))
}

/// Creates the socket of this process in `dir` (or [`socket_dir`]), after
/// removing the sockets of processes that are gone.
pub(super) fn bind(dir: Option<&Path>) -> io::Result<(UnixListener, PathBuf)> {
    let mut dir = dir.map_or_else(socket_dir, Path::to_owned);
    let name = format!("{}.sock", std::process::id());
    if dir.join(&name).as_os_str().len() > MAX_SOCKET_PATH {
        dir = fallback_dir();
    }
    prepare_dir(&dir)?;
    remove_stale(&dir);
    let path = dir.join(name);
    // Only a socket of an earlier process with the same number can be
    // there: this process has not made one yet.
    if fs::symlink_metadata(&path).is_ok() {
        fs::remove_file(&path)?;
    }
    let listener = std::os::unix::net::UnixListener::bind(&path)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    Ok((UnixListener::from_std(listener)?, path))
}

/// Makes sure `dir` exists, is a directory of the user, and that nobody
/// else can enter it.
fn prepare_dir(dir: &Path) -> io::Result<()> {
    match fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
    {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    // Not followed: a link planted by someone else is refused.
    let metadata = fs::symlink_metadata(dir)?;
    if !metadata.is_dir() || metadata.uid() != rustix::process::geteuid().as_raw() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "the directory of the socket does not belong to the user",
        ));
    }
    if metadata.mode() & 0o077 != 0 {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Removes the sockets that processes which are gone left in `dir`: those
/// of a termx that crashed.
fn remove_stale(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .and_then(|name| name.strip_suffix(".sock"))
            .and_then(|pid| pid.parse::<i32>().ok())
        else {
            continue;
        };
        if pid == i32::try_from(std::process::id()).unwrap_or(0) || is_alive(pid) {
            continue;
        }
        match fs::remove_file(entry.path()) {
            Ok(()) => debug!("socket: removed the socket of a termx that is gone"),
            Err(error) => debug!(error = %error, "socket: a stale socket could not be removed"),
        }
    }
}

/// Whether a process with this number exists.
fn is_alive(pid: i32) -> bool {
    let Some(pid) = rustix::process::Pid::from_raw(pid) else {
        return false;
    };
    match rustix::process::test_kill_process(pid) {
        Ok(()) => true,
        // It exists, and belongs to someone else.
        Err(error) => error == rustix::io::Errno::PERM,
    }
}

/// Removes the socket file.
pub(super) fn remove(path: &Path) {
    if let Err(error) = fs::remove_file(path)
        && error.kind() != io::ErrorKind::NotFound
    {
        warn!(error = %error, "the socket file could not be removed");
    }
}

/// Accepts connections until `stopped` says so.
pub(super) async fn serve(
    listener: UnixListener,
    shared: Arc<Shared>,
    mut stopped: watch::Receiver<bool>,
) {
    let uid = rustix::process::geteuid().as_raw();
    loop {
        // A place first: further connections wait in the backlog of the
        // socket.
        let permit = tokio::select! {
            permit = Arc::clone(&shared.permits).acquire_owned() => permit,
            _ = stopped.changed() => return,
        };
        let Ok(permit) = permit else { return };
        let accepted = tokio::select! {
            accepted = listener.accept() => accepted,
            _ = stopped.changed() => return,
        };
        let stream = match accepted {
            Ok((stream, _)) => stream,
            Err(error) => {
                debug!(error = %error, "socket: a connection could not be accepted");
                continue;
            }
        };
        if !stream.peer_cred().is_ok_and(|peer| peer.uid() == uid) {
            warn!("socket: a connection from another user was refused");
            continue;
        }
        let shared = Arc::clone(&shared);
        let mut stop = stopped.clone();
        tokio::spawn(async move {
            tokio::select! {
                () = serve_connection(stream, &shared) => {}
                _ = stop.changed() => {}
            }
            drop(permit);
        });
    }
}
