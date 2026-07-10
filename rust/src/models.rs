//! Core data structures for decoded MIL-STD-1553 MIE binary records.
//!
//! Mirrors the Python `models.py`. All structs are plain (not `Copy`) values;
//! immutability is enforced by Rust's type system rather than by `frozen=True`.
//! `DataWords` replaces `tuple[int, ...]` with an inline-buffer container
//! capped at the MIL-STD-1553B maximum of 32 data words, avoiding per-message
//! heap allocation.

use core::fmt;

// ── Enums ─────────────────────────────────────────────────────────────

/// MIL-STD-1553 redundant bus identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Bus {
    A = 0,
    B = 1,
}

impl Bus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
        }
    }
}

impl fmt::Display for Bus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// MIL-STD-1553 message transfer direction (from RT perspective).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Direction {
    /// BC sends data TO the RT.
    Receive = 0,
    /// RT sends data TO the BC.
    Transmit = 1,
}

/// DDC MIE Type Word message type code (bits 0–6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum MessageType {
    ModeCommand = 0x01,
    BcToRt = 0x02,
    RtToBc = 0x04,
    RtToRt = 0x08,
    BroadcastBcToRt = 0x10,
    BroadcastRtToRt = 0x18,
    SpuriousData = 0x20,
}

impl MessageType {
    /// Convert raw 8-bit code to enum, or `None` if unknown.
    pub fn from_code(code: u8) -> Option<Self> {
        match code {
            0x01 => Some(Self::ModeCommand),
            0x02 => Some(Self::BcToRt),
            0x04 => Some(Self::RtToBc),
            0x08 => Some(Self::RtToRt),
            0x10 => Some(Self::BroadcastBcToRt),
            0x18 => Some(Self::BroadcastRtToRt),
            0x20 => Some(Self::SpuriousData),
            _ => None,
        }
    }

    /// CLI-friendly canonical name (matches Python enum name).
    pub fn name(self) -> &'static str {
        match self {
            Self::ModeCommand => "MODE_COMMAND",
            Self::BcToRt => "BC_TO_RT",
            Self::RtToBc => "RT_TO_BC",
            Self::RtToRt => "RT_TO_RT",
            Self::BroadcastBcToRt => "BROADCAST_BC_TO_RT",
            Self::BroadcastRtToRt => "BROADCAST_RT_TO_RT",
            Self::SpuriousData => "SPURIOUS_DATA",
        }
    }
}

/// O(1) check that a raw type code is in the known set.
#[inline]
pub fn is_valid_message_type(code: u8) -> bool {
    matches!(code, 0x01 | 0x02 | 0x04 | 0x08 | 0x10 | 0x18 | 0x20)
}

/// Classified message format determining the payload layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum MessageFormat {
    Receive = 1,
    Transmit = 2,
    RtToRt = 3,
    ReceiveBroadcast = 4,
    RtToRtBroadcast = 5,
    ModeCodeTxData = 6,
    ModeCodeRxData = 7,
    ModeCodeNoData = 8,
    ModeCodeBcastNoData = 9,
    ModeCodeBcastData = 10,
    SpuriousData = 11,
}

/// Timestamp encoding format used in the MIE binary file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum TimestampFormat {
    Auto = 0,
    Irig = 1,
    Standard = 2,
}

impl TimestampFormat {
    /// Parse a `time_format` name (`auto` / `irig` / `standard`)
    /// case-insensitively. The single source of truth shared by the CLI
    /// (`--time-format`) and the config loader (`decode.time_format`) so the two
    /// can never disagree on which spellings are accepted. Returns `None` for an
    /// unrecognized name; each caller formats its own error type.
    pub(crate) fn from_name_ci(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "irig" => Some(Self::Irig),
            "standard" => Some(Self::Standard),
            _ => None,
        }
    }
}

/// How errored messages are routed in CSV output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ErrorMode {
    /// Errored + spurious go to a separate `<output>_errors.csv`.
    Separate = 0,
    /// Everything in one CSV with ERROR/ERROR_CODE columns populated.
    Inline = 1,
}

// ── DDC + decoder error codes ─────────────────────────────────────────

/// Manchester encoding, parity, or bit count error.
pub const ERROR_MANCHESTER_PARITY: u16 = 0x011E;
/// No status word response, or too few data words.
pub const ERROR_NO_RESPONSE: u16 = 0x0120;
/// Inverted sync pattern detected on a data word.
pub const ERROR_INVERTED_SYNC: u16 = 0x0136;
/// More data words received than the Command Word specified.
pub const ERROR_TOO_MANY_WORDS: u16 = 0x0140;
/// Unknown / undocumented DDC error.
pub const ERROR_UNKNOWN_DDC: u16 = 0x0150;

/// SPURIOUS_DATA continuation of a preceding errored message (decoder-assigned).
pub const ERROR_SPURIOUS_CONTINUATION: u16 = 0x2000;
/// Standalone SPURIOUS_DATA, no preceding error record (decoder-assigned).
pub const ERROR_SPURIOUS_STANDALONE: u16 = 0x2001;

/// True if `code` is a known DDC hardware error code (0x01xx range).
#[inline]
pub fn is_known_ddc_error_code(code: u16) -> bool {
    matches!(
        code,
        ERROR_MANCHESTER_PARITY
            | ERROR_NO_RESPONSE
            | ERROR_INVERTED_SYNC
            | ERROR_TOO_MANY_WORDS
            | ERROR_UNKNOWN_DDC
    )
}

/// True if `code` is a decoder-assigned spurious code (0x20xx range).
#[inline]
pub fn is_known_custom_error_code(code: u16) -> bool {
    matches!(
        code,
        ERROR_SPURIOUS_CONTINUATION | ERROR_SPURIOUS_STANDALONE
    )
}

/// True if `code` is in either known set.
#[inline]
pub fn is_known_error_code(code: u16) -> bool {
    is_known_ddc_error_code(code) || is_known_custom_error_code(code)
}

/// Human-readable description for a known error code, else empty string.
pub fn ddc_error_description(code: u16) -> &'static str {
    match code {
        ERROR_MANCHESTER_PARITY => "Manchester/Parity Error or Bit Count Error",
        ERROR_NO_RESPONSE => "No Status Response or Too Few Data Words",
        ERROR_INVERTED_SYNC => "Inverted Sync on Data Word",
        ERROR_TOO_MANY_WORDS => "Too Many Data Words",
        ERROR_UNKNOWN_DDC => "Unknown DDC Error",
        _ => "",
    }
}

// ── Timestamps ────────────────────────────────────────────────────────

/// IRIG-format timestamp decoded from a 3-word binary field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IrigTimestamp {
    pub day: u16,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub microsecond: u32,
    pub freerun: bool,
}

impl IrigTimestamp {
    /// Absolute microseconds from start of year.
    pub fn to_total_microseconds(self) -> u64 {
        let day = u64::from(self.day);
        let hour = u64::from(self.hour);
        let minute = u64::from(self.minute);
        let second = u64::from(self.second);
        let micro = u64::from(self.microsecond);
        (day * 86_400 + hour * 3_600 + minute * 60 + second) * 1_000_000 + micro
    }

    /// Format as `DAY:HH:MM:SS.uuuuuu` (matches DDC vendor CSV layout).
    ///
    /// Per L2-DEC-014 the microsecond field SHALL be exactly six
    /// digits. Validation in `sync::validate_record` should reject any
    /// record whose microsecond is >= 1_000_000 (L2-SYN-004), so this
    /// truncation is a defensive belt-and-suspenders: if a caller
    /// constructs an out-of-range `IrigTimestamp` directly (bypassing
    /// validation), the formatter still produces a well-formed string.
    pub fn format(&self) -> String {
        let micro = self.microsecond % 1_000_000;
        format!(
            "{day}:{h:02}:{m:02}:{s:02}.{u:06}",
            day = self.day,
            h = self.hour,
            m = self.minute,
            s = self.second,
            u = micro
        )
    }
}

/// Standard-format timestamp decoded from a 2-word free-running counter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StandardTimestamp {
    pub raw_value: u32,
    pub upper_word: u16,
    pub lower_word: u16,
}

impl StandardTimestamp {
    /// Raw 32-bit free-running counter value, in unknown tick units.
    /// The tick rate is card-dependent and not encoded in the file, so
    /// callers cannot convert this to seconds without external calibration.
    pub fn raw_ticks(self) -> u32 {
        self.raw_value
    }

    /// Convert raw counter ticks to microseconds using an external tick rate.
    ///
    /// `standard_tick_rate_hz` is the card-dependent counter frequency in Hz,
    /// supplied out-of-band (the file does not encode it). Returns `None`
    /// unless the rate is finite and strictly positive, so an uncalibrated or
    /// invalid rate can never be mistaken for real timing.
    ///
    /// Rounding is half-away-from-zero; ticks are non-negative so this matches
    /// the Python implementation's `int(x + 0.5)` exactly (see L2-DEC-017).
    pub fn to_microseconds(self, standard_tick_rate_hz: f64) -> Option<u64> {
        if !standard_tick_rate_hz.is_finite() || standard_tick_rate_hz <= 0.0 {
            return None;
        }
        let micros = f64::from(self.raw_value) * 1_000_000.0 / standard_tick_rate_hz;
        Some(micros.round() as u64)
    }

    /// Format as `0xNNNNNNNN`.
    pub fn format(&self) -> String {
        format!("0x{:08X}", self.raw_value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Timestamp {
    Irig(IrigTimestamp),
    Standard(StandardTimestamp),
}

impl Timestamp {
    /// Absolute microseconds from a known epoch, if convertible.
    ///
    /// `Some(us)` for IRIG (microseconds from start of year).
    ///
    /// For Standard, the result depends on calibration: `Some(us)` when
    /// `standard_tick_rate_hz` is a finite, strictly-positive counter
    /// frequency, otherwise `None` — raw counter ticks have no known tick
    /// rate or epoch, so DELTA in seconds cannot be computed truthfully
    /// without one. IRIG ignores `standard_tick_rate_hz`. See L2-DEC-017.
    pub fn to_microseconds(&self, standard_tick_rate_hz: Option<f64>) -> Option<u64> {
        match self {
            Self::Irig(t) => Some(t.to_total_microseconds()),
            Self::Standard(t) => standard_tick_rate_hz.and_then(|hz| t.to_microseconds(hz)),
        }
    }

    pub fn format(&self) -> String {
        match self {
            Self::Irig(t) => t.format(),
            Self::Standard(t) => t.format(),
        }
    }
}

/// Number of 16-bit words consumed by each timestamp format.
pub const fn timestamp_word_count(fmt: TimestampFormat) -> u16 {
    match fmt {
        TimestampFormat::Irig => 3,
        TimestampFormat::Standard => 2,
        TimestampFormat::Auto => 0,
    }
}

// ── TypeWord / CommandWord ────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypeWord {
    pub message_type: u8,
    pub bus: Bus,
    pub word_count: u16,
    pub error: bool,
    pub raw: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandWord {
    /// Remote Terminal address (0–30; 31 = broadcast).
    pub rt: u8,
    pub direction: Direction,
    /// Subaddress (0–31; 0 and 31 are mode codes).
    pub subaddress: u8,
    /// Number of data words (1–32; raw 0 maps to 32).
    pub data_word_count: u8,
    pub raw: u16,
}

impl CommandWord {
    pub fn is_broadcast(&self) -> bool {
        self.rt == 31
    }
    pub fn is_mode_code(&self) -> bool {
        self.subaddress == 0 || self.subaddress == 31
    }
}

// ── DataWords (inline-buffer Vec replacement) ─────────────────────────

/// MIL-STD-1553B caps a single transaction at 32 data words.
pub const MAX_DATA_WORDS: usize = 32;

/// Fixed-capacity 16-bit word buffer. Avoids heap allocation per message.
#[derive(Clone, Copy)]
pub struct DataWords {
    buf: [u16; MAX_DATA_WORDS],
    len: u8,
}

impl DataWords {
    pub const fn new() -> Self {
        Self {
            buf: [0; MAX_DATA_WORDS],
            len: 0,
        }
    }

    /// Build from a slice. Panics if `slice.len() > MAX_DATA_WORDS`.
    pub fn from_slice(slice: &[u16]) -> Self {
        assert!(
            slice.len() <= MAX_DATA_WORDS,
            "data words exceed 1553B max of {MAX_DATA_WORDS}"
        );
        let mut buf = [0u16; MAX_DATA_WORDS];
        buf[..slice.len()].copy_from_slice(slice);
        Self {
            buf,
            len: slice.len() as u8,
        }
    }

    /// Build from an iterator of `u16`. Stops at `MAX_DATA_WORDS`.
    pub fn from_iter_capped<I: IntoIterator<Item = u16>>(iter: I) -> Self {
        let mut out = Self::new();
        for w in iter {
            if !out.try_push(w) {
                break;
            }
        }
        out
    }

    /// Append one word; returns false if already full.
    #[inline]
    pub fn try_push(&mut self, word: u16) -> bool {
        let i = self.len as usize;
        if i >= MAX_DATA_WORDS {
            return false;
        }
        self.buf[i] = word;
        self.len += 1;
        true
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len as usize
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline]
    pub fn as_slice(&self) -> &[u16] {
        &self.buf[..self.len as usize]
    }

    pub fn iter(&self) -> std::slice::Iter<'_, u16> {
        self.as_slice().iter()
    }
}

impl Default for DataWords {
    fn default() -> Self {
        Self::new()
    }
}

impl PartialEq for DataWords {
    fn eq(&self, other: &Self) -> bool {
        self.as_slice() == other.as_slice()
    }
}
impl Eq for DataWords {}

impl fmt::Debug for DataWords {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            .entries(self.as_slice().iter().map(|w| format!("0x{w:04X}")))
            .finish()
    }
}

// ── MieMessage ────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct MieMessage {
    pub timestamp: Timestamp,
    pub type_word: TypeWord,
    pub message_format: MessageFormat,
    pub command_word: Option<CommandWord>,
    pub command_word_2: Option<CommandWord>,
    pub status_word: Option<u16>,
    pub status_word_2: Option<u16>,
    pub data_words: DataWords,
    pub error_word: Option<u16>,
    /// Seconds since prior message with the same RT+MSG.
    ///
    /// `Some(0.0)` on first occurrence of an RT/MSG key with a calibrated
    /// timestamp. `Some(s)` for a non-negative gap. `None` when no DELTA
    /// is meaningful: SPURIOUS_DATA (no RT/MSG key), uncalibrated Standard
    /// timestamps (no known tick rate), and non-monotonic timestamps.
    pub delta: Option<f64>,
    pub file_offset: u64,
    /// MUX column value derived from the source file name (L2-WRT-020), shared
    /// (one `Arc<str>` per input file) so per-record carry stays O(1) in resident
    /// memory. `None` when MUX population is disabled or the configured filename
    /// field is absent/empty.
    pub mux: Option<std::sync::Arc<str>>,
}

impl MieMessage {
    pub fn rt(&self) -> Option<u8> {
        self.command_word.map(|c| c.rt)
    }

    pub fn subaddress(&self) -> Option<u8> {
        self.command_word.map(|c| c.subaddress)
    }

    pub fn bus(&self) -> Bus {
        self.type_word.bus
    }

    /// Message label in `<SA><T|R>` format, or empty for SPURIOUS_DATA.
    pub fn msg_label(&self) -> String {
        match self.command_word {
            None => String::new(),
            Some(cw) => {
                let suffix = match cw.direction {
                    Direction::Transmit => 'T',
                    Direction::Receive => 'R',
                };
                format!("{}{}", cw.subaddress, suffix)
            }
        }
    }

    /// Unique key for per-RT/MSG delta tracking. Empty for SPURIOUS_DATA.
    pub fn delta_key(&self) -> String {
        match self.command_word {
            None => String::new(),
            Some(cw) => {
                let suffix = match cw.direction {
                    Direction::Transmit => 'T',
                    Direction::Receive => 'R',
                };
                format!("{}:{}{}", cw.rt, cw.subaddress, suffix)
            }
        }
    }

    pub fn is_error(&self) -> bool {
        self.type_word.error
    }

    pub fn is_spurious(&self) -> bool {
        self.message_format == MessageFormat::SpuriousData
    }

    /// CSV-column error label: `""`, `"ERROR"`, or `"SPURIOUS"`.
    pub fn error_label(&self) -> &'static str {
        if self.type_word.error {
            "ERROR"
        } else if self.is_spurious() {
            "SPURIOUS"
        } else {
            ""
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Requirements: L2-MSG-001
    #[test]
    fn message_type_round_trip() {
        for code in [0x01u8, 0x02, 0x04, 0x08, 0x10, 0x18, 0x20] {
            let mt = MessageType::from_code(code).unwrap();
            assert_eq!(mt as u8, code);
            assert!(is_valid_message_type(code));
        }
        assert!(MessageType::from_code(0x03).is_none());
        assert!(!is_valid_message_type(0x03));
    }

    /// Requirements: L2-WRT-011
    #[test]
    fn irig_format_matches_python_layout() {
        let t = IrigTimestamp {
            day: 10,
            hour: 15,
            minute: 54,
            second: 50,
            microsecond: 456_225,
            freerun: false,
        };
        assert_eq!(t.format(), "10:15:54:50.456225");
    }

    /// Requirements: L2-DEC-014
    #[test]
    fn irig_format_truncates_out_of_range_microseconds() {
        // L2-DEC-014: formatter SHALL emit exactly six microsecond
        // digits. Validation (L2-SYN-004) rejects records with
        // microsecond >= 1_000_000 before we get here, but a caller
        // who constructs an IrigTimestamp directly with a bad
        // microsecond MUST still produce a well-formed string.
        let t = IrigTimestamp {
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
            microsecond: 1_234_567, // > 999_999
            freerun: false,
        };
        let s = t.format();
        assert_eq!(s, "1:00:00:00.234567");
        // Sanity: the part after the '.' is exactly six characters.
        let micro_part = s.rsplit('.').next().unwrap();
        assert_eq!(micro_part.len(), 6);
    }

    /// Requirements: L2-DEC-007
    #[test]
    fn standard_format_hex() {
        let t = StandardTimestamp {
            raw_value: 0x1234_ABCD,
            upper_word: 0x1234,
            lower_word: 0xABCD,
        };
        assert_eq!(t.format(), "0x1234ABCD");
    }

    /// Requirements: L2-RDR-019, L2-DEC-017
    #[test]
    fn standard_to_microseconds_requires_calibration() {
        let t = StandardTimestamp {
            raw_value: 100_016,
            upper_word: 0x0001,
            lower_word: 0x86B0,
        };
        let ts = Timestamp::Standard(t);

        // Uncalibrated (and invalid rates) yield no microseconds.
        assert_eq!(ts.to_microseconds(None), None);
        assert_eq!(ts.to_microseconds(Some(0.0)), None);
        assert_eq!(ts.to_microseconds(Some(-1.0)), None);
        assert_eq!(ts.to_microseconds(Some(f64::NAN)), None);
        assert_eq!(ts.to_microseconds(Some(f64::INFINITY)), None);

        // 1 MHz: one tick == one microsecond.
        assert_eq!(ts.to_microseconds(Some(1_000_000.0)), Some(100_016));
        assert_eq!(t.to_microseconds(1_000_000.0), Some(100_016));
    }

    /// Requirements: L2-DEC-017
    #[test]
    fn standard_to_microseconds_rounds_half_away_from_zero() {
        // 3 ticks at 2 MHz = 1.5 µs → rounds up to 2 (half-away-from-zero,
        // matching Python's `int(x + 0.5)`).
        let t = StandardTimestamp {
            raw_value: 3,
            upper_word: 0,
            lower_word: 3,
        };
        assert_eq!(t.to_microseconds(2_000_000.0), Some(2));
        // 1 tick at 2 MHz = 0.5 µs → rounds up to 1.
        let one = StandardTimestamp {
            raw_value: 1,
            upper_word: 0,
            lower_word: 1,
        };
        assert_eq!(one.to_microseconds(2_000_000.0), Some(1));
    }

    /// Requirements: L3-RS-005
    #[test]
    fn data_words_inline_buffer() {
        let dw = DataWords::from_slice(&[1, 2, 3]);
        assert_eq!(dw.len(), 3);
        assert_eq!(dw.as_slice(), &[1, 2, 3]);
        assert!(!dw.is_empty());
        let empty = DataWords::new();
        assert!(empty.is_empty());
    }

    /// Requirements: L3-RS-005
    #[test]
    fn data_words_max_capacity() {
        let words: Vec<u16> = (0..MAX_DATA_WORDS as u16).collect();
        let dw = DataWords::from_slice(&words);
        assert_eq!(dw.len(), MAX_DATA_WORDS);
    }

    /// Requirements: L3-RS-005
    #[test]
    #[should_panic]
    fn data_words_overflow_panics() {
        let words: Vec<u16> = (0..(MAX_DATA_WORDS as u16 + 1)).collect();
        let _ = DataWords::from_slice(&words);
    }

    /// Requirements: L2-DEC-004
    #[test]
    fn command_word_predicates() {
        let bcast = CommandWord {
            rt: 31,
            direction: Direction::Receive,
            subaddress: 5,
            data_word_count: 1,
            raw: 0,
        };
        assert!(bcast.is_broadcast());
        assert!(!bcast.is_mode_code());

        let mode = CommandWord {
            rt: 1,
            direction: Direction::Transmit,
            subaddress: 0,
            data_word_count: 1,
            raw: 0,
        };
        assert!(!mode.is_broadcast());
        assert!(mode.is_mode_code());
    }

    /// Requirements: L2-ERR-003
    #[test]
    fn error_code_classification() {
        assert!(is_known_ddc_error_code(ERROR_MANCHESTER_PARITY));
        assert!(is_known_custom_error_code(ERROR_SPURIOUS_STANDALONE));
        assert!(is_known_error_code(ERROR_INVERTED_SYNC));
        assert!(!is_known_error_code(0xDEAD));
        assert_eq!(
            ddc_error_description(ERROR_NO_RESPONSE),
            "No Status Response or Too Few Data Words"
        );
        assert_eq!(ddc_error_description(0x9999), "");
    }

    /// Requirements: L2-DEC-002, L2-DEC-007
    #[test]
    fn timestamp_word_counts() {
        assert_eq!(timestamp_word_count(TimestampFormat::Irig), 3);
        assert_eq!(timestamp_word_count(TimestampFormat::Standard), 2);
    }

    /// Requirements: L2-MSG-003
    #[test]
    fn msg_label_and_delta_key() {
        let msg = make_msg(15, Direction::Receive, 11);
        assert_eq!(msg.msg_label(), "11R");
        assert_eq!(msg.delta_key(), "15:11R");

        let msg = make_msg(15, Direction::Transmit, 22);
        assert_eq!(msg.msg_label(), "22T");
    }

    /// Requirements: L2-MSG-003
    #[test]
    fn rt_and_subaddress_shortcuts() {
        let msg = make_msg(15, Direction::Receive, 11);
        assert_eq!(msg.rt(), Some(15));
        assert_eq!(msg.subaddress(), Some(11));

        // SPURIOUS_DATA carries no Command Word, so both shortcuts are None.
        let mut spurious = make_msg(15, Direction::Receive, 11);
        spurious.command_word = None;
        assert_eq!(spurious.rt(), None);
        assert_eq!(spurious.subaddress(), None);
    }

    fn make_msg(rt: u8, dir: Direction, sa: u8) -> MieMessage {
        MieMessage {
            timestamp: Timestamp::Standard(StandardTimestamp {
                raw_value: 0,
                upper_word: 0,
                lower_word: 0,
            }),
            type_word: TypeWord {
                message_type: 0x02,
                bus: Bus::A,
                word_count: 5,
                error: false,
                raw: 0,
            },
            message_format: MessageFormat::Receive,
            command_word: Some(CommandWord {
                rt,
                direction: dir,
                subaddress: sa,
                data_word_count: 1,
                raw: 0,
            }),
            command_word_2: None,
            status_word: None,
            status_word_2: None,
            data_words: DataWords::new(),
            error_word: None,
            delta: Some(0.0),
            file_offset: 0,
            mux: None,
        }
    }
}
