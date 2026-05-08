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

    def test_read_three_records(self, tmp_mie_file: Path) -> None:
        """Should decode exactly 3 messages from the multi-record fixture."""
        messages = list(MieFileReader(tmp_mie_file))
        assert len(messages) == 3

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

    def test_second_record_receive(self, tmp_mie_file: Path) -> None:
        """Second record: RT15 SA22 Receive, 11 data words."""
        msg = list(MieFileReader(tmp_mie_file))[1]
        assert msg.rt == 15
        assert msg.msg_label == "22R"
        assert msg.command_word.subaddress == 22
        assert msg.command_word.direction == Direction.RECEIVE
        assert len(msg.data_words) == 11
        assert msg.status_word == 0x7800

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

    def test_bus_b_record(self, tmp_busb_file: Path) -> None:
        """Bus B file should decode bus=B correctly."""
        msg = list(MieFileReader(tmp_busb_file))[0]
        assert msg.bus == Bus.B
        assert msg.rt == 15
        assert msg.msg_label == "10T"

    def test_delta_first_occurrence_is_zero(self, tmp_mie_file: Path) -> None:
        """First occurrence of any RT/MSG should have delta=0."""
        messages = list(MieFileReader(tmp_mie_file))
        for msg in messages:
            # All three have unique RT/MSG combos, so all should be 0
            assert msg.delta == 0.0

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

    def test_file_not_found(self) -> None:
        """Should raise MieFileNotFoundError for missing files."""
        from mie_decoder.exceptions import MieFileNotFoundError

        with pytest.raises(MieFileNotFoundError):
            MieFileReader("/nonexistent/file.mie")

    def test_empty_file(self, tmp_path: Path) -> None:
        """Should raise MieFileEmptyError for empty files."""
        from mie_decoder.exceptions import MieFileEmptyError

        fpath = tmp_path / "empty.mie"
        fpath.write_bytes(b"")
        with pytest.raises(MieFileEmptyError, match="empty"):
            MieFileReader(fpath)

    def test_truncated_record(self, tmp_path: Path) -> None:
        """Truncated final record should be silently skipped."""
        from tests.conftest import RECORD_RT15_SA11_RCV

        fpath = tmp_path / "truncated.mie"
        fpath.write_bytes(RECORD_RT15_SA11_RCV + RECORD_RT15_SA11_RCV[:20])
        messages = list(MieFileReader(fpath))
        assert len(messages) == 1

    def test_truncated_record_strict(self, tmp_path: Path) -> None:
        """Strict mode should raise MieRecordTruncatedError on truncation."""
        from tests.conftest import RECORD_RT15_SA11_RCV
        from mie_decoder.exceptions import MieRecordTruncatedError

        fpath = tmp_path / "truncated_strict.mie"
        fpath.write_bytes(RECORD_RT15_SA11_RCV + RECORD_RT15_SA11_RCV[:20])
        with pytest.raises(MieRecordTruncatedError):
            list(MieFileReader(fpath, strict=True))

    def test_invalid_record_strict(self, tmp_path: Path) -> None:
        """Strict mode should raise on invalid record after good data."""
        from mie_decoder.exceptions import MieDecoderError

        from tests.conftest import RECORD_RT15_SA11_RCV
        bad_record = b"\x03\x00" + b"\x00" * 18  # type 0x03, wc=0
        fpath = tmp_path / "bad_record.mie"
        fpath.write_bytes(RECORD_RT15_SA11_RCV * 2 + bad_record)
        with pytest.raises(MieDecoderError):
            list(MieFileReader(fpath, strict=True, time_format=TimestampFormat.IRIG))

    def test_file_offset_tracking(self, tmp_mie_file: Path) -> None:
        """Each message should report its byte offset in the file."""
        messages = list(MieFileReader(tmp_mie_file))
        assert messages[0].file_offset == 0
        assert messages[1].file_offset == 72  # 36 words * 2
        assert messages[2].file_offset == 72 + 34  # 36*2 + 17*2


class TestCsvWriter:
    """Integration tests for CSV output."""

    def test_csv_header(self, tmp_mie_file: Path) -> None:
        """CSV output should start with the correct header row."""
        buf = io.StringIO()
        write_csv(MieFileReader(tmp_mie_file), output=buf)
        buf.seek(0)
        reader = csv.reader(buf)
        header = next(reader)
        assert header == CSV_HEADER

    def test_csv_row_count(self, tmp_mie_file: Path) -> None:
        """Should produce one header + 3 data rows."""
        buf = io.StringIO()
        write_csv(MieFileReader(tmp_mie_file), output=buf)
        buf.seek(0)
        lines = buf.getvalue().strip().split("\n")
        assert len(lines) == 4  # header + 3 records

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

    def test_csv_to_file(self, tmp_mie_file: Path, tmp_path: Path) -> None:
        """Writing to a file path should produce a valid CSV file."""
        out = tmp_path / "output.csv"
        count = write_csv(MieFileReader(tmp_mie_file), output=out)
        assert out.exists()
        lines = out.read_text().strip().split("\n")
        assert len(lines) == 4
        assert count == 3

    def test_write_csv_returns_count(self, tmp_mie_file: Path) -> None:
        """write_csv should return the number of messages written."""
        buf = io.StringIO()
        count = write_csv(MieFileReader(tmp_mie_file), output=buf)
        assert count == 3

    def test_messages_to_dataframe(self, tmp_mie_file: Path) -> None:
        """messages_to_dataframe should produce a DataFrame with correct shape."""
        from mie_decoder.writer import messages_to_dataframe, CSV_HEADER

        df = messages_to_dataframe(MieFileReader(tmp_mie_file))
        assert len(df) == 3
        assert list(df.columns) == CSV_HEADER


class TestCliEndToEnd:
    """End-to-end CLI tests."""

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

    def test_cli_decode_output_file(self, tmp_mie_file: Path, tmp_path: Path) -> None:
        """CLI decode with -o should write to the specified file."""
        from mie_decoder.cli import main

        out = tmp_path / "cli_out.csv"
        rc = main(["decode", str(tmp_mie_file), "-o", str(out)])
        assert rc == 0
        assert out.exists()

    def test_cli_decode_count(self, tmp_mie_file: Path, capsys: pytest.CaptureFixture[str]) -> None:
        """CLI decode --count should print message count to stderr."""
        from mie_decoder.cli import main

        rc = main(["decode", str(tmp_mie_file), "--count"])
        assert rc == 0
        captured = capsys.readouterr()
        assert "3 messages" in captured.err

    def test_cli_decode_missing_file(self, capsys: pytest.CaptureFixture[str]) -> None:
        """CLI decode with nonexistent file should return exit code 1."""
        from mie_decoder.cli import main

        rc = main(["decode", "/nonexistent/file.mie"])
        assert rc == 1

    def test_cli_log_level_info(self, tmp_mie_file: Path, tmp_path: Path) -> None:
        """CLI --log-level INFO should emit log messages to stderr."""
        from mie_decoder.cli import main

        out = tmp_path / "log_test.csv"
        rc = main(["--log-level", "INFO", "decode", str(tmp_mie_file), "-o", str(out)])
        assert rc == 0
        assert out.exists()

    def test_cli_log_level_debug(self, tmp_mie_file: Path, tmp_path: Path) -> None:
        """CLI --log-level DEBUG should succeed without error."""
        from mie_decoder.cli import main

        out = tmp_path / "debug_test.csv"
        rc = main(["--log-level", "DEBUG", "decode", str(tmp_mie_file), "-o", str(out)])
        assert rc == 0

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

    def test_cli_dump_missing_file(self) -> None:
        """CLI dump with nonexistent file should return exit code 1."""
        from mie_decoder.cli import main

        rc = main(["dump", "/nonexistent/file.mie"])
        assert rc == 1

    def test_cli_no_subcommand(self) -> None:
        """CLI with no subcommand should return exit code 1."""
        from mie_decoder.cli import main

        rc = main([])
        assert rc == 1
