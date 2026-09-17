//! Raw access to an NTFS volume.
//!
//! Windows exposes a whole volume as the pseudo-file `\\.\C:`. Opening it
//! needs administrator rights and every read has to be aligned to the sector
//! size, which is why [`Volume::read_at`] does the alignment itself instead of
//! letting callers worry about it.
//!
//! Nothing here ever opens the volume for writing. Ferret only reads.

use std::fs::File;
use std::fs::OpenOptions;
use std::io::{self, Read, Seek, SeekFrom};

use crate::bytes::{u16le, u64le};

/// Offsets inside the NTFS boot sector (the "BIOS parameter block").
mod bpb {
    pub const OEM_ID: usize = 0x03;
    pub const BYTES_PER_SECTOR: usize = 0x0B;
    pub const SECTORS_PER_CLUSTER: usize = 0x0D;
    pub const TOTAL_SECTORS: usize = 0x28;
    pub const MFT_START_LCN: usize = 0x30;
    pub const CLUSTERS_PER_RECORD: usize = 0x40;
    pub const SERIAL: usize = 0x48;
}

/// An open, read-only NTFS volume plus the geometry read from its boot sector.
pub struct Volume {
    file: File,
    pub letter: char,
    pub bytes_per_sector: u32,
    pub sectors_per_cluster: u32,
    pub bytes_per_cluster: u64,
    pub total_sectors: u64,
    /// Cluster where `$MFT` begins.
    pub mft_start_lcn: u64,
    /// Size of a single MFT FILE record; 1024 on essentially every volume.
    pub bytes_per_record: u32,
    pub serial: u64,
}

impl Volume {
    /// Open a drive by letter, e.g. `Volume::open('C')`.
    ///
    /// Returns [`io::ErrorKind::PermissionDenied`] when the process is not
    /// elevated — the caller is expected to turn that into a readable message.
    pub fn open(letter: char) -> io::Result<Volume> {
        let letter = letter.to_ascii_uppercase();
        if !letter.is_ascii_alphabetic() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("gecersiz surucu harfi: {letter}"),
            ));
        }

        let path = format!(r"\\.\{letter}:");
        let mut file = OpenOptions::new().read(true).open(&path)?;

        // The sector size is not known yet, so read a block large enough to be
        // a whole number of sectors for every plausible sector size.
        let mut boot = [0u8; 4096];
        file.seek(SeekFrom::Start(0))?;
        file.read_exact(&mut boot)?;

        if &boot[bpb::OEM_ID..bpb::OEM_ID + 8] != b"NTFS    " {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{letter}: NTFS degil (Ferret su an yalnizca NTFS okur)"),
            ));
        }

        let bytes_per_sector = u16le(&boot, bpb::BYTES_PER_SECTOR) as u32;
        let sectors_per_cluster = boot[bpb::SECTORS_PER_CLUSTER] as u32;
        if bytes_per_sector == 0 || sectors_per_cluster == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "bozuk onyukleme sektoru: sifir sektor/kume boyutu",
            ));
        }

        // A negative value means "2^-value bytes" rather than a cluster count.
        let raw = boot[bpb::CLUSTERS_PER_RECORD] as i8;
        let bytes_per_cluster = bytes_per_sector as u64 * sectors_per_cluster as u64;
        let bytes_per_record = if raw < 0 {
            1u32 << ((-raw) as u32)
        } else {
            raw as u32 * bytes_per_cluster as u32
        };

        Ok(Volume {
            file,
            letter,
            bytes_per_sector,
            sectors_per_cluster,
            bytes_per_cluster,
            total_sectors: u64le(&boot, bpb::TOTAL_SECTORS),
            mft_start_lcn: u64le(&boot, bpb::MFT_START_LCN),
            bytes_per_record,
            serial: u64le(&boot, bpb::SERIAL),
        })
    }

    /// Total volume size in bytes.
    pub fn size_bytes(&self) -> u64 {
        self.total_sectors * self.bytes_per_sector as u64
    }

    /// Read `out.len()` bytes starting at byte `offset` of the volume.
    ///
    /// Volume handles reject unaligned reads, so an unaligned request is
    /// widened to sector boundaries and the wanted slice copied out.
    pub fn read_at(&mut self, offset: u64, out: &mut [u8]) -> io::Result<()> {
        if out.is_empty() {
            return Ok(());
        }
        let sector = self.bytes_per_sector as u64;

        // Fast path: already aligned, read straight into the caller's buffer.
        if offset.is_multiple_of(sector) && (out.len() as u64).is_multiple_of(sector) {
            self.file.seek(SeekFrom::Start(offset))?;
            return self.file.read_exact(out);
        }

        let start = offset - (offset % sector);
        let end_unaligned = offset + out.len() as u64;
        let end = end_unaligned.div_ceil(sector) * sector;

        let mut scratch = vec![0u8; (end - start) as usize];
        self.file.seek(SeekFrom::Start(start))?;
        self.file.read_exact(&mut scratch)?;

        let skip = (offset - start) as usize;
        out.copy_from_slice(&scratch[skip..skip + out.len()]);
        Ok(())
    }
}

/// Human-readable byte size, e.g. `1.4 GB`.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
