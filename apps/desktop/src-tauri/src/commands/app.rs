//! Commands about the application itself.

use tauri::{AppHandle, Runtime};

/// Returns the version of the `termx-core` library the app was built with.
#[tauri::command]
#[must_use]
pub fn core_version() -> &'static str {
    termx_core::version()
}

/// Quits the application, which ends every session: the answer of the
/// window to `quit-requested` (see `crate::quit`). Sessions come back,
/// dormant, the next time termx starts.
#[tauri::command]
pub fn app_quit<R: Runtime>(app: AppHandle<R>) {
    crate::quit::quit(&app);
}

/// The user said no to quitting: the next close or quit asks again.
#[tauri::command]
pub fn app_quit_cancel(state: tauri::State<'_, crate::quit::QuitState>) {
    state.cancel();
}

#[cfg(test)]
mod tests {
    use super::core_version;

    #[test]
    fn core_version_reports_the_core_library_version() {
        assert_eq!(core_version(), termx_core::version());
    }
}
