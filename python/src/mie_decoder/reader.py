"""Sequential reader for DDC MIE binary recording files.

This module provides :class:`MieFileReader`, an iterator that reads an
MIE binary file and yields fully decoded :class:`~mie_decoder.models.MieMessage`
instances in file order. It handles:

- **Header detection**: Automatically finds the first valid record,
  skipping any proprietary file headers.
- **Continuous sync validation**: Each record boundary is verified
  before decoding using a configurable N-record look-ahead (default 2).
- **Sync recovery**: If a record fails validation mid-file, scans
  forward in word-aligned steps to find the next valid record.
- **Timestamp format detection**: Auto-detects IRIG vs Standard.
- **All 10 MIL-STD-1553 message formats** plus SPURIOUS_DATA.
- **Error record handling**: Truncated payloads with Error Words.
- **SPURIOUS_DATA continuation detection**.
- **Per-RT/MSG DELTA** calculation.
- **Memory-efficient mmap I/O**.

Sync Recovery:

    The reader maintains sync through a validate-then-decode approach.
    At each record boundary, :func:`~mie_decoder.sync.validate_record`
    confirms the Type Word is valid, the word count is plausible, and
    the next record's Type Word also looks valid (look-ahead).

    If validation fails, the reader calls
    :func:`~mie_decoder.sync.recover_sync` which scans forward in
    2-byte steps until it finds a valid record. If recovery fails
    (no valid record within the scan window), iteration stops.

    Error records (bit 14 set) and their SPURIOUS_DATA continuations
    are valid records that pass sync validation normally. Sync loss
    only occurs when the DDC card writes truly corrupt data (e.g.,
    truncated mid-word, power loss during recording).
"""

from __future__ import annotations

import logging
import mmap
from pathlib import Path
from typing import Iterator

from mie_decoder.decode import (
    MIN_RECORD_BYTES,
    MIN_RECORD_BYTES_STANDARD,
    MIN_RECORD_WORDS,
    DEFAULT_DETECT_RECORDS,
    MIN_RECORD_WORDS_STANDARD,
    DetectionConfidence,
    classify_message_format,
    decode_command_word,
    decode_irig_timestamp,
    decode_standard_timestamp,
    decode_type_word,
    detect_record_anomalies,
    probe_timestamp_format,
    read_u16,
    read_u16_array,
    validate_post_extract_invariants,
    validate_structural_invariants,
)
from mie_decoder.exceptions import (
    MieFileEmptyError,
    MieFileNotFoundError,
    MieFirstRecordTruncatedError,
    MieHomogeneousPayloadError,
    MieInvalidTypeWordError,
    MieNoValidRecordsError,
    MiePayloadError,
    MieRecordTruncatedError,
    MieTimestampFormatMismatchError,
    MieUnknownErrorCodeError,
    MieUnknownTypeWordError,
    MieUnrecoverableSyncLossError,
)
from mie_decoder.models import (
    ALL_KNOWN_ERROR_CODES,
    CommandWord,
    Direction,
    ERROR_SPURIOUS_CONTINUATION,
    ERROR_SPURIOUS_STANDALONE,
    IrigTimestamp,
    KNOWN_DDC_ERROR_CODES,
    MessageFormat,
    MessageType,
    MieMessage,
    Timestamp,
    TimestampFormat,
    TIMESTAMP_WORD_COUNTS,
    TypeWord,
)
from mie_decoder.sync import (
    DEFAULT_LOOKAHEAD_RECORDS,
    ValidationFailure,
    find_first_record,
    recover_sync,
    validate_record_detailed,
)

logger = logging.getLogger(__name__)


class MieFileReader:
    """Memory-mapped sequential reader for MIE binary files.

    Reads a DDC MIE binary recording file and yields decoded
    :class:`~mie_decoder.models.MieMessage` instances. Uses
    ``mmap`` for efficient access to large files without loading
    the entire file into memory.

    The reader automatically handles:

    - **File headers**: Scans from offset 0 to find the first valid
      record. Files that start directly with records (offset 0) and
      files with proprietary headers (e.g., embedded equipment names)
      are both supported transparently.

    - **Sync loss and recovery**: If the reader encounters an invalid
      record mid-file (corruption, unexpected padding, partial writes),
      it scans forward in 2-byte steps to find the next valid record.
      In strict mode, sync loss raises an exception instead.

    - **Error records**: Records with bit 14 set contain a truncated
      payload plus an appended Error Word. These are valid records
      that maintain sync — the word count correctly describes the
      record length including the Error Word.

    - **SPURIOUS_DATA continuations**: After an error record, the
      remaining words from the interrupted transaction may appear as
      a SPURIOUS_DATA (0x20) record. The reader tracks the error→
      spurious linkage and assigns appropriate custom error codes
      (0x2000 for continuation, 0x2001 for standalone).

    Args:
        path: Path to the MIE binary file.
        strict: If ``True``, raise exceptions on sync loss, invalid
            records, and unknown error codes instead of recovering.
        time_format: Timestamp format. ``AUTO`` (default) detects from
            the first record.

    Raises:
        MieFileNotFoundError: If the file does not exist.
        MieFileEmptyError: If the file is zero bytes.
    """

    def __init__(
        self,
        path: str | Path,
        *,
        strict: bool = False,
        time_format: TimestampFormat = TimestampFormat.AUTO,
        detect_records: int = DEFAULT_DETECT_RECORDS,
        lookahead_records: int = DEFAULT_LOOKAHEAD_RECORDS,
        standard_tick_rate_hz: float | None = None,
    ) -> None:
        self._path = Path(path)
        self._strict = strict
        self._time_format = time_format
        # L2-DEC-015: probe size for auto-detection. Clamped to >= 1
        # (a no-probe call is nonsensical). The CLI / config layer
        # validates the upper bound; we mirror the clamp here so a
        # library caller can't break the invariant by passing 0.
        self._detect_records = max(1, detect_records)
        # L2-SYN-026: validate_record look-ahead depth. Clamped to
        # >= 1 with the same library-caller-friendly invariant as
        # detect_records.
        self._lookahead_records = max(1, lookahead_records)
        # L2-DEC-017: optional Standard-counter tick rate in Hz. None
        # (the default) keeps the historical empty-DELTA behavior for
        # Standard records; a finite, strictly-positive value enables
        # tick->microsecond conversion and DELTA participation. The CLI /
        # config layer validates the value; passed through to
        # _compute_delta.
        self._standard_tick_rate_hz = standard_tick_rate_hz
        # Cumulative sync-recovery count from the most recent __iter__
        # call. Reset to 0 on each iteration so the CLI can read it
        # after the loop completes (for L1-EXIT-003 / L1-EXIT-005 exit-class
        # summary). Mirrors `MieFileReader::sync_losses` in Rust.
        self._sync_losses: int = 0
        if not self._path.exists():
            raise MieFileNotFoundError(str(self._path))
        self._file_size = self._path.stat().st_size
        if self._file_size == 0:
            raise MieFileEmptyError(str(self._path))
        logger.debug(
            "Initialized reader for %s (%d bytes, strict=%s, "
            "time_format=%s, detect_records=%d)",
            self._path, self._file_size, self._strict,
            self._time_format.name, self._detect_records,
        )

    @property
    def path(self) -> Path:
        """Path to the source MIE binary file."""
        return self._path

    @property
    def file_size(self) -> int:
        """Size of the source file in bytes."""
        return self._file_size

    @property
    def sync_losses(self) -> int:
        """Cumulative sync-recovery count from the most recent
        ``__iter__`` call. Reset to 0 each iteration.

        Used by the CLI's L1-EXIT-005 exit-class summary to distinguish
        Complete (sync_losses == 0) from PartialRecovered (sync_losses
        > 0 with a successful full decode).
        """
        return self._sync_losses

    def __iter__(self) -> Iterator[MieMessage]:
        """Iterate over all decoded messages in file order.

        The iterator performs the following steps for each record:

        1. **Validate** the current offset with :func:`validate_record`.
        2. If invalid, **recover sync** by scanning forward.
        3. **Decode** the Type Word, timestamp, command word(s),
           status word(s), data words, and error word (if applicable).
        4. **Yield** the decoded :class:`MieMessage`.
        5. **Advance** to the next record boundary.

        Yields:
            Decoded MieMessage instances, one per binary record.
        """
        delta_tracker: dict[str, int] = {}
        warned_ooo_keys: set[str] = set()
        # PRA-9: one-time IRIG day-of-year discrepancy advisory per decode.
        warned_irig_day = False
        msg_count = 0
        sync_losses = 0
        # Reset the externally-visible counter at the start of each
        # iteration so successive __iter__ calls on the same reader
        # handle don't accumulate stale counts.
        self._sync_losses = 0
        prev_was_error = False
        resolved_format: TimestampFormat | None = None

        if self._time_format != TimestampFormat.AUTO:
            resolved_format = self._time_format

        logger.info("Beginning decode of %s", self._path.name)

        with open(self._path, "rb") as fh:
            fd = fh.fileno()
            with mmap.mmap(fd, 0, access=mmap.ACCESS_READ) as mm:
                file_len = len(mm)

                # ── Find first record (header detection) ───────
                start_offset = find_first_record(
                    mm, file_len,
                    ts_format=resolved_format,
                    lookahead_records=self._lookahead_records,
                )
                if start_offset is None:
                    # L2-RDR-004: distinguish "no Type Word at all" from
                    # "found a structurally-valid Type Word but its
                    # declared extent runs past EOF". The latter gets
                    # MieFirstRecordTruncatedError in strict mode and
                    # terminates cleanly with zero records in lenient.
                    from mie_decoder.sync import (
                        MAX_SCAN_BYTES,
                        diagnose_header_scan_failure,
                    )
                    truncated = diagnose_header_scan_failure(
                        mm, file_len, ts_format=resolved_format,
                    )
                    if truncated is not None:
                        offset_t, record_bytes, available = truncated
                        if self._strict:
                            logger.error(
                                "First record after header detection is "
                                "truncated at 0x%X: declared %d bytes, "
                                "only %d available",
                                offset_t, record_bytes, available,
                            )
                            raise MieFirstRecordTruncatedError(
                                offset_t, record_bytes, available
                            )
                        logger.warning(
                            "First record after header detection is "
                            "truncated at 0x%X: declared %d bytes, "
                            "only %d available — lenient mode terminates "
                            "cleanly with zero records",
                            offset_t, record_bytes, available,
                        )
                        return

                    # L1-EXIT-002: surface as an exception so the CLI maps
                    # to exit 2 (and so library callers can react)
                    # rather than silently yielding zero messages.
                    scan_bytes = min(file_len, MAX_SCAN_BYTES)
                    logger.error(
                        "No valid records found in %s", self._path.name
                    )
                    raise MieNoValidRecordsError(str(self._path), scan_bytes)

                # L2-SYN-018: reject pathological homogeneous-payload
                # inputs (e.g. 0x20-padded files where every "record"
                # parses as a synthetic SPURIOUS_DATA frame). The
                # candidate's Type Word tells us the record size; we
                # compare the next N candidate-sized chunks for byte
                # identity in non-timestamp positions.
                from mie_decoder.sync import (
                    HOMOGENEITY_SAMPLE_RECORDS,
                    is_homogeneous_payload,
                )
                candidate_tw = decode_type_word(read_u16(mm, start_offset))
                candidate_record_bytes = candidate_tw.word_count * 2
                if is_homogeneous_payload(
                    mm, start_offset, candidate_record_bytes
                ):
                    logger.error(
                        "Pathological homogeneous-payload input at "
                        "offset 0x%X in %s: %d consecutive candidate "
                        "records are byte-identical",
                        start_offset, self._path.name,
                        HOMOGENEITY_SAMPLE_RECORDS,
                    )
                    raise MieHomogeneousPayloadError(
                        str(self._path),
                        start_offset,
                        HOMOGENEITY_SAMPLE_RECORDS,
                    )

                # L2-DEC-015: multi-record probe to disambiguate IRIG
                # vs Standard BEFORE iteration begins. The chosen
                # format is final per L2-DEC-011 — no per-record
                # re-detection. Skipped when time_format is explicit.
                if resolved_format is None:
                    outcome = probe_timestamp_format(
                        mm, start_offset, self._detect_records,
                    )
                    resolved_format = outcome.format
                    if outcome.confidence == DetectionConfidence.DECISIVE:
                        logger.info(
                            "Auto-detected timestamp format: %s "
                            "(Decisive: IRIG=%d STD=%d over %d record(s))",
                            outcome.format.name,
                            outcome.irig_score,
                            outcome.std_score,
                            outcome.records_probed,
                        )
                    elif outcome.confidence == DetectionConfidence.MARGINAL:
                        logger.info(
                            "Auto-detected timestamp format: %s "
                            "(Marginal: IRIG=%d STD=%d over %d "
                            "record(s)) — pass --time-format to force "
                            "the choice if this is wrong",
                            outcome.format.name,
                            outcome.irig_score,
                            outcome.std_score,
                            outcome.records_probed,
                        )
                    else:
                        # L2-DEC-016 ambiguous case. Strict mode
                        # rejects; lenient logs WARN and uses the
                        # chosen format anyway (back-compat for
                        # borderline files that decoded acceptably
                        # under the old single-record detector).
                        if self._strict:
                            logger.error(
                                "Timestamp-format auto-detection is "
                                "ambiguous in %s starting at offset "
                                "0x%X: IRIG=%d STD=%d over %d record(s) — "
                                "strict mode rejects ambiguous files; "
                                "pass --time-format to force the choice",
                                self._path.name,
                                start_offset,
                                outcome.irig_score,
                                outcome.std_score,
                                outcome.records_probed,
                            )
                            raise MieTimestampFormatMismatchError(
                                start_offset,
                                outcome.irig_score,
                                outcome.std_score,
                                outcome.records_probed,
                            )
                        logger.warning(
                            "Auto-detected timestamp format: %s "
                            "(Ambiguous: IRIG=%d STD=%d over %d "
                            "record(s)) — using best guess; pass "
                            "--time-format to force the choice or "
                            "--strict to reject ambiguous files",
                            outcome.format.name,
                            outcome.irig_score,
                            outcome.std_score,
                            outcome.records_probed,
                        )
                else:
                    # L2-DEC-013: the format was forced via --time-format /
                    # decode.time_format. Sanity-check it against the same
                    # detection probe: if the probe is *Decisive* about the
                    # OTHER format, the forced selection is obviously wrong
                    # (e.g. --time-format standard on an IRIG file), which
                    # would otherwise emit garbage timestamps for the whole
                    # file. Marginal/Ambiguous probes are NOT flagged — those
                    # are exactly the cases where forcing is the legitimate
                    # override. resolved_format stays the forced format.
                    outcome = probe_timestamp_format(
                        mm, start_offset, self._detect_records,
                    )
                    if (
                        outcome.confidence == DetectionConfidence.DECISIVE
                        and outcome.format != resolved_format
                    ):
                        if self._strict:
                            logger.error(
                                "Forced timestamp format %s contradicts the "
                                "recording in %s at offset 0x%X: detection is "
                                "decisive for %s (IRIG=%d STD=%d over %d "
                                "record(s)) — strict mode rejects the mismatch; "
                                "drop --time-format to auto-detect",
                                resolved_format.name,
                                self._path.name,
                                start_offset,
                                outcome.format.name,
                                outcome.irig_score,
                                outcome.std_score,
                                outcome.records_probed,
                            )
                            raise MieTimestampFormatMismatchError(
                                start_offset,
                                outcome.irig_score,
                                outcome.std_score,
                                outcome.records_probed,
                            )
                        logger.warning(
                            "Forced timestamp format %s contradicts the "
                            "recording at offset 0x%X: detection is decisive "
                            "for %s (IRIG=%d STD=%d over %d record(s)) — "
                            "decoding with the forced format anyway; drop "
                            "--time-format to auto-detect or pass --strict to "
                            "reject the mismatch",
                            resolved_format.name,
                            start_offset,
                            outcome.format.name,
                            outcome.irig_score,
                            outcome.std_score,
                            outcome.records_probed,
                        )

                offset = start_offset
                loop_min = MIN_RECORD_BYTES_STANDARD

                while offset + loop_min <= file_len:
                    # ── Read and validate Type Word ────────────
                    type_raw = read_u16(mm, offset)
                    tw = decode_type_word(type_raw)

                    ts_words = TIMESTAMP_WORD_COUNTS[resolved_format]
                    record_bytes = tw.word_count * 2

                    # ── Validate current record ────────────────
                    validation_failure = validate_record_detailed(
                        mm,
                        offset,
                        file_len,
                        ts_format=resolved_format,
                        lookahead_records=self._lookahead_records,
                    )

                    if validation_failure is not None:
                        # ── Sync loss — attempt recovery ───────
                        sync_losses += 1
                        self._sync_losses = sync_losses
                        _log_validation_context(mm, offset)

                        if self._strict:
                            if validation_failure == ValidationFailure.INVALID_WORD_COUNT:
                                raise MieInvalidTypeWordError(
                                    offset, type_raw, tw.word_count
                                )
                            if validation_failure == ValidationFailure.RECORD_TRUNCATED:
                                raise MieRecordTruncatedError(
                                    offset, record_bytes, file_len - offset
                                )
                            if validation_failure == ValidationFailure.UNKNOWN_MESSAGE_TYPE:
                                raise MieUnknownTypeWordError(
                                    offset, type_raw, tw.message_type
                                )
                            raise MiePayloadError(
                                offset,
                                f"{validation_failure} (raw_type=0x{type_raw:04X})",
                            )

                        recovered = recover_sync(
                            mm, offset, file_len,
                            ts_format=resolved_format,
                            lookahead_records=self._lookahead_records,
                        )
                        if recovered is None:
                            # Distinguish truncation (file ended before
                            # the 64 KB scan window exhausted) from
                            # genuine mid-file corruption.
                            #
                            # - Truncation → L1-DEC-005 / L2-RDR-002:
                            #   lenient mode stops cleanly.
                            # - Corruption → L1-EXIT-004: raise so the CLI
                            #   maps to exit 3 (or `.partial` + exit 0
                            #   with --allow-partial).
                            from mie_decoder.sync import MAX_SCAN_BYTES
                            bytes_remaining = file_len - offset
                            if bytes_remaining < MAX_SCAN_BYTES:
                                logger.info(
                                    "Lenient mode: scan exhausted at EOF "
                                    "(offset 0x%X, %d bytes remain < %d "
                                    "scan window); treating as truncation",
                                    offset, bytes_remaining, MAX_SCAN_BYTES,
                                )
                                break
                            logger.error(
                                "Unrecoverable sync loss at 0x%X after %d messages",
                                offset, msg_count,
                            )
                            raise MieUnrecoverableSyncLossError(offset, sync_losses)

                        offset = recovered
                        prev_was_error = False
                        continue

                    # ── Decode timestamp ───────────────────────
                    timestamp: Timestamp
                    if resolved_format == TimestampFormat.IRIG:
                        timestamp = decode_irig_timestamp(
                            read_u16(mm, offset + 2),
                            read_u16(mm, offset + 4),
                            read_u16(mm, offset + 6),
                        )
                        if isinstance(timestamp, IrigTimestamp) and timestamp.freerun:
                            logger.warning(
                                "Freerun timestamp at 0x%X", offset
                            )
                        elif (
                            isinstance(timestamp, IrigTimestamp)
                            and not warned_irig_day
                        ):
                            # PRA-9: the IRIG day-of-year field has a known
                            # firmware-dependent decode discrepancy on some
                            # DDC cards; time-of-day fields are unaffected.
                            # Emit a one-time advisory (not a decode failure).
                            warned_irig_day = True
                            logger.warning(
                                "IRIG day-of-year decoded for this recording; the "
                                "day-of-year field has a known firmware-dependent "
                                "discrepancy on some DDC cards (hour/minute/second/"
                                "microsecond are unaffected) — see "
                                "docs/VENDOR-CSV-DIFFS.md §5"
                            )
                    else:
                        timestamp = decode_standard_timestamp(
                            read_u16(mm, offset + 2),
                            read_u16(mm, offset + 4),
                        )

                    cmd_byte_offset = offset + 2 + ts_words * 2

                    # ── SPURIOUS_DATA: no Command Word ────────
                    if tw.message_type == MessageType.SPURIOUS_DATA:
                        raw_word_count = tw.word_count - 1 - ts_words
                        data_words: tuple[int, ...] = ()
                        if raw_word_count > 0:
                            data_words = read_u16_array(
                                mm, cmd_byte_offset, raw_word_count
                            )

                        error_code = (
                            ERROR_SPURIOUS_CONTINUATION
                            if prev_was_error
                            else ERROR_SPURIOUS_STANDALONE
                        )

                        logger.debug(
                            "SPURIOUS_DATA at 0x%X: %d raw words, %s",
                            offset, raw_word_count,
                            "continuation" if prev_was_error else "standalone",
                        )

                        # SPURIOUS_DATA has no RT/MSG key. DELTA is None
                        # so the CSV writer emits an empty cell.
                        yield MieMessage(
                            timestamp=timestamp,
                            type_word=tw,
                            message_format=MessageFormat.SPURIOUS_DATA,
                            command_word=None,
                            command_word_2=None,
                            status_word=None,
                            status_word_2=None,
                            data_words=data_words,
                            error_word=error_code,
                            delta=None,
                            file_offset=offset,
                        )

                        msg_count += 1
                        offset += record_bytes
                        prev_was_error = False
                        continue

                    # ── Decode Command Word ────────────────────
                    cmd_raw = read_u16(mm, cmd_byte_offset)
                    cmd = decode_command_word(cmd_raw)

                    # ── Errored record (bit 14 set) ───────────
                    if tw.error:
                        msg = _decode_error_record(
                            mm, offset, tw, timestamp, cmd,
                            cmd_byte_offset, ts_words, self._strict,
                        )
                        # Error records participate in DELTA tracking under
                        # the shared contract: the diagnostic value of
                        # knowing inter-arrival gaps to a flaky RT/MSG is
                        # higher than the cost of including anomaly rows.
                        delta = _compute_delta(
                            delta_tracker, warned_ooo_keys,
                            msg.delta_key, timestamp, offset,
                            self._standard_tick_rate_hz,
                        )
                        msg = MieMessage(
                            timestamp=msg.timestamp,
                            type_word=msg.type_word,
                            message_format=msg.message_format,
                            command_word=msg.command_word,
                            command_word_2=msg.command_word_2,
                            status_word=msg.status_word,
                            status_word_2=msg.status_word_2,
                            data_words=msg.data_words,
                            error_word=msg.error_word,
                            delta=delta,
                            file_offset=msg.file_offset,
                        )

                        yield msg
                        msg_count += 1
                        offset += record_bytes
                        prev_was_error = True
                        continue

                    # ── Normal record ──────────────────────────
                    try:
                        msg_fmt = classify_message_format(
                            tw.message_type, cmd, tw.word_count, ts_words
                        )
                    except ValueError as exc:
                        logger.warning(
                            "Cannot classify at 0x%X: %s — skipping",
                            offset, exc,
                        )
                        offset += record_bytes
                        prev_was_error = False
                        continue

                    # L2-SYN-020..025: structural invariants. Strict mode
                    # aborts via MiePayloadError; lenient mode WARNs and
                    # skips the record.
                    inv = validate_structural_invariants(
                        tw, cmd, msg_fmt, ts_words,
                    )
                    if inv is not None:
                        if self._strict:
                            raise MiePayloadError(
                                offset,
                                f"L2-SYN structural invariant violation: {inv.detail}",
                            )
                        logger.warning(
                            "L2-SYN structural invariant violation at 0x%X: %s; skipping record",
                            offset, inv.detail,
                        )
                        offset += record_bytes
                        prev_was_error = False
                        continue

                    logger.debug(
                        "Record at 0x%X: type=0x%02X fmt=%s RT%d SA%d %s",
                        offset, tw.message_type, msg_fmt.name,
                        cmd.rt, cmd.subaddress, cmd.direction.name,
                    )

                    # Bound payload reads to this record's byte range so a
                    # Command Word that *claims* a larger data_word_count than
                    # the record can hold cannot read into the following record
                    # or past EOF. The Type Word's word_count defines the record
                    # length (already validated to fit the file); over-claims
                    # yield empty/partial data instead of raising. Mirrors the
                    # Rust reader's record-bounded extract_payload.
                    record_end = offset + record_bytes
                    payload_byte_offset = cmd_byte_offset + 2
                    cmd2, status, status2, data_words = _extract_payload(
                        mm, payload_byte_offset, record_end, msg_fmt, cmd,
                    )

                    # L2-SYN-023 / L2-SYN-027: post-extract Reject-class checks.
                    # Same strict/lenient policy as the pre-extract checks.
                    post_inv = validate_post_extract_invariants(msg_fmt, cmd, cmd2)
                    if post_inv is not None:
                        if self._strict:
                            raise MiePayloadError(
                                offset,
                                f"L2-SYN structural invariant violation: {post_inv.detail}",
                            )
                        logger.warning(
                            "L2-SYN structural invariant violation at 0x%X: %s; skipping record",
                            offset, post_inv.detail,
                        )
                        offset += record_bytes
                        prev_was_error = False
                        continue

                    # L2-SYN-024 / L2-SYN-025: AnomalyWarn-class.
                    # Both modes log a WARN and continue emitting.
                    for anomaly in detect_record_anomalies(tw, cmd, status):
                        logger.warning(
                            "L2-SYN anomaly at 0x%X: %s",
                            offset, anomaly.detail,
                        )

                    direction_char = (
                        "T" if cmd.direction == Direction.TRANSMIT else "R"
                    )
                    delta_key = f"{cmd.rt}:{cmd.subaddress}{direction_char}"
                    delta = _compute_delta(
                        delta_tracker, warned_ooo_keys,
                        delta_key, timestamp, offset,
                        self._standard_tick_rate_hz,
                    )

                    yield MieMessage(
                        timestamp=timestamp,
                        type_word=tw,
                        message_format=msg_fmt,
                        command_word=cmd,
                        command_word_2=cmd2,
                        status_word=status,
                        status_word_2=status2,
                        data_words=data_words,
                        error_word=None,
                        delta=delta,
                        file_offset=offset,
                    )

                    msg_count += 1
                    offset += record_bytes
                    prev_was_error = False

                    if msg_count % 100_000 == 0:
                        logger.info(
                            "Decoded %d messages (0x%X / 0x%X)",
                            msg_count, offset, file_len,
                        )

        logger.info(
            "Decode complete: %d messages, %d sync recoveries, "
            "format=%s, file=%s",
            msg_count, sync_losses,
            resolved_format.name if resolved_format else "unknown",
            self._path.name,
        )


def _decode_error_record(
    mm: mmap.mmap,
    offset: int,
    tw: "TypeWord",
    timestamp: "Timestamp",
    cmd: "CommandWord",
    cmd_byte_offset: int,
    ts_words: int,
    strict: bool,
) -> MieMessage:
    """Decode a record with the error flag (bit 14) set.

    Error Word is the last word of the record. Payload between
    Command Word and Error Word = truncated data words.
    """
    error_word_offset = offset + (tw.word_count - 1) * 2
    error_code = read_u16(mm, error_word_offset)

    if error_code not in KNOWN_DDC_ERROR_CODES:
        if strict:
            raise MieUnknownErrorCodeError(offset, error_code)
        logger.warning(
            "Unknown DDC error code 0x%04X at offset 0x%X",
            error_code, offset,
        )

    payload_words = tw.word_count - 1 - ts_words - 1 - 1
    data_words: tuple[int, ...] = ()
    if payload_words > 0:
        data_words = read_u16_array(mm, cmd_byte_offset + 2, payload_words)

    try:
        msg_fmt = classify_message_format(
            tw.message_type, cmd, tw.word_count, ts_words
        )
    except ValueError:
        msg_fmt = MessageFormat.RECEIVE

    from mie_decoder.models import DDC_ERROR_DESCRIPTIONS
    desc = DDC_ERROR_DESCRIPTIONS.get(error_code, "Unknown")
    logger.info(
        "Error record at 0x%X: RT%d SA%d %s, code=0x%04X (%s), "
        "%d payload words",
        offset, cmd.rt, cmd.subaddress, cmd.direction.name,
        error_code, desc, payload_words,
    )

    return MieMessage(
        timestamp=timestamp,
        type_word=tw,
        message_format=msg_fmt,
        command_word=cmd,
        command_word_2=None,
        status_word=None,
        status_word_2=None,
        data_words=data_words,
        error_word=error_code,
        delta=None,
        file_offset=offset,
    )


def _compute_delta(
    delta_tracker: dict[str, int],
    warned_ooo_keys: set[str],
    key: str,
    timestamp: Timestamp,
    offset: int,
    standard_tick_rate_hz: float | None = None,
) -> float | None:
    """Compute DELTA for ``key`` per the shared contract and update tracker.

    - ``timestamp.to_microseconds()`` returns ``None`` (Standard with no
      configured tick rate) → return ``None`` and skip tracker update.
    - SPURIOUS_DATA passes an empty key → return ``None``.
    - First occurrence of ``key`` → return ``0.0``, record current us.
    - Subsequent with non-negative gap → return ``seconds``, record current us.
    - Subsequent with negative gap (non-monotonic) → return ``None``, record
      current us, emit a WARN once per ``key`` per recording.
    """
    if not key:
        return None
    curr_us = timestamp.to_microseconds(standard_tick_rate_hz)
    if curr_us is None:
        return None
    prev = delta_tracker.get(key)
    delta_tracker[key] = curr_us
    if prev is None:
        return 0.0
    if curr_us < prev:
        if key not in warned_ooo_keys:
            warned_ooo_keys.add(key)
            logger.warning(
                "Non-monotonic timestamp at 0x%X for RT/MSG %s: "
                "prev_us=%d curr_us=%d (further out-of-order occurrences "
                "for this key suppressed)",
                offset, key, prev, curr_us,
            )
        return None
    return (curr_us - prev) / 1_000_000.0


def _extract_payload(
    mm: mmap.mmap,
    p: int,
    record_end: int,
    fmt: MessageFormat,
    cmd: CommandWord,
) -> tuple[CommandWord | None, int | None, int | None, tuple[int, ...]]:
    """Extract command words, status words, and data words per format.

    All reads are bounded to ``record_end`` (the byte just past this
    record, ``offset + word_count * 2``, already validated to fit the
    file). RT-to-RT formats take their data-word count from Cmd2, which
    the L2-SYN-022 capacity check — computed from Cmd1 — does not bound;
    a Cmd2 that over-claims must yield empty/partial data rather than
    reading into the following record or raising ``struct.error``. The
    local ``_r16``/``_read_n`` helpers return ``None``/``()`` on an
    out-of-bounds read, mirroring the Rust reader's record-bounded
    ``extract_payload`` (``read_u16`` → ``Option``, ``read_n`` → empty).
    """

    def _r16(off: int) -> int | None:
        if off + 2 > record_end:
            return None
        return read_u16(mm, off)

    def _read_n(start: int, n: int) -> tuple[int, ...]:
        if start + n * 2 > record_end:
            return ()
        return read_u16_array(mm, start, n)

    if fmt == MessageFormat.RECEIVE:
        n = cmd.data_word_count
        data_words = _read_n(p, n)
        status = _r16(p + n * 2)
        return None, status, None, data_words

    if fmt == MessageFormat.TRANSMIT:
        status = _r16(p)
        n = cmd.data_word_count
        data_words = _read_n(p + 2, n)
        return None, status, None, data_words

    if fmt == MessageFormat.RT_TO_RT:
        cmd2 = decode_command_word(_r16(p) or 0)
        tx_status = _r16(p + 2)
        n = cmd2.data_word_count
        data_words = _read_n(p + 4, n)
        rx_status = _r16(p + 4 + n * 2)
        return cmd2, tx_status, rx_status, data_words

    if fmt == MessageFormat.RECEIVE_BROADCAST:
        n = cmd.data_word_count
        data_words = _read_n(p, n)
        return None, None, None, data_words

    if fmt == MessageFormat.RT_TO_RT_BROADCAST:
        cmd2 = decode_command_word(_r16(p) or 0)
        tx_status = _r16(p + 2)
        n = cmd2.data_word_count
        data_words = _read_n(p + 4, n)
        return cmd2, tx_status, None, data_words

    if fmt == MessageFormat.MODE_CODE_TX_DATA:
        status = _r16(p)
        w = _r16(p + 2)
        return None, status, None, () if w is None else (w,)

    if fmt == MessageFormat.MODE_CODE_RX_DATA:
        w = _r16(p)
        status = _r16(p + 2)
        return None, status, None, () if w is None else (w,)

    if fmt == MessageFormat.MODE_CODE_NO_DATA:
        status = _r16(p)
        return None, status, None, ()

    if fmt == MessageFormat.MODE_CODE_BCAST_NO_DATA:
        return None, None, None, ()

    if fmt == MessageFormat.MODE_CODE_BCAST_DATA:
        w = _r16(p)
        return None, None, None, () if w is None else (w,)

    raise ValueError(f"Unhandled message format: {fmt}")


def _log_validation_context(mm: mmap.mmap, offset: int) -> None:
    """Emit at most 32 bytes around a validation failure at DEBUG."""
    if not logger.isEnabledFor(logging.DEBUG):
        return
    ctx_start = max(0, offset - 16)
    ctx_end = min(len(mm), ctx_start + 32)
    hex_part = " ".join(f"{byte:02X}" for byte in mm[ctx_start:ctx_end])
    logger.debug(
        "validation context at 0x%X (bytes 0x%X..0x%X, max 32): %s",
        offset,
        ctx_start,
        ctx_end,
        hex_part,
    )
