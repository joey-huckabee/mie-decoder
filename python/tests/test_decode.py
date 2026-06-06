"""Unit tests for mie_decoder.decode module.

All expected values are derived from empirically validated binary data
cross-referenced against vendor-generated CSV output.
"""

from __future__ import annotations

import struct

import pytest

from mie_decoder.decode import (
    classify_message_format,
    decode_command_word,
    decode_irig_timestamp,
    decode_type_word,
    is_valid_message_type,
    read_u16,
    read_u16_array,
)
from mie_decoder.models import Bus, Direction


class TestDecodeTypeWord:
    """Tests for Type Word decoding."""

    @pytest.mark.requirement("L2-DEC-001")
    def test_receive_bus_a(self) -> None:
        """0x2402: msg_type=0x02, Bus A, WC=36, no error."""
        tw = decode_type_word(0x2402)
        assert tw.message_type == 0x02
        assert tw.bus == Bus.A
        assert tw.word_count == 36
        assert tw.error is False
        assert tw.raw == 0x2402

    @pytest.mark.requirement("L2-DEC-001")
    def test_receive_bus_a_wc17(self) -> None:
        """0x1102: msg_type=0x02, Bus A, WC=17, no error."""
        tw = decode_type_word(0x1102)
        assert tw.message_type == 0x02
        assert tw.word_count == 17

    @pytest.mark.requirement("L2-DEC-001")
    def test_transmit_bus_a(self) -> None:
        """0x2404: msg_type=0x04, Bus A, WC=36, no error."""
        tw = decode_type_word(0x2404)
        assert tw.message_type == 0x04
        assert tw.bus == Bus.A
        assert tw.word_count == 36

    @pytest.mark.requirement("L2-DEC-001")
    def test_transmit_bus_b(self) -> None:
        """0x2484: msg_type=0x04, Bus B, WC=36, no error."""
        tw = decode_type_word(0x2484)
        assert tw.message_type == 0x04
        assert tw.bus == Bus.B
        assert tw.word_count == 36

    @pytest.mark.requirement("L2-ERR-001")
    def test_error_flag_set(self) -> None:
        """Bit 14 set should indicate an error."""
        tw = decode_type_word(0x2402 | (1 << 14))
        assert tw.error is True
        assert tw.message_type == 0x02

    @pytest.mark.requirement("L2-ERR-001")
    def test_error_flag_clear(self) -> None:
        """All known test records have error=False."""
        for raw in (0x2402, 0x1102, 0x2404, 0x2484):
            tw = decode_type_word(raw)
            assert tw.error is False


class TestDecodeIrigTimestamp:
    """Tests for IRIG timestamp decoding."""

    @pytest.mark.requirement("L2-DEC-002")
    def test_known_good_timestamp(self) -> None:
        """Validated against CSV row: 192:15:54:50.456225."""
        ts = decode_irig_timestamp(0x180F, 0xDB26, 0xF621)
        assert ts.day == 192
        assert ts.hour == 15
        assert ts.minute == 54
        assert ts.second == 50
        assert ts.microsecond == 456225
        assert ts.freerun is False

    @pytest.mark.requirement("L2-WRT-011")
    def test_format_string(self) -> None:
        """Format should produce DAY:HH:MM:SS.uuuuuu."""
        ts = decode_irig_timestamp(0x180F, 0xDB26, 0xF621)
        assert ts.format() == "192:15:54:50.456225"

    @pytest.mark.requirement("L2-DEC-002")
    def test_second_timestamp(self) -> None:
        """Second record: 192:15:54:50.456504."""
        ts = decode_irig_timestamp(0x180F, 0xDB26, 0xF738)
        assert ts.hour == 15
        assert ts.minute == 54
        assert ts.second == 50
        assert ts.microsecond == 456504

    @pytest.mark.requirement("L2-DEC-002")
    def test_bus_b_timestamp(self) -> None:
        """Bus B file timestamp: 192:15:54:42.688899."""
        ts = decode_irig_timestamp(0x180F, 0xDAAA, 0x8303)
        assert ts.hour == 15
        assert ts.minute == 54
        assert ts.second == 42
        assert ts.microsecond == 688899

    @pytest.mark.requirement("L2-DEC-003")
    def test_freerun_flag(self) -> None:
        """Bit 15 set should indicate freerun mode."""
        ts = decode_irig_timestamp(0x980F, 0xDB26, 0xF621)
        assert ts.freerun is True
        assert ts.hour == 15  # other fields unaffected

    @pytest.mark.requirement("L2-DEC-002")
    def test_total_microseconds(self) -> None:
        """Verify total microseconds calculation."""
        ts = decode_irig_timestamp(0x180F, 0xDB26, 0xF621)
        expected = (192 * 86400 + 15 * 3600 + 54 * 60 + 50) * 1_000_000 + 456225
        assert ts.to_total_microseconds() == expected


class TestDecodeCommandWord:
    """Tests for MIL-STD-1553 Command Word decoding."""

    @pytest.mark.requirement("L2-DEC-004")
    def test_rt15_sa11_receive_wc30(self) -> None:
        """0x797E: RT15, Receive, SA11, WC=30."""
        cw = decode_command_word(0x797E)
        assert cw.rt == 15
        assert cw.direction == Direction.RECEIVE
        assert cw.subaddress == 11
        assert cw.data_word_count == 30
        assert cw.raw == 0x797E

    @pytest.mark.requirement("L2-DEC-004")
    def test_rt15_sa22_receive_wc11(self) -> None:
        """0x7ACB: RT15, Receive, SA22, WC=11."""
        cw = decode_command_word(0x7ACB)
        assert cw.rt == 15
        assert cw.direction == Direction.RECEIVE
        assert cw.subaddress == 22
        assert cw.data_word_count == 11

    @pytest.mark.requirement("L2-DEC-004")
    def test_rt15_sa22_transmit_wc30(self) -> None:
        """0x7EDE: RT15, Transmit, SA22, WC=30."""
        cw = decode_command_word(0x7EDE)
        assert cw.rt == 15
        assert cw.direction == Direction.TRANSMIT
        assert cw.subaddress == 22
        assert cw.data_word_count == 30

    @pytest.mark.requirement("L2-DEC-004")
    def test_rt30_sa11_transmit_wc30(self) -> None:
        """0xF57E: RT30, Transmit, SA11, WC=30."""
        cw = decode_command_word(0xF57E)
        assert cw.rt == 30
        assert cw.direction == Direction.TRANSMIT
        assert cw.subaddress == 11
        assert cw.data_word_count == 30

    @pytest.mark.requirement("L2-DEC-004")
    def test_rt15_sa10_transmit_wc30(self) -> None:
        """0x7D5E: RT15, Transmit, SA10, WC=30."""
        cw = decode_command_word(0x7D5E)
        assert cw.rt == 15
        assert cw.direction == Direction.TRANSMIT
        assert cw.subaddress == 10
        assert cw.data_word_count == 30

    @pytest.mark.requirement("L2-DEC-004")
    def test_wc_zero_means_32(self) -> None:
        """A raw word count of 0 should decode to 32."""
        # RT0, Receive, SA1, WC=0 → 0x0020
        cw = decode_command_word(0x0020)
        assert cw.data_word_count == 32


class TestReadU16:
    """Tests for raw byte reading utilities."""

    @pytest.mark.requirement("L2-DEC-008")
    def test_read_single(self) -> None:
        """Read a known LE value."""
        data = struct.pack("<H", 0x797E)
        assert read_u16(data, 0) == 0x797E

    @pytest.mark.requirement("L2-DEC-008")
    def test_read_at_offset(self) -> None:
        """Read at a non-zero offset."""
        data = b"\x00\x00" + struct.pack("<H", 0xABCD)
        assert read_u16(data, 2) == 0xABCD

    @pytest.mark.requirement("L2-DEC-008")
    def test_read_array(self) -> None:
        """Read multiple consecutive words."""
        data = struct.pack("<3H", 0x0001, 0x0002, 0x0003)
        result = read_u16_array(data, 0, 3)
        assert result == (0x0001, 0x0002, 0x0003)

    @pytest.mark.requirement("L2-DEC-008")
    def test_read_array_at_offset(self) -> None:
        """Read array starting at a non-zero offset."""
        data = b"\xFF\xFF" + struct.pack("<2H", 0x1234, 0x5678)
        result = read_u16_array(data, 2, 2)
        assert result == (0x1234, 0x5678)


class TestIsValidMessageType:
    """Tests for message type validation."""

    @pytest.mark.requirement("L2-MSG-001")
    def test_all_known_types(self) -> None:
        for code in (0x01, 0x02, 0x04, 0x08, 0x10, 0x18, 0x20):
            assert is_valid_message_type(code) is True

    @pytest.mark.requirement("L2-SYN-001")
    def test_unknown_types(self) -> None:
        for code in (0x00, 0x03, 0x05, 0x09, 0x0F, 0x11, 0x40, 0x7F):
            assert is_valid_message_type(code) is False


class TestClassifyMessageFormat:
    """Tests for classify_message_format."""

    from mie_decoder.models import MessageFormat

    @pytest.mark.requirement("L2-MSG-001")
    def test_bc_to_rt(self) -> None:
        """0x02 → RECEIVE regardless of command word."""
        cmd = decode_command_word(0x797E)  # RT15, Receive, SA11, WC30
        fmt = classify_message_format(0x02, cmd, 36)
        assert fmt == self.MessageFormat.RECEIVE

    @pytest.mark.requirement("L2-MSG-001")
    def test_rt_to_bc(self) -> None:
        """0x04 → TRANSMIT regardless of command word."""
        cmd = decode_command_word(0x7EDE)  # RT15, Transmit, SA22, WC30
        fmt = classify_message_format(0x04, cmd, 36)
        assert fmt == self.MessageFormat.TRANSMIT

    @pytest.mark.requirement("L2-MSG-001")
    def test_rt_to_rt(self) -> None:
        """0x08 → RT_TO_RT."""
        cmd = decode_command_word(0x797E)
        fmt = classify_message_format(0x08, cmd, 40)
        assert fmt == self.MessageFormat.RT_TO_RT

    @pytest.mark.requirement("L2-MSG-001")
    def test_broadcast_bc_to_rt(self) -> None:
        """0x10 → RECEIVE_BROADCAST."""
        cmd = decode_command_word(0xF97E)  # RT31, Receive
        fmt = classify_message_format(0x10, cmd, 35)
        assert fmt == self.MessageFormat.RECEIVE_BROADCAST

    @pytest.mark.requirement("L2-MSG-001")
    def test_broadcast_rt_to_rt(self) -> None:
        """0x18 → RT_TO_RT_BROADCAST."""
        cmd = decode_command_word(0xF97E)
        fmt = classify_message_format(0x18, cmd, 39)
        assert fmt == self.MessageFormat.RT_TO_RT_BROADCAST

    @pytest.mark.requirement("L2-MSG-001")
    def test_mode_code_tx_data(self) -> None:
        """0x01 with non-broadcast RT, T/R=1 → MODE_CODE_TX_DATA."""
        # RT15, Transmit, SA0 (mode code), mode code number in WC
        cmd = decode_command_word(0x7C01)  # RT15, T, SA0, WC=1
        fmt = classify_message_format(0x01, cmd, 7)
        assert fmt == self.MessageFormat.MODE_CODE_TX_DATA

    @pytest.mark.requirement("L2-MSG-001")
    def test_mode_code_rx_data(self) -> None:
        """0x01 with non-broadcast RT, T/R=0, WC>=7 → MODE_CODE_RX_DATA."""
        cmd = decode_command_word(0x7801)  # RT15, R, SA0, WC=1
        fmt = classify_message_format(0x01, cmd, 7)
        assert fmt == self.MessageFormat.MODE_CODE_RX_DATA

    @pytest.mark.requirement("L2-MSG-001")
    def test_mode_code_no_data(self) -> None:
        """0x01 with non-broadcast RT, T/R=0, WC=6 → MODE_CODE_NO_DATA."""
        cmd = decode_command_word(0x7801)  # RT15, R, SA0
        fmt = classify_message_format(0x01, cmd, 6)
        assert fmt == self.MessageFormat.MODE_CODE_NO_DATA

    @pytest.mark.requirement("L2-MSG-001")
    def test_mode_code_bcast_no_data(self) -> None:
        """0x01 with RT=31, WC=5 → MODE_CODE_BCAST_NO_DATA."""
        cmd = decode_command_word(0xF801)  # RT31, R, SA0
        fmt = classify_message_format(0x01, cmd, 5)
        assert fmt == self.MessageFormat.MODE_CODE_BCAST_NO_DATA

    @pytest.mark.requirement("L2-MSG-001")
    def test_mode_code_bcast_data(self) -> None:
        """0x01 with RT=31, WC=6 → MODE_CODE_BCAST_DATA."""
        cmd = decode_command_word(0xF801)  # RT31, R, SA0
        fmt = classify_message_format(0x01, cmd, 6)
        assert fmt == self.MessageFormat.MODE_CODE_BCAST_DATA

    @pytest.mark.requirement("L2-MSG-001")
    def test_spurious_data(self) -> None:
        """0x20 → SPURIOUS_DATA format."""
        cmd = decode_command_word(0x0000)
        fmt = classify_message_format(0x20, cmd, 10)
        assert fmt == self.MessageFormat.SPURIOUS_DATA


class TestDecodeStandardTimestamp:
    """Tests for Standard (32-bit) timestamp decoding."""

    @pytest.mark.requirement("L2-DEC-007")
    def test_basic_decode(self) -> None:
        from mie_decoder.decode import decode_standard_timestamp
        ts = decode_standard_timestamp(0x0001, 0x86A0)
        assert ts.raw_value == 0x000186A0
        assert ts.raw_value == 100000
        assert ts.upper_word == 0x0001
        assert ts.lower_word == 0x86A0

    @pytest.mark.requirement("L2-DEC-007")
    def test_zero(self) -> None:
        from mie_decoder.decode import decode_standard_timestamp
        ts = decode_standard_timestamp(0x0000, 0x0000)
        assert ts.raw_value == 0

    @pytest.mark.requirement("L2-DEC-007")
    def test_max_value(self) -> None:
        from mie_decoder.decode import decode_standard_timestamp
        ts = decode_standard_timestamp(0xFFFF, 0xFFFF)
        assert ts.raw_value == 0xFFFFFFFF

    @pytest.mark.requirement("L2-DEC-007")
    def test_format(self) -> None:
        from mie_decoder.decode import decode_standard_timestamp
        ts = decode_standard_timestamp(0x0001, 0x86A0)
        assert ts.format() == "0x000186A0"

    @pytest.mark.requirement("L2-DEC-007")
    def test_raw_ticks_returns_counter_value(self) -> None:
        from mie_decoder.decode import decode_standard_timestamp
        ts = decode_standard_timestamp(0x0001, 0x86A0)
        # Raw 32-bit counter value. Tick rate is card-dependent and not
        # encoded in the file, so this is not microseconds.
        assert ts.raw_ticks() == 100000

    @pytest.mark.requirement("L2-RDR-019")
    def test_to_microseconds_returns_none(self) -> None:
        from mie_decoder.decode import decode_standard_timestamp
        ts = decode_standard_timestamp(0x0001, 0x86A0)
        # Standard timestamps have no known microsecond basis under the
        # shared DELTA contract; callers must treat this as "no DELTA".
        assert ts.to_microseconds() is None


class TestDetectTimestampFormat:
    """Tests for auto-detection of timestamp format."""

    @pytest.mark.requirement("L2-DEC-011")
    def test_detects_irig_from_known_data(self) -> None:
        """Known IRIG record should detect as IRIG."""
        from tests.conftest import RECORD_RT15_SA11_RCV
        from mie_decoder.decode import detect_timestamp_format, decode_type_word, read_u16
        from mie_decoder.models import TimestampFormat

        tw = decode_type_word(read_u16(RECORD_RT15_SA11_RCV, 0))
        result = detect_timestamp_format(RECORD_RT15_SA11_RCV, 0, tw)
        assert result == TimestampFormat.IRIG

    @pytest.mark.requirement("L2-DEC-011")
    def test_detects_irig_from_transmit(self) -> None:
        """Known IRIG transmit record should detect as IRIG."""
        from tests.conftest import RECORD_RT15_SA22_XMT
        from mie_decoder.decode import detect_timestamp_format, decode_type_word, read_u16
        from mie_decoder.models import TimestampFormat

        tw = decode_type_word(read_u16(RECORD_RT15_SA22_XMT, 0))
        result = detect_timestamp_format(RECORD_RT15_SA22_XMT, 0, tw)
        assert result == TimestampFormat.IRIG

    @pytest.mark.requirement("L2-DEC-013")
    def test_forced_irig(self, tmp_mie_file: Path) -> None:
        """Forcing IRIG should still decode correctly."""
        from mie_decoder.reader import MieFileReader
        from mie_decoder.models import TimestampFormat

        reader = MieFileReader(tmp_mie_file, time_format=TimestampFormat.IRIG)
        messages = list(reader)
        assert len(messages) == 3
        assert messages[0].timestamp.format() == "192:15:54:50.456225"

    @pytest.mark.requirement("L2-DEC-013")
    def test_cli_time_format_irig(self, tmp_mie_file: Path, tmp_path: Path) -> None:
        """CLI --time-format irig should work."""
        from mie_decoder.cli import main

        out = tmp_path / "irig.csv"
        rc = main(["decode", str(tmp_mie_file), "-o", str(out), "--time-format", "irig"])
        assert rc == 0
        assert out.exists()


class TestStructuralInvariants:
    """L2-SYN-020/021/022 structural invariants."""

    @staticmethod
    def _tw(message_type: int, word_count: int):
        from mie_decoder.models import Bus, TypeWord
        return TypeWord(
            message_type=message_type,
            bus=Bus.A,
            word_count=word_count,
            error=False,
            raw=0,
        )

    @staticmethod
    def _cmd(direction, dwc: int):
        from mie_decoder.models import CommandWord
        return CommandWord(
            rt=15,
            direction=direction,
            subaddress=11,
            data_word_count=dwc,
            raw=0,
        )

    @pytest.mark.requirement("L2-SYN-020")
    def test_canonical_bc_to_rt_passes(self) -> None:
        from mie_decoder.decode import validate_structural_invariants
        from mie_decoder.models import Direction, MessageFormat
        result = validate_structural_invariants(
            self._tw(0x02, 36), self._cmd(Direction.RECEIVE, 30),
            MessageFormat.RECEIVE, 3,
        )
        assert result is None

    @pytest.mark.requirement("L2-SYN-021")
    def test_canonical_rt_to_bc_passes(self) -> None:
        from mie_decoder.decode import validate_structural_invariants
        from mie_decoder.models import Direction, MessageFormat
        result = validate_structural_invariants(
            self._tw(0x04, 36), self._cmd(Direction.TRANSMIT, 30),
            MessageFormat.TRANSMIT, 3,
        )
        assert result is None

    @pytest.mark.requirement("L2-SYN-020")
    def test_bc_to_rt_with_transmit_cmd_rejected(self) -> None:
        from mie_decoder.decode import (
            WhichInvariant, validate_structural_invariants,
        )
        from mie_decoder.models import Direction, MessageFormat
        result = validate_structural_invariants(
            self._tw(0x02, 36), self._cmd(Direction.TRANSMIT, 30),
            MessageFormat.RECEIVE, 3,
        )
        assert result is not None
        assert result.kind == WhichInvariant.DIRECTION_BC_TO_RT

    @pytest.mark.requirement("L2-SYN-021")
    def test_rt_to_bc_with_receive_cmd_rejected(self) -> None:
        from mie_decoder.decode import (
            WhichInvariant, validate_structural_invariants,
        )
        from mie_decoder.models import Direction, MessageFormat
        result = validate_structural_invariants(
            self._tw(0x04, 36), self._cmd(Direction.RECEIVE, 30),
            MessageFormat.TRANSMIT, 3,
        )
        assert result is not None
        assert result.kind == WhichInvariant.DIRECTION_RT_TO_BC

    @pytest.mark.requirement("L2-SYN-022")
    def test_capacity_short_rejected(self) -> None:
        from mie_decoder.decode import (
            WhichInvariant, validate_structural_invariants,
        )
        from mie_decoder.models import Direction, MessageFormat
        # wc=5 too small for Receive with dwc=30 (needs 1+3+1+31=36)
        result = validate_structural_invariants(
            self._tw(0x02, 5), self._cmd(Direction.RECEIVE, 30),
            MessageFormat.RECEIVE, 3,
        )
        assert result is not None
        assert result.kind == WhichInvariant.WORD_COUNT_CAPACITY

    @pytest.mark.requirement("L2-SYN-022")
    def test_capacity_exact_accepted(self) -> None:
        from mie_decoder.decode import validate_structural_invariants
        from mie_decoder.models import Direction, MessageFormat
        # wc=36 is exactly minimum for Receive with dwc=30
        result = validate_structural_invariants(
            self._tw(0x02, 36), self._cmd(Direction.RECEIVE, 30),
            MessageFormat.RECEIVE, 3,
        )
        assert result is None

    @pytest.mark.requirement("L2-SYN-022")
    def test_spurious_skips_capacity_check(self) -> None:
        from mie_decoder.decode import validate_structural_invariants
        from mie_decoder.models import Direction, MessageFormat
        # SpuriousData has variable payload — capacity check skipped
        result = validate_structural_invariants(
            self._tw(0x20, 5), self._cmd(Direction.RECEIVE, 0),
            MessageFormat.SPURIOUS_DATA, 3,
        )
        assert result is None

    @pytest.mark.requirement("L2-SYN-020")
    def test_mode_code_directions_not_constrained(self) -> None:
        from mie_decoder.decode import validate_structural_invariants
        from mie_decoder.models import Direction, MessageFormat
        tw = self._tw(0x01, 7)
        # Mode codes accept either direction.
        for dir_ in (Direction.TRANSMIT, Direction.RECEIVE):
            fmt = (
                MessageFormat.MODE_CODE_TX_DATA if dir_ == Direction.TRANSMIT
                else MessageFormat.MODE_CODE_RX_DATA
            )
            result = validate_structural_invariants(
                tw, self._cmd(dir_, 1), fmt, 3,
            )
            assert result is None, f"unexpected violation for {dir_}: {result}"


class TestPostExtractInvariants:
    """L2-SYN-023 (Cmd2 direction for RT-to-RT)."""

    @staticmethod
    def _cmd(direction, raw: int = 0):
        from mie_decoder.models import CommandWord
        return CommandWord(
            rt=5, direction=direction, subaddress=10,
            data_word_count=3, raw=raw,
        )

    @pytest.mark.requirement("L2-SYN-023")
    def test_rt_to_rt_cmd2_receive_passes(self) -> None:
        from mie_decoder.decode import validate_post_extract_invariants
        from mie_decoder.models import Direction, MessageFormat
        result = validate_post_extract_invariants(
            MessageFormat.RT_TO_RT, self._cmd(Direction.RECEIVE),
        )
        assert result is None

    @pytest.mark.requirement("L2-SYN-023")
    def test_rt_to_rt_cmd2_transmit_rejected(self) -> None:
        from mie_decoder.decode import (
            InvariantSeverity, WhichInvariant, validate_post_extract_invariants,
        )
        from mie_decoder.models import Direction, MessageFormat
        result = validate_post_extract_invariants(
            MessageFormat.RT_TO_RT,
            self._cmd(Direction.TRANSMIT, raw=0xABCD),
        )
        assert result is not None
        assert result.kind == WhichInvariant.DIRECTION_RT_TO_RT_CMD2
        assert result.severity == InvariantSeverity.REJECT

    @pytest.mark.requirement("L2-SYN-023")
    def test_rt_to_rt_broadcast_also_checked(self) -> None:
        from mie_decoder.decode import (
            WhichInvariant, validate_post_extract_invariants,
        )
        from mie_decoder.models import Direction, MessageFormat
        result = validate_post_extract_invariants(
            MessageFormat.RT_TO_RT_BROADCAST,
            self._cmd(Direction.TRANSMIT),
        )
        assert result is not None
        assert result.kind == WhichInvariant.DIRECTION_RT_TO_RT_CMD2

    @pytest.mark.requirement("L2-SYN-023")
    def test_non_rt_to_rt_is_noop(self) -> None:
        from mie_decoder.decode import validate_post_extract_invariants
        from mie_decoder.models import Direction, MessageFormat
        # No cmd2 for non-RT-to-RT
        assert validate_post_extract_invariants(MessageFormat.RECEIVE, None) is None
        # Even with stray Cmd2 (shouldn't happen), no enforcement
        assert validate_post_extract_invariants(
            MessageFormat.RECEIVE, self._cmd(Direction.TRANSMIT),
        ) is None


class TestRecordAnomalies:
    """L2-SYN-024 / L2-SYN-025: anomaly detectors."""

    @staticmethod
    def _tw(raw: int, message_type: int = 0x02, wc: int = 36):
        from mie_decoder.models import Bus, TypeWord
        return TypeWord(
            message_type=message_type, bus=Bus.A, word_count=wc,
            error=False, raw=raw,
        )

    @staticmethod
    def _cmd(rt: int = 15):
        from mie_decoder.models import CommandWord, Direction
        return CommandWord(
            rt=rt, direction=Direction.RECEIVE, subaddress=11,
            data_word_count=30, raw=0,
        )

    @pytest.mark.requirement("L2-SYN-024")
    def test_status_rt_match_no_violation(self) -> None:
        from mie_decoder.decode import detect_record_anomalies
        # RT=15 in Cmd; Status raw 0x7800 → bits 15-11 = 15
        anomalies = detect_record_anomalies(self._tw(0x2402), self._cmd(15), 0x7800)
        assert anomalies == []

    @pytest.mark.requirement("L2-SYN-024")
    def test_status_rt_mismatch_anomaly(self) -> None:
        from mie_decoder.decode import (
            InvariantSeverity, WhichInvariant, detect_record_anomalies,
        )
        # Cmd RT=15, Status 0x2800 → status RT=5
        anomalies = detect_record_anomalies(self._tw(0x2402), self._cmd(15), 0x2800)
        assert len(anomalies) == 1
        assert anomalies[0].kind == WhichInvariant.STATUS_RT_MISMATCH
        assert anomalies[0].severity == InvariantSeverity.ANOMALY_WARN

    @pytest.mark.requirement("L2-SYN-024")
    def test_no_status_no_violation(self) -> None:
        from mie_decoder.decode import detect_record_anomalies
        anomalies = detect_record_anomalies(self._tw(0x2402), self._cmd(15), None)
        assert anomalies == []

    @pytest.mark.requirement("L2-SYN-025")
    def test_type_word_reserved_bit_anomaly(self) -> None:
        from mie_decoder.decode import (
            InvariantSeverity, WhichInvariant, detect_record_anomalies,
        )
        anomalies = detect_record_anomalies(self._tw(0x8402), self._cmd(15), None)
        assert len(anomalies) == 1
        assert anomalies[0].kind == WhichInvariant.TYPE_WORD_RESERVED_BIT
        assert anomalies[0].severity == InvariantSeverity.ANOMALY_WARN

    @pytest.mark.requirement("L2-SYN-025")
    def test_multiple_anomalies_can_fire_on_one_record(self) -> None:
        from mie_decoder.decode import detect_record_anomalies
        # Both status RT mismatch AND TW bit 15 set: 2 anomalies
        anomalies = detect_record_anomalies(self._tw(0xA402), self._cmd(15), 0x2800)
        assert len(anomalies) == 2
