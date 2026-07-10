"""Unit tests for the decode CLI helper functions in ``mie_decoder.cli``.

White-box tests for the helpers extracted from ``_run_decode`` (the override
builders, validators, exit-code classifiers, and the merge output-collision
check). End-to-end behavior is covered by ``test_e2e.py`` / ``test_merge.py``;
these exercise each helper branch in isolation so the decomposition is fully
covered and individually verifiable.
"""

from __future__ import annotations

import argparse
from pathlib import Path
from types import SimpleNamespace

import pytest

from mie_decoder import cli
from mie_decoder.cli import (
    EXIT_MERGE_INCOMPATIBLE,
    EXIT_NO_RECORDS,
    EXIT_OK,
    EXIT_RUNTIME,
    EXIT_SYNC_LOSS,
)
from mie_decoder.exceptions import (
    MieClobberRefusedError,
    MieHomogeneousPayloadError,
    MieIncompatibleMergeInputsError,
    MieInputOutputCollisionError,
    MieNonMonotonicInputError,
    MieNoValidRecordsError,
    MieRecordError,
    MieTimestampFormatMismatchError,
    MieUnrecoverableSyncLossError,
    MieWriterError,
)


# ── validators ─────────────────────────────────────────────────────────────


class TestValidators:
    def test_int_range_accepts_bounds_and_interior(self) -> None:
        assert cli._validate_int_range(1, "--x", 1, 10) == 1
        assert cli._validate_int_range(10, "--x", 1, 10) == 10
        assert cli._validate_int_range(5, "--x", 1, 10) == 5

    @pytest.mark.parametrize("value", [0, 11, -1])
    def test_int_range_rejects_out_of_range(self, value: int) -> None:
        with pytest.raises(ValueError, match=r"invalid --x: .*; valid range: \[1, 10\]"):
            cli._validate_int_range(value, "--x", 1, 10)

    def test_positive_finite_accepts(self) -> None:
        assert cli._validate_positive_finite(1.0, "--hz") == 1.0

    @pytest.mark.parametrize("value", [0.0, -1.0, float("inf"), float("nan")])
    def test_positive_finite_rejects(self, value: float) -> None:
        with pytest.raises(ValueError, match="must be a finite value greater than 0"):
            cli._validate_positive_finite(value, "--hz")

    def test_nonempty_accepts(self) -> None:
        assert cli._validate_nonempty(".", "--mux-delimiter") == "."

    def test_nonempty_rejects_empty(self) -> None:
        with pytest.raises(ValueError, match="must be a non-empty string"):
            cli._validate_nonempty("", "--mux-delimiter")

    @pytest.mark.parametrize(
        ("text", "expected"),
        [
            ("0", 0),
            ("16", 16),
            ("0x10", 16),
            ("0X10", 16),
            ("0o20", 16),
            ("0b10000", 16),
            ("  16  ", 16),
        ],
    )
    def test_nonneg_int_accepts_decimal_and_prefixed(self, text: str, expected: int) -> None:
        assert cli._nonneg_int(text) == expected

    @pytest.mark.parametrize("text", ["-1", "-0x10", "foo", "0x", "", "1.5"])
    def test_nonneg_int_rejects_invalid(self, text: str) -> None:
        with pytest.raises(argparse.ArgumentTypeError, match="non-negative integer"):
            cli._nonneg_int(text)

    @pytest.mark.parametrize("flag", ["--offset", "--length", "--records"])
    def test_dump_numeric_args_accept_hex(self, flag: str) -> None:
        # Every numeric dump argument accepts 0x hex identically (previously
        # --records was decimal-only, an internal inconsistency).
        args = cli.build_parser().parse_args(["dump", "f.mie", flag, "0x10"])
        assert getattr(args, flag.lstrip("-")) == 16

    def test_log_safe_neutralizes_crlf(self) -> None:
        # S5145: user-controlled values (e.g. an input path) must not be able to
        # inject newlines into the log. CR/LF are escaped; plain text is intact.
        assert cli._log_safe("plain/path.mie") == "plain/path.mie"
        assert cli._log_safe("evil\nINJECTED") == "evil\\nINJECTED"
        assert cli._log_safe("a\r\nb") == "a\\r\\nb"
        from pathlib import PurePosixPath

        assert cli._log_safe(PurePosixPath("dir/x.mie")) == "dir/x.mie"


# ── override building ───────────────────────────────────────────────────────


def _decode_ns(**overrides: object) -> argparse.Namespace:
    """A decode-args Namespace with every override field defaulted (None/False)."""
    base: dict[str, object] = {
        "time_format": None,
        "inline_errors": False,
        "no_clobber": False,
        "allow_partial": False,
        "strict": None,
        "format": None,
        "no_mux": False,
        "mux_field": None,
        "mux_delimiter": None,
        "collapse_duplicates": None,
        "collapse_window_us": None,
        "detect_records": None,
        "lookahead_records": None,
        "standard_tick_rate_hz": None,
        "exclude_types": None,
        "exclude_rts": None,
        "exclude_buses": None,
        "exclude_subaddresses": None,
        "include_types": None,
        "include_rts": None,
        "include_buses": None,
        "include_subaddresses": None,
    }
    base.update(overrides)
    return argparse.Namespace(**base)


class TestBuildDecodeOverrides:
    def test_empty_namespace_yields_no_overrides(self) -> None:
        assert cli._build_decode_overrides(_decode_ns()) == {}

    def test_simple_flag_passthroughs(self) -> None:
        ov = cli._build_decode_overrides(
            _decode_ns(no_clobber=True, allow_partial=True, no_mux=True, mux_field=2)
        )
        assert ov["no_clobber"] is True
        assert ov["allow_partial"] is True
        assert ov["mux_enabled"] is False
        assert ov["mux_field"] == 2

    def test_filter_values_parsed(self) -> None:
        ov = cli._build_decode_overrides(_decode_ns(include_rts=["15", "31"]))
        assert ov["include_rts"] == [15, 31]

    def test_all_filter_branches(self) -> None:
        ov = cli._build_decode_overrides(
            _decode_ns(
                exclude_types=["0x20"],
                exclude_rts=["31"],
                exclude_buses=["B"],
                exclude_subaddresses=["1"],
                include_types=["0x02"],
                include_rts=["15"],
                include_buses=["A"],
                include_subaddresses=["11"],
            )
        )
        for key in (
            "exclude_types",
            "exclude_buses",
            "exclude_subaddresses",
            "include_types",
            "include_buses",
            "include_subaddresses",
        ):
            assert key in ov
        assert ov["exclude_rts"] == [31]
        assert ov["include_rts"] == [15]

    def test_time_format_and_simple_value_overrides(self) -> None:
        from mie_decoder.models import ErrorMode, TimestampFormat

        ov = cli._build_decode_overrides(
            _decode_ns(
                time_format="standard",
                inline_errors=True,
                strict=True,
                format="csv",
            )
        )
        assert ov["time_format"] == TimestampFormat.STANDARD
        assert ov["error_mode"] == ErrorMode.INLINE
        assert ov["strict"] is True
        assert ov["output_format"] == "csv"

    @pytest.mark.parametrize(
        ("spelling", "expected"),
        [
            ("IRIG", "IRIG"),
            ("Irig", "IRIG"),
            ("AUTO", "AUTO"),
            ("Standard", "STANDARD"),
        ],
    )
    def test_time_format_is_case_insensitive(self, spelling: str, expected: str) -> None:
        from mie_decoder.models import TimestampFormat

        ov = cli._build_decode_overrides(_decode_ns(time_format=spelling))
        assert ov["time_format"] == TimestampFormat[expected]

    def test_time_format_invalid_raises_value_error(self) -> None:
        with pytest.raises(ValueError, match="Invalid time_format"):
            cli._build_decode_overrides(_decode_ns(time_format="bogus"))

    def test_detect_and_lookahead_valid_bounds(self) -> None:
        from mie_decoder.config import DETECT_RECORDS_MIN, LOOKAHEAD_RECORDS_MIN

        ov = cli._build_decode_overrides(
            _decode_ns(
                detect_records=DETECT_RECORDS_MIN,
                lookahead_records=LOOKAHEAD_RECORDS_MIN,
                standard_tick_rate_hz=1_000_000.0,
            )
        )
        assert ov["detect_records"] == DETECT_RECORDS_MIN
        assert ov["lookahead_records"] == LOOKAHEAD_RECORDS_MIN
        assert ov["standard_tick_rate_hz"] == 1_000_000.0

    def test_bad_filter_value_raises(self) -> None:
        with pytest.raises(ValueError):
            cli._build_decode_overrides(_decode_ns(include_rts=["999"]))

    def test_empty_mux_delimiter_raises(self) -> None:
        with pytest.raises(ValueError, match="must be a non-empty string"):
            cli._build_decode_overrides(_decode_ns(mux_delimiter=""))

    def test_detect_records_out_of_range_raises(self) -> None:
        with pytest.raises(ValueError, match="--detect-records"):
            cli._build_decode_overrides(_decode_ns(detect_records=10**9))

    def test_standard_tick_rate_nonpositive_raises(self) -> None:
        with pytest.raises(ValueError, match="--standard-tick-rate-hz"):
            cli._build_decode_overrides(_decode_ns(standard_tick_rate_hz=0.0))


# ── error classification ────────────────────────────────────────────────────


class TestClassifyDecodeError:
    def test_incompatible_merge(self, capsys: pytest.CaptureFixture[str]) -> None:
        exc = MieIncompatibleMergeInputsError(0, "a.mie", "freerun-leading")
        assert cli._classify_decode_error(exc) == EXIT_MERGE_INCOMPATIBLE
        assert "Error:" in capsys.readouterr().err

    def test_input_output_collision(self, capsys: pytest.CaptureFixture[str]) -> None:
        assert cli._classify_decode_error(MieInputOutputCollisionError("p")) == EXIT_RUNTIME
        assert "Error:" in capsys.readouterr().err

    def test_clobber_refused(self, capsys: pytest.CaptureFixture[str]) -> None:
        assert cli._classify_decode_error(MieClobberRefusedError("p")) == EXIT_RUNTIME
        assert "Error:" in capsys.readouterr().err

    def test_no_valid_records(self, capsys: pytest.CaptureFixture[str]) -> None:
        assert cli._classify_decode_error(MieNoValidRecordsError("p", 64)) == EXIT_NO_RECORDS
        assert "Error:" in capsys.readouterr().err

    def test_homogeneous_payload(self, capsys: pytest.CaptureFixture[str]) -> None:
        assert cli._classify_decode_error(MieHomogeneousPayloadError("p", 0, 4)) == EXIT_NO_RECORDS
        assert "Error:" in capsys.readouterr().err

    def test_timestamp_format_mismatch(self, capsys: pytest.CaptureFixture[str]) -> None:
        assert (
            cli._classify_decode_error(MieTimestampFormatMismatchError(0, 3, 2, 8))
            == EXIT_NO_RECORDS
        )
        assert "Error:" in capsys.readouterr().err

    def test_unrecoverable_sync_loss(self, capsys: pytest.CaptureFixture[str]) -> None:
        assert cli._classify_decode_error(MieUnrecoverableSyncLossError(0x10, 3)) == EXIT_SYNC_LOSS
        assert "Error:" in capsys.readouterr().err

    def test_broken_pipe_returns_ok_without_print(self, capsys: pytest.CaptureFixture[str]) -> None:
        assert cli._classify_decode_error(BrokenPipeError()) == EXIT_OK
        assert capsys.readouterr().err == ""

    def test_writer_error_uses_distinct_message(self, capsys: pytest.CaptureFixture[str]) -> None:
        exc = MieWriterError("stdout", OSError("disk full"))
        assert cli._classify_decode_error(exc) == EXIT_RUNTIME
        assert "Error writing output" in capsys.readouterr().err

    def test_non_monotonic_input(self, capsys: pytest.CaptureFixture[str]) -> None:
        assert cli._classify_decode_error(MieNonMonotonicInputError(0, "p", 5, 4)) == EXIT_RUNTIME
        assert "Error:" in capsys.readouterr().err

    def test_generic_decoder_error_falls_through(self, capsys: pytest.CaptureFixture[str]) -> None:
        # A MieRecordError that is not one of the specific handled subtypes
        # hits the generic "Decode failed" arm (exit 1).
        assert cli._classify_decode_error(MieRecordError(0x20, "boom")) == EXIT_RUNTIME
        assert "Error:" in capsys.readouterr().err


# ── success classification ──────────────────────────────────────────────────


class TestClassifyDecodeSuccess:
    def test_complete(self) -> None:
        outcome = SimpleNamespace(partial=None, normal_count=3, error_count=0)
        readers = [SimpleNamespace(sync_losses=0, empty_recording=False)]
        assert cli._classify_decode_success(outcome, readers) == EXIT_OK  # type: ignore[arg-type]

    def test_partial_recovered(self) -> None:
        outcome = SimpleNamespace(partial=None, normal_count=3, error_count=0)
        readers = [
            SimpleNamespace(sync_losses=2, empty_recording=False),
            SimpleNamespace(sync_losses=1, empty_recording=False),
        ]
        assert cli._classify_decode_success(outcome, readers) == EXIT_OK  # type: ignore[arg-type]

    def test_partial_unrecoverable(self) -> None:
        outcome = SimpleNamespace(partial=Path("out.csv.partial"), normal_count=1, error_count=0)
        readers = [SimpleNamespace(sync_losses=5, empty_recording=False)]
        assert cli._classify_decode_success(outcome, readers) == EXIT_OK  # type: ignore[arg-type]

    def test_empty_recording(self, caplog: pytest.LogCaptureFixture) -> None:
        # L1-EXIT-010: every input an empty recording + zero rows written →
        # the summary line names the empty-recording class.
        outcome = SimpleNamespace(partial=None, normal_count=0, error_count=0)
        readers = [SimpleNamespace(sync_losses=0, empty_recording=True)]
        with caplog.at_level("INFO"):
            assert cli._classify_decode_success(outcome, readers) == EXIT_OK  # type: ignore[arg-type]
        assert "empty-recording" in caplog.text


# ── merge output-collision check ────────────────────────────────────────────


class TestMergeOutputCollision:
    def test_collision_detected(self, tmp_path: Path) -> None:
        f = tmp_path / "a.mie"
        f.write_bytes(b"x")
        msg = cli._merge_output_collision(f, [tmp_path / "b.mie", f])
        assert msg is not None
        assert "resolves to merge input" in msg

    def test_no_collision_distinct_paths(self, tmp_path: Path) -> None:
        out = tmp_path / "out.csv"
        assert cli._merge_output_collision(out, [tmp_path / "a.mie", tmp_path / "b.mie"]) is None


class TestCheckMergeOutputCollision:
    def test_no_merge_skips(self) -> None:
        # A single-input decode (merge not requested) defers to the writer's own
        # input/output check, so this guard is a no-op.
        args = SimpleNamespace(output=Path("out.csv"))
        assert (
            cli._check_merge_output_collision(args, [Path("a.mie")], merge_requested=False) is None
        )

    def test_no_output_skips(self) -> None:
        args = SimpleNamespace(output=None)
        rc = cli._check_merge_output_collision(
            args, [Path("a.mie"), Path("b.mie")], merge_requested=True
        )
        assert rc is None

    def test_collision_returns_runtime(
        self, tmp_path: Path, capsys: pytest.CaptureFixture[str]
    ) -> None:
        f = tmp_path / "a.mie"
        f.write_bytes(b"x")
        args = SimpleNamespace(output=f)
        rc = cli._check_merge_output_collision(args, [tmp_path / "b.mie", f], merge_requested=True)
        assert rc == EXIT_RUNTIME
        assert "Error:" in capsys.readouterr().err
