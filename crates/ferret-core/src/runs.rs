//! Data run decoding.
//!
//! A non-resident NTFS attribute does not store its content inline; it stores a
//! "run list" describing where on disk the content lives. Each run is a
//! variable-width pair: a length in clusters, and a cluster offset *relative to
//! the previous run's start*. The first byte packs the widths of the two fields
//! into its two nibbles, and a zero byte ends the list.
//!
//! This is how `$MFT` itself is located: the MFT is a file, and its `$DATA`
//! attribute is a run list like any other.

/// One extent of an attribute's content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Run {
    /// Starting cluster on the volume, or `None` for a sparse run (a hole).
    pub lcn: Option<u64>,
    /// Length of the run in clusters.
    pub clusters: u64,
}

/// Decode a run list.
///
/// Stops at the terminator, at the end of the slice, or at the first entry that
/// does not fit — a malformed tail yields the runs decoded so far rather than
/// an error, which keeps one damaged attribute from aborting a whole scan.
pub fn parse_runs(buf: &[u8]) -> Vec<Run> {
    let mut runs = Vec::new();
    let mut pos = 0usize;
    let mut previous_lcn: i64 = 0;

    while pos < buf.len() {
        let header = buf[pos];
        if header == 0 {
            break; // end of the run list
        }
        pos += 1;

        let len_size = (header & 0x0F) as usize;
        let off_size = (header >> 4) as usize;

        // Both fields are at most 8 bytes; a larger nibble means corruption.
        if len_size == 0 || len_size > 8 || off_size > 8 {
            break;
        }
        if pos + len_size + off_size > buf.len() {
            break;
        }

        let clusters = read_unsigned(&buf[pos..pos + len_size]);
        pos += len_size;

        if off_size == 0 {
            // No offset field: a sparse run, i.e. a hole with no storage.
            runs.push(Run { lcn: None, clusters });
            continue;
        }

        // The offset is signed and relative to the previous run's start, so
        // runs can point backwards on a fragmented volume.
        let delta = read_signed(&buf[pos..pos + off_size]);
        pos += off_size;

        let lcn = previous_lcn.wrapping_add(delta);
        previous_lcn = lcn;

        if lcn < 0 {
            break; // would point outside the volume
        }
        runs.push(Run { lcn: Some(lcn as u64), clusters });
    }

    runs
}

/// Total cluster count covered by a run list, holes included.
pub fn total_clusters(runs: &[Run]) -> u64 {
    runs.iter().map(|r| r.clusters).sum()
}

fn read_unsigned(bytes: &[u8]) -> u64 {
    let mut value = 0u64;
    for (i, b) in bytes.iter().enumerate() {
        value |= (*b as u64) << (i * 8);
    }
    value
}

/// Read a little-endian two's-complement integer of 1..=8 bytes.
fn read_signed(bytes: &[u8]) -> i64 {
    let mut value = read_unsigned(bytes);
    let bits = bytes.len() * 8;
    if bits < 64 {
        // Sign-extend from the field's own width.
        let sign_bit = 1u64 << (bits - 1);
        if value & sign_bit != 0 {
            value |= !0u64 << bits;
        }
    }
    value as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_single_run() {
        // 0x21: 1 length byte, 2 offset bytes. Length 0x18, offset 0x0233.
        let buf = [0x21, 0x18, 0x33, 0x02, 0x00];
        assert_eq!(
            parse_runs(&buf),
            vec![Run { lcn: Some(0x0233), clusters: 0x18 }]
        );
    }

    #[test]
    fn offsets_are_relative_and_can_go_backwards() {
        // Run 1 at 0x60, then a negative delta of -0x20 -> run 2 at 0x40.
        let buf = [
            0x11, 0x10, 0x60, // len 0x10 @ +0x60  => lcn 0x60
            0x11, 0x08, 0xE0, // len 0x08 @ -0x20  => lcn 0x40
            0x00,
        ];
        assert_eq!(
            parse_runs(&buf),
            vec![
                Run { lcn: Some(0x60), clusters: 0x10 },
                Run { lcn: Some(0x40), clusters: 0x08 },
            ]
        );
    }

    #[test]
    fn zero_offset_field_means_sparse() {
        let buf = [0x01, 0x05, 0x00];
        assert_eq!(parse_runs(&buf), vec![Run { lcn: None, clusters: 5 }]);
    }

    #[test]
    fn truncated_list_yields_what_was_decoded() {
        // Second entry claims 2 offset bytes but only one is present.
        let buf = [0x11, 0x10, 0x60, 0x21, 0x08, 0x33];
        assert_eq!(
            parse_runs(&buf),
            vec![Run { lcn: Some(0x60), clusters: 0x10 }]
        );
    }

    #[test]
    fn sums_clusters() {
        let runs = [
            Run { lcn: Some(0), clusters: 3 },
            Run { lcn: None, clusters: 4 },
        ];
        assert_eq!(total_clusters(&runs), 7);
    }
}
