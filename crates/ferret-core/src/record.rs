//! MFT FILE record parsing.
//!
//! Every file and directory on an NTFS volume owns at least one 1 KB record in
//! `$MFT`. A record is a header followed by a chain of attributes; the ones
//! Ferret cares about are `$FILE_NAME` (0x30), which holds the name and the
//! parent directory, and `$DATA` (0x80), which holds the size.
//!
//! Records are stored with "fixups": the last two bytes of each sector are
//! replaced by a check value and the real bytes are parked in an array in the
//! header. [`apply_fixups`] puts them back — skip it and every 512th byte pair
//! of the record is wrong.

use crate::bytes::{u16le, u32le, u64le, utf16le};

pub const SIGNATURE: &[u8; 4] = b"FILE";

/// Record header flag: the record describes a live file (not a deleted one).
pub const FLAG_IN_USE: u16 = 0x0001;
/// Record header flag: the record describes a directory.
pub const FLAG_DIRECTORY: u16 = 0x0002;

pub const ATTR_STANDARD_INFO: u32 = 0x10;
pub const ATTR_FILE_NAME: u32 = 0x30;
pub const ATTR_DATA: u32 = 0x80;
const ATTR_END: u32 = 0xFFFF_FFFF;

/// DOS attribute bits stored in `$STANDARD_INFORMATION`.
const DOS_READONLY: u32 = 0x0001;
const DOS_HIDDEN: u32 = 0x0002;
const DOS_SYSTEM: u32 = 0x0004;

mod hdr {
    pub const USA_OFFSET: usize = 0x04;
    pub const USA_COUNT: usize = 0x06;
    pub const FLAGS: usize = 0x16;
    pub const FIRST_ATTR: usize = 0x14;
    pub const USED_SIZE: usize = 0x18;
}

/// Reverse the fixup encoding in place.
///
/// Returns `false` when the record is not a FILE record or its fixup array is
/// inconsistent, which is the signal to skip it.
pub fn apply_fixups(record: &mut [u8], bytes_per_sector: usize) -> bool {
    if record.len() < 0x30 || &record[0..4] != SIGNATURE {
        return false;
    }

    let usa_offset = u16le(record, hdr::USA_OFFSET) as usize;
    let usa_count = u16le(record, hdr::USA_COUNT) as usize;

    // The array is one check value plus one saved pair per sector.
    if usa_count == 0 || usa_offset + usa_count * 2 > record.len() {
        return false;
    }
    let sectors = usa_count - 1;
    if sectors == 0 || sectors * bytes_per_sector > record.len() {
        return false;
    }

    let check = u16le(record, usa_offset);

    for sector in 0..sectors {
        let tail = (sector + 1) * bytes_per_sector - 2;
        // Each sector must currently end with the check value; if it does not,
        // the record was torn mid-write and its contents cannot be trusted.
        if u16le(record, tail) != check {
            return false;
        }
        let saved = u16le(record, usa_offset + 2 + sector * 2);
        record[tail] = (saved & 0xFF) as u8;
        record[tail + 1] = (saved >> 8) as u8;
    }

    true
}

/// Header fields of a FILE record, after fixups have been applied.
#[derive(Debug, Clone, Copy)]
pub struct RecordHeader {
    pub flags: u16,
    pub first_attribute: usize,
    pub used_size: u32,
}

impl RecordHeader {
    pub fn in_use(&self) -> bool {
        self.flags & FLAG_IN_USE != 0
    }
    pub fn is_directory(&self) -> bool {
        self.flags & FLAG_DIRECTORY != 0
    }
}

pub fn parse_header(record: &[u8]) -> Option<RecordHeader> {
    if record.len() < 0x30 || &record[0..4] != SIGNATURE {
        return None;
    }
    let first_attribute = u16le(record, hdr::FIRST_ATTR) as usize;
    let used_size = u32le(record, hdr::USED_SIZE);
    if first_attribute >= record.len() {
        return None;
    }
    Some(RecordHeader {
        flags: u16le(record, hdr::FLAGS),
        first_attribute,
        used_size,
    })
}

/// One attribute in a record's attribute chain.
pub struct Attribute<'a> {
    pub kind: u32,
    pub non_resident: bool,
    pub name_len: u8,
    /// The whole attribute, header included.
    pub raw: &'a [u8],
}

impl<'a> Attribute<'a> {
    /// Content of a resident attribute, or `None` if it is non-resident.
    pub fn resident_value(&self) -> Option<&'a [u8]> {
        if self.non_resident {
            return None;
        }
        let length = u32le(self.raw, 0x10) as usize;
        let offset = u16le(self.raw, 0x14) as usize;
        self.raw.get(offset..offset + length)
    }

    /// Run list bytes of a non-resident attribute.
    pub fn run_list(&self) -> Option<&'a [u8]> {
        if !self.non_resident {
            return None;
        }
        let offset = u16le(self.raw, 0x20) as usize;
        self.raw.get(offset..)
    }

    /// Real (not allocated) content size of a non-resident attribute.
    pub fn non_resident_size(&self) -> Option<u64> {
        if !self.non_resident {
            return None;
        }
        Some(u64le(self.raw, 0x30))
    }
}

/// Walk the attribute chain of a record.
///
/// The iterator stops at the end marker, at the used-size boundary, or at the
/// first attribute whose declared length is impossible — a corrupt length must
/// not become an infinite loop.
pub fn attributes<'a>(record: &'a [u8], header: &RecordHeader) -> Attributes<'a> {
    let limit = (header.used_size as usize).min(record.len());
    Attributes {
        record,
        pos: header.first_attribute,
        limit,
    }
}

pub struct Attributes<'a> {
    record: &'a [u8],
    pos: usize,
    limit: usize,
}

impl<'a> Iterator for Attributes<'a> {
    type Item = Attribute<'a>;

    fn next(&mut self) -> Option<Attribute<'a>> {
        if self.pos + 8 > self.limit {
            return None;
        }
        let kind = u32le(self.record, self.pos);
        if kind == ATTR_END {
            return None;
        }
        let length = u32le(self.record, self.pos + 4) as usize;
        // A zero or oversized length would loop forever / read out of bounds.
        if length < 0x10 || self.pos + length > self.limit {
            return None;
        }

        let raw = &self.record[self.pos..self.pos + length];
        self.pos += length;

        Some(Attribute {
            kind,
            non_resident: raw[0x08] != 0,
            name_len: raw[0x09],
            raw,
        })
    }
}

/// Which naming scheme a `$FILE_NAME` attribute uses.
///
/// A file usually has two of these: its real long name and an 8.3 `Dos` alias.
/// Indexing the alias would show `PROGRA~1` in results, so it is filtered out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Namespace {
    Posix,
    Win32,
    Dos,
    Win32AndDos,
    Unknown(u8),
}

impl Namespace {
    fn from(value: u8) -> Namespace {
        match value {
            0 => Namespace::Posix,
            1 => Namespace::Win32,
            2 => Namespace::Dos,
            3 => Namespace::Win32AndDos,
            other => Namespace::Unknown(other),
        }
    }

    /// Whether this is a name a person would recognise.
    pub fn is_preferred(&self) -> bool {
        matches!(
            self,
            Namespace::Win32 | Namespace::Win32AndDos | Namespace::Posix
        )
    }
}

/// The parts of `$FILE_NAME` that Ferret indexes.
#[derive(Debug, Clone)]
pub struct FileName {
    /// MFT record number of the containing directory.
    pub parent: u64,
    pub name: String,
    pub namespace: Namespace,
    /// Size as recorded in the directory entry; only a hint, `$DATA` wins.
    pub real_size: u64,
}

/// The parts of `$STANDARD_INFORMATION` that Ferret indexes.
///
/// `$FILE_NAME` carries timestamps too, but Windows does not keep those in step
/// with reality — the authoritative "date modified" a user recognises lives
/// here.
#[derive(Debug, Clone, Copy)]
pub struct StandardInfo {
    pub created: u64,
    pub modified: u64,
    pub dos_attributes: u32,
}

impl StandardInfo {
    /// Translate the DOS attribute bits into [`crate::mft`] entry flags.
    pub fn flags(&self) -> u16 {
        let mut flags = 0u16;
        if self.dos_attributes & DOS_HIDDEN != 0 {
            flags |= crate::mft::IS_HIDDEN;
        }
        if self.dos_attributes & DOS_SYSTEM != 0 {
            flags |= crate::mft::IS_SYSTEM;
        }
        if self.dos_attributes & DOS_READONLY != 0 {
            flags |= crate::mft::IS_READONLY;
        }
        flags
    }
}

pub fn parse_standard_info(value: &[u8]) -> Option<StandardInfo> {
    if value.len() < 0x24 {
        return None;
    }
    Some(StandardInfo {
        created: u64le(value, 0x00),
        modified: u64le(value, 0x08),
        dos_attributes: u32le(value, 0x20),
    })
}

pub fn parse_file_name(value: &[u8]) -> Option<FileName> {
    if value.len() < 0x42 {
        return None;
    }
    let name_chars = value[0x40] as usize;
    let name = utf16le(value, 0x42, name_chars)?;

    Some(FileName {
        // The top 16 bits are a sequence number, not part of the reference.
        parent: u64le(value, 0x00) & 0x0000_FFFF_FFFF_FFFF,
        name,
        namespace: Namespace::from(value[0x41]),
        real_size: u64le(value, 0x30),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal record with a valid fixup array.
    fn record_with_fixups(sectors: usize, sector_size: usize) -> Vec<u8> {
        let mut rec = vec![0u8; sectors * sector_size];
        rec[0..4].copy_from_slice(SIGNATURE);
        let usa_offset = 0x30usize;
        rec[hdr::USA_OFFSET] = usa_offset as u8;
        rec[hdr::USA_COUNT] = (sectors + 1) as u8;

        // Check value 0xBEEF at the array head and at the tail of each sector.
        rec[usa_offset] = 0xEF;
        rec[usa_offset + 1] = 0xBE;
        for s in 0..sectors {
            let tail = (s + 1) * sector_size - 2;
            rec[tail] = 0xEF;
            rec[tail + 1] = 0xBE;
            // The real bytes that belong there: 0xAA00 + sector index.
            rec[usa_offset + 2 + s * 2] = s as u8;
            rec[usa_offset + 3 + s * 2] = 0xAA;
        }
        rec
    }

    #[test]
    fn fixups_restore_the_real_sector_tails() {
        let mut rec = record_with_fixups(2, 512);
        assert!(apply_fixups(&mut rec, 512));
        assert_eq!(u16le(&rec, 512 - 2), 0xAA00);
        assert_eq!(u16le(&rec, 1024 - 2), 0xAA01);
    }

    #[test]
    fn a_torn_record_is_rejected() {
        let mut rec = record_with_fixups(2, 512);
        // Corrupt the second sector's check value.
        rec[1024 - 1] = 0x00;
        assert!(!apply_fixups(&mut rec, 512));
    }

    #[test]
    fn non_file_records_are_rejected() {
        let mut rec = vec![0u8; 1024];
        rec[0..4].copy_from_slice(b"BAAD");
        assert!(!apply_fixups(&mut rec, 512));
        assert!(parse_header(&rec).is_none());
    }

    #[test]
    fn parses_a_file_name_attribute() {
        let mut value = vec![0u8; 0x42];
        value[0x00] = 0x05; // parent = record 5 (the root directory)
        value[0x30] = 0x10; // real size = 16
        value[0x40] = 6; // six characters
        value[0x41] = 1; // Win32 namespace
        for ch in "Ferret".encode_utf16() {
            value.extend_from_slice(&ch.to_le_bytes());
        }

        let parsed = parse_file_name(&value).expect("should parse");
        assert_eq!(parsed.parent, 5);
        assert_eq!(parsed.name, "Ferret");
        assert_eq!(parsed.namespace, Namespace::Win32);
        assert_eq!(parsed.real_size, 16);
        assert!(parsed.namespace.is_preferred());
    }

    #[test]
    fn parses_standard_information() {
        let mut value = vec![0u8; 0x30];
        value[0x08..0x10].copy_from_slice(&126_227_808_000_000_000u64.to_le_bytes());
        value[0x20..0x24].copy_from_slice(&(DOS_HIDDEN | DOS_SYSTEM).to_le_bytes());

        let info = parse_standard_info(&value).expect("should parse");
        assert_eq!(info.modified, 126_227_808_000_000_000);
        assert!(info.flags() & crate::mft::IS_HIDDEN != 0);
        assert!(info.flags() & crate::mft::IS_SYSTEM != 0);
        assert!(info.flags() & crate::mft::IS_READONLY == 0);
    }

    #[test]
    fn a_truncated_standard_information_is_rejected() {
        assert!(parse_standard_info(&[0u8; 8]).is_none());
    }

    #[test]
    fn dos_alias_names_are_not_preferred() {
        assert!(!Namespace::from(2).is_preferred());
        assert!(Namespace::from(3).is_preferred());
    }

    #[test]
    fn attribute_walk_stops_on_a_corrupt_length() {
        let mut rec = vec![0u8; 1024];
        rec[0..4].copy_from_slice(SIGNATURE);
        rec[hdr::FIRST_ATTR] = 0x38;
        rec[hdr::USED_SIZE] = 0xFF;

        // One well-formed attribute, then one claiming length 0.
        let first = 0x38usize;
        rec[first..first + 4].copy_from_slice(&ATTR_FILE_NAME.to_le_bytes());
        rec[first + 4..first + 8].copy_from_slice(&0x20u32.to_le_bytes());
        let second = first + 0x20;
        rec[second..second + 4].copy_from_slice(&ATTR_DATA.to_le_bytes());
        // length stays 0 -> iterator must stop rather than spin.

        let header = parse_header(&rec).unwrap();
        let kinds: Vec<u32> = attributes(&rec, &header).map(|a| a.kind).collect();
        assert_eq!(kinds, vec![ATTR_FILE_NAME]);
    }
}
