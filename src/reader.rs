//! mmap-backed sequential reader.
//!
//! `MieFileReader` opens an MIE binary file with `memmap2`, finds the first
//! valid record (skipping any header), auto-detects the timestamp format on
//! first record if requested, and yields decoded `MieMessage`s in file order.
//!
//! Sync recovery happens internally — only unrecoverable errors (or strict
//! mode opt-ins) surface as `Err` items from the iterator.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use memmap2::Mmap;

use crate::decode::{
    MIN_RECORD_BYTES_STANDARD, classify_message_format, decode_command_word, decode_irig_timestamp,
    decode_standard_timestamp, decode_type_word, detect_timestamp_format, message_type_is_valid,
    read_u16, read_u16_array,
};
use crate::error::{MieError, MieResult};
use crate::models::{
    CommandWord, DataWords, ERROR_SPURIOUS_CONTINUATION, ERROR_SPURIOUS_STANDALONE, MessageFormat,
    MessageType, MieMessage, Timestamp, TimestampFormat, TypeWord, ddc_error_description,
    is_known_ddc_error_code, timestamp_word_count,
};
use crate::sync::{MAX_SCAN_BYTES, find_first_record, recover_sync};
use crate::{log_debug, log_error, log_info, log_warn};

/// Reader handle. Construct with [`new`]; iterate by calling `.iter()` or
/// using `IntoIterator`.
pub struct MieFileReader {
    path: PathBuf,
    mmap: Mmap,
    file_size: u64,
    strict: bool,
    time_format: TimestampFormat,
    /// Cumulative sync-recovery attempts during the most recent iter()
    /// call. Reset to 0 at the start of each iter(). Shared with the
    /// active RecordIter via a reference so the CLI can query it
    /// post-iteration (e.g., to distinguish L1-022 partial-recovered
    /// from L1-021 complete in the exit-class summary).
    sync_losses: AtomicU64,
}

/// Builder-style options. `strict=false`, `time_format=Auto` by default.
#[derive(Debug, Clone, Copy)]
pub struct ReaderOptions {
    pub strict: bool,
    pub time_format: TimestampFormat,
}

impl Default for ReaderOptions {
    fn default() -> Self {
        Self {
            strict: false,
            time_format: TimestampFormat::Auto,
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

        // SAFETY: we take a read-only mmap of an already-opened file. The
        // file is moved into the closure; the mmap holds it alive for its
        // lifetime. We don't expose the underlying bytes outside the reader.
        let mmap = unsafe { Mmap::map(&file) }.map_err(|source| MieError::FileIo {
            path: path.clone(),
            source,
        })?;

        log_debug!(
            "reader opened {} ({} bytes, strict={}, time_format={:?})",
            path.display(),
            file_size,
            opts.strict,
            opts.time_format
        );

        Ok(Self {
            path,
            mmap,
            file_size,
            strict: opts.strict,
            time_format: opts.time_format,
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
    /// iterator is exhausted to derive the L1-022/024 exit class.
    pub fn sync_losses(&self) -> u64 {
        self.sync_losses.load(Ordering::Relaxed)
    }

    /// Borrow an iterator over decoded messages.
    pub fn iter(&self) -> RecordIter<'_> {
        // Reset the per-call counter so successive iter() calls on the
        // same reader handle don't accumulate stale counts.
        self.sync_losses.store(0, Ordering::Relaxed);

        let resolved_format = if self.time_format == TimestampFormat::Auto {
            None
        } else {
            Some(self.time_format)
        };

        let data: &[u8] = &self.mmap;
        let file_len = data.len();

        let start_offset = find_first_record(data, file_len, resolved_format, MAX_SCAN_BYTES);

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
                None
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
        };

        log_info!("beginning decode of {}", self.path.display());

        RecordIter {
            data,
            file_len,
            offset: start_offset.map(|h| h.offset).unwrap_or(file_len),
            done: false,
            pending_error,
            pending_unrecoverable: None,
            strict: self.strict,
            resolved_format,
            prev_was_error: false,
            delta_tracker: HashMap::new(),
            warned_ooo_keys: HashSet::new(),
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
    /// errors. Per L1-023 the CLI catches this variant to decide
    /// between exit 3 (default) and a `.partial` commit + exit 0
    /// (when `--allow-partial` is set).
    pending_unrecoverable: Option<MieError>,
    strict: bool,
    resolved_format: Option<TimestampFormat>,
    prev_was_error: bool,
    /// Per-RT/MSG last-seen timestamp in microseconds. Only populated when
    /// the source timestamp has a microsecond basis (IRIG today). Standard
    /// timestamps yield None from `Timestamp::to_microseconds()` and bypass
    /// the tracker entirely.
    delta_tracker: HashMap<u32, u64>,
    /// RT/MSG keys for which a non-monotonic-timestamp WARN has already
    /// been emitted. Limits log volume on chronically out-of-order files
    /// to one line per key per recording.
    warned_ooo_keys: HashSet<u32>,
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

            // Auto-detect timestamp format on the first valid record.
            if self.resolved_format.is_none() {
                let fmt = detect_timestamp_format(self.data, self.offset, &tw);
                log_info!("auto-detected timestamp format: {:?}", fmt);
                self.resolved_format = Some(fmt);
            }
            let resolved = self.resolved_format.unwrap();
            let ts_words = timestamp_word_count(resolved);
            let min_wc = 1 + ts_words + 1;
            let record_bytes = usize::from(tw.word_count) * 2;

            // ── Validate this record ────────────────────────────────
            // Delegate to sync::validate_record so the per-record path
            // matches the header-skip and recovery paths exactly. This
            // applies all five heuristics: valid type, plausible word
            // count, fits in file, IRIG field ranges, and two-record
            // look-ahead. A weaker inline check would let corrupt-but-
            // plausible records slip through and be emitted as garbage
            // rows.
            let is_valid =
                crate::sync::validate_record(self.data, self.offset, self.file_len, Some(resolved));

            if !is_valid {
                self.sync_losses += 1;
                self.sync_losses_atomic.fetch_add(1, Ordering::Relaxed);
                if self.strict {
                    // Classify in priority order. The first three arms
                    // mirror the three coarse checks; if validation fails
                    // for an IRIG-range or look-ahead reason, fall through
                    // to PayloadError with descriptive detail.
                    let err = if !message_type_is_valid(tw.message_type) {
                        MieError::UnknownTypeWord {
                            offset: self.offset as u64,
                            raw_type_word: type_raw,
                            message_type: tw.message_type,
                        }
                    } else if tw.word_count < min_wc {
                        MieError::InvalidTypeWord {
                            offset: self.offset as u64,
                            raw_type_word: type_raw,
                            word_count: tw.word_count,
                        }
                    } else if self.offset + record_bytes > self.file_len {
                        MieError::RecordTruncated {
                            offset: self.offset as u64,
                            record_bytes: record_bytes as u64,
                            available_bytes: (self.file_len - self.offset) as u64,
                        }
                    } else {
                        MieError::PayloadError {
                            offset: self.offset as u64,
                            detail: format!(
                                "record fails IRIG-range or look-ahead validation \
                                 (raw_type=0x{:04X})",
                                type_raw
                            ),
                        }
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
                    self.resolved_format,
                    MAX_SCAN_BYTES,
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
                        // - Truncation → L1-008 / L2-RDR-002: lenient
                        //   mode stops cleanly with no error.
                        // - Corruption → L1-023: surface as terminal
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
    /// - `Timestamp::to_microseconds()` returns `None` (Standard, uncalibrated)
    ///   → return `None` and skip tracker update (nothing to compare against).
    /// - First occurrence of `key` → return `Some(0.0)`, record current us.
    /// - Subsequent with non-negative gap → return `Some(seconds)`, record current us.
    /// - Subsequent with negative gap (non-monotonic) → return `None`, record
    ///   current us, emit a WARN once per key per recording.
    fn delta_for(&mut self, key: u32, timestamp: &Timestamp) -> Option<f64> {
        let curr_us = timestamp.to_microseconds()?;
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

    #[test]
    fn rejects_missing_file() {
        match MieFileReader::new("/no/such/path/12345.mie") {
            Err(e) => assert_eq!(e.kind(), crate::error::MieErrorKind::FileNotFound),
            Ok(_) => panic!("expected FileNotFound"),
        }
    }

    #[test]
    fn rejects_empty_file() {
        let f = write_temp(&[]);
        match MieFileReader::new(f.path()) {
            Err(e) => assert_eq!(e.kind(), crate::error::MieErrorKind::FileEmpty),
            Ok(_) => panic!("expected FileEmpty"),
        }
    }

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
}
