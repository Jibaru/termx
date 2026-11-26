//! Resolution of the user's default shell.

use std::path::PathBuf;

/// Returns the shell a new terminal should run when the user has not
/// configured one.
///
/// - **macOS and Linux**: the user's login shell. `$SHELL` is used when it
///   points to an executable; otherwise the shell recorded in the password
///   database is used, and `/bin/sh` is the last resort.
/// - **Windows**: PowerShell 7 (`pwsh.exe`) when it is found on `PATH`,
///   otherwise Windows PowerShell (`powershell.exe`), which ships with every
///   supported version of Windows.
///
/// Use [`PtyCommand::default_shell`](super::PtyCommand::default_shell) to get
/// a ready-to-spawn command for it.
#[must_use]
pub fn default_shell() -> PathBuf {
    #[cfg(unix)]
    {
        // portable-pty implements exactly the lookup described above, and
        // keeps the unsafe password database access out of this crate.
        PathBuf::from(portable_pty::CommandBuilder::new_default_prog().get_shell())
    }
    #[cfg(windows)]
    {
        windows_shell(std::env::var_os("PATH").as_deref())
    }
}

/// File name of PowerShell 7 and later.
#[cfg(any(windows, test))]
const PWSH: &str = "pwsh.exe";

/// File name of Windows PowerShell 5, present on every Windows installation.
#[cfg(any(windows, test))]
const WINDOWS_POWERSHELL: &str = "powershell.exe";

/// Picks the Windows default shell given the value of `PATH`.
///
/// Compiled on every platform when testing, so the selection logic is
/// exercised by the test suite on all of them.
#[cfg(any(windows, test))]
fn windows_shell(path: Option<&std::ffi::OsStr>) -> PathBuf {
    path.and_then(|path| {
        std::env::split_paths(path)
            .filter(|dir| !dir.as_os_str().is_empty())
            .map(|dir| dir.join(PWSH))
            .find(|candidate| candidate.is_file())
    })
    .unwrap_or_else(|| PathBuf::from(WINDOWS_POWERSHELL))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    /// Creates an empty, uniquely named directory for one test.
    fn scratch_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("termx-shell-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn default_shell_is_not_empty() {
        assert!(!default_shell().as_os_str().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn default_shell_is_an_existing_absolute_path_on_unix() {
        let shell = default_shell();
        assert!(shell.is_absolute(), "{shell:?} is not absolute");
        assert!(shell.is_file(), "{shell:?} is not a file");
    }

    #[cfg(windows)]
    #[test]
    fn default_shell_is_a_powershell_on_windows() {
        let shell = default_shell();
        let name = shell.file_name().unwrap().to_string_lossy().to_lowercase();
        assert!(
            name == PWSH || name == WINDOWS_POWERSHELL,
            "unexpected default shell {shell:?}"
        );
    }

    #[test]
    fn windows_shell_prefers_pwsh_found_on_path() {
        let empty = scratch_dir("prefers-empty");
        let with_pwsh = scratch_dir("prefers-pwsh");
        fs::write(with_pwsh.join(PWSH), b"").unwrap();

        let path = std::env::join_paths([&empty, &with_pwsh]).unwrap();
        assert_eq!(windows_shell(Some(&path)), with_pwsh.join(PWSH));

        fs::remove_dir_all(empty).unwrap();
        fs::remove_dir_all(with_pwsh).unwrap();
    }

    #[test]
    fn windows_shell_falls_back_to_windows_powershell() {
        let empty = scratch_dir("fallback");
        let path = std::env::join_paths([&empty]).unwrap();

        assert_eq!(
            windows_shell(Some(&path)),
            Path::new(WINDOWS_POWERSHELL),
            "pwsh.exe is not in the given PATH"
        );
        assert_eq!(windows_shell(None), Path::new(WINDOWS_POWERSHELL));
        assert_eq!(
            windows_shell(Some(std::ffi::OsStr::new(""))),
            Path::new(WINDOWS_POWERSHELL)
        );

        fs::remove_dir_all(empty).unwrap();
    }

    #[test]
    fn windows_shell_ignores_a_directory_named_like_pwsh() {
        let dir = scratch_dir("directory");
        fs::create_dir(dir.join(PWSH)).unwrap();
        let path = std::env::join_paths([&dir]).unwrap();

        assert_eq!(windows_shell(Some(&path)), Path::new(WINDOWS_POWERSHELL));

        fs::remove_dir_all(dir).unwrap();
    }
}
