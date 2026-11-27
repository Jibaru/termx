//! Finds the process of Claude Code that ran a hook.
//!
//! termx uses it to notice that Claude Code is gone without having said
//! goodbye (killed, crashed): its agent state must not stay "working". A
//! hook is a child of Claude Code, directly when the hook runs without a
//! shell (the exec form of a hook command), or through a shell (`sh -c`,
//! Git Bash, PowerShell). So the answer is the closest ancestor that is
//! not a shell, looking at most [`MAX_HOPS`] processes up.
//!
//! This is a hint: a wrapper (`CLAUDE_CODE_SHELL_PREFIX`) can come between,
//! and its process ends with the hook. termx only trusts a process that
//! several hooks report (see `termx_core::workspace::agent`).

/// How many processes up the search goes.
pub const MAX_HOPS: usize = 4;

/// Programs that only run the hook on behalf of Claude Code.
const SHELLS: &[&str] = &[
    "sh",
    "bash",
    "zsh",
    "dash",
    "ksh",
    "fish",
    "ash",
    "busybox",
    "cmd",
    "powershell",
    "pwsh",
    "env",
];

/// The process of Claude Code that ran this one, if it can be told.
#[must_use]
pub fn claude_process() -> Option<u32> {
    let mut pid = parent_of(std::process::id())?.0;
    for _ in 0..MAX_HOPS {
        if pid <= 1 {
            return None;
        }
        let (parent, name) = parent_of(pid)?;
        if !is_shell(&name) {
            return Some(pid);
        }
        pid = parent;
    }
    None
}

/// Whether `name`, the name of a program with or without `.exe`, is one
/// of [`SHELLS`].
fn is_shell(name: &str) -> bool {
    let name = name.trim();
    let base = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(name)
        .trim_start_matches('-');
    let base = base
        .strip_suffix(".exe")
        .or_else(|| base.strip_suffix(".EXE"))
        .unwrap_or(base);
    SHELLS.iter().any(|shell| base.eq_ignore_ascii_case(shell))
}

/// The parent of `pid`, and the name of `pid`.
#[cfg(target_os = "linux")]
fn parent_of(pid: u32) -> Option<(u32, String)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // `pid (name) state ppid ...`; the name may contain spaces and
    // parentheses, so it ends at the last `)`.
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    let name = stat.get(open + 1..close)?.to_owned();
    let mut rest = stat.get(close + 1..)?.split_whitespace();
    let _state = rest.next()?;
    let parent = rest.next()?.parse().ok()?;
    Some((parent, name))
}

/// The parent of `pid`, and the name of `pid`.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn parent_of(pid: u32) -> Option<(u32, String)> {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = i32::try_from(std::mem::size_of::<libc::proc_bsdinfo>()).ok()?;
    // SAFETY: the buffer is as large as the size given, and the kernel
    // writes at most that much.
    let written = unsafe {
        libc::proc_pidinfo(
            i32::try_from(pid).ok()?,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if written != size {
        return None;
    }
    // SAFETY: the kernel filled the whole structure.
    let info = unsafe { info.assume_init() };
    let name: Vec<u8> = info
        .pbi_comm
        .iter()
        .take_while(|c| **c != 0)
        .map(|c| c.cast_unsigned())
        .collect();
    Some((info.pbi_ppid, String::from_utf8_lossy(&name).into_owned()))
}

/// The parent of `pid`, and the name of `pid`.
#[cfg(windows)]
#[allow(unsafe_code)]
fn parent_of(pid: u32) -> Option<(u32, String)> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };

    // SAFETY: plain call; the handle is closed below.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return None;
    }
    let mut entry = PROCESSENTRY32W {
        dwSize: u32::try_from(std::mem::size_of::<PROCESSENTRY32W>()).ok()?,
        // SAFETY: an all-zero PROCESSENTRY32W is valid.
        ..unsafe { std::mem::zeroed() }
    };
    let mut found = None;
    // SAFETY: `entry` is a valid PROCESSENTRY32W with its size set.
    let mut more = unsafe { Process32FirstW(snapshot, &raw mut entry) } != 0;
    while more {
        if entry.th32ProcessID == pid {
            let length = entry
                .szExeFile
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(entry.szExeFile.len());
            let name = String::from_utf16_lossy(&entry.szExeFile[..length]);
            found = Some((entry.th32ParentProcessID, name));
            break;
        }
        // SAFETY: as above.
        more = unsafe { Process32NextW(snapshot, &raw mut entry) } != 0;
    }
    // SAFETY: the snapshot handle is open and owned here.
    unsafe { CloseHandle(snapshot) };
    found
}

/// Other systems: unknown.
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn parent_of(_pid: u32) -> Option<(u32, String)> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shells_are_recognised_by_their_name() {
        for shell in [
            "sh",
            "bash",
            "-zsh",
            "/bin/bash",
            "bash.exe",
            r"C:\Program Files\Git\usr\bin\bash.exe",
            "PowerShell.EXE",
            "pwsh.exe",
            "cmd.exe",
        ] {
            assert!(is_shell(shell), "{shell}");
        }
        for program in ["claude", "node", "node.exe", "claude.exe", "bashful"] {
            assert!(!is_shell(program), "{program}");
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    #[test]
    fn the_parent_of_this_process_is_found() {
        let (parent, name) = parent_of(std::process::id()).unwrap();
        assert!(parent > 0);
        assert!(!name.is_empty());
    }
}
