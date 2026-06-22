"""Tests for the multi-file time-sorted merge (L1-MRG / L2-MRG).

Mirrors the Rust `tests/integration.rs` merge tests so both implementations
exercise the same behavior.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from mie_decoder.exceptions import MieIncompatibleMergeInputsError
from mie_decoder.merge import (
    MAX_MERGE_FILES,
    expand_glob,
    glob_match,
    merge_readers,
    read_manifest,
)
from mie_decoder.models import TimestampFormat
from mie_decoder.reader import MieFileReader
from tests.conftest import RECORD_RT15_SA11_RCV


def rt15_record_at(
    day: int,
    hour: int,
    minute: int,
    second: int,
    micro: int,
    freerun: bool = False,
) -> bytes:
    """An RT15 SA11 Receive record at a chosen IRIG instant, by patching the
    timestamp triple of the canonical fixture (bytes 2..8). Mirrors the Rust
    `rt15_record_at` helper."""
    fr = (1 if freerun else 0) << 15
    upper = fr | ((day & 0x1FF) << 5) | (hour & 0x1F)
    middle = ((minute & 0x3F) << 10) | ((second & 0x3F) << 4) | ((micro >> 16) & 0xF)
    lower = micro & 0xFFFF
    ts = (
        upper.to_bytes(2, "little")
        + middle.to_bytes(2, "little")
        + lower.to_bytes(2, "little")
    )
    return RECORD_RT15_SA11_RCV[:2] + ts + RECORD_RT15_SA11_RCV[8:]


@pytest.mark.requirement("L1-MRG-001")
@pytest.mark.requirement("L2-MRG-002")
@pytest.mark.requirement("L2-MRG-005")
def test_merge_orders_records_across_files_by_absolute_time(tmp_path: Path) -> None:
    # File A: 100µs, 300µs; File B: 200µs, 400µs (same sec → micros discriminate).
    a = rt15_record_at(192, 15, 54, 50, 100) + rt15_record_at(192, 15, 54, 50, 300)
    b = rt15_record_at(192, 15, 54, 50, 200) + rt15_record_at(192, 15, 54, 50, 400)
    fa = tmp_path / "a.mie"
    fb = tmp_path / "b.mie"
    fa.write_bytes(a)
    fb.write_bytes(b)

    readers = [MieFileReader(fa), MieFileReader(fb)]
    msgs = list(merge_readers(readers))
    assert len(msgs) == 4

    us = [m.timestamp.to_microseconds(None) for m in msgs]
    assert all(us[i] < us[i + 1] for i in range(len(us) - 1)), f"not ordered: {us}"

    # Global DELTA (L2-MRG-005): first occurrence 0.0, then non-negative.
    assert msgs[0].delta == 0.0
    assert all(m.delta is not None and m.delta >= 0.0 for m in msgs[1:])


@pytest.mark.requirement("L2-MRG-001")
def test_merge_single_input_is_unchanged(tmp_path: Path) -> None:
    a = rt15_record_at(192, 15, 54, 50, 10) + rt15_record_at(192, 15, 54, 50, 20)
    fa = tmp_path / "a.mie"
    fa.write_bytes(a)
    msgs = list(merge_readers([MieFileReader(fa)]))
    assert len(msgs) == 2


@pytest.mark.requirement("L1-MRG-002")
@pytest.mark.requirement("L2-MRG-003")
def test_merge_rejects_freerun_leading_input(tmp_path: Path) -> None:
    good = rt15_record_at(192, 15, 54, 50, 100) + rt15_record_at(192, 15, 54, 50, 300)
    freerun = rt15_record_at(0, 0, 0, 0, 0, freerun=True) + rt15_record_at(
        0, 0, 0, 1, 0, freerun=True
    )
    fa = tmp_path / "a.mie"
    fb = tmp_path / "b.mie"
    fa.write_bytes(good)
    fb.write_bytes(freerun)
    with pytest.raises(MieIncompatibleMergeInputsError):
        merge_readers([MieFileReader(fa), MieFileReader(fb)])


@pytest.mark.requirement("L1-MRG-002")
@pytest.mark.requirement("L2-MRG-003")
def test_merge_rejects_standard_format_input(tmp_path: Path) -> None:
    a = rt15_record_at(192, 15, 54, 50, 100) + rt15_record_at(192, 15, 54, 50, 300)
    fa = tmp_path / "a.mie"
    fa.write_bytes(a)
    # Forcing Standard makes the records decode as Standard timestamps, which
    # have no shared epoch → not mergeable.
    readers = [
        MieFileReader(fa, time_format=TimestampFormat.STANDARD),
        MieFileReader(fa, time_format=TimestampFormat.STANDARD),
    ]
    with pytest.raises(MieIncompatibleMergeInputsError):
        merge_readers(readers)


@pytest.mark.requirement("L2-MRG-001")
def test_read_manifest_skips_blanks_and_comments(tmp_path: Path) -> None:
    manifest = tmp_path / "list.txt"
    manifest.write_text(
        "# a comment\n\nfile1.mie\n  file2.mie  \n# another\nfile3.mie\n",
        encoding="utf-8",
    )
    paths = read_manifest(manifest)
    assert paths == [
        Path("file1.mie"),
        Path("file2.mie"),
        Path("file3.mie"),
    ]


@pytest.mark.requirement("L2-MRG-001")
def test_expand_glob_matches_and_sorts(tmp_path: Path) -> None:
    (tmp_path / "b.mie").write_bytes(b"")
    (tmp_path / "a.mie").write_bytes(b"")
    (tmp_path / "c.csv").write_bytes(b"")
    matched = [p.name for p in expand_glob(str(tmp_path / "*.mie"))]
    assert matched == ["a.mie", "b.mie"]  # sorted, .csv excluded


@pytest.mark.requirement("L2-MRG-001")
def test_glob_match_wildcards() -> None:
    assert glob_match("*.mie", "rec1.mie")
    assert glob_match("rec?.mie", "rec5.mie")
    assert not glob_match("rec?.mie", "rec55.mie")
    assert glob_match("*", "anything")
    assert glob_match("a*b*c", "axxbyyc")
    assert not glob_match("*.mie", "rec.csv")
    assert glob_match("", "")
    assert not glob_match("", "x")
    assert glob_match("a.b", "a.b")
    assert not glob_match("a.b", "axb")


def test_max_merge_files_matches_rust() -> None:
    # The cap is shared in value with the Rust constant (L3-PY-014).
    assert MAX_MERGE_FILES == 256


# ── CLI bad-input / cap / robustness (L2-MRG-001, L1-ROB-001) ──────────────


@pytest.mark.requirement("L2-MRG-001")
def test_cli_rejects_combined_input_methods(tmp_path: Path) -> None:
    from mie_decoder.cli import EXIT_USAGE, main

    out = tmp_path / "o.csv"
    # positional + --manifest, positional + --glob, --manifest + --glob
    assert main(["decode", "a.mie", "--manifest", "list.txt", "-o", str(out)]) == EXIT_USAGE
    assert main(["decode", "a.mie", "--glob", "*.mie", "-o", str(out)]) == EXIT_USAGE
    assert main(["decode", "--manifest", "l.txt", "--glob", "*.mie", "-o", str(out)]) == EXIT_USAGE
    assert not out.exists()


@pytest.mark.requirement("L2-MRG-001")
def test_cli_rejects_over_cap(tmp_path: Path) -> None:
    from mie_decoder.cli import EXIT_USAGE, main

    manifest = tmp_path / "many.txt"
    manifest.write_text(
        "\n".join(f"f{i}.mie" for i in range(MAX_MERGE_FILES + 1)) + "\n",
        encoding="utf-8",
    )
    out = tmp_path / "o.csv"
    # Cap is checked before any file is opened, so non-existent paths are fine.
    assert main(["decode", "--manifest", str(manifest), "-o", str(out)]) == EXIT_USAGE
    assert not out.exists()


@pytest.mark.requirement("L2-MRG-001")
def test_cli_glob_no_match_is_usage_error(tmp_path: Path) -> None:
    from mie_decoder.cli import EXIT_USAGE, main

    out = tmp_path / "o.csv"
    assert main(["decode", "--glob", str(tmp_path / "*.nomatch"), "-o", str(out)]) == EXIT_USAGE


@pytest.mark.requirement("L1-ROB-001")
def test_cli_manifest_missing_is_runtime_error(tmp_path: Path) -> None:
    from mie_decoder.cli import EXIT_RUNTIME, main

    out = tmp_path / "o.csv"
    assert main(["decode", "--manifest", str(tmp_path / "nope.txt"), "-o", str(out)]) == EXIT_RUNTIME


@pytest.mark.requirement("L1-ROB-001")
def test_cli_manifest_non_utf8_is_runtime_error(tmp_path: Path) -> None:
    # Matches the Rust reader's read_to_string failure → exit 1 (not a usage
    # error), keeping the two implementations' exit codes identical.
    from mie_decoder.cli import EXIT_RUNTIME, main

    manifest = tmp_path / "bin.txt"
    manifest.write_bytes(b"\xff\xfe\x00\x01\x80\x81 not utf-8")
    out = tmp_path / "o.csv"
    assert main(["decode", "--manifest", str(manifest), "-o", str(out)]) == EXIT_RUNTIME


@pytest.mark.requirement("L1-ROB-001")
def test_read_manifest_tolerates_arbitrary_bytes(tmp_path: Path) -> None:
    # read_manifest on arbitrary bytes must only ever return a list or raise
    # UnicodeDecodeError — never an unexpected exception. Deterministic.
    import random

    rng = random.Random(0x0DDCD1EC)
    manifest = tmp_path / "fuzz.txt"
    for _ in range(512):
        n = rng.randint(0, 96)
        manifest.write_bytes(bytes(rng.randint(0, 255) for _ in range(n)))
        try:
            result = read_manifest(manifest)
        except UnicodeDecodeError:
            continue  # non-UTF8 is a documented failure, not a crash
        assert isinstance(result, list)
