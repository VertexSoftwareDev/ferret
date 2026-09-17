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
        eprintln!("HATA: okunabilir NTFS birimi yok. Yonetici olarak calistir.");
        return 2;
    };

    println!("Ferret kendi kendini test  ·  surucu {letter}:");
    println!();

    let index = match scan_with(letter, ScanOptions::default()) {
        Ok(index) => index,
        Err(err) => {
            eprintln!("HATA: tarama basarisiz: {err}");
            return 2;
        }
    };

    let built = Instant::now();
    let search = SearchIndex::build(&index);
    let build_time = built.elapsed();
    let volume = Volume { index, search };
    let stats = volume.index.stats;

    println!("  dosya            : {}", stats.files);
    println!("  klasor           : {}", stats.dirs);
    println!(
        "  tarama           : {:.2} sn",
        stats.total_time.as_secs_f64()
    );
    println!("  arama indeksi    : {:.2} sn", build_time.as_secs_f64());
    println!(
        "  bellek           : {}",
        ferret_core::human_size(volume.memory_bytes() as u64)
    );
    println!();

    let mut failures = 0;

    if stats.files == 0 {
        eprintln!("  BASARISIZ: hic dosya bulunamadi");
        failures += 1;
    }

    println!("  {:<32} {:>10} {:>10}", "sorgu", "sonuc", "sure");
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
            eprintln!("  BASARISIZ: '{text}' sorgusu cok yavas ({ms:.1} ms)");
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
        "  yol dogrulama    : {checked}/{} yol diskte bulundu",
        rows.len()
    );
    if !missing.is_empty() {
        // A handful can legitimately vanish between the scan and this check on
        // a live system; a large share means path reconstruction is broken.
        let ratio = missing.len() as f64 / rows.len().max(1) as f64;
        for path in missing.iter().take(5) {
            println!("      eksik: {path}");
        }
        if ratio > 0.05 {
            eprintln!("  BASARISIZ: yollarin %{:.0}'i bulunamadi", ratio * 100.0);
            failures += 1;
        }
    }

    // Row rendering must never produce a blank name or a path without a drive.
    for row in rows.iter().take(50) {
        if row.name.is_empty() || !row.path.contains(':') {
            eprintln!("  BASARISIZ: bozuk satir: {row:?}");
            failures += 1;
            break;
        }
    }

    println!();
    if failures == 0 {
        println!("  SONUC: gecti");
        0
    } else {
        println!("  SONUC: {failures} kontrol basarisiz");
        1
    }
}

/// First drive whose raw volume can actually be opened.
fn first_readable_volume() -> Option<char> {
    ('A'..='Z').find(|letter| {
        std::path::Path::new(&format!("{letter}:\\")).exists()
            && ferret_core::Volume::open(*letter).is_ok()
    })
}
