//! Pseudo-terminal (PTY) handling.
//!
//! This module will own spawning shell processes attached to a PTY, resizing
//! them, and streaming bytes in both directions on every supported platform
//! (ConPTY on Windows, POSIX PTYs on Linux and macOS).
//!
//! It is intentionally empty for now.
