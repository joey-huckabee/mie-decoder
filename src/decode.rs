//! Pure binary → struct decoders for MIE record fields.
//!
//! All functions operate on `&[u8]` slices and use little-endian encoding,
//! matching `struct.unpack_from("<H", ...)` in the Python reference.

use crate::error::{MieError, MieResult};
use crate::models::{
    Bus, CommandWord, Direction, IrigTimestamp, MessageFormat, MessageType, StandardTimestamp,
    TimestampFormat, TypeWord, is_valid_message_type,
};

/// Minimum record size in bytes for IRIG: Type(2) + TS(6) + Cmd(2) = 10
pub const MIN_RECORD_BYTES: usize = 10;
/// Minimum record size in bytes for Standard: Type(2) + TS(4) + Cmd(2) = 8
pub const MIN_RECORD_BYTES_STANDARD: usize = 8;
/// Minimum record size in 16-bit words for IRIG: Type(1) + TS(3) + Cmd(1) = 5
pub const MIN_RECORD_WORDS: u16 = 5;
/// Minimum record size in 16-bit words for Standard: Type(1) + TS(2) + Cmd(1) = 4
pub const MIN_RECORD_WORDS_STANDARD: u16 = 4;

// ── Primitive readers ─────────────────────────────────────────────────

/// Read a single little-endian `u16` at `offset`. Returns `None` if OOB.
#[inline]
pub fn read_u16(data: &[u8], offset: usize) -> Option<u16> {
    let bytes = data.get(offset..offset + 2)?;
    Some(u16::from_le_bytes([bytes[0], bytes[1]]))
}

/// Read `count` little-endian `u16`s starting at `offset` into `out`.
/// Returns false if OOB; on success `out[..count]` is populated.
pub fn read_u16_array(data: &[u8], offset: usize, count: usize, out: &mut [u16]) -> bool {
    let needed = count * 2;
    let Some(slice) = data.get(offset..offset + needed) else {
        return false;
    };
    for (i, chunk) in slice.chunks_exact(2).enumerate() {
        out[i] = u16::from_le_bytes([chunk[0], chunk[1]]);
    }
    true
}

// ── Field decoders ────────────────────────────────────────────────────

#[inline]
pub fn decode_type_word(raw: u16) -> TypeWord {
    let message_type = (raw & 0x7F) as u8;
    let bus = if (raw >> 7) & 1 == 0 { Bus::A } else { Bus::B };
    let word_count = (raw >> 8) & 0x3F;
    let error = ((raw >> 14) & 1) != 0;
    TypeWord {
        message_type,
        bus,
        word_count,
        error,
        raw,
    }
}

pub fn decode_irig_timestamp(upper: u16, middle: u16, lower: u16) -> IrigTimestamp {
    let freerun = ((upper >> 15) & 1) != 0;
    let day = (upper >> 5) & 0x01FF;
    let hour = (upper & 0x1F) as u8;
    let minute = ((middle >> 10) & 0x3F) as u8;
    let second = ((middle >> 4) & 0x3F) as u8;
    let us_hi = u32::from(middle & 0xF);
    let us_lo = u32::from(lower);
    let microsecond = (us_hi << 16) | us_lo;
    IrigTimestamp {
        day,
        hour,
        minute,
        second,
        microsecond,
        freerun,
    }
}

pub fn decode_standard_timestamp(upper: u16, lower: u16) -> StandardTimestamp {
    let raw_value = (u32::from(upper) << 16) | u32::from(lower);
    StandardTimestamp {
        raw_value,
        upper_word: upper,
        lower_word: lower,
    }
}

pub fn decode_command_word(raw: u16) -> CommandWord {
    let rt = ((raw >> 11) & 0x1F) as u8;
    let direction = if (raw >> 10) & 1 == 0 {
        Direction::Receive
    } else {
        Direction::Transmit
    };
    let subaddress = ((raw >> 5) & 0x1F) as u8;
    let mut data_word_count = (raw & 0x1F) as u8;
    if data_word_count == 0 {
        data_word_count = 32;
    }
    CommandWord {
        rt,
        direction,
        subaddress,
        data_word_count,
        raw,
    }
}

// ── Format classification ─────────────────────────────────────────────

pub fn classify_message_format(
    message_type: u8,
    command_word: &CommandWord,
    word_count: u16,
) -> MieResult<MessageFormat> {
    use MessageType::*;
    match MessageType::from_code(message_type) {
        Some(BcToRt) => Ok(MessageFormat::Receive),
        Some(RtToBc) => Ok(MessageFormat::Transmit),
        Some(RtToRt) => Ok(MessageFormat::RtToRt),
        Some(BroadcastBcToRt) => Ok(MessageFormat::ReceiveBroadcast),
        Some(BroadcastRtToRt) => Ok(MessageFormat::RtToRtBroadcast),
        Some(ModeCommand) => Ok(classify_mode_code(command_word, word_count)),
        Some(SpuriousData) => Ok(MessageFormat::SpuriousData),
        None => Err(MieError::UnknownTypeWord {
            offset: 0,
            raw_type_word: 0,
            message_type,
        }),
    }
}

/// Which structural invariant a record violated. Used by callers
/// (the reader) to phrase a precise diagnostic; the strict-mode
/// path otherwise maps every violation to a single `PayloadError`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhichInvariant {
    /// L2-SYN-INV-001: Type 0x02 (BC→RT) requires Cmd direction = Receive.
    DirectionBcToRt,
    /// L2-SYN-INV-002: Type 0x04 (RT→BC) requires Cmd direction = Transmit.
    DirectionRtToBc,
    /// L2-SYN-INV-003: Type Word word_count too small for declared payload.
    WordCountCapacity,
    /// L2-SYN-INV-004: Cmd2 direction for RT-to-RT must be Receive.
    DirectionRtToRtCmd2,
    /// L2-SYN-INV-005: Status Word RT field does not match Cmd RT.
    /// AnomalyWarn-class — real-bus noise possible.
    StatusRtMismatch,
    /// L2-SYN-INV-006: Type Word bit 15 (reserved) is set.
    /// AnomalyWarn-class — possible vendor extension.
    TypeWordReservedBit,
}

/// Policy class for a structural invariant violation, per the locked
/// schema in `docs/REQUIREMENTS.md` (Phase 7 severity classes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvariantSeverity {
    /// Strict mode aborts with a record-error class; lenient mode
    /// WARN+skips the record (advance offset without emission).
    Reject,
    /// Both modes log a WARN and continue emitting the record. Used
    /// where outright rejection would produce false negatives on
    /// real-bus noise or vendor extensions (L2-SYN-INV-005, 006).
    AnomalyWarn,
}

#[derive(Debug)]
pub struct InvariantViolation {
    pub kind: WhichInvariant,
    pub severity: InvariantSeverity,
    pub detail: String,
}

/// Per-format minimum payload word count, computed from the primary
/// Command Word's declared data_word_count. Used by the L2-SYN-INV-003
/// capacity check. `SpuriousData` has variable size so returns 0
/// (skip the check). For RT-to-RT formats the second Command Word is
/// not yet decoded at the time we call this; we use Cmd1's
/// data_word_count as an approximation (the bus protocol requires
/// Cmd1 and Cmd2 to agree on data_word_count).
fn min_payload_words(fmt: MessageFormat, cmd: &CommandWord) -> u16 {
    let dwc = u16::from(cmd.data_word_count);
    match fmt {
        MessageFormat::Receive | MessageFormat::Transmit => dwc + 1, // data + status
        MessageFormat::RtToRt => dwc + 3, // cmd2 + tx_status + data + rx_status
        MessageFormat::ReceiveBroadcast => dwc, // data only (no status)
        MessageFormat::RtToRtBroadcast => dwc + 2, // cmd2 + tx_status + data
        MessageFormat::ModeCodeTxData => 2, // status + data
        MessageFormat::ModeCodeRxData => 2, // data + status
        MessageFormat::ModeCodeNoData => 1, // status only
        MessageFormat::ModeCodeBcastNoData => 0,
        MessageFormat::ModeCodeBcastData => 1, // data only (no status)
        MessageFormat::SpuriousData => 0,      // variable; no capacity check
    }
}

/// L2-SYN-INV: structural invariants per the locked schema. Caller
/// has already framing-validated (sync.rs) and classified the format
/// (classify_message_format). Returns Err describing the first
/// invariant the record violates; Ok if all hold.
///
/// Current invariant set:
/// - INV-001: Type 0x02 → Cmd direction = Receive
/// - INV-002: Type 0x04 → Cmd direction = Transmit
/// - INV-003: TW.word_count >= 1 + ts_words + 1 + min_payload_words(format, cmd)
///
/// Deferred (Phase 7b): cmd2 direction for RT-to-RT, Status RT vs
/// Cmd RT match, reserved-bit zero check.
pub fn validate_structural_invariants(
    tw: &TypeWord,
    cmd: &CommandWord,
    msg_fmt: MessageFormat,
    ts_words: u16,
) -> Result<(), InvariantViolation> {
    // L2-SYN-INV-001 / INV-002: per-type direction.
    if tw.message_type == MessageType::BcToRt as u8 && cmd.direction != Direction::Receive {
        return Err(InvariantViolation {
            kind: WhichInvariant::DirectionBcToRt,
            severity: InvariantSeverity::Reject,
            detail: format!(
                "Type 0x02 (BC→RT) requires Cmd direction = Receive; got Transmit \
                 (raw Cmd = 0x{:04X})",
                cmd.raw
            ),
        });
    }
    if tw.message_type == MessageType::RtToBc as u8 && cmd.direction != Direction::Transmit {
        return Err(InvariantViolation {
            kind: WhichInvariant::DirectionRtToBc,
            severity: InvariantSeverity::Reject,
            detail: format!(
                "Type 0x04 (RT→BC) requires Cmd direction = Transmit; got Receive \
                 (raw Cmd = 0x{:04X})",
                cmd.raw
            ),
        });
    }

    // L2-SYN-INV-003: word-count capacity check.
    let min_wc = 1 + ts_words + 1 + min_payload_words(msg_fmt, cmd);
    if tw.word_count < min_wc {
        return Err(InvariantViolation {
            kind: WhichInvariant::WordCountCapacity,
            severity: InvariantSeverity::Reject,
            detail: format!(
                "TW.word_count = {} is too small for declared payload \
                 (need at least {} for {:?} with data_word_count = {})",
                tw.word_count, min_wc, msg_fmt, cmd.data_word_count
            ),
        });
    }

    Ok(())
}

/// L2-SYN-INV-004: Cmd2 direction check for RT-to-RT formats.
///
/// Called post-extract because Cmd2 lives inside the payload and is
/// only available after `extract_payload`. For non-RT-to-RT formats
/// (or when cmd2 is None) this is a no-op.
pub fn validate_post_extract_invariants(
    msg_fmt: MessageFormat,
    cmd2: Option<&CommandWord>,
) -> Result<(), InvariantViolation> {
    let is_rt_to_rt = matches!(
        msg_fmt,
        MessageFormat::RtToRt | MessageFormat::RtToRtBroadcast
    );
    if !is_rt_to_rt {
        return Ok(());
    }
    let Some(c2) = cmd2 else {
        return Ok(());
    };
    if c2.direction != Direction::Receive {
        return Err(InvariantViolation {
            kind: WhichInvariant::DirectionRtToRtCmd2,
            severity: InvariantSeverity::Reject,
            detail: format!(
                "RT-to-RT Cmd2 requires direction = Receive; got Transmit \
                 (raw Cmd2 = 0x{:04X})",
                c2.raw
            ),
        });
    }
    Ok(())
}

/// L2-SYN-INV-005 / L2-SYN-INV-006: AnomalyWarn-class observations.
///
/// Both invariants are anomaly detectors rather than corruption
/// rejections; the reader logs each violation as a WARN and continues
/// emitting the record. Returns a Vec because multiple anomalies can
/// fire on a single record (e.g., Status RT mismatch AND reserved
/// bit set simultaneously).
pub fn detect_record_anomalies(
    tw: &TypeWord,
    cmd: &CommandWord,
    status_word: Option<u16>,
) -> Vec<InvariantViolation> {
    let mut out = Vec::new();

    // L2-SYN-INV-005: Status RT vs Cmd RT.
    if let Some(status_raw) = status_word {
        let status_rt = ((status_raw >> 11) & 0x1F) as u8;
        if status_rt != cmd.rt {
            out.push(InvariantViolation {
                kind: WhichInvariant::StatusRtMismatch,
                severity: InvariantSeverity::AnomalyWarn,
                detail: format!(
                    "Status RT = {status_rt} does not match Cmd RT = {} \
                     (raw Status = 0x{status_raw:04X}); possible bus interference",
                    cmd.rt
                ),
            });
        }
    }

    // L2-SYN-INV-006: Type Word bit 15 reserved.
    if (tw.raw >> 15) & 1 != 0 {
        out.push(InvariantViolation {
            kind: WhichInvariant::TypeWordReservedBit,
            severity: InvariantSeverity::AnomalyWarn,
            detail: format!(
                "Type Word bit 15 (reserved) is set in raw 0x{:04X}; \
                 possible undocumented vendor extension",
                tw.raw
            ),
        });
    }

    out
}

fn classify_mode_code(cmd: &CommandWord, word_count: u16) -> MessageFormat {
    let is_broadcast = cmd.rt == 31;
    if is_broadcast {
        // Broadcast mode codes have no status word.
        //   With data:    Type + 3×TS + ModeCmd + Data = 6
        //   Without data: Type + 3×TS + ModeCmd        = 5
        return if word_count > 5 {
            MessageFormat::ModeCodeBcastData
        } else {
            MessageFormat::ModeCodeBcastNoData
        };
    }

    // Non-broadcast mode codes always have a status word.
    if cmd.direction == Direction::Transmit {
        return MessageFormat::ModeCodeTxData;
    }
    if word_count >= 7 {
        MessageFormat::ModeCodeRxData
    } else {
        MessageFormat::ModeCodeNoData
    }
}

// ── Timestamp format auto-detection ───────────────────────────────────

/// Probe the first record at both candidate Command Word offsets and
/// return whichever scoring layout matches better. Defaults to IRIG on tie.
pub fn detect_timestamp_format(
    data: &[u8],
    offset: usize,
    type_word: &TypeWord,
) -> TimestampFormat {
    let mut irig_score: i32 = 0;
    let mut std_score: i32 = 0;

    // IRIG candidate: Cmd at offset+8 (Type + 3 TS words)
    if let Some(irig_cmd_raw) = read_u16(data, offset + 8) {
        let irig_cmd = decode_command_word(irig_cmd_raw);
        // T/R consistency with the Type Word: receive code expects Receive
        // direction, transmit code expects Transmit. Either match adds 2.
        if (type_word.message_type == MessageType::BcToRt as u8
            && irig_cmd.direction == Direction::Receive)
            || (type_word.message_type == MessageType::RtToBc as u8
                && irig_cmd.direction == Direction::Transmit)
        {
            irig_score += 2;
        }
        // Word count plausibility: IRIG overhead = TS(3) + Cmd(1) + Stat(1) + Type(1) = 6
        if i32::from(type_word.word_count) - 6 == i32::from(irig_cmd.data_word_count) {
            irig_score += 2;
        }
        // Range check on the candidate IRIG fields
        if let (Some(ts_upper), Some(ts_middle)) =
            (read_u16(data, offset + 2), read_u16(data, offset + 4))
        {
            let hour = ts_upper & 0x1F;
            let minute = (ts_middle >> 10) & 0x3F;
            let second = (ts_middle >> 4) & 0x3F;
            let us_hi = ts_middle & 0xF;
            if hour < 24 && minute < 60 && second < 60 && us_hi < 16 {
                irig_score += 1;
            }
        }
    }

    // Standard candidate: Cmd at offset+6 (Type + 2 TS words)
    if let Some(std_cmd_raw) = read_u16(data, offset + 6) {
        let std_cmd = decode_command_word(std_cmd_raw);
        if (type_word.message_type == MessageType::BcToRt as u8
            && std_cmd.direction == Direction::Receive)
            || (type_word.message_type == MessageType::RtToBc as u8
                && std_cmd.direction == Direction::Transmit)
        {
            std_score += 2;
        }
        // Standard overhead = TS(2) + Cmd(1) + Stat(1) + Type(1) = 5
        if i32::from(type_word.word_count) - 5 == i32::from(std_cmd.data_word_count) {
            std_score += 2;
        }
    }

    if irig_score > std_score {
        TimestampFormat::Irig
    } else if std_score > irig_score {
        TimestampFormat::Standard
    } else {
        // Tie-break: IRIG (more common in flight test recordings).
        TimestampFormat::Irig
    }
}

#[inline]
pub fn message_type_is_valid(code: u8) -> bool {
    is_valid_message_type(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_word_layout() {
        // 0x2402: word_count=36 in upper byte (0x24), type=0x02 in lower
        let tw = decode_type_word(0x2402);
        assert_eq!(tw.message_type, 2);
        assert_eq!(tw.bus, Bus::A);
        assert_eq!(tw.word_count, 36);
        assert!(!tw.error);
        assert_eq!(tw.raw, 0x2402);
    }

    #[test]
    fn type_word_bus_b_and_error_bit() {
        // bit 7 set → Bus B, bit 14 set → error
        let raw = 0b0100_0000_1000_0000 | 0x02; // type 0x02
        let tw = decode_type_word(raw);
        assert_eq!(tw.bus, Bus::B);
        assert!(tw.error);
    }

    #[test]
    fn irig_timestamp_known_value() {
        // From Python doctest fixture
        let ts = decode_irig_timestamp(0x180F, 0xDB26, 0xF621);
        assert_eq!(ts.hour, 15);
        assert_eq!(ts.minute, 54);
        assert_eq!(ts.second, 50);
        assert_eq!(ts.microsecond, 456_225);
        assert!(!ts.freerun);
    }

    #[test]
    fn irig_freerun_bit() {
        let ts = decode_irig_timestamp(0x8000, 0, 0);
        assert!(ts.freerun);
    }

    #[test]
    fn standard_timestamp_round_trip() {
        let ts = decode_standard_timestamp(0x0001, 0x86A0);
        assert_eq!(ts.raw_value, 100_000);
        assert_eq!(ts.upper_word, 0x0001);
        assert_eq!(ts.lower_word, 0x86A0);
    }

    #[test]
    fn command_word_known_value() {
        // 0x797E → RT 15, Receive, SA 11, 30 data words
        let cw = decode_command_word(0x797E);
        assert_eq!(cw.rt, 15);
        assert_eq!(cw.direction, Direction::Receive);
        assert_eq!(cw.subaddress, 11);
        assert_eq!(cw.data_word_count, 30);
    }

    #[test]
    fn command_word_zero_means_thirty_two() {
        // raw bits 0..4 = 0 → 32 data words
        let cw = decode_command_word(0b0000_1000_0010_0000); // arbitrary upper bits
        assert_eq!(cw.data_word_count, 32);
    }

    #[test]
    fn read_u16_le() {
        assert_eq!(read_u16(&[0x34, 0x12], 0), Some(0x1234));
        assert_eq!(read_u16(&[0x34], 0), None);
    }

    #[test]
    fn read_u16_array_into_slice() {
        let bytes = [0x01, 0x00, 0x02, 0x00, 0x03, 0x00];
        let mut out = [0u16; 3];
        assert!(read_u16_array(&bytes, 0, 3, &mut out));
        assert_eq!(out, [1, 2, 3]);
    }

    #[test]
    fn classify_simple_types() {
        let cmd = decode_command_word(0x797E);
        assert_eq!(
            classify_message_format(0x02, &cmd, 36).unwrap(),
            MessageFormat::Receive
        );
        assert_eq!(
            classify_message_format(0x04, &cmd, 10).unwrap(),
            MessageFormat::Transmit
        );
        assert_eq!(
            classify_message_format(0x08, &cmd, 12).unwrap(),
            MessageFormat::RtToRt
        );
        assert_eq!(
            classify_message_format(0x20, &cmd, 8).unwrap(),
            MessageFormat::SpuriousData
        );
    }

    #[test]
    fn classify_mode_code_broadcast_no_data() {
        let cmd = CommandWord {
            rt: 31,
            direction: Direction::Receive,
            subaddress: 0,
            data_word_count: 0,
            raw: 0,
        };
        assert_eq!(
            classify_message_format(0x01, &cmd, 5).unwrap(),
            MessageFormat::ModeCodeBcastNoData
        );
        assert_eq!(
            classify_message_format(0x01, &cmd, 6).unwrap(),
            MessageFormat::ModeCodeBcastData
        );
    }

    #[test]
    fn classify_mode_code_tx_data() {
        let cmd = CommandWord {
            rt: 5,
            direction: Direction::Transmit,
            subaddress: 0,
            data_word_count: 1,
            raw: 0,
        };
        assert_eq!(
            classify_message_format(0x01, &cmd, 7).unwrap(),
            MessageFormat::ModeCodeTxData
        );
    }

    #[test]
    fn classify_mode_code_rx_vs_no_data() {
        let cmd = CommandWord {
            rt: 5,
            direction: Direction::Receive,
            subaddress: 0,
            data_word_count: 1,
            raw: 0,
        };
        assert_eq!(
            classify_message_format(0x01, &cmd, 7).unwrap(),
            MessageFormat::ModeCodeRxData
        );
        assert_eq!(
            classify_message_format(0x01, &cmd, 6).unwrap(),
            MessageFormat::ModeCodeNoData
        );
    }

    #[test]
    fn classify_unknown_type() {
        let cmd = decode_command_word(0);
        assert!(classify_message_format(0x03, &cmd, 5).is_err());
    }

    // ── L2-SYN-INV structural invariants (Phase 7a) ──────────────────

    fn tw(message_type: u8, word_count: u16) -> TypeWord {
        TypeWord {
            message_type,
            bus: Bus::A,
            word_count,
            error: false,
            raw: 0,
        }
    }

    fn cmd_with(direction: Direction, dwc: u8) -> CommandWord {
        CommandWord {
            rt: 15,
            direction,
            subaddress: 11,
            data_word_count: dwc,
            raw: 0,
        }
    }

    #[test]
    fn invariants_pass_for_canonical_bc_to_rt() {
        let t = tw(0x02, 36); // 1 + 3 + 1 + 30 + 1 = 36
        let c = cmd_with(Direction::Receive, 30);
        validate_structural_invariants(&t, &c, MessageFormat::Receive, 3).unwrap();
    }

    #[test]
    fn invariants_pass_for_canonical_rt_to_bc() {
        let t = tw(0x04, 36);
        let c = cmd_with(Direction::Transmit, 30);
        validate_structural_invariants(&t, &c, MessageFormat::Transmit, 3).unwrap();
    }

    #[test]
    fn invariants_reject_bc_to_rt_with_transmit_cmd() {
        let t = tw(0x02, 36);
        let c = cmd_with(Direction::Transmit, 30); // wrong
        let err = validate_structural_invariants(&t, &c, MessageFormat::Receive, 3).unwrap_err();
        assert_eq!(err.kind, WhichInvariant::DirectionBcToRt);
    }

    #[test]
    fn invariants_reject_rt_to_bc_with_receive_cmd() {
        let t = tw(0x04, 36);
        let c = cmd_with(Direction::Receive, 30); // wrong
        let err = validate_structural_invariants(&t, &c, MessageFormat::Transmit, 3).unwrap_err();
        assert_eq!(err.kind, WhichInvariant::DirectionRtToBc);
    }

    #[test]
    fn invariants_reject_capacity_short() {
        // wc=5 too small for Receive with dwc=30 (needs 1+3+1+31=36).
        let t = tw(0x02, 5);
        let c = cmd_with(Direction::Receive, 30);
        let err = validate_structural_invariants(&t, &c, MessageFormat::Receive, 3).unwrap_err();
        assert_eq!(err.kind, WhichInvariant::WordCountCapacity);
    }

    #[test]
    fn invariants_accept_capacity_exact() {
        // wc=36 is exactly the minimum for Receive with dwc=30.
        let t = tw(0x02, 36);
        let c = cmd_with(Direction::Receive, 30);
        validate_structural_invariants(&t, &c, MessageFormat::Receive, 3).unwrap();
    }

    #[test]
    fn invariants_skip_capacity_for_spurious() {
        // SpuriousData has variable payload so the capacity check is
        // intentionally skipped (min_payload_words returns 0).
        let t = tw(0x20, 5); // wc=5 — only TW + 3 TS + 1 extra
        let c = cmd_with(Direction::Receive, 0);
        validate_structural_invariants(&t, &c, MessageFormat::SpuriousData, 3).unwrap();
    }

    #[test]
    fn invariants_mode_code_not_constrained_by_direction() {
        // Mode codes (type 0x01) can be Transmit OR Receive. The
        // direction invariants only apply to 0x02 and 0x04.
        let t = tw(0x01, 7);
        let c_tx = cmd_with(Direction::Transmit, 1);
        validate_structural_invariants(&t, &c_tx, MessageFormat::ModeCodeTxData, 3).unwrap();
        let c_rx = cmd_with(Direction::Receive, 1);
        validate_structural_invariants(&t, &c_rx, MessageFormat::ModeCodeRxData, 3).unwrap();
    }

    // ── L2-SYN-INV-004 (post-extract Cmd2 direction) ─────────────────

    #[test]
    fn post_extract_invariant_rt_to_rt_cmd2_receive_passes() {
        let c2 = CommandWord {
            rt: 5,
            direction: Direction::Receive,
            subaddress: 10,
            data_word_count: 3,
            raw: 0,
        };
        validate_post_extract_invariants(MessageFormat::RtToRt, Some(&c2)).unwrap();
    }

    #[test]
    fn post_extract_invariant_rt_to_rt_cmd2_transmit_rejected() {
        let c2 = CommandWord {
            rt: 5,
            direction: Direction::Transmit, // WRONG: should be Receive
            subaddress: 10,
            data_word_count: 3,
            raw: 0xABCD,
        };
        let err = validate_post_extract_invariants(MessageFormat::RtToRt, Some(&c2)).unwrap_err();
        assert_eq!(err.kind, WhichInvariant::DirectionRtToRtCmd2);
        assert_eq!(err.severity, InvariantSeverity::Reject);
    }

    #[test]
    fn post_extract_invariant_rt_to_rt_broadcast_also_checked() {
        let c2 = CommandWord {
            rt: 5,
            direction: Direction::Transmit,
            subaddress: 10,
            data_word_count: 3,
            raw: 0,
        };
        let err = validate_post_extract_invariants(MessageFormat::RtToRtBroadcast, Some(&c2))
            .unwrap_err();
        assert_eq!(err.kind, WhichInvariant::DirectionRtToRtCmd2);
    }

    #[test]
    fn post_extract_invariant_non_rt_to_rt_is_noop() {
        // No cmd2 for non-RT-to-RT formats; function returns Ok.
        validate_post_extract_invariants(MessageFormat::Receive, None).unwrap();
        // Even if a stray Cmd2 is passed in (shouldn't happen), other
        // formats don't enforce the direction invariant.
        let c2 = CommandWord {
            rt: 5,
            direction: Direction::Transmit,
            subaddress: 10,
            data_word_count: 3,
            raw: 0,
        };
        validate_post_extract_invariants(MessageFormat::Receive, Some(&c2)).unwrap();
    }

    // ── L2-SYN-INV-005 / INV-006 (anomaly detectors) ─────────────────

    #[test]
    fn anomaly_status_rt_match_no_violation() {
        // RT=15 in Cmd; status's bits 15-11 also = 15 (raw 0x7800).
        let t = tw(0x02, 36);
        let c = cmd_with(Direction::Receive, 30); // rt=15
        let anomalies = detect_record_anomalies(&t, &c, Some(0x7800));
        assert!(anomalies.is_empty());
    }

    #[test]
    fn anomaly_status_rt_mismatch_logged() {
        // Cmd RT=15 but Status raw 0x2800 → status RT = 5.
        let t = tw(0x02, 36);
        let c = cmd_with(Direction::Receive, 30); // rt=15
        let anomalies = detect_record_anomalies(&t, &c, Some(0x2800));
        assert_eq!(anomalies.len(), 1);
        assert_eq!(anomalies[0].kind, WhichInvariant::StatusRtMismatch);
        assert_eq!(anomalies[0].severity, InvariantSeverity::AnomalyWarn);
    }

    #[test]
    fn anomaly_no_status_no_violation() {
        // Broadcast formats and SPURIOUS_DATA have no Status Word;
        // INV-005 is silent.
        let t = tw(0x02, 36);
        let c = cmd_with(Direction::Receive, 30);
        let anomalies = detect_record_anomalies(&t, &c, None);
        assert!(anomalies.is_empty());
    }

    #[test]
    fn anomaly_type_word_reserved_bit_set_logged() {
        // Type word raw with bit 15 set: 0x8402 (wc=4, bit15=1, type=0x02).
        // (The framing parts here are irrelevant — the anomaly check
        // only looks at bit 15.)
        let t = TypeWord {
            message_type: 0x02,
            bus: Bus::A,
            word_count: 4,
            error: false,
            raw: 0x8402,
        };
        let c = cmd_with(Direction::Receive, 1);
        let anomalies = detect_record_anomalies(&t, &c, None);
        assert_eq!(anomalies.len(), 1);
        assert_eq!(anomalies[0].kind, WhichInvariant::TypeWordReservedBit);
        assert_eq!(anomalies[0].severity, InvariantSeverity::AnomalyWarn);
    }

    #[test]
    fn anomaly_multiple_can_fire_on_one_record() {
        // Status RT mismatch + reserved bit set: expect TWO anomalies.
        let t = TypeWord {
            message_type: 0x02,
            bus: Bus::A,
            word_count: 36,
            error: false,
            raw: 0xA402, // bit 15 set
        };
        let c = cmd_with(Direction::Receive, 30); // rt=15
        let anomalies = detect_record_anomalies(&t, &c, Some(0x2800)); // status RT=5
        assert_eq!(anomalies.len(), 2);
    }
}
