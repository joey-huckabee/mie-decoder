"""Streaming-specific behavior tests for ``write_csv_split`` (PY-streaming).

Covers the split partial-commit path when error rows are present before
an unrecoverable sync loss — both the main and errors temp files are
renamed to ``.partial`` and surfaced on the WriteOutcome. The
main-before-errors commit ordering and errors-commit-failure cleanup are
covered by ``test_e2e.py``; the byte-image equivalence by
``test_writer_streaming_golden.py``.
"""

from __future__ import annotations

import dataclasses
from collections.abc import Iterator
from pathlib import Path

import pytest

from mie_decoder.exceptions import MieUnrecoverableSyncLossError
from mie_decoder.models import MieMessage
from mie_decoder.reader import MieFileReader
from mie_decoder.writer import WriteOptions, write_csv_split

from tests.conftest import RECORD_RT15_SA11_RCV


def _normal_and_errored(tmp_path: Path) -> tuple[MieMessage, MieMessage]:
    fpath = tmp_path / "in.mie"
    fpath.write_bytes(RECORD_RT15_SA11_RCV)
    normal = next(iter(MieFileReader(fpath)))
    errored = dataclasses.replace(
        normal, type_word=dataclasses.replace(normal.type_word, error=True)
    )
    assert errored.error_label == "ERROR"
    return normal, errored


@pytest.mark.requirement("L2-WRT-016")
@pytest.mark.requirement("L1-EXIT-004")
def test_split_allow_partial_with_errors_commits_both_partials(
    tmp_path: Path,
) -> None:
    """A sync loss after a normal + an errored row commits BOTH files as
    ``.partial`` and reports them on the outcome."""
    normal, errored = _normal_and_errored(tmp_path)

    def stream() -> Iterator[MieMessage]:
        yield normal
        yield errored
        raise MieUnrecoverableSyncLossError(offset=0x1234, sync_losses=2)

    dest = tmp_path / "out.csv"
    outcome = write_csv_split(stream(), dest, WriteOptions(allow_partial=True))

    assert outcome.partial is not None
    assert outcome.partial.offset == 0x1234
    assert outcome.partial.sync_losses == 2
    assert outcome.normal_count == 1
    assert outcome.error_count == 1

    main_partial = dest.with_name("out.csv.partial")
    errors_partial = (tmp_path / "out_errors.csv").with_name("out_errors.csv.partial")
    assert outcome.partial.main_path == main_partial
    assert outcome.partial.errors_path == errors_partial

    # The non-.partial destinations must NOT exist (loss was unrecoverable).
    assert not dest.exists()
    assert not (tmp_path / "out_errors.csv").exists()

    # Both partials hold their header + one row, LF-only.
    main_bytes = main_partial.read_bytes()
    err_bytes = errors_partial.read_bytes()
    assert main_bytes.startswith(b"TIME_STAMP,RT,MSG,")
    assert main_bytes.count(b"\n") == 2  # header + 1 normal row
    assert b"\r" not in main_bytes
    assert err_bytes.count(b"\n") == 2  # header + 1 error row
    assert b",ERROR," in err_bytes

    # No temp files left behind.
    assert list(tmp_path.glob("*.mie-decoder.tmp.*")) == []


@pytest.mark.requirement("L2-WRT-016")
@pytest.mark.requirement("L1-EXIT-004")
def test_split_allow_partial_no_errors_omits_errors_partial(
    tmp_path: Path,
) -> None:
    """When no error rows precede the sync loss, only the main ``.partial``
    is committed and ``errors_path`` stays None."""
    normal, _ = _normal_and_errored(tmp_path)

    def stream() -> Iterator[MieMessage]:
        yield normal
        raise MieUnrecoverableSyncLossError(offset=0x10, sync_losses=1)

    dest = tmp_path / "out.csv"
    outcome = write_csv_split(stream(), dest, WriteOptions(allow_partial=True))

    assert outcome.partial is not None
    assert outcome.partial.errors_path is None
    assert outcome.error_count == 0
    assert (dest.with_name("out.csv.partial")).exists()
    assert not (tmp_path / "out_errors.csv.partial").exists()
    assert list(tmp_path.glob("*.mie-decoder.tmp.*")) == []
