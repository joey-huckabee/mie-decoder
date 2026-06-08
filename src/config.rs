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

use crate::decode::DEFAULT_DETECT_RECORDS;
use crate::filter::FilterConfig;
use crate::models::{Bus, ErrorMode, MessageType, TimestampFormat};

/// L2-DEC-015 valid range for `decode.detect_records`. Values outside
/// this range are rejected at config-load time with a clear error.
pub const DETECT_RECORDS_MIN: usize = 1;
pub const DETECT_RECORDS_MAX: usize = 32;

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
        }
    }
}

/// Override container: every field is optional and only applied if `Some`.
/// Filter overrides MERGE into the existing set rather than replacing it,
/// matching the Python `with_overrides` semantics.
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

    if let Some(level) = toml.get_string("logging", "level")? {
        let upper = level.to_uppercase();
        // Validate at load time so the error points at the config file
        // rather than surfacing later as a silent no-op.
        if crate::log::Level::parse(&upper).is_none() {
            return Err(ConfigError(format!(
                "Invalid logging.level: {level:?}. \
                 Valid: DEBUG, INFO, WARNING, ERROR, CRITICAL"
            )));
        }
        cfg.log_level = upper;
    }
    if let Some(tf) = toml.get_string("decode", "time_format")? {
        cfg.time_format = parse_time_format(tf)?;
    }
    if let Some(b) = toml.get_bool("decode", "strict")? {
        cfg.strict = b;
    }
    if let Some(em) = toml.get_string("decode", "error_mode")? {
        cfg.error_mode = parse_error_mode(em)?;
    }
    if let Some(fmt) = toml.get_string("output", "format")? {
        // L2-CFG-010: validate enum membership at load time. `csv` is
        // the only output format in v1; forward-compat for future
        // formats (Parquet, etc.) will widen this list.
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
    if let Some(b) = toml.get_bool("decode", "allow_partial")? {
        cfg.allow_partial = b;
    }
    if let Some(n) = toml.get_int("decode", "detect_records")? {
        // L2-DEC-015: validate range [1, 32] at load time per
        // L2-CFG-010. A nonpositive or oversized value would otherwise
        // silently degrade detection quality.
        if n < DETECT_RECORDS_MIN as i64 || n > DETECT_RECORDS_MAX as i64 {
            return Err(ConfigError(format!(
                "Invalid decode.detect_records: {n}. \
                 Valid range: [{DETECT_RECORDS_MIN}, {DETECT_RECORDS_MAX}]"
            )));
        }
        cfg.detect_records = n as usize;
    }

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

    // L2-CFG-009: WARN on unknown `[section] key` entries so typos in a
    // config file (e.g., `exclude_subdresses`) surface to the operator
    // instead of being silently dropped. Non-fatal so forward-compatible
    // additions don't break older configs.
    for (section, key, _) in &toml.entries {
        if !is_known_shared_key(section.as_str(), key.as_str()) {
            crate::log_warn!("unknown TOML key: [{section}] {key}");
        }
    }

    Ok(cfg)
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
            | ("output", "format")
            | ("output", "no_clobber")
            | ("filter", "exclude_types")
            | ("filter", "exclude_rts")
            | ("filter", "exclude_buses")
            | ("filter", "exclude_subaddresses")
    )
}

// ── Helpers for value coercion ────────────────────────────────────────

fn parse_time_format(s: &str) -> Result<TimestampFormat, ConfigError> {
    match s.to_ascii_lowercase().as_str() {
        "auto" => Ok(TimestampFormat::Auto),
        "irig" => Ok(TimestampFormat::Irig),
        "standard" => Ok(TimestampFormat::Standard),
        other => Err(ConfigError(format!(
            "Invalid time_format: {other:?}. Valid: auto, irig, standard"
        ))),
    }
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
        b'-' | b'+' | b'0'..=b'9' => parse_int(s, lineno).map(TomlValue::Int),
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
            cur.push(c);
            if c == '\\' && !prev_backslash {
                prev_backslash = true;
                continue;
            }
            if c == '"' && !prev_backslash {
                in_quote = false;
            }
            prev_backslash = false;
        } else {
            match c {
                ',' => {
                    out.push(cur.trim().to_string());
                    cur.clear();
                }
                '"' => {
                    in_quote = true;
                    cur.push(c);
                }
                _ => cur.push(c),
            }
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
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
        for level in ["DEBUG", "info", "Warning", "WARN", "error"] {
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
        }
    }
}
