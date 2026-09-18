//! Keeping the index true to the disk.
//!
//! A scan is a snapshot taken at one instant; a second later a download has
//! finished and a build has written ten thousand files. Rather than rescan,
//! Ferret tails the NTFS change journal, whose entries carry everything an
//! index entry needs: the record number, the parent, the name and the
//! attributes.
//!
//! One thread per indexed volume. It holds its own read-only volume handle so
//! that polling never contends with a search, and takes the state lock only for
//! the moment it applies a batch.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, UNIX_EPOCH};

use ferret_core::journal::{self, Change};
use ferret_core::{SearchIndex, Volume};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::state::AppState;

/// How often the journal is polled. Fast enough to feel immediate, slow enough
/// that an idle machine costs nothing measurable.
const POLL: Duration = Duration::from_millis(700);

/// Never apply more than this many changes in one cycle: a mass operation —
/// unpacking an archive, a Windows update — must not hold the lock for seconds.
const MAX_BATCH: usize = 4_000;

/// Rebuilding the search arena costs about 100 ms, so it is never done more
/// often than this even while changes keep arriving.
const REBUILD_EVERY: Duration = Duration::from_millis(1_500);

/// Bumped whenever a volume is rescanned, which retires any older watcher.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// What the front end is told after a batch of changes lands.
#[derive(Serialize, Clone)]
pub struct IndexUpdate {
    pub letter: String,
    pub applied: usize,
    pub files: u64,
    pub dirs: u64,
}

/// Retire every running watcher. Called before a rescan replaces an index.
pub fn retire_all() {
    GENERATION.fetch_add(1, Ordering::SeqCst);
}

/// Start tailing `letter`'s change journal.
///
/// Returns quietly when the volume has no journal — that is a normal
/// configuration, and the app simply keeps its snapshot until a manual refresh.
pub fn spawn(app: AppHandle, state: AppState, letter: char) {
    let generation = GENERATION.load(Ordering::SeqCst);

    std::thread::Builder::new()
        .name(format!("ferret-watch-{letter}"))
        .spawn(move || run(app, state, letter, generation))
        .ok();
}

fn run(app: AppHandle, state: AppState, letter: char, generation: u64) {
    let Ok(volume) = Volume::open(letter) else {
        return;
    };
    let Ok(mut cursor) = journal::cursor_at_end(&volume) else {
        // No journal on this volume: nothing to tail.
        return;
    };

    let mut last_rebuild = Instant::now();
    let mut arena_stale = false;

    loop {
        std::thread::sleep(POLL);

        // A rescan happened, or the app is shutting down.
        if GENERATION.load(Ordering::SeqCst) != generation {
            return;
        }

        // Drain whatever has accumulated: one read returns a single buffer, and
        // a busy moment can produce several.
        let mut batch: Vec<Change> = Vec::new();
        loop {
            match journal::read(&volume, &mut cursor) {
                Ok(changes) if changes.is_empty() => break,
                Ok(changes) => {
                    batch.extend(changes);
                    if batch.len() >= MAX_BATCH {
                        break;
                    }
                }
                Err(err) => {
                    // The journal was reset or wrapped past our position: the
                    // index can no longer be patched, so ask for a rescan.
                    let _ = app.emit("index-stale", err.to_string());
                    return;
                }
            }
        }

        let rebuild_due = last_rebuild.elapsed() >= REBUILD_EVERY;
        if batch.is_empty() && !(arena_stale && rebuild_due) {
            continue;
        }

        let Some(update) = apply(&state, letter, &batch, &mut arena_stale) else {
            continue;
        };

        // Rebuilding the arena takes about 100 ms. Doing it under the write
        // lock would stall whoever is typing, so it happens under a read lock
        // and only the swap takes the write lock.
        if arena_stale && rebuild_due {
            if rebuild_arena(&state, letter) {
                arena_stale = false;
            }
            last_rebuild = Instant::now();
        }

        if update.applied > 0 {
            let _ = app.emit("index-updated", update);
        }
    }
}

/// Fold a batch of journal entries into the index.
///
/// Returns `None` when the volume is no longer indexed, which means a rescan
/// replaced it and this watcher is about to be retired anyway.
fn apply(
    state: &AppState,
    letter: char,
    batch: &[Change],
    arena_stale: &mut bool,
) -> Option<IndexUpdate> {
    // A single save produces several entries for the same file — create, extend,
    // close. Only the last state matters, and collapsing them here saves both
    // work and a pile of filesystem calls.
    let mut latest: HashMap<u32, &Change> = HashMap::with_capacity(batch.len());
    let mut order: Vec<u32> = Vec::with_capacity(batch.len());
    for change in batch {
        if latest.insert(change.record, change).is_none() {
            order.push(change.record);
        }
    }

    let mut inner = state.write();
    let indexed = inner.volumes.get_mut(&letter)?;

    let mut applied = 0usize;
    for record in &order {
        let Some(change) = latest.get(record) else {
            continue;
        };

        // Size and time have to come through the filesystem, not the raw
        // volume, or a file written moments ago reads back as empty.
        let stat = if change.is_delete() {
            None
        } else {
            indexed
                .index
                .by_record(change.parent)
                .and_then(|(position, _)| indexed.index.full_path(position))
                .and_then(|parent| stat_file(&parent, &change.name))
        };

        let refresh = indexed.index.apply_change(change, stat);
        if refresh.changed {
            applied += 1;
        }
        if refresh.names_changed {
            *arena_stale = true;
        }
    }

    if applied > 0 {
        indexed.generation += 1;
    }

    let stats = indexed.index.stats;
    Some(IndexUpdate {
        letter: letter.to_string(),
        applied,
        files: stats.files,
        dirs: stats.dirs,
    })
}

/// Rebuild the search arena for `letter`, holding the write lock only to swap.
///
/// Returns whether the new arena was actually installed. It is thrown away when
/// the index moved while it was being built — a rebuilt arena is only valid for
/// the index it was built from, and the next cycle will simply try again.
fn rebuild_arena(state: &AppState, letter: char) -> bool {
    let (rebuilt, generation) = {
        let inner = state.read();
        let Some(volume) = inner.volumes.get(&letter) else {
            return false;
        };
        (SearchIndex::build(&volume.index), volume.generation)
    };

    let mut inner = state.write();
    let Some(volume) = inner.volumes.get_mut(&letter) else {
        return false;
    };
    if volume.generation != generation {
        return false;
    }
    volume.search = rebuilt;
    true
}

/// Size and modification time of a file, as `(bytes, FILETIME)`.
///
/// `None` when the file has already gone again, which is common: the journal
/// reports work that finished before Ferret got to look.
fn stat_file(parent: &str, name: &str) -> Option<(u64, u64)> {
    let path = format!("{parent}\\{name}");
    let meta = std::fs::symlink_metadata(&path).ok()?;

    let modified = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|since| (since.as_secs() + 11_644_473_600) * 10_000_000)
        .unwrap_or(0);

    Some((if meta.is_dir() { 0 } else { meta.len() }, modified))
}
