//! Streaming CSV writer.
//!
//! Rows are written directly to a `Write` impl as they are produced — no
//! intermediate `Vec<Row>` or DataFrame buffering. Memory usage is constant
//! regardless of file size.
//!
//! Column order matches the DDC vendor CSV byte-for-byte. The `MUX`,
//! `TERM_NAME`, `IM_GAP`, `RCV_GAP`, `XMT_GAP` columns are emitted as empty
//! strings — they exist for compatibility, not because we populate them.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use crate::error::{MieError, MieResult};
use crate::models::{MAX_DATA_WORDS, MieMessage};
use crate::{log_info, log_warn};

/// Header row written before the first data row. Public so callers can
/// embed it elsewhere if needed.
pub const CSV_HEADER: &str = concat!(
    "TIME_STAMP,RT,MSG,",
    "WD01,WD02,WD03,WD04,WD05,WD06,WD07,WD08,WD09,WD10,",
    "WD11,WD12,WD13,WD14,WD15,WD16,WD17,WD18,WD19,WD20,",
    "WD21,WD22,WD23,WD24,WD25,WD26,WD27,WD28,WD29,WD30,",
    "WD31,WD32,",
    "STAT,CMD,MUX,TERM_NAME,BUS,DELTA,ERROR,ERROR_CODE,IM_GAP,RCV_GAP,XMT_GAP\n",
);

/// Streaming CSV row writer.
pub struct CsvWriter<W: Write> {
    out: W,
    rows_written: u64,
    destination: String,
}

impl<W: Write> CsvWriter<W> {
    /// Create a writer and emit the header row immediately.
    pub fn new(out: W, destination: impl Into<String>) -> MieResult<Self> {
        let mut w = Self {
            out,
            rows_written: 0,
            destination: destination.into(),
        };
        w.write_str(CSV_HEADER)?;
        Ok(w)
    }

    pub fn write_message(&mut self, msg: &MieMessage) -> MieResult<()> {
        write_row(&mut self.out, msg).map_err(|source| MieError::WriterError {
            destination: self.destination.clone(),
            source,
        })?;
        self.rows_written += 1;
        Ok(())
    }

    pub fn finish(mut self) -> MieResult<u64> {
        self.out.flush().map_err(|source| MieError::WriterError {
            destination: self.destination.clone(),
            source,
        })?;
        Ok(self.rows_written)
    }

    pub fn rows_written(&self) -> u64 {
        self.rows_written
    }

    fn write_str(&mut self, s: &str) -> MieResult<()> {
        self.out
            .write_all(s.as_bytes())
            .map_err(|source| MieError::WriterError {
                destination: self.destination.clone(),
                source,
            })
    }
}

fn write_row<W: Write>(out: &mut W, msg: &MieMessage) -> std::io::Result<()> {
    // TIME_STAMP
    out.write_all(msg.timestamp.format().as_bytes())?;
    out.write_all(b",")?;

    // RT
    if let Some(rt) = msg.rt() {
        write!(out, "{rt}")?;
    }
    out.write_all(b",")?;

    // MSG
    out.write_all(msg.msg_label().as_bytes())?;
    out.write_all(b",")?;

    // WD01..WD32
    for i in 0..MAX_DATA_WORDS {
        if let Some(&w) = msg.data_words.as_slice().get(i) {
            write!(out, "{w:04X}")?;
        }
        out.write_all(b",")?;
    }

    // STAT
    if let Some(s) = msg.status_word {
        write!(out, "{s:04X}")?;
    }
    out.write_all(b",")?;

    // CMD
    if let Some(cw) = msg.command_word {
        write!(out, "{:04X}", cw.raw)?;
    }
    out.write_all(b",")?;

    // MUX, TERM_NAME (always empty)
    out.write_all(b",,")?;

    // BUS
    out.write_all(msg.bus().as_str().as_bytes())?;
    out.write_all(b",")?;

    // DELTA
    write!(out, "{:.6}", msg.delta)?;
    out.write_all(b",")?;

    // ERROR
    out.write_all(msg.error_label().as_bytes())?;
    out.write_all(b",")?;

    // ERROR_CODE
    if let Some(c) = msg.error_word {
        write!(out, "{c:04X}")?;
    }
    out.write_all(b",")?;

    // IM_GAP, RCV_GAP, XMT_GAP (always empty)
    out.write_all(b",,\n")?;

    Ok(())
}

// ── Top-level entry points matching the Python API ────────────────────

/// Stream `messages` to a single CSV. Errors and spurious records are
/// included with their ERROR / ERROR_CODE columns populated (INLINE mode,
/// or stdout where splitting is not possible).
///
/// `output` may be `None` for stdout.
pub fn write_csv<I>(messages: I, output: Option<&Path>) -> MieResult<u64>
where
    I: IntoIterator<Item = MieResult<MieMessage>>,
{
    match output {
        Some(path) => {
            let file = File::create(path).map_err(|source| MieError::WriterError {
                destination: path.display().to_string(),
                source,
            })?;
            let buf = BufWriter::new(file);
            let mut writer = CsvWriter::new(buf, path.display().to_string())?;
            stream_into(&mut writer, messages)?;
            let n = writer.finish()?;
            log_info!("wrote {} rows to {}", n, path.display());
            Ok(n)
        }
        None => {
            let stdout = std::io::stdout();
            let buf = BufWriter::new(stdout.lock());
            let mut writer = CsvWriter::new(buf, "stdout".to_string())?;
            stream_into(&mut writer, messages)?;
            let n = writer.finish()?;
            log_info!("wrote {} rows to stdout", n);
            Ok(n)
        }
    }
}

/// Split-output streaming: normal records to `output`, errored / spurious
/// to `<stem>_errors<ext>`. Only opens the error file lazily on first error
/// row, so files with no errors don't produce an empty `_errors.csv`.
///
/// Returns `(normal_count, error_count)`.
pub fn write_csv_split<I>(messages: I, output: &Path) -> MieResult<(u64, u64)>
where
    I: IntoIterator<Item = MieResult<MieMessage>>,
{
    let main_file = File::create(output).map_err(|source| MieError::WriterError {
        destination: output.display().to_string(),
        source,
    })?;
    let mut main = CsvWriter::new(BufWriter::new(main_file), output.display().to_string())?;

    let error_path = error_path_for(output);
    let mut errors: Option<CsvWriter<BufWriter<File>>> = None;

    for item in messages {
        let msg = item?;
        if !msg.error_label().is_empty() {
            if errors.is_none() {
                let f = File::create(&error_path).map_err(|source| MieError::WriterError {
                    destination: error_path.display().to_string(),
                    source,
                })?;
                errors = Some(CsvWriter::new(
                    BufWriter::new(f),
                    error_path.display().to_string(),
                )?);
            }
            errors.as_mut().unwrap().write_message(&msg)?;
        } else {
            main.write_message(&msg)?;
        }
    }

    let normal_count = main.rows_written();
    main.finish()?;

    let error_count = match errors {
        Some(w) => {
            let n = w.rows_written();
            w.finish()?;
            log_info!(
                "wrote {} error/spurious rows to {}",
                n,
                error_path.display()
            );
            n
        }
        None => {
            log_info!("no error/spurious records — error file not created");
            0
        }
    };

    log_info!("wrote {} normal rows to {}", normal_count, output.display());
    if normal_count == 0 {
        log_warn!("main CSV is empty (header only)");
    }

    Ok((normal_count, error_count))
}

fn stream_into<W, I>(writer: &mut CsvWriter<W>, messages: I) -> MieResult<()>
where
    W: Write,
    I: IntoIterator<Item = MieResult<MieMessage>>,
{
    for item in messages {
        let msg = item?;
        writer.write_message(&msg)?;
    }
    Ok(())
}

fn error_path_for(output: &Path) -> std::path::PathBuf {
    let stem = output
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = output.extension().map(|e| e.to_string_lossy().into_owned());
    let parent = output.parent().unwrap_or_else(|| Path::new(""));
    let name = match ext {
        Some(e) if !e.is_empty() => format!("{stem}_errors.{e}"),
        _ => format!("{stem}_errors"),
    };
    parent.join(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::*;

    fn sample_msg() -> MieMessage {
        MieMessage {
            timestamp: Timestamp::Irig(IrigTimestamp {
                day: 10,
                hour: 15,
                minute: 54,
                second: 50,
                microsecond: 456_225,
                freerun: false,
            }),
            type_word: TypeWord {
                message_type: 0x02,
                bus: Bus::A,
                word_count: 36,
                error: false,
                raw: 0x2402,
            },
            message_format: MessageFormat::Receive,
            command_word: Some(CommandWord {
                rt: 15,
                direction: Direction::Receive,
                subaddress: 11,
                data_word_count: 30,
                raw: 0x797E,
            }),
            command_word_2: None,
            status_word: Some(0x7800),
            status_word_2: None,
            data_words: DataWords::from_slice(&[0x0400, 0x0000, 0x0000, 0x002F]),
            error_word: None,
            delta: 0.123456,
            file_offset: 0,
        }
    }

    #[test]
    fn header_present() {
        let mut buf = Vec::new();
        let writer = CsvWriter::new(&mut buf, "memory").unwrap();
        writer.finish().unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.starts_with("TIME_STAMP,RT,MSG,WD01"));
        assert!(s.trim_end().ends_with("XMT_GAP"));
    }

    #[test]
    fn row_format_matches_python_layout() {
        let mut buf = Vec::new();
        let mut w = CsvWriter::new(&mut buf, "memory").unwrap();
        w.write_message(&sample_msg()).unwrap();
        let n = w.finish().unwrap();
        assert_eq!(n, 1);

        let s = String::from_utf8(buf).unwrap();
        let mut lines = s.lines();
        let _header = lines.next().unwrap();
        let row = lines.next().unwrap();

        // Spot-check critical fields
        assert!(row.starts_with("10:15:54:50.456225,15,11R,"));
        assert!(row.contains("0400,0000,0000,002F,"));
        // Status, Cmd
        assert!(row.contains(",7800,797E,"));
        // Bus, Delta
        assert!(row.contains(",A,0.123456,"));
        // Trailing empty MUX, TERM_NAME, ERROR, ERROR_CODE, gap columns
        assert!(row.ends_with(",,,,"));
    }

    #[test]
    fn data_words_padded_to_32() {
        let mut buf = Vec::new();
        let mut w = CsvWriter::new(&mut buf, "memory").unwrap();
        w.write_message(&sample_msg()).unwrap();
        w.finish().unwrap();
        let s = String::from_utf8(buf).unwrap();
        let row = s.lines().nth(1).unwrap();
        // Count commas before STAT (should be: 3 fixed + 32 WD + 1 = 36 commas before STAT value)
        let commas: usize = row.matches(',').count();
        // Total fields in row = 3 + 32 + 11 = 46; commas = 45
        assert_eq!(commas, 45);
    }

    #[test]
    fn error_path_naming() {
        assert_eq!(
            error_path_for(Path::new("out.csv")),
            Path::new("out_errors.csv")
        );
        assert_eq!(
            error_path_for(Path::new("data/x/out.csv")),
            Path::new("data/x/out_errors.csv")
        );
        assert_eq!(error_path_for(Path::new("out")), Path::new("out_errors"));
    }
}
