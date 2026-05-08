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
use crate::models::{Direction, MessageType};

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
    file.read_to_end(&mut buf).map_err(|source| MieError::FileIo {
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

fn write_hex_line<W: Write>(out: &mut W, indent: &str, addr: usize, chunk: &[u8]) -> std::io::Result<()> {
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
        let c = if (32..127).contains(&b) { b as char } else { '.' };
        out.write_all(&[c as u8])?;
    }
    out.write_all(b"|\n")
}

/// Print a raw hex+ASCII dump of `[start_offset, start_offset + length)`.
pub fn hex_dump_raw(
    path: &Path,
    start_offset: usize,
    length: Option<usize>,
    mut out: impl Write,
) -> MieResult<()> {
    let data = read_file(path)?;
    let end = match length {
        Some(n) => (start_offset + n).min(data.len()),
        None => data.len(),
    };
    let chunk = &data[start_offset.min(data.len())..end];

    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let _ = writeln!(out, "File: {name} ({} bytes)", data.len());
    let _ = writeln!(out, "Range: 0x{:08X}-0x{:08X}", start_offset, end);
    let _ = writeln!(out);

    let mut i = 0;
    while i < chunk.len() {
        let line = &chunk[i..(i + 16).min(chunk.len())];
        let _ = write_hex_line(&mut out, "  ", start_offset + i, line);
        i += 16;
    }
    Ok(())
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
    let file_len = data.len();
    let mut offset = start_offset;
    let mut record_num: u64 = 0;

    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let _ = writeln!(out, "File: {name} ({file_len} bytes)");
    let _ = writeln!(out, "Record dump starting at offset 0x{:08X}", start_offset);
    let _ = writeln!(out);

    while offset + MIN_RECORD_BYTES <= file_len {
        if let Some(max) = max_records {
            if record_num >= max {
                break;
            }
        }

        let type_raw = match read_u16(&data, offset) {
            Some(v) => v,
            None => break,
        };
        let tw = decode_type_word(type_raw);

        if tw.word_count < MIN_RECORD_WORDS {
            let _ = writeln!(
                out,
                "  !! Invalid word_count={} at 0x{:08X}, stopping",
                tw.word_count, offset
            );
            break;
        }
        let record_bytes = usize::from(tw.word_count) * 2;
        if offset + record_bytes > file_len {
            let _ = writeln!(
                out,
                "  !! Truncated record at 0x{:08X} ({} bytes needed, {} available)",
                offset, record_bytes, file_len - offset
            );
            break;
        }

        // Decode IRIG-shaped header for annotation. (For Standard timestamps
        // this is still useful to display the raw bytes; the IRIG decode is
        // a best-effort summary.)
        let ts_upper = read_u16(&data, offset + 2).unwrap_or(0);
        let ts_middle = read_u16(&data, offset + 4).unwrap_or(0);
        let ts_lower = read_u16(&data, offset + 6).unwrap_or(0);
        let ts = decode_irig_timestamp(ts_upper, ts_middle, ts_lower);
        let cmd_raw = read_u16(&data, offset + 8).unwrap_or(0);
        let cmd = decode_command_word(cmd_raw);

        let bus_label = tw.bus.as_str();
        let err_label = if tw.error { "ERROR" } else { "OK" };
        let dir_char = if cmd.direction == Direction::Transmit {
            'T'
        } else {
            'R'
        };

        let _ = writeln!(out, "{}", "-".repeat(72));
        let _ = writeln!(
            out,
            "  Record #{record_num}  @  0x{offset:08X}  ({record_bytes} bytes, {wc} words)",
            wc = tw.word_count
        );
        let _ = writeln!(
            out,
            "  Type: 0x{:04X}  ->  {}  Bus {}  {}",
            tw.raw,
            type_name(tw.message_type),
            bus_label,
            err_label
        );
        let _ = writeln!(
            out,
            "  Time: {}{}",
            ts.format(),
            if ts.freerun { "  [FREERUN]" } else { "" }
        );
        let _ = writeln!(
            out,
            "  Cmd:  0x{:04X}  ->  RT{} SA{} {} WC={}",
            cmd.raw, cmd.rt, cmd.subaddress, dir_char, cmd.data_word_count
        );

        let record_data = &data[offset..offset + record_bytes];
        let mut i = 0;
        while i < record_data.len() {
            let line = &record_data[i..(i + 16).min(record_data.len())];
            let _ = write_hex_line(&mut out, "    ", offset + i, line);
            i += 16;
        }
        let _ = writeln!(out);

        offset += record_bytes;
        record_num += 1;
    }

    let _ = writeln!(out, "{}", "-".repeat(72));
    let _ = writeln!(out, "{record_num} records dumped.");
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

    #[test]
    fn missing_file_returns_error() {
        let mut out = Vec::new();
        let err = hex_dump_raw(Path::new("/no/such/file.bin"), 0, None, &mut out).unwrap_err();
        assert_eq!(err.kind(), crate::error::MieErrorKind::FileNotFound);
    }
}
