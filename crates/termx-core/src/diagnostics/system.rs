//! Facts about the application and the machine it runs on.
//!
//! This is what the startup record and the diagnostics bundle say about the
//! environment. It is deliberately limited to versions and platform names:
//! no host name, no user name, no paths, no hardware identifiers.

use serde::Serialize;

/// Versions and platform of a running termx.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SystemInfo {
    /// Version of the application.
    pub app_version: String,
    /// Version of the `termx-core` library.
    pub core_version: String,
    /// Operating system family: `linux`, `macos` or `windows`.
    pub os: String,
    /// Name and version of the operating system, such as `Ubuntu 24.04`.
    pub os_version: String,
    /// CPU architecture the application was built for, such as `aarch64`.
    pub arch: String,
    /// Version of the webview that renders the interface, when known.
    pub webview_version: Option<String>,
    /// Locale of the user interface, such as `en-US`, when known.
    pub locale: Option<String>,
    /// Whether this is a debug build.
    pub debug_build: bool,
}

impl SystemInfo {
    /// Collects the information for this process.
    ///
    /// The application layer supplies what the core cannot know: its own
    /// version and the version of the webview.
    #[must_use]
    pub fn collect(app_version: &str, webview_version: Option<String>) -> Self {
        let os = os_info::get();
        Self {
            app_version: app_version.to_owned(),
            core_version: crate::version().to_owned(),
            os: std::env::consts::OS.to_owned(),
            os_version: format!("{} {}", os.os_type(), os.version()),
            arch: std::env::consts::ARCH.to_owned(),
            webview_version,
            locale: sys_locale::get_locale(),
            debug_build: cfg!(debug_assertions),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SystemInfo;

    #[test]
    fn collect_reports_versions_and_platform() {
        let info = SystemInfo::collect("1.2.3", Some("131.0".to_owned()));

        assert_eq!(info.app_version, "1.2.3");
        assert_eq!(info.core_version, crate::version());
        assert_eq!(info.os, std::env::consts::OS);
        assert_eq!(info.arch, std::env::consts::ARCH);
        assert_eq!(info.webview_version.as_deref(), Some("131.0"));
        assert!(!info.os_version.trim().is_empty());
    }

    #[test]
    fn it_serialises_to_exactly_the_documented_keys() {
        let info = SystemInfo::collect("1.2.3", None);

        let json = serde_json::to_value(&info).unwrap();
        let mut keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();

        assert_eq!(
            keys,
            [
                "app_version",
                "arch",
                "core_version",
                "debug_build",
                "locale",
                "os",
                "os_version",
                "webview_version"
            ]
        );
    }
}
