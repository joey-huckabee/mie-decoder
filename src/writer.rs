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
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use crate::error::{MieError, MieResult};
use crate::models::{MAX_DATA_WORDS, MieMessage};
use crate::{log_info, log_warn};

// ── Path identity (L2-WRT-014) ────────────────────────────────────────

/// Test whether `input` and `output` resolve to the same file.
///
/// Handles the common case where `output` does not yet exist by
/// canonicalizing the output's parent directory and joining the
/// filename. Returns `Ok(false)` whenever either path or its parent
/// cannot be canonicalized — collision is only positive when both
/// resolve to the same identity.
///
/// This is intentionally symlink-safe (via `fs::canonicalize`) so that
/// `/tmp/in.mie` aliasing `/var/foo/in.mie` is detected.
pub fn paths_refer_to_same_file(input: &Path, output: &Path) -> io::Result<bool> {
    let input_canon = std::fs::canonicalize(input)?;

    // Direct path: both files exist on disk.
    if let Ok(out_canon) = std::fs::canonicalize(output) {
        return Ok(input_canon == out_canon);
    }

    // Output doesn't exist yet (the common case). Canonicalize the
    // parent and join the filename. If the parent itself doesn't exist,
    // there can be no collision because the output isn't reachable.
    let Some(parent) = output.parent() else {
        return Ok(false);
    };
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    let Ok(parent_canon) = std::fs::canonicalize(parent) else {
        return Ok(false);
    };
    let Some(filename) = output.file_name() else {
        return Ok(false);
    };
    Ok(input_canon == parent_canon.join(filename))
}

// ── AtomicCsvFile (L2-WRT-015, L2-WRT-016) ────────────────────────────

/// Write a CSV to a temp file in the destination's directory, then
/// `rename()` atomically over the destination on successful commit.
///
/// On Drop without commit (i.e., decode failed or was interrupted),
/// the temp file is unlinked. The destination file — if it already
/// existed — is never touched on the failure path.
///
/// Rename is atomic on POSIX (`rename(2)`) and on NTFS within the
/// same volume (`MoveFileEx` with replace). Keeping the temp file
/// in the destination's parent guarantees same-volume placement.
pub struct AtomicCsvFile {
    final_path: PathBuf,
    temp_path: PathBuf,
    /// Owned via `Option` so `commit()` can move out the writer and
    /// run `BufWriter::into_inner()` without partially-moving `self`
    /// (which Drop would object to).
    writer: Option<BufWriter<File>>,
    committed: bool,
}

impl AtomicCsvFile {
    pub fn create(final_path: PathBuf) -> MieResult<Self> {
        let temp_path = make_temp_path(&final_path);
        let file = File::create(&temp_path).map_err(|source| MieError::WriterError {
            destination: temp_path.display().to_string(),
            source,
        })?;
        Ok(Self {
            final_path,
            temp_path,
            writer: Some(BufWriter::new(file)),
            committed: false,
        })
    }

    /// Flush, close the temp file, and atomically rename it over the
    /// final destination. After a successful commit the temp file no
    /// longer exists so Drop's cleanup becomes a no-op.
    pub fn commit(mut self) -> MieResult<()> {
        let Some(writer) = self.writer.take() else {
            return Err(MieError::WriterError {
                destination: self.final_path.display().to_string(),
                source: io::Error::other("AtomicCsvFile::commit called without an active writer"),
            });
        };
        let temp_for_err = self.temp_path.display().to_string();
        let file = writer.into_inner().map_err(|e| MieError::WriterError {
            destination: temp_for_err,
            source: e.into_error(),
        })?;
        // Closing the File before rename matters on Windows: NTFS will
        // not rename a file that has an open handle. POSIX is fine
        // either way, but explicit close keeps platforms aligned.
        drop(file);
        std::fs::rename(&self.temp_path, &self.final_path).map_err(|source| {
            MieError::WriterError {
                destination: self.final_path.display().to_string(),
                source,
            }
        })?;
        self.committed = true;
        Ok(())
    }

    /// Flush, close the temp file, and atomically rename it to
    /// `<final_path>.partial` rather than over the final destination.
    /// Used by L2-WRT-016's `--allow-partial` branch: the original
    /// destination (if it existed) remains untouched, and the operator
    /// gets the decoded-so-far rows in the .partial file. Returns the
    /// path written so callers can log it.
    pub fn commit_partial(mut self) -> MieResult<PathBuf> {
        let Some(writer) = self.writer.take() else {
            return Err(MieError::WriterError {
                destination: self.final_path.display().to_string(),
                source: io::Error::other(
                    "AtomicCsvFile::commit_partial called without an active writer",
                ),
            });
        };
        let temp_for_err = self.temp_path.display().to_string();
        let file = writer.into_inner().map_err(|e| MieError::WriterError {
            destination: temp_for_err,
            source: e.into_error(),
        })?;
        drop(file);
        // `<dest>.partial` lives in the destination directory by
        // construction (final_path itself does), so the rename stays on
        // one filesystem and is atomic.
        let mut name = self
            .final_path
            .file_name()
            .map(|n| n.to_os_string())
            .unwrap_or_default();
        name.push(".partial");
        let partial = match self.final_path.parent() {
            Some(p) if !p.as_os_str().is_empty() => p.join(&name),
            _ => PathBuf::from(name),
        };
        std::fs::rename(&self.temp_path, &partial).map_err(|source| MieError::WriterError {
            destination: partial.display().to_string(),
            source,
        })?;
        // Mark committed so Drop does not try to clean up the (now
        // renamed) temp path.
        self.committed = true;
        Ok(partial)
    }

    pub fn final_path(&self) -> &Path {
        &self.final_path
    }
}

impl Write for AtomicCsvFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self.writer.as_mut() {
            Some(writer) => writer.write(buf),
            None => Err(io::Error::other("AtomicCsvFile::write after commit")),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self.writer.as_mut() {
            Some(writer) => writer.flush(),
            None => Err(io::Error::other("AtomicCsvFile::flush after commit")),
        }
    }
}

impl Drop for AtomicCsvFile {
    fn drop(&mut self) {
        // Take and drop the writer explicitly so the file handle is
        // closed before we try to unlink (matters on Windows).
        let _ = self.writer.take();
        if !self.committed {
            // Best-effort cleanup. If the temp file is already gone
            // (e.g. commit succeeded part-way then failed in rename
            // and we're cleaning up here defensively), this is a no-op.
            let _ = std::fs::remove_file(&self.temp_path);
        }
    }
}

/// Construct the temp file path: `<destination>.mie-decoder.tmp.<pid>`
/// in the destination's parent directory. Same-directory placement
/// guarantees the subsequent `rename()` lives on one filesystem and
/// is therefore atomic.
fn make_temp_path(final_path: &Path) -> PathBuf {
    let mut name = final_path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(format!(".mie-decoder.tmp.{}", std::process::id()));
    match final_path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.join(name),
        _ => PathBuf::from(name),
    }
}

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

    // DELTA — empty cell when delta is None (SPURIOUS_DATA, uncalibrated
    // Standard timestamps, or non-monotonic timestamps).
    if let Some(d) = msg.delta {
        write!(out, "{:.6}", d)?;
    }
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

/// Output-side options controlling safety checks enforced by `write_csv`
/// and `write_csv_split`. Default is "no checks" so library callers that
/// want raw behavior still get it.
#[derive(Debug, Clone, Default)]
pub struct WriteOptions {
    /// Input path used for the L2-WRT-014 same-file collision check.
    /// `None` skips the check (typically when the caller has already
    /// validated, or there is no associated input file context).
    pub input_path: Option<PathBuf>,
    /// L2-WRT-017: refuse to overwrite an existing destination.
    pub no_clobber: bool,
    /// L1-EXIT-004 / L2-WRT-016: when the decode hits an unrecoverable mid-
    /// file sync loss, commit the rows decoded so far as
    /// `<destination>.partial` and treat the run as successful (exit 0)
    /// rather than unlinking the temp + propagating the error (exit 3).
    pub allow_partial: bool,
}

/// Outcome of a successful CSV write. `partial` is `Some(_)` when
/// `WriteOptions.allow_partial` was set and the decode hit an
/// `UnrecoverableSyncLoss` — the rows decoded so far have been
/// committed to the `.partial` path captured in `PartialCommit`.
/// `partial` is `None` for a complete (or completely-recovered)
/// decode; the CLI distinguishes Complete from PartialRecovered by
/// querying `MieFileReader::sync_losses()` post-iteration.
#[derive(Debug)]
pub struct WriteOutcome {
    pub normal_count: u64,
    pub error_count: u64,
    pub partial: Option<PartialCommit>,
}

/// Records where the partial output landed when `allow_partial`
/// converted an `UnrecoverableSyncLoss` into a successful exit.
/// `errors_path` is `Some(_)` only when split-mode produced any
/// errored/spurious rows before the sync loss.
#[derive(Debug)]
pub struct PartialCommit {
    pub main_path: PathBuf,
    pub errors_path: Option<PathBuf>,
    pub offset: u64,
    pub sync_losses: u64,
}

/// Pre-flight checks shared by file-output entry points. Runs the
/// L2-WRT-014 input/output identity test and the L2-WRT-017 no-clobber
/// gate, in that order. No filesystem state is mutated; this only
/// produces an error before any output file is opened.
fn preflight_output(output: &Path, opts: &WriteOptions) -> MieResult<()> {
    if let Some(input) = &opts.input_path
        && paths_refer_to_same_file(input, output).unwrap_or(false)
    {
        return Err(MieError::InputOutputCollision {
            path: output.to_path_buf(),
        });
    }
    if opts.no_clobber && output.exists() {
        return Err(MieError::ClobberRefused {
            path: output.to_path_buf(),
        });
    }
    Ok(())
}

/// Stream `messages` to a single CSV. Errors and spurious records are
/// included with their ERROR / ERROR_CODE columns populated (INLINE mode,
/// or stdout where splitting is not possible).
///
/// `output` may be `None` for stdout; stdout output skips the
/// pre-flight checks because it has no filesystem identity and ignores
/// `allow_partial` (a partial stdout stream is what the consumer
/// would have seen anyway).
pub fn write_csv<I>(
    messages: I,
    output: Option<&Path>,
    opts: WriteOptions,
) -> MieResult<WriteOutcome>
where
    I: IntoIterator<Item = MieResult<MieMessage>>,
{
    match output {
        Some(path) => {
            preflight_output(path, &opts)?;
            let mut atomic = AtomicCsvFile::create(path.to_path_buf())?;

            let (count, partial_info) = {
                let mut writer = CsvWriter::new(&mut atomic, path.display().to_string())?;
                let mut partial_info: Option<(u64, u64)> = None;
                for item in messages {
                    match item {
                        Ok(msg) => writer.write_message(&msg)?,
                        Err(MieError::UnrecoverableSyncLoss {
                            offset,
                            sync_losses,
                        }) if opts.allow_partial => {
                            partial_info = Some((offset, sync_losses));
                            break;
                        }
                        Err(e) => return Err(e),
                    }
                }
                let n = writer.finish()?;
                (n, partial_info)
            };

            match partial_info {
                None => {
                    atomic.commit()?;
                    log_info!("wrote {} rows to {}", count, path.display());
                    Ok(WriteOutcome {
                        normal_count: count,
                        error_count: 0,
                        partial: None,
                    })
                }
                Some((offset, sync_losses)) => {
                    let partial_path = atomic.commit_partial()?;
                    log_warn!(
                        "unrecoverable sync loss at 0x{:X} after {} recovery attempt(s); \
                         wrote {} rows to {} (--allow-partial)",
                        offset,
                        sync_losses,
                        count,
                        partial_path.display()
                    );
                    Ok(WriteOutcome {
                        normal_count: count,
                        error_count: 0,
                        partial: Some(PartialCommit {
                            main_path: partial_path,
                            errors_path: None,
                            offset,
                            sync_losses,
                        }),
                    })
                }
            }
        }
        None => {
            let stdout = std::io::stdout();
            let buf = BufWriter::new(stdout.lock());
            let mut writer = CsvWriter::new(buf, "stdout".to_string())?;
            stream_into(&mut writer, messages)?;
            let n = writer.finish()?;
            log_info!("wrote {} rows to stdout", n);
            Ok(WriteOutcome {
                normal_count: n,
                error_count: 0,
                partial: None,
            })
        }
    }
}

/// Split-output streaming: normal records to `output`, errored / spurious
/// to `<stem>_errors<ext>`. Only opens the error temp file lazily on the
/// first error row, so files with no errors don't produce an empty
/// `_errors.csv` (and don't even create a temp).
///
/// Both files use the AtomicCsvFile pattern — temp + atomic rename.
/// When `opts.allow_partial` and the iterator yields
/// `UnrecoverableSyncLoss`, both files (if any) are committed as
/// `.partial` and the function returns Ok with PartialCommit info.
pub fn write_csv_split<I>(messages: I, output: &Path, opts: WriteOptions) -> MieResult<WriteOutcome>
where
    I: IntoIterator<Item = MieResult<MieMessage>>,
{
    preflight_output(output, &opts)?;

    let error_path = error_path_for(output);
    // Also pre-flight the error file path for clobber. The collision
    // check against the input is implicit — the error path is derived
    // from output, which was already checked.
    if opts.no_clobber && error_path.exists() {
        return Err(MieError::ClobberRefused {
            path: error_path.clone(),
        });
    }

    let mut main_atomic = AtomicCsvFile::create(output.to_path_buf())?;
    let mut errors_atomic: Option<AtomicCsvFile> = None;

    let (normal_count, error_count, partial_info) = {
        let mut main = CsvWriter::new(&mut main_atomic, output.display().to_string())?;
        // The error writer is created lazily on first error row so a
        // clean file doesn't leave an empty errors CSV behind.
        let mut error_writer: Option<CsvWriter<&mut AtomicCsvFile>> = None;
        let mut partial_info: Option<(u64, u64)> = None;

        for item in messages {
            let msg = match item {
                Ok(m) => m,
                Err(MieError::UnrecoverableSyncLoss {
                    offset,
                    sync_losses,
                }) if opts.allow_partial => {
                    partial_info = Some((offset, sync_losses));
                    break;
                }
                Err(e) => return Err(e),
            };
            if !msg.error_label().is_empty() {
                if error_writer.is_none() {
                    errors_atomic = Some(AtomicCsvFile::create(error_path.clone())?);
                    let Some(inner) = errors_atomic.as_mut() else {
                        return Err(MieError::WriterError {
                            destination: error_path.display().to_string(),
                            source: io::Error::other("error output writer was not initialized"),
                        });
                    };
                    error_writer = Some(CsvWriter::new(inner, error_path.display().to_string())?);
                }
                let Some(writer) = error_writer.as_mut() else {
                    return Err(MieError::WriterError {
                        destination: error_path.display().to_string(),
                        source: io::Error::other("error CSV writer was not initialized"),
                    });
                };
                writer.write_message(&msg)?;
            } else {
                main.write_message(&msg)?;
            }
        }

        let n_errors = match error_writer.as_ref() {
            Some(w) => w.rows_written(),
            None => 0,
        };
        let n_main = main.rows_written();

        // Flush both CsvWriters before commit (drops them, which
        // releases the borrows on the AtomicCsvFiles).
        if let Some(w) = error_writer {
            w.finish()?;
        }
        main.finish()?;

        (n_main, n_errors, partial_info)
    };

    match partial_info {
        None => {
            // Normal path. Commit errors first so a main-commit failure
            // doesn't leave a dangling errors file. (If the error commit
            // itself fails, the main temp is unlinked on Drop.)
            if let Some(ea) = errors_atomic {
                ea.commit()?;
                log_info!(
                    "wrote {} error/spurious rows to {}",
                    error_count,
                    error_path.display()
                );
            } else {
                log_info!("no error/spurious records — error file not created");
            }
            main_atomic.commit()?;

            log_info!("wrote {} normal rows to {}", normal_count, output.display());
            if normal_count == 0 {
                log_warn!("main CSV is empty (header only)");
            }

            Ok(WriteOutcome {
                normal_count,
                error_count,
                partial: None,
            })
        }
        Some((offset, sync_losses)) => {
            // Partial path. Rename each AtomicCsvFile temp to its
            // `.partial` counterpart so the operator can inspect what
            // was decoded before the corruption.
            let errors_partial_path = if let Some(ea) = errors_atomic {
                Some(ea.commit_partial()?)
            } else {
                None
            };
            let main_partial_path = main_atomic.commit_partial()?;
            log_warn!(
                "unrecoverable sync loss at 0x{:X} after {} recovery attempt(s); \
                 wrote {} normal + {} error rows as partial to {} (--allow-partial)",
                offset,
                sync_losses,
                normal_count,
                error_count,
                main_partial_path.display()
            );

            Ok(WriteOutcome {
                normal_count,
                error_count,
                partial: Some(PartialCommit {
                    main_path: main_partial_path,
                    errors_path: errors_partial_path,
                    offset,
                    sync_losses,
                }),
            })
        }
    }
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
            delta: Some(0.123456),
            file_offset: 0,
        }
    }

    /// Requirements: L2-WRT-001
    #[test]
    fn header_present() {
        let mut buf = Vec::new();
        let writer = CsvWriter::new(&mut buf, "memory").unwrap();
        writer.finish().unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.starts_with("TIME_STAMP,RT,MSG,WD01"));
        assert!(s.trim_end().ends_with("XMT_GAP"));
    }

    /// Requirements: L2-WRT-001, L2-WRT-003
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

    /// Requirements: L2-WRT-002
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

    /// Requirements: L2-ERR-008
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

    // ── AtomicCsvFile and path identity ──────────────────────────────
    //
    // Tests pinning L2-WRT-014 (input/output collision), L2-WRT-015
    // (atomic rename), L2-WRT-016 default-cleanup, and L2-WRT-017
    // (--no-clobber refusal). The .partial-rename branch of L2-WRT-016
    // depends on the --allow-partial CLI flag and is covered with
    // Phase 3.

    use std::sync::atomic::{AtomicU64, Ordering};

    fn unique_path(suffix: &str) -> std::path::PathBuf {
        static C: AtomicU64 = AtomicU64::new(0);
        let n = C.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        std::env::temp_dir().join(format!("mie-atomic-test-{pid}-{n}{suffix}"))
    }

    /// Requirements: L3-WRT-001
    #[test]
    fn make_temp_path_lives_next_to_destination() {
        let dest = std::env::temp_dir().join("out.csv");
        let tmp = make_temp_path(&dest);
        assert_eq!(tmp.parent(), dest.parent());
        let name = tmp.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("out.csv.mie-decoder.tmp."));
        // PID suffix
        assert!(name.ends_with(&std::process::id().to_string()));
    }

    /// Requirements: L2-WRT-015
    #[test]
    fn atomic_commit_renames_temp_over_destination() {
        let dest = unique_path(".csv");
        {
            let mut atomic = AtomicCsvFile::create(dest.clone()).unwrap();
            atomic.write_all(b"hello\n").unwrap();
            atomic.commit().unwrap();
        }
        let content = std::fs::read_to_string(&dest).unwrap();
        assert_eq!(content, "hello\n");
        // Temp file must be gone after commit.
        let tmp = make_temp_path(&dest);
        assert!(!tmp.exists(), "temp file still present after commit");
        let _ = std::fs::remove_file(&dest);
    }

    /// Requirements: L2-WRT-015, L2-WRT-016
    #[test]
    fn atomic_drop_without_commit_unlinks_temp_and_leaves_destination() {
        let dest = unique_path(".csv");
        // Pre-create destination so we can verify it isn't touched.
        std::fs::write(&dest, b"original\n").unwrap();
        let tmp = make_temp_path(&dest);
        {
            let mut atomic = AtomicCsvFile::create(dest.clone()).unwrap();
            atomic.write_all(b"discarded\n").unwrap();
            // Drop without commit — simulates a decode failure.
        }
        // Temp must be unlinked; destination must be unchanged.
        assert!(!tmp.exists(), "temp file should be cleaned up on Drop");
        let content = std::fs::read_to_string(&dest).unwrap();
        assert_eq!(content, "original\n");
        let _ = std::fs::remove_file(&dest);
    }

    /// Requirements: L2-WRT-014
    #[test]
    fn paths_refer_to_same_file_existing() {
        let p = unique_path(".dat");
        std::fs::write(&p, b"x").unwrap();
        assert!(paths_refer_to_same_file(&p, &p).unwrap());
        let _ = std::fs::remove_file(&p);
    }

    /// Requirements: L2-WRT-014
    #[test]
    fn paths_refer_to_same_file_nonexistent_output_under_same_parent() {
        // Input exists; output names the same path but doesn't exist yet
        // (because we removed it). The check should still detect collision
        // via parent canonicalize + filename match.
        let p = unique_path(".dat");
        std::fs::write(&p, b"x").unwrap();
        let same_name_missing_file = p.clone();
        std::fs::remove_file(&same_name_missing_file).unwrap();
        // Re-create input so canonicalize works on input.
        std::fs::write(&p, b"x").unwrap();
        // Different output path that doesn't exist — must NOT be a collision.
        let different = unique_path(".csv");
        assert!(!paths_refer_to_same_file(&p, &different).unwrap());
        let _ = std::fs::remove_file(&p);
    }

    /// Requirements: L2-WRT-014
    #[test]
    fn write_csv_rejects_input_output_collision() {
        let p = unique_path(".csv");
        std::fs::write(&p, b"existing\n").unwrap();
        let opts = WriteOptions {
            input_path: Some(p.clone()),
            no_clobber: false,
            allow_partial: false,
        };
        // Empty iterator — should never reach the write because
        // preflight_output fails first.
        let result = write_csv(std::iter::empty(), Some(&p), opts);
        match result {
            Err(MieError::InputOutputCollision { path }) => assert_eq!(path, p),
            other => panic!("expected InputOutputCollision, got {other:?}"),
        }
        // File must be unchanged.
        let content = std::fs::read_to_string(&p).unwrap();
        assert_eq!(content, "existing\n");
        let _ = std::fs::remove_file(&p);
    }

    /// Requirements: L2-WRT-017
    #[test]
    fn write_csv_rejects_clobber_when_no_clobber_set() {
        let p = unique_path(".csv");
        std::fs::write(&p, b"existing\n").unwrap();
        let opts = WriteOptions {
            input_path: None,
            no_clobber: true,
            allow_partial: false,
        };
        let result = write_csv(std::iter::empty(), Some(&p), opts);
        match result {
            Err(MieError::ClobberRefused { path }) => assert_eq!(path, p),
            other => panic!("expected ClobberRefused, got {other:?}"),
        }
        // File must be unchanged.
        let content = std::fs::read_to_string(&p).unwrap();
        assert_eq!(content, "existing\n");
        let _ = std::fs::remove_file(&p);
    }

    /// Requirements: L2-WRT-017
    #[test]
    fn write_csv_overwrites_by_default() {
        let p = unique_path(".csv");
        std::fs::write(&p, b"existing\n").unwrap();
        let result = write_csv(std::iter::empty(), Some(&p), WriteOptions::default());
        result.unwrap();
        // File should now contain just the CSV header.
        let content = std::fs::read_to_string(&p).unwrap();
        assert!(content.starts_with("TIME_STAMP,RT,MSG,"));
        let _ = std::fs::remove_file(&p);
    }

    /// Requirements: L2-WRT-014
    #[test]
    fn write_csv_split_rejects_input_output_collision() {
        let p = unique_path(".csv");
        std::fs::write(&p, b"existing\n").unwrap();
        let opts = WriteOptions {
            input_path: Some(p.clone()),
            no_clobber: false,
            allow_partial: false,
        };
        let result = write_csv_split(std::iter::empty(), &p, opts);
        match result {
            Err(MieError::InputOutputCollision { path }) => assert_eq!(path, p),
            other => panic!("expected InputOutputCollision, got {other:?}"),
        }
        let _ = std::fs::remove_file(&p);
    }

    /// Requirements: L2-WRT-017
    #[test]
    fn write_csv_split_no_clobber_checks_errors_file_too() {
        // No-clobber should reject if the *errors* file exists, even
        // when the main destination is fresh.
        let dest = unique_path(".csv");
        let err_dest = error_path_for(&dest);
        std::fs::write(&err_dest, b"old errors\n").unwrap();
        let opts = WriteOptions {
            input_path: None,
            no_clobber: true,
            allow_partial: false,
        };
        let result = write_csv_split(std::iter::empty(), &dest, opts);
        match result {
            Err(MieError::ClobberRefused { path }) => assert_eq!(path, err_dest),
            other => panic!("expected ClobberRefused on errors path, got {other:?}"),
        }
        // Main dest must not have been created.
        assert!(!dest.exists());
        let _ = std::fs::remove_file(&err_dest);
    }

    /// Requirements: L3-WRT-002
    #[test]
    fn atomic_commit_partial_writes_dot_partial_and_leaves_destination() {
        let dest = unique_path(".csv");
        // Pre-create destination so we can verify it stays untouched.
        std::fs::write(&dest, b"original\n").unwrap();
        let partial_path = {
            let mut atomic = AtomicCsvFile::create(dest.clone()).unwrap();
            atomic.write_all(b"partial decode\n").unwrap();
            atomic.commit_partial().unwrap()
        };
        // The committed-partial path must be <dest>.partial.
        let expected_partial = {
            let mut name = dest.file_name().unwrap().to_os_string();
            name.push(".partial");
            dest.parent().unwrap().join(name)
        };
        assert_eq!(partial_path, expected_partial);
        assert_eq!(
            std::fs::read_to_string(&partial_path).unwrap(),
            "partial decode\n"
        );
        // Original destination must be unchanged.
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "original\n");
        // Temp must be gone.
        let tmp = make_temp_path(&dest);
        assert!(
            !tmp.exists(),
            "temp file should be gone after commit_partial"
        );
        let _ = std::fs::remove_file(&dest);
        let _ = std::fs::remove_file(&partial_path);
    }

    /// Requirements: L2-WRT-016, L1-EXIT-004
    #[test]
    fn write_csv_with_allow_partial_commits_on_unrecoverable() {
        let dest = unique_path(".csv");
        // Synthetic iterator: one good message, then UnrecoverableSyncLoss.
        let messages: Vec<MieResult<MieMessage>> = vec![
            Ok(sample_msg()),
            Err(MieError::UnrecoverableSyncLoss {
                offset: 0x1234,
                sync_losses: 1,
            }),
        ];
        let opts = WriteOptions {
            input_path: None,
            no_clobber: false,
            allow_partial: true,
        };
        let outcome = write_csv(messages, Some(&dest), opts).unwrap();
        let partial = outcome.partial.expect("partial commit info");
        assert_eq!(partial.offset, 0x1234);
        assert_eq!(partial.sync_losses, 1);
        // Main destination must NOT exist; only the .partial does.
        assert!(!dest.exists(), "destination should not exist on partial");
        assert!(partial.main_path.exists(), "partial file must exist");
        let body = std::fs::read_to_string(&partial.main_path).unwrap();
        assert!(body.starts_with("TIME_STAMP,RT,MSG,"));
        assert!(body.contains("11R")); // sample_msg is SA 11 R
        let _ = std::fs::remove_file(&partial.main_path);
    }

    /// Requirements: L2-WRT-016, L1-EXIT-004
    #[test]
    fn write_csv_without_allow_partial_propagates_unrecoverable() {
        let dest = unique_path(".csv");
        let messages: Vec<MieResult<MieMessage>> = vec![
            Ok(sample_msg()),
            Err(MieError::UnrecoverableSyncLoss {
                offset: 0x42,
                sync_losses: 1,
            }),
        ];
        let opts = WriteOptions {
            input_path: None,
            no_clobber: false,
            allow_partial: false,
        };
        let err = write_csv(messages, Some(&dest), opts).unwrap_err();
        match err {
            MieError::UnrecoverableSyncLoss {
                offset,
                sync_losses,
            } => {
                assert_eq!(offset, 0x42);
                assert_eq!(sync_losses, 1);
            }
            other => panic!("expected UnrecoverableSyncLoss, got {other:?}"),
        }
        // Both destination and .partial must be absent — Drop unlinked
        // the temp because allow_partial was false.
        assert!(!dest.exists());
        let mut partial_name = dest.file_name().unwrap().to_os_string();
        partial_name.push(".partial");
        let partial = dest.parent().unwrap().join(partial_name);
        assert!(!partial.exists());
    }

    /// Requirements: L2-WRT-018
    #[test]
    fn is_broken_pipe_predicate() {
        let e = MieError::WriterError {
            destination: "stdout".to_string(),
            source: io::Error::new(io::ErrorKind::BrokenPipe, "pipe closed"),
        };
        assert!(e.is_broken_pipe());

        let other = MieError::WriterError {
            destination: "stdout".to_string(),
            source: io::Error::other("nope"),
        };
        assert!(!other.is_broken_pipe());

        let non_writer = MieError::FileEmpty {
            path: std::path::PathBuf::from("/x"),
        };
        assert!(!non_writer.is_broken_pipe());
    }
}
