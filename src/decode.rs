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
pub fn detect_timestamp_format(data: &[u8], offset: usize, type_word: &TypeWord) -> TimestampFormat {
    let mut irig_score: i32 = 0;
    let mut std_score: i32 = 0;

    // IRIG candidate: Cmd at offset+8 (Type + 3 TS words)
    if let Some(irig_cmd_raw) = read_u16(data, offset + 8) {
        let irig_cmd = decode_command_word(irig_cmd_raw);
        if type_word.message_type == MessageType::BcToRt as u8
            && irig_cmd.direction == Direction::Receive
        {
            irig_score += 2;
        } else if type_word.message_type == MessageType::RtToBc as u8
            && irig_cmd.direction == Direction::Transmit
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
        if type_word.message_type == MessageType::BcToRt as u8
            && std_cmd.direction == Direction::Receive
        {
            std_score += 2;
        } else if type_word.message_type == MessageType::RtToBc as u8
            && std_cmd.direction == Direction::Transmit
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
}
