"""End-to-end tests for MIE file reading and CSV output.

These tests exercise the full pipeline: binary file → MieFileReader →
MieMessage → CSV writer, and validate against known-good CSV rows
from DDC vendor output.
"""

from __future__ import annotations

import csv
import io
from pathlib import Path

import pytest

from mie_decoder.models import Bus, Direction, MessageFormat, TimestampFormat
from mie_decoder.reader import MieFileReader
from mie_decoder.writer import CSV_HEADER, write_csv


class TestMieFileReader:
    """Integration tests for MieFileReader."""

    @pytest.mark.requirement("L2-RDR-015")
    def test_read_three_records(self, tmp_mie_file: Path) -> None:
        """Should decode exactly 3 messages from the multi-record fixture."""
        messages = list(MieFileReader(tmp_mie_file))
        assert len(messages) == 3

    @pytest.mark.requirement("L2-RDR-007")
    def test_first_record_matches_csv(self, tmp_mie_file: Path) -> None:
        """First record should match validated CSV row exactly.

        Expected: 192:15:54:50.456225,15,11R,...,7800,797E,...,A
        """
        msg = list(MieFileReader(tmp_mie_file))[0]
        assert msg.timestamp.format() == "192:15:54:50.456225"
        assert msg.rt == 15
        assert msg.msg_label == "11R"
        assert msg.command_word.raw == 0x797E
        assert msg.status_word == 0x7800
        assert msg.bus == Bus.A
        assert msg.data_words[0] == 0x0400
        assert msg.data_words[3] == 0x002F
        assert msg.data_words[4] == 0xCA22
        assert msg.data_words[29] == 0xC771
        assert len(msg.data_words) == 30
        assert msg.message_format == MessageFormat.RECEIVE
        assert msg.command_word_2 is None
        assert msg.status_word_2 is None
        assert msg.error_word is None
        assert msg.is_error is False
        assert msg.is_spurious is False
        assert msg.error_label == ""

    @pytest.mark.requirement("L2-RDR-007")
    def test_second_record_receive(self, tmp_mie_file: Path) -> None:
        """Second record: RT15 SA22 Receive, 11 data words."""
        msg = list(MieFileReader(tmp_mie_file))[1]
        assert msg.rt == 15
        assert msg.msg_label == "22R"
        assert msg.command_word.subaddress == 22
        assert msg.command_word.direction == Direction.RECEIVE
        assert len(msg.data_words) == 11
        assert msg.status_word == 0x7800

    @pytest.mark.requirement("L2-RDR-008")
    def test_third_record_transmit(self, tmp_mie_file: Path) -> None:
        """Third record: RT15 SA22 Transmit, 30 data words.

        For Transmit, Status Word comes before Data Words in wire order.
        """
        msg = list(MieFileReader(tmp_mie_file))[2]
        assert msg.rt == 15
        assert msg.msg_label == "22T"
        assert msg.command_word.direction == Direction.TRANSMIT
        assert len(msg.data_words) == 30
        assert msg.status_word == 0x7800
        assert msg.data_words[0] == 0x1020
        assert msg.message_format == MessageFormat.TRANSMIT

    @pytest.mark.requirement("L2-MSG-002")
    def test_bus_b_record(self, tmp_busb_file: Path) -> None:
        """Bus B file should decode bus=B correctly."""
        msg = list(MieFileReader(tmp_busb_file))[0]
        assert msg.bus == Bus.B
        assert msg.rt == 15
        assert msg.msg_label == "10T"

    @pytest.mark.requirement("L2-RDR-010")
    def test_delta_first_occurrence_is_zero(self, tmp_mie_file: Path) -> None:
        """First occurrence of any RT/MSG should have delta=0."""
        messages = list(MieFileReader(tmp_mie_file))
        for msg in messages:
            # All three have unique RT/MSG combos, so all should be 0
            assert msg.delta == 0.0

    @pytest.mark.requirement("L2-RDR-009")
    def test_delta_same_rtmsg(self, tmp_path: Path) -> None:
        """Two identical RT/MSG records should produce correct delta."""
        from tests.conftest import RECORD_RT15_SA11_RCV

        # Concatenate same record twice
        fpath = tmp_path / "delta.mie"
        fpath.write_bytes(RECORD_RT15_SA11_RCV * 2)
        messages = list(MieFileReader(fpath))
        assert len(messages) == 2
        assert messages[0].delta == 0.0
        # Same timestamp → delta = 0.0 (not a useful real-world case
        # but validates the calculation path)
        assert messages[1].delta == 0.0

    @pytest.mark.requirement("L2-RDR-005")
    def test_file_not_found(self) -> None:
        """Should raise MieFileNotFoundError for missing files."""
        from mie_decoder.exceptions import MieFileNotFoundError

        with pytest.raises(MieFileNotFoundError):
            MieFileReader("/nonexistent/file.mie")

    @pytest.mark.requirement("L2-RDR-006")
    def test_empty_file(self, tmp_path: Path) -> None:
        """Should raise MieFileEmptyError for empty files."""
        from mie_decoder.exceptions import MieFileEmptyError

        fpath = tmp_path / "empty.mie"
        fpath.write_bytes(b"")
        with pytest.raises(MieFileEmptyError, match="empty"):
            MieFileReader(fpath)

    @pytest.mark.requirement("L2-RDR-002")
    def test_truncated_record(self, tmp_path: Path) -> None:
        """Truncated final record should be silently skipped."""
        from tests.conftest import RECORD_RT15_SA11_RCV

        fpath = tmp_path / "truncated.mie"
        fpath.write_bytes(RECORD_RT15_SA11_RCV + RECORD_RT15_SA11_RCV[:20])
        messages = list(MieFileReader(fpath))
        assert len(messages) == 1

    @pytest.mark.requirement("L2-RDR-003")
    def test_truncated_record_strict(self, tmp_path: Path) -> None:
        """Strict mode should raise MieRecordTruncatedError on truncation."""
        from tests.conftest import RECORD_RT15_SA11_RCV
        from mie_decoder.exceptions import MieRecordTruncatedError

        fpath = tmp_path / "truncated_strict.mie"
        fpath.write_bytes(RECORD_RT15_SA11_RCV + RECORD_RT15_SA11_RCV[:20])
        with pytest.raises(MieRecordTruncatedError):
            list(MieFileReader(fpath, strict=True))

    @pytest.mark.requirement("L2-SYN-016")
    def test_invalid_record_strict(self, tmp_path: Path) -> None:
        """Strict mode should raise on invalid record after good data."""
        from mie_decoder.exceptions import MieDecoderError

        from tests.conftest import RECORD_RT15_SA11_RCV
        bad_record = b"\x03\x00" + b"\x00" * 18  # type 0x03, wc=0
        fpath = tmp_path / "bad_record.mie"
        fpath.write_bytes(RECORD_RT15_SA11_RCV * 2 + bad_record)
        with pytest.raises(MieDecoderError):
            list(MieFileReader(fpath, strict=True, time_format=TimestampFormat.IRIG))

    @pytest.mark.requirement("L2-DEC-009")
    def test_payload_extraction_does_not_overrun_into_next_record(
        self, tmp_path: Path
    ) -> None:
        """L2-DEC-009: payload extraction is bounded by the Type Word's
        declared extent and never consumes bytes from the following record.
        A Command Word declaring more data words than `word_count` can hold
        is rejected by the L2-SYN-022 capacity invariant before extraction,
        and the reader slices to the record extent — so a malformed record
        cannot overrun its successor. Mirrors the Rust
        payload_extraction_does_not_overrun_into_next_record test.
        """
        from mie_decoder.exceptions import MiePayloadError
        from tests.conftest import RECORD_RT15_SA11_RCV

        # R1: Type Word word_count=10 (20 bytes) but Command Word 0x797E
        # declares data_word_count=30. R2: a normal valid record after it.
        r1 = (
            b"\x02\x0a"  # Type: wc=10, type=0x02 (BC->RT), little-endian 0x0A02
            + b"\x0f\x18\x26\xdb\x21\xf6"  # IRIG timestamp (3 words)
            + b"\x7e\x79"  # Cmd 0x797E (RT15 R SA11 dwc=30), little-endian
            + bytes(10)  # 5 payload words -> total 10 words = 20 bytes
        )
        assert len(r1) == 20
        data = r1 + RECORD_RT15_SA11_RCV
        fpath = tmp_path / "overrun.mie"
        fpath.write_bytes(data)

        # Strict: the over-declaration is rejected, not decoded into an overrun.
        with pytest.raises(MiePayloadError):
            list(MieFileReader(fpath, strict=True, time_format=TimestampFormat.IRIG))

        # Lenient: R1 is skipped; R2 decodes intact at its true offset,
        # proving R1 consumed nothing beyond its 20-byte declared extent.
        messages = list(MieFileReader(fpath, time_format=TimestampFormat.IRIG))
        assert len(messages) == 1
        assert messages[0].file_offset == 20
        assert messages[0].command_word is not None
        assert messages[0].command_word.rt == 15

    @pytest.mark.requirement("L2-DEC-002")
    def test_irig_day_of_year_warns_once_per_decode(
        self, tmp_path: Path, caplog: pytest.LogCaptureFixture
    ) -> None:
        """PRA-9: decoding calendar-locked (non-freerun) IRIG records emits a
        one-time advisory about the known day-of-year firmware discrepancy —
        once per decode, not once per record."""
        import logging

        from tests.conftest import RECORD_RT15_SA11_RCV

        fpath = tmp_path / "irig_day.mie"
        fpath.write_bytes(RECORD_RT15_SA11_RCV * 3)  # 3 non-freerun IRIG records
        with caplog.at_level(logging.WARNING, logger="mie_decoder.reader"):
            messages = list(MieFileReader(fpath, time_format=TimestampFormat.IRIG))
        assert len(messages) == 3
        day_warns = [
            r for r in caplog.records if "day-of-year" in r.getMessage()
        ]
        assert len(day_warns) == 1, (
            "day-of-year advisory should fire exactly once per decode, got "
            f"{[w.getMessage() for w in day_warns]}"
        )

    @pytest.mark.requirement("L2-DEC-010")
    def test_file_offset_tracking(self, tmp_mie_file: Path) -> None:
        """Each message should report its byte offset in the file."""
        messages = list(MieFileReader(tmp_mie_file))
        assert messages[0].file_offset == 0
        assert messages[1].file_offset == 72  # 36 words * 2
        assert messages[2].file_offset == 72 + 34  # 36*2 + 17*2

    @pytest.mark.requirement("L2-RDR-004")
    def test_first_record_truncation_strict_raises_distinct_error(
        self, tmp_path: Path
    ) -> None:
        """L2-RDR-004: a file containing a structurally-valid Type Word
        whose declared extent runs past EOF SHALL surface a distinct
        error class in strict mode (MieFirstRecordTruncatedError, NOT
        the generic MieRecordTruncatedError)."""
        from tests.conftest import RECORD_RT15_SA11_RCV
        from mie_decoder.exceptions import (
            MieFirstRecordTruncatedError,
            MieRecordTruncatedError,
        )

        # First 20 bytes of a 72-byte record: Type Word valid (msg_type
        # 0x02, wc=36), but the record needs 72 bytes and only 20 exist.
        fpath = tmp_path / "first_truncated.mie"
        fpath.write_bytes(RECORD_RT15_SA11_RCV[:20])
        with pytest.raises(MieFirstRecordTruncatedError) as exc_info:
            list(MieFileReader(fpath, strict=True))
        # Distinct from the generic MieRecordTruncatedError.
        assert not isinstance(exc_info.value, MieRecordTruncatedError)
        assert exc_info.value.record_bytes == 72
        assert exc_info.value.available_bytes == 20

    @pytest.mark.requirement("L2-DEC-013")
    def test_forced_format_mismatch_strict_raises(self, tmp_path: Path) -> None:
        """L2-DEC-013: forcing the wrong timestamp format on a recording
        the probe is decisive about SHALL raise in strict mode rather than
        silently emit garbage timestamps."""
        from tests.conftest import RECORD_RT15_SA11_RCV
        from mie_decoder.exceptions import MieTimestampFormatMismatchError

        # Two valid IRIG records → the probe is decisive for IRIG.
        fpath = tmp_path / "forced_mismatch.mie"
        fpath.write_bytes(RECORD_RT15_SA11_RCV * 2)
        with pytest.raises(MieTimestampFormatMismatchError):
            list(MieFileReader(fpath, strict=True, time_format=TimestampFormat.STANDARD))

    @pytest.mark.requirement("L2-DEC-013")
    def test_forced_format_mismatch_lenient_proceeds(self, tmp_path: Path) -> None:
        """L2-DEC-013: in lenient mode the same forced-format mismatch
        SHALL log a WARN but proceed with the forced format, not abort."""
        from tests.conftest import RECORD_RT15_SA11_RCV

        fpath = tmp_path / "forced_mismatch_lenient.mie"
        fpath.write_bytes(RECORD_RT15_SA11_RCV * 2)
        # Does not raise; records may be skipped on invariant violations,
        # but the stream completes.
        list(MieFileReader(fpath, time_format=TimestampFormat.STANDARD))

    @pytest.mark.requirement("L2-DEC-013")
    def test_forced_format_matching_is_not_flagged(self, tmp_path: Path) -> None:
        """L2-DEC-013: forcing the format the probe agrees with SHALL NOT
        trip the mismatch check — the records decode normally."""
        from tests.conftest import RECORD_RT15_SA11_RCV

        fpath = tmp_path / "forced_match.mie"
        fpath.write_bytes(RECORD_RT15_SA11_RCV * 2)
        messages = list(
            MieFileReader(fpath, strict=True, time_format=TimestampFormat.IRIG)
        )
        assert len(messages) == 2

    @pytest.mark.requirement("L2-RDR-004")
    def test_first_record_truncation_lenient_terminates_clean(
        self, tmp_path: Path
    ) -> None:
        """L2-RDR-004: lenient mode SHALL terminate cleanly with zero
        records emitted on first-record truncation."""
        from tests.conftest import RECORD_RT15_SA11_RCV

        fpath = tmp_path / "first_truncated_lenient.mie"
        fpath.write_bytes(RECORD_RT15_SA11_RCV[:20])
        messages = list(MieFileReader(fpath))
        assert messages == []


class TestCsvWriter:
    """Integration tests for CSV output."""

    @pytest.mark.requirement("L2-WRT-001")
    @pytest.mark.requirement("L2-WRT-013")
    def test_csv_header(self, tmp_mie_file: Path) -> None:
        """CSV output should start with the correct header row."""
        buf = io.StringIO()
        write_csv(MieFileReader(tmp_mie_file), output=buf)
        buf.seek(0)
        reader = csv.reader(buf)
        header = next(reader)
        assert header == CSV_HEADER

    @pytest.mark.requirement("L2-WRT-001")
    def test_csv_row_count(self, tmp_mie_file: Path) -> None:
        """Should produce one header + 3 data rows."""
        buf = io.StringIO()
        write_csv(MieFileReader(tmp_mie_file), output=buf)
        buf.seek(0)
        lines = buf.getvalue().strip().split("\n")
        assert len(lines) == 4  # header + 3 records

    @pytest.mark.requirement("L2-WRT-003")
    @pytest.mark.requirement("L2-WRT-004")
    @pytest.mark.requirement("L2-WRT-013")
    @pytest.mark.requirement("L2-ERR-007")
    def test_csv_first_row_fields(self, tmp_mie_file: Path) -> None:
        """First CSV data row should match validated vendor output."""
        buf = io.StringIO()
        write_csv(MieFileReader(tmp_mie_file), output=buf)
        buf.seek(0)
        reader = csv.reader(buf)
        next(reader)  # skip header
        row = next(reader)

        assert row[0] == "192:15:54:50.456225"  # TIME_STAMP
        assert row[1] == "15"                    # RT
        assert row[2] == "11R"                   # MSG
        assert row[3] == "0400"                  # WD01
        assert row[35] == "7800"                 # STAT (index 3+32)
        assert row[36] == "797E"                 # CMD
        assert row[39] == "A"                    # BUS
        assert row[40] == "0.000000"             # DELTA
        assert row[41] == ""                     # ERROR (normal message)
        assert row[42] == ""                     # ERROR_CODE (normal message)

    @pytest.mark.requirement("L2-WRT-002")
    def test_csv_data_word_padding(self, tmp_mie_file: Path) -> None:
        """Records with <32 data words should have empty trailing WD cols."""
        buf = io.StringIO()
        write_csv(MieFileReader(tmp_mie_file), output=buf)
        buf.seek(0)
        reader = csv.reader(buf)
        next(reader)  # skip header
        next(reader)  # skip first row (30 words)
        row = next(reader)  # second row: 11 data words
        # WD12 through WD32 (indices 14..34) should be empty
        assert row[14] == ""  # WD12
        assert row[34 - 1] == ""  # WD32

    @pytest.mark.requirement("L2-WRT-012")
    def test_csv_to_file(self, tmp_mie_file: Path, tmp_path: Path) -> None:
        """Writing to a file path should produce a valid CSV file."""
        out = tmp_path / "output.csv"
        outcome = write_csv(MieFileReader(tmp_mie_file), output=out)
        assert out.exists()
        raw = out.read_bytes()
        assert b"\r\n" not in raw
        lines = raw.decode().strip().split("\n")
        assert len(lines) == 4
        assert outcome.normal_count == 3
        assert outcome.partial is None

    @pytest.mark.requirement("L3-PY-012")
    def test_write_csv_returns_count(self, tmp_mie_file: Path) -> None:
        """write_csv should return a WriteOutcome whose normal_count
        matches the number of messages written."""
        buf = io.StringIO()
        outcome = write_csv(MieFileReader(tmp_mie_file), output=buf)
        assert outcome.normal_count == 3
        assert outcome.partial is None


class TestAtomicWriteSafety:
    """L2-WRT-014 through L2-WRT-018 enforcement tests for the Python writer."""

    @pytest.mark.requirement("L2-WRT-014")
    def test_paths_refer_to_same_file_existing(self, tmp_path: Path) -> None:
        from mie_decoder.writer import paths_refer_to_same_file

        p = tmp_path / "x.dat"
        p.write_bytes(b"x")
        assert paths_refer_to_same_file(p, p) is True

    @pytest.mark.requirement("L2-WRT-014")
    def test_paths_refer_to_same_file_distinct(self, tmp_path: Path) -> None:
        from mie_decoder.writer import paths_refer_to_same_file

        a = tmp_path / "a.dat"
        a.write_bytes(b"a")
        b = tmp_path / "b.dat"  # doesn't exist
        assert paths_refer_to_same_file(a, b) is False

    @pytest.mark.requirement("L2-WRT-014")
    def test_write_csv_rejects_input_output_collision(
        self, tmp_mie_file: Path
    ) -> None:
        """L2-WRT-014: refuse to write CSV over the input file."""
        from mie_decoder.exceptions import MieInputOutputCollisionError
        from mie_decoder.writer import WriteOptions

        opts = WriteOptions(input_path=tmp_mie_file, no_clobber=False)
        original_bytes = tmp_mie_file.read_bytes()
        with pytest.raises(MieInputOutputCollisionError) as exc_info:
            write_csv(
                MieFileReader(tmp_mie_file),
                output=tmp_mie_file,
                opts=opts,
            )
        assert str(tmp_mie_file) in str(exc_info.value)
        # Input file MUST be unchanged.
        assert tmp_mie_file.read_bytes() == original_bytes

    @pytest.mark.requirement("L2-WRT-017")
    def test_write_csv_rejects_clobber_with_no_clobber(
        self, tmp_mie_file: Path, tmp_path: Path
    ) -> None:
        """L2-WRT-017: refuse to overwrite an existing destination."""
        from mie_decoder.exceptions import MieClobberRefusedError
        from mie_decoder.writer import WriteOptions

        out = tmp_path / "out.csv"
        out.write_text("EXISTING\n", encoding="utf-8")
        opts = WriteOptions(no_clobber=True)
        with pytest.raises(MieClobberRefusedError):
            write_csv(MieFileReader(tmp_mie_file), output=out, opts=opts)
        # Existing file untouched.
        assert out.read_text(encoding="utf-8") == "EXISTING\n"

    @pytest.mark.requirement("L2-WRT-017")
    def test_write_csv_overwrites_by_default(
        self, tmp_mie_file: Path, tmp_path: Path
    ) -> None:
        """No-clobber off (default): existing destination is replaced."""
        out = tmp_path / "out.csv"
        out.write_text("OLD\n", encoding="utf-8")
        outcome = write_csv(MieFileReader(tmp_mie_file), output=out)
        assert outcome.normal_count == 3
        text = out.read_text(encoding="utf-8")
        assert text.startswith("TIME_STAMP,RT,MSG,")

    @pytest.mark.requirement("L3-WRT-001")
    def test_write_csv_to_file_cleans_up_temp(
        self, tmp_mie_file: Path, tmp_path: Path
    ) -> None:
        """L3-WRT-001: after a successful write, no temp file should remain."""
        out = tmp_path / "out.csv"
        write_csv(MieFileReader(tmp_mie_file), output=out)
        # Temp pattern: <output>.mie-decoder.tmp.<pid>
        leftovers = [
            p for p in tmp_path.iterdir()
            if p.name.startswith("out.csv.mie-decoder.tmp.")
        ]
        assert leftovers == [], f"unexpected temp file(s): {leftovers}"

    @pytest.mark.requirement("L2-WRT-014")
    def test_write_csv_split_rejects_input_output_collision(
        self, tmp_mie_file: Path
    ) -> None:
        from mie_decoder.exceptions import MieInputOutputCollisionError
        from mie_decoder.writer import WriteOptions, write_csv_split

        opts = WriteOptions(input_path=tmp_mie_file, no_clobber=False)
        with pytest.raises(MieInputOutputCollisionError):
            write_csv_split(MieFileReader(tmp_mie_file), output=tmp_mie_file, opts=opts)

    @pytest.mark.requirement("L2-SYN-011")
    @pytest.mark.requirement("L1-EXIT-002")
    def test_no_valid_records_raises(self, tmp_path: Path) -> None:
        """L1-EXIT-002: input with no decodable records raises MieNoValidRecordsError."""
        from mie_decoder.exceptions import MieNoValidRecordsError

        bad = tmp_path / "garbage.bin"
        bad.write_bytes(b"\xff" * 1024)  # 1 KB of 0xFF — no valid Type Word
        reader = MieFileReader(bad)
        with pytest.raises(MieNoValidRecordsError):
            list(reader)

    @pytest.mark.requirement("L2-SYN-011")
    @pytest.mark.requirement("L1-EXIT-004")
    def test_lenient_unrecoverable_sync_loss_raises(
        self, tmp_path: Path
    ) -> None:
        """L1-EXIT-004: lenient-mode mid-file sync loss (not truncation) raises."""
        from mie_decoder.exceptions import MieUnrecoverableSyncLossError
        from tests.conftest import RECORD_RT15_SA11_RCV

        # Two valid records + 70 KB of 0xFF — the second record's
        # look-ahead succeeds (next bytes are 0xFF Type Word — invalid),
        # so recover_sync from offset 72 scans the full 64 KB and finds
        # no valid record → unrecoverable corruption.
        fpath = tmp_path / "corruption.mie"
        fpath.write_bytes(RECORD_RT15_SA11_RCV + RECORD_RT15_SA11_RCV + b"\xff" * 70_000)
        reader = MieFileReader(fpath)
        with pytest.raises(MieUnrecoverableSyncLossError) as exc_info:
            list(reader)
        assert exc_info.value.sync_losses >= 1

    @pytest.mark.requirement("L3-WRT-002")
    @pytest.mark.requirement("L1-EXIT-004")
    def test_write_csv_with_allow_partial_commits_dot_partial(
        self, tmp_path: Path
    ) -> None:
        """allow_partial converts UnrecoverableSyncLoss to a .partial file
        and a non-None WriteOutcome.partial."""
        from mie_decoder.writer import WriteOptions
        from tests.conftest import RECORD_RT15_SA11_RCV

        fpath = tmp_path / "corruption.mie"
        fpath.write_bytes(RECORD_RT15_SA11_RCV + RECORD_RT15_SA11_RCV + b"\xff" * 70_000)
        out = tmp_path / "out.csv"
        opts = WriteOptions(allow_partial=True)
        outcome = write_csv(MieFileReader(fpath), output=out, opts=opts)
        assert outcome.partial is not None
        assert outcome.partial.sync_losses >= 1
        partial_path = outcome.partial.main_path
        assert partial_path.exists()
        assert partial_path.name == "out.csv.partial"
        # Main destination must NOT exist.
        assert not out.exists()
        # The partial should contain the records that decoded
        # successfully before the sync loss.
        body = partial_path.read_text(encoding="utf-8")
        assert body.startswith("TIME_STAMP,RT,MSG,")
        assert "11R" in body

    @pytest.mark.requirement("L2-DEC-015")
    def test_cli_detect_records_flag_accepts_valid_size(
        self, tmp_mie_file: Path, tmp_path: Path
    ) -> None:
        """--detect-records N accepts a value in [1, 32] and decodes
        normally on a valid fixture."""
        from mie_decoder.cli import main

        out = tmp_path / "out.csv"
        rc = main([
            "decode",
            str(tmp_mie_file),
            "-o",
            str(out),
            "--detect-records",
            "2",
        ])
        assert rc == 0
        assert out.exists()

    @pytest.mark.requirement("L2-DEC-015")
    def test_cli_detect_records_flag_rejects_out_of_range(
        self, tmp_mie_file: Path, tmp_path: Path, capsys: pytest.CaptureFixture[str]
    ) -> None:
        """--detect-records above the max (32) is rejected at parse
        time with a non-zero exit and the offending value in
        stderr."""
        from mie_decoder.cli import main

        out = tmp_path / "out.csv"
        rc = main([
            "decode",
            str(tmp_mie_file),
            "-o",
            str(out),
            "--detect-records",
            "999",
        ])
        assert rc != 0
        captured = capsys.readouterr()
        assert "--detect-records" in captured.err and "999" in captured.err

    @pytest.mark.requirement("L2-CLI-012")
    @pytest.mark.requirement("L2-DEC-017")
    @pytest.mark.requirement("L2-CLI-011")
    @pytest.mark.requirement("L1-EXIT-007")
    def test_cli_standard_tick_rate_hz_flag_rejects_nonpositive(
        self, tmp_mie_file: Path, tmp_path: Path, capsys: pytest.CaptureFixture[str]
    ) -> None:
        """--standard-tick-rate-hz <= 0 is a CLI usage error: exit 4
        (L2-CLI-011/L2-CLI-012) with the offending flag in stderr."""
        from mie_decoder.cli import main

        out = tmp_path / "out.csv"
        rc = main([
            "decode",
            str(tmp_mie_file),
            "-o",
            str(out),
            "--standard-tick-rate-hz",
            "0",
        ])
        assert rc == 4
        captured = capsys.readouterr()
        assert "--standard-tick-rate-hz" in captured.err

    @pytest.mark.requirement("L2-CLI-012")
    @pytest.mark.requirement("L2-DEC-017")
    @pytest.mark.requirement("L2-RDR-019")
    def test_cli_standard_tick_rate_hz_enables_delta(
        self, tmp_path: Path, capsys: pytest.CaptureFixture[str]
    ) -> None:
        """With --standard-tick-rate-hz set, Standard-timestamp records
        get a non-empty DELTA; without it, DELTA stays empty. Uses the
        shared conformance fixture so the bytes match what CI validates.

        The two records are 16 ticks apart (100000 -> 100016); at 1 MHz
        that is 0.000016 s.
        """
        import csv as _csv

        from mie_decoder.cli import main

        fixture = (
            Path(__file__).resolve().parents[2]
            / "tests" / "conformance" / "inputs" / "standard-timestamps.hex"
        )
        # Same hex-fixture parse as tests/conformance/run.py:read_hex —
        # strip per-line `#` comments, join, decode (fromhex ignores
        # whitespace).
        hex_text = "".join(
            line.split("#", 1)[0] for line in fixture.read_text().splitlines()
        )
        data = bytes.fromhex(hex_text)
        rec = tmp_path / "standard.mie"
        rec.write_bytes(data)

        # Calibrated: DELTA populated.
        out = tmp_path / "calibrated.csv"
        rc = main([
            "decode", str(rec), "-o", str(out),
            "--time-format", "standard",
            "--standard-tick-rate-hz", "1000000",
        ])
        assert rc == 0
        rows = list(_csv.DictReader(out.read_text().splitlines()))
        assert rows[0]["DELTA"] == "0.000000"
        assert rows[1]["DELTA"] == "0.000016"

        # Uncalibrated: DELTA empty (unchanged historical behavior).
        out2 = tmp_path / "uncalibrated.csv"
        rc = main([
            "decode", str(rec), "-o", str(out2),
            "--time-format", "standard",
        ])
        assert rc == 0
        rows2 = list(_csv.DictReader(out2.read_text().splitlines()))
        assert rows2[0]["DELTA"] == ""
        assert rows2[1]["DELTA"] == ""

    @pytest.mark.requirement("L2-CLI-011")
    @pytest.mark.requirement("L1-EXIT-002")
    def test_cli_no_valid_records_returns_exit_2(self, tmp_path: Path) -> None:
        """CLI maps MieNoValidRecordsError to exit code 2 (L1-EXIT-002)."""
        from mie_decoder.cli import main

        bad = tmp_path / "garbage.bin"
        bad.write_bytes(b"\xff" * 1024)
        rc = main(["decode", str(bad), "-o", str(tmp_path / "out.csv")])
        assert rc == 2
        # No output file should have been created.
        assert not (tmp_path / "out.csv").exists()

    @pytest.mark.requirement("L2-CLI-011")
    @pytest.mark.requirement("L1-EXIT-004")
    def test_cli_unrecoverable_default_returns_exit_3(
        self, tmp_path: Path
    ) -> None:
        """CLI maps MieUnrecoverableSyncLossError to exit code 3 (L1-EXIT-004)."""
        from mie_decoder.cli import main
        from tests.conftest import RECORD_RT15_SA11_RCV

        fpath = tmp_path / "corruption.mie"
        fpath.write_bytes(RECORD_RT15_SA11_RCV + RECORD_RT15_SA11_RCV + b"\xff" * 70_000)
        out = tmp_path / "out.csv"
        rc = main(["decode", str(fpath), "-o", str(out)])
        assert rc == 3
        # No main output and no .partial under default behavior.
        assert not out.exists()
        assert not (tmp_path / "out.csv.partial").exists()

    @pytest.mark.requirement("L2-CLI-011")
    @pytest.mark.requirement("L1-EXIT-004")
    def test_cli_unrecoverable_allow_partial_returns_exit_0(
        self, tmp_path: Path
    ) -> None:
        """--allow-partial converts exit 3 into exit 0 + .partial file."""
        from mie_decoder.cli import main
        from tests.conftest import RECORD_RT15_SA11_RCV

        fpath = tmp_path / "corruption.mie"
        fpath.write_bytes(RECORD_RT15_SA11_RCV + RECORD_RT15_SA11_RCV + b"\xff" * 70_000)
        out = tmp_path / "out.csv"
        rc = main(["decode", str(fpath), "-o", str(out), "--allow-partial"])
        assert rc == 0
        assert (tmp_path / "out.csv.partial").exists()
        assert not out.exists()

    @pytest.mark.requirement("L2-WRT-017")
    def test_write_csv_split_no_clobber_checks_errors_file(
        self, tmp_mie_file: Path, tmp_path: Path
    ) -> None:
        """no_clobber must also refuse if the errors-file destination exists."""
        from mie_decoder.exceptions import MieClobberRefusedError
        from mie_decoder.writer import WriteOptions, write_csv_split

        out = tmp_path / "out.csv"
        err = tmp_path / "out_errors.csv"
        err.write_text("OLD ERRORS\n", encoding="utf-8")
        opts = WriteOptions(no_clobber=True)
        with pytest.raises(MieClobberRefusedError) as exc_info:
            write_csv_split(MieFileReader(tmp_mie_file), output=out, opts=opts)
        assert str(err) in str(exc_info.value)
        # Main destination must not have been created.
        assert not out.exists()
        # Errors file untouched.
        assert err.read_text(encoding="utf-8") == "OLD ERRORS\n"


class TestCliEndToEnd:
    """End-to-end CLI tests."""

    @pytest.mark.requirement("L1-EXIT-005")
    def test_cli_emits_exit_class_summary_on_complete_decode(
        self,
        tmp_mie_file: Path,
        tmp_path: Path,
        caplog: pytest.LogCaptureFixture,
    ) -> None:
        """L1-EXIT-005: decode SHALL log a one-line exit-class summary
        naming one of {complete, partial-recovered, partial-unrecoverable,
        no-records}. This case exercises the `complete` branch."""
        from mie_decoder.cli import main
        import logging

        out = tmp_path / "summary.csv"
        with caplog.at_level(logging.INFO, logger="mie_decoder.cli"):
            rc = main(["--log-level", "INFO", "decode", str(tmp_mie_file), "-o", str(out)])
        assert rc == 0
        summary_lines = [
            r.getMessage() for r in caplog.records
            if "decode exit class:" in r.getMessage()
        ]
        assert summary_lines, (
            "expected at least one `decode exit class:` summary line; "
            f"got {[r.getMessage() for r in caplog.records]}"
        )
        assert any("complete" in line for line in summary_lines), (
            f"expected `complete` in summary; got {summary_lines}"
        )

    @pytest.mark.requirement("L1-EXIT-005")
    def test_cli_emits_no_records_exit_class_summary(
        self,
        tmp_path: Path,
        caplog: pytest.LogCaptureFixture,
    ) -> None:
        """L1-EXIT-005: the `no-records` exit-class summary branch."""
        from mie_decoder.cli import main
        import logging

        bad = tmp_path / "garbage.bin"
        bad.write_bytes(b"\xff" * 1024)
        out = tmp_path / "summary.csv"
        with caplog.at_level(logging.INFO, logger="mie_decoder.cli"):
            rc = main(["--log-level", "INFO", "decode", str(bad), "-o", str(out)])
        assert rc == 2
        summary_lines = [
            r.getMessage() for r in caplog.records
            if "decode exit class:" in r.getMessage()
        ]
        assert any("no-records" in line for line in summary_lines), (
            f"expected `no-records` in summary; got {summary_lines}"
        )

    @pytest.mark.requirement("L2-WRT-007")
    def test_cli_decode_stdout(self, tmp_mie_file: Path) -> None:
        """CLI decode should produce CSV on stdout."""
        from mie_decoder.cli import main

        import sys
        buf = io.StringIO()
        old_stdout = sys.stdout
        sys.stdout = buf
        try:
            rc = main(["decode", str(tmp_mie_file)])
        finally:
            sys.stdout = old_stdout
        assert rc == 0
        lines = buf.getvalue().strip().split("\n")
        assert len(lines) == 4

    @pytest.mark.requirement("L2-CLI-002")
    def test_cli_decode_output_file(self, tmp_mie_file: Path, tmp_path: Path) -> None:
        """CLI decode with -o should write to the specified file."""
        from mie_decoder.cli import main

        out = tmp_path / "cli_out.csv"
        rc = main(["decode", str(tmp_mie_file), "-o", str(out)])
        assert rc == 0
        assert out.exists()

    @pytest.mark.requirement("L3-PY-010")
    def test_cli_count_subcommand(self, tmp_mie_file: Path, capsys: pytest.CaptureFixture[str]) -> None:
        """The `count` subcommand prints the integer count to stdout and a
        human-readable status line to stderr."""
        from mie_decoder.cli import main

        rc = main(["count", str(tmp_mie_file)])
        assert rc == 0
        captured = capsys.readouterr()
        assert captured.out.strip() == "3"  # machine-readable datum on stdout
        assert "counted 3 messages in" in captured.err

    @pytest.mark.requirement("L2-CLI-005")
    def test_cli_decode_missing_file(self, capsys: pytest.CaptureFixture[str]) -> None:
        """CLI decode with nonexistent file should return exit code 1."""
        from mie_decoder.cli import main

        rc = main(["decode", "/nonexistent/file.mie"])
        assert rc == 1

    @pytest.mark.requirement("L2-CLI-004")
    def test_cli_log_level_info(self, tmp_mie_file: Path, tmp_path: Path) -> None:
        """CLI --log-level INFO should emit log messages to stderr."""
        from mie_decoder.cli import main

        out = tmp_path / "log_test.csv"
        rc = main(["--log-level", "INFO", "decode", str(tmp_mie_file), "-o", str(out)])
        assert rc == 0
        assert out.exists()

    @pytest.mark.requirement("L2-CLI-004")
    def test_cli_log_level_debug(self, tmp_mie_file: Path, tmp_path: Path) -> None:
        """CLI --log-level DEBUG should succeed without error."""
        from mie_decoder.cli import main

        out = tmp_path / "debug_test.csv"
        rc = main(["--log-level", "DEBUG", "decode", str(tmp_mie_file), "-o", str(out)])
        assert rc == 0

    @pytest.mark.requirement("L2-CFG-003")
    def test_cli_toml_logging_level_is_honored_when_no_cli_override(
        self,
        tmp_mie_file: Path,
        tmp_path: Path,
        caplog: pytest.LogCaptureFixture,
    ) -> None:
        """L2-CFG-003 precedence: TOML [logging] level takes effect when
        no --log-level CLI flag is passed. Regression coverage for the
        bug where the TOML value was parsed into config.log_level but
        the CLI never re-configured the logger after loading."""
        from mie_decoder.cli import main
        import logging

        config_path = tmp_path / "config.toml"
        config_path.write_text('[logging]\nlevel = "INFO"\n', encoding="utf-8")
        out = tmp_path / "decoded.csv"

        with caplog.at_level(logging.INFO, logger="mie_decoder"):
            rc = main([
                "--config", str(config_path),  # global: before subcommand
                "decode",
                str(tmp_mie_file),
                "-o", str(out),
            ])

        assert rc == 0
        # `decode exit class:` is INFO-level; it appears only if the
        # mie_decoder logger is effectively at INFO or finer.
        summary_lines = [
            r.getMessage() for r in caplog.records
            if "decode exit class:" in r.getMessage()
        ]
        assert summary_lines, (
            "expected `decode exit class:` (INFO) line after TOML set "
            "level=INFO; got: "
            f"{[r.getMessage() for r in caplog.records]}"
        )

    @pytest.mark.requirement("L2-CFG-003")
    def test_cli_log_level_overrides_toml_logging_level(
        self,
        tmp_mie_file: Path,
        tmp_path: Path,
        caplog: pytest.LogCaptureFixture,
    ) -> None:
        """L2-CFG-003 precedence: --log-level CLI flag overrides the
        TOML [logging] level (CLI > TOML > default)."""
        from mie_decoder.cli import main
        import logging

        # TOML asks for DEBUG (most verbose); CLI asks for ERROR
        # (suppresses INFO). CLI must win — no `decode exit class:`
        # INFO line should appear.
        config_path = tmp_path / "config.toml"
        config_path.write_text('[logging]\nlevel = "DEBUG"\n', encoding="utf-8")
        out = tmp_path / "decoded.csv"

        with caplog.at_level(logging.DEBUG, logger="mie_decoder"):
            rc = main([
                "--log-level", "ERROR",
                "--config", str(config_path),  # both global: before subcommand
                "decode",
                str(tmp_mie_file),
                "-o", str(out),
            ])

        assert rc == 0
        info_lines = [
            r.getMessage() for r in caplog.records
            if r.levelno < logging.ERROR
        ]
        assert not info_lines, (
            f"CLI --log-level ERROR should suppress all sub-ERROR records "
            f"even when TOML set level=DEBUG; got: {info_lines}"
        )

    @pytest.mark.requirement("L2-CFG-003")
    def test_cli_dump_honors_toml_logging_level(
        self,
        tmp_mie_file: Path,
        tmp_path: Path,
    ) -> None:
        """L2-CFG-003 precedence: TOML [logging] level applies to the
        dump subcommand too (mirrors Rust where --config is global).
        dump.py emits no INFO messages of its own, so the assertion is
        on the effective log level after the run rather than captured
        records."""
        from mie_decoder.cli import main
        import logging
        import sys
        import io

        config_path = tmp_path / "config.toml"
        config_path.write_text('[logging]\nlevel = "DEBUG"\n', encoding="utf-8")

        buf = io.StringIO()
        old_stdout = sys.stdout
        sys.stdout = buf
        try:
            rc = main([
                "--config", str(config_path),  # global: before subcommand
                "dump",
                str(tmp_mie_file),
                "--records", "1",
            ])
        finally:
            sys.stdout = old_stdout

        assert rc == 0
        assert logging.getLogger("mie_decoder").getEffectiveLevel() == logging.DEBUG

    @pytest.mark.requirement("L2-CLI-009")
    def test_cli_dump_records(self, tmp_mie_file: Path, capsys: pytest.CaptureFixture[str]) -> None:
        """CLI dump should print record-aware hex dump to stdout."""
        from mie_decoder.cli import main

        import sys
        buf = io.StringIO()
        old_stdout = sys.stdout
        sys.stdout = buf
        try:
            rc = main(["dump", str(tmp_mie_file), "--records", "2"])
        finally:
            sys.stdout = old_stdout
        assert rc == 0
        output = buf.getvalue()
        assert "Record #0" in output
        assert "Record #1" in output

    @pytest.mark.requirement("L2-CLI-009")
    def test_cli_dump_raw(self, tmp_mie_file: Path) -> None:
        """CLI dump --raw should print raw hex to stdout."""
        from mie_decoder.cli import main

        import sys
        buf = io.StringIO()
        old_stdout = sys.stdout
        sys.stdout = buf
        try:
            rc = main(["dump", str(tmp_mie_file), "--raw", "--length", "32"])
        finally:
            sys.stdout = old_stdout
        assert rc == 0
        output = buf.getvalue()
        assert "00000000" in output

    @pytest.mark.requirement("L2-CLI-009")
    def test_cli_dump_missing_file(self) -> None:
        """CLI dump with nonexistent file should return exit code 1."""
        from mie_decoder.cli import main

        rc = main(["dump", "/nonexistent/file.mie"])
        assert rc == 1

    @pytest.mark.requirement("L2-CLI-005")
    @pytest.mark.requirement("L2-CLI-011")
    @pytest.mark.requirement("L1-EXIT-007")
    def test_cli_no_subcommand(self) -> None:
        """CLI with no subcommand is a usage error: exit 4 (L2-CLI-011)."""
        from mie_decoder.cli import main

        rc = main([])
        assert rc == 4

    @pytest.mark.requirement("L2-CLI-011")
    @pytest.mark.requirement("L1-EXIT-007")
    def test_cli_unknown_flag_is_usage_error(self) -> None:
        """An unknown flag is a usage error: exit 4. argparse defaults to 2,
        which would collide with no-records; the parser remaps it to 4."""
        from mie_decoder.cli import main

        # argparse usage errors raise SystemExit rather than returning.
        with pytest.raises(SystemExit) as exc_info:
            main(["decode", "--no-such-flag", "rec.mie"])
        assert exc_info.value.code == 4

    @pytest.mark.requirement("L2-CLI-011")
    @pytest.mark.requirement("L1-EXIT-008")
    def test_cli_malformed_config_is_config_error(self, tmp_path: Path) -> None:
        """A malformed/invalid config is a configuration error: exit 5
        (distinct from a usage error and from a runtime error)."""
        from mie_decoder.cli import main

        bad = tmp_path / "bad.toml"
        bad.write_text('[decode]\ntime_format = "potato"\n')
        # Config load fails before the input file is opened, so the input
        # path need not exist.
        rc = main(["--config", str(bad), "decode", str(tmp_path / "missing.mie")])
        assert rc == 5


class TestDeltaAndErrorRecords:
    """L2-RDR-016/017/018 and L2-ERR-002/005: DELTA edge cases and
    error/SPURIOUS record decoding. Synthetic records are built via the
    helpers in conftest.py so the fixtures stay reviewable in hex form."""

    @pytest.mark.requirement("L2-RDR-016")
    @pytest.mark.requirement("L2-ERR-002")
    def test_errored_record_participates_in_delta(self, tmp_path: Path) -> None:
        """L2-RDR-016: errored records (Type Word bit 14 set) update the per-
        RT/MSG cursor and SHALL receive a DELTA computed against the prior
        message sharing the same key.

        L2-ERR-002: the final word of an errored record decodes as the DDC
        Error Word.
        """
        from tests.conftest import errored_record_rt15_sa11_us, normal_record_rt15_sa11_us

        normal = normal_record_rt15_sa11_us(456_225)
        errored = errored_record_rt15_sa11_us(456_484)  # +0.000259 s
        anchor = normal_record_rt15_sa11_us(456_500)    # for look-ahead
        fpath = tmp_path / "errored_delta.mie"
        fpath.write_bytes(normal + errored + anchor)
        messages = list(MieFileReader(fpath))
        assert len(messages) == 3
        assert messages[0].delta == 0.0
        assert messages[1].is_error
        assert messages[1].error_word == 0x011E
        assert messages[1].delta is not None
        assert messages[1].delta == pytest.approx(0.000259, abs=1e-6)

    @pytest.mark.requirement("L2-RDR-017")
    def test_non_monotonic_timestamp_warns_once_per_key(
        self, tmp_path: Path, caplog: pytest.LogCaptureFixture,
    ) -> None:
        """L2-RDR-017: when a record's timestamp is older than the prior
        message for the same RT/MSG key, DELTA SHALL be empty and a WARN
        SHALL be logged, at most once per key per file."""
        import logging
        from tests.conftest import normal_record_rt15_sa11_us

        late = normal_record_rt15_sa11_us(500_000)
        early = normal_record_rt15_sa11_us(400_000)
        earlier = normal_record_rt15_sa11_us(350_000)
        fpath = tmp_path / "out_of_order.mie"
        fpath.write_bytes(late + early + earlier)
        with caplog.at_level(logging.WARNING, logger="mie_decoder.reader"):
            messages = list(MieFileReader(fpath))
        assert len(messages) == 3
        assert messages[0].delta == 0.0
        assert messages[1].delta is None
        assert messages[2].delta is None
        warns = [
            r for r in caplog.records
            if "non-monotonic" in r.getMessage().lower()
        ]
        assert len(warns) == 1, (
            f"expected exactly one non-monotonic WARN per key; got "
            f"{[w.getMessage() for w in warns]}"
        )

    @pytest.mark.requirement("L2-RDR-018")
    @pytest.mark.requirement("L2-ERR-005")
    def test_spurious_data_empty_delta_and_continuation_code(
        self, tmp_path: Path,
    ) -> None:
        """L2-RDR-018: SPURIOUS_DATA records have no RT/MSG key, SHALL have
        an empty DELTA, and SHALL NOT update any per-key cursor.

        L2-ERR-005: SPURIOUS_DATA immediately following an errored record
        uses decoder code 0x2000 (continuation).
        """
        from tests.conftest import (
            errored_record_rt15_sa11_us,
            normal_record_rt15_sa11_us,
            spurious_record_us,
        )

        normal = normal_record_rt15_sa11_us(450_000)
        errored = errored_record_rt15_sa11_us(500_000)
        spurious = spurious_record_us(550_000)
        anchor = normal_record_rt15_sa11_us(560_000)
        fpath = tmp_path / "spurious.mie"
        fpath.write_bytes(normal + errored + spurious + anchor)
        messages = list(MieFileReader(fpath))
        assert len(messages) == 4
        # Index 2 is SPURIOUS — empty DELTA, continuation code 0x2000.
        assert messages[2].is_spurious
        assert messages[2].delta is None
        assert messages[2].error_word == 0x2000
        # The SPURIOUS record did NOT update the RT15:11R cursor, so the
        # anchor's DELTA tracks back to the errored record (index 1):
        # (560_000 - 500_000) microseconds = 0.06 seconds.
        assert messages[3].delta is not None
        assert messages[3].delta == pytest.approx(0.06, abs=1e-6)

    @pytest.mark.requirement("L2-SYN-017")
    def test_error_and_spurious_records_pass_validation(
        self, tmp_path: Path,
    ) -> None:
        """L2-SYN-017: valid error records and SPURIOUS_DATA records SHALL
        remain eligible record boundaries during validation and recovery —
        i.e. they pass validate_record like normal records."""
        from tests.conftest import (
            errored_record_rt15_sa11_us,
            normal_record_rt15_sa11_us,
            spurious_record_us,
        )

        normal = normal_record_rt15_sa11_us(450_000)
        errored = errored_record_rt15_sa11_us(500_000)
        spurious = spurious_record_us(550_000)
        anchor = normal_record_rt15_sa11_us(560_000)
        fpath = tmp_path / "err_spurious_validation.mie"
        fpath.write_bytes(normal + errored + spurious + anchor)
        messages = list(MieFileReader(fpath))
        # All four records emit cleanly without any skip-due-to-invalid path.
        assert len(messages) == 4
        assert messages[1].is_error
        assert messages[2].is_spurious


class TestFuzzHarness:
    """L1-ROB-001: fuzz harness asserting no panic on arbitrary input bytes.

    Mirrors the Rust harness in tests/integration.rs. Same seed and
    PRNG (xorshift64), same iteration count, same size band — so a
    failure in one impl is reproducible against the other.
    """

    @staticmethod
    def _xorshift64(state: int) -> tuple[int, int]:
        x = state & 0xFFFFFFFFFFFFFFFF
        x ^= (x << 13) & 0xFFFFFFFFFFFFFFFF
        x ^= (x >> 7) & 0xFFFFFFFFFFFFFFFF
        x ^= (x << 17) & 0xFFFFFFFFFFFFFFFF
        x &= 0xFFFFFFFFFFFFFFFF
        return x, x  # new state, output

    @pytest.mark.requirement("L1-ROB-001")
    def test_arbitrary_bytes_never_raise_unexpected_exceptions(
        self, tmp_path,
    ) -> None:
        from mie_decoder.exceptions import MieDecoderError
        from mie_decoder.reader import MieFileReader

        seed = 0x0DDCD1ECDDC0DEC0
        state = seed
        # The scheduled CI fuzz job overrides the iteration count via
        # MIE_FUZZ_ITERATIONS for a longer burn-in; the default-suite cost
        # stays bounded. Deterministic PRNG, so a burn-in is a strict
        # superset of the default run (same first 256 iterations). Mirrors
        # the Rust harness's MIE_FUZZ_ITERATIONS handling.
        import os

        iterations = 256
        override = os.environ.get("MIE_FUZZ_ITERATIONS")
        if override and override.isdigit() and int(override) > 0:
            iterations = int(override)

        for i in range(iterations):
            state, r = self._xorshift64(state)
            size = 32 + (r % 8192)
            payload = bytearray(size)
            j = 0
            while j + 8 <= size:
                state, r = self._xorshift64(state)
                payload[j:j + 8] = r.to_bytes(8, "little")
                j += 8
            while j < size:
                state, r = self._xorshift64(state)
                payload[j] = r & 0xFF
                j += 1

            fpath = tmp_path / f"fuzz-{i}.bin"
            fpath.write_bytes(bytes(payload))

            try:
                reader = MieFileReader(fpath)
            except MieDecoderError:
                # Constructor errors (e.g., MieFileEmptyError) are
                # documented and acceptable.
                continue
            except Exception as exc:
                raise AssertionError(
                    f"Unexpected non-MieDecoderError on construction "
                    f"(seed=0x{seed:X}, iter={i}, size={size}): "
                    f"{type(exc).__name__}: {exc}"
                ) from exc

            try:
                yielded = 0
                for _ in reader:
                    yielded += 1
                    if yielded > 100_000:
                        raise AssertionError(
                            f"iterator yielded over 100k items "
                            f"(seed=0x{seed:X}, iter={i}, size={size}) "
                            f"— possible unbounded loop"
                        )
            except MieDecoderError:
                # Decode-time errors are documented and acceptable —
                # the fuzz harness exists to catch IndexError,
                # struct.error, RecursionError, etc.
                pass
            except AssertionError:
                raise
            except Exception as exc:
                raise AssertionError(
                    f"Unexpected non-MieDecoderError during iteration "
                    f"(seed=0x{seed:X}, iter={i}, size={size}): "
                    f"{type(exc).__name__}: {exc}\n"
                    f"First 32 bytes: {bytes(payload[:32]).hex()}"
                ) from exc


class TestSeparateModeCommitOrder:
    """L2-WRT-019: separate mode commits the main CSV before the errors CSV.

    The two files are committed sequentially (each atomic on its own, but
    there is no cross-file atomic rename), so on a mid-commit failure the
    residue must be the primary main CSV, never an orphan errors file.
    """

    @pytest.mark.requirement("L2-WRT-019")
    def test_errors_commit_failure_leaves_main_not_orphan_errors(
        self, tmp_path: Path
    ) -> None:
        """If the errors-file commit fails, the already-committed main CSV
        remains and no orphan errors file (or temp) is left behind."""
        import dataclasses

        from mie_decoder.exceptions import MieWriterError
        from mie_decoder.writer import write_csv_split
        from tests.conftest import RECORD_RT15_SA11_RCV

        fpath = tmp_path / "in.mie"
        fpath.write_bytes(RECORD_RT15_SA11_RCV)
        normal = next(iter(MieFileReader(fpath)))
        errored = dataclasses.replace(
            normal, type_word=dataclasses.replace(normal.type_word, error=True)
        )
        assert errored.error_label == "ERROR"

        dest = tmp_path / "out.csv"
        err_dest = tmp_path / "out_errors.csv"
        # Force the SECOND (errors) commit's os.replace to fail by making
        # the errors destination a directory.
        err_dest.mkdir()

        with pytest.raises(MieWriterError):
            write_csv_split([normal, errored], dest)

        # Main was committed first → present and complete.
        assert dest.read_text().startswith("TIME_STAMP,RT,MSG,")
        # No orphan errors *file*: the destination is still the directory.
        assert err_dest.is_dir()
        # No leftover temp files anywhere in the directory.
        leftover = list(tmp_path.glob("*.mie-decoder.tmp.*"))
        assert leftover == [], f"temp file leaked after failed commit: {leftover}"
