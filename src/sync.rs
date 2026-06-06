//! Record alignment, validation, and sync recovery.
//!
//! Pure functions: no logging, no side effects. The reader is responsible
//! for emitting any log output based on the returned values.

use crate::decode::{MIN_RECORD_WORDS_STANDARD, decode_type_word, message_type_is_valid, read_u16};
use crate::models::{TimestampFormat, timestamp_word_count};

/// 64 KB scan cap. Covers any reasonable header or corruption gap without
/// risking a runaway scan over multi-gigabyte files.
pub const MAX_SCAN_BYTES: usize = 65_536;

/// Word count field is 6 bits → max record = 63 × 2 = 126 bytes.
pub const MAX_RECORD_BYTES: usize = 126;

/// Minimum word count for a record under `ts_format`. If `None`, uses
/// the smaller (Standard) minimum so an unknown format is permissive.
#[inline]
fn min_word_count(ts_format: Option<TimestampFormat>) -> u16 {
    match ts_format {
        Some(fmt @ (TimestampFormat::Irig | TimestampFormat::Standard)) => {
            1 + timestamp_word_count(fmt) + 1
        }
        _ => MIN_RECORD_WORDS_STANDARD,
    }
}

/// True if a valid MIE record starts at `offset` per all heuristics
/// including a two-record look-ahead.
pub fn validate_record(
    data: &[u8],
    offset: usize,
    file_len: usize,
    ts_format: Option<TimestampFormat>,
) -> bool {
    // Check 1: Type Word readable.
    let Some(type_raw) = read_u16(data, offset) else {
        return false;
    };
    if offset + 2 > file_len {
        return false;
    }
    let tw = decode_type_word(type_raw);

    // Check 2: Valid message type.
    if !message_type_is_valid(tw.message_type) {
        return false;
    }

    // Check 3: Plausible word count.
    let min_wc = min_word_count(ts_format);
    if tw.word_count < min_wc || tw.word_count > 63 {
        return false;
    }

    // Check 4: Record fits in file.
    let record_bytes = usize::from(tw.word_count) * 2;
    if offset + record_bytes > file_len {
        return false;
    }

    // Check 5: IRIG timestamp field range checks per L2-SYN-004
    // and L2-SYN-004a. We need all three timestamp words to evaluate
    // microsecond and day; offset + 8 <= file_len covers reading
    // upper (offset+2), middle (offset+4), and lower (offset+6) words.
    if ts_format == Some(TimestampFormat::Irig) && offset + 8 <= file_len {
        if let (Some(ts_upper), Some(ts_middle), Some(ts_lower)) = (
            read_u16(data, offset + 2),
            read_u16(data, offset + 4),
            read_u16(data, offset + 6),
        ) {
            let freerun = (ts_upper >> 15) & 1 == 1;
            let day = (ts_upper >> 5) & 0x1FF; // bits 13-5
            let hour = ts_upper & 0x1F;
            let minute = (ts_middle >> 10) & 0x3F;
            let second = (ts_middle >> 4) & 0x3F;
            let microsecond_hi4 = u32::from(ts_middle & 0xF);
            let microsecond_lo16 = u32::from(ts_lower);
            let microsecond = (microsecond_hi4 << 16) | microsecond_lo16;

            if hour >= 24 || minute >= 60 || second >= 60 {
                return false;
            }
            if microsecond > 999_999 {
                return false;
            }
            // L2-SYN-004a: skip the day-of-year range check when
            // freerun is set, because the card's free-running
            // oscillator is not calendar-locked. Hour/minute/second/
            // microsecond constraints still apply because those are
            // a function of the counter modulus, not the external
            // IRIG-B feed.
            if !freerun && !(1..=366).contains(&day) {
                return false;
            }
        }
    }

    // Check 6: Two-record look-ahead. If next record would be at EOF, the
    // candidate is accepted on checks 1–5 alone.
    let next_offset = offset + record_bytes;
    if next_offset + 2 <= file_len {
        if let Some(next_raw) = read_u16(data, next_offset) {
            let next_tw = decode_type_word(next_raw);
            if !message_type_is_valid(next_tw.message_type) {
                return false;
            }
            if next_tw.word_count < min_wc || next_tw.word_count > 63 {
                return false;
            }
        }
    }

    true
}

/// Outcome of a scan: where the next valid record is, and how many bytes
/// of header/garbage were skipped to reach it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanHit {
    pub offset: usize,
    pub skipped: usize,
}

/// Find the byte offset of the first valid record in the file.
///
/// Scans `[0, min(file_len, max_scan))` in 2-byte increments. If `offset == 0`
/// is valid, returns immediately (no header). If a header is present, returns
/// the offset just after it. Returns `None` if no valid record found within
/// the scan window.
pub fn find_first_record(
    data: &[u8],
    file_len: usize,
    ts_format: Option<TimestampFormat>,
    max_scan: usize,
) -> Option<ScanHit> {
    let scan_end = file_len.min(max_scan);
    let mut offset = 0;
    while offset < scan_end {
        if validate_record(data, offset, file_len, ts_format) {
            return Some(ScanHit {
                offset,
                skipped: offset,
            });
        }
        offset += 2;
    }
    None
}

/// Walk forward from `offset` looking for the next valid record. Used after
/// validation fails mid-file.
pub fn recover_sync(
    data: &[u8],
    offset: usize,
    file_len: usize,
    ts_format: Option<TimestampFormat>,
    max_scan: usize,
) -> Option<ScanHit> {
    let scan_start = offset.saturating_add(2);
    let scan_end = file_len.min(offset.saturating_add(max_scan));
    let mut candidate = scan_start;
    while candidate < scan_end {
        if validate_record(data, candidate, file_len, ts_format) {
            return Some(ScanHit {
                offset: candidate,
                skipped: candidate - offset,
            });
        }
        candidate += 2;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One synthetic record:
    ///   Type Word 0x2402 → type=0x02, bus A, word_count=36, no error
    ///   Followed by 35 zeroed words to fill to 36 total
    fn make_valid_record_36w(count: usize) -> Vec<u8> {
        let mut buf = Vec::with_capacity(count * 36 * 2);
        for _ in 0..count {
            // Type word 0x2402 LE
            buf.extend_from_slice(&[0x02, 0x24]);
            // 35 zero words (timestamp + cmd + data + status)
            buf.extend_from_slice(&[0u8; 70]);
        }
        buf
    }

    #[test]
    fn validate_accepts_clean_record() {
        let buf = make_valid_record_36w(2);
        assert!(validate_record(&buf, 0, buf.len(), None));
    }

    #[test]
    fn validate_rejects_invalid_type() {
        // Type word with bad message type 0x03
        let buf = vec![0x03, 0x24, 0x00, 0x00];
        assert!(!validate_record(&buf, 0, buf.len(), None));
    }

    #[test]
    fn validate_rejects_truncated() {
        // Type word claims 36 words = 72 bytes, but only 4 bytes available
        let buf = vec![0x02, 0x24, 0x00, 0x00];
        assert!(!validate_record(&buf, 0, buf.len(), None));
    }

    #[test]
    fn find_first_record_at_zero() {
        let buf = make_valid_record_36w(2);
        let hit = find_first_record(&buf, buf.len(), None, MAX_SCAN_BYTES).unwrap();
        assert_eq!(hit.offset, 0);
        assert_eq!(hit.skipped, 0);
    }

    #[test]
    fn find_first_record_skips_header() {
        // 16-byte ASCII header + valid records
        let mut buf = b"DDC-HEADER-1234\n".to_vec(); // 16 bytes
        buf.extend(make_valid_record_36w(2));
        let hit = find_first_record(&buf, buf.len(), None, MAX_SCAN_BYTES).unwrap();
        assert_eq!(hit.offset, 16);
        assert_eq!(hit.skipped, 16);
    }

    #[test]
    fn find_first_record_returns_none_when_no_valid() {
        let buf = vec![0xFFu8; 64];
        assert!(find_first_record(&buf, buf.len(), None, MAX_SCAN_BYTES).is_none());
    }

    #[test]
    fn recover_sync_walks_forward() {
        let mut buf = vec![0xFFu8; 6]; // 6 bytes of garbage
        buf.extend(make_valid_record_36w(2));
        let hit = recover_sync(&buf, 0, buf.len(), None, MAX_SCAN_BYTES).unwrap();
        assert_eq!(hit.offset, 6);
        assert_eq!(hit.skipped, 6);
    }

    #[test]
    fn recover_sync_capped_at_max_scan() {
        let mut buf = vec![0xFFu8; 200];
        // Valid record only after 200 bytes of garbage
        buf.extend(make_valid_record_36w(2));
        // Cap scan at 100 → can't find it
        assert!(recover_sync(&buf, 0, buf.len(), None, 100).is_none());
        // Cap at 400 → found
        assert!(recover_sync(&buf, 0, buf.len(), None, 400).is_some());
    }

    // ── IRIG range validation (L2-SYN-004, L2-SYN-004a) ──────────────

    /// Build a minimal IRIG-shaped record from explicit timestamp word
    /// values. word_count = 5 (Type + 3 TS + Cmd, no data, no status).
    /// Two records back-to-back so the look-ahead check passes.
    fn make_irig_record_with_ts(upper: u16, middle: u16, lower: u16) -> Vec<u8> {
        // Type word 0x0502: type=0x02 BcToRt, bus A, wc=5, error=0.
        const TYPE_RAW: u16 = 0x0502;
        // Command Word: rt=5, dir=Recv, sa=1, dwc=30 (raw 0x283E).
        const CMD_RAW: u16 = 0x283E;

        let mut buf = Vec::with_capacity(20);
        for _ in 0..2 {
            buf.extend_from_slice(&TYPE_RAW.to_le_bytes());
            buf.extend_from_slice(&upper.to_le_bytes());
            buf.extend_from_slice(&middle.to_le_bytes());
            buf.extend_from_slice(&lower.to_le_bytes());
            buf.extend_from_slice(&CMD_RAW.to_le_bytes());
        }
        buf
    }

    /// Helper: build upper TS word from explicit fields.
    fn irig_upper(freerun: bool, day: u16, hour: u8) -> u16 {
        ((freerun as u16) << 15) | ((day & 0x1FF) << 5) | u16::from(hour & 0x1F)
    }

    /// Helper: build middle TS word from explicit fields.
    fn irig_middle(minute: u8, second: u8, us_hi4: u8) -> u16 {
        ((u16::from(minute) & 0x3F) << 10)
            | ((u16::from(second) & 0x3F) << 4)
            | (u16::from(us_hi4) & 0xF)
    }

    #[test]
    fn validate_accepts_irig_with_valid_ranges() {
        // day=192, hour=15, minute=54, second=50, microsecond=456_225,
        // freerun=0 — matches the canonical conformance fixture.
        let upper = irig_upper(false, 192, 15);
        let middle = irig_middle(54, 50, 6); // us_hi4 = 6
        let lower = 0xF621u16; // us_lo16 = 0xF621
        let buf = make_irig_record_with_ts(upper, middle, lower);
        assert!(validate_record(
            &buf,
            0,
            buf.len(),
            Some(TimestampFormat::Irig)
        ));
    }

    #[test]
    fn validate_rejects_irig_day_zero() {
        // day=0 is out of range per L2-SYN-004.
        let upper = irig_upper(false, 0, 15);
        let middle = irig_middle(54, 50, 0);
        let buf = make_irig_record_with_ts(upper, middle, 0);
        assert!(!validate_record(
            &buf,
            0,
            buf.len(),
            Some(TimestampFormat::Irig)
        ));
    }

    #[test]
    fn validate_rejects_irig_day_above_366() {
        // day=367 is out of range per L2-SYN-004.
        let upper = irig_upper(false, 367, 15);
        let middle = irig_middle(54, 50, 0);
        let buf = make_irig_record_with_ts(upper, middle, 0);
        assert!(!validate_record(
            &buf,
            0,
            buf.len(),
            Some(TimestampFormat::Irig)
        ));
    }

    #[test]
    fn validate_accepts_irig_day_zero_when_freerun() {
        // L2-SYN-004a: freerun bypasses the day-of-year check.
        let upper = irig_upper(true, 0, 15);
        let middle = irig_middle(54, 50, 0);
        let buf = make_irig_record_with_ts(upper, middle, 0);
        assert!(validate_record(
            &buf,
            0,
            buf.len(),
            Some(TimestampFormat::Irig)
        ));
    }

    #[test]
    fn validate_rejects_irig_microsecond_at_one_million() {
        // microsecond = 1_000_000 = (0xF << 16) | 0x4240 = 0xF4240.
        // us_hi4 = 0xF, us_lo16 = 0x4240. Rejected per L2-SYN-004.
        let upper = irig_upper(false, 192, 15);
        let middle = irig_middle(54, 50, 0xF);
        let lower = 0x4240u16;
        let buf = make_irig_record_with_ts(upper, middle, lower);
        assert!(!validate_record(
            &buf,
            0,
            buf.len(),
            Some(TimestampFormat::Irig)
        ));
    }

    #[test]
    fn validate_accepts_irig_microsecond_at_max_valid() {
        // microsecond = 999_999 = (0xF << 16) | 0x423F = 0xF423F.
        let upper = irig_upper(false, 192, 15);
        let middle = irig_middle(54, 50, 0xF);
        let lower = 0x423Fu16;
        let buf = make_irig_record_with_ts(upper, middle, lower);
        assert!(validate_record(
            &buf,
            0,
            buf.len(),
            Some(TimestampFormat::Irig)
        ));
    }

    #[test]
    fn validate_rejects_irig_microsecond_when_freerun_too() {
        // L2-SYN-004a only relaxes the DAY check, not microsecond.
        // freerun=true with out-of-range microseconds is still rejected.
        let upper = irig_upper(true, 0, 15);
        let middle = irig_middle(54, 50, 0xF);
        let lower = 0x4240u16; // 1_000_000
        let buf = make_irig_record_with_ts(upper, middle, lower);
        assert!(!validate_record(
            &buf,
            0,
            buf.len(),
            Some(TimestampFormat::Irig)
        ));
    }

    #[test]
    fn min_word_count_helper() {
        assert_eq!(min_word_count(Some(TimestampFormat::Irig)), 5);
        assert_eq!(min_word_count(Some(TimestampFormat::Standard)), 4);
        assert_eq!(min_word_count(None), MIN_RECORD_WORDS_STANDARD);
    }
}
