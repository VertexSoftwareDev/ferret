//! The commands the web view calls.
//!
//! Two rules shape everything here. A scan takes seconds, so it runs on a
//! blocking task and reports progress through events. A search takes
//! milliseconds but still touches tens of megabytes, so it also runs off the UI
//! thread — and its hit list is kept, so scrolling pages through the result
//! without searching again.

use std::process::Command;

use ferret_core::{scan_with_progress, Filter, Query, ScanOptions, SearchIndex, SortBy, SortOrder};
use serde::{Deserialize, Serialize};
use tauri::{Emitter, State};

use crate::format;
use crate::state::{AppState, LastResult, Volume};

/// One row as the table renders it.
#[derive(Serialize, Debug)]
pub struct Row {
    pub name: String,
    pub path: String,
    pub folder: String,
    pub size: String,
    pub size_bytes: u64,
    pub modified: String,
    pub kind: String,
    pub is_dir: bool,
}

#[derive(Serialize, Debug)]
pub struct VolumeInfo {
    pub letter: String,
    pub indexed: bool,
    pub files: u64,
    pub dirs: u64,
    pub scan_seconds: f64,
    pub memory: String,
}

/// Sent to the window while a scan runs, so the wait has a bar rather than a
/// spinner. Throttled: a scan fires the callback dozens of times a second and
/// the web view has better things to do.
#[derive(Serialize, Clone)]
pub struct ScanProgress {
    pub letter: String,
    /// 0 to 100.
    pub percent: u8,
}

/// Progress events are emitted no more often than this.
const PROGRESS_EVERY: std::time::Duration = std::time::Duration::from_millis(120);

#[derive(Serialize, Debug)]
pub struct SearchResponse {
    pub total: usize,
    /// Combined size of every matching file, already formatted. Answers the
    /// question behind most size-filtered searches: how much is this costing me?
    pub total_size: String,
    pub took_ms: f64,
    pub rows: Vec<Row>,
    /// True when the sort was skipped because the result set was enormous.
    pub sort_skipped: bool,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SearchArgs {
    pub letter: String,
    pub text: String,
    #[serde(default)]
    pub sort_by: String,
    #[serde(default)]
    pub descending: bool,
    #[serde(default)]
    pub files_only: bool,
    #[serde(default)]
    pub dirs_only: bool,
    #[serde(default)]
    pub skip_hidden: bool,
    #[serde(default)]
    pub min_size: Option<u64>,
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Sorting a result set costs O(n log n) path walks. Past this many hits the
/// list is no longer something a person reads top-to-bottom, so the sort is
/// skipped and the UI says so rather than freezing for a second.
const SORT_LIMIT: usize = 200_000;

/// Drive letters that look like fixed NTFS volumes.
#[tauri::command]
pub async fn list_volumes(state: State<'_, AppState>) -> Result<Vec<VolumeInfo>, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let inner = state.read();
        let mut out = Vec::new();

        for letter in 'A'..='Z' {
            let root = format!("{letter}:\\");
            if !std::path::Path::new(&root).exists() {
                continue;
            }
            match inner.volumes.get(&letter) {
                Some(volume) => out.push(VolumeInfo {
                    letter: letter.to_string(),
                    indexed: true,
                    files: volume.index.stats.files,
                    dirs: volume.index.stats.dirs,
                    scan_seconds: volume.index.stats.total_time.as_secs_f64(),
                    memory: ferret_core::human_size(volume.memory_bytes() as u64),
                }),
                None => out.push(VolumeInfo {
                    letter: letter.to_string(),
                    indexed: false,
                    files: 0,
                    dirs: 0,
                    scan_seconds: 0.0,
                    memory: String::new(),
                }),
            }
        }
        Ok(out)
    })
    .await
    .map_err(|e| e.to_string())?
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

    let info = tauri::async_runtime::spawn_blocking(move || -> Result<VolumeInfo, String> {
        let mut last = std::time::Instant::now() - PROGRESS_EVERY;
        let index = scan_with_progress(letter, ScanOptions::default(), |progress| {
            if last.elapsed() < PROGRESS_EVERY {
                return;
            }
            last = std::time::Instant::now();
            let _ = reporter.emit(
                "scan-progress",
                ScanProgress {
                    letter: letter.to_string(),
                    percent: (progress.fraction() * 100.0).round() as u8,
                },
            );
        })
        .map_err(describe_io)?;
        let search = SearchIndex::build(&index);
        let volume = Volume {
            index,
            search,
            generation: 0,
        };

        let info = VolumeInfo {
            letter: letter.to_string(),
            indexed: true,
            files: volume.index.stats.files,
            dirs: volume.index.stats.dirs,
            scan_seconds: volume.index.stats.total_time.as_secs_f64(),
            memory: ferret_core::human_size(volume.memory_bytes() as u64),
        };

        let mut inner = state.write();
        inner.volumes.insert(letter, volume);
        // Any cached result belonged to the old index.
        inner.last = LastResult::default();
        Ok(info)
    })
    .await
    .map_err(|e| e.to_string())??;

    // Any watcher from a previous scan is now looking at a replaced index.
    crate::watcher::retire_all();
    if let Some(letter) = info.letter.chars().next() {
        crate::watcher::spawn(app.clone(), for_watcher, letter);
    }

    let _ = app.emit("scan-done", &info.letter);
    Ok(info)
}

/// Run a query and return its first page.
#[tauri::command]
pub async fn search(
    state: State<'_, AppState>,
    args: SearchArgs,
) -> Result<SearchResponse, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let Some(letter) = args.letter.chars().next() else {
            return Err("no_drive_letter".to_string());
        };
        let limit = args.limit.unwrap_or(200).min(2_000);

        let query = Query {
            text: args.text.clone(),
            filter: Filter {
                files_only: args.files_only,
                dirs_only: args.dirs_only,
                skip_hidden: args.skip_hidden,
                min_size: args.min_size,
                ..Default::default()
            },
            sort_by: parse_sort(&args.sort_by),
            order: if args.descending {
                SortOrder::Descending
            } else {
                SortOrder::Ascending
            },
        };

        let mut inner = state.write();
        let Some(volume) = inner.volumes.get(&letter) else {
            return Err(format!("not_indexed:{letter}"));
        };

        // Matching first, then sorting — so a query that returns half the disk
        // still answers instantly, just unsorted.
        let started = std::time::Instant::now();
        let mut sort_skipped = false;

        let hits = {
            let unsorted = Query {
                sort_by: SortBy::None,
                ..query.clone()
            };
            let mut hits = volume.search.run(&volume.index, &unsorted);
            if query.sort_by != SortBy::None {
                if hits.len() <= SORT_LIMIT {
                    ferret_core::sort_hits(&volume.index, &mut hits, query.sort_by, query.order);
                } else {
                    sort_skipped = true;
                }
            }
            hits
        };
        let took_ms = started.elapsed().as_secs_f64() * 1000.0;

        let rows = build_rows(volume, &hits, 0, limit);
        let total = hits.len();

        // Directories carry no size of their own, so counting them would
        // double the answer for anyone who left folders in the results.
        let entries = volume.index.entries();
        let total_bytes: u64 = hits
            .iter()
            .filter_map(|hit| entries.get(*hit as usize))
            .filter(|entry| !entry.is_dir())
            .map(|entry| entry.size)
            .sum();
        let total_size = ferret_core::human_size(total_bytes);

        inner.last = LastResult {
            letter: Some(letter),
            hits,
        };

        Ok(SearchResponse {
            total,
            total_size,
            took_ms,
            rows,
            sort_skipped,
        })
    })
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
    tauri::async_runtime::spawn_blocking(move || {
        let inner = state.read();
        let Some(letter) = inner.last.letter else {
            return Ok(Vec::new());
        };
        let Some(volume) = inner.volumes.get(&letter) else {
            return Ok(Vec::new());
        };
        Ok(build_rows(
            volume,
            &inner.last.hits,
            offset,
            limit.min(2_000),
        ))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Whether this process can actually read a raw volume.
///
/// Probed by opening one rather than inspecting the process token: raw volume
/// access is the single privilege Ferret needs, so asking the question directly
/// beats inferring it. Drives that are simply not NTFS are skipped rather than
/// mistaken for a permission problem.
#[tauri::command]
pub async fn is_elevated() -> bool {
    for letter in 'A'..='Z' {
        if !std::path::Path::new(&format!("{letter}:\\")).exists() {
            continue;
        }
        match ferret_core::Volume::open(letter) {
            Ok(_) => return true,
            Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => return false,
            Err(_) => continue,
        }
    }
    false
}

/// Relaunch Ferret with administrator rights and close this instance.
///
/// The UAC prompt is raised through PowerShell's `RunAs` verb, which avoids
/// taking a dependency on the Windows API crate for a single call.
#[tauri::command]
pub async fn restart_elevated(app: tauri::AppHandle) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    // Single quotes are PowerShell's literal string; an apostrophe inside the
    // path has to be doubled or it would end the string early.
    let quoted = exe.display().to_string().replace('\'', "''");

    Command::new("powershell")
        .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command"])
        .arg(format!("Start-Process -FilePath '{quoted}' -Verb RunAs"))
        .spawn()
        .map_err(|e| format!("restart_failed:{e}"))?;

    app.exit(0);
    Ok(())
}

/// Open a file or folder with its default handler.
#[tauri::command]
pub async fn open_path(path: String) -> Result<(), String> {
    // `explorer` is used rather than ShellExecute so that no extra Windows
    // crate is pulled in for two calls; it is the same shell open verb.
    Command::new("explorer")
        .arg(&path)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("open_failed:{e}"))
}

/// Open the containing folder with the file selected.
#[tauri::command]
pub async fn reveal_path(path: String) -> Result<(), String> {
    Command::new("explorer")
        .arg("/select,")
        .arg(&path)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("reveal_failed:{e}"))
}

pub(crate) fn build_rows(volume: &Volume, hits: &[u32], offset: usize, limit: usize) -> Vec<Row> {
    let end = offset.saturating_add(limit).min(hits.len());
    if offset >= end {
        return Vec::new();
    }

    hits[offset..end]
        .iter()
        .filter_map(|hit| {
            let position = *hit as usize;
            let entry = volume.index.entries().get(position)?;
            let name = volume.index.name(position);
            let path = volume.index.full_path(position)?;
            let folder = volume.index.parent_path(position).unwrap_or_default();

            Some(Row {
                name: name.to_string(),
                path,
                folder,
                size: format::size(entry.size, entry.is_dir()),
                size_bytes: entry.size,
                modified: format::timestamp(entry.modified),
                kind: format::kind(name, entry.is_dir()),
                is_dir: entry.is_dir(),
            })
        })
        .collect()
}

fn parse_sort(value: &str) -> SortBy {
    match value {
        "name" => SortBy::Name,
        "size" => SortBy::Size,
        "modified" => SortBy::Modified,
        "path" => SortBy::Path,
        _ => SortBy::None,
    }
}

/// Turn an io error into a code the front end can phrase in the user's own
/// language.
///
/// Errors cross the boundary as `code` or `code:detail` rather than as finished
/// sentences: the window can be switched between Turkish and English at any
/// moment, and a message baked in Rust would be stuck in whichever language it
/// was written in.
fn describe_io(err: std::io::Error) -> String {
    match err.kind() {
        std::io::ErrorKind::PermissionDenied => "needs_elevation".to_string(),
        std::io::ErrorKind::NotFound => "drive_not_found".to_string(),
        _ => format!("scan_failed:{err}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferret_core::testing::{dir, file, index_from_specs};
    use ferret_core::SearchIndex;

    /// A small volume: C:\Users\pc\ with one file in it and one folder.
    fn sample() -> Volume {
        let index = index_from_specs(vec![
            dir(20, ferret_core::mft::ROOT_RECORD, "Users"),
            dir(21, 20, "pc"),
            file(22, 21, "rapor.pdf"),
            dir(23, 21, "Belgeler"),
        ]);
        let search = SearchIndex::build(&index);
        Volume {
            index,
            search,
            generation: 0,
        }
    }

    #[test]
    fn a_row_carries_everything_the_table_shows() {
        let volume = sample();
        let rows = build_rows(&volume, &[2], 0, 10);

        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.name, "rapor.pdf");
        assert_eq!(row.path, r"C:\Users\pc\rapor.pdf");
        assert_eq!(row.folder, r"C:\Users\pc");
        assert_eq!(row.kind, "PDF");
        assert!(!row.is_dir);
    }

    #[test]
    fn a_folder_row_reports_no_size_and_no_type() {
        let volume = sample();
        let rows = build_rows(&volume, &[3], 0, 10);

        assert_eq!(rows[0].name, "Belgeler");
        assert_eq!(rows[0].path, r"C:\Users\pc\Belgeler");
        // Both are worded by the front end, which knows the language.
        assert_eq!(rows[0].size, "");
        assert_eq!(rows[0].kind, "");
        assert!(rows[0].is_dir);
    }

    #[test]
    fn paging_stays_inside_the_hit_list() {
        let volume = sample();
        let hits = [0u32, 1, 2, 3];

        assert_eq!(build_rows(&volume, &hits, 0, 2).len(), 2);
        assert_eq!(build_rows(&volume, &hits, 2, 10).len(), 2);
        // Past the end is empty, not a panic.
        assert!(build_rows(&volume, &hits, 4, 10).is_empty());
        assert!(build_rows(&volume, &hits, 99, 10).is_empty());
        assert!(build_rows(&volume, &hits, 0, 0).is_empty());
    }

    #[test]
    fn a_hit_pointing_nowhere_is_dropped_rather_than_panicking() {
        let volume = sample();
        // The journal can retire an entry between a search and a page request.
        assert!(build_rows(&volume, &[9999], 0, 10).is_empty());
    }

    #[test]
    fn sort_names_map_to_the_engine() {
        assert_eq!(parse_sort("name"), SortBy::Name);
        assert_eq!(parse_sort("size"), SortBy::Size);
        assert_eq!(parse_sort("modified"), SortBy::Modified);
        assert_eq!(parse_sort("path"), SortBy::Path);
        // Anything unknown, including the empty string, leaves scan order.
        assert_eq!(parse_sort(""), SortBy::None);
        assert_eq!(parse_sort("colour"), SortBy::None);
    }

    #[test]
    fn io_errors_become_codes_the_front_end_can_phrase() {
        use std::io::{Error, ErrorKind};

        assert_eq!(
            describe_io(Error::from(ErrorKind::PermissionDenied)),
            "needs_elevation"
        );
        assert_eq!(
            describe_io(Error::from(ErrorKind::NotFound)),
            "drive_not_found"
        );
        // Anything else keeps the operating system's own words after the code.
        let other = describe_io(Error::other("disk on fire"));
        assert!(other.starts_with("scan_failed:"), "{other}");
        assert!(other.contains("disk on fire"));
    }
}
