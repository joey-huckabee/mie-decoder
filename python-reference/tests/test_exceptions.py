"""Tests for mie_decoder.exceptions and mie_decoder.logger modules."""

from __future__ import annotations

import logging
from pathlib import Path

import pytest

from mie_decoder.exceptions import (
    MieDecoderError,
    MieFileEmptyError,
    MieFileError,
    MieFileNotFoundError,
    MieInvalidTypeWordError,
    MiePayloadError,
    MieRecordError,
    MieRecordTruncatedError,
    MieUnknownErrorCodeError,
    MieUnknownTypeWordError,
    MieWriterError,
)
from mie_decoder.logger import LOGGER_NAME, configure_logging


class TestExceptionHierarchy:
    """Verify the exception inheritance chain."""

    def test_file_not_found_is_file_error(self) -> None:
        exc = MieFileNotFoundError("/tmp/missing.mie")
        assert isinstance(exc, MieFileError)
        assert isinstance(exc, MieDecoderError)

    def test_file_empty_is_file_error(self) -> None:
        exc = MieFileEmptyError("/tmp/empty.mie")
        assert isinstance(exc, MieFileError)
        assert isinstance(exc, MieDecoderError)

    def test_invalid_type_word_is_record_error(self) -> None:
        exc = MieInvalidTypeWordError(0x100, 0x0000, 0)
        assert isinstance(exc, MieRecordError)
        assert isinstance(exc, MieDecoderError)

    def test_record_truncated_is_record_error(self) -> None:
        exc = MieRecordTruncatedError(0x200, 72, 20)
        assert isinstance(exc, MieRecordError)
        assert isinstance(exc, MieDecoderError)

    def test_payload_error_is_record_error(self) -> None:
        exc = MiePayloadError(0x300, "bad payload")
        assert isinstance(exc, MieRecordError)
        assert isinstance(exc, MieDecoderError)

    def test_unknown_type_word_is_record_error(self) -> None:
        exc = MieUnknownTypeWordError(0x400, 0x0503, 0x03)
        assert isinstance(exc, MieRecordError)
        assert isinstance(exc, MieDecoderError)

    def test_unknown_error_code_is_record_error(self) -> None:
        exc = MieUnknownErrorCodeError(0x500, 0x9999)
        assert isinstance(exc, MieRecordError)
        assert isinstance(exc, MieDecoderError)

    def test_writer_error_is_decoder_error(self) -> None:
        cause = OSError("disk full")
        exc = MieWriterError("output.csv", cause)
        assert isinstance(exc, MieDecoderError)
        assert exc.cause is cause

    def test_catch_all_with_base_class(self) -> None:
        """All custom exceptions should be catchable via MieDecoderError."""
        exceptions = [
            MieFileNotFoundError("/x"),
            MieFileEmptyError("/x"),
            MieInvalidTypeWordError(0, 0, 0),
            MieUnknownTypeWordError(0, 0x0503, 0x03),
            MieUnknownErrorCodeError(0, 0x9999),
            MieRecordTruncatedError(0, 72, 10),
            MiePayloadError(0, "test"),
            MieWriterError("out", OSError()),
        ]
        for exc in exceptions:
            with pytest.raises(MieDecoderError):
                raise exc


class TestExceptionAttributes:
    """Verify exception attributes are accessible."""

    def test_file_not_found_path(self) -> None:
        exc = MieFileNotFoundError("/data/test.mie")
        assert exc.path == "/data/test.mie"
        assert "not found" in str(exc)

    def test_file_empty_path(self) -> None:
        exc = MieFileEmptyError("/data/empty.mie")
        assert exc.path == "/data/empty.mie"
        assert "empty" in str(exc)

    def test_invalid_type_word_fields(self) -> None:
        exc = MieInvalidTypeWordError(0x48, 0x0003, 0)
        assert exc.offset == 0x48
        assert exc.raw_type_word == 0x0003
        assert exc.word_count == 0
        assert "0x0003" in str(exc)

    def test_record_truncated_fields(self) -> None:
        exc = MieRecordTruncatedError(0x100, 72, 20)
        assert exc.offset == 0x100
        assert exc.record_bytes == 72
        assert exc.available_bytes == 20
        assert "72" in str(exc)
        assert "20" in str(exc)

    def test_unknown_type_word_fields(self) -> None:
        exc = MieUnknownTypeWordError(0x200, 0x0503, 0x03)
        assert exc.offset == 0x200
        assert exc.raw_type_word == 0x0503
        assert exc.message_type == 0x03
        assert "0x03" in str(exc)
        assert "Unknown" in str(exc)

    def test_unknown_error_code_fields(self) -> None:
        exc = MieUnknownErrorCodeError(0x300, 0x9999)
        assert exc.offset == 0x300
        assert exc.error_code == 0x9999
        assert "0x9999" in str(exc).lower()
        assert "Unknown" in str(exc)

    def test_writer_error_fields(self) -> None:
        cause = OSError("permission denied")
        exc = MieWriterError("/output/decoded.csv", cause)
        assert exc.destination == "/output/decoded.csv"
        assert exc.cause is cause
        assert "permission denied" in str(exc)


class TestConfigureLogging:
    """Tests for logger.configure_logging()."""

    def test_sets_level(self) -> None:
        configure_logging("DEBUG")
        log = logging.getLogger(LOGGER_NAME)
        assert log.level == logging.DEBUG

    def test_sets_info_level(self) -> None:
        configure_logging("INFO")
        log = logging.getLogger(LOGGER_NAME)
        assert log.level == logging.INFO

    def test_case_insensitive(self) -> None:
        configure_logging("debug")
        log = logging.getLogger(LOGGER_NAME)
        assert log.level == logging.DEBUG

    def test_invalid_level_raises(self) -> None:
        with pytest.raises(ValueError, match="Invalid log level"):
            configure_logging("BOGUS")

    def test_no_duplicate_handlers(self) -> None:
        """Calling configure_logging twice should not add duplicate handlers."""
        configure_logging("INFO")
        configure_logging("DEBUG")
        log = logging.getLogger(LOGGER_NAME)
        assert len(log.handlers) == 1

    def test_outputs_to_stderr_by_default(self) -> None:
        configure_logging("WARNING")
        log = logging.getLogger(LOGGER_NAME)
        assert len(log.handlers) == 1
        import sys
        assert log.handlers[0].stream is sys.stderr  # type: ignore[attr-defined]
