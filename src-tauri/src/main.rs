//! suiteDFIR desktop app: the thin Tauri shell around `suitedfir-core` (ARCHITECTURE.md §5.2).
//!
//! - `commands`: the IPC commands of CONTRACTS.md §10 and §13.5; the work is in `ops`, as methods
//!   of `state::AppState`.
//! - `policy`: the path policy of ARCHITECTURE.md §9.
//! - `lifecycle`: startup (instance lock, temp sweep, app log, main window) and the quit guard.
//! - `lock`, `logger`, `host`, `opener`: the instance lock, the app log, the record's host and
//!   opening files in their default app.

// Release builds use the GUI subsystem on Windows, so no console window opens next to the app.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod host;
mod lifecycle;
mod lock;
mod logger;
mod opener;
mod ops;
mod policy;
mod state;

#[cfg(test)]
mod testing;

use std::process::ExitCode;

fn main() -> ExitCode {
    let app = tauri::Builder::default()
        // The only registered plugin (ARCHITECTURE.md D5): JS open/save dialogs, and the native
        // message dialogs of the shell.
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(commands::handler())
        .setup(lifecycle::setup)
        .build(tauri::generate_context!());
    match app {
        Ok(app) => {
            app.run(lifecycle::on_run_event);
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("suiteDFIR failed to start: {err}");
            ExitCode::FAILURE
        }
    }
}
