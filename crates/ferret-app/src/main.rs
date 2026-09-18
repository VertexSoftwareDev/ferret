//! Ferret — instant file search for Windows.
//!
//! The web-view window. Every line of work lives in `ferret-core` and
//! `ferret-shell`; what is here is the wiring between those and a WebView2
//! surface. The native window in `ferret-native` wires the same two crates to
//! a different surface.

// No console window behind the app in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;

use ferret_shell::AppState;

fn main() {
    // A headless pass over the real pipeline, for release gating and for
    // diagnosing a machine where the window itself cannot be inspected.
    if std::env::args().any(|arg| arg == "--selftest") {
        std::process::exit(ferret_shell::selftest::run());
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
        .expect("could not start the Ferret window");
}
