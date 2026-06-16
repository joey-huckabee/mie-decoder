"""Tests for mie_decoder.config and mie_decoder.filters modules."""

from __future__ import annotations

from pathlib import Path

import pytest

from mie_decoder.config import (
    DecoderConfig,
    FilterConfig,
    load_config,
    _parse_type_names,
    _parse_bus_names,
)
from mie_decoder.filters import apply_filters
from mie_decoder.models import (
    Bus,
    CommandWord,
    Direction,
    IrigTimestamp,
    MessageFormat,
    MieMessage,
    TimestampFormat,
    TypeWord,
)


def _make_msg(
    msg_type: int = 0x02,
    bus: Bus = Bus.A,
    rt: int = 15,
    sa: int = 11,
    direction: Direction = Direction.RECEIVE,
) -> MieMessage:
    """Helper to create a minimal MieMessage for filter testing."""
    return MieMessage(
        timestamp=IrigTimestamp(192, 15, 54, 50, 456225, False),
        type_word=TypeWord(msg_type, bus, 36, False, 0x2402),
        message_format=MessageFormat.RECEIVE,
        command_word=CommandWord(rt, direction, sa, 30, 0x797E),
        command_word_2=None,
        status_word=0x7800,
        status_word_2=None,
        data_words=(0x0400,),
        error_word=None,
        delta=0.0,
        file_offset=0,
    )


class TestFilterConfig:
    """Tests for FilterConfig."""

    @pytest.mark.requirement("L2-FLT-001")
    def test_no_filters_active(self) -> None:
        fc = FilterConfig()
        assert fc.is_active is False

    @pytest.mark.requirement("L2-CFG-006")
    def test_type_filter_active(self) -> None:
        fc = FilterConfig(exclude_types={0x20})
        assert fc.is_active is True

    @pytest.mark.requirement("L2-CFG-006")
    def test_should_exclude_by_type(self) -> None:
        fc = FilterConfig(exclude_types={0x20})
        assert fc.should_exclude(0x20, 15, Bus.A, 11) is True
        assert fc.should_exclude(0x02, 15, Bus.A, 11) is False

    @pytest.mark.requirement("L2-CFG-006")
    def test_should_exclude_by_rt(self) -> None:
        fc = FilterConfig(exclude_rts={31})
        assert fc.should_exclude(0x02, 31, Bus.A, 11) is True
        assert fc.should_exclude(0x02, 15, Bus.A, 11) is False

    @pytest.mark.requirement("L2-CFG-006")
    def test_should_exclude_by_bus(self) -> None:
        fc = FilterConfig(exclude_buses={Bus.B})
        assert fc.should_exclude(0x02, 15, Bus.B, 11) is True
        assert fc.should_exclude(0x02, 15, Bus.A, 11) is False

    @pytest.mark.requirement("L2-CFG-006")
    def test_should_exclude_by_subaddress(self) -> None:
        fc = FilterConfig(exclude_subaddresses={0, 31})
        assert fc.should_exclude(0x02, 15, Bus.A, 0) is True
        assert fc.should_exclude(0x02, 15, Bus.A, 31) is True
        assert fc.should_exclude(0x02, 15, Bus.A, 11) is False

    @pytest.mark.requirement("L2-FLT-002")
    def test_or_logic(self) -> None:
        """Message matching ANY criterion should be excluded."""
        fc = FilterConfig(exclude_types={0x20}, exclude_rts={31})
        assert fc.should_exclude(0x02, 31, Bus.A, 11) is True
        assert fc.should_exclude(0x20, 15, Bus.A, 11) is True
        assert fc.should_exclude(0x02, 15, Bus.A, 11) is False


class TestParseTypeNames:
    """Tests for type name parsing."""

    @pytest.mark.requirement("L2-CFG-007")
    def test_by_name(self) -> None:
        result = _parse_type_names(["BC_TO_RT", "RT_TO_BC"])
        assert result == {0x02, 0x04}

    @pytest.mark.requirement("L2-CFG-007")
    def test_case_insensitive(self) -> None:
        result = _parse_type_names(["bc_to_rt"])
        assert result == {0x02}

    @pytest.mark.requirement("L2-CFG-007")
    def test_by_hex(self) -> None:
        result = _parse_type_names(["0x02", "0x20"])
        assert result == {0x02, 0x20}

    @pytest.mark.requirement("L2-CFG-007")
    def test_invalid_name_raises(self) -> None:
        with pytest.raises(ValueError, match="Unknown"):
            _parse_type_names(["NONEXISTENT"])

    @pytest.mark.requirement("L2-CFG-007")
    def test_invalid_hex_raises(self) -> None:
        with pytest.raises(ValueError, match="Invalid hex"):
            _parse_type_names(["0xZZ"])


class TestParseBusNames:
    """Tests for bus name parsing."""

    @pytest.mark.requirement("L2-CFG-006")
    def test_valid(self) -> None:
        result = _parse_bus_names(["A", "B"])
        assert result == {Bus.A, Bus.B}

    @pytest.mark.requirement("L2-CFG-006")
    def test_case_insensitive(self) -> None:
        result = _parse_bus_names(["a", "b"])
        assert result == {Bus.A, Bus.B}

    @pytest.mark.requirement("L2-CFG-006")
    def test_invalid_raises(self) -> None:
        with pytest.raises(ValueError, match="Invalid bus"):
            _parse_bus_names(["C"])


class TestDecoderConfig:
    """Tests for DecoderConfig."""

    @pytest.mark.requirement("L2-CFG-003")
    def test_defaults(self) -> None:
        config = DecoderConfig()
        assert config.log_level == "WARNING"
        assert config.time_format == TimestampFormat.AUTO
        assert config.strict is False
        assert config.filters.is_active is False

    @pytest.mark.requirement("L2-CFG-003")
    def test_with_overrides(self) -> None:
        config = DecoderConfig()
        updated = config.with_overrides(
            log_level="DEBUG",
            exclude_types={0x20},
        )
        assert updated.log_level == "DEBUG"
        assert 0x20 in updated.filters.exclude_types
        assert config.log_level == "WARNING"  # original unchanged

    @pytest.mark.requirement("L2-CFG-004")
    def test_overrides_merge_filters(self) -> None:
        """CLI filters should merge with config file filters."""
        config = DecoderConfig(
            filters=FilterConfig(exclude_types={0x20})
        )
        updated = config.with_overrides(exclude_types={0x01})
        assert updated.filters.exclude_types == {0x20, 0x01}


class TestLoadConfig:
    """Tests for TOML config loading."""

    @pytest.mark.requirement("L2-CFG-001")
    def test_none_returns_defaults(self) -> None:
        config = load_config(None)
        assert config.log_level == "WARNING"

    @pytest.mark.requirement("L2-CFG-005")
    def test_missing_file_raises(self) -> None:
        with pytest.raises(FileNotFoundError):
            load_config("/nonexistent/config.toml")

    @pytest.mark.requirement("L3-PY-005")
    def test_load_valid_toml(self, tmp_path: Path) -> None:
        cfg_file = tmp_path / "test.toml"
        cfg_file.write_text(
            '[logging]\nlevel = "DEBUG"\n\n'
            '[filter]\nexclude_types = ["SPURIOUS_DATA"]\n'
            'exclude_rts = [31]\n'
        )
        config = load_config(cfg_file)
        assert config.log_level == "DEBUG"
        assert 0x20 in config.filters.exclude_types
        assert 31 in config.filters.exclude_rts

    @pytest.mark.requirement("L2-DEC-013")
    def test_load_with_time_format(self, tmp_path: Path) -> None:
        cfg_file = tmp_path / "tf.toml"
        cfg_file.write_text('[decode]\ntime_format = "irig"\n')
        config = load_config(cfg_file)
        assert config.time_format == TimestampFormat.IRIG

    @pytest.mark.requirement("L2-CFG-010")
    def test_invalid_time_format_raises(self, tmp_path: Path) -> None:
        cfg_file = tmp_path / "bad_tf.toml"
        cfg_file.write_text('[decode]\ntime_format = "bogus"\n')
        with pytest.raises(ValueError, match="Invalid time_format"):
            load_config(cfg_file)


class TestApplyFilters:
    """Tests for the filter generator wrapper."""

    @pytest.mark.requirement("L2-FLT-001")
    def test_no_filters_passes_all(self) -> None:
        msgs = [_make_msg(), _make_msg(), _make_msg()]
        fc = FilterConfig()
        result = list(apply_filters(msgs, fc))
        assert len(result) == 3

    @pytest.mark.requirement("L2-FLT-001")
    def test_exclude_by_type(self) -> None:
        msgs = [
            _make_msg(msg_type=0x02),
            _make_msg(msg_type=0x04),
            _make_msg(msg_type=0x20),
        ]
        fc = FilterConfig(exclude_types={0x20})
        result = list(apply_filters(msgs, fc))
        assert len(result) == 2
        assert all(m.type_word.message_type != 0x20 for m in result)

    @pytest.mark.requirement("L2-FLT-001")
    def test_exclude_by_rt(self) -> None:
        msgs = [_make_msg(rt=15), _make_msg(rt=30), _make_msg(rt=31)]
        fc = FilterConfig(exclude_rts={31})
        result = list(apply_filters(msgs, fc))
        assert len(result) == 2

    @pytest.mark.requirement("L2-FLT-001")
    def test_exclude_by_bus(self) -> None:
        msgs = [_make_msg(bus=Bus.A), _make_msg(bus=Bus.B)]
        fc = FilterConfig(exclude_buses={Bus.B})
        result = list(apply_filters(msgs, fc))
        assert len(result) == 1
        assert result[0].bus == Bus.A

    @pytest.mark.requirement("L2-FLT-001")
    def test_exclude_all(self) -> None:
        msgs = [_make_msg(rt=15), _make_msg(rt=15)]
        fc = FilterConfig(exclude_rts={15})
        result = list(apply_filters(msgs, fc))
        assert len(result) == 0

    @pytest.mark.requirement("L2-FLT-001")
    def test_spurious_without_command_word_survives_rt_filter(self) -> None:
        """SPURIOUS_DATA records have no Command Word (rt/subaddress are
        None). An RT or subaddress filter must not match them — and must
        not raise on the None Command Word — mirroring the Rust filter.
        They remain excludable by type or bus.
        """
        spurious = MieMessage(
            timestamp=IrigTimestamp(192, 15, 54, 50, 456225, False),
            type_word=TypeWord(0x20, Bus.A, 12, False, 0x2420),
            message_format=MessageFormat.SPURIOUS_DATA,
            command_word=None,
            command_word_2=None,
            status_word=None,
            status_word_2=None,
            data_words=(0x1234,),
            error_word=0x2001,
            delta=None,
            file_offset=0,
        )
        # RT/subaddress filters never match a None Command Word: the
        # spurious record passes through (and does not raise).
        assert list(apply_filters([spurious], FilterConfig(exclude_rts={15}))) == [spurious]
        assert list(
            apply_filters([spurious], FilterConfig(exclude_subaddresses={11}))
        ) == [spurious]
        # But type and bus filters still apply to it.
        assert list(apply_filters([spurious], FilterConfig(exclude_types={0x20}))) == []
        assert list(apply_filters([spurious], FilterConfig(exclude_buses={Bus.A}))) == []

    @pytest.mark.requirement("L2-FLT-001")
    def test_end_to_end_with_reader(self, tmp_mie_file: Path) -> None:
        """Filter should work with actual MieFileReader output."""
        from mie_decoder.reader import MieFileReader

        reader = MieFileReader(tmp_mie_file)
        # Exclude RT 15 SA 22 — should drop the 2nd and 3rd records
        fc = FilterConfig(exclude_subaddresses={22})
        result = list(apply_filters(reader, fc))
        assert len(result) == 1
        assert result[0].msg_label == "11R"


class TestCliFilters:
    """CLI integration tests for filtering."""

    @pytest.mark.requirement("L2-CLI-010")
    def test_exclude_types_cli(self, tmp_mie_file: Path, tmp_path: Path) -> None:
        from mie_decoder.cli import main

        out = tmp_path / "filtered.csv"
        # Exclude RT_TO_BC (type 0x04) — should remove 3rd record
        rc = main([
            "decode", str(tmp_mie_file),
            "-o", str(out),
            "--exclude-types", "RT_TO_BC",
        ])
        assert rc == 0
        lines = out.read_text().strip().split("\n")
        assert len(lines) == 3  # header + 2 data rows

    @pytest.mark.requirement("L2-CLI-010")
    def test_exclude_rts_cli(self, tmp_mie_file: Path, tmp_path: Path) -> None:
        from mie_decoder.cli import main

        out = tmp_path / "rt_filtered.csv"
        # All records are RT 15, so excluding it should leave 0 data rows
        rc = main([
            "decode", str(tmp_mie_file),
            "-o", str(out),
            "--exclude-rts", "15",
        ])
        assert rc == 0
        lines = out.read_text().strip().split("\n")
        assert len(lines) == 1  # header only, no data rows

    @pytest.mark.requirement("L2-CFG-004")
    def test_config_file_cli(self, tmp_mie_file: Path, tmp_path: Path) -> None:
        from mie_decoder.cli import main

        cfg = tmp_path / "test.toml"
        cfg.write_text('[filter]\nexclude_types = ["RT_TO_BC"]\n')
        out = tmp_path / "cfg_filtered.csv"
        rc = main([
            "decode", str(tmp_mie_file),
            "-o", str(out),
            "--config", str(cfg),
        ])
        assert rc == 0
        lines = out.read_text().strip().split("\n")
        assert len(lines) == 3  # header + 2 (RT_TO_BC excluded)


class TestErrorModeConfig:
    """Tests for error mode configuration."""

    @pytest.mark.requirement("L2-CFG-001")
    def test_default_is_separate(self) -> None:
        from mie_decoder.models import ErrorMode
        config = DecoderConfig()
        assert config.error_mode == ErrorMode.SEPARATE

    @pytest.mark.requirement("L2-CFG-001")
    def test_override_to_inline(self) -> None:
        from mie_decoder.models import ErrorMode
        config = DecoderConfig()
        updated = config.with_overrides(error_mode=ErrorMode.INLINE)
        assert updated.error_mode == ErrorMode.INLINE

    @pytest.mark.requirement("L2-ERR-011")
    def test_load_from_toml(self, tmp_path: Path) -> None:
        from mie_decoder.models import ErrorMode
        cfg = tmp_path / "em.toml"
        cfg.write_text('[decode]\nerror_mode = "inline"\n')
        config = load_config(cfg)
        assert config.error_mode == ErrorMode.INLINE

    @pytest.mark.requirement("L2-CFG-010")
    def test_invalid_error_mode_raises(self, tmp_path: Path) -> None:
        cfg = tmp_path / "bad_em.toml"
        cfg.write_text('[decode]\nerror_mode = "bogus"\n')
        with pytest.raises(ValueError, match="Invalid error_mode"):
            load_config(cfg)

    @pytest.mark.requirement("L3-PY-011")
    def test_cli_error_mode_inline(self, tmp_mie_file: Path, tmp_path: Path) -> None:
        from mie_decoder.cli import main

        out = tmp_path / "inline.csv"
        rc = main(["decode", str(tmp_mie_file), "-o", str(out), "--error-mode", "inline"])
        assert rc == 0
        assert out.exists()

    @pytest.mark.requirement("L2-ERR-008")
    def test_cli_error_mode_separate(self, tmp_mie_file: Path, tmp_path: Path) -> None:
        from mie_decoder.cli import main

        out = tmp_path / "main.csv"
        rc = main(["decode", str(tmp_mie_file), "-o", str(out), "--error-mode", "separate"])
        assert rc == 0
        assert out.exists()
        # No errors in test data, so error file should not be created
        error_file = tmp_path / "main_errors.csv"
        assert not error_file.exists()


class TestSchemaValidation:
    """L2-CFG schema validation tests."""

    @pytest.mark.requirement("L2-CFG-010")
    def test_unknown_log_level_rejected(self, tmp_path: Path) -> None:
        cfg = tmp_path / "bad.toml"
        cfg.write_text("[logging]\nlevel = \"NOPE\"\n")
        with pytest.raises(ValueError, match="logging"):
            load_config(cfg)

    @pytest.mark.requirement("L2-CFG-010")
    def test_known_log_levels_accepted_case_insensitively(self, tmp_path: Path) -> None:
        for level in ["DEBUG", "info", "Warning", "WARN", "error", "CRITICAL"]:
            cfg = tmp_path / f"l_{level}.toml"
            cfg.write_text(f"[logging]\nlevel = \"{level}\"\n")
            config = load_config(cfg)
            assert config.log_level == level.upper()

    @pytest.mark.requirement("L2-CFG-010")
    def test_unknown_output_format_rejected(self, tmp_path: Path) -> None:
        cfg = tmp_path / "bad_fmt.toml"
        cfg.write_text("[output]\nformat = \"json\"\n")
        with pytest.raises(ValueError, match="output.format"):
            load_config(cfg)

    @pytest.mark.requirement("L2-CFG-010")
    def test_output_format_csv_accepted(self, tmp_path: Path) -> None:
        cfg = tmp_path / "ok_fmt.toml"
        cfg.write_text("[output]\nformat = \"csv\"\n")
        config = load_config(cfg)
        assert config.output_format == "csv"

    @pytest.mark.requirement("L2-CFG-010")
    def test_strict_must_be_bool(self, tmp_path: Path) -> None:
        # TOML supports bool natively. A string here is rejected by
        # tomllib at parse time (TypeError), so we test the dataclass
        # path instead.
        from mie_decoder.config import _require_bool
        with pytest.raises(ValueError, match="expected boolean"):
            _require_bool("decode", "strict", "yes")

    @pytest.mark.requirement("L2-CFG-010")
    def test_exclude_rts_out_of_range_rejected(self, tmp_path: Path) -> None:
        cfg = tmp_path / "rt_high.toml"
        cfg.write_text("[filter]\nexclude_rts = [32]\n")
        with pytest.raises(ValueError, match=r"\[0, 31\]"):
            load_config(cfg)

    @pytest.mark.requirement("L2-CFG-010")
    def test_exclude_subaddresses_negative_rejected(self, tmp_path: Path) -> None:
        cfg = tmp_path / "sa_neg.toml"
        cfg.write_text("[filter]\nexclude_subaddresses = [-1]\n")
        with pytest.raises(ValueError, match=r"\[0, 31\]"):
            load_config(cfg)

    @pytest.mark.requirement("L2-CFG-010")
    def test_exclude_rts_boundary_values_accepted(self, tmp_path: Path) -> None:
        cfg = tmp_path / "rt_bounds.toml"
        cfg.write_text("[filter]\nexclude_rts = [0, 31]\n")
        config = load_config(cfg)
        assert config.filters.exclude_rts == {0, 31}

    @pytest.mark.requirement("L2-CFG-011")
    @pytest.mark.requirement("L2-DEC-017")
    def test_standard_tick_rate_hz_default_is_none(self, tmp_path: Path) -> None:
        cfg = tmp_path / "std.toml"
        cfg.write_text("[decode]\ntime_format = \"standard\"\n")
        config = load_config(cfg)
        assert config.standard_tick_rate_hz is None

    @pytest.mark.requirement("L2-CFG-011")
    def test_standard_tick_rate_hz_accepts_float_and_int(self, tmp_path: Path) -> None:
        cfg = tmp_path / "std_float.toml"
        cfg.write_text("[decode]\nstandard_tick_rate_hz = 1000000.0\n")
        assert load_config(cfg).standard_tick_rate_hz == 1_000_000.0
        # A bare integer is accepted and coerced to float.
        cfg2 = tmp_path / "std_int.toml"
        cfg2.write_text("[decode]\nstandard_tick_rate_hz = 1000000\n")
        assert load_config(cfg2).standard_tick_rate_hz == 1_000_000.0

    @pytest.mark.requirement("L2-CFG-011")
    @pytest.mark.requirement("L2-CFG-010")
    def test_standard_tick_rate_hz_rejects_nonpositive(self, tmp_path: Path) -> None:
        for bad in ["0", "0.0", "-1.0"]:
            cfg = tmp_path / f"std_bad_{bad}.toml"
            cfg.write_text(f"[decode]\nstandard_tick_rate_hz = {bad}\n")
            with pytest.raises(ValueError, match="standard_tick_rate_hz"):
                load_config(cfg)

    @pytest.mark.requirement("L2-CFG-003")
    def test_standard_tick_rate_hz_override_applies(self) -> None:
        config = load_config(None).with_overrides(standard_tick_rate_hz=2_000_000.0)
        assert config.standard_tick_rate_hz == 2_000_000.0

    @pytest.mark.requirement("L2-CFG-009")
    def test_unknown_top_level_key_is_warned_not_rejected(
        self, tmp_path: Path, caplog: pytest.LogCaptureFixture,
    ) -> None:
        cfg = tmp_path / "unknown.toml"
        cfg.write_text(
            "[output]\nformat = \"csv\"\nunknown_thing = true\n"
        )
        import logging
        with caplog.at_level(logging.WARNING, logger="mie_decoder.config"):
            config = load_config(cfg)
        assert config.output_format == "csv"
        # The WARN should mention the offending key.
        assert any("unknown_thing" in rec.getMessage() for rec in caplog.records)

    @pytest.mark.requirement("L2-CFG-009")
    def test_unknown_filter_key_warned_not_rejected(
        self, tmp_path: Path, caplog: pytest.LogCaptureFixture,
    ) -> None:
        # Common typo: exclude_subdresses (missing "ad").
        cfg = tmp_path / "typo.toml"
        cfg.write_text("[filter]\nexclude_subdresses = [0]\n")
        import logging
        with caplog.at_level(logging.WARNING, logger="mie_decoder.config"):
            config = load_config(cfg)
        assert config.filters.exclude_subaddresses == set()
        assert any(
            "exclude_subdresses" in rec.getMessage() for rec in caplog.records
        )


class TestSharedDefaultConfig:
    """The repo-root config/default.toml is a shared artifact; the Python
    loader must accept it and resolve every documented key (L2-CFG-001).
    Guards against the starter file drifting into something only one
    implementation can parse."""

    @pytest.mark.requirement("L2-CFG-001")
    def test_loads_shared_default_toml(self) -> None:
        default_toml = (
            Path(__file__).resolve().parents[2] / "config" / "default.toml"
        )
        if not default_toml.exists():
            pytest.skip("config/default.toml not present in this checkout")
        cfg = load_config(default_toml)
        # The four keys added to keep the starter file complete must parse to
        # their documented defaults under the Python loader too.
        assert cfg.allow_partial is False
        assert cfg.no_clobber is False
        assert cfg.detect_records == 8
        assert cfg.lookahead_records == 2
        assert cfg.output_format == "csv"
