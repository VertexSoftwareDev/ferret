//! Development harness for the Ferret engine.
//!
//! This is not the product — the product is the desktop app. It exists so the
//! NTFS scanner and the search index can be exercised and timed against a real
//! disk, which needs an elevated terminal.
//!
//!     ferret scan  C
//!     ferret find  C report
//!     ferret bench C

use std::io;
use std::process::ExitCode;
use std::time::Instant;

use ferret_core::{human_size, scan_with, Index, ScanOptions, SearchIndex};

/// Queries the benchmark runs; a mix of common, rare and no-hit needles.
const BENCH_QUERIES: &[&str] = &["a", "e", "exe", "dll", "report", "setup", "zzqqxx"];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map(String::as_str).unwrap_or("scan");

    let include_system = args.iter().any(|a| a == "--system");
    let positional: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let options = ScanOptions { include_system };

    let result = match command {
        "scan" => cmd_scan(drive_letter(positional.get(1)), options),
        "bench" => cmd_bench(drive_letter(positional.get(1)), options),
        "find" => {
            let letter = drive_letter(positional.get(1));
            match positional.get(2) {
                Some(needle) => cmd_find(letter, needle, options),
                None => {
                    eprintln!("usage: ferret find <drive> <text>");
                    return ExitCode::from(2);
                }
            }
        }
        "help" | "--help" | "-h" => {
            print_usage();
            return ExitCode::SUCCESS;
        }
        other => {
            eprintln!("unknown command: {other}");
            print_usage();
            return ExitCode::from(2);
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            report_error(&err);
            ExitCode::FAILURE
        }
    }
}

fn print_usage() {
    println!("Ferret — NTFS file index (development harness)");
    println!();
    println!("  ferret scan  [drive]          Index a volume and summarise it");
    println!("  ferret find  [drive] <text>   Index, then search file names");
    println!("  ferret bench [drive]          Measure query times");
    println!();
    println!("  --system                      Include NTFS's own metafiles");
    println!();
    println!("Defaults to C. Administrator rights are required.");
}

fn drive_letter(arg: Option<&&String>) -> char {
    arg.and_then(|s| s.chars().next()).unwrap_or('C')
}

fn cmd_scan(letter: char, options: ScanOptions) -> io::Result<()> {
    let index = scan_with(letter, options)?;
    print_summary(&index);

    let built = Instant::now();
    let search = SearchIndex::build(&index);
    println!(
        "search index    : {} (built in {:.2} s)",
        human_size(search.memory_bytes() as u64),
        built.elapsed().as_secs_f64()
    );

    println!();
    println!("Sample paths:");
    for (position, entry) in index
        .entries()
        .iter()
        .enumerate()
        .filter(|(_, e)| !e.is_dir())
        .take(5)
    {
        match index.full_path(position) {
            Some(path) => println!("  {path}  ({})", human_size(entry.size)),
            None => println!("  <no path> {}", index.name(position)),
        }
    }

    Ok(())
}

fn cmd_find(letter: char, needle: &str, options: ScanOptions) -> io::Result<()> {
    let index = scan_with(letter, options)?;
    print_summary(&index);
    let search = SearchIndex::build(&index);

    let started = Instant::now();
    let hits = search.search(needle);
    let elapsed = started.elapsed();

    println!();
    println!(
        "{} results for \"{needle}\", filtered in {:.2} ms",
        hits.len(),
        elapsed.as_secs_f64() * 1000.0
    );
    for hit in hits.iter().take(20) {
        let entry = &index.entries()[*hit as usize];
        match index.full_path(*hit as usize) {
            Some(path) if entry.is_dir() => println!("  [dir] {path}"),
            Some(path) => println!("  {path}  ({})", human_size(entry.size)),
            None => println!("  <no path> {}", index.name(*hit as usize)),
        }
    }
    if hits.len() > 20 {
        println!("  … and {} more", hits.len() - 20);
    }

    Ok(())
}

fn cmd_bench(letter: char, options: ScanOptions) -> io::Result<()> {
    let index = scan_with(letter, options)?;
    print_summary(&index);

    let started = Instant::now();
    let search = SearchIndex::build(&index);
    let build_time = started.elapsed();

    println!(
        "search index    : {} ({:.2} s)",
        human_size(search.memory_bytes() as u64),
        build_time.as_secs_f64()
    );
    println!();
    println!("query        results        best      average");
    println!("-------------------------------------------------");

    for query in BENCH_QUERIES {
        // Several runs: the first pulls the arena into cache, the later ones
        // show the speed someone typing in the box would actually feel.
        let mut best = f64::MAX;
        let mut total = 0.0;
        let mut hits = 0usize;
        const ROUNDS: u32 = 5;

        for _ in 0..ROUNDS {
            let started = Instant::now();
            let found = search.search(query);
            let ms = started.elapsed().as_secs_f64() * 1000.0;
            hits = found.len();
            best = best.min(ms);
            total += ms;
        }

        println!(
            "{:<10}  {:>9}   {:>7.2} ms   {:>7.2} ms",
            query,
            hits,
            best,
            total / ROUNDS as f64
        );
    }

    Ok(())
}

fn print_summary(index: &Index) {
    let s = &index.stats;
    println!("drive           : {}:", index.letter);
    println!("MFT size        : {}", human_size(s.mft_bytes));
    println!("records scanned : {}", s.records_total);
    println!("in use          : {}", s.records_in_use);
    println!("  files         : {}", s.files);
    println!("  folders       : {}", s.dirs);
    if s.records_system > 0 {
        println!("  (metafiles    : {})", s.records_system);
    }
    if s.records_orphaned > 0 {
        println!("  (no path      : {})", s.records_orphaned);
    }
    if s.records_damaged > 0 {
        println!("damaged, skipped: {}", s.records_damaged);
    }
    println!(
        "memory          : {}",
        human_size(index.memory_bytes() as u64)
    );
    println!("disk read       : {:.2} s", s.read_time.as_secs_f64());
    println!("scan            : {:.2} s", s.total_time.as_secs_f64());
    if s.total_time.as_secs_f64() > 0.0 {
        let per_sec = s.records_total as f64 / s.total_time.as_secs_f64();
        println!("rate            : {per_sec:.0} records/s");
    }
}

fn report_error(err: &io::Error) {
    eprintln!();
    if err.kind() == io::ErrorKind::PermissionDenied {
        eprintln!("Access denied.");
        eprintln!();
        eprintln!("Reading a raw volume requires administrator rights.");
        eprintln!("Open the terminal with \"Run as administrator\" and try again.");
    } else {
        eprintln!("Error: {err}");
    }
}
