//! Full `$MFT` scan: walk every record on a volume and build a file index.
//!
//! The point of reading the MFT directly is that it is one sequential pass over
//! a table, instead of millions of directory-tree syscalls. Walking `C:\` with
//! the normal Windows APIs takes minutes; this takes seconds.
//!
//! `$MFT` is itself a file, so the scan bootstraps: read record 0, decode its
//! `$DATA` run list, and that gives the location of every other record.

use std::io;
use std::time::{Duration, Instant};

use crate::record::{self, Attribute, ATTR_DATA, ATTR_FILE_NAME};
use crate::runs::{parse_runs, Run};
use crate::volume::Volume;

/// MFT record number of the root directory. Its parent is itself.
pub const ROOT_RECORD: u32 = 5;

/// Records 0..16 are NTFS's own metafiles: `$MFT`, `$LogFile`, `$Bitmap` and
/// friends. They are real files, but nobody searching their disk means them.
const FIRST_USER_RECORD: u32 = 16;

/// Read this much of the MFT per volume read. Large enough that the syscall
/// overhead disappears, small enough to stay friendly to memory.
const CHUNK_BYTES: u64 = 4 * 1024 * 1024;

/// Marker for "no entry" in the record -> entry lookup table.
const NO_ENTRY: u32 = u32::MAX;

/// One indexed file or directory.
#[derive(Debug, Clone)]
pub struct Entry {
    pub record: u32,
    /// Record number of the containing directory.
    pub parent: u32,
    pub name: Box<str>,
    pub is_dir: bool,
    pub size: u64,
}

/// Knobs for [`scan_with`].
#[derive(Debug, Default, Clone, Copy)]
pub struct ScanOptions {
    /// Include NTFS's internal metafiles (`$MFT`, `$LogFile`, …).
    /// Off by default: nobody searching their disk means those.
    pub include_system: bool,
}

/// What a scan found, for reporting and benchmarking.
#[derive(Debug, Default, Clone, Copy)]
pub struct ScanStats {
    pub records_total: u64,
    pub records_in_use: u64,
    pub files: u64,
    pub dirs: u64,
    /// Records skipped because they failed the fixup or header checks.
    pub records_damaged: u64,
    /// Metafiles left out because `include_system` was false.
    pub records_system: u64,
    pub mft_bytes: u64,
    pub read_time: Duration,
    pub total_time: Duration,
}

/// An in-memory index of one volume.
///
/// Entries are stored densely — a 2 M record MFT is typically 10 % free space,
/// and the dense layout is what makes the search pass cache-friendly. The
/// sparse record numbers are mapped back through [`Index::by_record`].
pub struct Index {
    pub letter: char,
    entries: Vec<Entry>,
    /// `record number -> position in entries`, or [`NO_ENTRY`].
    by_record: Vec<u32>,
    pub stats: ScanStats,
}

impl Index {
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Look up an entry by its MFT record number.
    pub fn by_record(&self, record: u32) -> Option<&Entry> {
        let slot = *self.by_record.get(record as usize)?;
        if slot == NO_ENTRY {
            return None;
        }
        self.entries.get(slot as usize)
    }

    /// Build the full path of the entry at `position`, e.g.
    /// `C:\Users\pc\notes.txt`.
    ///
    /// Returns `None` when the chain to the root is broken — which happens for
    /// entries whose parent directory was skipped or is damaged.
    pub fn full_path(&self, position: usize) -> Option<String> {
        let entry = self.entries.get(position)?;

        let mut parts: Vec<&str> = Vec::with_capacity(8);
        parts.push(&entry.name);
        let mut current = entry.parent;

        // The root is its own parent, so the walk needs an explicit stop, and a
        // depth cap in case a damaged volume produces a cycle.
        for _ in 0..256 {
            if current == ROOT_RECORD {
                break;
            }
            let parent = self.by_record(current)?;
            parts.push(&parent.name);
            if parent.parent == current {
                break;
            }
            current = parent.parent;
        }

        let mut path = String::with_capacity(4 + parts.len() * 12);
        path.push(self.letter);
        path.push(':');
        for part in parts.iter().rev() {
            path.push('\\');
            path.push_str(part);
        }
        Some(path)
    }
}

/// Scan a volume with the default options.
pub fn scan(letter: char) -> io::Result<Index> {
    scan_with(letter, ScanOptions::default())
}

/// Scan a whole volume and return its index.
pub fn scan_with(letter: char, options: ScanOptions) -> io::Result<Index> {
    let started = Instant::now();
    let mut volume = Volume::open(letter)?;

    let runs = read_mft_runs(&mut volume)?;
    let mft_bytes = crate::runs::total_clusters(&runs) * volume.bytes_per_cluster;
    let record_size = volume.bytes_per_record as usize;
    if record_size == 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "kayit boyutu sifir"));
    }

    let estimated = (mft_bytes / record_size as u64) as usize;
    let mut entries: Vec<Entry> = Vec::with_capacity(estimated * 9 / 10);
    let mut by_record: Vec<u32> = vec![NO_ENTRY; estimated];

    let mut stats = ScanStats { mft_bytes, ..Default::default() };
    let sector = volume.bytes_per_sector as usize;

    // Round the chunk down to a whole number of records so a record never
    // straddles two chunks.
    let chunk_bytes = (CHUNK_BYTES - CHUNK_BYTES % record_size as u64).max(record_size as u64);
    let mut buffer = vec![0u8; chunk_bytes as usize];

    let mut record_number: u64 = 0;

    for run in &runs {
        let Some(lcn) = run.lcn else {
            // A sparse region inside $MFT: no records stored there.
            record_number += run.clusters * volume.bytes_per_cluster / record_size as u64;
            continue;
        };

        let run_bytes = run.clusters * volume.bytes_per_cluster;
        let mut consumed = 0u64;

        while consumed < run_bytes {
            let want = chunk_bytes.min(run_bytes - consumed);
            let slice = &mut buffer[..want as usize];

            let read_started = Instant::now();
            volume.read_at(lcn * volume.bytes_per_cluster + consumed, slice)?;
            stats.read_time += read_started.elapsed();

            for raw in slice.chunks_mut(record_size) {
                let number = record_number as u32;
                record_number += 1;
                stats.records_total += 1;

                match parse_entry(raw, sector, number) {
                    ParseOutcome::Live(entry) => {
                        stats.records_in_use += 1;

                        if !options.include_system && is_system_record(&entry) {
                            stats.records_system += 1;
                            continue;
                        }
                        if entry.is_dir {
                            stats.dirs += 1;
                        } else {
                            stats.files += 1;
                        }

                        if number as usize >= by_record.len() {
                            // The MFT grew past the estimate mid-scan.
                            by_record.resize(number as usize + 1, NO_ENTRY);
                        }
                        by_record[number as usize] = entries.len() as u32;
                        entries.push(entry);
                    }
                    ParseOutcome::Free => {}
                    ParseOutcome::Damaged => stats.records_damaged += 1,
                }
            }

            consumed += want;
        }
    }

    entries.shrink_to_fit();
    stats.total_time = started.elapsed();

    Ok(Index { letter: volume.letter, entries, by_record, stats })
}

/// NTFS metafiles: the low records, plus the `$…` entries sitting in the root.
fn is_system_record(entry: &Entry) -> bool {
    entry.record < FIRST_USER_RECORD
        || (entry.parent == ROOT_RECORD && entry.name.starts_with('$'))
}

enum ParseOutcome {
    Live(Entry),
    /// A record that is not in use, or one Ferret deliberately skips.
    Free,
    Damaged,
}

/// Turn one raw record into an [`Entry`].
fn parse_entry(raw: &mut [u8], bytes_per_sector: usize, number: u32) -> ParseOutcome {
    // An all-zero record is simply unused space in the MFT, not damage.
    if raw.iter().take(4).all(|b| *b == 0) {
        return ParseOutcome::Free;
    }
    if !record::apply_fixups(raw, bytes_per_sector) {
        return ParseOutcome::Damaged;
    }
    let Some(header) = record::parse_header(raw) else {
        return ParseOutcome::Damaged;
    };
    if !header.in_use() {
        return ParseOutcome::Free;
    }

    let mut best: Option<record::FileName> = None;
    let mut data_size: Option<u64> = None;

    for attr in record::attributes(raw, &header) {
        match attr.kind {
            ATTR_FILE_NAME => {
                if let Some(value) = attr.resident_value() {
                    if let Some(parsed) = record::parse_file_name(value) {
                        best = Some(choose_name(best, parsed));
                    }
                }
            }
            ATTR_DATA if attr.name_len == 0 => {
                // Only the unnamed $DATA stream is the file's own size; named
                // streams are alternate data streams.
                data_size = Some(data_stream_size(&attr));
            }
            _ => {}
        }
    }

    let Some(name) = best else {
        // Records without a $FILE_NAME are things like $MFT's own extents.
        return ParseOutcome::Free;
    };

    ParseOutcome::Live(Entry {
        record: number,
        parent: name.parent as u32,
        name: name.name.into_boxed_str(),
        is_dir: header.is_directory(),
        size: data_size.unwrap_or(name.real_size),
    })
}

/// Prefer the human-readable name over the 8.3 alias.
fn choose_name(current: Option<record::FileName>, candidate: record::FileName) -> record::FileName {
    match current {
        None => candidate,
        Some(existing) => {
            if !existing.namespace.is_preferred() && candidate.namespace.is_preferred() {
                candidate
            } else {
                existing
            }
        }
    }
}

fn data_stream_size(attr: &Attribute<'_>) -> u64 {
    if let Some(size) = attr.non_resident_size() {
        size
    } else {
        // A small file lives inside its own record; its size is the length of
        // the resident value.
        attr.resident_value().map(|v| v.len() as u64).unwrap_or(0)
    }
}

/// Read record 0 (`$MFT` itself) and decode the run list of its `$DATA`.
fn read_mft_runs(volume: &mut Volume) -> io::Result<Vec<Run>> {
    let record_size = volume.bytes_per_record as usize;
    let mut raw = vec![0u8; record_size];
    let offset = volume.mft_start_lcn * volume.bytes_per_cluster;
    volume.read_at(offset, &mut raw)?;

    let sector = volume.bytes_per_sector as usize;
    if !record::apply_fixups(&mut raw, sector) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "$MFT kayit 0 okunamadi (imza veya fixup hatali)",
        ));
    }
    let header = record::parse_header(&raw)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "$MFT kayit basligi bozuk"))?;

    for attr in record::attributes(&raw, &header) {
        if attr.kind == ATTR_DATA && attr.name_len == 0 {
            if let Some(list) = attr.run_list() {
                let runs = parse_runs(list);
                if !runs.is_empty() {
                    return Ok(runs);
                }
                break;
            }
        }
    }

    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "$MFT icinde $DATA calisma listesi bulunamadi",
    ))
}

/// Index construction helpers shared by the tests of several modules, so that
/// path walking and searching can be exercised without an actual disk.
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    pub fn entry(record: u32, parent: u32, name: &str, is_dir: bool) -> Entry {
        Entry { record, parent, name: name.into(), is_dir, size: 0 }
    }

    pub fn index_from_entries(entries: Vec<Entry>) -> Index {
        let max = entries.iter().map(|e| e.record).max().unwrap_or(0);
        let mut by_record = vec![NO_ENTRY; max as usize + 1];
        for (i, e) in entries.iter().enumerate() {
            by_record[e.record as usize] = i as u32;
        }
        Index { letter: 'C', entries, by_record, stats: ScanStats::default() }
    }

    /// Flat index of files, all sitting directly in the root.
    pub fn index_from_names(names: &[&str]) -> Index {
        let entries = names
            .iter()
            .enumerate()
            .map(|(i, name)| entry(FIRST_USER_RECORD + i as u32, ROOT_RECORD, name, false))
            .collect();
        index_from_entries(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{entry, index_from_entries as test_index};
    use super::*;

    #[test]
    fn builds_a_full_path_up_to_the_root() {
        let index = test_index(vec![
            entry(20, ROOT_RECORD, "Users", true),
            entry(21, 20, "pc", true),
            entry(22, 21, "notes.txt", false),
        ]);
        assert_eq!(index.full_path(2).as_deref(), Some(r"C:\Users\pc\notes.txt"));
        assert_eq!(index.full_path(0).as_deref(), Some(r"C:\Users"));
    }

    #[test]
    fn a_missing_parent_yields_no_path_instead_of_a_wrong_one() {
        // Parent 99 was never indexed.
        let index = test_index(vec![entry(22, 99, "orphan.txt", false)]);
        assert_eq!(index.full_path(0), None);
    }

    #[test]
    fn a_parent_cycle_terminates() {
        // Two directories claiming each other as parent: must not hang.
        let index = test_index(vec![
            entry(20, 21, "a", true),
            entry(21, 20, "b", true),
        ]);
        // Depth-capped, so it returns *something* rather than looping forever.
        let _ = index.full_path(0);
    }

    #[test]
    fn metafiles_are_recognised() {
        assert!(is_system_record(&entry(0, ROOT_RECORD, "$MFT", false)));
        assert!(is_system_record(&entry(11, ROOT_RECORD, "$Extend", true)));
        // A user file that merely starts with $ deeper in the tree is not one.
        assert!(!is_system_record(&entry(500, 42, "$recycle-note.txt", false)));
        assert!(!is_system_record(&entry(500, ROOT_RECORD, "Users", true)));
    }
}
