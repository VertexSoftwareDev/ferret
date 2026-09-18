//! Ferret — instant file search for Windows.
//!
//! The native window: one executable, no web view, no browser engine behind it.
//! Every line of work lives in `ferret-core` and `ferret-shell`; what is here is
//! the wiring between those and egui.
//!
//! `ferret-app` wires the same two crates to a WebView2 surface instead. The two
//! exist side by side on purpose — same engine, same colours, same keyboard, two
//! ways of getting pixels on screen.

// No console window behind the app in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod i18n;
mod prefs;
mod theme;
mod worker;

use eframe::egui;

/// 1180×720 fits a path column wide enough to be read; the minimum is the point
/// below which the toolbar starts to fold.
const INITIAL_SIZE: [f32; 2] = [1180.0, 720.0];
const MINIMUM_SIZE: [f32; 2] = [720.0, 420.0];

fn main() -> eframe::Result {
    // A headless pass over the real pipeline, for release gating and for
    // diagnosing a machine where the window itself cannot be inspected.
    if std::env::args().any(|arg| arg == "--selftest") {
        std::process::exit(ferret_shell::selftest::run());
    }

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Ferret")
        .with_inner_size(INITIAL_SIZE)
        .with_min_inner_size(MINIMUM_SIZE)
        // On a small or scaled display the preferred size does not fit, and a
        // window hanging off the bottom of the screen takes the status bar with
        // it.
        .with_clamp_size_to_monitor_size(true)
        .with_app_id("dev.ferret.search");

    if let Some(icon) = icon() {
        viewport = viewport.with_icon(icon);
    }

    eframe::run_native(
        "Ferret",
        eframe::NativeOptions {
            viewport,
            // Where a tool like this belongs when it opens. Left to the window
            // manager it lands wherever the last thing did, which on this
            // machine was half off the screen.
            centered: true,
            ..Default::default()
        },
        Box::new(|cc| Ok(Box::new(app::Ferret::new(cc)))),
    )
}

/// The taskbar and title-bar icon, decoded from the same PNG the installer uses.
///
/// A missing or unreadable icon is not worth failing to start over: Windows
/// falls back to its default and everything else works.
fn icon() -> Option<egui::IconData> {
    const PNG: &[u8] = include_bytes!("../../../icons/256x256.png");

    let decoded = image::load_from_memory(PNG).ok()?.into_rgba8();
    let (width, height) = decoded.dimensions();
    Some(egui::IconData {
        rgba: decoded.into_raw(),
        width,
        height,
    })
}
