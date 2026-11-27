//! Gives the programs of the terminals the default handling of Ctrl+C
//! (Windows).
//!
//! A process can be started with Ctrl+C ignored: `CreateProcess` with
//! `CREATE_NEW_PROCESS_GROUP` does it, and so do CI runners, some launchers
//! and `start /b`. That disposition is an attribute of the process, and
//! every process it starts inherits it, through the pseudo-console too. A
//! shell started by such a termx would then ignore Ctrl+C, and so would
//! every program run in it: Ctrl+C would interrupt nothing.
//!
//! `SetConsoleCtrlHandler(NULL, FALSE)` gives the calling process the
//! default handling back, and with it every process it starts from then
//! on: "This attribute of ignoring or processing CTRL+C is inherited by
//! child processes" (the documentation of `SetConsoleCtrlHandler`). termx
//! has no console handler of its own, so nothing else changes for it. It
//! is done once, before the first terminal starts.

#![allow(unsafe_code)]

use std::sync::Once;

use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;

use crate::diagnostics::tracing::warn;

/// Restores the default handling of Ctrl+C for this process, once. A
/// failure is logged: terminals still start, Ctrl+C may then not work in
/// them.
pub(super) fn restore_default_handling() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // SAFETY: no handler routine is passed; the call only changes an
        // attribute of the process.
        let restored = unsafe { SetConsoleCtrlHandler(None, 0) };
        if restored == 0 {
            let error = std::io::Error::last_os_error();
            warn!(error = %error, "the default handling of Ctrl+C could not be restored");
        }
    });
}
