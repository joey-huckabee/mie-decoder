"""Shared test fixtures for MIE-Decoder test suite.

Provides binary test data derived from empirically validated DDC MIE
recordings. All expected values have been cross-referenced against
vendor-generated CSV output.
"""

from __future__ import annotations

from pathlib import Path

import pytest

# Known-good record: RT15 SA11 Receive, Bus A, 30 data words
# From validated CSV row:
#   10:15:54:50.456225,15,11R,0400,...,C771,,,7800,797E,...,A,0.000000
RECORD_RT15_SA11_RCV = bytes.fromhex(
    "02240F1826DB21F6"  # TypeWord + Timestamp
    "7E79"  # Command Word (0x797E LE)
    "0004"  # WD01: 0x0400
    "0000"  # WD02: 0x0000
    "0000"  # WD03: 0x0000
    "2F00"  # WD04: 0x002F
    "22CA"  # WD05: 0xCA22
    "2F00"  # WD06: 0x002F
    "22CA"  # WD07: 0xCA22
    "0000"  # WD08
    "0000"  # WD09
    "0000"  # WD10
    "0000"  # WD11
    "0000"  # WD12
    "0000"  # WD13
    "0000"  # WD14
    "0000"  # WD15
    "0000"  # WD16
    "0000"  # WD17
    "0000"  # WD18
    "0000"  # WD19
    "0000"  # WD20
    "0000"  # WD21
    "0000"  # WD22
    "0000"  # WD23
    "0000"  # WD24
    "0000"  # WD25
    "0000"  # WD26
    "0000"  # WD27
    "0000"  # WD28
    "0000"  # WD29
    "71C7"  # WD30: 0xC771
    "0078"  # Status Word: 0x7800
)

# Known-good record: RT15 SA22 Receive, Bus A, 11 data words
RECORD_RT15_SA22_RCV = bytes.fromhex(
    "02110F1826DB38F7"  # TypeWord + Timestamp
    "CB7A"  # Command Word (0x7ACB LE)
    "0010"  # WD01: 0x1000
    "0000"  # WD02
    "0700"  # WD03: 0x0007
    "0008"  # WD04: 0x0800
    "0000"  # WD05
    "0000"  # WD06
    "0000"  # WD07
    "0000"  # WD08
    "0000"  # WD09
    "C880"  # WD10: 0x80C8
    "E803"  # WD11: 0x03E8
    "0078"  # Status Word: 0x7800
)

# Known-good record: RT15 SA22 Transmit, Bus A, 30 data words
RECORD_RT15_SA22_XMT = bytes.fromhex(
    "04240F1826DBE3F9"  # TypeWord + Timestamp
    "DE7E"  # Command Word (0x7EDE LE)
    "0078"  # Status Word: 0x7800
    "2010"  # WD01: 0x1020
    "8241"  # WD02: 0x4182
    "0000"  # WD03
    "0815"  # WD04: 0x1508
    "0000"  # WD05
    "0000"  # WD06
    "0000"  # WD07
    "0000"  # WD08
    "00FE"  # WD09: 0xFE00
    "0000"  # WD10
    "0000"  # WD11
    "0000"  # WD12
    "0000"  # WD13
    "0000"  # WD14
    "0000"  # WD15
    "0000"  # WD16
    "0000"  # WD17
    "0000"  # WD18
    "0300"  # WD19: 0x0003
    "0000"  # WD20
    "0000"  # WD21
    "0000"  # WD22
    "0000"  # WD23
    "0000"  # WD24
    "0000"  # WD25
    "0020"  # WD26: 0x2000
    "0000"  # WD27
    "0000"  # WD28
    "0000"  # WD29
    "0000"  # WD30
)

# Known-good Bus B record: RT15 SA10 Transmit, Bus B, 30 data words
RECORD_RT15_SA10_XMT_BUSB = bytes.fromhex(
    "84240F18AADA03835E7D"  # TypeWord + Timestamp + CmdWord
    "0078"  # Status Word: 0x7800
    "0305"  # WD01: 0x0503
    "0000"  # WD02
    "0000"  # WD03
    "DE0E"  # WD04: 0x0EDE
    "0000"  # WD05
    "0080"  # WD06: 0x8000
    "0000"  # WD07
    "0000"  # WD08
    "8800"  # WD09: 0x0088
    "7300"  # WD10: 0x0073
    "7300"  # WD11: 0x0073
    "7300"  # WD12: 0x0073
    "7380"  # WD13: 0x8073  -- wait, LE: 80 73 → 0x7380
    "7380"  # WD14: 0x7380
    "0000"  # WD15
    "0000"  # WD16
    "0000"  # WD17
    "0000"  # WD18
    "0000"  # WD19
    "0000"  # WD20
    "0000"  # WD21
    "0000"  # WD22
    "0000"  # WD23
    "0000"  # WD24
    "0000"  # WD25
    "0000"  # WD26
    "0000"  # WD27
    "0000"  # WD28
    "0000"  # WD29
    "8FE8"  # WD30: 0xE88F
)


# ── Synthetic record builders ────────────────────────────────────────
#
# Used by tests that need to exercise DELTA edge cases, errored-record
# decoding, and recovery anchors with timestamps the canonical fixtures
# above don't cover. All builders produce IRIG-format records with
# day=192, hour=15, minute=54, second=50 so they pair cleanly with
# RECORD_RT15_SA11_RCV (whose timestamp matches) for look-ahead.


def _irig_timestamp_bytes(microseconds: int) -> bytes:
    """3-word IRIG timestamp LE bytes for day=192, hour=15, min=54, sec=50."""
    upper = ((0 << 15) | (192 << 5) | 15) & 0xFFFF
    middle = (54 << 10) | (50 << 4) | ((microseconds >> 16) & 0xF)
    lower = microseconds & 0xFFFF
    return upper.to_bytes(2, "little") + middle.to_bytes(2, "little") + lower.to_bytes(2, "little")


def normal_record_rt15_sa11_us(microseconds: int) -> bytes:
    """Build a normal RT15 SA11 Receive record with the given microsecond.

    Reuses every field of RECORD_RT15_SA11_RCV except the timestamp, so the
    resulting bytes are byte-identical apart from the IRIG timestamp triple.
    """
    return RECORD_RT15_SA11_RCV[:2] + _irig_timestamp_bytes(microseconds) + RECORD_RT15_SA11_RCV[8:]


def errored_record_rt15_sa11_us(microseconds: int) -> bytes:
    """Build an errored RT15 SA11 Receive record (Type Word bit 14 set).

    Layout: TypeWord + IRIG timestamp + CmdWord + 2 data words + Error Word.
    Total wc = 8 = 16 bytes. Error code is 0x011E (Manchester/Parity).
    """
    type_raw = 0x02 | (8 << 8) | (1 << 14)
    return (
        type_raw.to_bytes(2, "little")
        + _irig_timestamp_bytes(microseconds)
        + (0x797E).to_bytes(2, "little")  # RT15, R, SA11, dwc=30
        + b"\x00\x00\x00\x00"  # 2 data words
        + (0x011E).to_bytes(2, "little")  # Error Word
    )


def spurious_record_us(microseconds: int, data_word: int = 0x0000) -> bytes:
    """Build a SPURIOUS_DATA record (message type 0x20, no Command Word).

    Layout: TypeWord + IRIG timestamp + 1 data word. Total wc = 5 = 10 bytes.
    """
    type_raw = 0x20 | (5 << 8)
    return (
        type_raw.to_bytes(2, "little")
        + _irig_timestamp_bytes(microseconds)
        + data_word.to_bytes(2, "little")
    )


@pytest.fixture
def single_receive_record() -> bytes:
    """A single RT15 SA11 Receive record (72 bytes)."""
    return RECORD_RT15_SA11_RCV


@pytest.fixture
def multi_record_data() -> bytes:
    """Three consecutive records of different types."""
    return RECORD_RT15_SA11_RCV + RECORD_RT15_SA22_RCV + RECORD_RT15_SA22_XMT


@pytest.fixture
def tmp_mie_file(tmp_path: Path, multi_record_data: bytes) -> Path:
    """Write multi-record test data to a temporary .mie file."""
    fpath = tmp_path / "test.mie"
    fpath.write_bytes(multi_record_data)
    return fpath


@pytest.fixture
def tmp_busb_file(tmp_path: Path) -> Path:
    """Write a Bus B record to a temporary .mie file."""
    fpath = tmp_path / "busb.mie"
    fpath.write_bytes(RECORD_RT15_SA10_XMT_BUSB)
    return fpath
