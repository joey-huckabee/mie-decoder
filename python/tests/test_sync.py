"""Tests for mie_decoder.sync module.

Tests cover header detection, record validation, sync loss recovery,
and interaction with the reader.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from mie_decoder.models import TimestampFormat
from mie_decoder.sync import find_first_record, recover_sync, validate_record


class TestValidateRecord:
    """Tests for validate_record."""

    def test_valid_irig_record(self, single_receive_record: bytes) -> None:
        """Known-good IRIG receive record should validate."""
        # Append a second record for look-ahead
        data = single_receive_record * 2
        assert validate_record(data, 0, len(data), TimestampFormat.IRIG) is True

    def test_invalid_type_at_offset(self) -> None:
        """Random data should not validate."""
        data = b"\xFF\xFF" * 40
        assert validate_record(data, 0, len(data), TimestampFormat.IRIG) is False

    def test_too_short(self) -> None:
        """Data shorter than Type Word should not validate."""
        assert validate_record(b"\x02", 0, 1, TimestampFormat.IRIG) is False

    def test_zero_word_count(self) -> None:
        """Type Word with zero word count should fail."""
        data = b"\x02\x00" + b"\x00" * 20
        assert validate_record(data, 0, len(data), TimestampFormat.IRIG) is False


class TestFindFirstRecord:
    """Tests for find_first_record (header detection)."""

    def test_no_header(self, single_receive_record: bytes) -> None:
        """File starting directly with records should find offset 0."""
        data = single_receive_record * 2
        offset = find_first_record(data, len(data), TimestampFormat.IRIG)
        assert offset == 0

    def test_with_header(self, single_receive_record: bytes) -> None:
        """File with a header before records should skip the header."""
        header = b"\x00" * 20  # 20 bytes of padding
        data = header + single_receive_record * 2
        offset = find_first_record(data, len(data), TimestampFormat.IRIG)
        assert offset == 20

    def test_all_garbage(self) -> None:
        """File with no valid records should return None."""
        data = b"\xFF\xFF" * 100
        offset = find_first_record(data, len(data), TimestampFormat.IRIG)
        assert offset is None

    def test_reader_skips_header(self, tmp_path: Path, single_receive_record: bytes) -> None:
        """MieFileReader should skip headers and decode records after."""
        from mie_decoder.reader import MieFileReader

        header = b"\x00" * 20
        fpath = tmp_path / "headed.mie"
        fpath.write_bytes(header + single_receive_record * 2)
        messages = list(MieFileReader(fpath, time_format=TimestampFormat.IRIG))
        assert len(messages) == 2

    def test_reader_real_header(self, tmp_path: Path, multi_record_data: bytes) -> None:
        """Simulate a file header (non-record data before first record)."""
        from mie_decoder.reader import MieFileReader

        # Build a header using 0xFF bytes (type 0x7F = invalid)
        # so find_first_record will skip past it
        header = b"\xFF\x00" * 36  # 72 bytes, type 0x7F each word
        fpath = tmp_path / "s4_sim.mie"
        fpath.write_bytes(header + multi_record_data)
        messages = list(MieFileReader(fpath, time_format=TimestampFormat.IRIG))
        assert len(messages) == 3


class TestRecoverSync:
    """Tests for recover_sync."""

    def test_recovery_after_garbage(self, single_receive_record: bytes) -> None:
        """Should find a valid record after a gap of garbage."""
        garbage = b"\xFF\xFF" * 10  # 20 bytes of garbage
        data = garbage + single_receive_record * 2
        recovered = recover_sync(data, 0, len(data), TimestampFormat.IRIG)
        assert recovered == 20

    def test_no_recovery_possible(self) -> None:
        """Should return None when no valid record exists."""
        data = b"\xFF\xFF" * 100
        recovered = recover_sync(data, 0, len(data), TimestampFormat.IRIG)
        assert recovered is None

    def test_reader_recovers_from_corruption(
        self, tmp_path: Path, single_receive_record: bytes
    ) -> None:
        """Reader should skip corruption and continue decoding."""
        from mie_decoder.reader import MieFileReader

        # 2 good records, corruption, 2 more good records
        good = single_receive_record * 2
        corruption = b"\xFF\xFF" * 10
        data = good + corruption + single_receive_record * 2
        fpath = tmp_path / "corrupt.mie"
        fpath.write_bytes(data)
        messages = list(MieFileReader(fpath, time_format=TimestampFormat.IRIG))
        # The second pre-corruption record fails look-ahead validation because
        # its next boundary is corrupt. The first and both recovered records
        # remain.
        assert len(messages) == 3

    def test_reader_strict_raises_on_corruption(
        self, tmp_path: Path, single_receive_record: bytes
    ) -> None:
        """Strict mode should raise on sync loss."""
        from mie_decoder.reader import MieFileReader
        from mie_decoder.exceptions import MiePayloadError

        good = single_receive_record * 2
        corruption = b"\x03\x00" * 5  # invalid type 0x03
        data = good + corruption
        fpath = tmp_path / "strict_corrupt.mie"
        fpath.write_bytes(data)
        with pytest.raises(MiePayloadError, match="look-ahead validation"):
            list(MieFileReader(fpath, strict=True, time_format=TimestampFormat.IRIG))
