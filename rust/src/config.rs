//! Configuration loader and a hand-rolled TOML parser for our small schema.
//!
//! The parser supports exactly what `config/default.toml` needs:
//!   - `[section]` headers
//!   - `key = value` pairs
//!   - Quoted strings (`"..."`), integers, booleans (`true`/`false`)
//!   - Primitive arrays (`[1, 2, 3]` or `["a", "b"]`)
//!   - `#` line comments and trailing comments on value lines
//!   - Whitespace-insensitive
//!
//! Anything outside this subset is rejected with a line number.
//!
//! Precedence: CLI args > config file > built-in defaults
//! (implemented via [`DecoderConfig::with_overrides`]).

use std::fs;
use std::path::Path;

use crate::decode::{
    DEFAULT_DETECT_RECORDS, DEFAULT_MUX_DELIMITER, DEFAULT_MUX_ENABLED, DEFAULT_MUX_FIELD,
};
use crate::filter::FilterConfig;
use crate::models::{Bus, ErrorMode, MessageType, TimestampFormat};
use crate::sync::DEFAULT_LOOKAHEAD_RECORDS;

/// L2-DEC-015 valid range for `decode.detect_records`. Values outside
/// this range are rejected at config-load time with a clear error.
pub const DETECT_RECORDS_MIN: usize = 1;
pub const DETECT_RECORDS_MAX: usize = 32;

/// L2-SYN-026 valid range for `decode.lookahead_records`. Same shape as
/// DETECT_RECORDS_MIN/_MAX — the two configurable record-count knobs
/// share their valid range for consistency.
pub const LOOKAHEAD_RECORDS_MIN: usize = 1;
pub const LOOKAHEAD_RECORDS_MAX: usize = 32;

/// Internal config state, assembled from the TOML loader and CLI overrides and
/// consumed within the crate / binary. **Not** part of the crate's stable public
/// API — that surface is the `pub use` re-exports in `lib.rs` — so it is
/// `#[doc(hidden)]` and excluded from SemVer checks (its field set grows as
/// decode options are added; see the `cargo-semver-checks` note in Cargo.toml).
#[doc(hidden)]
#[derive(Debug, Clone)]
pub struct DecoderConfig {
    pub log_level: String,
    pub time_format: TimestampFormat,
    pub strict: bool,
    pub error_mode: ErrorMode,
    pub filters: FilterConfig,
    pub output_format: String,
    /// L2-WRT-017: refuse to overwrite an existing destination. Defaults
    /// to `false` (overwrite is permitted) to preserve historical
    /// behavior. Set via `output.no_clobber = true` in TOML or
    /// `--no-clobber` on the CLI.
    pub no_clobber: bool,
    /// L1-EXIT-004: on unrecoverable mid-file sync loss, commit the rows
    /// decoded so far as `<destination>.partial` and exit 0, rather
    /// than unlinking and exiting 3. Set via `decode.allow_partial =
    /// true` in TOML or `--allow-partial` on the CLI.
    pub allow_partial: bool,
    /// L2-DEC-015: number of records the timestamp-format auto-detect
    /// probe walks before committing to IRIG vs Standard. Default
    /// `DEFAULT_DETECT_RECORDS` (`8`). Set via
    /// `decode.detect_records = N` in TOML or `--detect-records N` on
    /// the CLI. Validated against `[DETECT_RECORDS_MIN,
    /// DETECT_RECORDS_MAX]` at load time.
    pub detect_records: usize,
    /// L2-SYN-026: total number of records `sync::validate_record`
    /// checks per call (1 candidate + N-1 look-ahead). Default
    /// `DEFAULT_LOOKAHEAD_RECORDS` (`2`), preserving the historical
    /// two-record look-ahead from L2-SYN-005. Set via
    /// `decode.lookahead_records = N` in TOML or
    /// `--lookahead-records N` on the CLI. Validated against
    /// `[LOOKAHEAD_RECORDS_MIN, LOOKAHEAD_RECORDS_MAX]` at load time.
    pub lookahead_records: usize,
    /// L2-DEC-017: optional Standard-counter tick rate in Hz. `None`
    /// (the default) keeps the historical empty-`DELTA` behavior for
    /// Standard-timestamp records. When set to a finite, strictly-positive
    /// value, Standard ticks are converted to microseconds and the records
    /// participate in DELTA tracking like IRIG. Set via
    /// `decode.standard_tick_rate_hz = <hz>` in TOML or
    /// `--standard-tick-rate-hz <hz>` on the CLI. Validated as finite and
    /// `> 0` at load time.
    pub standard_tick_rate_hz: Option<f64>,
    /// L2-WRT-020: populate the MUX column from a field of the input file name.
    /// Enabled by default (`[mux] enabled = false` / `--no-mux` disables it for
    /// vendor-exact output). The file name is split on `mux_delimiter` and the
    /// `mux_field`-th field (0-based; negative counts from the end) becomes MUX.
    pub mux_enabled: bool,
    pub mux_delimiter: String,
    pub mux_field: i64,
    /// L2-MRG-007: collapse the same bus transaction witnessed by multiple
    /// recorders into one row, in a multi-file merge. Off by default (loss-free);
    /// `[merge] collapse_duplicates = true` / `--collapse-duplicates` enables it.
    pub collapse_duplicates: bool,
    /// Timestamp tolerance in microseconds for collapsing (0 = exact-µs match).
    /// Widen it for recorders whose IRIG clocks differ slightly.
    pub collapse_window_us: u64,
}

impl Default for DecoderConfig {
    fn default() -> Self {
        Self {
            log_level: "WARNING".to_string(),
            time_format: TimestampFormat::Auto,
            strict: false,
            error_mode: ErrorMode::Separate,
            filters: FilterConfig::default(),
            output_format: "csv".to_string(),
            no_clobber: false,
            allow_partial: false,
            detect_records: DEFAULT_DETECT_RECORDS,
            lookahead_records: DEFAULT_LOOKAHEAD_RECORDS,
            standard_tick_rate_hz: None,
            mux_enabled: DEFAULT_MUX_ENABLED,
            mux_delimiter: DEFAULT_MUX_DELIMITER.to_string(),
            mux_field: DEFAULT_MUX_FIELD,
            collapse_duplicates: false,
            collapse_window_us: 0,
        }
    }
}

/// Override container: every field is optional and only applied if `Some`.
/// Filter overrides MERGE into the existing set rather than replacing it,
/// matching the Python `with_overrides` semantics. Internal plumbing — not part
/// of the stable public API; `#[doc(hidden)]` and excluded from SemVer checks.
#[doc(hidden)]
#[derive(Debug, Default, Clone)]
pub struct ConfigOverrides {
    pub log_level: Option<String>,
    pub time_format: Option<TimestampFormat>,
    pub strict: Option<bool>,
    pub error_mode: Option<ErrorMode>,
    pub output_format: Option<String>,
    pub no_clobber: Option<bool>,
    pub allow_partial: Option<bool>,
    pub detect_records: Option<usize>,
    pub lookahead_records: Option<usize>,
    pub standard_tick_rate_hz: Option<f64>,
    pub mux_enabled: Option<bool>,
    pub mux_delimiter: Option<String>,
    pub mux_field: Option<i64>,
    pub collapse_duplicates: Option<bool>,
    pub collapse_window_us: Option<i64>,

    pub exclude_types: Vec<u8>,
    pub exclude_rts: Vec<u8>,
    pub exclude_buses: Vec<Bus>,
    pub exclude_subaddresses: Vec<u8>,

    pub include_types: Vec<u8>,
    pub include_rts: Vec<u8>,
    pub include_buses: Vec<Bus>,
    pub include_subaddresses: Vec<u8>,
}

impl DecoderConfig {
    pub fn with_overrides(mut self, ov: ConfigOverrides) -> Self {
        if let Some(v) = ov.log_level {
            self.log_level = v;
        }
        if let Some(v) = ov.time_format {
            self.time_format = v;
        }
        if let Some(v) = ov.strict {
            self.strict = v;
        }
        if let Some(v) = ov.error_mode {
            self.error_mode = v;
        }
        if let Some(v) = ov.output_format {
            self.output_format = v;
        }
        if let Some(v) = ov.no_clobber {
            self.no_clobber = v;
        }
        if let Some(v) = ov.allow_partial {
            self.allow_partial = v;
        }
        if let Some(v) = ov.detect_records {
            self.detect_records = v;
        }
        if let Some(v) = ov.lookahead_records {
            self.lookahead_records = v;
        }
        if let Some(v) = ov.standard_tick_rate_hz {
            self.standard_tick_rate_hz = Some(v);
        }
        if let Some(v) = ov.mux_enabled {
            self.mux_enabled = v;
        }
        if let Some(v) = ov.mux_delimiter {
            self.mux_delimiter = v;
        }
        if let Some(v) = ov.mux_field {
            self.mux_field = v;
        }
        if let Some(v) = ov.collapse_duplicates {
            self.collapse_duplicates = v;
        }
        if let Some(v) = ov.collapse_window_us {
            // CLI / config-load validation already rejects negatives; clamp
            // defensively so the cast can never wrap.
            self.collapse_window_us = v.max(0) as u64;
        }

        merge_unique(&mut self.filters.exclude_types, ov.exclude_types);
        merge_unique(&mut self.filters.exclude_rts, ov.exclude_rts);
        merge_unique(&mut self.filters.exclude_buses, ov.exclude_buses);
        merge_unique(
            &mut self.filters.exclude_subaddresses,
            ov.exclude_subaddresses,
        );

        merge_unique(&mut self.filters.include_types, ov.include_types);
        merge_unique(&mut self.filters.include_rts, ov.include_rts);
        merge_unique(&mut self.filters.include_buses, ov.include_buses);
        merge_unique(
            &mut self.filters.include_subaddresses,
            ov.include_subaddresses,
        );

        self
    }
}

fn merge_unique<T: PartialEq>(target: &mut Vec<T>, source: Vec<T>) {
    for v in source {
        if !target.contains(&v) {
            target.push(v);
        }
    }
}

// ── Public loader ─────────────────────────────────────────────────────

#[derive(Debug)]
pub struct ConfigError(pub String);

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for ConfigError {}

pub fn load_config(path: Option<&Path>) -> Result<DecoderConfig, ConfigError> {
    let Some(path) = path else {
        return Ok(DecoderConfig::default());
    };
    if !path.exists() {
        return Err(ConfigError(format!(
            "Config file not found: {}",
            path.display()
        )));
    }
    let text = fs::read_to_string(path)
        .map_err(|e| ConfigError(format!("Reading {}: {}", path.display(), e)))?;
    parse_into_config(&text)
}

pub fn parse_into_config(text: &str) -> Result<DecoderConfig, ConfigError> {
    let toml = parse_toml(text).map_err(ConfigError)?;
    let mut cfg = DecoderConfig::default();
    // Each `[section]` is applied by its own helper (all validate at load time
    // per L2-CFG-010) so no single function carries the whole schema.
    apply_logging_section(&toml, &mut cfg)?;
    apply_decode_section(&toml, &mut cfg)?;
    apply_output_section(&toml, &mut cfg)?;
    apply_mux_section(&toml, &mut cfg)?;
    apply_filter_sections(&toml, &mut cfg)?;
    warn_unknown_keys(&toml);
    Ok(cfg)
}

/// `[logging]`: validate at load time so the error points at the config file
/// rather than surfacing later as a silent no-op.
fn apply_logging_section(toml: &TomlDoc, cfg: &mut DecoderConfig) -> Result<(), ConfigError> {
    if let Some(level) = toml.get_string("logging", "level")? {
        let upper = level.to_uppercase();
        if crate::log::Level::parse(&upper).is_none() {
            return Err(ConfigError(format!(
                "Invalid logging.level: {level:?}. \
                 Valid: DEBUG, INFO, WARNING, WARN, ERROR, CRITICAL, OFF"
            )));
        }
        cfg.log_level = upper;
    }
    Ok(())
}

/// `[decode]`: timestamp format, strict/allow-partial flags, detection and
/// look-ahead ranges, and the Standard tick-rate calibration.
fn apply_decode_section(toml: &TomlDoc, cfg: &mut DecoderConfig) -> Result<(), ConfigError> {
    if let Some(tf) = toml.get_string("decode", "time_format")? {
        cfg.time_format = parse_time_format(tf)?;
    }
    if let Some(b) = toml.get_bool("decode", "strict")? {
        cfg.strict = b;
    }
    if let Some(em) = toml.get_string("decode", "error_mode")? {
        cfg.error_mode = parse_error_mode(em)?;
    }
    if let Some(b) = toml.get_bool("decode", "allow_partial")? {
        cfg.allow_partial = b;
    }
    if let Some(n) = toml.get_int("decode", "detect_records")? {
        // L2-DEC-015: validate range [1, 32] at load time per L2-CFG-010.
        cfg.detect_records = require_int_range(
            n,
            "decode.detect_records",
            DETECT_RECORDS_MIN,
            DETECT_RECORDS_MAX,
        )?;
    }
    if let Some(n) = toml.get_int("decode", "lookahead_records")? {
        // L2-SYN-026: validate range [1, 32] at load time per L2-CFG-010.
        cfg.lookahead_records = require_int_range(
            n,
            "decode.lookahead_records",
            LOOKAHEAD_RECORDS_MIN,
            LOOKAHEAD_RECORDS_MAX,
        )?;
    }
    if let Some(hz) = toml.get_float("decode", "standard_tick_rate_hz")? {
        // L2-DEC-017: the tick rate must be a real, strictly-positive frequency.
        cfg.standard_tick_rate_hz =
            Some(require_positive_finite(hz, "decode.standard_tick_rate_hz")?);
    }
    Ok(())
}

/// Validate a TOML integer within `[lo, hi]` at load time (L2-CFG-010). `key`
/// names the offending key for the error message.
fn require_int_range(n: i64, key: &str, lo: usize, hi: usize) -> Result<usize, ConfigError> {
    if n < lo as i64 || n > hi as i64 {
        return Err(ConfigError(format!(
            "Invalid {key}: {n}. Valid range: [{lo}, {hi}]"
        )));
    }
    Ok(n as usize)
}

/// Validate a Standard tick rate: finite and strictly positive (L2-DEC-017).
fn require_positive_finite(hz: f64, key: &str) -> Result<f64, ConfigError> {
    if !hz.is_finite() || hz <= 0.0 {
        return Err(ConfigError(format!(
            "Invalid {key}: {hz}. Must be a finite value greater than 0"
        )));
    }
    Ok(hz)
}

/// `[output]`: output format (only `csv` today, L2-CFG-010) and no-clobber.
fn apply_output_section(toml: &TomlDoc, cfg: &mut DecoderConfig) -> Result<(), ConfigError> {
    if let Some(fmt) = toml.get_string("output", "format")? {
        if fmt != "csv" {
            return Err(ConfigError(format!(
                "Invalid output.format: {fmt:?}. Valid: csv"
            )));
        }
        cfg.output_format = fmt.to_string();
    }
    if let Some(b) = toml.get_bool("output", "no_clobber")? {
        cfg.no_clobber = b;
    }
    Ok(())
}

/// `[mux]`: MUX-from-filename configuration (L2-WRT-020).
fn apply_mux_section(toml: &TomlDoc, cfg: &mut DecoderConfig) -> Result<(), ConfigError> {
    if let Some(b) = toml.get_bool("mux", "enabled")? {
        cfg.mux_enabled = b;
    }
    if let Some(d) = toml.get_string("mux", "delimiter")? {
        if d.is_empty() {
            return Err(ConfigError(
                "Invalid mux.delimiter: must be a non-empty string".to_string(),
            ));
        }
        cfg.mux_delimiter = d.to_string();
    }
    if let Some(n) = toml.get_int("mux", "field")? {
        cfg.mux_field = n;
    }
    Ok(())
}

/// `[filter]`: the four exclude-array keys, each element validated on push.
fn apply_filter_sections(toml: &TomlDoc, cfg: &mut DecoderConfig) -> Result<(), ConfigError> {
    if let Some(types) = toml.get_array("filter", "exclude_types")? {
        for v in types {
            cfg.filters.exclude_types.push(parse_type_value(v)?);
        }
    }
    if let Some(rts) = toml.get_array("filter", "exclude_rts")? {
        for v in rts {
            cfg.filters
                .exclude_rts
                .push(parse_int_rt_sa(v, "exclude_rts")?);
        }
    }
    if let Some(buses) = toml.get_array("filter", "exclude_buses")? {
        for v in buses {
            cfg.filters.exclude_buses.push(parse_bus_value(v)?);
        }
    }
    if let Some(sas) = toml.get_array("filter", "exclude_subaddresses")? {
        for v in sas {
            cfg.filters
                .exclude_subaddresses
                .push(parse_int_rt_sa(v, "exclude_subaddresses")?);
        }
    }
    Ok(())
}

/// L2-CFG-009: WARN on unknown `[section] key` entries so typos in a config
/// file (e.g., `exclude_subdresses`) surface to the operator instead of being
/// silently dropped. Non-fatal so forward-compatible additions don't break
/// older configs.
fn warn_unknown_keys(toml: &TomlDoc) {
    for (section, key, _) in &toml.entries {
        if !is_known_shared_key(section.as_str(), key.as_str()) {
            crate::log_warn!("unknown TOML key: [{section}] {key}");
        }
    }
}

/// Shared schema membership check used by L2-CFG-009. Any
/// `(section, key)` pair not in this list triggers an unknown-key WARN
/// at load time.
fn is_known_shared_key(section: &str, key: &str) -> bool {
    matches!(
        (section, key),
        ("logging", "level")
            | ("decode", "time_format")
            | ("decode", "strict")
            | ("decode", "error_mode")
            | ("decode", "allow_partial")
            | ("decode", "detect_records")
            | ("decode", "lookahead_records")
            | ("decode", "standard_tick_rate_hz")
            | ("output", "format")
            | ("output", "no_clobber")
            | ("mux", "enabled")
            | ("mux", "delimiter")
            | ("mux", "field")
            | ("filter", "exclude_types")
            | ("filter", "exclude_rts")
            | ("filter", "exclude_buses")
            | ("filter", "exclude_subaddresses")
    )
}

// ── Helpers for value coercion ────────────────────────────────────────

fn parse_time_format(s: &str) -> Result<TimestampFormat, ConfigError> {
    TimestampFormat::from_name_ci(s).ok_or_else(|| {
        ConfigError(format!(
            "Invalid time_format: {s:?}. Valid: auto, irig, standard"
        ))
    })
}

fn parse_error_mode(s: &str) -> Result<ErrorMode, ConfigError> {
    match s.to_ascii_lowercase().as_str() {
        "separate" => Ok(ErrorMode::Separate),
        "inline" => Ok(ErrorMode::Inline),
        other => Err(ConfigError(format!(
            "Invalid error_mode: {other:?}. Valid: separate, inline"
        ))),
    }
}

pub fn parse_type_value(v: &TomlValue) -> Result<u8, ConfigError> {
    match v {
        TomlValue::String(s) => parse_type_name(s),
        TomlValue::Int(i) => {
            u8::try_from(*i).map_err(|_| ConfigError(format!("Type code out of range: {i}")))
        }
        _ => Err(ConfigError(
            "exclude_types entries must be strings or integers".into(),
        )),
    }
}

/// Parse a message-type identifier: name (e.g. "BC_TO_RT") or hex (e.g. "0x02").
pub fn parse_type_name(s: &str) -> Result<u8, ConfigError> {
    let upper = s.trim().to_uppercase();
    let by_name: &[(&str, u8)] = &[
        ("MODE_COMMAND", MessageType::ModeCommand as u8),
        ("BC_TO_RT", MessageType::BcToRt as u8),
        ("RT_TO_BC", MessageType::RtToBc as u8),
        ("RT_TO_RT", MessageType::RtToRt as u8),
        ("BROADCAST_BC_TO_RT", MessageType::BroadcastBcToRt as u8),
        ("BROADCAST_RT_TO_RT", MessageType::BroadcastRtToRt as u8),
        ("SPURIOUS_DATA", MessageType::SpuriousData as u8),
    ];
    for (name, code) in by_name {
        if upper == *name {
            return Ok(*code);
        }
    }
    if let Some(rest) = upper.strip_prefix("0X") {
        return u8::from_str_radix(rest, 16)
            .map_err(|_| ConfigError(format!("Invalid hex type code: {s:?}")));
    }
    Err(ConfigError(format!(
        "Unknown message type: {s:?}. \
         Valid: MODE_COMMAND, BC_TO_RT, RT_TO_BC, RT_TO_RT, \
         BROADCAST_BC_TO_RT, BROADCAST_RT_TO_RT, SPURIOUS_DATA"
    )))
}

pub fn parse_bus_value(v: &TomlValue) -> Result<Bus, ConfigError> {
    if let TomlValue::String(s) = v {
        parse_bus_name(s)
    } else {
        Err(ConfigError("exclude_buses entries must be strings".into()))
    }
}

pub fn parse_bus_name(s: &str) -> Result<Bus, ConfigError> {
    match s.trim().to_ascii_uppercase().as_str() {
        "A" => Ok(Bus::A),
        "B" => Ok(Bus::B),
        other => Err(ConfigError(format!("Invalid bus: {other:?}. Valid: A, B"))),
    }
}

/// Parse a MIL-STD-1553 RT address or subaddress: integer in [0, 31].
/// Per the L2-CFG schema reference, values outside this range are
/// rejected at load time because they could never match a real record.
fn parse_int_rt_sa(v: &TomlValue, field: &str) -> Result<u8, ConfigError> {
    match v {
        TomlValue::Int(i) => {
            if !(0..=31).contains(i) {
                return Err(ConfigError(format!(
                    "{field} value out of MIL-STD-1553 range [0, 31]: {i}"
                )));
            }
            Ok(*i as u8)
        }
        _ => Err(ConfigError(format!("{field} entries must be integers"))),
    }
}

// ── TOML parser ───────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum TomlValue {
    String(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Array(Vec<TomlValue>),
}

#[derive(Debug, Default)]
pub struct TomlDoc {
    /// Indexed by `(section, key)` → value. Order-insensitive.
    entries: Vec<(String, String, TomlValue)>,
}

impl TomlDoc {
    pub fn get(&self, section: &str, key: &str) -> Option<&TomlValue> {
        self.entries
            .iter()
            .find(|(s, k, _)| s == section && k == key)
            .map(|(_, _, v)| v)
    }
    pub fn get_string(&self, section: &str, key: &str) -> Result<Option<&str>, ConfigError> {
        match self.get(section, key) {
            None => Ok(None),
            Some(TomlValue::String(s)) => Ok(Some(s)),
            Some(_) => Err(ConfigError(format!("[{section}] {key} must be a string"))),
        }
    }
    pub fn get_bool(&self, section: &str, key: &str) -> Result<Option<bool>, ConfigError> {
        match self.get(section, key) {
            None => Ok(None),
            Some(TomlValue::Bool(b)) => Ok(Some(*b)),
            Some(_) => Err(ConfigError(format!("[{section}] {key} must be a boolean"))),
        }
    }
    pub fn get_array(&self, section: &str, key: &str) -> Result<Option<&[TomlValue]>, ConfigError> {
        match self.get(section, key) {
            None => Ok(None),
            Some(TomlValue::Array(a)) => Ok(Some(a)),
            Some(_) => Err(ConfigError(format!("[{section}] {key} must be an array"))),
        }
    }
    pub fn get_int(&self, section: &str, key: &str) -> Result<Option<i64>, ConfigError> {
        match self.get(section, key) {
            None => Ok(None),
            Some(TomlValue::Int(i)) => Ok(Some(*i)),
            Some(_) => Err(ConfigError(format!("[{section}] {key} must be an integer"))),
        }
    }
    /// Read a float value. Accepts a TOML integer as well so operators can
    /// write either `1000000` or `1000000.0` for a rate-style key.
    pub fn get_float(&self, section: &str, key: &str) -> Result<Option<f64>, ConfigError> {
        match self.get(section, key) {
            None => Ok(None),
            Some(TomlValue::Float(f)) => Ok(Some(*f)),
            #[allow(clippy::cast_precision_loss)]
            Some(TomlValue::Int(i)) => Ok(Some(*i as f64)),
            Some(_) => Err(ConfigError(format!("[{section}] {key} must be a number"))),
        }
    }
}

/// Parse the supported TOML subset. Returns a flat key/section map.
pub fn parse_toml(text: &str) -> Result<TomlDoc, String> {
    let mut doc = TomlDoc::default();
    let mut section = String::new();

    for (lineno, raw) in text.lines().enumerate() {
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }

        if let Some(stripped) = line.strip_prefix('[') {
            let inner = stripped
                .strip_suffix(']')
                .ok_or_else(|| format!("line {}: unterminated section header", lineno + 1))?;
            section = inner.trim().to_string();
            if section.is_empty() {
                return Err(format!("line {}: empty section name", lineno + 1));
            }
            continue;
        }

        let eq = line
            .find('=')
            .ok_or_else(|| format!("line {}: expected '=' in {line:?}", lineno + 1))?;
        let key = line[..eq].trim().to_string();
        let value_text = line[eq + 1..].trim();
        if key.is_empty() {
            return Err(format!("line {}: empty key", lineno + 1));
        }

        let value = parse_value(value_text, lineno + 1)?;
        doc.entries.push((section.clone(), key, value));
    }

    Ok(doc)
}

/// Strip a trailing `# comment`, but preserve `#` inside double-quoted strings.
fn strip_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut in_quote = false;
    let mut prev_backslash = false;
    for (i, &b) in bytes.iter().enumerate() {
        if in_quote {
            if b == b'\\' && !prev_backslash {
                prev_backslash = true;
                continue;
            }
            if b == b'"' && !prev_backslash {
                in_quote = false;
            }
            prev_backslash = false;
        } else if b == b'"' {
            in_quote = true;
        } else if b == b'#' {
            return &line[..i];
        }
    }
    line
}

fn parse_value(s: &str, lineno: usize) -> Result<TomlValue, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err(format!("line {lineno}: empty value"));
    }
    match s.as_bytes()[0] {
        b'"' => parse_string(s, lineno).map(TomlValue::String),
        b'[' => parse_array(s, lineno),
        b't' | b'f' => parse_bool(s, lineno).map(TomlValue::Bool),
        // A numeric literal containing a decimal point or exponent is a
        // float; otherwise an integer. (Hex literals like `0x02` are always
        // quoted strings in our schema, so they never reach this branch.)
        b'-' | b'+' | b'0'..=b'9' => {
            if s.contains('.') || s.contains('e') || s.contains('E') {
                parse_float(s, lineno).map(TomlValue::Float)
            } else {
                parse_int(s, lineno).map(TomlValue::Int)
            }
        }
        _ => Err(format!("line {lineno}: cannot parse value {s:?}")),
    }
}

fn parse_string(s: &str, lineno: usize) -> Result<String, String> {
    if !s.starts_with('"') || !s.ends_with('"') || s.len() < 2 {
        return Err(format!("line {lineno}: malformed string {s:?}"));
    }
    let inner = &s[1..s.len() - 1];
    // Minimal escape handling: \", \\, \n, \t
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(o) => return Err(format!("line {lineno}: bad escape \\{o}")),
                None => return Err(format!("line {lineno}: trailing backslash")),
            }
        } else if c == '"' {
            return Err(format!("line {lineno}: unescaped quote in string"));
        } else {
            out.push(c);
        }
    }
    Ok(out)
}

fn parse_bool(s: &str, lineno: usize) -> Result<bool, String> {
    match s {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(format!("line {lineno}: expected boolean, got {s:?}")),
    }
}

fn parse_int(s: &str, lineno: usize) -> Result<i64, String> {
    s.parse::<i64>()
        .map_err(|_| format!("line {lineno}: invalid integer {s:?}"))
}

fn parse_float(s: &str, lineno: usize) -> Result<f64, String> {
    s.parse::<f64>()
        .map_err(|_| format!("line {lineno}: invalid float {s:?}"))
}

fn parse_array(s: &str, lineno: usize) -> Result<TomlValue, String> {
    if !s.starts_with('[') || !s.ends_with(']') {
        return Err(format!("line {lineno}: malformed array {s:?}"));
    }
    let inner = s[1..s.len() - 1].trim();
    if inner.is_empty() {
        return Ok(TomlValue::Array(Vec::new()));
    }
    let mut items = Vec::new();
    for piece in split_array_items(inner) {
        let v = parse_value(piece.trim(), lineno)?;
        if matches!(v, TomlValue::Array(_)) {
            return Err(format!("line {lineno}: nested arrays not supported"));
        }
        items.push(v);
    }
    Ok(TomlValue::Array(items))
}

/// Split on commas, respecting double-quoted strings.
fn split_array_items(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    let mut prev_backslash = false;
    for c in s.chars() {
        if in_quote {
            (in_quote, prev_backslash) = push_quoted_char(c, &mut cur, prev_backslash);
        } else if c == ',' {
            out.push(cur.trim().to_string());
            cur.clear();
        } else {
            if c == '"' {
                in_quote = true;
            }
            cur.push(c);
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// Consume one char while inside a quoted string, pushing it to `cur`. Returns
/// the updated `(in_quote, prev_backslash)` state: a backslash escapes the next
/// character (suppressing a closing quote), and an unescaped `"` closes the
/// quote.
fn push_quoted_char(c: char, cur: &mut String, prev_backslash: bool) -> (bool, bool) {
    cur.push(c);
    if c == '\\' && !prev_backslash {
        return (true, true); // still quoted; next char is escaped
    }
    let closing = c == '"' && !prev_backslash;
    (!closing, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Requirements: L2-CFG-001
    #[test]
    fn parse_minimal_doc() {
        let text = r#"
[logging]
level = "INFO"

[decode]
time_format = "irig"
strict = true
error_mode = "inline"

[filter]
exclude_types = ["SPURIOUS_DATA", "MODE_COMMAND"]
exclude_rts = [0, 31]
exclude_buses = ["B"]
exclude_subaddresses = []

[output]
format = "csv"
"#;
        let cfg = parse_into_config(text).unwrap();
        assert_eq!(cfg.log_level, "INFO");
        assert_eq!(cfg.time_format, TimestampFormat::Irig);
        assert!(cfg.strict);
        assert_eq!(cfg.error_mode, ErrorMode::Inline);
        assert!(cfg.filters.exclude_types.contains(&0x20));
        assert!(cfg.filters.exclude_types.contains(&0x01));
        assert!(cfg.filters.exclude_rts.contains(&31));
        assert!(cfg.filters.exclude_buses.contains(&Bus::B));
        assert!(cfg.filters.exclude_subaddresses.is_empty());
    }

    /// Requirements: L2-CFG-001
    #[test]
    fn comments_are_stripped() {
        let text = r#"
# leading comment
[decode]
strict = true  # trailing comment
time_format = "auto"
"#;
        let cfg = parse_into_config(text).unwrap();
        assert!(cfg.strict);
        assert_eq!(cfg.time_format, TimestampFormat::Auto);
    }

    /// Requirements: L2-CFG-001
    #[test]
    fn time_format_is_case_insensitive() {
        for (spelling, expected) in [
            ("IRIG", TimestampFormat::Irig),
            ("Irig", TimestampFormat::Irig),
            ("AUTO", TimestampFormat::Auto),
            ("Standard", TimestampFormat::Standard),
        ] {
            let text = format!("[decode]\ntime_format = \"{spelling}\"\n");
            let cfg = parse_into_config(&text).unwrap();
            assert_eq!(cfg.time_format, expected, "spelling {spelling:?}");
        }
        // An unrecognized spelling is still rejected.
        let bad = "[decode]\ntime_format = \"bogus\"\n";
        assert!(parse_into_config(bad).is_err());
    }

    /// Requirements: L2-CFG-001
    #[test]
    fn hash_in_string_not_a_comment() {
        // Verify the parser preserves `#` inside a quoted string rather
        // than treating it as a comment delimiter. Tested at the
        // TOML-parser layer because the validator (Phase 5) now
        // restricts known string-valued keys to enum members, none of
        // which contain `#`.
        let text = r#"
[output]
format = "csv#weird"
"#;
        let doc = parse_toml(text).unwrap();
        match doc.get("output", "format") {
            Some(TomlValue::String(s)) => assert_eq!(s, "csv#weird"),
            other => panic!("expected String(\"csv#weird\"), got {other:?}"),
        }
    }

    /// Requirements: L2-CFG-010
    #[test]
    fn unknown_time_format_rejected() {
        let text = r#"
[decode]
time_format = "potato"
"#;
        assert!(parse_into_config(text).is_err());
    }

    /// Requirements: L2-CFG-007
    #[test]
    fn unknown_type_name_rejected() {
        let text = r#"
[filter]
exclude_types = ["UNICORN"]
"#;
        assert!(parse_into_config(text).is_err());
    }

    /// Requirements: L2-CFG-010
    #[test]
    fn unknown_log_level_rejected_at_parse_time() {
        // Regression: previously the config parser accepted any string
        // as logging.level and the bad value was silently dropped at
        // apply time. Now the parser fails fast with a value-level
        // diagnostic.
        let text = "[logging]\nlevel = \"NOPE\"\n";
        let err = parse_into_config(text).unwrap_err();
        assert!(
            err.0.contains("logging.level"),
            "error should mention the field: {}",
            err.0
        );
        assert!(err.0.contains("NOPE"));
    }

    /// Requirements: L2-CFG-010
    #[test]
    fn known_log_levels_accepted_case_insensitively() {
        for level in [
            "DEBUG", "info", "Warning", "WARN", "error", "CRITICAL", "OFF", "off",
        ] {
            let text = format!("[logging]\nlevel = \"{level}\"\n");
            parse_into_config(&text)
                .unwrap_or_else(|e| panic!("expected {level:?} to parse, got: {}", e.0));
        }
    }

    // ── L2-CFG schema validations (Phase 5) ──────────────────────────

    /// Requirements: L2-CFG-010
    #[test]
    fn unknown_output_format_rejected() {
        let text = "[output]\nformat = \"json\"\n";
        let err = parse_into_config(text).unwrap_err();
        assert!(
            err.0.contains("output.format"),
            "error should name the field: {}",
            err.0
        );
        assert!(err.0.contains("json"));
    }

    /// Requirements: L2-CFG-010
    #[test]
    fn output_format_csv_still_accepted() {
        let text = "[output]\nformat = \"csv\"\n";
        let cfg = parse_into_config(text).unwrap();
        assert_eq!(cfg.output_format, "csv");
    }

    /// Requirements: L2-CFG-010
    #[test]
    fn exclude_rts_out_of_range_rejected() {
        // L2-CFG: RT must be in [0, 31]. 32 is out of range.
        let text = "[filter]\nexclude_rts = [32]\n";
        let err = parse_into_config(text).unwrap_err();
        assert!(
            err.0.contains("exclude_rts") && err.0.contains("[0, 31]"),
            "expected range error mentioning [0, 31]: {}",
            err.0
        );
    }

    /// Requirements: L2-CFG-010
    #[test]
    fn exclude_subaddresses_negative_rejected() {
        let text = "[filter]\nexclude_subaddresses = [-1]\n";
        let err = parse_into_config(text).unwrap_err();
        assert!(
            err.0.contains("exclude_subaddresses") && err.0.contains("[0, 31]"),
            "expected range error mentioning [0, 31]: {}",
            err.0
        );
    }

    /// Requirements: L2-CFG-010
    #[test]
    fn exclude_rts_zero_and_thirty_one_accepted() {
        // Boundary values must still parse.
        let text = "[filter]\nexclude_rts = [0, 31]\n";
        let cfg = parse_into_config(text).unwrap();
        assert_eq!(cfg.filters.exclude_rts, vec![0, 31]);
    }

    /// Requirements: L2-CFG-009
    #[test]
    fn unknown_top_level_key_is_warned_not_rejected() {
        // L2-CFG-009: unknown keys WARN at load time but do not fail
        // the load — preserves forward compatibility.
        let text = "[output]\nformat = \"csv\"\nunknown_thing = true\n";
        let cfg = parse_into_config(text).expect("unknown key should warn, not fail");
        assert_eq!(cfg.output_format, "csv");
    }

    /// Requirements: L2-CFG-009
    #[test]
    fn unknown_filter_key_is_warned_not_rejected() {
        // Common typo: exclude_subdresses (missing 'ad').
        let text = "[filter]\nexclude_subdresses = [0]\n";
        let cfg = parse_into_config(text).expect("typo'd key should warn, not fail");
        // The misspelled key gets WARN'd; the correctly-spelled key
        // (had it been written) would not be filtered, so default empty.
        assert!(cfg.filters.exclude_subaddresses.is_empty());
    }

    /// Requirements: L2-CFG-001
    #[test]
    fn missing_eq_returns_line_number() {
        let text = "[decode]\nstrict true\n";
        let err = parse_into_config(text).unwrap_err();
        assert!(err.0.contains("line 2"));
    }

    /// Requirements: L2-CFG-003
    #[test]
    fn defaults_when_no_path() {
        let cfg = load_config(None).unwrap();
        assert_eq!(cfg.log_level, "WARNING");
        assert_eq!(cfg.time_format, TimestampFormat::Auto);
        assert_eq!(cfg.error_mode, ErrorMode::Separate);
        assert!(!cfg.strict);
    }

    /// Requirements: L2-CFG-003, L2-CFG-004
    #[test]
    fn overrides_apply_and_filter_merge() {
        let cfg = DecoderConfig {
            filters: FilterConfig {
                exclude_rts: vec![31],
                ..Default::default()
            },
            ..Default::default()
        };
        let merged = cfg.with_overrides(ConfigOverrides {
            time_format: Some(TimestampFormat::Standard),
            exclude_rts: vec![0],
            ..Default::default()
        });
        assert_eq!(merged.time_format, TimestampFormat::Standard);
        assert_eq!(merged.filters.exclude_rts, vec![31, 0]);
    }

    /// Requirements: L2-CFG-007
    #[test]
    fn type_name_parsing() {
        assert_eq!(parse_type_name("BC_TO_RT").unwrap(), 0x02);
        assert_eq!(parse_type_name("0x20").unwrap(), 0x20);
        assert!(parse_type_name("nope").is_err());
    }

    /// Requirements: L2-CFG-008
    #[test]
    fn parses_default_toml_from_disk() {
        let path = Path::new("config/default.toml");
        if path.exists() {
            let cfg = load_config(Some(path)).unwrap();
            assert_eq!(cfg.output_format, "csv");
            // The four keys added to keep the starter file complete must
            // parse to their documented defaults (correct section + type).
            assert!(!cfg.allow_partial);
            assert!(!cfg.no_clobber);
            assert_eq!(cfg.detect_records, 8);
            assert_eq!(cfg.lookahead_records, 2);
        }
    }

    /// The advertised "fully-commented starter file" must actually contain
    /// every key documented in `docs/CONFIG-REFERENCE.md` (active or as a
    /// commented example), so the reference config can't silently drift
    /// incomplete again.
    /// Requirements: L2-CFG-001
    #[test]
    fn default_toml_documents_every_schema_key() {
        let path = Path::new("config/default.toml");
        if !path.exists() {
            return;
        }
        let text = std::fs::read_to_string(path).unwrap();
        let documents = |key: &str| {
            text.lines().any(|line| {
                let line = line.trim_start_matches('#').trim_start();
                line.starts_with(&format!("{key} ")) || line.starts_with(&format!("{key}="))
            })
        };
        for key in [
            "level",
            "time_format",
            "strict",
            "error_mode",
            "allow_partial",
            "detect_records",
            "lookahead_records",
            "standard_tick_rate_hz",
            "format",
            "no_clobber",
            "exclude_types",
            "exclude_rts",
            "exclude_buses",
            "exclude_subaddresses",
        ] {
            assert!(
                documents(key),
                "config/default.toml is missing documented key `{key}` \
                 (see docs/CONFIG-REFERENCE.md)"
            );
        }
    }

    /// Requirements: L2-CFG-011, L2-DEC-017
    #[test]
    fn standard_tick_rate_hz_default_is_none() {
        let cfg = parse_into_config("[decode]\ntime_format = \"standard\"\n").unwrap();
        assert_eq!(cfg.standard_tick_rate_hz, None);
    }

    /// Requirements: L2-CFG-011
    #[test]
    fn standard_tick_rate_hz_accepts_float_and_int() {
        let as_float = parse_into_config("[decode]\nstandard_tick_rate_hz = 1000000.0\n").unwrap();
        assert_eq!(as_float.standard_tick_rate_hz, Some(1_000_000.0));
        // An operator may write a bare integer; get_float coerces it.
        let as_int = parse_into_config("[decode]\nstandard_tick_rate_hz = 1000000\n").unwrap();
        assert_eq!(as_int.standard_tick_rate_hz, Some(1_000_000.0));
    }

    /// Requirements: L2-CFG-011, L2-CFG-010
    #[test]
    fn standard_tick_rate_hz_rejects_nonpositive() {
        for bad in ["0", "0.0", "-1.0"] {
            let text = format!("[decode]\nstandard_tick_rate_hz = {bad}\n");
            let err = parse_into_config(&text).unwrap_err();
            assert!(
                err.0.contains("standard_tick_rate_hz"),
                "error should name the field for {bad:?}: {}",
                err.0
            );
        }
    }

    /// Requirements: L2-CFG-003
    #[test]
    fn standard_tick_rate_hz_override_applies() {
        let merged = DecoderConfig::default().with_overrides(ConfigOverrides {
            standard_tick_rate_hz: Some(2_000_000.0),
            ..Default::default()
        });
        assert_eq!(merged.standard_tick_rate_hz, Some(2_000_000.0));
    }

    /// Requirements: L2-CFG-001
    #[test]
    fn parses_float_value() {
        let doc = parse_toml("[decode]\nstandard_tick_rate_hz = 1.5e6\n").unwrap();
        match doc.get("decode", "standard_tick_rate_hz") {
            Some(TomlValue::Float(f)) => assert_eq!(*f, 1_500_000.0),
            other => panic!("expected Float(1500000.0), got {other:?}"),
        }
    }
}
