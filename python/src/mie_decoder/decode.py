"""Binary decoding routines for DDC MIE record fields.

This module contains pure functions that decode raw bytes into the
structured types defined in :mod:`mie_decoder.models`. All functions
operate on ``bytes`` or ``memoryview`` objects and use ``struct`` for
portable little-endian unpacking.

No external packages are required.
"""

from __future__ import annotations

import struct
from dataclasses import dataclass
from enum import IntEnum
from typing import Final

from mie_decoder.models import (
    Bus,
    CommandWord,
    Direction,
    IrigTimestamp,
    MessageFormat,
    MessageType,
    StandardTimestamp,
    Timestamp,
    TimestampFormat,
    TypeWord,
    TIMESTAMP_WORD_COUNTS,
    VALID_MESSAGE_TYPES,
)

#: Minimum record size in bytes for IRIG: Type(2) + TS(6) + Cmd(2) = 10
MIN_RECORD_BYTES: Final[int] = 10

#: Minimum record size in bytes for Standard: Type(2) + TS(4) + Cmd(2) = 8
MIN_RECORD_BYTES_STANDARD: Final[int] = 8

#: Minimum record size in 16-bit words for IRIG: Type(1) + TS(3) + Cmd(1) = 5
MIN_RECORD_WORDS: Final[int] = 5

#: Minimum record size in 16-bit words for Standard: Type(1) + TS(2) + Cmd(1) = 4
MIN_RECORD_WORDS_STANDARD: Final[int] = 4

#: struct format for a single little-endian unsigned 16-bit word
_LE_U16: Final[str] = "<H"


def decode_type_word(raw: int) -> TypeWord:
    """Decode a 16-bit Type Word into its constituent fields.

    Args:
        raw: The raw 16-bit unsigned integer value read from the binary
            file in little-endian byte order.

    Returns:
        A populated TypeWord with message_type, bus, word_count, error,
        and the original raw value.

    Examples:
        >>> tw = decode_type_word(0x2402)
        >>> tw.message_type
        2
        >>> tw.bus
        <Bus.A: 0>
        >>> tw.word_count
        36
    """
    message_type = raw & 0x7F
    bus = Bus((raw >> 7) & 1)
    word_count = (raw >> 8) & 0x3F
    error = bool((raw >> 14) & 1)
    return TypeWord(
        message_type=message_type,
        bus=bus,
        word_count=word_count,
        error=error,
        raw=raw,
    )


def decode_irig_timestamp(upper: int, middle: int, lower: int) -> IrigTimestamp:
    """Decode a 3-word IRIG timestamp into structured fields.

    The three 16-bit words are read from the binary file in little-endian
    order immediately following the Type Word.

    Args:
        upper: Upper word containing freerun flag, day, and hour.
        middle: Middle word containing minutes, seconds, and upper
            microsecond bits [19:16].
        lower: Lower word containing microsecond bits [15:0].

    Returns:
        A populated IrigTimestamp.

    Examples:
        >>> ts = decode_irig_timestamp(0x180F, 0xDB26, 0xF621)
        >>> ts.hour
        15
        >>> ts.minute
        54
        >>> ts.microsecond
        456225
    """
    freerun = bool((upper >> 15) & 1)
    day = (upper >> 5) & 0x1FF
    hour = upper & 0x1F
    minute = (middle >> 10) & 0x3F
    second = (middle >> 4) & 0x3F
    us_hi = middle & 0xF
    us_lo = lower
    microsecond = (us_hi << 16) | us_lo
    return IrigTimestamp(
        day=day,
        hour=hour,
        minute=minute,
        second=second,
        microsecond=microsecond,
        freerun=freerun,
    )


def decode_standard_timestamp(upper: int, lower: int) -> StandardTimestamp:
    """Decode a 2-word Standard-format timestamp.

    The Standard timestamp is a 32-bit monotonic counter split across
    two 16-bit little-endian words. The upper word contains bits [31:16]
    and the lower word contains bits [15:0].

    Args:
        upper: Upper word (bits [31:16] of the 32-bit counter).
        lower: Lower word (bits [15:0] of the 32-bit counter).

    Returns:
        A populated StandardTimestamp with the reconstructed 32-bit value.

    Examples:
        >>> ts = decode_standard_timestamp(0x0001, 0x86A0)
        >>> ts.raw_value
        100000
    """
    raw_value = (upper << 16) | lower
    return StandardTimestamp(
        raw_value=raw_value,
        upper_word=upper,
        lower_word=lower,
    )


def detect_timestamp_format(
    data: bytes | memoryview,
    offset: int,
    type_word: TypeWord,
) -> TimestampFormat:
    """Auto-detect whether a record uses IRIG or Standard timestamps.

    Probes the first record by attempting to read the Command Word at
    both possible offsets and checking which produces a valid 1553
    Command Word. The detection uses multiple heuristics for robustness:

    1. **Command Word validity**: A valid Command Word has RT address
       0–31, subaddress 0–31, and word count 0–31. Both offsets will
       always satisfy these trivially (all fields are 5-bit), so
       additional checks are needed.

    2. **Message type consistency**: For type 0x02 (BC→RT), the Command
       Word's T/R bit should be 0 (Receive). For type 0x04 (RT→BC),
       it should be 1 (Transmit). The offset that produces a Command
       Word consistent with the Type Word's message type is preferred.

    3. **Word count plausibility**: The Type Word's word count minus
       the fixed overhead (type + timestamp + cmd + status) should
       equal the Command Word's data word count. The offset that
       produces a consistent word count relationship is preferred.

    4. **IRIG field range checks**: If IRIG is a candidate, check that
       decoded hour < 24, minute < 60, second < 60, microsecond < 1M.
       Valid ranges increase IRIG confidence.

    Args:
        data: Raw byte buffer (file contents or mmap).
        offset: Byte offset of the record to probe.
        type_word: The already-decoded Type Word.

    Returns:
        ``TimestampFormat.IRIG`` or ``TimestampFormat.STANDARD``.
        Never returns ``TimestampFormat.AUTO``.
    """
    file_len = len(data)

    # ── Gather candidates ──────────────────────────────────────────
    irig_score = 0
    std_score = 0

    # IRIG: Command Word at offset+8 (after 1 TypeWord + 3 TS words)
    irig_cmd_offset = offset + 8
    if irig_cmd_offset + 2 <= file_len:
        irig_cmd_raw = read_u16(data, irig_cmd_offset)
        irig_cmd = decode_command_word(irig_cmd_raw)

        # Check T/R consistency with message type
        if type_word.message_type == MessageType.BC_TO_RT:
            if irig_cmd.direction == Direction.RECEIVE:
                irig_score += 2
        elif type_word.message_type == MessageType.RT_TO_BC:
            if irig_cmd.direction == Direction.TRANSMIT:
                irig_score += 2

        # Check word count plausibility (IRIG overhead = 4 + 1 cmd + 1 stat = 6)
        expected_data_wc_irig = type_word.word_count - 6
        if expected_data_wc_irig == irig_cmd.data_word_count:
            irig_score += 2

        # Check IRIG timestamp field ranges
        ts_upper = read_u16(data, offset + 2)
        ts_middle = read_u16(data, offset + 4)
        hour = ts_upper & 0x1F
        minute = (ts_middle >> 10) & 0x3F
        second = (ts_middle >> 4) & 0x3F
        us_hi = ts_middle & 0xF
        if hour < 24 and minute < 60 and second < 60 and us_hi < 16:
            irig_score += 1

    # Standard: Command Word at offset+6 (after 1 TypeWord + 2 TS words)
    std_cmd_offset = offset + 6
    if std_cmd_offset + 2 <= file_len:
        std_cmd_raw = read_u16(data, std_cmd_offset)
        std_cmd = decode_command_word(std_cmd_raw)

        # Check T/R consistency
        if type_word.message_type == MessageType.BC_TO_RT:
            if std_cmd.direction == Direction.RECEIVE:
                std_score += 2
        elif type_word.message_type == MessageType.RT_TO_BC:
            if std_cmd.direction == Direction.TRANSMIT:
                std_score += 2

        # Check word count plausibility (Standard overhead = 3 + 1 cmd + 1 stat = 5)
        expected_data_wc_std = type_word.word_count - 5
        if expected_data_wc_std == std_cmd.data_word_count:
            std_score += 2

    # ── Decision ───────────────────────────────────────────────────
    if irig_score > std_score:
        return TimestampFormat.IRIG
    elif std_score > irig_score:
        return TimestampFormat.STANDARD
    else:
        # Tie-break: default to IRIG (more common in flight test)
        return TimestampFormat.IRIG


def decode_command_word(raw: int) -> CommandWord:
    """Decode a 16-bit MIL-STD-1553 Command Word.

    Args:
        raw: The raw 16-bit unsigned integer value.

    Returns:
        A populated CommandWord with RT address, direction, subaddress,
        and data word count.

    Examples:
        >>> cw = decode_command_word(0x797E)
        >>> cw.rt
        15
        >>> cw.subaddress
        11
        >>> cw.data_word_count
        30
    """
    rt = (raw >> 11) & 0x1F
    direction = Direction((raw >> 10) & 1)
    subaddress = (raw >> 5) & 0x1F
    data_word_count = raw & 0x1F
    if data_word_count == 0:
        data_word_count = 32
    return CommandWord(
        rt=rt,
        direction=direction,
        subaddress=subaddress,
        data_word_count=data_word_count,
        raw=raw,
    )


def classify_message_format(
    message_type: int,
    command_word: CommandWord,
    word_count: int,
) -> MessageFormat:
    """Classify a record into one of the 10 MIL-STD-1553 message formats.

    Uses a multi-signal approach for robust classification:

    1. The Type Word's message_type code (bits 0–6) determines the
       high-level category:
       - 0x02 → RECEIVE
       - 0x04 → TRANSMIT
       - 0x08 → RT_TO_RT
       - 0x10 → RECEIVE_BROADCAST
       - 0x18 → RT_TO_RT_BROADCAST

    2. For 0x01 (MODE_COMMAND), the Command Word is inspected:
       a. RT address == 31 → broadcast mode code
          - Record word count > 5 (has data word) → MODE_CODE_BCAST_DATA
          - Else → MODE_CODE_BCAST_NO_DATA
       b. RT address != 31 → non-broadcast mode code
          - T/R bit == 1 (transmit) → MODE_CODE_TX_DATA
          - T/R bit == 0 (receive) and word count indicates data
            → MODE_CODE_RX_DATA
          - T/R bit == 0 and no data → MODE_CODE_NO_DATA

       The word count cross-check adds robustness:
       - MODE_CODE_BCAST_NO_DATA: WC = 5 (Type + 3×TS + ModeCmd)
       - MODE_CODE_BCAST_DATA:    WC = 6 (+ 1 DataWord)
       - MODE_CODE_NO_DATA:       WC = 6 (+ Status)
       - MODE_CODE_TX_DATA:       WC = 7 (+ Status + DataWord)
       - MODE_CODE_RX_DATA:       WC = 7 (+ DataWord + Status)

    3. For 0x20 (SPURIOUS_DATA), classification is deferred pending
       error format documentation. Currently raises ValueError.

    Args:
        message_type: The type code from bits 0–6 of the Type Word.
        command_word: The decoded primary Command Word.
        word_count: Total record word count from the Type Word.

    Returns:
        The classified MessageFormat.

    Raises:
        ValueError: If the message_type is 0x20 (SPURIOUS_DATA) or
            cannot be classified.

    Examples:
        >>> cmd = decode_command_word(0x797E)  # RT15, Receive, SA11
        >>> classify_message_format(0x02, cmd, 36)
        <MessageFormat.RECEIVE: 1>
    """
    # ── Direct type-to-format mappings (one code → one format) ──────
    if message_type == MessageType.BC_TO_RT:
        return MessageFormat.RECEIVE

    if message_type == MessageType.RT_TO_BC:
        return MessageFormat.TRANSMIT

    if message_type == MessageType.RT_TO_RT:
        return MessageFormat.RT_TO_RT

    if message_type == MessageType.BROADCAST_BC_TO_RT:
        return MessageFormat.RECEIVE_BROADCAST

    if message_type == MessageType.BROADCAST_RT_TO_RT:
        return MessageFormat.RT_TO_RT_BROADCAST

    # ── Mode Command sub-classification (0x01 → 5 possible formats) ─
    if message_type == MessageType.MODE_COMMAND:
        return _classify_mode_code(command_word, word_count)

    # ── Spurious data — raw bus words, no command structure ──────
    if message_type == MessageType.SPURIOUS_DATA:
        return MessageFormat.SPURIOUS_DATA

    # ── Should not be reachable if VALID_MESSAGE_TYPES was checked ──
    raise ValueError(f"Cannot classify message_type=0x{message_type:02X}")


def _classify_mode_code(cmd: CommandWord, word_count: int) -> MessageFormat:
    """Sub-classify a Mode Command (type 0x01) into one of five formats.

    Uses a layered approach combining Command Word fields with the
    record word count for cross-validation:

    Decision tree:
        ┌─ RT == 31 (broadcast)?
        │  ├─ WC > 5 → MODE_CODE_BCAST_DATA    (ModeCmd + DataWord = 6 words)
        │  └─ WC == 5 → MODE_CODE_BCAST_NO_DATA (ModeCmd only = 5 words)
        └─ RT != 31 (non-broadcast)?
           ├─ T/R == 1 (transmit) → MODE_CODE_TX_DATA (ModeCmd+Status+Data = 7 words)
           ├─ WC >= 7 → MODE_CODE_RX_DATA (ModeCmd+Data+Status = 7 words)
           └─ WC == 6 → MODE_CODE_NO_DATA (ModeCmd+Status = 6 words)

    Args:
        cmd: The decoded Mode Command Word.
        word_count: Total record word count from the Type Word.

    Returns:
        One of the five mode code MessageFormat variants.
    """
    is_broadcast = cmd.rt == 31

    if is_broadcast:
        # Broadcast mode codes have no status word.
        # With data: Type(1) + TS(3) + ModeCmd(1) + Data(1) = 6
        # Without:   Type(1) + TS(3) + ModeCmd(1)            = 5
        if word_count > 5:
            return MessageFormat.MODE_CODE_BCAST_DATA
        return MessageFormat.MODE_CODE_BCAST_NO_DATA

    # Non-broadcast mode codes always have a status word.
    if cmd.direction == Direction.TRANSMIT:
        # RT transmits: ModeCmd → Status → DataWord (WC = 7)
        return MessageFormat.MODE_CODE_TX_DATA

    # RT receives or no data:
    # With data:    ModeCmd → DataWord → Status (WC = 7)
    # Without data: ModeCmd → Status            (WC = 6)
    if word_count >= 7:
        return MessageFormat.MODE_CODE_RX_DATA

    return MessageFormat.MODE_CODE_NO_DATA


class WhichInvariant(IntEnum):
    """Which structural invariant a record violated (L2-SYN-INV).

    Used by callers (the reader) to phrase a precise diagnostic; the
    strict-mode path otherwise maps every violation to a single
    record-error class.
    """

    DIRECTION_BC_TO_RT = 1
    """L2-SYN-INV-001: Type 0x02 requires Cmd direction = Receive."""

    DIRECTION_RT_TO_BC = 2
    """L2-SYN-INV-002: Type 0x04 requires Cmd direction = Transmit."""

    WORD_COUNT_CAPACITY = 3
    """L2-SYN-INV-003: TW.word_count too small for declared payload."""

    DIRECTION_RT_TO_RT_CMD2 = 4
    """L2-SYN-INV-004: Cmd2 direction for RT-to-RT must be Receive."""

    STATUS_RT_MISMATCH = 5
    """L2-SYN-INV-005: Status RT does not match Cmd RT.
    AnomalyWarn-class — real-bus noise possible."""

    TYPE_WORD_RESERVED_BIT = 6
    """L2-SYN-INV-006: Type Word bit 15 (reserved) is set.
    AnomalyWarn-class — possible vendor extension."""


class InvariantSeverity(IntEnum):
    """Policy class for a structural-invariant violation.

    - ``REJECT``: strict mode aborts; lenient mode WARN+skips the record.
    - ``ANOMALY_WARN``: both modes log a WARN and emit the record
      anyway. Used where outright rejection would produce false
      negatives on real-bus noise or vendor extensions (INV-005, 006).
    """

    REJECT = 1
    ANOMALY_WARN = 2


@dataclass(frozen=True)
class InvariantViolation:
    """Detail object returned by invariant-check functions."""

    kind: WhichInvariant
    severity: InvariantSeverity
    detail: str


def _min_payload_words(fmt: MessageFormat, command_word: CommandWord) -> int:
    """Per-format minimum payload word count, computed from Cmd1's
    declared data_word_count. Mirrors the Rust ``min_payload_words``
    helper. ``SPURIOUS_DATA`` returns 0 (capacity check skipped).
    """
    dwc = command_word.data_word_count
    if fmt == MessageFormat.RECEIVE or fmt == MessageFormat.TRANSMIT:
        return dwc + 1
    if fmt == MessageFormat.RT_TO_RT:
        return dwc + 3
    if fmt == MessageFormat.RECEIVE_BROADCAST:
        return dwc
    if fmt == MessageFormat.RT_TO_RT_BROADCAST:
        return dwc + 2
    if fmt == MessageFormat.MODE_CODE_TX_DATA:
        return 2
    if fmt == MessageFormat.MODE_CODE_RX_DATA:
        return 2
    if fmt == MessageFormat.MODE_CODE_NO_DATA:
        return 1
    if fmt == MessageFormat.MODE_CODE_BCAST_NO_DATA:
        return 0
    if fmt == MessageFormat.MODE_CODE_BCAST_DATA:
        return 1
    if fmt == MessageFormat.SPURIOUS_DATA:
        return 0  # variable; no capacity check
    raise ValueError(f"Unhandled message format: {fmt}")


def validate_structural_invariants(
    type_word: TypeWord,
    command_word: CommandWord,
    msg_fmt: MessageFormat,
    ts_words: int,
) -> InvariantViolation | None:
    """L2-SYN-INV: structural invariants per the locked schema.

    Returns ``None`` if all invariants hold; otherwise returns an
    :class:`InvariantViolation` describing the first failure.

    Current invariant set:

    - INV-001: Type 0x02 (BC→RT) → Cmd direction = Receive
    - INV-002: Type 0x04 (RT→BC) → Cmd direction = Transmit
    - INV-003: ``TW.word_count >= 1 + ts_words + 1 + min_payload_words(format, cmd)``

    Deferred (Phase 7b): cmd2 direction for RT-to-RT, Status RT vs
    Cmd RT match, reserved-bit zero check.
    """
    # INV-001 / INV-002: per-type direction.
    if (
        type_word.message_type == MessageType.BC_TO_RT
        and command_word.direction != Direction.RECEIVE
    ):
        return InvariantViolation(
            kind=WhichInvariant.DIRECTION_BC_TO_RT,
            severity=InvariantSeverity.REJECT,
            detail=(
                f"Type 0x02 (BC→RT) requires Cmd direction = Receive; "
                f"got Transmit (raw Cmd = 0x{command_word.raw:04X})"
            ),
        )
    if (
        type_word.message_type == MessageType.RT_TO_BC
        and command_word.direction != Direction.TRANSMIT
    ):
        return InvariantViolation(
            kind=WhichInvariant.DIRECTION_RT_TO_BC,
            severity=InvariantSeverity.REJECT,
            detail=(
                f"Type 0x04 (RT→BC) requires Cmd direction = Transmit; "
                f"got Receive (raw Cmd = 0x{command_word.raw:04X})"
            ),
        )

    # INV-003: word-count capacity check.
    min_wc = 1 + ts_words + 1 + _min_payload_words(msg_fmt, command_word)
    if type_word.word_count < min_wc:
        return InvariantViolation(
            kind=WhichInvariant.WORD_COUNT_CAPACITY,
            severity=InvariantSeverity.REJECT,
            detail=(
                f"TW.word_count = {type_word.word_count} is too small for "
                f"declared payload (need at least {min_wc} for {msg_fmt.name} "
                f"with data_word_count = {command_word.data_word_count})"
            ),
        )

    return None


def validate_post_extract_invariants(
    msg_fmt: MessageFormat,
    cmd2: CommandWord | None,
) -> InvariantViolation | None:
    """L2-SYN-INV-004: Cmd2 direction check for RT-to-RT formats.

    Called post-extract because Cmd2 lives inside the payload. For
    non-RT-to-RT formats (or when cmd2 is None) this is a no-op.
    """
    if msg_fmt not in (MessageFormat.RT_TO_RT, MessageFormat.RT_TO_RT_BROADCAST):
        return None
    if cmd2 is None:
        return None
    if cmd2.direction != Direction.RECEIVE:
        return InvariantViolation(
            kind=WhichInvariant.DIRECTION_RT_TO_RT_CMD2,
            severity=InvariantSeverity.REJECT,
            detail=(
                f"RT-to-RT Cmd2 requires direction = Receive; got "
                f"Transmit (raw Cmd2 = 0x{cmd2.raw:04X})"
            ),
        )
    return None


def detect_record_anomalies(
    type_word: TypeWord,
    command_word: CommandWord,
    status_word: int | None,
) -> list[InvariantViolation]:
    """L2-SYN-INV-005 / L2-SYN-INV-006: anomaly-class observations.

    Both invariants are anomaly detectors rather than corruption
    rejections; the reader logs each violation as a WARN and continues
    emitting the record. Returns a list because multiple anomalies can
    fire on the same record (e.g., status RT mismatch AND reserved bit
    set simultaneously).
    """
    out: list[InvariantViolation] = []

    # INV-005: Status RT vs Cmd RT.
    if status_word is not None:
        status_rt = (status_word >> 11) & 0x1F
        if status_rt != command_word.rt:
            out.append(
                InvariantViolation(
                    kind=WhichInvariant.STATUS_RT_MISMATCH,
                    severity=InvariantSeverity.ANOMALY_WARN,
                    detail=(
                        f"Status RT = {status_rt} does not match Cmd RT = "
                        f"{command_word.rt} (raw Status = 0x{status_word:04X}); "
                        f"possible bus interference"
                    ),
                )
            )

    # INV-006: Type Word bit 15 reserved.
    if (type_word.raw >> 15) & 1 != 0:
        out.append(
            InvariantViolation(
                kind=WhichInvariant.TYPE_WORD_RESERVED_BIT,
                severity=InvariantSeverity.ANOMALY_WARN,
                detail=(
                    f"Type Word bit 15 (reserved) is set in raw "
                    f"0x{type_word.raw:04X}; possible undocumented vendor "
                    f"extension"
                ),
            )
        )

    return out


def is_valid_message_type(message_type: int) -> bool:
    """Check if a message type code is one of the 7 known DDC types.

    Args:
        message_type: The type code from bits 0–6 of the Type Word.

    Returns:
        True if the code is recognized (0x01, 0x02, 0x04, 0x08, 0x10,
        0x18, or 0x20).
    """
    return message_type in VALID_MESSAGE_TYPES


def read_u16(data: bytes | memoryview, offset: int) -> int:
    """Read a single little-endian unsigned 16-bit integer.

    Args:
        data: Raw byte buffer.
        offset: Byte offset to read from.

    Returns:
        The decoded 16-bit unsigned integer.

    Raises:
        struct.error: If there are not enough bytes at the given offset.
    """
    return struct.unpack_from(_LE_U16, data, offset)[0]


def read_u16_array(data: bytes | memoryview, offset: int, count: int) -> tuple[int, ...]:
    """Read an array of little-endian unsigned 16-bit integers.

    Args:
        data: Raw byte buffer.
        offset: Starting byte offset.
        count: Number of 16-bit words to read.

    Returns:
        A tuple of decoded 16-bit unsigned integers.

    Raises:
        struct.error: If there are not enough bytes for the requested count.
    """
    fmt = f"<{count}H"
    return struct.unpack_from(fmt, data, offset)
