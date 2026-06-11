"""Record synchronization and alignment for MIE binary files.

This module provides functions for detecting valid record boundaries,
recovering from sync loss, and detecting file headers. It is the first
line of defense against corrupted data, unexpected file layouts, and
mid-file sync loss caused by error records or truncated writes.

Synchronization Strategy:

    1. **Initial Alignment (Header Detection):**
       Before decoding begins, the reader must find the first valid
       record. Some files start immediately with records (e.g., the
       ``aa`` file), while others have a proprietary header (e.g.,
       the ``s4`` file with embedded ASCII identifiers like equipment
       names). The :func:`find_first_record` function scans from offset
       0 looking for the first position that passes multi-point
       validation.

    2. **Continuous Validation:**
       At each record boundary during iteration, :func:`validate_record`
       confirms the current position is a valid record before committing
       to decode it. This catches corruption that occurs mid-file.

    3. **Sync Recovery (Walk Forward):**
       If a record fails validation, :func:`recover_sync` scans forward
       from the current position in 2-byte (word-aligned) increments
       looking for the next valid record. This handles:
       - Corrupted records in the middle of a file
       - Unexpected padding or filler bytes
       - Error record sequences that produced unusual layouts
       - Partial writes from recording termination

    4. **Look-Ahead Confirmation:**
       A single valid-looking Type Word is not sufficient — random data
       can coincidentally match. :func:`validate_record` uses a
       **two-record look-ahead**: a candidate is confirmed valid only
       if the NEXT record (at offset + word_count * 2) also starts
       with a valid Type Word. This dramatically reduces false positives.

Validation Heuristics (applied in order, fast checks first):

    1. Type Word message type (bits 0–6) must be in VALID_MESSAGE_TYPES.
    2. Word count (bits 8–13) must be >= minimum for the timestamp
       format (4 for Standard, 5 for IRIG) and <= 63 (6-bit field max).
    3. The record must not extend past the end of file.
    4. If IRIG timestamp: hour < 24, minute < 60, second < 60.
    5. Look-ahead: the next record's Type Word must also have a valid
       message type and plausible word count.

Performance Considerations:

    - All checks use O(1) bit operations on already-read 16-bit words.
    - No string allocations or complex parsing during scanning.
    - Look-ahead reads only 2 bytes (the next Type Word), not the full
      next record.
    - The scan in :func:`recover_sync` advances 2 bytes per step
      (word-aligned) and caps at a configurable maximum distance to
      prevent scanning entire multi-gigabyte files.
    - :func:`find_first_record` uses the same capped scan, defaulting
      to 4096 bytes — sufficient for any known DDC file header.

Error Records and Sync:

    Error records (Type Word bit 14 set) and their SPURIOUS_DATA
    continuations are valid records with valid Type Words. They pass
    sync validation normally. The sync machinery does not need special
    error-aware logic — the reader's error handling (in ``reader.py``)
    processes them after sync validation confirms the record boundary.

    The one exception is if an error causes the DDC card to write a
    corrupt record (e.g., truncated mid-word). In that case, the word
    count will point past valid data, and the look-ahead check on the
    NEXT record will fail, triggering sync recovery. This is the
    correct behavior: skip the corrupt record, find the next good one.
"""

from __future__ import annotations

import logging
from enum import Enum
from typing import Final

from mie_decoder.decode import (
    MIN_RECORD_WORDS,
    MIN_RECORD_WORDS_STANDARD,
    decode_irig_timestamp,
    decode_type_word,
    is_valid_message_type,
    read_u16,
)
from mie_decoder.models import TimestampFormat, TIMESTAMP_WORD_COUNTS

logger = logging.getLogger(__name__)

#: Maximum number of bytes to scan when searching for sync.
#: 64 KB covers any reasonable header or corruption gap.
MAX_SCAN_BYTES: Final[int] = 65_536

#: Maximum plausible record size in bytes. The word count field is 6 bits
#: (max 63), so the absolute maximum record is 63 × 2 = 126 bytes.
MAX_RECORD_BYTES: Final[int] = 126

#: L2-SYN-026 default look-ahead depth. Two-record look-ahead preserves
#: the historical default established by L2-SYN-005. Configurable via
#: ``decode.lookahead_records`` (TOML) or ``--lookahead-records`` (CLI),
#: range ``[1, 32]``.
DEFAULT_LOOKAHEAD_RECORDS: Final[int] = 2


class ValidationFailure(Enum):
    """Precise reason a candidate record failed sync validation."""

    TYPE_WORD_UNREADABLE = "Type Word is not readable"
    UNKNOWN_MESSAGE_TYPE = "message type is unknown"
    INVALID_WORD_COUNT = "word count is outside the valid range"
    RECORD_TRUNCATED = "record extends beyond end of file"
    IRIG_HOUR_OUT_OF_RANGE = "IRIG hour is out of range"
    IRIG_MINUTE_OUT_OF_RANGE = "IRIG minute is out of range"
    IRIG_SECOND_OUT_OF_RANGE = "IRIG second is out of range"
    IRIG_MICROSECOND_OUT_OF_RANGE = "IRIG microsecond is out of range"
    IRIG_DAY_OUT_OF_RANGE = "IRIG day-of-year is out of range"
    LOOKAHEAD_UNKNOWN_MESSAGE_TYPE = "look-ahead message type is unknown"
    LOOKAHEAD_INVALID_WORD_COUNT = "look-ahead word count is outside the valid range"

    def __str__(self) -> str:
        return self.value


def validate_record(
    data: bytes | memoryview,
    offset: int,
    file_len: int,
    ts_format: TimestampFormat | None = None,
    lookahead_records: int = DEFAULT_LOOKAHEAD_RECORDS,
) -> bool:
    """Return whether a valid MIE record starts at the given offset."""
    return validate_record_detailed(
        data,
        offset,
        file_len,
        ts_format,
        lookahead_records,
    ) is None


def validate_record_detailed(
    data: bytes | memoryview,
    offset: int,
    file_len: int,
    ts_format: TimestampFormat | None = None,
    lookahead_records: int = DEFAULT_LOOKAHEAD_RECORDS,
) -> ValidationFailure | None:
    """Return the precise reason a candidate record fails validation.

    Applies fast heuristic checks in order of increasing cost. Does
    NOT decode the full record — only reads the Type Word (2 bytes)
    and optionally the timestamp words (4–6 bytes) for range validation.

    Args:
        data: Raw byte buffer (mmap or bytes).
        offset: Candidate byte offset to test.
        file_len: Total file length in bytes.
        ts_format: Known timestamp format, or None to skip timestamp
            field-range checks.

    Returns:
        ``None`` for a valid record, otherwise the failure reason.
    """
    # ── Check 1: Enough bytes for a minimal Type Word read ─────────
    if offset + 2 > file_len:
        return ValidationFailure.TYPE_WORD_UNREADABLE

    type_raw = read_u16(data, offset)
    tw = decode_type_word(type_raw)

    # ── Check 2: Valid message type ────────────────────────────────
    if not is_valid_message_type(tw.message_type):
        return ValidationFailure.UNKNOWN_MESSAGE_TYPE

    # ── Check 3: Plausible word count ──────────────────────────────
    if ts_format is not None:
        min_wc = 1 + TIMESTAMP_WORD_COUNTS[ts_format] + 1
    else:
        # Use the smaller minimum if format unknown
        min_wc = MIN_RECORD_WORDS_STANDARD
    if tw.word_count < min_wc or tw.word_count > 63:
        return ValidationFailure.INVALID_WORD_COUNT

    # ── Check 4: Record fits within file ───────────────────────────
    record_bytes = tw.word_count * 2
    if offset + record_bytes > file_len:
        return ValidationFailure.RECORD_TRUNCATED

    # ── Check 5: IRIG timestamp field ranges (L2-SYN-004, L2-SYN-019)
    # All three timestamp words are needed to evaluate microsecond
    # and day; offset + 8 <= file_len covers reading upper (offset+2),
    # middle (offset+4), and lower (offset+6) words.
    if ts_format == TimestampFormat.IRIG and offset + 8 <= file_len:
        ts_upper = read_u16(data, offset + 2)
        ts_middle = read_u16(data, offset + 4)
        ts_lower = read_u16(data, offset + 6)
        freerun = bool((ts_upper >> 15) & 1)
        day = (ts_upper >> 5) & 0x1FF  # bits 13-5
        hour = ts_upper & 0x1F
        minute = (ts_middle >> 10) & 0x3F
        second = (ts_middle >> 4) & 0x3F
        microsecond_hi4 = ts_middle & 0xF
        microsecond_lo16 = ts_lower
        microsecond = (microsecond_hi4 << 16) | microsecond_lo16

        if hour >= 24:
            return ValidationFailure.IRIG_HOUR_OUT_OF_RANGE
        if minute >= 60:
            return ValidationFailure.IRIG_MINUTE_OUT_OF_RANGE
        if second >= 60:
            return ValidationFailure.IRIG_SECOND_OUT_OF_RANGE
        if microsecond > 999_999:
            return ValidationFailure.IRIG_MICROSECOND_OUT_OF_RANGE
        # L2-SYN-019: skip the day-of-year range check when freerun
        # is set, because the card's free-running oscillator is not
        # calendar-locked. Hour/minute/second/microsecond constraints
        # still apply because those are a function of the counter
        # modulus, not the external IRIG-B feed.
        if not freerun and not (1 <= day <= 366):
            return ValidationFailure.IRIG_DAY_OUT_OF_RANGE

    # ── Check 6: N-record look-ahead (L2-SYN-005, L2-SYN-026) ──────
    # Walk up to lookahead_records - 1 subsequent records, validating
    # each Type Word's message type + word count plausibility. Advance
    # by each candidate's declared word_count. EOF terminates the walk
    # gracefully without rejecting the original candidate.
    n = max(1, lookahead_records)
    next_offset = offset + record_bytes
    for _ in range(1, n):
        if next_offset + 2 > file_len:
            break
        next_raw = read_u16(data, next_offset)
        next_tw = decode_type_word(next_raw)
        if not is_valid_message_type(next_tw.message_type):
            return ValidationFailure.LOOKAHEAD_UNKNOWN_MESSAGE_TYPE
        if next_tw.word_count < min_wc or next_tw.word_count > 63:
            return ValidationFailure.LOOKAHEAD_INVALID_WORD_COUNT
        next_record_bytes = next_tw.word_count * 2
        if next_record_bytes == 0:
            break
        next_offset += next_record_bytes

    return None


def find_first_record(
    data: bytes | memoryview,
    file_len: int,
    ts_format: TimestampFormat | None = None,
    max_scan: int = MAX_SCAN_BYTES,
    lookahead_records: int = DEFAULT_LOOKAHEAD_RECORDS,
) -> int | None:
    """Find the byte offset of the first valid record in the file.

    Scans from offset 0 in 2-byte (word-aligned) increments, applying
    :func:`validate_record` at each position. Returns the offset of
    the first position that passes all validation checks, or None if
    no valid record is found within the scan distance.

    This handles files with and without headers:
    - Files starting directly with records return offset 0.
    - Files with proprietary headers (e.g., embedded ASCII equipment
      names) return the offset immediately after the header.

    Args:
        data: Raw byte buffer.
        file_len: Total file length in bytes.
        ts_format: Known timestamp format, or None for auto-detection
            (skips timestamp range checks during header scan).
        max_scan: Maximum bytes to scan before giving up.

    Returns:
        Byte offset of the first valid record, or None if not found.
    """
    scan_end = min(file_len, max_scan)

    for offset in range(0, scan_end, 2):
        if validate_record(data, offset, file_len, ts_format, lookahead_records):
            if offset > 0:
                logger.info(
                    "File header detected: %d bytes before first record "
                    "at offset 0x%X",
                    offset, offset,
                )
            else:
                logger.debug("First record at offset 0 (no header)")
            return offset

    logger.warning(
        "No valid record found in first %d bytes of file", scan_end
    )
    return None


#: Number of consecutive candidate records sampled by
#: :func:`is_homogeneous_payload` for the L2-SYN-018 defense. Must be
#: >= 4 per the spec.
HOMOGENEITY_SAMPLE_RECORDS: Final[int] = 4


def is_homogeneous_payload(
    data: bytes | memoryview,
    offset: int,
    record_bytes: int,
) -> bool:
    """L2-SYN-018: detect pathological homogeneous-payload inputs.

    Checks whether the first :data:`HOMOGENEITY_SAMPLE_RECORDS`
    consecutive ``record_bytes``-sized chunks starting at ``offset``
    are byte-identical in every position except the timestamp triple
    (bytes 2..8 of each record). A homogeneous match means the file is
    most likely a single-byte pad (e.g. 0x20-fill) where every "record"
    parses as a synthetic SPURIOUS_DATA frame and look-ahead validation
    alone admits the stream.

    Returns True iff the run is homogeneous and the reader should
    reject the input as pathological.
    """
    total = HOMOGENEITY_SAMPLE_RECORDS * record_bytes
    if offset + total > len(data):
        return False
    first = bytes(data[offset:offset + record_bytes])
    for i in range(1, HOMOGENEITY_SAMPLE_RECORDS):
        rec_start = offset + i * record_bytes
        other = bytes(data[rec_start:rec_start + record_bytes])
        # Compare positions [0..2) (Type Word) and [8..record_bytes)
        # (Cmd + payload). Skip bytes 2..8 (IRIG timestamp triple).
        # For Standard-format records (4-byte timestamp), this skip
        # is conservative — we ignore 2 extra bytes of the Cmd field,
        # which only weakens the rejection slightly.
        if first[:2] != other[:2]:
            return False
        if record_bytes > 8 and first[8:] != other[8:]:
            return False
    return True


def diagnose_header_scan_failure(
    data: bytes | memoryview,
    file_len: int,
    ts_format: TimestampFormat | None = None,
    max_scan: int = MAX_SCAN_BYTES,
) -> tuple[int, int, int] | None:
    """Locate the first structurally-valid Type Word that's truncated.

    Called after :func:`find_first_record` returns None to distinguish
    "no MIE record at all" (L1-EXIT-002 / MieNoValidRecordsError) from
    "valid Type Word found but its declared extent runs past EOF"
    (L2-RDR-004 / MieFirstRecordTruncatedError).

    Walks the same 2-byte-aligned grid as :func:`find_first_record` but
    omits the fits-in-file check and the two-record look-ahead — so it
    matches a Type Word that *would have been valid* if the file were
    long enough.

    Returns:
        ``(offset, declared_bytes, available_bytes)`` for the first
        structurally-valid Type Word that fails only the length check,
        or ``None`` if no such Type Word exists in the scan window.
    """
    scan_end = min(file_len, max_scan)
    for offset in range(0, scan_end, 2):
        if offset + 2 > file_len:
            break
        type_raw = read_u16(data, offset)
        tw = decode_type_word(type_raw)
        if not is_valid_message_type(tw.message_type):
            continue
        # Minimum payload size depends on timestamp format; assume IRIG
        # when unknown (the longer minimum is the more permissive bound
        # for "what looks like a Type Word").
        ts_words = 3 if (ts_format or TimestampFormat.IRIG) == TimestampFormat.IRIG else 2
        if tw.word_count < 1 + ts_words + 1 or tw.word_count > 63:
            continue
        record_bytes = tw.word_count * 2
        # Only the length check is allowed to fail; everything else
        # must look right. If extent fits in file, find_first_record
        # would have already returned this offset, so the only way to
        # reach here is a length-driven rejection.
        if offset + record_bytes > file_len:
            return offset, record_bytes, file_len - offset
        # Type Word looks valid and fits, but find_first_record didn't
        # pick it — most likely the IRIG range check or two-record
        # look-ahead rejected it. Keep scanning.
    return None


def recover_sync(
    data: bytes | memoryview,
    offset: int,
    file_len: int,
    ts_format: TimestampFormat | None = None,
    max_scan: int = MAX_SCAN_BYTES,
    lookahead_records: int = DEFAULT_LOOKAHEAD_RECORDS,
) -> int | None:
    """Recover sync by scanning forward for the next valid record.

    Called when the reader encounters an invalid record at the current
    position. Scans forward in 2-byte increments from ``offset + 2``,
    applying :func:`validate_record` at each position.

    Args:
        data: Raw byte buffer.
        offset: Current (invalid) position.
        file_len: Total file length.
        ts_format: Known timestamp format, or None.
        max_scan: Maximum bytes to scan from the current offset.

    Returns:
        Byte offset of the next valid record, or None if sync cannot
        be recovered within the scan distance.
    """
    scan_start = offset + 2
    scan_end = min(file_len, offset + max_scan)

    logger.warning(
        "Sync lost at offset 0x%X — scanning forward for next valid record",
        offset,
    )

    for candidate in range(scan_start, scan_end, 2):
        if validate_record(data, candidate, file_len, ts_format, lookahead_records):
            skipped = candidate - offset
            logger.info(
                "Sync recovered at offset 0x%X (skipped %d bytes from 0x%X)",
                candidate, skipped, offset,
            )
            return candidate

    logger.error(
        "Sync recovery failed — no valid record found within %d bytes "
        "of offset 0x%X",
        max_scan, offset,
    )
    return None
