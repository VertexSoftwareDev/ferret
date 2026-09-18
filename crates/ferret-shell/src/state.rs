//! Shared application state: the indexes, and the last result set.
//!
//! Scanning a volume takes seconds and searching it takes milliseconds, so the
//! two live under one lock but are used very differently — the UI must never
//! wait on a scan to render, which is why every command that touches this does
//! so from a blocking task rather than the UI thread.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use ferret_core::{Index, SearchIndex};

/// One indexed volume.
pub struct Volume {
    pub index: Index,
    pub search: SearchIndex,
    /// Bumped every time the change journal alters the index.
    ///
    /// Rebuilding the search arena takes about 100 ms, which is too long to
    /// hold a write lock for while someone is typing. The watcher therefore
    /// builds the new arena under a *read* lock — searches carry on — and takes
    /// the write lock only to swap it in. This counter is how it notices that
    /// the index moved underneath it in the meantime.
    pub generation: u64,
}

impl Volume {
    pub fn memory_bytes(&self) -> usize {
        self.index.memory_bytes() + self.search.memory_bytes()
    }
}

/// The hit list of the last query, so that scrolling can page through results
/// without searching again.
#[derive(Default)]
pub struct LastResult {
    pub letter: Option<char>,
    pub hits: Vec<u32>,
}

#[derive(Default)]
pub struct Inner {
    pub volumes: HashMap<char, Volume>,
    pub last: LastResult,
}

/// Handle shared by the window, the engine and the watcher.
#[derive(Clone, Default)]
pub struct AppState(pub Arc<RwLock<Inner>>);

impl AppState {
    pub fn read(&self) -> std::sync::RwLockReadGuard<'_, Inner> {
        // A poisoned lock means a worker panicked mid-update. The index is
        // rebuildable from disk, so recovering the data beats killing the app.
        self.0.read().unwrap_or_else(|e| e.into_inner())
    }

    pub fn write(&self) -> std::sync::RwLockWriteGuard<'_, Inner> {
        self.0.write().unwrap_or_else(|e| e.into_inner())
    }
}
