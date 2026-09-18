//! The NTFS change journal (USN journal).
//!
//! A full scan gives a snapshot. The journal is how that snapshot stays true:
//! NTFS records every create, rename, delete and write into a circular log, and
//! each entry carries the MFT record number of the file it concerns. Reading the
//! log tells Ferret exactly which records to re-read, so keeping an index of two
//! million files current costs a few kilobytes a second instead of a rescan.
//!
//! Two `DeviceIoControl` codes do the work: one asks the volume about its
//! journal, the other reads entries after a given sequence number. They are
//! declared here by hand rather than pulling in a Windows API crate for four
//! calls.

use std::ffi::c_void;
use std::io;
use std::os::windows::io::AsRawHandle;

use crate::bytes::{u16le, u32le, u64le, utf16le};
use crate::volume::Volume;

// SAFETY-relevant note: these are the standard kernel32 signatures. The only
// pointers handed over are to local buffers whose sizes are passed alongside.
unsafe extern "system" {
    fn DeviceIoControl(
        device: *mut c_void,
        control_code: u32,
        in_buffer: *const c_void,
        in_size: u32,
        out_buffer: *mut c_void,
        out_size: u32,
        bytes_returned: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
}

const FSCTL_QUERY_USN_JOURNAL: u32 = 0x0009_00F4;
const FSCTL_READ_USN_JOURNAL: u32 = 0x0009_00BB;

/// Journal reasons Ferret cares about: anything that changes a name, a parent,
/// a size or an attribute. Access-time-only churn is ignored.
const REASON_MASK: u32 = REASON_FILE_CREATE
    | REASON_FILE_DELETE
    | REASON_RENAME_NEW_NAME
    | REASON_RENAME_OLD_NAME
    | REASON_DATA_EXTEND
    | REASON_DATA_TRUNCATION
    | REASON_DATA_OVERWRITE
    | REASON_BASIC_INFO_CHANGE
    | REASON_CLOSE;

pub const REASON_DATA_OVERWRITE: u32 = 0x0000_0001;
pub const REASON_DATA_EXTEND: u32 = 0x0000_0002;
pub const REASON_DATA_TRUNCATION: u32 = 0x0000_0004;
pub const REASON_BASIC_INFO_CHANGE: u32 = 0x0000_8000;
pub const REASON_FILE_CREATE: u32 = 0x0000_0100;
pub const REASON_FILE_DELETE: u32 = 0x0000_0200;
pub const REASON_RENAME_OLD_NAME: u32 = 0x0000_1000;
pub const REASON_RENAME_NEW_NAME: u32 = 0x0000_2000;
pub const REASON_CLOSE: u32 = 0x8000_0000;

/// Read buffer size. The journal is read in a loop, so this only bounds how
/// much comes back per call; 64 KB holds several hundred entries.
const READ_BUFFER: usize = 64 * 1024;

/// What the volume says about its journal.
#[derive(Debug, Clone, Copy)]
pub struct JournalInfo {
    pub id: u64,
    /// Sequence number one past the newest entry — where to start listening.
    pub next_usn: i64,
    /// Oldest sequence number still in the log.
    pub lowest_valid_usn: i64,
}

/// `FILE_ATTRIBUTE_DIRECTORY`.
const ATTRIBUTE_DIRECTORY: u32 = 0x0000_0010;
/// `FILE_ATTRIBUTE_HIDDEN`.
const ATTRIBUTE_HIDDEN: u32 = 0x0000_0002;
/// `FILE_ATTRIBUTE_SYSTEM`.
const ATTRIBUTE_SYSTEM: u32 = 0x0000_0004;

/// One change, already reduced to what the index needs.
///
/// The journal entry carries the name, the parent and the attributes, which is
/// deliberately everything needed to update an entry *without* going back to
/// the disk. Re-reading the MFT record would be the obvious move and is the
/// wrong one: raw volume reads bypass the filesystem cache, so a file created a
/// moment ago is still an empty record on the platter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// MFT record number of the file that changed.
    pub record: u32,
    /// MFT record number of its parent directory.
    pub parent: u32,
    pub name: String,
    pub reason: u32,
    pub attributes: u32,
}

impl Change {
    pub fn is_delete(&self) -> bool {
        self.reason & REASON_FILE_DELETE != 0
    }
    pub fn is_dir(&self) -> bool {
        self.attributes & ATTRIBUTE_DIRECTORY != 0
    }
    pub fn is_hidden(&self) -> bool {
        self.attributes & ATTRIBUTE_HIDDEN != 0
    }
    pub fn is_system(&self) -> bool {
        self.attributes & ATTRIBUTE_SYSTEM != 0
    }
}

/// A position in the journal, carried between polls.
#[derive(Debug, Clone, Copy)]
pub struct Cursor {
    pub journal_id: u64,
    pub next_usn: i64,
}

/// Ask a volume about its change journal.
///
/// Returns [`io::ErrorKind::Unsupported`] when the journal is disabled, which is
/// a normal configuration rather than a failure — callers fall back to manual
/// refresh.
pub fn query(volume: &Volume) -> io::Result<JournalInfo> {
    let mut out = [0u8; 80];
    let mut returned = 0u32;

    let ok = unsafe {
        DeviceIoControl(
            volume.file().as_raw_handle(),
            FSCTL_QUERY_USN_JOURNAL,
            std::ptr::null(),
            0,
            out.as_mut_ptr() as *mut c_void,
            out.len() as u32,
            &mut returned,
            std::ptr::null_mut(),
        )
    };

    if ok == 0 {
        let err = io::Error::last_os_error();
        // ERROR_JOURNAL_NOT_ACTIVE / ERROR_INVALID_FUNCTION
        return Err(match err.raw_os_error() {
            Some(1179) | Some(1) => io::Error::new(
                io::ErrorKind::Unsupported,
                "bu birimde degisiklik gunlugu (USN journal) kapali",
            ),
            _ => err,
        });
    }
    if returned < 32 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "USN journal yaniti eksik",
        ));
    }

    Ok(JournalInfo {
        id: u64le(&out, 0),
        next_usn: u64le(&out, 16) as i64,
        lowest_valid_usn: u64le(&out, 24) as i64,
    })
}

/// Start listening from wherever the journal is now.
pub fn cursor_at_end(volume: &Volume) -> io::Result<Cursor> {
    let info = query(volume)?;
    Ok(Cursor {
        journal_id: info.id,
        next_usn: info.next_usn,
    })
}

/// Read everything that happened since `cursor`, and advance it.
///
/// Returns at most one buffer's worth per call; call again while the returned
/// list is non-empty to drain a backlog.
pub fn read(volume: &Volume, cursor: &mut Cursor) -> io::Result<Vec<Change>> {
    // READ_USN_JOURNAL_DATA_V0
    let mut input = [0u8; 40];
    input[0..8].copy_from_slice(&cursor.next_usn.to_le_bytes());
    input[8..12].copy_from_slice(&REASON_MASK.to_le_bytes());
    // ReturnOnlyOnClose = 0, Timeout = 0, BytesToWaitFor = 0 -> never block.
    input[32..40].copy_from_slice(&cursor.journal_id.to_le_bytes());

    let mut out = vec![0u8; READ_BUFFER];
    let mut returned = 0u32;

    let ok = unsafe {
        DeviceIoControl(
            volume.file().as_raw_handle(),
            FSCTL_READ_USN_JOURNAL,
            input.as_ptr() as *const c_void,
            input.len() as u32,
            out.as_mut_ptr() as *mut c_void,
            out.len() as u32,
            &mut returned,
            std::ptr::null_mut(),
        )
    };

    if ok == 0 {
        let err = io::Error::last_os_error();
        return Err(match err.raw_os_error() {
            // The journal was deleted and recreated, or our start point has
            // scrolled out of the circular log: the index must be rebuilt.
            Some(1179) | Some(1181) | Some(1182) => io::Error::new(
                io::ErrorKind::InvalidData,
                "degisiklik gunlugu sifirlandi; yeniden tarama gerekiyor",
            ),
            _ => err,
        });
    }

    // The first eight bytes are the sequence number to resume from.
    if returned < 8 {
        return Ok(Vec::new());
    }
    cursor.next_usn = u64le(&out, 0) as i64;

    Ok(parse_records(&out[8..returned as usize]))
}

/// Decode a run of `USN_RECORD_V2` structures.
pub fn parse_records(buffer: &[u8]) -> Vec<Change> {
    let mut changes = Vec::new();
    let mut pos = 0usize;

    while pos + 60 <= buffer.len() {
        let length = u32le(buffer, pos) as usize;
        // A zero or oversized length would loop forever.
        if length < 60 || pos + length > buffer.len() {
            break;
        }
        let record = &buffer[pos..pos + length];
        pos += length;

        // Only version 2 is laid out as parsed below; anything else is skipped
        // rather than misread.
        if u16le(record, 4) != 2 {
            continue;
        }

        let name_len = u16le(record, 56) as usize;
        let name_offset = u16le(record, 58) as usize;
        if name_offset + name_len > record.len() || !name_len.is_multiple_of(2) {
            continue;
        }
        let Some(name) = utf16le(record, name_offset, name_len / 2) else {
            continue;
        };

        changes.push(Change {
            // The top 16 bits of a file reference are a sequence number.
            record: (u64le(record, 8) & 0x0000_FFFF_FFFF_FFFF) as u32,
            parent: (u64le(record, 16) & 0x0000_FFFF_FFFF_FFFF) as u32,
            name,
            reason: u32le(record, 40),
            attributes: u32le(record, 52),
        });
    }

    changes
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a USN_RECORD_V2 the way Windows lays one out.
    fn record(file_ref: u64, parent_ref: u64, name: &str, reason: u32) -> Vec<u8> {
        record_with(file_ref, parent_ref, name, reason, 0)
    }

    fn record_with(
        file_ref: u64,
        parent_ref: u64,
        name: &str,
        reason: u32,
        attributes: u32,
    ) -> Vec<u8> {
        let name_utf16: Vec<u8> = name
            .encode_utf16()
            .flat_map(|unit| unit.to_le_bytes())
            .collect();
        let length = 60 + name_utf16.len();

        let mut buf = vec![0u8; length];
        buf[0..4].copy_from_slice(&(length as u32).to_le_bytes());
        buf[4..6].copy_from_slice(&2u16.to_le_bytes()); // major version
        buf[8..16].copy_from_slice(&file_ref.to_le_bytes());
        buf[16..24].copy_from_slice(&parent_ref.to_le_bytes());
        buf[40..44].copy_from_slice(&reason.to_le_bytes());
        buf[52..56].copy_from_slice(&attributes.to_le_bytes());
        buf[56..58].copy_from_slice(&(name_utf16.len() as u16).to_le_bytes());
        buf[58..60].copy_from_slice(&60u16.to_le_bytes());
        buf[60..].copy_from_slice(&name_utf16);
        buf
    }

    #[test]
    fn parses_a_sequence_of_records() {
        let mut buffer = record(42, 5, "rapor.pdf", REASON_FILE_CREATE);
        buffer.extend(record(43, 42, "notlar.txt", REASON_FILE_DELETE));

        let changes = parse_records(&buffer);
        assert_eq!(changes.len(), 2);

        assert_eq!(changes[0].record, 42);
        assert_eq!(changes[0].parent, 5);
        assert_eq!(changes[0].name, "rapor.pdf");
        assert!(!changes[0].is_delete());

        assert_eq!(changes[1].record, 43);
        assert!(changes[1].is_delete());
    }

    #[test]
    fn strips_the_sequence_number_from_file_references() {
        // Windows packs a 16-bit sequence number above the 48-bit record number.
        let reference = (7u64 << 48) | 1234;
        let changes = parse_records(&record(reference, reference, "x", REASON_CLOSE));
        assert_eq!(changes[0].record, 1234);
        assert_eq!(changes[0].parent, 1234);
    }

    #[test]
    fn reads_the_directory_attribute() {
        let dir = parse_records(&record_with(10, 5, "Belgeler", REASON_FILE_CREATE, 0x10));
        assert!(dir[0].is_dir());
        let file = parse_records(&record_with(11, 5, "a.txt", REASON_FILE_CREATE, 0x20));
        assert!(!file[0].is_dir());
    }

    #[test]
    fn handles_non_ascii_names() {
        let changes = parse_records(&record(10, 5, "Çalışma Raporu.docx", REASON_CLOSE));
        assert_eq!(changes[0].name, "Çalışma Raporu.docx");
    }

    #[test]
    fn a_corrupt_length_stops_the_walk_instead_of_looping() {
        let mut buffer = record(42, 5, "ok.txt", REASON_CLOSE);
        // Append a record claiming length 0.
        buffer.extend_from_slice(&[0u8; 60]);

        let changes = parse_records(&buffer);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].name, "ok.txt");
    }

    #[test]
    fn a_truncated_tail_is_ignored() {
        let mut buffer = record(42, 5, "ok.txt", REASON_CLOSE);
        let partial = record(43, 5, "cut.txt", REASON_CLOSE);
        buffer.extend_from_slice(&partial[..30]);

        let changes = parse_records(&buffer);
        assert_eq!(changes.len(), 1);
    }

    #[test]
    fn unknown_record_versions_are_skipped() {
        let mut buffer = record(42, 5, "v3.txt", REASON_CLOSE);
        buffer[4..6].copy_from_slice(&3u16.to_le_bytes());
        assert!(parse_records(&buffer).is_empty());
    }

    #[test]
    fn a_name_running_past_the_record_is_skipped() {
        let mut buffer = record(42, 5, "ok.txt", REASON_CLOSE);
        // Claim a name far longer than the record.
        buffer[56..58].copy_from_slice(&9000u16.to_le_bytes());
        assert!(parse_records(&buffer).is_empty());
    }
}
