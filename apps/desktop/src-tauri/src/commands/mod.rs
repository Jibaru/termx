//! Tauri commands exposed to the frontend.
//!
//! Each feature area gets its own submodule. To add a command, write a
//! `#[tauri::command]` function in the relevant submodule (or a new one
//! declared below) and register it in the `generate_handler!` list in
//! `lib.rs`.
//!
//! A command that can fail or panic returns [`error::CommandResult`] and runs
//! its body inside [`error::logged`] (or [`error::logged_async`]), so that
//! every failure is logged exactly once and a panic becomes an error.

pub mod app;
pub mod diagnostics;
pub mod error;
pub mod layout;
pub mod session;
pub mod workspace;
