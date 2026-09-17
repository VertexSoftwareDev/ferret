//! Build script for the Ferret desktop app.
//!
//! No custom Windows manifest here on purpose. Ferret needs administrator
//! rights to read a raw volume, but demanding them in the manifest means a UAC
//! prompt before the window even appears — and tauri-build's own manifest and a
//! second one do not coexist. Instead the app starts unprivileged, detects that
//! it cannot open the volume, and offers to relaunch itself elevated. See
//! `commands::elevation`.

fn main() {
    tauri_build::build()
}
