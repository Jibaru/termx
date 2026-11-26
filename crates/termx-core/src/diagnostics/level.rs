//! Log verbosity.

use std::fmt;
use std::str::FromStr;

use tracing_subscriber::filter::LevelFilter;

/// How much is written to the log.
///
/// Each level includes the ones before it: `Info` also records warnings and
/// errors. The default is [`LogLevel::Info`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum LogLevel {
    /// Nothing is recorded, except panics.
    Off,
    /// Failures only.
    Error,
    /// Failures and unexpected but recoverable situations.
    Warn,
    /// Normal lifecycle events as well. The default.
    #[default]
    Info,
    /// Details useful while investigating a problem.
    Debug,
    /// Everything, including very frequent events.
    Trace,
}

impl LogLevel {
    /// Environment variable that selects the level, for example
    /// `TERMX_LOG=debug`.
    pub const ENV_VAR: &'static str = "TERMX_LOG";

    /// Reads the level from [`LogLevel::ENV_VAR`].
    ///
    /// An unset, empty or unrecognised value gives the default level.
    #[must_use]
    pub fn from_env() -> Self {
        Self::parse_or_default(std::env::var(Self::ENV_VAR).ok().as_deref())
    }

    /// Parses an optional setting, falling back to the default level when it
    /// is missing or not a level name.
    ///
    /// This is the entry point for any source of the setting: the environment
    /// variable today, the configuration file later.
    #[must_use]
    pub fn parse_or_default(value: Option<&str>) -> Self {
        value
            .and_then(|value| value.parse().ok())
            .unwrap_or_default()
    }

    /// Lower-case name of the level, as accepted by [`FromStr`].
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
        }
    }

    pub(crate) const fn filter(self) -> LevelFilter {
        match self {
            Self::Off => LevelFilter::OFF,
            Self::Error => LevelFilter::ERROR,
            Self::Warn => LevelFilter::WARN,
            Self::Info => LevelFilter::INFO,
            Self::Debug => LevelFilter::DEBUG,
            Self::Trace => LevelFilter::TRACE,
        }
    }
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Returned when a string is not the name of a [`LogLevel`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseLogLevelError(String);

impl fmt::Display for ParseLogLevelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unknown log level {:?}: expected off, error, warn, info, debug or trace",
            self.0
        )
    }
}

impl std::error::Error for ParseLogLevelError {}

impl FromStr for LogLevel {
    type Err = ParseLogLevelError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" | "none" => Ok(Self::Off),
            "error" => Ok(Self::Error),
            "warn" | "warning" => Ok(Self::Warn),
            "info" => Ok(Self::Info),
            "debug" => Ok(Self::Debug),
            "trace" => Ok(Self::Trace),
            _ => Err(ParseLogLevelError(value.to_owned())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::LogLevel;

    #[test]
    fn the_default_level_is_info() {
        assert_eq!(LogLevel::default(), LogLevel::Info);
        assert_eq!(LogLevel::parse_or_default(None), LogLevel::Info);
    }

    #[test]
    fn level_names_are_parsed_case_insensitively() {
        assert_eq!("debug".parse(), Ok(LogLevel::Debug));
        assert_eq!(" TRACE ".parse(), Ok(LogLevel::Trace));
        assert_eq!("Warning".parse(), Ok(LogLevel::Warn));
        assert_eq!("off".parse(), Ok(LogLevel::Off));
    }

    #[test]
    fn every_level_round_trips_through_its_name() {
        for level in [
            LogLevel::Off,
            LogLevel::Error,
            LogLevel::Warn,
            LogLevel::Info,
            LogLevel::Debug,
            LogLevel::Trace,
        ] {
            assert_eq!(level.as_str().parse(), Ok(level));
        }
    }

    #[test]
    fn an_unknown_value_falls_back_to_the_default() {
        assert!("verbose".parse::<LogLevel>().is_err());
        assert_eq!(LogLevel::parse_or_default(Some("verbose")), LogLevel::Info);
        assert_eq!(LogLevel::parse_or_default(Some("")), LogLevel::Info);
        assert_eq!(LogLevel::parse_or_default(Some("error")), LogLevel::Error);
    }
}
