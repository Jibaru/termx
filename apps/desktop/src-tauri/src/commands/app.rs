//! Commands about the application itself.

/// Returns the version of the `termx-core` library the app was built with.
#[tauri::command]
#[must_use]
pub fn core_version() -> &'static str {
    termx_core::version()
}

#[cfg(test)]
mod tests {
    use super::core_version;

    #[test]
    fn core_version_reports_the_core_library_version() {
        assert_eq!(core_version(), termx_core::version());
    }
}
