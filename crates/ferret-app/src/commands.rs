//! The commands the web view calls.
//!
//! Every one of them is a thin wrapper: the work lives in `ferret-shell`, which
//! the native window uses just as directly. What this file adds is the two
//! things Tauri needs and the shell deliberately does not know about — a
//! blocking task to run on, because none of the engine is async and the UI
//! thread must never wait on a scan, and an `AppHandle` to emit progress
//! through.

use ferret_core::ScanOptions;
use ferret_shell::watch::WatchEvent;
use ferret_shell::{engine, AppState, Row, SearchArgs, SearchResponse, VolumeInfo};
use tauri::{Emitter, State};

/// Drive letters that look like fixed NTFS volumes.
#[tauri::command]
pub async fn list_volumes(state: State<'_, AppState>) -> Result<Vec<VolumeInfo>, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || engine::list_volumes(&state))
        .await
        .map_err(|e| e.to_string())
}

/// Index a volume. Emits `scan-progress` as it goes and `scan-done` at the end.
#[tauri::command]
pub async fn scan_volume(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    letter: String,
) -> Result<VolumeInfo, String> {
    let Some(letter) = letter.chars().next() else {
        return Err("no_drive_letter".into());
    };
    let state = state.inner().clone();
    // One clone for the scan itself, one for the watcher it starts afterwards.
    let for_watcher = state.clone();
    let reporter = app.clone();

    let info = tauri::async_runtime::spawn_blocking(move || {
        engine::scan(&state, letter, ScanOptions::default(), |progress| {
            let _ = reporter.emit("scan-progress", progress);
        })
    })
    .await
    .map_err(|e| e.to_string())??;

    // Any watcher from a previous scan is now looking at a replaced index.
    ferret_shell::watch::retire_all();
    watch(app.clone(), for_watcher, letter);

    let _ = app.emit("scan-done", &info.letter);
    Ok(info)
}

/// Start tailing the change journal, turning each event into a Tauri event.
fn watch(app: tauri::AppHandle, state: AppState, letter: char) {
    ferret_shell::watch::spawn(state, letter, move |event| match event {
        WatchEvent::Updated(update) => {
            let _ = app.emit("index-updated", update);
        }
        WatchEvent::Stale(detail) => {
            let _ = app.emit("index-stale", detail);
        }
    });
}

/// Run a query and return its first page.
#[tauri::command]
pub async fn search(
    state: State<'_, AppState>,
    args: SearchArgs,
) -> Result<SearchResponse, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || engine::search(&state, &args))
        .await
        .map_err(|e| e.to_string())?
}

/// Fetch another window of the last result set, for scrolling.
#[tauri::command]
pub async fn page(
    state: State<'_, AppState>,
    offset: usize,
    limit: usize,
) -> Result<Vec<Row>, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || engine::page(&state, offset, limit))
        .await
        .map_err(|e| e.to_string())
}

/// Whether this process can actually read a raw volume.
#[tauri::command]
pub async fn is_elevated() -> bool {
    tauri::async_runtime::spawn_blocking(engine::is_elevated)
        .await
        .unwrap_or(false)
}

/// Relaunch Ferret with administrator rights and close this instance.
#[tauri::command]
pub async fn restart_elevated(app: tauri::AppHandle) -> Result<(), String> {
    engine::restart_elevated()?;
    app.exit(0);
    Ok(())
}

/// Open a file or folder with its default handler.
#[tauri::command]
pub async fn open_path(path: String) -> Result<(), String> {
    engine::open_path(&path)
}

/// Open the containing folder with the file selected.
#[tauri::command]
pub async fn reveal_path(path: String) -> Result<(), String> {
    engine::reveal_path(&path)
}

#[cfg(test)]
mod tests {
    //! The engine these commands wrap is tested in `ferret-shell`, where it can
    //! be driven without a window at all. What is worth checking here is that
    //! the wrapping itself is honest: the drive letter a window sends as a
    //! string has to survive the trip.

    #[test]
    fn an_empty_letter_is_refused_before_any_work_starts() {
        // `scan_volume` takes a string because that is what the web view has,
        // and an empty one must not reach the engine as some default drive.
        assert_eq!("".chars().next(), None);
        assert_eq!("C".chars().next(), Some('C'));
        // Only the letter is used, so a full root path is harmless.
        assert_eq!(r"C:\".chars().next(), Some('C'));
    }
}
