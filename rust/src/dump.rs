//! Hex dump utilities. Two modes:
//!   - raw: classic hex+ASCII view of any byte range
//!   - record-aware: parses Type Word + IRIG timestamp + Command Word and
//!     prints an annotated header per record before the bytes
//!
//! Mirrors the Python `dump.py` output format closely enough for diffing.

use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::path::Path;

use crate::decode::{
    MIN_RECORD_BYTES, MIN_RECORD_WORDS, decode_command_word, decode_irig_timestamp,
    decode_type_word, read_u16,
};
use crate::error::{MieError, MieResult};
use crate::log_warn;
use crate::models::{Direction, MessageType, TypeWord};

fn type_name(code: u8) -> String {
    match MessageType::from_code(code) {
        Some(MessageType::ModeCommand) => "Mode Command".into(),
        Some(MessageType::BcToRt) => "BC->RT (Receive)".into(),
        Some(MessageType::RtToBc) => "RT->BC (Transmit)".into(),
        Some(MessageType::RtToRt) => "RT->RT".into(),
        Some(MessageType::BroadcastBcToRt) => "Broadcast BC->RT".into(),
        Some(MessageType::BroadcastRtToRt) => "Broadcast RT->RT".into(),
        Some(MessageType::SpuriousData) => "Spurious Data".into(),
        None => format!("UNKNOWN(0x{code:02X})"),
    }
}

fn read_file(path: &Path) -> MieResult<Vec<u8>> {
    if !path.exists() {
        return Err(MieError::FileNotFound {
            path: path.to_path_buf(),
        });
    }
    let mut file = File::open(path).map_err(|source| MieError::FileIo {
        path: path.to_path_buf(),
        source,
    })?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)
        .map_err(|source| MieError::FileIo {
            path: path.to_path_buf(),
            source,
        })?;
    if buf.is_empty() {
        return Err(MieError::FileEmpty {
            path: path.to_path_buf(),
        });
    }
    Ok(buf)
}

/// Map an output `io::Error` to a `MieError::WriterError`. The dump always
/// writes to stdout in production (the `*_to_stdout` wrappers), so the
/// destination is labelled accordingly. The underlying error kind is
/// preserved so the CLI can treat a broken pipe as a clean exit (L2-WRT-018).
fn writer_error(source: std::io::Error) -> MieError {
    MieError::WriterError {
        destination: "stdout".to_string(),
        source,
    }
}

fn write_hex_line<W: Write>(
    out: &mut W,
    indent: &str,
    addr: usize,
    chunk: &[u8],
) -> std::io::Result<()> {
    write!(out, "{indent}{addr:08X}  ")?;
    let mut hex_part = String::with_capacity(48);
    for b in chunk {
        hex_part.push_str(&format!("{b:02X} "));
    }
    if hex_part.ends_with(' ') {
        hex_part.pop();
    }
    write!(out, "{hex_part:<48}  |")?;
    for &b in chunk {
        let c = if (32..127).contains(&b) {
            b as char
        } else {
            '.'
        };
        out.write_all(&[c as u8])?;
    }
    out.write_all(b"|\n")
}

/// Print a raw hex+ASCII dump of `[start_offset, start_offset + length)`.
///
/// Inputs are user-supplied (`--offset`, `--length`); both default to
/// `usize::MAX`-tolerant arithmetic. A start beyond EOF or a length
/// that would overflow simply yields an empty dump rather than panicking.
pub fn hex_dump_raw(
    path: &Path,
    start_offset: usize,
    length: Option<usize>,
    mut out: impl Write,
) -> MieResult<()> {
    let data = read_file(path)?;
    write_hex_dump_raw(&data, path, start_offset, length, &mut out).map_err(writer_error)
}

/// Inner writer for `hex_dump_raw`. Returns `io::Result` so every write —
/// including the final flush — propagates; the public wrapper maps the
/// failure to `MieError::WriterError`. Previously these writes were
/// discarded with `let _ =`, so an output failure (disk full, broken pipe)
/// was silently reported as success (L2-WRT-018).
fn write_hex_dump_raw<W: Write>(
    data: &[u8],
    path: &Path,
    start_offset: usize,
    length: Option<usize>,
    out: &mut W,
) -> std::io::Result<()> {
    let file_len = data.len();

    // Clamp start to [0, file_len] up-front; reads at start >= file_len
    // produce an empty chunk.
    let chunk_start = start_offset.min(file_len);
    // saturating_add clamps to usize::MAX on overflow; .min(file_len)
    // then bounds it. .max(chunk_start) handles the case where length=0
    // or arithmetic produced an end before start.
    let end = match length {
        Some(n) => start_offset.saturating_add(n).min(file_len),
        None => file_len,
    }
    .max(chunk_start);
    let chunk = &data[chunk_start..end];

    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    writeln!(out, "File: {name} ({} bytes)", file_len)?;
    writeln!(out, "Range: 0x{:08X}-0x{:08X}", chunk_start, end)?;
    writeln!(out)?;

    let mut i = 0;
    while i < chunk.len() {
        let line = &chunk[i..(i + 16).min(chunk.len())];
        // chunk_start is bounded by file_len; i is bounded by chunk.len()
        // which is bounded by file_len - chunk_start. Sum can't overflow.
        write_hex_line(out, "  ", chunk_start.saturating_add(i), line)?;
        i += 16;
    }
    out.flush()
}

/// Print a record-aware hex dump. Each record is preceded by a one-line
/// header summarising decoded Type Word, timestamp, and Command Word.
pub fn hex_dump_records(
    path: &Path,
    max_records: Option<u64>,
    start_offset: usize,
    mut out: impl Write,
) -> MieResult<()> {
    let data = read_file(path)?;
    write_hex_dump_records(&data, path, max_records, start_offset, &mut out).map_err(writer_error)
}

/// Inner writer for `hex_dump_records`; see `write_hex_dump_raw` for why
/// this returns `io::Result` (every write, including the trailing flush,
/// must propagate so an output failure is not silently swallowed).
fn write_hex_dump_records<W: Write>(
    data: &[u8],
    path: &Path,
    max_records: Option<u64>,
    start_offset: usize,
    out: &mut W,
) -> std::io::Result<()> {
    let file_len = data.len();
    let mut offset = start_offset;
    let mut record_num: u64 = 0;

    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    writeln!(out, "File: {name} ({file_len} bytes)")?;
    writeln!(out, "Record dump starting at offset 0x{:08X}", start_offset)?;
    writeln!(out)?;

    // Loop guard uses checked_add so a start_offset of usize::MAX (or a
    // file shorter than MIN_RECORD_BYTES) exits cleanly instead of
    // wrapping arithmetic and panicking.
    while let Some(min_end) = offset.checked_add(MIN_RECORD_BYTES) {
        if min_end > file_len {
            break;
        }
        if let Some(max) = max_records
            && record_num >= max
        {
            break;
        }

        let type_raw = match read_u16(data, offset) {
            Some(v) => v,
            None => break,
        };
        let tw = decode_type_word(type_raw);

        // Validate the record's extent; a stop reason is written inline and
        // logged (L2-CLI-013) inside the helper, so here we just break.
        let record_end = match dump_record_extent(&tw, offset, file_len, out)? {
            Some(end) => end,
            None => break,
        };
        let record_bytes = record_end - offset;

        write_record_annotation(out, data, &tw, offset, record_bytes, record_num)?;
        write_record_hex_payload(out, &data[offset..record_end], offset)?;
        writeln!(out)?;

        // offset can advance to record_end; checked again at the top of the
        // next iteration.
        offset = record_end;
        record_num += 1;
    }

    writeln!(out, "{}", "-".repeat(72))?;
    writeln!(out, "{record_num} records dumped.")?;
    out.flush()
}

/// Validate the record at `offset` for the dump scan. Returns `Some(record_end)`
/// to proceed, or `None` to stop scanning — writing the inline anomaly note to
/// `out` and logging it (L2-CLI-013) on each stop path.
fn dump_record_extent<W: Write>(
    tw: &TypeWord,
    offset: usize,
    file_len: usize,
    out: &mut W,
) -> std::io::Result<Option<usize>> {
    if tw.word_count < MIN_RECORD_WORDS {
        writeln!(
            out,
            "  !! Invalid word_count={} at 0x{:08X}, stopping",
            tw.word_count, offset
        )?;
        log_warn!(
            "dump: invalid word_count={} at 0x{:X}; stopping record scan",
            tw.word_count,
            offset
        );
        return Ok(None);
    }
    let record_bytes = usize::from(tw.word_count) * 2;
    // record_bytes maxes at 63 * 2 = 126; offset is bounded by the loop
    // guard. checked_add belt-and-suspenders.
    let Some(record_end) = offset.checked_add(record_bytes) else {
        writeln!(
            out,
            "  !! Offset overflow at 0x{:08X} (record_bytes={}), stopping",
            offset, record_bytes
        )?;
        log_warn!(
            "dump: offset overflow at 0x{:X} (record_bytes={}); stopping record scan",
            offset,
            record_bytes
        );
        return Ok(None);
    };
    if record_end > file_len {
        writeln!(
            out,
            "  !! Truncated record at 0x{:08X} ({} bytes needed, {} available)",
            offset,
            record_bytes,
            file_len - offset
        )?;
        log_warn!(
            "dump: truncated record at 0x{:X} ({} bytes needed, {} available); \
             stopping record scan",
            offset,
            record_bytes,
            file_len - offset
        );
        return Ok(None);
    }
    Ok(Some(record_end))
}

/// Write the decoded-header annotation block (Type / Time / Cmd) for one record.
/// The IRIG timestamp decode is a best-effort summary — for Standard-format
/// files the raw bytes below the header remain authoritative.
fn write_record_annotation<W: Write>(
    out: &mut W,
    data: &[u8],
    tw: &TypeWord,
    offset: usize,
    record_bytes: usize,
    record_num: u64,
) -> std::io::Result<()> {
    let ts_upper = read_u16(data, offset + 2).unwrap_or(0);
    let ts_middle = read_u16(data, offset + 4).unwrap_or(0);
    let ts_lower = read_u16(data, offset + 6).unwrap_or(0);
    let ts = decode_irig_timestamp(ts_upper, ts_middle, ts_lower);
    let cmd = decode_command_word(read_u16(data, offset + 8).unwrap_or(0));
    let dir_char = if cmd.direction == Direction::Transmit {
        'T'
    } else {
        'R'
    };

    writeln!(out, "{}", "-".repeat(72))?;
    writeln!(
        out,
        "  Record #{record_num}  @  0x{offset:08X}  ({record_bytes} bytes, {wc} words)",
        wc = tw.word_count
    )?;
    writeln!(
        out,
        "  Type: 0x{:04X}  ->  {}  Bus {}  {}",
        tw.raw,
        type_name(tw.message_type),
        tw.bus.as_str(),
        if tw.error { "ERROR" } else { "OK" }
    )?;
    writeln!(
        out,
        "  Time: {}{}",
        ts.format(),
        if ts.freerun { "  [FREERUN]" } else { "" }
    )?;
    writeln!(
        out,
        "  Cmd:  0x{:04X}  ->  RT{} SA{} {} WC={}",
        cmd.raw, cmd.rt, cmd.subaddress, dir_char, cmd.data_word_count
    )
}

/// Hex-dump a record's raw bytes, 16 per line, each line offset-annotated.
fn write_record_hex_payload<W: Write>(
    out: &mut W,
    record_data: &[u8],
    offset: usize,
) -> std::io::Result<()> {
    let mut i = 0;
    while i < record_data.len() {
        let line = &record_data[i..(i + 16).min(record_data.len())];
        // offset + i: offset is bounded by the caller's loop guard, i by
        // record_bytes (≤ 126). saturating_add belt-and-suspenders.
        write_hex_line(out, "    ", offset.saturating_add(i), line)?;
        i += 16;
    }
    Ok(())
}

/// Convenience wrapper that writes to stdout (buffered).
pub fn hex_dump_raw_to_stdout(path: &Path, offset: usize, length: Option<usize>) -> MieResult<()> {
    let stdout = std::io::stdout();
    let buf = BufWriter::new(stdout.lock());
    hex_dump_raw(path, offset, length, buf)
}

pub fn hex_dump_records_to_stdout(
    path: &Path,
    max_records: Option<u64>,
    offset: usize,
) -> MieResult<()> {
    let stdout = std::io::stdout();
    let buf = BufWriter::new(stdout.lock());
    hex_dump_records(path, max_records, offset, buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TempFile(std::path::PathBuf);
    impl TempFile {
        fn write(bytes: &[u8]) -> Self {
            static C: AtomicU64 = AtomicU64::new(0);
            let n = C.fetch_add(1, Ordering::Relaxed);
            let pid = std::process::id();
            let p = std::env::temp_dir().join(format!("mie-dump-test-{pid}-{n}.bin"));
            std::fs::write(&p, bytes).unwrap();
            Self(p)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    /// Requirements: L2-CLI-009
    #[test]
    fn raw_dump_format() {
        let f = TempFile::write(b"AB\x00\x01\x02\x03");
        let mut out = Vec::new();
        hex_dump_raw(f.path(), 0, None, &mut out).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("00000000"));
        assert!(s.contains("41 42")); // "AB"
        assert!(s.contains("|AB"));
    }

    /// Requirements: L2-CLI-009
    #[test]
    fn record_dump_handles_one_record() {
        // Minimal valid record: type 0x2402, then 35 zero words = 70 bytes
        let mut buf = Vec::with_capacity(72);
        buf.extend_from_slice(&[0x02, 0x24]);
        buf.extend_from_slice(&[0u8; 70]);
        let f = TempFile::write(&buf);
        let mut out = Vec::new();
        hex_dump_records(f.path(), Some(1), 0, &mut out).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("Record #0"));
        assert!(s.contains("BC->RT (Receive)"));
        assert!(s.contains("1 records dumped"));
    }

    /// L2-CLI-013: a scan-stop anomaly emits a logger WARN in addition to the
    /// inline report note. The crate logger writes to process stderr and is
    /// not capturable in-process, so this asserts the inline note (the same
    /// branch that emits `log_warn!`); the WARN emission is verified by
    /// inspection and exercised live by `dump_arbitrary_bytes_never_panics`
    /// under `--nocapture`. Mirrors the Python `test_dump_logs_warning_on_truncated_record`.
    /// Requirements: L2-CLI-013
    #[test]
    fn record_dump_notes_and_warns_truncated_record() {
        // Type 0x2402 declares word_count=36 (72 bytes) but only 20 bytes
        // exist → the record-aware scan hits the truncated-record branch.
        let mut buf = Vec::with_capacity(20);
        buf.extend_from_slice(&[0x02, 0x24]);
        buf.extend_from_slice(&[0u8; 18]);
        let f = TempFile::write(&buf);
        let mut out = Vec::new();
        hex_dump_records(f.path(), Some(1), 0, &mut out).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("!! Truncated record"));
        assert!(s.contains("72 bytes needed, 20 available"));
    }

    /// Requirements: L2-RDR-005
    #[test]
    fn missing_file_returns_error() {
        let mut out = Vec::new();
        let err = hex_dump_raw(Path::new("/no/such/file.bin"), 0, None, &mut out).unwrap_err();
        assert_eq!(err.kind(), crate::error::MieErrorKind::FileNotFound);
    }

    /// Regression test: `--raw --offset usize::MAX --length 1` must not
    /// panic on the `start_offset + n` computation. The fix uses
    /// saturating_add and clamps to file_len; the result is an empty
    /// dump rather than a crash.
    /// Requirements: L1-ROB-001
    #[test]
    fn raw_dump_offset_max_length_one_does_not_panic() {
        let f = TempFile::write(b"AB\x00\x01\x02\x03");
        let mut out = Vec::new();
        hex_dump_raw(f.path(), usize::MAX, Some(1), &mut out).unwrap();
        let s = String::from_utf8(out).unwrap();
        // Header is still printed; the data range is empty.
        assert!(s.contains("File:"));
    }

    /// Requirements: L1-ROB-001
    #[test]
    fn raw_dump_offset_max_length_max_does_not_panic() {
        let f = TempFile::write(b"AB\x00\x01");
        let mut out = Vec::new();
        hex_dump_raw(f.path(), usize::MAX, Some(usize::MAX), &mut out).unwrap();
        // No assertion on contents; the test passes if no panic.
    }

    /// Requirements: L2-CLI-009
    #[test]
    fn raw_dump_offset_beyond_eof_yields_empty() {
        let f = TempFile::write(b"AB\x00\x01");
        let mut out = Vec::new();
        hex_dump_raw(f.path(), 1000, Some(16), &mut out).unwrap();
        let s = String::from_utf8(out).unwrap();
        // No hex line in the body; only the header.
        assert!(!s.contains("41 42"));
    }

    /// Regression test for the record-mode loop guard.
    /// `offset + MIN_RECORD_BYTES <= file_len` overflows if offset is
    /// near usize::MAX; the fix uses checked_add so the loop simply
    /// doesn't enter.
    /// Requirements: L1-ROB-001
    #[test]
    fn record_dump_offset_max_does_not_panic() {
        let f = TempFile::write(b"AB\x00\x01\x02\x03\x04\x05\x06\x07\x08\x09");
        let mut out = Vec::new();
        hex_dump_records(f.path(), Some(1), usize::MAX, &mut out).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("0 records dumped"));
    }

    /// Requirements: L1-ROB-001
    #[test]
    fn record_dump_offset_just_short_of_max_does_not_panic() {
        let f = TempFile::write(b"AB\x00\x01\x02\x03\x04\x05\x06\x07\x08\x09");
        let mut out = Vec::new();
        // offset large enough that offset + MIN_RECORD_BYTES overflows
        // but offset itself does not.
        hex_dump_records(f.path(), Some(1), usize::MAX - 4, &mut out).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("0 records dumped"));
    }

    /// A `Write` that fails every write/flush with a configurable error
    /// kind — used to prove the dump surfaces output I/O failures rather
    /// than reporting success (L2-WRT-018).
    struct FailingWriter(std::io::ErrorKind);
    impl Write for FailingWriter {
        fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(self.0, "injected write failure"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Err(std::io::Error::new(self.0, "injected flush failure"))
        }
    }

    /// Requirements: L2-WRT-018
    #[test]
    fn raw_dump_surfaces_write_failure() {
        let f = TempFile::write(b"AB\x00\x01");
        let err =
            hex_dump_raw(f.path(), 0, None, FailingWriter(std::io::ErrorKind::Other)).unwrap_err();
        assert_eq!(err.kind(), crate::error::MieErrorKind::WriterError);
    }

    /// Requirements: L2-WRT-018
    #[test]
    fn records_dump_surfaces_write_failure() {
        let mut buf = Vec::with_capacity(72);
        buf.extend_from_slice(&[0x02, 0x24]);
        buf.extend_from_slice(&[0u8; 70]);
        let f = TempFile::write(&buf);
        let err = hex_dump_records(
            f.path(),
            Some(1),
            0,
            FailingWriter(std::io::ErrorKind::Other),
        )
        .unwrap_err();
        assert_eq!(err.kind(), crate::error::MieErrorKind::WriterError);
    }

    /// A broken pipe on the dump's stdout must be *classified* as such so
    /// the CLI can exit 0 (L2-WRT-018) rather than fail.
    /// Requirements: L2-WRT-018
    #[test]
    fn dump_broken_pipe_is_classified_for_exit_zero() {
        let f = TempFile::write(b"AB\x00\x01");
        let err = hex_dump_raw(
            f.path(),
            0,
            None,
            FailingWriter(std::io::ErrorKind::BrokenPipe),
        )
        .unwrap_err();
        assert!(
            err.is_broken_pipe(),
            "broken pipe must be classified so run_dump can exit 0 (L2-WRT-018)"
        );
    }
}
