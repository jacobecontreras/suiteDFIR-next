//! suiteDFIR desktop app: the thin Tauri shell around `suitedfir-core`.

// Release builds use the GUI subsystem on Windows, so no console window opens next to the app.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::process::ExitCode;

fn main() -> ExitCode {
    let result = tauri::Builder::default()
        // The only registered plugin (ARCHITECTURE.md D5): JS open/save dialogs.
        .plugin(tauri_plugin_dialog::init())
        .run(tauri::generate_context!());
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("suiteDFIR failed to start: {err}");
            ExitCode::FAILURE
        }
    }
}
