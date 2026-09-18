//! Ferret's engine: read an NTFS volume's master file table and turn it into a
//! searchable in-memory index.
//!
//! The whole point is speed. Windows Search crawls directories and maintains a
//! database; Ferret reads the filesystem's own table of contents in one pass,
//! which is why a million files take seconds rather than minutes.
//!
//! Everything in this crate is read-only. Ferret opens volumes for reading and
//! never writes a byte back.

#![cfg(windows)]

pub mod bytes;
pub mod journal;
pub mod mft;
pub mod record;
pub mod runs;
pub mod search;
pub mod terms;
pub mod volume;

pub use mft::{
    filetime_to_unix, scan, scan_with, scan_with_progress, Entry, Index, Progress, ScanOptions,
    ScanStats,
};
pub use search::{sort_hits, Filter, Hit, Query, SearchIndex, SortBy, SortOrder};
pub use volume::{human_size, Volume};
