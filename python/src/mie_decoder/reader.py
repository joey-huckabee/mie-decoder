"""Sequential reader for DDC MIE binary recording files.

This module provides :class:`MieFileReader`, an iterator that reads an
MIE binary file and yields fully decoded :class:`~mie_decoder.models.MieMessage`
instances in file order. It handles:

- **Header detection**: Automatically finds the first valid record,
  skipping any proprietary file headers.
- **Continuous sync validation**: Each record boundary is verified
  before decoding using a two-record look-ahead.
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
import sys
from pathlib import Path
from typing import Iterator

from mie_decoder.decode import (
    MIN_RECORD_BYTES,
    MIN_RECORD_BYTES_STANDARD,
    MIN_RECORD_WORDS,
    MIN_RECORD_WORDS_STANDARD,
    classify_message_format,
    decode_command_word,
    decode_irig_timestamp,
    decode_standard_timestamp,
    decode_type_word,
    detect_timestamp_format,
    is_valid_message_type,
    read_u16,
    read_u16_array,
)
from mie_decoder.exceptions import (
    MieFileEmptyError,
    MieFileNotFoundError,
    MieInvalidTypeWordError,
    MieNoValidRecordsError,
    MiePayloadError,
    MieRecordTruncatedError,
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
)
from mie_decoder.sync import (
    find_first_record,
    recover_sync,
    validate_record,
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
    ) -> None:
        self._path = Path(path)
        self._strict = strict
        self._time_format = time_format
        # Cumulative sync-recovery count from the most recent __iter__
        # call. Reset to 0 on each iteration so the CLI can read it
        # after the loop completes (for L1-022 / L1-024 exit-class
        # summary). Mirrors `MieFileReader::sync_losses` in Rust.
        self._sync_losses: int = 0
        if not self._path.exists():
            raise MieFileNotFoundError(str(self._path))
        self._file_size = self._path.stat().st_size
        if self._file_size == 0:
            raise MieFileEmptyError(str(self._path))
        logger.debug(
            "Initialized reader for %s (%d bytes, strict=%s, time_format=%s)",
            self._path, self._file_size, self._strict, self._time_format.name,
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

        Used by the CLI's L1-024 exit-class summary to distinguish
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
                )
                if start_offset is None:
                    # L1-021: surface as an exception so the CLI maps
                    # to exit 2 (and so library callers can react)
                    # rather than silently yielding zero messages.
                    from mie_decoder.sync import MAX_SCAN_BYTES
                    scan_bytes = min(file_len, MAX_SCAN_BYTES)
                    logger.error(
                        "No valid records found in %s", self._path.name
                    )
                    raise MieNoValidRecordsError(str(self._path), scan_bytes)

                offset = start_offset
                loop_min = MIN_RECORD_BYTES_STANDARD

                while offset + loop_min <= file_len:
                    # ── Read and validate Type Word ────────────
                    type_raw = read_u16(mm, offset)
                    tw = decode_type_word(type_raw)

                    # Auto-detect timestamp on first valid record
                    if resolved_format is None:
                        resolved_format = detect_timestamp_format(
                            mm, offset, tw
                        )
                        logger.info(
                            "Auto-detected timestamp format: %s",
                            resolved_format.name,
                        )

                    ts_words = TIMESTAMP_WORD_COUNTS[resolved_format]
                    min_wc = 1 + ts_words + 1
                    record_bytes = tw.word_count * 2

                    # ── Validate current record ────────────────
                    is_valid = validate_record(
                        mm,
                        offset,
                        file_len,
                        ts_format=resolved_format,
                    )

                    if not is_valid:
                        # ── Sync loss — attempt recovery ───────
                        sync_losses += 1
                        self._sync_losses = sync_losses

                        if self._strict:
                            if tw.word_count < min_wc:
                                raise MieInvalidTypeWordError(
                                    offset, type_raw, tw.word_count
                                )
                            if offset + record_bytes > file_len:
                                raise MieRecordTruncatedError(
                                    offset, record_bytes, file_len - offset
                                )
                            if not is_valid_message_type(tw.message_type):
                                _print_unknown_type_diagnostic(mm, offset, tw)
                                raise MieUnknownTypeWordError(
                                    offset, type_raw, tw.message_type
                                )
                            raise MiePayloadError(
                                offset,
                                "record fails IRIG-range or look-ahead validation "
                                f"(raw_type=0x{type_raw:04X})",
                            )

                        recovered = recover_sync(
                            mm, offset, file_len,
                            ts_format=resolved_format,
                        )
                        if recovered is None:
                            # Distinguish truncation (file ended before
                            # the 64 KB scan window exhausted) from
                            # genuine mid-file corruption.
                            #
                            # - Truncation → L1-008 / L2-RDR-002:
                            #   lenient mode stops cleanly.
                            # - Corruption → L1-023: raise so the CLI
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
                            tw.message_type, cmd, tw.word_count
                        )
                    except ValueError as exc:
                        logger.warning(
                            "Cannot classify at 0x%X: %s — skipping",
                            offset, exc,
                        )
                        offset += record_bytes
                        prev_was_error = False
                        continue

                    logger.debug(
                        "Record at 0x%X: type=0x%02X fmt=%s RT%d SA%d %s",
                        offset, tw.message_type, msg_fmt.name,
                        cmd.rt, cmd.subaddress, cmd.direction.name,
                    )

                    payload_byte_offset = cmd_byte_offset + 2
                    cmd2, status, status2, data_words = _extract_payload(
                        mm, payload_byte_offset, tw.word_count, msg_fmt, cmd,
                    )

                    direction_char = (
                        "T" if cmd.direction == Direction.TRANSMIT else "R"
                    )
                    delta_key = f"{cmd.rt}:{cmd.subaddress}{direction_char}"
                    delta = _compute_delta(
                        delta_tracker, warned_ooo_keys,
                        delta_key, timestamp, offset,
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
            tw.message_type, cmd, tw.word_count
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
) -> float | None:
    """Compute DELTA for ``key`` per the shared contract and update tracker.

    - ``timestamp.to_microseconds()`` returns ``None`` (Standard, uncalibrated)
      → return ``None`` and skip tracker update.
    - SPURIOUS_DATA passes an empty key → return ``None``.
    - First occurrence of ``key`` → return ``0.0``, record current us.
    - Subsequent with non-negative gap → return ``seconds``, record current us.
    - Subsequent with negative gap (non-monotonic) → return ``None``, record
      current us, emit a WARN once per ``key`` per recording.
    """
    if not key:
        return None
    curr_us = timestamp.to_microseconds()
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
    word_count: int,
    fmt: MessageFormat,
    cmd: CommandWord,
) -> tuple[CommandWord | None, int | None, int | None, tuple[int, ...]]:
    """Extract command words, status words, and data words per format."""
    if fmt == MessageFormat.RECEIVE:
        n = cmd.data_word_count
        data_words = read_u16_array(mm, p, n)
        status = read_u16(mm, p + n * 2)
        return None, status, None, data_words

    if fmt == MessageFormat.TRANSMIT:
        status = read_u16(mm, p)
        n = cmd.data_word_count
        data_words = read_u16_array(mm, p + 2, n)
        return None, status, None, data_words

    if fmt == MessageFormat.RT_TO_RT:
        cmd2_raw = read_u16(mm, p)
        cmd2 = decode_command_word(cmd2_raw)
        tx_status = read_u16(mm, p + 2)
        n = cmd2.data_word_count
        data_words = read_u16_array(mm, p + 4, n)
        rx_status = read_u16(mm, p + 4 + n * 2)
        return cmd2, tx_status, rx_status, data_words

    if fmt == MessageFormat.RECEIVE_BROADCAST:
        n = cmd.data_word_count
        data_words = read_u16_array(mm, p, n)
        return None, None, None, data_words

    if fmt == MessageFormat.RT_TO_RT_BROADCAST:
        cmd2_raw = read_u16(mm, p)
        cmd2 = decode_command_word(cmd2_raw)
        tx_status = read_u16(mm, p + 2)
        n = cmd2.data_word_count
        data_words = read_u16_array(mm, p + 4, n)
        return cmd2, tx_status, None, data_words

    if fmt == MessageFormat.MODE_CODE_TX_DATA:
        status = read_u16(mm, p)
        data_words = (read_u16(mm, p + 2),)
        return None, status, None, data_words

    if fmt == MessageFormat.MODE_CODE_RX_DATA:
        data_words = (read_u16(mm, p),)
        status = read_u16(mm, p + 2)
        return None, status, None, data_words

    if fmt == MessageFormat.MODE_CODE_NO_DATA:
        status = read_u16(mm, p)
        return None, status, None, ()

    if fmt == MessageFormat.MODE_CODE_BCAST_NO_DATA:
        return None, None, None, ()

    if fmt == MessageFormat.MODE_CODE_BCAST_DATA:
        data_words = (read_u16(mm, p),)
        return None, None, None, data_words

    raise ValueError(f"Unhandled message format: {fmt}")


def _print_unknown_type_diagnostic(
    mm: mmap.mmap,
    offset: int,
    tw: "TypeWord",
) -> None:
    """Print diagnostic information for an unknown Type Word to stderr."""
    file_len = len(mm)
    ctx_start = max(0, offset - 16)
    ctx_end = min(file_len, offset + tw.word_count * 2 + 16)
    ctx_bytes = bytes(mm[ctx_start:ctx_end])

    print(
        f"\n{'='*72}\n"
        f"UNKNOWN MESSAGE TYPE ENCOUNTERED\n"
        f"{'='*72}\n"
        f"  File offset:    0x{offset:08X} ({offset} bytes)\n"
        f"  Raw Type Word:  0x{tw.raw:04X}\n"
        f"  Message type:   0x{tw.message_type:02X} (bits 0-6)\n"
        f"  Bus:            {'B' if tw.bus else 'A'}\n"
        f"  Word count:     {tw.word_count} ({tw.word_count * 2} bytes)\n"
        f"  Error flag:     {tw.error}\n"
        f"\n"
        f"  Known types: 0x01=Mode, 0x02=BC→RT, 0x04=RT→BC, 0x08=RT→RT,\n"
        f"               0x10=Bcast BC→RT, 0x18=Bcast RT→RT, 0x20=Spurious\n"
        f"\n"
        f"  Context hex dump (offset 0x{ctx_start:08X}–0x{ctx_end:08X}):",
        file=sys.stderr,
    )

    for i in range(0, len(ctx_bytes), 16):
        addr = ctx_start + i
        hex_part = " ".join(f"{b:02X}" for b in ctx_bytes[i:i + 16])
        ascii_part = "".join(
            chr(b) if 32 <= b < 127 else "." for b in ctx_bytes[i:i + 16]
        )
        marker = "  >>>" if addr <= offset < addr + 16 else "     "
        print(
            f"{marker} {addr:08X}  {hex_part:<48s}  |{ascii_part}|",
            file=sys.stderr,
        )

    print(f"{'='*72}\n", file=sys.stderr)
