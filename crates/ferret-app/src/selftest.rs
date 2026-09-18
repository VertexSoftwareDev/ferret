//! Headless smoke test: `ferret-app.exe --selftest`.
//!
//! The window cannot be inspected from a script, but everything behind it can.
//! This drives the exact code the commands use — scan, build the search index,
//! run queries, render rows — against a real volume and then checks the result
//! the only way that really counts: by asking Windows whether the paths Ferret
//! reconstructed actually exist.
//!
//! Useful as a release gate, and as the first thing to run when something looks
//! wrong on a machine that is not this one.

use std::time::Instant;

use ferret_core::journal;
use ferret_core::{scan_with, Query, ScanOptions, SearchIndex, SortBy};

use crate::commands::build_rows;
use crate::state::Volume;

/// Queries the smoke test runs: common, rare, path-scoped and no-hit.
const QUERIES: &[&str] = &[
    "a",
    "exe",
    "setup",
    r"windows\system32\kernel",
    "zzqqxxnope",
];

/// How many reconstructed paths to verify against the filesystem.
const PATHS_TO_VERIFY: usize = 200;

pub fn run() -> i32 {
    let Some(letter) = first_readable_volume() else {
        eprintln!("Error: no readable NTFS volume. Run as administrator.");
        return 2;
    };

    println!("Ferret self-test  ·  drive {letter}:");
    println!();

    let index = match scan_with(letter, ScanOptions::default()) {
        Ok(index) => index,
        Err(err) => {
            eprintln!("Error: the scan failed: {err}");
            return 2;
        }
    };

    let built = Instant::now();
    let search = SearchIndex::build(&index);
    let build_time = built.elapsed();
    let mut volume = Volume { index, search };
    let stats = volume.index.stats;

    println!("  files            : {}", stats.files);
    println!("  folders          : {}", stats.dirs);
    println!(
        "  scan             : {:.2} s",
        stats.total_time.as_secs_f64()
    );
    println!("  search index     : {:.2} s", build_time.as_secs_f64());
    println!(
        "  memory           : {}",
        ferret_core::human_size(volume.memory_bytes() as u64)
    );
    println!();

    let mut failures = 0;

    if stats.files == 0 {
        eprintln!("  FAILED: no files found at all");
        failures += 1;
    }

    println!("  {:<32} {:>10} {:>10}", "query", "results", "time");
    for text in QUERIES {
        let query = Query {
            text: (*text).to_string(),
            sort_by: SortBy::None,
            ..Default::default()
        };
        let started = Instant::now();
        let hits = volume.search.run(&volume.index, &query);
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        println!("  {:<32} {:>10} {:>8.2} ms", text, hits.len(), ms);

        // Anything under a tenth of a second still feels instant while typing.
        if ms > 100.0 {
            eprintln!("  FAILED: the query '{text}' is too slow ({ms:.1} ms)");
            failures += 1;
        }
    }
    println!();

    // The real end-to-end check: does every path Ferret built actually exist?
    let all = volume.search.run(
        &volume.index,
        &Query {
            text: String::new(),
            sort_by: SortBy::None,
            ..Default::default()
        },
    );
    let step = (all.len() / PATHS_TO_VERIFY).max(1);
    let sample: Vec<u32> = all
        .iter()
        .copied()
        .step_by(step)
        .take(PATHS_TO_VERIFY)
        .collect();
    let rows = build_rows(&volume, &sample, 0, sample.len());

    let mut checked = 0;
    let mut missing = Vec::new();
    for row in &rows {
        // symlink_metadata, so a broken link still counts as present.
        if std::fs::symlink_metadata(&row.path).is_ok() {
            checked += 1;
        } else {
            missing.push(row.path.clone());
        }
    }

    println!(
        "  path check       : {checked}/{} paths exist on disk",
        rows.len()
    );
    if !missing.is_empty() {
        // A handful can legitimately vanish between the scan and this check on
        // a live system; a large share means path reconstruction is broken.
        let ratio = missing.len() as f64 / rows.len().max(1) as f64;
        for path in missing.iter().take(5) {
            println!("      missing: {path}");
        }
        if ratio > 0.05 {
            eprintln!(
                "  FAILED: {:.0}% of the paths were not found",
                ratio * 100.0
            );
            failures += 1;
        }
    }

    // Row rendering must never produce a blank name or a path without a drive.
    for row in rows.iter().take(50) {
        if row.name.is_empty() || !row.path.contains(':') {
            eprintln!("  FAILED: malformed row: {row:?}");
            failures += 1;
            break;
        }
    }

    println!();
    failures += live_update_check(&mut volume);

    println!();
    if failures == 0 {
        println!("  RESULT: passed");
        0
    } else {
        println!("  RESULT: {failures} check(s) failed");
        1
    }
}

/// How many files the live-update check creates.
const LIVE_FILES: usize = 25;

/// Prove that the change journal actually keeps the index current.
///
/// Counting entries is not enough — a busy machine creates and deletes files
/// underneath the test. So this writes files whose names cannot collide with
/// anything else on the disk, then asks the *search index* for them: found
/// means the journal path works end to end, from `DeviceIoControl` through to a
/// query hit. The files are removed again, and their disappearance checked too.
fn live_update_check(volume: &mut crate::state::Volume) -> u32 {
    println!("  live updates");

    let letter = volume.index.letter;
    let Ok(raw) = ferret_core::Volume::open(letter) else {
        println!("      skipped: could not open the volume");
        return 0;
    };
    let mut cursor = match journal::cursor_at_end(&raw) {
        Ok(cursor) => cursor,
        Err(err) => {
            // A volume without a journal is a supported configuration.
            println!("      skipped: {err}");
            return 0;
        }
    };

    // A tag no other file on the volume can share.
    let tag = format!(
        "ferretselftest{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let dir = std::env::temp_dir().join(&tag);
    if std::fs::create_dir_all(&dir).is_err() {
        println!("      skipped: could not create a temporary folder");
        return 0;
    }

    for i in 0..LIVE_FILES {
        let _ = std::fs::write(dir.join(format!("{tag}_{i}.txt")), b"ferret");
    }

    // The files are `<tag>_N.txt` while their folder is plain `<tag>`, so
    // searching for the trailing underscore counts the files and not the folder
    // they sit in.
    let needle = format!("{tag}_");

    let created = drain_and_count(volume, &raw, &mut cursor, &needle);
    println!("      created and then found       : {created} / {LIVE_FILES}");

    for i in 0..LIVE_FILES {
        let _ = std::fs::remove_file(dir.join(format!("{tag}_{i}.txt")));
    }

    let remaining = drain_and_count(volume, &raw, &mut cursor, &needle);
    println!("      still indexed after deletion : {remaining}");

    let _ = std::fs::remove_dir_all(&dir);

    let mut failures = 0;
    if created < LIVE_FILES {
        eprintln!("      FAILED: only {created}/{LIVE_FILES} files reached the index");
        failures += 1;
    }
    if remaining > 0 {
        eprintln!("      FAILED: {remaining} deleted files are still indexed");
        failures += 1;
    }
    failures
}

/// Read the journal until it goes quiet, apply everything, and count how many
/// entries matching `tag` the search index now returns.
fn drain_and_count(
    volume: &mut crate::state::Volume,
    raw: &ferret_core::Volume,
    cursor: &mut journal::Cursor,
    needle: &str,
) -> usize {
    // Windows writes journal entries a moment after the call returns, so the
    // loop keeps asking until two consecutive reads come back empty.
    let mut quiet = 0;
    for _ in 0..40 {
        std::thread::sleep(std::time::Duration::from_millis(150));
        match journal::read(raw, cursor) {
            Ok(changes) if changes.is_empty() => {
                quiet += 1;
                if quiet >= 3 {
                    break;
                }
            }
            Ok(changes) => {
                quiet = 0;
                for change in &changes {
                    let stat = if change.is_delete() {
                        None
                    } else {
                        volume
                            .index
                            .by_record(change.parent)
                            .and_then(|(position, _)| volume.index.full_path(position))
                            .and_then(|parent| {
                                let path = format!("{parent}\\{}", change.name);
                                std::fs::symlink_metadata(&path).ok().map(|m| (m.len(), 0))
                            })
                    };
                    volume.index.apply_change(change, stat);
                }
            }
            Err(_) => break,
        }
    }

    volume.search = SearchIndex::build(&volume.index);
    volume
        .search
        .run(
            &volume.index,
            &Query {
                text: needle.to_string(),
                sort_by: SortBy::None,
                ..Default::default()
            },
        )
        .len()
}

/// First drive whose raw volume can actually be opened.
fn first_readable_volume() -> Option<char> {
    ('A'..='Z').find(|letter| {
        std::path::Path::new(&format!("{letter}:\\")).exists()
            && ferret_core::Volume::open(*letter).is_ok()
    })
}
