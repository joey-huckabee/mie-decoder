//! mmap-backed sequential reader.
//!
//! `MieFileReader` opens an MIE binary file with `memmap2`, finds the first
//! valid record (skipping any header), auto-detects the timestamp format from
//! the first records if requested (bounded multi-record probe, L2-DEC-015),
//! and yields decoded `MieMessage`s in file order.
//!
//! Sync recovery happens internally — only unrecoverable errors (or strict
//! mode opt-ins) surface as `Err` items from the iterator.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use memmap2::Mmap;

use crate::decode::{
    DEFAULT_DETECT_RECORDS, DetectionConfidence, MIN_RECORD_BYTES_STANDARD,
    classify_message_format, decode_command_word, decode_irig_timestamp, decode_standard_timestamp,
    decode_type_word, probe_timestamp_format, read_u16, read_u16_array,
};
use crate::error::{MieError, MieResult};
use crate::models::{
    CommandWord, DataWords, ERROR_SPURIOUS_CONTINUATION, ERROR_SPURIOUS_STANDALONE, MessageFormat,
    MessageType, MieMessage, Timestamp, TimestampFormat, TypeWord, ddc_error_description,
    is_known_ddc_error_code, timestamp_word_count,
};
use crate::sync::{
    DEFAULT_LOOKAHEAD_RECORDS, MAX_SCAN_BYTES, ValidationFailure, find_first_record, recover_sync,
    validate_record_detailed,
};
use crate::{log_debug, log_error, log_info, log_warn};

/// Reader handle. Construct with [`new`]; iterate by calling `.iter()` or
/// using `IntoIterator`.
pub struct MieFileReader {
    path: PathBuf,
    mmap: Mmap,
    file_size: u64,
    strict: bool,
    time_format: TimestampFormat,
    /// L2-DEC-015: number of records the auto-detect probe walks
    /// before committing to IRIG vs Standard. Ignored when
    /// `time_format` is anything other than `Auto`. Default
    /// `DEFAULT_DETECT_RECORDS` (8).
    detect_records: usize,
    /// L2-SYN-026: total number of records `validate_record` checks
    /// (1 candidate + N-1 look-ahead). Default
    /// `DEFAULT_LOOKAHEAD_RECORDS` (2), preserving the historical
    /// two-record look-ahead behavior.
    lookahead_records: usize,
    /// L2-DEC-017: optional Standard-counter tick rate in Hz. When `Some`
    /// with a finite, strictly-positive value, Standard timestamps are
    /// converted to microseconds and participate in DELTA tracking like
    /// IRIG. `None` (the default) preserves the historical empty-DELTA
    /// behavior for Standard records.
    standard_tick_rate_hz: Option<f64>,
    /// Cumulative sync-recovery attempts during the most recent iter()
    /// call. Reset to 0 at the start of each iter(). Shared with the
    /// active RecordIter via a reference so the CLI can query it
    /// post-iteration (e.g., to distinguish L1-EXIT-003 partial-recovered
    /// from L1-EXIT-002 complete in the exit-class summary).
    sync_losses: AtomicU64,
}

/// Builder-style options. `strict=false`, `time_format=Auto`,
/// `detect_records=DEFAULT_DETECT_RECORDS` by default.
#[derive(Debug, Clone, Copy)]
pub struct ReaderOptions {
    pub strict: bool,
    pub time_format: TimestampFormat,
    /// L2-DEC-015 probe size. Number of records auto-detection
    /// walks before committing to a format. Clamped to [1, 32]
    /// upstream by config / CLI parsing.
    pub detect_records: usize,
    /// L2-SYN-026 look-ahead depth. Total number of records
    /// `validate_record` checks (1 candidate + N-1 look-ahead).
    /// Clamped to [1, 32] upstream by config / CLI parsing.
    pub lookahead_records: usize,
    /// L2-DEC-017 Standard-counter tick rate in Hz. `Some` with a
    /// finite, strictly-positive value enables tick→microsecond
    /// conversion and DELTA for Standard records; `None` keeps the
    /// historical empty-DELTA behavior. Validated upstream by
    /// config / CLI parsing.
    pub standard_tick_rate_hz: Option<f64>,
}

impl Default for ReaderOptions {
    fn default() -> Self {
        Self {
            strict: false,
            time_format: TimestampFormat::Auto,
            detect_records: DEFAULT_DETECT_RECORDS,
            lookahead_records: DEFAULT_LOOKAHEAD_RECORDS,
            standard_tick_rate_hz: None,
        }
    }
}

impl MieFileReader {
    pub fn new(path: impl AsRef<Path>) -> MieResult<Self> {
        Self::with_options(path, ReaderOptions::default())
    }

    pub fn with_options(path: impl AsRef<Path>, opts: ReaderOptions) -> MieResult<Self> {
        let path = path.as_ref().to_path_buf();
        if !path.exists() {
            return Err(MieError::FileNotFound { path });
        }
        let file = File::open(&path).map_err(|source| MieError::FileIo {
            path: path.clone(),
            source,
        })?;
        let metadata = file.metadata().map_err(|source| MieError::FileIo {
            path: path.clone(),
            source,
        })?;
        let file_size = metadata.len();
        if file_size == 0 {
            return Err(MieError::FileEmpty { path });
        }

        // SAFETY: `Mmap::map` creates a read-only memory map of the
        // already-opened file. memmap2's contract requires the underlying
        // file not be mutated or truncated while mapped; we document that as
        // a precondition (L1-EXIT-006 — modifying the input during decode is
        // undefined). The returned `Mmap` owns the OS mapping and keeps it
        // valid independently of `file`, which is dropped at the end of this
        // function (closing the fd does not invalidate the mapping). The raw
        // bytes are never exposed outside the reader.
        let mmap = unsafe { Mmap::map(&file) }.map_err(|source| MieError::FileIo {
            path: path.clone(),
            source,
        })?;

        log_debug!(
            "reader opened {} ({} bytes, strict={}, time_format={:?}, detect_records={})",
            path.display(),
            file_size,
            opts.strict,
            opts.time_format,
            opts.detect_records
        );

        Ok(Self {
            path,
            mmap,
            file_size,
            strict: opts.strict,
            time_format: opts.time_format,
            detect_records: opts.detect_records.max(1),
            lookahead_records: opts.lookahead_records.max(1),
            standard_tick_rate_hz: opts.standard_tick_rate_hz,
            sync_losses: AtomicU64::new(0),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn file_size(&self) -> u64 {
        self.file_size
    }

    /// Cumulative sync-recovery count from the most recent iter() call.
    /// Reset to 0 each time `iter()` is invoked. Query after the
    /// iterator is exhausted to derive the L1-EXIT-003/005 exit class.
    pub fn sync_losses(&self) -> u64 {
        self.sync_losses.load(Ordering::Relaxed)
    }

    /// Borrow an iterator over decoded messages.
    pub fn iter(&self) -> RecordIter<'_> {
        // Reset the per-call counter so successive iter() calls on the
        // same reader handle don't accumulate stale counts.
        self.sync_losses.store(0, Ordering::Relaxed);

        // `format_hint` is the Option-typed value threaded through
        // `find_first_record` and `diagnose_header_scan_failure`: None
        // tells those helpers to scan format-agnostically; Some pins the
        // expected layout.
        let format_hint = if self.time_format == TimestampFormat::Auto {
            None
        } else {
            Some(self.time_format)
        };

        let data: &[u8] = &self.mmap;
        let file_len = data.len();

        let start_offset = find_first_record(
            data,
            file_len,
            format_hint,
            MAX_SCAN_BYTES,
            self.lookahead_records,
        );

        // Tracks whether the iterator should terminate immediately with
        // no records and no error (the L2-RDR-004 lenient-mode case).
        let mut early_done = false;
        // The format the iterator will use for the entire decode.
        // Defaults to the explicit choice (or IRIG as a placeholder for
        // the Auto-but-no-records-found case where iteration never
        // happens anyway); rewritten to the L2-DEC-015 probe result
        // below when we have a real start offset and time_format=Auto.
        let mut resolved_format = match self.time_format {
            TimestampFormat::Auto => TimestampFormat::Irig,
            explicit => explicit,
        };

        let pending_error = match start_offset {
            Some(hit) => {
                if hit.offset == 0 {
                    log_debug!("first record at offset 0 (no header)");
                } else {
                    log_info!(
                        "file header detected: {} bytes before first record at 0x{:X}",
                        hit.offset,
                        hit.offset
                    );
                }
                // L2-SYN-018: reject pathological homogeneous-payload
                // inputs (e.g. 0x20-padded files where every "record"
                // parses as a synthetic SPURIOUS_DATA frame).
                let candidate_type_raw = read_u16(data, hit.offset).unwrap_or(0);
                let candidate_tw = decode_type_word(candidate_type_raw);
                let candidate_record_bytes = usize::from(candidate_tw.word_count) * 2;
                if crate::sync::is_homogeneous_payload(data, hit.offset, candidate_record_bytes) {
                    log_error!(
                        "pathological homogeneous-payload input at offset 0x{:X} \
                         in {}: {} consecutive candidate records are byte-identical",
                        hit.offset,
                        self.path.display(),
                        crate::sync::HOMOGENEITY_SAMPLE_RECORDS,
                    );
                    Some(MieError::HomogeneousPayload {
                        path: self.path.clone(),
                        offset: hit.offset as u64,
                        sample_records: crate::sync::HOMOGENEITY_SAMPLE_RECORDS as u32,
                    })
                } else if self.time_format == TimestampFormat::Auto {
                    // L2-DEC-015: multi-record probe to disambiguate
                    // IRIG vs Standard before iteration begins. The
                    // chosen format is final per L2-DEC-011 — no
                    // per-record re-detection.
                    let outcome = probe_timestamp_format(data, hit.offset, self.detect_records);
                    resolved_format = outcome.format;
                    match outcome.confidence {
                        DetectionConfidence::Decisive => {
                            log_info!(
                                "auto-detected timestamp format: {:?} \
                                 (Decisive: IRIG={} STD={} over {} record(s))",
                                outcome.format,
                                outcome.irig_score,
                                outcome.std_score,
                                outcome.records_probed,
                            );
                            None
                        }
                        DetectionConfidence::Marginal => {
                            log_info!(
                                "auto-detected timestamp format: {:?} \
                                 (Marginal: IRIG={} STD={} over {} record(s)) — \
                                 pass --time-format to force the choice if this is wrong",
                                outcome.format,
                                outcome.irig_score,
                                outcome.std_score,
                                outcome.records_probed,
                            );
                            None
                        }
                        DetectionConfidence::Ambiguous => {
                            // L2-DEC-016: the probe could not
                            // confidently distinguish the two
                            // formats. Strict mode rejects the file;
                            // lenient mode uses the chosen format
                            // anyway with a WARN so existing operator
                            // workflows on borderline files don't
                            // break silently.
                            if self.strict {
                                log_error!(
                                    "timestamp-format auto-detection is ambiguous in {} \
                                     starting at offset 0x{:X}: IRIG={} STD={} over {} \
                                     record(s) — strict mode rejects ambiguous files; \
                                     pass --time-format to force the choice",
                                    self.path.display(),
                                    hit.offset,
                                    outcome.irig_score,
                                    outcome.std_score,
                                    outcome.records_probed,
                                );
                                Some(MieError::TimestampFormatMismatch {
                                    offset: hit.offset as u64,
                                    irig_score: outcome.irig_score,
                                    std_score: outcome.std_score,
                                    records_probed: outcome.records_probed as u32,
                                })
                            } else {
                                log_warn!(
                                    "auto-detected timestamp format: {:?} \
                                     (Ambiguous: IRIG={} STD={} over {} record(s)) — \
                                     using best guess; pass --time-format to force the \
                                     choice or --strict to reject ambiguous files",
                                    outcome.format,
                                    outcome.irig_score,
                                    outcome.std_score,
                                    outcome.records_probed,
                                );
                                None
                            }
                        }
                    }
                } else {
                    // L2-DEC-013: the format was forced via --time-format /
                    // decode.time_format. Sanity-check it against the same
                    // detection probe: if the probe is *Decisive* about the
                    // OTHER format, the forced selection is obviously wrong
                    // (e.g. --time-format standard on an IRIG file), which
                    // would otherwise emit garbage timestamps for the whole
                    // file. Marginal/Ambiguous probes are NOT flagged — those
                    // are exactly the cases where forcing is the legitimate
                    // override of a detection the heuristic can't make
                    // confidently. resolved_format stays the forced format.
                    let outcome = probe_timestamp_format(data, hit.offset, self.detect_records);
                    if outcome.confidence == DetectionConfidence::Decisive
                        && outcome.format != self.time_format
                    {
                        if self.strict {
                            log_error!(
                                "forced timestamp format {:?} contradicts the recording in {} \
                                 at offset 0x{:X}: detection is decisive for {:?} (IRIG={} \
                                 STD={} over {} record(s)) — strict mode rejects the mismatch; \
                                 drop --time-format to auto-detect",
                                self.time_format,
                                self.path.display(),
                                hit.offset,
                                outcome.format,
                                outcome.irig_score,
                                outcome.std_score,
                                outcome.records_probed,
                            );
                            Some(MieError::TimestampFormatMismatch {
                                offset: hit.offset as u64,
                                irig_score: outcome.irig_score,
                                std_score: outcome.std_score,
                                records_probed: outcome.records_probed as u32,
                            })
                        } else {
                            log_warn!(
                                "forced timestamp format {:?} contradicts the recording at \
                                 offset 0x{:X}: detection is decisive for {:?} (IRIG={} STD={} \
                                 over {} record(s)) — decoding with the forced format anyway; \
                                 drop --time-format to auto-detect or pass --strict to reject \
                                 the mismatch",
                                self.time_format,
                                hit.offset,
                                outcome.format,
                                outcome.irig_score,
                                outcome.std_score,
                                outcome.records_probed,
                            );
                            None
                        }
                    } else {
                        None
                    }
                }
            }
            None => {
                // L2-RDR-004: distinguish "no MIE record at all" from
                // "structurally-valid Type Word truncated past EOF".
                let truncated = crate::sync::diagnose_header_scan_failure(
                    data,
                    file_len,
                    format_hint,
                    MAX_SCAN_BYTES,
                );
                match truncated {
                    Some((trunc_offset, record_bytes, available)) => {
                        if self.strict {
                            log_error!(
                                "first record after header detection is truncated \
                                 at 0x{:X}: declared {} bytes, only {} available",
                                trunc_offset,
                                record_bytes,
                                available
                            );
                            Some(MieError::FirstRecordTruncated {
                                offset: trunc_offset as u64,
                                record_bytes: record_bytes as u64,
                                available_bytes: available as u64,
                            })
                        } else {
                            log_warn!(
                                "first record after header detection is truncated \
                                 at 0x{:X}: declared {} bytes, only {} available — \
                                 lenient mode terminates cleanly with zero records",
                                trunc_offset,
                                record_bytes,
                                available
                            );
                            early_done = true;
                            None
                        }
                    }
                    None => {
                        let scan_bytes = file_len.min(MAX_SCAN_BYTES) as u64;
                        log_error!(
                            "no valid records found in first {} bytes of {}",
                            scan_bytes,
                            self.path.display()
                        );
                        Some(MieError::NoValidRecords {
                            path: self.path.clone(),
                            scan_bytes,
                        })
                    }
                }
            }
        };

        log_info!("beginning decode of {}", self.path.display());

        RecordIter {
            data,
            file_len,
            offset: start_offset.map(|h| h.offset).unwrap_or(file_len),
            done: early_done,
            pending_error,
            pending_unrecoverable: None,
            strict: self.strict,
            resolved_format,
            lookahead_records: self.lookahead_records,
            standard_tick_rate_hz: self.standard_tick_rate_hz,
            prev_was_error: false,
            delta_tracker: HashMap::new(),
            warned_ooo_keys: HashSet::new(),
            warned_irig_day: false,
            msg_count: 0,
            sync_losses: 0,
            sync_losses_atomic: &self.sync_losses,
            path_for_log: &self.path,
        }
    }
}

impl<'a> IntoIterator for &'a MieFileReader {
    type Item = MieResult<MieMessage>;
    type IntoIter = RecordIter<'a>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

pub struct RecordIter<'a> {
    data: &'a [u8],
    file_len: usize,
    offset: usize,
    done: bool,
    /// If set, the very next call to `next()` returns `Some(Err(_))` and
    /// then transitions to `done = true`. Used to surface conditions
    /// detected at iterator construction (e.g. no valid records in the
    /// scan window) without silently yielding an empty stream.
    pending_error: Option<MieError>,
    /// Set when lenient-mode sync recovery exhausts mid-file. The next
    /// next() call yields this terminal Err once, then transitions to
    /// done = true. Distinct from `pending_error` so the message-decoding
    /// loop can populate it without entangling with construction-time
    /// errors. Per L1-EXIT-004 the CLI catches this variant to decide
    /// between exit 3 (default) and a `.partial` commit + exit 0
    /// (when `--allow-partial` is set).
    pending_unrecoverable: Option<MieError>,
    strict: bool,
    /// L2-DEC-011 / L2-DEC-015: format is resolved eagerly in
    /// `iter()` (via `probe_timestamp_format`) before the iterator is
    /// constructed, so by the time `next()` runs the format is final
    /// and stays fixed for the rest of the decode.
    resolved_format: TimestampFormat,
    /// L2-SYN-026 look-ahead depth threaded from the reader. Used by
    /// the per-record `validate_record` call inside `next()` and by
    /// the `recover_sync` call on sync-loss recovery.
    lookahead_records: usize,
    /// L2-DEC-017 Standard-counter tick rate threaded from the reader.
    /// Passed to `Timestamp::to_microseconds` in `delta_for`; `None`
    /// keeps Standard records out of DELTA tracking.
    standard_tick_rate_hz: Option<f64>,
    prev_was_error: bool,
    /// Per-RT/MSG last-seen timestamp in microseconds. Populated when the
    /// source timestamp has a microsecond basis: IRIG always, and Standard
    /// when a tick rate is configured. Uncalibrated Standard timestamps
    /// yield None from `Timestamp::to_microseconds()` and bypass the
    /// tracker entirely.
    delta_tracker: HashMap<u32, u64>,
    /// RT/MSG keys for which a non-monotonic-timestamp WARN has already
    /// been emitted. Limits log volume on chronically out-of-order files
    /// to one line per key per recording.
    warned_ooo_keys: HashSet<u32>,
    /// Whether the one-time IRIG day-of-year discrepancy advisory has been
    /// emitted for this decode (PRA-9). Fires once on the first
    /// calendar-locked (non-freerun) IRIG record.
    warned_irig_day: bool,
    msg_count: u64,
    /// Per-iteration sync-loss counter, kept locally to avoid an
    /// atomic load on every record. Mirrored to `sync_losses_atomic`
    /// so the reader-level getter can surface it post-iteration.
    sync_losses: u64,
    /// Shared with `MieFileReader::sync_losses` so the CLI can query
    /// the cumulative count after iteration ends.
    sync_losses_atomic: &'a AtomicU64,
    path_for_log: &'a Path,
}

fn log_validation_context(data: &[u8], offset: usize) {
    if !crate::log::enabled(crate::log::Level::Debug) {
        return;
    }
    let start = offset.saturating_sub(16);
    let end = data.len().min(start.saturating_add(32));
    let hex = data[start..end]
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ");
    log_debug!(
        "validation context at 0x{:X} (bytes 0x{:X}..0x{:X}, max 32): {}",
        offset,
        start,
        end,
        hex
    );
}

/// Encode `(rt, sa, dir)` into a single u32 key for the delta tracker.
/// Avoids per-record String allocation and HashMap key construction.
#[inline]
fn delta_key(rt: u8, subaddress: u8, transmit: bool) -> u32 {
    (u32::from(rt) << 16) | (u32::from(subaddress) << 8) | u32::from(transmit)
}

impl<'a> Iterator for RecordIter<'a> {
    type Item = MieResult<MieMessage>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        // Surface a pending construction-time error exactly once, then
        // transition to Done. This makes "no valid records" a real Err
        // item rather than a silent empty stream.
        if let Some(err) = self.pending_error.take() {
            self.done = true;
            return Some(Err(err));
        }
        // Surface a deferred mid-iteration unrecoverable-sync-loss
        // error exactly once. Lenient mode populates this when
        // `recover_sync` exhausts; emitting it as a terminal Err item
        // (instead of silently returning None) lets the CLI distinguish
        // exit 3 from a clean completion and lets `--allow-partial`
        // commit the partial output.
        if let Some(err) = self.pending_unrecoverable.take() {
            self.done = true;
            return Some(Err(err));
        }

        loop {
            // Need at least a Type Word + minimum-format payload.
            if self.offset + MIN_RECORD_BYTES_STANDARD > self.file_len {
                self.done = true;
                self.log_complete();
                return None;
            }

            // ── Read Type Word ─────────────────────────────────────
            let Some(type_raw) = read_u16(self.data, self.offset) else {
                self.done = true;
                return None;
            };
            let tw = decode_type_word(type_raw);

            // L2-DEC-011 / L2-DEC-015: the timestamp format is now
            // resolved eagerly in iter() (multi-record probe), so by
            // the time we reach next() the format is already chosen
            // and stays fixed for the rest of the decode.
            let resolved = self.resolved_format;
            let ts_words = timestamp_word_count(resolved);
            let record_bytes = usize::from(tw.word_count) * 2;

            // ── Validate this record ────────────────────────────────
            // Delegate to sync::validate_record so the per-record path
            // matches the header-skip and recovery paths exactly. This
            // applies all five heuristics: valid type, plausible word
            // count, fits in file, IRIG field ranges, and two-record
            // look-ahead. A weaker inline check would let corrupt-but-
            // plausible records slip through and be emitted as garbage
            // rows.
            let validation = validate_record_detailed(
                self.data,
                self.offset,
                self.file_len,
                Some(resolved),
                self.lookahead_records,
            );

            if let Err(failure) = validation {
                self.sync_losses += 1;
                self.sync_losses_atomic.fetch_add(1, Ordering::Relaxed);
                log_validation_context(self.data, self.offset);
                if self.strict {
                    let err = match failure {
                        ValidationFailure::UnknownMessageType => MieError::UnknownTypeWord {
                            offset: self.offset as u64,
                            raw_type_word: type_raw,
                            message_type: tw.message_type,
                        },
                        ValidationFailure::InvalidWordCount => MieError::InvalidTypeWord {
                            offset: self.offset as u64,
                            raw_type_word: type_raw,
                            word_count: tw.word_count,
                        },
                        ValidationFailure::RecordTruncated => MieError::RecordTruncated {
                            offset: self.offset as u64,
                            record_bytes: record_bytes as u64,
                            available_bytes: self.file_len.saturating_sub(self.offset) as u64,
                        },
                        other => MieError::PayloadError {
                            offset: self.offset as u64,
                            detail: format!("{other} (raw_type=0x{type_raw:04X})"),
                        },
                    };
                    self.done = true;
                    return Some(Err(err));
                }

                log_warn!(
                    "sync lost at 0x{:X} (type=0x{:02X} wc={}); scanning forward",
                    self.offset,
                    tw.message_type,
                    tw.word_count
                );
                match recover_sync(
                    self.data,
                    self.offset,
                    self.file_len,
                    Some(self.resolved_format),
                    MAX_SCAN_BYTES,
                    self.lookahead_records,
                ) {
                    Some(hit) => {
                        log_info!(
                            "sync recovered at 0x{:X} (skipped {} bytes from 0x{:X})",
                            hit.offset,
                            hit.skipped,
                            self.offset
                        );
                        self.offset = hit.offset;
                        self.prev_was_error = false;
                        continue;
                    }
                    None => {
                        // Distinguish truncation (ran out of file
                        // before the scan window exhausted) from
                        // genuine mid-file corruption (full 64 KB
                        // window scanned, no valid record).
                        //
                        // - Truncation → L1-DEC-005 / L2-RDR-002: lenient
                        //   mode stops cleanly with no error.
                        // - Corruption → L1-EXIT-004: surface as terminal
                        //   `UnrecoverableSyncLoss` so the CLI maps to
                        //   exit 3 (or to a `.partial` commit + exit
                        //   0 when `--allow-partial` is set).
                        let bytes_remaining = self.file_len.saturating_sub(self.offset);
                        if bytes_remaining < MAX_SCAN_BYTES {
                            log_info!(
                                "lenient mode: scan exhausted at EOF \
                                 (offset 0x{:X}, {} bytes remain < {} \
                                 scan window); treating as truncation",
                                self.offset,
                                bytes_remaining,
                                MAX_SCAN_BYTES
                            );
                            self.done = true;
                            self.log_complete();
                            return None;
                        }
                        log_error!(
                            "unrecoverable sync loss at 0x{:X} after {} messages",
                            self.offset,
                            self.msg_count
                        );
                        // Stash a terminal Err so the next next() call
                        // surfaces UnrecoverableSyncLoss exactly once.
                        // The writer can then either commit a `.partial`
                        // (allow_partial) or unlink the temp + propagate
                        // for exit 3.
                        self.pending_unrecoverable = Some(MieError::UnrecoverableSyncLoss {
                            offset: self.offset as u64,
                            sync_losses: self.sync_losses,
                        });
                        self.log_complete();
                        // Recurse once to pop the pending error and set
                        // `done`. Single recursion: the branch above
                        // returns before re-entering this loop.
                        return self.next();
                    }
                }
            }

            // ── Decode timestamp ───────────────────────────────────
            let timestamp = match resolved {
                TimestampFormat::Irig => {
                    let upper = read_u16(self.data, self.offset + 2)?;
                    let middle = read_u16(self.data, self.offset + 4)?;
                    let lower = read_u16(self.data, self.offset + 6)?;
                    let irig = decode_irig_timestamp(upper, middle, lower);
                    if irig.freerun {
                        log_warn!("freerun timestamp at 0x{:X}", self.offset);
                    } else if !self.warned_irig_day {
                        // PRA-9: the IRIG day-of-year field has a known
                        // firmware-dependent decode discrepancy on some DDC
                        // cards; the time-of-day fields are unaffected. Emit a
                        // one-time advisory (not a decode failure) so the
                        // operator is nudged to the documented limitation.
                        self.warned_irig_day = true;
                        log_warn!(
                            "IRIG day-of-year decoded for this recording; the day-of-year field \
                             has a known firmware-dependent discrepancy on some DDC cards \
                             (hour/minute/second/microsecond are unaffected) — see \
                             docs/VENDOR-CSV-DIFFS.md §5"
                        );
                    }
                    Timestamp::Irig(irig)
                }
                TimestampFormat::Standard => {
                    let upper = read_u16(self.data, self.offset + 2)?;
                    let lower = read_u16(self.data, self.offset + 4)?;
                    Timestamp::Standard(decode_standard_timestamp(upper, lower))
                }
                TimestampFormat::Auto => unreachable!(),
            };

            let cmd_byte_offset = self.offset + 2 + usize::from(ts_words) * 2;

            // ── SPURIOUS_DATA: no Command Word ─────────────────────
            if tw.message_type == MessageType::SpuriousData as u8 {
                let raw_word_count = i32::from(tw.word_count) - 1 - i32::from(ts_words);
                let mut data_words = DataWords::new();
                if raw_word_count > 0 {
                    let n = raw_word_count as usize;
                    let mut buf = [0u16; crate::models::MAX_DATA_WORDS];
                    let n_capped = n.min(crate::models::MAX_DATA_WORDS);
                    // Bound the read to the current record. raw_word_count is
                    // computed from tw.word_count so this is structurally safe
                    // already; bounding is defense-in-depth in case the math
                    // is refactored later.
                    let record_end = self.offset + record_bytes;
                    let record_data = &self.data[..record_end];
                    if read_u16_array(record_data, cmd_byte_offset, n_capped, &mut buf) {
                        data_words = DataWords::from_slice(&buf[..n_capped]);
                    }
                }

                let error_code = if self.prev_was_error {
                    ERROR_SPURIOUS_CONTINUATION
                } else {
                    ERROR_SPURIOUS_STANDALONE
                };
                log_debug!(
                    "SPURIOUS_DATA at 0x{:X}: {} raw words, {}",
                    self.offset,
                    raw_word_count.max(0),
                    if self.prev_was_error {
                        "continuation"
                    } else {
                        "standalone"
                    }
                );

                // SPURIOUS_DATA has no RT/MSG key and is never tracked
                // for DELTA. The CSV writer emits an empty DELTA cell.
                let msg = MieMessage {
                    timestamp,
                    type_word: tw,
                    message_format: MessageFormat::SpuriousData,
                    command_word: None,
                    command_word_2: None,
                    status_word: None,
                    status_word_2: None,
                    data_words,
                    error_word: Some(error_code),
                    delta: None,
                    file_offset: self.offset as u64,
                };
                self.advance_after_yield(record_bytes);
                self.prev_was_error = false;
                return Some(Ok(msg));
            }

            // ── Decode Command Word ────────────────────────────────
            let Some(cmd_raw) = read_u16(self.data, cmd_byte_offset) else {
                self.done = true;
                return None;
            };
            let cmd = decode_command_word(cmd_raw);

            // ── Errored record (bit 14 set) ─────────────────────────
            if tw.error {
                let key = delta_key(
                    cmd.rt,
                    cmd.subaddress,
                    matches!(cmd.direction, crate::models::Direction::Transmit),
                );
                let delta = self.delta_for(key, &timestamp);
                let msg = self.decode_error_record(
                    &tw,
                    timestamp,
                    &cmd,
                    cmd_byte_offset,
                    ts_words,
                    delta,
                );
                self.advance_after_yield(record_bytes);
                self.prev_was_error = true;
                return Some(msg);
            }

            // ── Normal record: classify and extract payload ────────
            let msg_fmt = match classify_message_format(tw.message_type, &cmd, tw.word_count) {
                Ok(f) => f,
                Err(_) => {
                    log_warn!(
                        "cannot classify record at 0x{:X} (type=0x{:02X}); skipping",
                        self.offset,
                        tw.message_type
                    );
                    self.offset += record_bytes;
                    self.prev_was_error = false;
                    continue;
                }
            };

            log_debug!(
                "record at 0x{:X}: type=0x{:02X} fmt={:?} RT{} SA{}",
                self.offset,
                tw.message_type,
                msg_fmt,
                cmd.rt,
                cmd.subaddress
            );

            // L2-SYN-020..025: structural invariants. Strict mode aborts;
            // lenient mode logs a WARN, advances past the record, and
            // continues iteration.
            if let Err(v) =
                crate::decode::validate_structural_invariants(&tw, &cmd, msg_fmt, ts_words)
            {
                if self.strict {
                    self.done = true;
                    return Some(Err(crate::error::MieError::PayloadError {
                        offset: self.offset as u64,
                        detail: format!("L2-SYN structural invariant violation: {}", v.detail),
                    }));
                }
                log_warn!(
                    "L2-SYN structural invariant violation at 0x{:X}: {}; skipping record",
                    self.offset,
                    v.detail
                );
                self.offset += record_bytes;
                self.prev_was_error = false;
                continue;
            }

            // Bound the slice to the current record's byte range so
            // payload reads can never spill into the next record. The
            // Type Word's word_count defines the record length; a
            // Command Word that *claims* a larger data_word_count than
            // the record can hold must NOT cause us to read those
            // extra words from whatever follows.
            let record_end = self.offset + record_bytes;
            let record_data = &self.data[..record_end];
            let payload_offset = cmd_byte_offset + 2;
            let (cmd2, status, status2, data_words) =
                extract_payload(record_data, payload_offset, msg_fmt, &cmd);

            // L2-SYN-023 / L2-SYN-027: post-extract Reject-class checks
            // (Cmd2 direction and Cmd1/Cmd2 data_word_count agreement for
            // RT-to-RT formats). Same strict/lenient policy as the
            // pre-extract invariants.
            if let Err(v) =
                crate::decode::validate_post_extract_invariants(msg_fmt, &cmd, cmd2.as_ref())
            {
                if self.strict {
                    self.done = true;
                    return Some(Err(crate::error::MieError::PayloadError {
                        offset: self.offset as u64,
                        detail: format!("L2-SYN structural invariant violation: {}", v.detail),
                    }));
                }
                log_warn!(
                    "L2-SYN structural invariant violation at 0x{:X}: {}; skipping record",
                    self.offset,
                    v.detail
                );
                self.offset += record_bytes;
                self.prev_was_error = false;
                continue;
            }

            // L2-SYN-024 / L2-SYN-025: AnomalyWarn-class observations.
            // Both modes log a WARN and continue emitting the record.
            for v in crate::decode::detect_record_anomalies(&tw, &cmd, status) {
                log_warn!("L2-SYN anomaly at 0x{:X}: {}", self.offset, v.detail);
            }

            let key = delta_key(
                cmd.rt,
                cmd.subaddress,
                matches!(cmd.direction, crate::models::Direction::Transmit),
            );
            let delta = self.delta_for(key, &timestamp);

            let msg = MieMessage {
                timestamp,
                type_word: tw,
                message_format: msg_fmt,
                command_word: Some(cmd),
                command_word_2: cmd2,
                status_word: status,
                status_word_2: status2,
                data_words,
                error_word: None,
                delta,
                file_offset: self.offset as u64,
            };
            self.advance_after_yield(record_bytes);
            self.prev_was_error = false;

            if self.msg_count > 0 && self.msg_count % 100_000 == 0 {
                log_info!(
                    "decoded {} messages (0x{:X} / 0x{:X})",
                    self.msg_count,
                    self.offset,
                    self.file_len
                );
            }

            return Some(Ok(msg));
        }
    }
}

impl<'a> RecordIter<'a> {
    fn advance_after_yield(&mut self, record_bytes: usize) {
        self.offset += record_bytes;
        self.msg_count += 1;
    }

    fn log_complete(&self) {
        log_info!(
            "decode complete: {} messages, {} sync recoveries, format={:?}, file={}",
            self.msg_count,
            self.sync_losses,
            self.resolved_format,
            self.path_for_log.display()
        );
    }

    fn decode_error_record(
        &self,
        tw: &TypeWord,
        timestamp: Timestamp,
        cmd: &CommandWord,
        cmd_byte_offset: usize,
        ts_words: u16,
        delta: Option<f64>,
    ) -> MieResult<MieMessage> {
        let error_word_offset = self.offset + (usize::from(tw.word_count) - 1) * 2;
        let error_code = match read_u16(self.data, error_word_offset) {
            Some(c) => c,
            None => {
                return Err(MieError::PayloadError {
                    offset: self.offset as u64,
                    detail: "error word out of bounds".into(),
                });
            }
        };

        if !is_known_ddc_error_code(error_code) {
            if self.strict {
                return Err(MieError::UnknownErrorCode {
                    offset: self.offset as u64,
                    error_code,
                });
            }
            log_warn!(
                "unknown DDC error code 0x{:04X} at 0x{:X}",
                error_code,
                self.offset
            );
        }

        // Payload words = total - Type(1) - TS - Cmd(1) - ErrorWord(1)
        let payload_words = i32::from(tw.word_count) - 1 - i32::from(ts_words) - 1 - 1;
        let mut data_words = DataWords::new();
        if payload_words > 0 {
            let n = payload_words as usize;
            let n_capped = n.min(crate::models::MAX_DATA_WORDS);
            let mut buf = [0u16; crate::models::MAX_DATA_WORDS];
            // Bound to the current record's bytes. payload_words is
            // already derived from tw.word_count, so this is structurally
            // safe; bounding makes it explicit.
            let record_end = self.offset + usize::from(tw.word_count) * 2;
            let record_data = &self.data[..record_end];
            if read_u16_array(record_data, cmd_byte_offset + 2, n_capped, &mut buf) {
                data_words = DataWords::from_slice(&buf[..n_capped]);
            }
        }

        let msg_fmt = classify_message_format(tw.message_type, cmd, tw.word_count)
            .unwrap_or(MessageFormat::Receive);

        log_info!(
            "error record at 0x{:X}: RT{} SA{} code=0x{:04X} ({}), {} payload words",
            self.offset,
            cmd.rt,
            cmd.subaddress,
            error_code,
            ddc_error_description(error_code),
            payload_words.max(0),
        );

        Ok(MieMessage {
            timestamp,
            type_word: *tw,
            message_format: msg_fmt,
            command_word: Some(*cmd),
            command_word_2: None,
            status_word: None,
            status_word_2: None,
            data_words,
            error_word: Some(error_code),
            delta,
            file_offset: self.offset as u64,
        })
    }

    /// Compute DELTA for `key` given the current record's `timestamp`,
    /// and update the tracker accordingly. Implements the shared contract:
    ///
    /// - `Timestamp::to_microseconds()` returns `None` (Standard with no
    ///   configured tick rate) → return `None` and skip tracker update
    ///   (nothing to compare against).
    /// - First occurrence of `key` → return `Some(0.0)`, record current us.
    /// - Subsequent with non-negative gap → return `Some(seconds)`, record current us.
    /// - Subsequent with negative gap (non-monotonic) → return `None`, record
    ///   current us, emit a WARN once per key per recording.
    fn delta_for(&mut self, key: u32, timestamp: &Timestamp) -> Option<f64> {
        let curr_us = timestamp.to_microseconds(self.standard_tick_rate_hz)?;
        let result = match self.delta_tracker.get(&key) {
            None => Some(0.0),
            Some(&prev) if curr_us >= prev => Some((curr_us - prev) as f64 / 1_000_000.0),
            Some(&prev) => {
                if self.warned_ooo_keys.insert(key) {
                    log_warn!(
                        "non-monotonic timestamp at 0x{:X} for RT/MSG key 0x{:08X}: \
                         prev_us={} curr_us={} (further out-of-order occurrences for \
                         this key suppressed)",
                        self.offset,
                        key,
                        prev,
                        curr_us
                    );
                }
                None
            }
        };
        self.delta_tracker.insert(key, curr_us);
        result
    }
}

/// Per-format payload extraction. Returns the second command word (for
/// RT-to-RT formats), primary status, secondary status, and data words.
fn extract_payload(
    data: &[u8],
    p: usize,
    fmt: MessageFormat,
    cmd: &CommandWord,
) -> (Option<CommandWord>, Option<u16>, Option<u16>, DataWords) {
    use MessageFormat::*;

    let read_n = |start: usize, n: usize| -> DataWords {
        let mut buf = [0u16; crate::models::MAX_DATA_WORDS];
        let n_capped = n.min(crate::models::MAX_DATA_WORDS);
        if read_u16_array(data, start, n_capped, &mut buf) {
            DataWords::from_slice(&buf[..n_capped])
        } else {
            DataWords::new()
        }
    };

    match fmt {
        Receive => {
            let n = usize::from(cmd.data_word_count);
            let dw = read_n(p, n);
            let status = read_u16(data, p + n * 2);
            (None, status, None, dw)
        }
        Transmit => {
            let status = read_u16(data, p);
            let n = usize::from(cmd.data_word_count);
            let dw = read_n(p + 2, n);
            (None, status, None, dw)
        }
        RtToRt => {
            let cmd2_raw = read_u16(data, p).unwrap_or(0);
            let cmd2 = decode_command_word(cmd2_raw);
            let tx_status = read_u16(data, p + 2);
            let n = usize::from(cmd2.data_word_count);
            let dw = read_n(p + 4, n);
            let rx_status = read_u16(data, p + 4 + n * 2);
            (Some(cmd2), tx_status, rx_status, dw)
        }
        ReceiveBroadcast => {
            let n = usize::from(cmd.data_word_count);
            let dw = read_n(p, n);
            (None, None, None, dw)
        }
        RtToRtBroadcast => {
            let cmd2_raw = read_u16(data, p).unwrap_or(0);
            let cmd2 = decode_command_word(cmd2_raw);
            let tx_status = read_u16(data, p + 2);
            let n = usize::from(cmd2.data_word_count);
            let dw = read_n(p + 4, n);
            (Some(cmd2), tx_status, None, dw)
        }
        ModeCodeTxData => {
            let status = read_u16(data, p);
            let dw = match read_u16(data, p + 2) {
                Some(w) => DataWords::from_slice(&[w]),
                None => DataWords::new(),
            };
            (None, status, None, dw)
        }
        ModeCodeRxData => {
            let dw = match read_u16(data, p) {
                Some(w) => DataWords::from_slice(&[w]),
                None => DataWords::new(),
            };
            let status = read_u16(data, p + 2);
            (None, status, None, dw)
        }
        ModeCodeNoData => {
            let status = read_u16(data, p);
            (None, status, None, DataWords::new())
        }
        ModeCodeBcastNoData => (None, None, None, DataWords::new()),
        ModeCodeBcastData => {
            let dw = match read_u16(data, p) {
                Some(w) => DataWords::from_slice(&[w]),
                None => DataWords::new(),
            };
            (None, None, None, dw)
        }
        SpuriousData => (None, None, None, DataWords::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Minimal temp-file helper. Removes itself on drop.
    struct TempFile(PathBuf);
    impl TempFile {
        fn write(bytes: &[u8]) -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let pid = std::process::id();
            let path = std::env::temp_dir().join(format!("mie-decoder-test-{pid}-{n}.bin"));
            let mut f = std::fs::File::create(&path).unwrap();
            f.write_all(bytes).unwrap();
            f.flush().unwrap();
            Self(path)
        }
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    // The Python `tests/conftest.py` RECORD_RT15_SA11_RCV fixture, as a
    // ground-truth byte sequence we can mmap and decode end-to-end.
    fn rt15_sa11_rcv() -> Vec<u8> {
        // 1 type + 3 ts + 1 cmd + 30 data + 1 status = 36 words = 72 bytes
        let mut hex = String::new();
        hex.push_str("02240F1826DB21F6"); // Type 0x2402 + TS (8 bytes)
        hex.push_str("7E79"); // Cmd 0x797E (RT15, R, SA11, 30 dw)
        hex.push_str("0004"); // wd01
        hex.push_str("0000"); // wd02
        hex.push_str("0000"); // wd03
        hex.push_str("2F00"); // wd04
        hex.push_str("22CA"); // wd05
        hex.push_str("2F00"); // wd06
        hex.push_str("22CA"); // wd07
        // wd08–wd29 zero (22 words = 88 hex chars)
        for _ in 0..22 {
            hex.push_str("0000");
        }
        hex.push_str("71C7"); // wd30
        hex.push_str("0078"); // status 0x7800
        hex_decode(&hex)
    }

    fn hex_decode(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    fn write_temp(bytes: &[u8]) -> TempFile {
        TempFile::write(bytes)
    }

    /// Requirements: L2-RDR-005
    #[test]
    fn rejects_missing_file() {
        match MieFileReader::new("/no/such/path/12345.mie") {
            Err(e) => assert_eq!(e.kind(), crate::error::MieErrorKind::FileNotFound),
            Ok(_) => panic!("expected FileNotFound"),
        }
    }

    /// Requirements: L2-RDR-006
    #[test]
    fn rejects_empty_file() {
        let f = write_temp(&[]);
        match MieFileReader::new(f.path()) {
            Err(e) => assert_eq!(e.kind(), crate::error::MieErrorKind::FileEmpty),
            Ok(_) => panic!("expected FileEmpty"),
        }
    }

    /// Requirements: L2-RDR-007, L3-RS-003
    #[test]
    fn decodes_rt15_sa11_record() {
        let bytes = rt15_sa11_rcv();
        assert_eq!(bytes.len(), 72);
        let f = write_temp(&bytes);
        let reader = MieFileReader::new(f.path()).unwrap();
        let msgs: Vec<_> = reader.iter().collect::<Result<_, _>>().unwrap();
        assert_eq!(msgs.len(), 1);
        let m = &msgs[0];
        assert_eq!(m.command_word.unwrap().rt, 15);
        assert_eq!(m.command_word.unwrap().subaddress, 11);
        assert_eq!(m.message_format, MessageFormat::Receive);
        assert_eq!(m.data_words.len(), 30);
        assert_eq!(m.status_word, Some(0x7800));
        assert_eq!(m.file_offset, 0);
    }

    /// Requirements: L2-SYN-006
    #[test]
    fn skips_proprietary_header() {
        let mut bytes = b"DDC-EQUIPMENT-NAME\0\0".to_vec(); // 20 bytes, even
        bytes.extend(rt15_sa11_rcv());
        let f = write_temp(&bytes);
        let reader = MieFileReader::new(f.path()).unwrap();
        let msgs: Vec<_> = reader.iter().collect::<Result<_, _>>().unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].file_offset, 20);
    }

    /// Regression: a non-empty file with no decodable records must
    /// surface MieError::NoValidRecords on the first iter() call,
    /// then return None forever after. Previously the iterator was
    /// silently marked done so callers like `count` and `decode` saw
    /// zero messages and exited successfully — producing a header-only
    /// CSV for a TOML file passed as input.
    /// Requirements: L2-SYN-011, L1-EXIT-002
    #[test]
    fn no_valid_records_surfaces_as_iter_error() {
        // 1 KB of 0xFF — definitely no valid Type Word (message_type
        // bits = 0x7F) and no chance of a coincidental valid record.
        let bytes = vec![0xFFu8; 1024];
        let f = write_temp(&bytes);
        let reader = MieFileReader::new(f.path()).unwrap();
        let mut it = reader.iter();

        // First call: Err(NoValidRecords).
        match it.next() {
            Some(Err(e)) => {
                assert_eq!(e.kind(), crate::error::MieErrorKind::NoValidRecords);
                if let crate::error::MieError::NoValidRecords { scan_bytes, .. } = e {
                    assert!(scan_bytes > 0);
                    assert!(scan_bytes <= 1024);
                } else {
                    unreachable!()
                }
            }
            other => panic!("expected Some(Err(NoValidRecords)), got {other:?}"),
        }

        // Subsequent calls: None forever.
        assert!(it.next().is_none());
        assert!(it.next().is_none());
    }

    /// Regression: L2-RDR-004. A file that contains a structurally-
    /// valid Type Word whose declared extent runs past EOF SHALL surface
    /// MieError::FirstRecordTruncated in strict mode (distinct from the
    /// generic RecordTruncated) and SHALL terminate cleanly with zero
    /// records in lenient mode.
    /// Requirements: L2-RDR-004
    #[test]
    fn first_record_truncated_strict_raises_distinct_error() {
        // First 20 bytes of a 72-byte record: Type Word looks valid
        // (msg_type=0x02, bus A, wc=36) but the declared 72-byte extent
        // runs past EOF.
        let full = rt15_sa11_rcv();
        let truncated = &full[..20];
        let f = write_temp(truncated);
        let reader = MieFileReader::with_options(
            f.path(),
            ReaderOptions {
                strict: true,
                time_format: TimestampFormat::Auto,
                detect_records: DEFAULT_DETECT_RECORDS,
                lookahead_records: DEFAULT_LOOKAHEAD_RECORDS,
                standard_tick_rate_hz: None,
            },
        )
        .unwrap();
        let mut it = reader.iter();
        match it.next() {
            Some(Err(e)) => {
                assert_eq!(
                    e.kind(),
                    crate::error::MieErrorKind::FirstRecordTruncated,
                    "expected FirstRecordTruncated, got {:?}",
                    e.kind()
                );
                if let crate::error::MieError::FirstRecordTruncated {
                    record_bytes,
                    available_bytes,
                    ..
                } = e
                {
                    assert_eq!(record_bytes, 72);
                    assert_eq!(available_bytes, 20);
                } else {
                    unreachable!()
                }
            }
            other => panic!("expected Some(Err(FirstRecordTruncated)), got {other:?}"),
        }
        assert!(it.next().is_none());
    }

    /// Requirements: L2-RDR-004
    #[test]
    fn first_record_truncated_lenient_terminates_clean() {
        let full = rt15_sa11_rcv();
        let truncated = &full[..20];
        let f = write_temp(truncated);
        let reader = MieFileReader::new(f.path()).unwrap();
        let msgs: Vec<_> = reader.iter().collect::<Result<_, _>>().unwrap();
        assert!(msgs.is_empty(), "lenient mode SHALL yield zero records");
    }

    /// L2-DEC-013: forcing the wrong timestamp format on a recording the
    /// detection probe is *decisive* about SHALL surface a mismatch in
    /// strict mode rather than silently emit garbage timestamps.
    /// Requirements: L2-DEC-013
    #[test]
    fn forced_format_mismatch_strict_errors() {
        // Two valid IRIG records → the probe is decisive for IRIG.
        let mut data = rt15_sa11_rcv();
        data.extend(rt15_sa11_rcv());
        let f = write_temp(&data);
        let reader = MieFileReader::with_options(
            f.path(),
            ReaderOptions {
                strict: true,
                time_format: TimestampFormat::Standard,
                detect_records: DEFAULT_DETECT_RECORDS,
                lookahead_records: DEFAULT_LOOKAHEAD_RECORDS,
                standard_tick_rate_hz: None,
            },
        )
        .unwrap();
        match reader.iter().next() {
            Some(Err(e)) => assert_eq!(
                e.kind(),
                crate::error::MieErrorKind::TimestampFormatMismatch,
                "expected TimestampFormatMismatch, got {:?}",
                e.kind()
            ),
            other => panic!("expected Some(Err(TimestampFormatMismatch)), got {other:?}"),
        }
    }

    /// L2-DEC-013: in lenient mode the same forced-format mismatch SHALL
    /// log a WARN but proceed with the forced format rather than abort.
    /// Requirements: L2-DEC-013
    #[test]
    fn forced_format_mismatch_lenient_proceeds() {
        let mut data = rt15_sa11_rcv();
        data.extend(rt15_sa11_rcv());
        let f = write_temp(&data);
        let reader = MieFileReader::with_options(
            f.path(),
            ReaderOptions {
                strict: false,
                time_format: TimestampFormat::Standard,
                detect_records: DEFAULT_DETECT_RECORDS,
                lookahead_records: DEFAULT_LOOKAHEAD_RECORDS,
                standard_tick_rate_hz: None,
            },
        )
        .unwrap();
        // No terminal error: lenient mode decodes with the forced format
        // (records may be skipped on invariant violations, but the stream
        // does not abort).
        let result: Result<Vec<_>, _> = reader.iter().collect();
        assert!(
            result.is_ok(),
            "lenient forced-format mismatch must not abort the stream"
        );
    }

    /// L2-DEC-013: forcing a format the probe agrees with (or is not
    /// decisive against) SHALL NOT trip the mismatch check.
    /// Requirements: L2-DEC-013
    #[test]
    fn forced_format_matching_is_not_flagged() {
        let mut data = rt15_sa11_rcv();
        data.extend(rt15_sa11_rcv());
        let f = write_temp(&data);
        let reader = MieFileReader::with_options(
            f.path(),
            ReaderOptions {
                strict: true,
                time_format: TimestampFormat::Irig,
                detect_records: DEFAULT_DETECT_RECORDS,
                lookahead_records: DEFAULT_LOOKAHEAD_RECORDS,
                standard_tick_rate_hz: None,
            },
        )
        .unwrap();
        let msgs: Vec<_> = reader
            .iter()
            .collect::<Result<_, _>>()
            .expect("forcing the correct format must decode cleanly");
        assert_eq!(msgs.len(), 2);
    }

    /// L2-SYN-018: 0x20-fill parses as a SPURIOUS_DATA Type Word
    /// (msg_type=0x20, wc=32) and passes basic validation, but every
    /// "record" is byte-identical to its successor. The reader SHALL
    /// reject the input with MieError::HomogeneousPayload rather than
    /// emit a torrent of synthetic SPURIOUS_DATA frames.
    /// Requirements: L2-SYN-018
    #[test]
    fn homogeneous_payload_input_rejected() {
        // 1 KB of 0x20 — enough for 4 candidate records of 64 bytes each.
        let bytes = vec![0x20u8; 1024];
        let f = write_temp(&bytes);
        let reader = MieFileReader::new(f.path()).unwrap();
        let mut it = reader.iter();
        match it.next() {
            Some(Err(e)) => {
                assert_eq!(
                    e.kind(),
                    crate::error::MieErrorKind::HomogeneousPayload,
                    "expected HomogeneousPayload, got {:?}",
                    e.kind()
                );
            }
            other => panic!("expected Some(Err(HomogeneousPayload)), got {other:?}"),
        }
        // Subsequent calls: None forever.
        assert!(it.next().is_none());
    }

    /// L2-SYN-018: the defense SHALL NOT false-positive on legitimate
    /// recordings whose payload bytes vary between records. The
    /// canonical RT15-SA11 fixture replicated 4 times has identical
    /// bytes everywhere (including timestamp triple, which is the same
    /// fixture), so it would trip the defense — but real multi-record
    /// streams use varied records. Test with the 3-record multi stream
    /// used by other tests.
    /// Requirements: L2-SYN-018
    #[test]
    fn non_homogeneous_valid_records_accepted() {
        // Stitch together 3 of the canonical records, then a 4th of
        // a different type to make sure consecutive candidate-sized
        // chunks differ in non-timestamp bytes.
        let r1 = rt15_sa11_rcv(); // 72 bytes, type 0x02
        let mut data = Vec::new();
        data.extend_from_slice(&r1);
        // Second record: same shape but with non-zero data words so
        // the byte content differs from r1.
        let mut r2 = r1.clone();
        // Patch Cmd word position to a different value to break payload
        // identity outside the timestamp range.
        if r2.len() > 9 {
            r2[8] = 0xCB;
            r2[9] = 0x7A;
        }
        data.extend_from_slice(&r2);
        data.extend_from_slice(&r1);
        data.extend_from_slice(&r2);
        let f = write_temp(&data);
        let reader = MieFileReader::new(f.path()).unwrap();
        // Should decode without HomogeneousPayload firing. The records
        // may or may not all decode cleanly (the patched CmdWord may
        // trigger an L2-SYN invariant rejection in lenient mode), but
        // we should NOT see a HomogeneousPayload error.
        let result: Result<Vec<_>, _> = reader.iter().collect();
        if let Err(e) = result {
            assert_ne!(
                e.kind(),
                crate::error::MieErrorKind::HomogeneousPayload,
                "defense false-fired on legitimately varied records: {e}"
            );
        }
    }
}
