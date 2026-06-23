"""Core data structures for decoded MIL-STD-1553 MIE binary records.

This module defines the immutable data structures that represent decoded
1553 messages extracted from DDC MIE binary recording files. All structures
use ``dataclass(frozen=True, slots=True)`` for memory efficiency and
immutability guarantees.

Type Word Message Type Codes (bits 0–6):

    0x01  Mode Command — sub-classified into 5 formats by Command Word
    0x02  BC→RT (Receive) — Bus Controller sends data to Remote Terminal
    0x04  RT→BC (Transmit) — Remote Terminal sends data to Bus Controller
    0x08  RT→RT (Terminal-to-Terminal) — BC commands one RT to send to another
    0x10  Broadcast BC→RT — BC sends data to all RTs (RT address 31)
    0x18  Broadcast RT→RT — BC commands RT-to-RT transfer, all RTs listen
    0x20  Spurious Data — unstructured bus noise captured by the monitor

Error Handling:

    When the DDC card detects an error mid-transaction (Manchester error,
    parity error, missing response, etc.), it:
    1. Sets bit 14 of the Type Word to flag the record as errored.
    2. Captures bus words received up to the point of error.
    3. Appends a 16-bit Error Word (error code) as the last word.
    4. If remaining words exist from the original transaction, they are
       written as a separate SPURIOUS_DATA (0x20) record immediately
       following.

    DDC Hardware Error Codes (0x01xx range):
        0x011E  Manchester/Parity Error or Bit Count Error
        0x0120  No Status Response or Too Few Data Words
        0x0136  Inverted Sync on Data Word
        0x0140  Too Many Data Words
        0x0150  Unknown/TBD

    MIE-Decoder Custom Error Codes (0x20xx range):
        0x2000  Spurious Data: Continuation of preceding errored message
        0x2001  Spurious Data: Standalone (no preceding error record)
"""

from __future__ import annotations

import math
import mmap
from dataclasses import dataclass
from enum import IntEnum, unique
from typing import Final

#: A read-only byte source accepted by the low-level decode/sync helpers:
#: an in-memory buffer (bytes/memoryview) or a memory-mapped file. The
#: reader passes an ``mmap.mmap``; tests and conformance fixtures pass
#: ``bytes``. All three support the indexing and slicing these helpers do.
ByteSource = bytes | memoryview | mmap.mmap


@unique
class Bus(IntEnum):
    """MIL-STD-1553 redundant bus identifier."""

    A = 0
    B = 1


@unique
class Direction(IntEnum):
    """MIL-STD-1553 message transfer direction.

    From the perspective of the Remote Terminal:
        RECEIVE: Bus Controller sends data TO the RT (BC→RT).
        TRANSMIT: RT sends data TO the Bus Controller (RT→BC).
    """

    RECEIVE = 0
    TRANSMIT = 1


@unique
class MessageType(IntEnum):
    """DDC MIE Type Word message type codes (bits 0–6).

    Values:
        MODE_COMMAND: 0x01 — Mode code message.
        BC_TO_RT: 0x02 — Bus Controller sends data to RT (Receive).
        RT_TO_BC: 0x04 — RT sends data to Bus Controller (Transmit).
        RT_TO_RT: 0x08 — Terminal-to-Terminal transfer.
        BROADCAST_BC_TO_RT: 0x10 — Broadcast Receive.
        BROADCAST_RT_TO_RT: 0x18 — Broadcast Terminal-to-Terminal.
        SPURIOUS_DATA: 0x20 — Spurious data / bus noise / error continuation.
    """

    MODE_COMMAND = 0x01
    BC_TO_RT = 0x02
    RT_TO_BC = 0x04
    RT_TO_RT = 0x08
    BROADCAST_BC_TO_RT = 0x10
    BROADCAST_RT_TO_RT = 0x18
    SPURIOUS_DATA = 0x20


#: Set of all valid message type codes for fast membership testing.
VALID_MESSAGE_TYPES: frozenset[int] = frozenset(m.value for m in MessageType)


@unique
class MessageFormat(IntEnum):
    """Classified message format determining the payload layout.

    Each format has a distinct sequence of command words, status words,
    and data words after the Type Word and timestamp.

    Values 1–10 are the standard MIL-STD-1553 message formats.
    Value 11 is the SPURIOUS_DATA format (raw bus words, no command).
    """

    RECEIVE = 1
    TRANSMIT = 2
    RT_TO_RT = 3
    RECEIVE_BROADCAST = 4
    RT_TO_RT_BROADCAST = 5
    MODE_CODE_TX_DATA = 6
    MODE_CODE_RX_DATA = 7
    MODE_CODE_NO_DATA = 8
    MODE_CODE_BCAST_NO_DATA = 9
    MODE_CODE_BCAST_DATA = 10
    SPURIOUS_DATA = 11


@unique
class TimestampFormat(IntEnum):
    """Timestamp encoding format used in the MIE binary file.

    Values:
        AUTO: Auto-detect from the first records (bounded multi-record probe).
        IRIG: 48-bit IRIG-B timestamp (3 × 16-bit words).
        STANDARD: 32-bit free-running counter (2 × 16-bit words).
    """

    AUTO = 0
    IRIG = 1
    STANDARD = 2


@unique
class ErrorMode(IntEnum):
    """Controls how errored messages are handled in output.

    Values:
        SEPARATE: Errored and spurious messages are written to a
            separate ``<output>_errors.csv`` file. The main CSV
            contains only clean messages. This is the default.
        INLINE: Errored and spurious messages are included in the
            main CSV with ERROR and ERROR_CODE columns populated.
    """

    SEPARATE = 0
    INLINE = 1


# ── DDC Hardware Error Codes ───────────────────────────────────────────
# These codes are written by the DDC recording card into the Error Word
# appended to errored records. The Error Word describes the 1553 bus
# word immediately preceding it.

#: Manchester encoding error, parity error, or incorrect bit count.
ERROR_MANCHESTER_PARITY: Final[int] = 0x011E

#: No status word response from RT, or too few data words received.
ERROR_NO_RESPONSE: Final[int] = 0x0120

#: Inverted sync pattern detected on a data word.
ERROR_INVERTED_SYNC: Final[int] = 0x0136

#: More data words received than the Command Word specified.
ERROR_TOO_MANY_WORDS: Final[int] = 0x0140

#: Unknown or undocumented DDC error condition.
ERROR_UNKNOWN_DDC: Final[int] = 0x0150

#: Set of all known DDC hardware error codes.
KNOWN_DDC_ERROR_CODES: frozenset[int] = frozenset({
    ERROR_MANCHESTER_PARITY,
    ERROR_NO_RESPONSE,
    ERROR_INVERTED_SYNC,
    ERROR_TOO_MANY_WORDS,
    ERROR_UNKNOWN_DDC,
})

#: Human-readable descriptions for DDC error codes.
DDC_ERROR_DESCRIPTIONS: dict[int, str] = {
    ERROR_MANCHESTER_PARITY: "Manchester/Parity Error or Bit Count Error",
    ERROR_NO_RESPONSE: "No Status Response or Too Few Data Words",
    ERROR_INVERTED_SYNC: "Inverted Sync on Data Word",
    ERROR_TOO_MANY_WORDS: "Too Many Data Words",
    ERROR_UNKNOWN_DDC: "Unknown DDC Error",
}

# ── MIE-Decoder Custom Error Codes ────────────────────────────────────
# These codes are assigned by the decoder (not the hardware) to identify
# spurious data records. The 0x20 prefix mirrors the SPURIOUS_DATA type
# code from Type Word bits 0–6.

#: Spurious data that is a continuation of a preceding errored message.
ERROR_SPURIOUS_CONTINUATION: Final[int] = 0x2000

#: Standalone spurious data with no preceding error record.
ERROR_SPURIOUS_STANDALONE: Final[int] = 0x2001

#: Set of all known MIE-Decoder custom error codes.
KNOWN_CUSTOM_ERROR_CODES: frozenset[int] = frozenset({
    ERROR_SPURIOUS_CONTINUATION,
    ERROR_SPURIOUS_STANDALONE,
})

#: All known error codes (DDC + custom).
ALL_KNOWN_ERROR_CODES: frozenset[int] = KNOWN_DDC_ERROR_CODES | KNOWN_CUSTOM_ERROR_CODES


@dataclass(frozen=True, slots=True)
class IrigTimestamp:
    """IRIG-format timestamp decoded from a 3-word binary field.

    Attributes:
        day: Day of year (1-366).
        hour: Hour of day (0-23).
        minute: Minute of hour (0-59).
        second: Second of minute (0-59).
        microsecond: Microsecond within the second (0-999999).
        freerun: True if external IRIG source was unavailable.
    """

    day: int
    hour: int
    minute: int
    second: int
    microsecond: int
    freerun: bool

    def to_total_microseconds(self) -> int:
        """Convert to absolute microseconds from start of year."""
        return (
            (self.day * 86_400 + self.hour * 3_600 + self.minute * 60 + self.second)
            * 1_000_000
            + self.microsecond
        )

    def to_microseconds(self, standard_tick_rate_hz: float | None = None) -> int | None:
        """Absolute microseconds from a known epoch.

        Always returns the IRIG conversion. Defined with the same
        signature as :meth:`StandardTimestamp.to_microseconds` so the
        reader can call ``timestamp.to_microseconds(rate)`` without a
        type check; ``standard_tick_rate_hz`` is accepted and ignored
        here because IRIG already has an absolute microsecond basis.
        """
        return self.to_total_microseconds()

    def format(self) -> str:
        """Format as ``DAY:HH:MM:SS.uuuuuu`` string.

        Per L2-DEC-014 the microsecond field SHALL be exactly six
        digits. Validation in :mod:`mie_decoder.sync` should reject
        any record with ``microsecond >= 1_000_000`` (L2-SYN-004), so
        the modulo here is a defensive belt-and-suspenders: a caller
        constructing an :class:`IrigTimestamp` directly with an out-of-
        range microsecond still gets a well-formed string.
        """
        micro = self.microsecond % 1_000_000
        return (
            f"{self.day}:{self.hour:02d}:{self.minute:02d}"
            f":{self.second:02d}.{micro:06d}"
        )


@dataclass(frozen=True, slots=True)
class StandardTimestamp:
    """Standard-format timestamp decoded from a 2-word binary field.

    Attributes:
        raw_value: The full 32-bit counter value.
        upper_word: Raw upper 16-bit word (bits [31:16]).
        lower_word: Raw lower 16-bit word (bits [15:0]).
    """

    raw_value: int
    upper_word: int
    lower_word: int

    def raw_ticks(self) -> int:
        """Raw 32-bit free-running counter value, in unknown tick units.

        The tick rate is card-dependent and not encoded in the file, so
        callers cannot convert this to seconds without external calibration.
        """
        return self.raw_value

    def to_microseconds(self, standard_tick_rate_hz: float | None = None) -> int | None:
        """Convert raw counter ticks to microseconds, if calibrated.

        ``standard_tick_rate_hz`` is the card-dependent counter frequency
        in Hz, supplied out-of-band (the file does not encode it). Returns
        ``None`` unless the rate is finite and strictly positive, so an
        uncalibrated or invalid rate can never be mistaken for real timing
        — callers (DELTA computation) treat the absence explicitly rather
        than using raw ticks as if they were microseconds.

        Rounding is half-away-from-zero (``int(x + 0.5)``); ticks are
        non-negative so this matches the Rust implementation's
        ``f64::round`` exactly (see L2-DEC-017).
        """
        if (
            standard_tick_rate_hz is None
            or not math.isfinite(standard_tick_rate_hz)
            or standard_tick_rate_hz <= 0.0
        ):
            return None
        micros = self.raw_value * 1_000_000 / standard_tick_rate_hz
        return int(micros + 0.5)

    def format(self) -> str:
        """Format as ``0xNNNNNNNN`` hexadecimal string."""
        return f"0x{self.raw_value:08X}"


#: Union type for timestamps.
Timestamp = IrigTimestamp | StandardTimestamp

#: Number of 16-bit words consumed by each timestamp format.
TIMESTAMP_WORD_COUNTS: dict[TimestampFormat, int] = {
    TimestampFormat.IRIG: 3,
    TimestampFormat.STANDARD: 2,
}


@dataclass(frozen=True, slots=True)
class TypeWord:
    """Decoded DDC MIE record Type Word.

    Attributes:
        message_type: DDC message type code (0x01–0x20).
        bus: Which redundant 1553 bus this message was captured on.
        word_count: Total record size in 16-bit words.
        error: True if the recording card flagged an error (bit 14).
        raw: The original 16-bit value for round-trip fidelity.
    """

    message_type: int
    bus: Bus
    word_count: int
    error: bool
    raw: int


@dataclass(frozen=True, slots=True)
class CommandWord:
    """Decoded MIL-STD-1553 Command Word.

    Attributes:
        rt: Remote Terminal address (0-30; 31 = broadcast).
        direction: TRANSMIT or RECEIVE.
        subaddress: Subaddress (0-31; 0 and 31 are mode codes).
        data_word_count: Number of data words (1-32; raw 0 = 32).
        raw: The original 16-bit value for round-trip fidelity.
    """

    rt: int
    direction: Direction
    subaddress: int
    data_word_count: int
    raw: int

    @property
    def is_broadcast(self) -> bool:
        """True if this command targets all RTs (RT address 31)."""
        return self.rt == 31

    @property
    def is_mode_code(self) -> bool:
        """True if this command is a mode code (SA 0 or SA 31)."""
        return self.subaddress in (0, 31)


@dataclass(frozen=True, slots=True)
class MieMessage:
    """A single decoded MIL-STD-1553 message from an MIE binary file.

    This is the primary output structure of the decoder. Each instance
    represents one complete bus transaction as captured by the DDC
    recording card.

    The structure accommodates all 10 standard message formats plus
    SPURIOUS_DATA and errored records:

    - For simple BC→RT and RT→BC transfers, ``command_word`` and
      ``status_word`` are populated.
    - For RT-to-RT, ``command_word_2`` and ``status_word_2`` are also
      populated.
    - For broadcast messages, ``status_word`` is None.
    - For mode codes, ``data_words`` contains 0 or 1 words.
    - For **errored records** (Type Word bit 14 set), ``error_word``
      contains the DDC error code (0x01xx), data_words contains only
      the words received before the error, and status_word is typically
      None (transaction interrupted).
    - For **SPURIOUS_DATA** (type 0x20), ``command_word`` is None,
      ``error_word`` contains a custom 0x20xx code, and data_words
      contains the raw bus words.

    Attributes:
        timestamp: IRIG or Standard timestamp.
        type_word: Decoded Type Word containing message metadata.
        message_format: Classified message format.
        command_word: Primary Command Word. None for SPURIOUS_DATA
            records which have no command structure.
        command_word_2: Second Command Word for RT-to-RT. None otherwise.
        status_word: Primary Status Word. None for broadcast formats
            and errored records where the RT never responded.
        status_word_2: Second Status Word for RT-to-RT. None otherwise.
        data_words: Tuple of raw 16-bit data words in bus wire order.
            For errored records, contains only words received before
            the error. For SPURIOUS_DATA, contains raw bus words.
        error_word: DDC error code (0x01xx) for errored records, or
            custom decoder code (0x20xx) for SPURIOUS_DATA records.
            None for normal messages.
        delta: Seconds since prior message with same RT+MSG.
            ``0.0`` on first occurrence of an RT/MSG key with a calibrated
            timestamp. A positive float for a non-negative gap. ``None``
            when no DELTA is meaningful: SPURIOUS_DATA (no RT/MSG key),
            uncalibrated Standard timestamps (no known tick rate), and
            non-monotonic timestamps.
        file_offset: Byte offset of this record in the source file.
        mux: MUX column value derived from the source file name (L2-WRT-020),
            or None when MUX population is disabled or the configured filename
            field is absent. Shared (one str per input file) so per-record
            carry stays O(1) in resident memory.
    """

    timestamp: Timestamp
    type_word: TypeWord
    message_format: MessageFormat
    command_word: CommandWord | None
    command_word_2: CommandWord | None
    status_word: int | None
    status_word_2: int | None
    data_words: tuple[int, ...]
    error_word: int | None
    delta: float | None
    file_offset: int
    mux: str | None = None

    @property
    def rt(self) -> int | None:
        """Remote Terminal address, or None for SPURIOUS_DATA."""
        return self.command_word.rt if self.command_word is not None else None

    @property
    def bus(self) -> Bus:
        """Bus identifier shortcut."""
        return self.type_word.bus

    @property
    def msg_label(self) -> str:
        """Message label in ``<SA><T|R>`` format, or empty for SPURIOUS_DATA."""
        if self.command_word is None:
            return ""
        suffix = "T" if self.command_word.direction == Direction.TRANSMIT else "R"
        return f"{self.command_word.subaddress}{suffix}"

    @property
    def delta_key(self) -> str:
        """Unique key for per-RT/MSG delta tracking.

        Returns empty string for SPURIOUS_DATA (no RT/MSG to track).
        """
        if self.command_word is None:
            return ""
        return f"{self.rt}:{self.msg_label}"

    @property
    def is_error(self) -> bool:
        """True if this record has the error flag set (bit 14)."""
        return self.type_word.error

    @property
    def is_spurious(self) -> bool:
        """True if this is a SPURIOUS_DATA record (type 0x20)."""
        return self.message_format == MessageFormat.SPURIOUS_DATA

    @property
    def error_label(self) -> str:
        """Error classification label for CSV output.

        Returns:
            ``""`` for normal messages, ``"ERROR"`` for errored records
            (bit 14 set), ``"SPURIOUS"`` for spurious data records.
        """
        if self.type_word.error:
            return "ERROR"
        if self.is_spurious:
            return "SPURIOUS"
        return ""
