"""Unit tests for the PY-streaming primitives in ``writer.py``.

Exercises ``_AtomicCsvFile`` (temp + atomic rename / partial / cleanup)
and ``_StreamingCsvRowWriter`` (header + streamed rows) in isolation,
ahead of wiring them into ``write_csv`` / ``write_csv_split``. The
byte-level output equivalence with the pandas path is pinned separately
by ``test_writer_streaming_golden.py``.
"""

from __future__ import annotations

import io
from pathlib import Path

import pytest

from mie_decoder.exceptions import MieWriterError
from mie_decoder.writer import (
    CSV_HEADER,
    _AtomicCsvFile,
    _StreamingCsvRowWriter,
    _make_temp_path,
)

from tests.conftest import normal_record_rt15_sa11_us
from mie_decoder.reader import MieFileReader


# ── _AtomicCsvFile ─────────────────────────────────────────────────────


@pytest.mark.requirement("L2-WRT-015")
def test_atomic_commit_renames_temp_over_destination(tmp_path: Path) -> None:
    dest = tmp_path / "out.csv"
    atomic = _AtomicCsvFile(dest)
    atomic.stream.write("hello\n")
    atomic.commit()
    assert dest.read_text() == "hello\n"
    # Temp must be gone after commit.
    assert not _make_temp_path(dest).exists()


@pytest.mark.requirement("L2-WRT-016")
def test_atomic_close_without_commit_unlinks_temp_and_keeps_destination(
    tmp_path: Path,
) -> None:
    dest = tmp_path / "out.csv"
    dest.write_text("original\n")
    tmp = _make_temp_path(dest)
    atomic = _AtomicCsvFile(dest)
    atomic.stream.write("discarded\n")
    atomic.close()  # simulates a decode failure before commit
    assert not tmp.exists(), "temp should be unlinked on uncommitted close"
    assert dest.read_text() == "original\n", "destination must be untouched"


@pytest.mark.requirement("L2-WRT-016")
def test_atomic_context_manager_cleans_up_on_exception(tmp_path: Path) -> None:
    dest = tmp_path / "out.csv"
    tmp = _make_temp_path(dest)
    with pytest.raises(RuntimeError):
        with _AtomicCsvFile(dest) as atomic:
            atomic.stream.write("partial work\n")
            raise RuntimeError("boom")
    assert not tmp.exists(), "temp leaked after exception in context manager"
    assert not dest.exists(), "destination must not be created on failure"


@pytest.mark.requirement("L2-WRT-016")
def test_atomic_commit_partial_writes_dot_partial_and_keeps_destination(
    tmp_path: Path,
) -> None:
    dest = tmp_path / "out.csv"
    dest.write_text("original\n")
    atomic = _AtomicCsvFile(dest)
    atomic.stream.write("partial decode\n")
    partial_path = atomic.commit_partial()
    assert partial_path == dest.with_name("out.csv.partial")
    assert partial_path.read_text() == "partial decode\n"
    # Original destination untouched; temp gone.
    assert dest.read_text() == "original\n"
    assert not _make_temp_path(dest).exists()


@pytest.mark.requirement("L2-WRT-016")
def test_atomic_commit_failure_wraps_writer_error(tmp_path: Path) -> None:
    # Force the rename to fail by making the destination a directory:
    # os.replace of a file over a non-empty dir fails on POSIX and Windows.
    dest = tmp_path / "out.csv"
    dest.mkdir()
    atomic = _AtomicCsvFile(dest)
    atomic.stream.write("data\n")
    with pytest.raises(MieWriterError):
        atomic.commit()
    # Temp must be cleaned up after the failed commit.
    assert not _make_temp_path(dest).exists()


# ── _StreamingCsvRowWriter ─────────────────────────────────────────────


@pytest.mark.requirement("L2-WRT-001")
def test_streaming_writer_emits_header_on_construction() -> None:
    buf = io.StringIO()
    writer = _StreamingCsvRowWriter(buf, "memory")
    assert writer.rows_written == 0
    header_line = buf.getvalue().rstrip("\n")
    assert header_line.split(",") == CSV_HEADER


@pytest.mark.requirement("L2-WRT-001")
def test_streaming_writer_streams_rows_incrementally(tmp_path: Path) -> None:
    """Rows must reach the stream as they are written, not buffered."""
    data = normal_record_rt15_sa11_us(100) + normal_record_rt15_sa11_us(16100)
    mie = tmp_path / "two.mie"
    mie.write_bytes(data)

    buf = io.StringIO()
    writer = _StreamingCsvRowWriter(buf, "memory")
    messages = list(MieFileReader(mie))

    writer.write_message(messages[0])
    after_one = buf.getvalue()
    assert writer.rows_written == 1
    # The first data row is already present in the stream before the
    # second is written — i.e. output is streamed, not materialized.
    assert after_one.count("\n") == 2  # header + 1 row

    writer.write_message(messages[1])
    assert writer.rows_written == 2
    assert buf.getvalue().count("\n") == 3  # header + 2 rows
    # LF-only, no CR.
    assert "\r" not in buf.getvalue()
