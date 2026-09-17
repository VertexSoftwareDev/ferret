//! Little-endian readers used by the on-disk structure parsers.
//!
//! Every one of these returns `0` instead of panicking when the slice is too
//! short: MFT records can be truncated or corrupt, and a file index should skip
//! a bad record rather than take the whole scan down with it. Parsers check
//! lengths before trusting a value.

#[inline]
pub fn u16le(buf: &[u8], off: usize) -> u16 {
    match buf.get(off..off + 2) {
        Some(b) => u16::from_le_bytes([b[0], b[1]]),
        None => 0,
    }
}

#[inline]
pub fn u32le(buf: &[u8], off: usize) -> u32 {
    match buf.get(off..off + 4) {
        Some(b) => u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        None => 0,
    }
}

#[inline]
pub fn u64le(buf: &[u8], off: usize) -> u64 {
    match buf.get(off..off + 8) {
        Some(b) => u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]),
        None => 0,
    }
}

/// Decode `len` UTF-16LE code units starting at `off`.
///
/// Unpaired surrogates are replaced rather than rejected — a file with a
/// broken name still belongs in the index.
pub fn utf16le(buf: &[u8], off: usize, len_chars: usize) -> Option<String> {
    buf.get(off..off + len_chars * 2)?;
    let units: Vec<u16> = (0..len_chars).map(|i| u16le(buf, off + i * 2)).collect();
    Some(String::from_utf16_lossy(&units))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_little_endian_values() {
        let buf = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08];
        assert_eq!(u16le(&buf, 0), 0x0201);
        assert_eq!(u32le(&buf, 0), 0x04030201);
        assert_eq!(u64le(&buf, 0), 0x0807060504030201);
    }

    #[test]
    fn out_of_range_reads_are_zero_not_panics() {
        let buf = [0x01, 0x02];
        assert_eq!(u16le(&buf, 1), 0);
        assert_eq!(u32le(&buf, 0), 0);
        assert_eq!(u64le(&buf, 0), 0);
    }

    #[test]
    fn decodes_utf16_names() {
        // "Ferret" in UTF-16LE.
        let mut buf = vec![0u8; 4];
        for ch in "Ferret".encode_utf16() {
            buf.extend_from_slice(&ch.to_le_bytes());
        }
        assert_eq!(utf16le(&buf, 4, 6).as_deref(), Some("Ferret"));
        // Asking for more characters than exist must fail, not truncate.
        assert!(utf16le(&buf, 4, 7).is_none());
    }
}
