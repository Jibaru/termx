//! Tauri commands exposed to the frontend.
//!
//! Each feature area gets its own submodule. To add a command, write a
//! `#[tauri::command]` function in the relevant submodule (or a new one
//! declared below) and register it in the `generate_handler!` list in
//! `lib.rs`.

pub mod app;
