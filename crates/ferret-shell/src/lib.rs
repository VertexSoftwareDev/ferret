//! The part of Ferret that both front ends share.
//!
//! Ferret has two windows: a web view (`ferret-app`) and a native one
//! (`ferret-native`). Neither of them should own a line of NTFS logic, and
//! neither should own its own copy of the change-journal rules — the moment
//! those diverge, one window starts showing a truth the other does not.
//!
//! So everything between the engine and the pixels lives here: the indexes and
//! the lock around them, the queries, the row formatting, and the watcher that
//! keeps the index true to the disk. What is left in a front end is the window.
//!
//! Two shapes recur and are worth knowing up front.
//!
//! *Errors are codes, not sentences.* A failure crosses this boundary as
//! `code` or `code:detail`. Both windows can be switched between Turkish and
//! English at any moment, and a message written in Rust would be stuck in
//! whichever language it was written in.
//!
//! *Nothing here is async.* Every call blocks for as long as it takes, and the
//! caller decides which thread pays. The web view hands them to a blocking
//! task; the native window hands them to a worker thread. Both arrangements
//! want the same plain function.

#![cfg(windows)]

pub mod display;
pub mod engine;
pub mod selftest;
pub mod state;
pub mod watch;

/// Re-exported so a front end needs only this crate: scanning takes options,
/// and asking for them should not mean depending on the engine directly.
pub use ferret_core::ScanOptions;
pub use engine::{
    is_elevated, list_volumes, open_path, page, restart_elevated, reveal_path, scan, search,
    Row, ScanProgress, SearchArgs, SearchResponse, VolumeInfo,
};
pub use state::{AppState, Inner, LastResult, Volume};
pub use watch::{IndexUpdate, WatchEvent};
