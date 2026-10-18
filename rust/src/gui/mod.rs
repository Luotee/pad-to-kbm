//! GUI layer. Tauri builder, IPC commands, system tray, file watcher.
//!
//! Phase 3 of the v0.2.0 rewrite adds the window. This module hosts
//! everything Tauri-related so the engine/mapper/safety modules stay
//! a pure Rust library that the CLI binary (`--cli`) can still use.
//!
//! All Tauri code is gated behind the `gui` cargo feature. To build
//! a binary with the GUI, pass `--features gui` to cargo.
//!
//! `commands` is always compiled so integration tests can call `*_impl`
//! helpers without enabling the full Tauri build. The `#[tauri::command]`
//! wrappers inside are individually gated on `cfg(feature = "gui")`.

pub mod file_watcher;

// `commands` is always public — the `*_impl` helpers are Tauri-free and
// used by integration tests. The `#[tauri::command]` wrappers inside are
// each gated on `cfg(feature = "gui")`.
pub mod commands;

#[cfg(feature = "gui")]
pub mod events;

#[cfg(feature = "gui")]
pub mod runtime;

#[cfg(feature = "gui")]
pub mod tray;

// Tauri-free (serde only), so tests can reach it without the gui feature.
pub mod ui_prefs;

// Whole-pixel window size for the drawn window (pure; tests reach it without the gui feature).
pub mod winscale;

#[cfg(feature = "gui")]
pub use runtime::{run, RunOptions};
