//! Ferret — instant file search for Windows.
//!
//! The desktop shell. All of the work lives in `ferret-core`; this crate wires
//! it to a web view and to the shell (open a file, show it in Explorer).

// No console window behind the app in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod format;
mod selftest;
mod state;

use state::AppState;

fn main() {
    // A headless pass over the real pipeline, for release gating and for
    // diagnosing a machine where the window itself cannot be inspected.
    if std::env::args().any(|arg| arg == "--selftest") {
        std::process::exit(selftest::run());
    }

    tauri::Builder::default()
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            commands::list_volumes,
            commands::scan_volume,
            commands::search,
            commands::page,
            commands::open_path,
            commands::reveal_path,
            commands::is_elevated,
            commands::restart_elevated,
        ])
        .run(tauri::generate_context!())
        .expect("Ferret penceresi baslatilamadi");
}
