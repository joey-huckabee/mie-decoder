//! Hand-rolled argument parser and CLI dispatch.
//!
//! Surface:
//!
//! ```text
//! mie-decoder [--log-level L] [--config PATH] <command> [opts...]
//! ```
//!
//! Commands: `decode`, `count`, `dump`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::config::{ConfigOverrides, DecoderConfig, load_config, parse_bus_name, parse_type_name};
use crate::dump::{hex_dump_raw_to_stdout, hex_dump_records_to_stdout};
use crate::error::MieError;
use crate::filter::FilterIterExt;
use crate::log::{self, Level};
use crate::models::{ErrorMode, TimestampFormat};
use crate::reader::{MieFileReader, ReaderOptions};
use crate::writer::{WriteOptions, write_csv, write_csv_split};
use crate::{log_error, log_info, log_warn};

const VERSION: &str = env!("CARGO_PKG_VERSION");

const HELP: &str = "\
mie-decoder — DDC MIL-STD-1553 MIE binary decoder

USAGE:
  mie-decoder [--log-level L] [--config PATH] <command> [options]

COMMANDS:
  decode <INPUT>... Decode MIE file(s) to CSV (2+ inputs → time-sorted merge)
  count  <INPUT>    Print message count (no CSV)
  dump   <INPUT>    Hex dump (raw or record-aware)

GLOBAL OPTIONS:
  --log-level LEVEL                     DEBUG|INFO|WARNING|WARN|ERROR|
                                        CRITICAL|OFF (default WARNING;
                                        case-insensitive; CRITICAL/OFF silence)
  --config PATH                         TOML configuration file
  -V, --version                         Print version and exit
  -h, --help                            Print this help and exit

DECODE OPTIONS:
  -o, --output PATH                     Output CSV (default stdout)
  --manifest PATH                       Read input paths from a file (one per
                                        line; blank/#-comment lines ignored).
                                        Mutually exclusive with positionals /
                                        --glob (L2-MRG-001)
  --glob PATTERN                        Expand a single-directory *|? filename
                                        glob (no recursion). Mutually exclusive
                                        with positionals / --manifest
  --inline-errors                       Errors inline in main CSV
                                        (default: separate <stem>_errors.csv)
  --no-clobber                          Refuse to overwrite an existing
                                        output file (L2-WRT-017)
  --allow-partial                       On unrecoverable mid-file sync
                                        loss, write a <output>.partial
                                        file and exit 0 instead of 3
                                        (L1-EXIT-004)
  --time-format auto|irig|standard      Default auto
  --detect-records N                    Records probed by timestamp-
                                        format auto-detection (1..=32,
                                        default 8). L2-DEC-015.
  --lookahead-records N                 Total records checked by sync
                                        validation per call (1 candidate
                                        + N-1 look-ahead, range 1..=32,
                                        default 2). L2-SYN-026.
  --standard-tick-rate-hz HZ            Standard-counter frequency in Hz.
                                        When set, Standard timestamps are
                                        converted to microseconds and join
                                        DELTA tracking. Must be > 0
                                        (default: unset → empty DELTA for
                                        Standard). L2-DEC-017.
  --strict                              Raise on invalid records
  --format csv                          Output format (csv only at present)
  --no-mux                              Leave the MUX column empty
                                        (vendor-exact). Default: MUX is
                                        derived from the file name (L2-WRT-020)
  --mux-delimiter D                     MUX field separator (default '.')
  --mux-field N                         0-based MUX field index; negative
                                        counts from the end (default 4)
  --collapse-duplicates                 Collapse the same bus transaction seen
                                        by multiple recorders into one row
                                        (multi-file merge only). Default: off
  --collapse-window-us N                Timestamp tolerance in microseconds for
                                        collapsing (default 0 = exact match)
  --exclude-types VAL                   Comma-separated names or 0xNN
  --exclude-rts VAL                     Comma-separated RT addresses
  --exclude-buses VAL                   Comma-separated A|B
  --exclude-subaddresses VAL            Comma-separated subaddresses
  --include-types VAL                   (same syntax as --exclude-types)
  --include-rts VAL
  --include-buses VAL
  --include-subaddresses VAL

  Filter flags accept ONE value (comma-separable). Repeat the flag to
  accumulate. `--include-rts 15,31` and `--include-rts 15 --include-rts 31`
  are equivalent. Appending `=value` (e.g. `--include-rts=15`) also works.

DUMP OPTIONS:
  --raw                                 Raw hex dump (no record parsing)
  --offset N                            Start offset (decimal or 0xHEX)
  --length N                            Bytes to dump (raw mode)
  --records N                           Max records to dump (record mode)

EXAMPLES:
  mie-decoder decode rec.mie -o out.csv
  mie-decoder decode rec.mie --inline-errors --include-rts 15
  mie-decoder decode a.mie b.mie c.mie -o merged.csv   # time-sorted merge
  mie-decoder decode --glob 'recordings/*.mie' -o merged.csv
  mie-decoder count rec.mie
  mie-decoder dump rec.mie --records 10
";

#[derive(Debug)]
enum Command {
    // Boxed because DecodeArgs is much larger than the other variants
    // (clippy::large_enum_variant). Heap-allocating the rare path keeps
    // the common variants cheap.
    Decode(Box<DecodeArgs>),
    Count(PathBuf),
    Dump(Box<DumpArgs>),
}

#[derive(Debug, Default)]
struct DecodeArgs {
    /// One or more positional input paths. Mutually exclusive with
    /// `manifest` / `glob` (L2-MRG-001). More than one resolved input ⇒ merge.
    inputs: Vec<PathBuf>,
    /// `--manifest <file>`: read input paths from a file (one per line).
    manifest: Option<PathBuf>,
    /// `--glob <pattern>`: expand a single-directory `*`/`?` filename glob.
    glob: Option<String>,
    output: Option<PathBuf>,
    inline_errors: bool,
    no_clobber: bool,
    allow_partial: bool,
    time_format: Option<TimestampFormat>,
    detect_records: Option<usize>,
    lookahead_records: Option<usize>,
    standard_tick_rate_hz: Option<f64>,
    strict: Option<bool>,
    output_format: Option<String>,
    /// `--no-mux`: disable MUX-from-filename population (vendor-exact output).
    no_mux: bool,
    /// `--mux-delimiter <D>`: field separator for MUX extraction.
    mux_delimiter: Option<String>,
    /// `--mux-field <N>`: 0-based field index (negative = from end) for MUX.
    mux_field: Option<i64>,
    /// `--collapse-duplicates`: collapse cross-recorder duplicate messages.
    collapse_duplicates: bool,
    /// `--collapse-window-us <N>`: timestamp tolerance (µs) for collapsing.
    collapse_window_us: Option<i64>,

    exclude_types: Vec<u8>,
    exclude_rts: Vec<u8>,
    exclude_buses: Vec<crate::models::Bus>,
    exclude_subaddresses: Vec<u8>,

    include_types: Vec<u8>,
    include_rts: Vec<u8>,
    include_buses: Vec<crate::models::Bus>,
    include_subaddresses: Vec<u8>,
}

#[derive(Debug, Default)]
struct DumpArgs {
    input: PathBuf,
    raw: bool,
    offset: usize,
    length: Option<usize>,
    records: Option<u64>,
}

#[derive(Debug, Default)]
struct GlobalArgs {
    log_level: Option<String>,
    config: Option<PathBuf>,
}

/// Process exit codes, the normative contract pinned by L2-CLI-011 /
/// L1-EXIT-002..008. Kept as named constants so every exit site is
/// self-documenting and the taxonomy lives in one place.
mod exit_code {
    /// Runtime / decode error: input I/O (incl. file-not-found), writer
    /// failure, strict-mode record & structural-invariant failures.
    pub(crate) const RUNTIME: u8 = 1;
    /// No valid records — the input is not an MIE recording.
    pub(crate) const NO_RECORDS: u8 = 2;
    /// Unrecoverable mid-file sync loss without `--allow-partial`.
    pub(crate) const SYNC_LOSS: u8 = 3;
    /// CLI usage error: unknown/missing/invalid flag or argument,
    /// unknown subcommand, bad flag value.
    pub(crate) const USAGE: u8 = 4;
    /// Configuration error: config file not found, malformed TOML, or an
    /// invalid configuration value.
    pub(crate) const CONFIG: u8 = 5;
    /// Merge-incompatible inputs: a multi-file merge whose inputs cannot be
    /// ordered on a common absolute timeline (L1-EXIT-009 / L2-MRG-003).
    pub(crate) const MERGE_INCOMPATIBLE: u8 = 6;
}

/// A subcommand failure carrying the exit code it should map to. Lets the
/// runners distinguish a configuration error (`CONFIG`) from a generic
/// runtime/decode error (`RUNTIME`) without flattening both to exit 1.
struct CliError {
    code: u8,
    message: String,
}

impl CliError {
    fn runtime(message: impl Into<String>) -> Self {
        Self {
            code: exit_code::RUNTIME,
            message: message.into(),
        }
    }
    fn config(message: impl Into<String>) -> Self {
        Self {
            code: exit_code::CONFIG,
            message: message.into(),
        }
    }
    fn usage(message: impl Into<String>) -> Self {
        Self {
            code: exit_code::USAGE,
            message: message.into(),
        }
    }
    /// Map a decode-time `MieError` surfaced by the reader to the CLI exit
    /// class, mirroring `classify_decode_exit`'s error arms so `count` and
    /// `decode` agree: a "wrong file type" rejection (`NoValidRecords`,
    /// `HomogeneousPayload`, or a strict-mode `TimestampFormatMismatch`)
    /// maps to `NO_RECORDS` (exit 2); everything else stays `RUNTIME`
    /// (exit 1). (Previously `count` flattened all reader errors to exit 1.)
    fn from_decode_error(e: MieError) -> Self {
        let code = match e.kind() {
            crate::error::MieErrorKind::NoValidRecords
            | crate::error::MieErrorKind::HomogeneousPayload
            | crate::error::MieErrorKind::TimestampFormatMismatch => exit_code::NO_RECORDS,
            _ => exit_code::RUNTIME,
        };
        Self {
            code,
            message: format_mie_error(e),
        }
    }
}

// ── Top-level entry ───────────────────────────────────────────────────

pub fn run(argv: Vec<String>) -> ExitCode {
    let mut iter = argv.into_iter().skip(1).peekable();

    // Pull global flags + --help / --version that may appear before the command.
    let mut globals = GlobalArgs::default();
    let cmd_token = match parse_global_flags(&mut iter, &mut globals) {
        Ok(token) => token,
        Err(code) => return code,
    };
    let Some(cmd_token) = cmd_token else {
        eprint!("{HELP}");
        return ExitCode::from(exit_code::USAGE);
    };

    // Parse subcommand-specific args. Process control (printing help,
    // selecting an exit code) is decided inside `parse_subcommand`.
    let command = match parse_subcommand(&cmd_token, &mut iter) {
        Ok(c) => c,
        Err(code) => return code,
    };

    // Apply log level early so the version banner respects it. CLI
    // value if provided, else WARN default. An invalid CLI value
    // (e.g. `--log-level NOPE`) fails fast here with exit 2 instead
    // of being silently ignored. The config file's level is layered
    // on top later inside resolve_config.
    if let Some(s) = globals.log_level.as_deref() {
        if let Err(msg) = apply_log_level("--log-level", s) {
            return die(&msg);
        }
    } else {
        log::set_level(Level::Warn);
    }

    log_info!("mie-decoder v{VERSION}");

    // Decode returns a Result<ExitCode, CliError> so it can choose exit
    // codes 2 (no-records) and 3 (partial-unrecoverable) directly. The
    // CliError on the failure path carries its own code so a config
    // error (5) is distinguished from a generic runtime error (1).
    // Count/Dump use the simpler Result<(), CliError> contract and map
    // Ok to exit 0.
    let result: Result<ExitCode, CliError> = match command {
        Command::Decode(args) => run_decode(globals, *args),
        Command::Count(input) => run_count(globals, input).map(|()| ExitCode::SUCCESS),
        Command::Dump(args) => run_dump(globals, *args).map(|()| ExitCode::SUCCESS),
    };

    match result {
        Ok(code) => code,
        Err(e) => {
            log_error!("{}", e.message);
            eprintln!("Error: {}", e.message);
            ExitCode::from(e.code)
        }
    }
}

fn die(msg: &str) -> ExitCode {
    eprintln!("Error: {msg}\n\n{HELP}");
    ExitCode::from(exit_code::USAGE)
}

/// Consume leading global flags (`--log-level`, `--config`) and `-h`/`-V`.
/// Returns `Ok(Some(token))` with the first non-flag token (the subcommand),
/// `Ok(None)` if none follows, or `Err(code)` when the caller (`run`) should
/// return that exit code immediately (help/version printed, or a usage error).
fn parse_global_flags(
    iter: &mut ArgIter<'_>,
    globals: &mut GlobalArgs,
) -> Result<Option<String>, ExitCode> {
    loop {
        match iter.peek().map(String::as_str) {
            Some("-h") | Some("--help") => {
                print!("{HELP}");
                return Err(ExitCode::SUCCESS);
            }
            Some("-V") | Some("--version") => {
                println!("mie-decoder {VERSION}");
                return Err(ExitCode::SUCCESS);
            }
            Some("--log-level") => {
                iter.next();
                match iter.next() {
                    Some(v) => globals.log_level = Some(v),
                    None => return Err(die("--log-level requires a value")),
                }
            }
            Some(s) if s.starts_with("--log-level=") => {
                let Some(v) = iter.next() else {
                    return Err(die("--log-level requires a value"));
                };
                globals.log_level = Some(v["--log-level=".len()..].to_string());
            }
            Some("--config") => {
                iter.next();
                match iter.next() {
                    Some(v) => globals.config = Some(PathBuf::from(v)),
                    None => return Err(die("--config requires a path")),
                }
            }
            Some(s) if s.starts_with("--config=") => {
                let Some(v) = iter.next() else {
                    return Err(die("--config requires a path"));
                };
                globals.config = Some(PathBuf::from(&v["--config=".len()..]));
            }
            Some(_) => return Ok(iter.next()),
            None => {
                eprint!("{HELP}");
                return Err(ExitCode::from(exit_code::USAGE));
            }
        }
    }
}

/// Turn a subcommand parse result into `Command`, or an `Err(code)` for `run`
/// to return: `HelpRequested` prints help and exits `0`; `Other` dies (usage).
fn parse_or_help<T>(result: Result<T, ParseError>) -> Result<T, ExitCode> {
    match result {
        Ok(c) => Ok(c),
        Err(ParseError::HelpRequested) => {
            print!("{HELP}");
            Err(ExitCode::SUCCESS)
        }
        Err(ParseError::Other(e)) => Err(die(&e)),
    }
}

/// Dispatch `cmd_token` to its argument parser, wrapping the parsed args in a
/// `Command`. `Err(code)` means `run` should return that exit code.
fn parse_subcommand(cmd_token: &str, iter: &mut ArgIter<'_>) -> Result<Command, ExitCode> {
    match cmd_token {
        "decode" => Ok(Command::Decode(Box::new(parse_or_help(parse_decode(
            iter,
        ))?))),
        "count" => Ok(Command::Count(parse_or_help(parse_count(iter))?)),
        "dump" => Ok(Command::Dump(Box::new(parse_or_help(parse_dump(iter))?))),
        "-h" | "--help" => {
            print!("{HELP}");
            Err(ExitCode::SUCCESS)
        }
        other => Err(die(&format!("Unknown command: {other:?}"))),
    }
}

// ── Subcommand parsing ────────────────────────────────────────────────

type ArgIter<'a> = std::iter::Peekable<std::iter::Skip<std::vec::IntoIter<String>>>;

/// Outcome of parsing subcommand arguments.
///
/// `HelpRequested` is a control-flow signal, not a failure: the user
/// passed `-h`/`--help` and the caller (`run`) is responsible for
/// printing help text and returning the appropriate exit code. Library
/// helpers MUST NOT call `std::process::exit` directly; that decision
/// belongs to the binary entry point.
#[derive(Debug)]
pub enum ParseError {
    HelpRequested,
    Other(String),
}

impl From<String> for ParseError {
    fn from(s: String) -> Self {
        Self::Other(s)
    }
}

fn next_value(name: &str, iter: &mut ArgIter<'_>) -> Result<String, String> {
    iter.next()
        .ok_or_else(|| format!("{name} requires a value"))
}

fn parse_int_value(s: &str, name: &str) -> Result<usize, String> {
    let s = s.trim();
    let parsed = if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        usize::from_str_radix(hex, 16)
    } else {
        s.parse::<usize>()
    };
    parsed.map_err(|_| format!("{name} expected integer, got {s:?}"))
}

fn parse_u8_value(s: &str, name: &str) -> Result<u8, String> {
    parse_int_value(s, name).and_then(|n| {
        u8::try_from(n).map_err(|_| format!("{name} value out of range (0-255): {n}"))
    })
}

/// Split a single value on commas (trimmed, empties dropped).
///
/// Replaces the old greedy "consume tokens until next flag" helper, which
/// produced surprising behavior when a positional argument followed a
/// filter flag (`--include-rts 15 file.mie` ate `file.mie` as another RT).
/// Filter flags now take one value; pass multiple with commas
/// (`--include-rts 15,31`) or by repeating the flag
/// (`--include-rts 15 --include-rts 31`).
fn split_csv(s: &str) -> Vec<String> {
    s.split(',')
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

/// Parse a comma-separated filter value and push each parsed item into `target`.
/// Shared by all `--exclude-*` / `--include-*` flags so their per-value loop
/// lives in one place.
fn push_filter<T>(
    csv: &str,
    target: &mut Vec<T>,
    parse: impl Fn(&str) -> Result<T, ParseError>,
) -> Result<(), ParseError> {
    for v in split_csv(csv) {
        target.push(parse(&v)?);
    }
    Ok(())
}

/// One `--exclude-types` / `--include-types` element → a message-type code.
fn parse_type_filter(v: &str) -> Result<u8, ParseError> {
    parse_type_name(v).map_err(|e| e.0.into())
}

/// One `--exclude-buses` / `--include-buses` element → a `Bus`.
fn parse_bus_filter(v: &str) -> Result<crate::models::Bus, ParseError> {
    parse_bus_name(v).map_err(|e| e.0.into())
}

fn parse_decode(iter: &mut ArgIter<'_>) -> Result<DecodeArgs, ParseError> {
    let mut args = DecodeArgs::default();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-o" | "--output" => {
                args.output = Some(PathBuf::from(next_value("--output", iter)?));
            }
            s if s.starts_with("--output=") => {
                args.output = Some(PathBuf::from(&s["--output=".len()..]));
            }
            "--inline-errors" => args.inline_errors = true,
            "--no-clobber" => args.no_clobber = true,
            "--allow-partial" => args.allow_partial = true,
            "--strict" => args.strict = Some(true),
            "--time-format" => {
                let v = next_value("--time-format", iter)?;
                args.time_format = Some(parse_time_format_arg(&v)?);
            }
            s if s.starts_with("--time-format=") => {
                args.time_format = Some(parse_time_format_arg(&s["--time-format=".len()..])?);
            }
            "--detect-records" => {
                let v = next_value("--detect-records", iter)?;
                args.detect_records = Some(parse_detect_records(&v)?);
            }
            s if s.starts_with("--detect-records=") => {
                args.detect_records = Some(parse_detect_records(&s["--detect-records=".len()..])?);
            }
            "--lookahead-records" => {
                let v = next_value("--lookahead-records", iter)?;
                args.lookahead_records = Some(parse_lookahead_records(&v)?);
            }
            s if s.starts_with("--lookahead-records=") => {
                args.lookahead_records =
                    Some(parse_lookahead_records(&s["--lookahead-records=".len()..])?);
            }
            "--standard-tick-rate-hz" => {
                let v = next_value("--standard-tick-rate-hz", iter)?;
                args.standard_tick_rate_hz = Some(parse_standard_tick_rate_hz(&v)?);
            }
            s if s.starts_with("--standard-tick-rate-hz=") => {
                args.standard_tick_rate_hz = Some(parse_standard_tick_rate_hz(
                    &s["--standard-tick-rate-hz=".len()..],
                )?);
            }
            "--format" => {
                args.output_format = Some(next_value("--format", iter)?);
            }
            s if s.starts_with("--format=") => {
                args.output_format = Some(s["--format=".len()..].to_string());
            }
            "--no-mux" => args.no_mux = true,
            "--mux-delimiter" => {
                args.mux_delimiter =
                    Some(parse_mux_delimiter(&next_value("--mux-delimiter", iter)?)?);
            }
            s if s.starts_with("--mux-delimiter=") => {
                args.mux_delimiter = Some(parse_mux_delimiter(&s["--mux-delimiter=".len()..])?);
            }
            "--mux-field" => {
                args.mux_field = Some(parse_mux_field(&next_value("--mux-field", iter)?)?);
            }
            s if s.starts_with("--mux-field=") => {
                args.mux_field = Some(parse_mux_field(&s["--mux-field=".len()..])?);
            }
            "--collapse-duplicates" => args.collapse_duplicates = true,
            "--collapse-window-us" => {
                args.collapse_window_us = Some(parse_collapse_window_us(&next_value(
                    "--collapse-window-us",
                    iter,
                )?)?);
            }
            s if s.starts_with("--collapse-window-us=") => {
                args.collapse_window_us = Some(parse_collapse_window_us(
                    &s["--collapse-window-us=".len()..],
                )?);
            }
            "--manifest" => {
                args.manifest = Some(PathBuf::from(next_value("--manifest", iter)?));
            }
            s if s.starts_with("--manifest=") => {
                args.manifest = Some(PathBuf::from(&s["--manifest=".len()..]));
            }
            "--glob" => {
                args.glob = Some(next_value("--glob", iter)?);
            }
            s if s.starts_with("--glob=") => {
                args.glob = Some(s["--glob=".len()..].to_string());
            }
            // Filter flags: each takes ONE value. Multiple values either
            // repeat the flag or comma-separate within one value:
            //   --include-rts 15
            //   --include-rts 15,20,31
            //   --include-rts 15 --include-rts 31
            // Any of those leaves trailing positionals like `file.mie`
            // free to bind to `args.inputs`. Both space- and `=`-form
            // value syntax are accepted.
            "--exclude-types" => push_filter(
                &next_value("--exclude-types", iter)?,
                &mut args.exclude_types,
                parse_type_filter,
            )?,
            s if s.starts_with("--exclude-types=") => push_filter(
                &s["--exclude-types=".len()..],
                &mut args.exclude_types,
                parse_type_filter,
            )?,
            "--include-types" => push_filter(
                &next_value("--include-types", iter)?,
                &mut args.include_types,
                parse_type_filter,
            )?,
            s if s.starts_with("--include-types=") => push_filter(
                &s["--include-types=".len()..],
                &mut args.include_types,
                parse_type_filter,
            )?,
            "--exclude-rts" => push_filter(
                &next_value("--exclude-rts", iter)?,
                &mut args.exclude_rts,
                |v| parse_u8_value(v, "--exclude-rts").map_err(Into::into),
            )?,
            s if s.starts_with("--exclude-rts=") => {
                push_filter(&s["--exclude-rts=".len()..], &mut args.exclude_rts, |v| {
                    parse_u8_value(v, "--exclude-rts").map_err(Into::into)
                })?
            }
            "--include-rts" => push_filter(
                &next_value("--include-rts", iter)?,
                &mut args.include_rts,
                |v| parse_u8_value(v, "--include-rts").map_err(Into::into),
            )?,
            s if s.starts_with("--include-rts=") => {
                push_filter(&s["--include-rts=".len()..], &mut args.include_rts, |v| {
                    parse_u8_value(v, "--include-rts").map_err(Into::into)
                })?
            }
            "--exclude-buses" => push_filter(
                &next_value("--exclude-buses", iter)?,
                &mut args.exclude_buses,
                parse_bus_filter,
            )?,
            s if s.starts_with("--exclude-buses=") => push_filter(
                &s["--exclude-buses=".len()..],
                &mut args.exclude_buses,
                parse_bus_filter,
            )?,
            "--include-buses" => push_filter(
                &next_value("--include-buses", iter)?,
                &mut args.include_buses,
                parse_bus_filter,
            )?,
            s if s.starts_with("--include-buses=") => push_filter(
                &s["--include-buses=".len()..],
                &mut args.include_buses,
                parse_bus_filter,
            )?,
            "--exclude-subaddresses" => push_filter(
                &next_value("--exclude-subaddresses", iter)?,
                &mut args.exclude_subaddresses,
                |v| parse_u8_value(v, "--exclude-subaddresses").map_err(Into::into),
            )?,
            s if s.starts_with("--exclude-subaddresses=") => push_filter(
                &s["--exclude-subaddresses=".len()..],
                &mut args.exclude_subaddresses,
                |v| parse_u8_value(v, "--exclude-subaddresses").map_err(Into::into),
            )?,
            "--include-subaddresses" => push_filter(
                &next_value("--include-subaddresses", iter)?,
                &mut args.include_subaddresses,
                |v| parse_u8_value(v, "--include-subaddresses").map_err(Into::into),
            )?,
            s if s.starts_with("--include-subaddresses=") => push_filter(
                &s["--include-subaddresses=".len()..],
                &mut args.include_subaddresses,
                |v| parse_u8_value(v, "--include-subaddresses").map_err(Into::into),
            )?,
            "-h" | "--help" => return Err(ParseError::HelpRequested),
            s if s.starts_with('-') => {
                return Err(format!("unknown decode option: {s}").into());
            }
            // Positional input path(s). One or more is accepted; more than one
            // resolved input triggers the time-sorted merge (L2-MRG-001).
            _ => args.inputs.push(PathBuf::from(arg)),
        }
    }

    // Exactly one input *method* (positionals XOR --manifest XOR --glob).
    let methods = usize::from(!args.inputs.is_empty())
        + usize::from(args.manifest.is_some())
        + usize::from(args.glob.is_some());
    if methods == 0 {
        return Err(
            "decode requires an input file (positional, --manifest, or --glob)"
                .to_string()
                .into(),
        );
    }
    if methods > 1 {
        return Err(
            "decode accepts only one input method: positional paths, --manifest, or --glob — not a combination"
                .to_string()
                .into(),
        );
    }
    Ok(args)
}

fn parse_count(iter: &mut ArgIter<'_>) -> Result<PathBuf, ParseError> {
    let mut path: Option<PathBuf> = None;
    for arg in iter.by_ref() {
        match arg.as_str() {
            "-h" | "--help" => return Err(ParseError::HelpRequested),
            s if s.starts_with('-') => {
                return Err(format!("unknown count option: {s}").into());
            }
            _ => {
                if path.is_some() {
                    return Err(format!("unexpected positional argument: {arg}").into());
                }
                path = Some(PathBuf::from(arg));
            }
        }
    }
    path.ok_or_else(|| ParseError::Other("count requires an input file".to_string()))
}

fn parse_dump(iter: &mut ArgIter<'_>) -> Result<DumpArgs, ParseError> {
    let mut args = DumpArgs::default();
    let mut input_seen = false;

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--raw" => args.raw = true,
            "--offset" => {
                let v = next_value("--offset", iter)?;
                args.offset = parse_int_value(&v, "--offset")?;
            }
            s if s.starts_with("--offset=") => {
                args.offset = parse_int_value(&s["--offset=".len()..], "--offset")?;
            }
            "--length" => {
                let v = next_value("--length", iter)?;
                args.length = Some(parse_int_value(&v, "--length")?);
            }
            s if s.starts_with("--length=") => {
                args.length = Some(parse_int_value(&s["--length=".len()..], "--length")?);
            }
            "--records" => {
                let v = next_value("--records", iter)?;
                args.records = Some(parse_int_value(&v, "--records")? as u64);
            }
            s if s.starts_with("--records=") => {
                args.records = Some(parse_int_value(&s["--records=".len()..], "--records")? as u64);
            }
            "-h" | "--help" => return Err(ParseError::HelpRequested),
            s if s.starts_with('-') => {
                return Err(format!("unknown dump option: {s}").into());
            }
            _ => {
                if input_seen {
                    return Err(format!("unexpected positional argument: {arg}").into());
                }
                args.input = PathBuf::from(arg);
                input_seen = true;
            }
        }
    }

    if !input_seen {
        return Err("dump requires an input file".to_string().into());
    }
    Ok(args)
}

fn parse_time_format_arg(s: &str) -> Result<TimestampFormat, String> {
    match s.to_ascii_lowercase().as_str() {
        "auto" => Ok(TimestampFormat::Auto),
        "irig" => Ok(TimestampFormat::Irig),
        "standard" => Ok(TimestampFormat::Standard),
        other => Err(format!(
            "invalid --time-format: {other:?}; valid: auto, irig, standard"
        )),
    }
}

/// L2-DEC-015: validate the `--detect-records` argument against the
/// `[1, 32]` range pinned by `DETECT_RECORDS_MIN` / `DETECT_RECORDS_MAX`.
/// The same range is checked at config-load time for the TOML form;
/// duplicating the validation here surfaces malformed CLI input with a
/// clear error before the config layer is even consulted.
fn parse_detect_records(s: &str) -> Result<usize, String> {
    let n: usize = s
        .trim()
        .parse()
        .map_err(|_| format!("invalid --detect-records: {s:?}; must be an integer"))?;
    if !(crate::config::DETECT_RECORDS_MIN..=crate::config::DETECT_RECORDS_MAX).contains(&n) {
        return Err(format!(
            "invalid --detect-records: {n}; valid range: [{}, {}]",
            crate::config::DETECT_RECORDS_MIN,
            crate::config::DETECT_RECORDS_MAX
        ));
    }
    Ok(n)
}

/// L2-SYN-026: validate the `--lookahead-records` argument against
/// `[1, 32]`. Same shape as `parse_detect_records`.
fn parse_lookahead_records(s: &str) -> Result<usize, String> {
    let n: usize = s
        .trim()
        .parse()
        .map_err(|_| format!("invalid --lookahead-records: {s:?}; must be an integer"))?;
    if !(crate::config::LOOKAHEAD_RECORDS_MIN..=crate::config::LOOKAHEAD_RECORDS_MAX).contains(&n) {
        return Err(format!(
            "invalid --lookahead-records: {n}; valid range: [{}, {}]",
            crate::config::LOOKAHEAD_RECORDS_MIN,
            crate::config::LOOKAHEAD_RECORDS_MAX
        ));
    }
    Ok(n)
}

/// L2-DEC-017 / L2-CLI-012: validate the `--standard-tick-rate-hz`
/// argument. Mirrors the config-load validation in
/// `config::parse_into_config` so the CLI and TOML paths reject the same
/// inputs with the same shape of message: the rate must be a finite,
/// strictly-positive frequency.
fn parse_standard_tick_rate_hz(s: &str) -> Result<f64, String> {
    let hz: f64 = s
        .trim()
        .parse()
        .map_err(|_| format!("invalid --standard-tick-rate-hz: {s:?}; must be a number"))?;
    if !hz.is_finite() || hz <= 0.0 {
        return Err(format!(
            "invalid --standard-tick-rate-hz: {hz}; must be a finite value greater than 0"
        ));
    }
    Ok(hz)
}

fn parse_mux_delimiter(s: &str) -> Result<String, String> {
    if s.is_empty() {
        return Err("invalid --mux-delimiter: must be a non-empty string".to_string());
    }
    Ok(s.to_string())
}

fn parse_mux_field(s: &str) -> Result<i64, String> {
    s.trim()
        .parse::<i64>()
        .map_err(|_| format!("invalid --mux-field: {s:?}; must be an integer"))
}

fn parse_collapse_window_us(s: &str) -> Result<i64, String> {
    match s.trim().parse::<i64>() {
        Ok(n) if n >= 0 => Ok(n),
        _ => Err(format!(
            "invalid --collapse-window-us: {s:?}; must be a non-negative integer"
        )),
    }
}

// ── Subcommand runners ────────────────────────────────────────────────

/// Apply a log-level string. Returns Err on an unrecognized name so the
/// caller can surface the failure instead of silently no-op'ing.
///
/// `source` is included in the error for diagnosability (it'll be
/// `--log-level` for CLI-supplied values, `[logging].level` for config
/// file values). Validated names are DEBUG, INFO, WARNING, ERROR,
/// CRITICAL (CRITICAL maps to OFF).
fn apply_log_level(source: &str, value: &str) -> Result<(), String> {
    match Level::parse(value) {
        Some(lvl) => {
            log::set_level(lvl);
            Ok(())
        }
        None => Err(format!(
            "invalid {source}: {value:?}; valid: DEBUG, INFO, WARNING, WARN, ERROR, CRITICAL, OFF"
        )),
    }
}

/// Load `--config` (or the built-in defaults if none was specified) and
/// apply log-level precedence: config overrides the run() default; CLI
/// overrides config. Used by every subcommand so a malformed config
/// file is rejected uniformly regardless of whether you ran `decode`,
/// `count`, or `dump`.
fn resolve_config(globals: &GlobalArgs) -> Result<DecoderConfig, CliError> {
    let cfg = load_config(globals.config.as_deref()).map_err(|e| CliError::config(e.0))?;

    // The config file's log_level is validated at load time (see
    // config::parse_into_config), so apply_log_level cannot fail here
    // unless someone constructed a DecoderConfig manually with a bogus
    // string — treat that as a configuration error.
    apply_log_level("[logging].level (in config)", &cfg.log_level).map_err(CliError::config)?;

    // An invalid `--log-level` is a CLI usage error. In practice run()
    // validates it earlier (and exits via die()), so this is defensive.
    if let Some(s) = &globals.log_level {
        apply_log_level("--log-level", s).map_err(CliError::usage)?;
    }

    Ok(cfg)
}

/// Open the input file and configure the reader from `cfg`. The
/// String-flavored error type is what every subcommand runner returns,
/// so the conversion is folded in here.
fn open_reader(path: &Path, cfg: &DecoderConfig) -> Result<MieFileReader, CliError> {
    MieFileReader::with_options(
        path,
        ReaderOptions {
            strict: cfg.strict,
            time_format: cfg.time_format,
            detect_records: cfg.detect_records,
            lookahead_records: cfg.lookahead_records,
            standard_tick_rate_hz: cfg.standard_tick_rate_hz,
            mux_enabled: cfg.mux_enabled,
            mux_delimiter: cfg.mux_delimiter.clone(),
            mux_field: cfg.mux_field,
        },
    )
    .map_err(|e| CliError::runtime(format_mie_error(e)))
}

/// Assemble the CLI `ConfigOverrides` from parsed decode args (CLI > config >
/// default). The filter vectors are moved out of `args` (leaving them empty),
/// so `args.output` stays usable afterward.
fn build_config_overrides(args: &mut DecodeArgs, log_level: Option<String>) -> ConfigOverrides {
    ConfigOverrides {
        time_format: args.time_format,
        strict: args.strict,
        error_mode: if args.inline_errors {
            Some(ErrorMode::Inline)
        } else {
            None
        },
        output_format: args.output_format.clone(),
        // A CLI flag flips the option on; absence leaves the config value intact
        // (Some(false) would clobber a `true` from the config).
        no_clobber: if args.no_clobber { Some(true) } else { None },
        allow_partial: if args.allow_partial { Some(true) } else { None },
        detect_records: args.detect_records,
        lookahead_records: args.lookahead_records,
        standard_tick_rate_hz: args.standard_tick_rate_hz,
        // --no-mux disables MUX population; absence leaves the config value.
        mux_enabled: if args.no_mux { Some(false) } else { None },
        mux_delimiter: args.mux_delimiter.clone(),
        mux_field: args.mux_field,
        collapse_duplicates: if args.collapse_duplicates {
            Some(true)
        } else {
            None
        },
        collapse_window_us: args.collapse_window_us,
        exclude_types: std::mem::take(&mut args.exclude_types),
        exclude_rts: std::mem::take(&mut args.exclude_rts),
        exclude_buses: std::mem::take(&mut args.exclude_buses),
        exclude_subaddresses: std::mem::take(&mut args.exclude_subaddresses),
        include_types: std::mem::take(&mut args.include_types),
        include_rts: std::mem::take(&mut args.include_rts),
        include_buses: std::mem::take(&mut args.include_buses),
        include_subaddresses: std::mem::take(&mut args.include_subaddresses),
        log_level,
    }
}

/// Open a reader per resolved input (L2-MRG-001). Under `--allow-partial` a
/// *merge* tolerates a per-file open failure — it drops that input with a WARN
/// and reports `open_dropped = true` so the batch commits a `.partial`
/// (L2-MRG-004). A single-input decode propagates the open error. Returns the
/// opened readers and whether any input was dropped.
fn open_all_readers(
    input_paths: &[PathBuf],
    cfg: &DecoderConfig,
    merge_requested: bool,
) -> Result<(Vec<MieFileReader>, bool), CliError> {
    let mut readers: Vec<MieFileReader> = Vec::with_capacity(input_paths.len());
    let mut open_dropped = false;
    for p in input_paths {
        match open_reader(p, cfg) {
            Ok(r) => readers.push(r),
            Err(e) if merge_requested && cfg.allow_partial => {
                log_warn!(
                    "merge: input {} could not be opened; truncating it from the \
                     merge (--allow-partial): {}",
                    p.display(),
                    e.message
                );
                open_dropped = true;
            }
            Err(e) => return Err(e),
        }
    }
    Ok((readers, open_dropped))
}

/// Run the decode (single input) or k-way merge (two or more) and return the
/// writer result. `Err(code)` is an early exit: an incompatible-merge or
/// prime-time failure (L2-MRG-003) whose exit code is chosen here.
fn execute_decode_or_merge(
    readers: &[MieFileReader],
    cfg: &DecoderConfig,
    output: Option<&Path>,
    write_opts: WriteOptions,
    merge_requested: bool,
    open_dropped: bool,
) -> Result<crate::error::MieResult<crate::writer::WriteOutcome>, ExitCode> {
    if !merge_requested {
        let messages = readers[0].iter().filter_messages(cfg.filters.clone());
        return Ok(write_messages(messages, output, cfg.error_mode, write_opts));
    }

    match crate::merge::MergedRecordIter::new(
        readers,
        cfg.standard_tick_rate_hz,
        cfg.allow_partial,
        cfg.strict,
    ) {
        Ok(merged) => {
            let merged = merged.collapse(cfg.collapse_duplicates, cfg.collapse_window_us);
            // Clone the suppressed-duplicate counter before the writer consumes
            // the iterator, then report it after (L2-MRG-007).
            let collapsed = merged.collapsed_handle();
            // An input dropped at open time surfaces a terminal after the good
            // rows so the writer commits a `.partial` (L2-MRG-004).
            let open_tail =
                open_dropped.then_some(Err(crate::error::MieError::UnrecoverableSyncLoss {
                    offset: 0,
                    sync_losses: 0,
                }));
            let result = write_messages(
                merged.filter_messages(cfg.filters.clone()).chain(open_tail),
                output,
                cfg.error_mode,
                write_opts,
            );
            let n = collapsed.load(std::sync::atomic::Ordering::Relaxed);
            if n > 0 {
                log_info!("merge: collapsed {n} duplicate message(s) across recorders");
            }
            Ok(result)
        }
        // Incompatible inputs (L2-MRG-003) and prime-time file failures surface
        // here, before any output — route through the same exit classifier.
        Err(e) => Err(classify_decode_exit(Err(e), 0, false)),
    }
}

fn run_decode(globals: GlobalArgs, mut args: DecodeArgs) -> Result<ExitCode, CliError> {
    let cfg = resolve_config(&globals)?;

    // Resolve the input set before `build_config_overrides` moves the filter
    // fields out of `args` (so we can still read inputs/manifest/glob).
    let input_paths = resolve_inputs(&args)?;

    let overrides = build_config_overrides(&mut args, globals.log_level.clone());
    let cfg = cfg.with_overrides(overrides);

    if cfg.output_format != "csv" {
        return Err(CliError::runtime(format!(
            "output format {:?} not yet supported (only 'csv')",
            cfg.output_format
        )));
    }

    // Open a reader per resolved input file (L2-MRG-001). `input_paths` was
    // resolved above, before `with_overrides` consumed the filter fields.
    // Under --allow-partial a *merge* tolerates a per-file OPEN failure (an
    // empty / unreadable / missing input): it drops that input with a WARN and
    // commits the batch as `.partial` (L2-MRG-004), mirroring how a priming-time
    // or mid-file failure is handled. A single-input decode is unaffected.
    let merge_requested = input_paths.len() > 1;
    let (readers, open_dropped) = open_all_readers(&input_paths, &cfg, merge_requested)?;
    for r in &readers {
        log_info!("opened {} ({} bytes)", r.path().display(), r.file_size());
    }

    // WriteOptions: file-output safety checks (collision per L2-WRT-014,
    // no-clobber per L2-WRT-017) and L1-EXIT-004 allow_partial. For a single
    // input the writer performs its own input/output collision check; for a
    // merge we check the output against *every* input here and disable the
    // writer's single-path check.
    if merge_requested && let Some(out) = args.output.as_deref() {
        check_output_collision(out, &input_paths)?;
    }
    let write_opts = WriteOptions {
        input_path: if merge_requested {
            None
        } else {
            input_paths.first().cloned()
        },
        no_clobber: cfg.no_clobber,
        allow_partial: cfg.allow_partial,
    };

    // One input → the single-file path (per-file DELTA); two or more → the
    // time-sorted k-way merge with global DELTA. Both feed the same writer. An
    // incompatible-merge / prime-time failure returns its exit code directly.
    let write_result = match execute_decode_or_merge(
        &readers,
        &cfg,
        args.output.as_deref(),
        write_opts,
        merge_requested,
        open_dropped,
    ) {
        Ok(r) => r,
        Err(code) => return Ok(code),
    };

    // Cumulative sync-loss count across all inputs drives the L1-EXIT-005
    // exit-class summary. Safe to query after the iterator(s) are consumed.
    let sync_losses: u64 = readers.iter().map(|r| r.sync_losses()).sum();

    // L1-EXIT-010: report the empty-recording class only when *every* opened
    // input was a valid empty recording (so a merge that also drew rows from a
    // non-empty input stays `complete`). A single-file empty decode is the
    // common case; the writer has already produced a header-only CSV.
    let empty_recording = !readers.is_empty() && readers.iter().all(|r| r.empty_recording());

    Ok(classify_decode_exit(
        write_result,
        sync_losses,
        empty_recording,
    ))
}

/// Resolve the `decode` input set from exactly one method (positionals,
/// `--manifest`, or `--glob`; mutual exclusivity already enforced at parse
/// time), enforcing the `MAX_MERGE_FILES` cap (L2-MRG-001).
fn resolve_inputs(args: &DecodeArgs) -> Result<Vec<PathBuf>, CliError> {
    let paths = if let Some(manifest) = &args.manifest {
        crate::merge::read_manifest(manifest).map_err(|e| {
            CliError::runtime(format!(
                "failed to read manifest {}: {e}",
                manifest.display()
            ))
        })?
    } else if let Some(pattern) = &args.glob {
        crate::merge::expand_glob(pattern)
            .map_err(|e| CliError::runtime(format!("failed to expand --glob {pattern:?}: {e}")))?
    } else {
        args.inputs.clone()
    };

    if paths.is_empty() {
        return Err(CliError::usage(match (&args.manifest, &args.glob) {
            (Some(m), _) => format!("manifest {} contains no input paths", m.display()),
            (_, Some(g)) => format!("--glob {g:?} matched no files"),
            _ => "decode requires at least one input file".to_string(),
        }));
    }
    if paths.len() > crate::merge::MAX_MERGE_FILES {
        return Err(CliError::usage(format!(
            "too many input files: {} (maximum is {}); split the set into smaller batches",
            paths.len(),
            crate::merge::MAX_MERGE_FILES
        )));
    }
    Ok(paths)
}

/// Reject a merge whose output path resolves to one of its inputs
/// (L2-WRT-014 extended across the input set). Best-effort canonicalization;
/// falls back to a literal path comparison when a path cannot be canonicalized
/// (e.g. the output does not exist yet).
fn check_output_collision(output: &Path, inputs: &[PathBuf]) -> Result<(), CliError> {
    let out_canon = std::fs::canonicalize(output).ok();
    for inp in inputs {
        let collides = match (&out_canon, std::fs::canonicalize(inp).ok()) {
            (Some(o), Some(i)) => *o == i,
            _ => output == inp.as_path(),
        };
        if collides {
            return Err(CliError::runtime(format!(
                "output path {} resolves to merge input {}; choose a different output path",
                output.display(),
                inp.display()
            )));
        }
    }
    Ok(())
}

/// Route a message stream to the CSV writer per the configured error mode.
/// Generic over the iterator so both the single-file reader and the merge
/// iterator are monomorphized (no dynamic dispatch on the hot path).
fn write_messages<I>(
    messages: I,
    output: Option<&Path>,
    error_mode: ErrorMode,
    write_opts: WriteOptions,
) -> crate::error::MieResult<crate::writer::WriteOutcome>
where
    I: Iterator<Item = crate::error::MieResult<crate::models::MieMessage>>,
{
    // Separate mode requires a file path; stdout in that case forces inline
    // behavior with a WARN (you cannot split stdout) per L3-RS-009.
    if error_mode == ErrorMode::Separate {
        match output {
            None => {
                crate::log_warn!("stdout output forces inline error mode");
                write_csv(messages, None, WriteOptions::default())
            }
            Some(out) => write_csv_split(messages, out, write_opts).inspect(|outcome| {
                log_info!(
                    "wrote {} messages + {} errors to {}",
                    outcome.normal_count,
                    outcome.error_count,
                    out.display()
                );
            }),
        }
    } else {
        write_csv(messages, output, write_opts)
    }
}

/// Map a writer-side result + the reader's sync-loss count to an
/// `ExitCode` per L1-EXIT-002 through L1-EXIT-005 and L2-CLI-011. Emits the
/// one-line exit-class summary required by L1-EXIT-005 in every branch.
fn classify_decode_exit(
    r: crate::error::MieResult<crate::writer::WriteOutcome>,
    sync_losses: u64,
    empty_recording: bool,
) -> ExitCode {
    match r {
        Ok(outcome) => {
            // L1-EXIT-010: a valid but empty recording (opened on the
            // end-of-records terminator) is a successful decode that produces a
            // header-only CSV. Distinguish it from an ordinary complete decode
            // so operators can tell "captured nothing" apart from "captured and
            // wrote rows" by the exit-class line alone.
            let empty = empty_recording
                && outcome.partial.is_none()
                && outcome.normal_count == 0
                && outcome.error_count == 0;
            let class = if outcome.partial.is_some() {
                "partial-unrecoverable"
            } else if empty {
                "empty-recording"
            } else if sync_losses > 0 {
                "partial-recovered"
            } else {
                "complete"
            };
            log_info!("decode exit class: {class} (sync_losses={sync_losses})");
            ExitCode::SUCCESS
        }
        Err(e) if e.is_broken_pipe() => {
            log_info!("decode exit class: complete (broken-pipe on stdout)");
            ExitCode::SUCCESS
        }
        Err(e @ MieError::NoValidRecords { .. }) => {
            log_error!("{e}");
            eprintln!("Error: {e}");
            log_info!("decode exit class: no-records");
            ExitCode::from(exit_code::NO_RECORDS)
        }
        Err(e @ MieError::HomogeneousPayload { .. }) => {
            // L2-SYN-018 + L1-EXIT-002: semantically a "wrong file
            // type" rejection (the input is a single-byte pad rather
            // than an MIE recording), same exit-code class as
            // NoValidRecords.
            log_error!("{e}");
            eprintln!("Error: {e}");
            log_info!("decode exit class: no-records");
            ExitCode::from(exit_code::NO_RECORDS)
        }
        Err(e @ MieError::TimestampFormatMismatch { .. }) => {
            // L2-DEC-016 + L1-EXIT-002: ambiguous timestamp format is
            // semantically another "wrong file type" rejection — the
            // probe could not confidently distinguish IRIG from
            // Standard, so we treat the file the same way we'd treat
            // an unrecognized stream. Same exit class (2) as
            // NoValidRecords / HomogeneousPayload.
            log_error!("{e}");
            eprintln!("Error: {e}");
            log_info!("decode exit class: no-records (timestamp-format-mismatch)");
            ExitCode::from(exit_code::NO_RECORDS)
        }
        Err(e @ MieError::UnrecoverableSyncLoss { .. }) => {
            log_error!("{e}");
            eprintln!("Error: {e}");
            log_info!(
                "decode exit class: partial-unrecoverable (sync_losses={sync_losses}); \
                 pass --allow-partial to preserve the rows decoded so far"
            );
            ExitCode::from(exit_code::SYNC_LOSS)
        }
        Err(e @ MieError::IncompatibleMergeInputs { .. }) => {
            // L1-EXIT-009 / L2-MRG-003: inputs cannot be ordered on a common
            // absolute timeline. Rejected before any output was written.
            log_error!("{e}");
            eprintln!("Error: {e}");
            log_info!("decode exit class: merge-incompatible");
            ExitCode::from(exit_code::MERGE_INCOMPATIBLE)
        }
        Err(e @ MieError::NonMonotonicInput { .. }) => {
            // L2-MRG-006: a strict-mode merge hit an input whose records are
            // not in chronological capture order. Same record-error class as
            // other strict-mode record failures (exit 1).
            log_error!("{e}");
            eprintln!("Error: {e}");
            log_info!("decode exit class: non-monotonic-input (strict)");
            ExitCode::from(exit_code::RUNTIME)
        }
        Err(e) => {
            log_error!("{e}");
            eprintln!("Error: {e}");
            ExitCode::from(exit_code::RUNTIME)
        }
    }
}

fn run_count(globals: GlobalArgs, input: PathBuf) -> Result<(), CliError> {
    let cfg = resolve_config(&globals)?;
    let reader = open_reader(&input, &cfg)?;

    // Apply config's filters to the count, matching decode's behavior.
    // A user who wants raw counts can omit [filter] from their config.
    let filter_cfg = cfg.filters.clone();
    let mut count: u64 = 0;
    for item in reader.iter().filter_messages(filter_cfg) {
        match item {
            Ok(_) => count += 1,
            // Align with `decode`: a wrong-file rejection maps to exit 2, not 1
            // (L2-CLI-011). An empty recording surfaces no error at all — the
            // iterator simply yields zero records — so `count` prints 0 and
            // exits 0 (L1-EXIT-010).
            Err(e) => return Err(CliError::from_decode_error(e)),
        }
    }
    // L3-RS-008: integer count to stdout (the machine-readable data),
    // human-friendly status with path context to stderr (always emitted,
    // not gated by --log-level so an interactive operator sees it
    // without having to opt into INFO logging).
    println!("{count}");
    if reader.empty_recording() {
        eprintln!(
            "no records in {} (empty recording — opens on the end-of-records terminator)",
            reader.path().display()
        );
    } else {
        eprintln!("counted {count} messages in {}", reader.path().display());
    }
    Ok(())
}

fn run_dump(globals: GlobalArgs, args: DumpArgs) -> Result<(), CliError> {
    // dump only consumes log_level from config; time_format / strict /
    // filters don't apply to a hex view. We still call resolve_config
    // so a malformed config errors out consistently with the other
    // subcommands.
    let _cfg = resolve_config(&globals)?;

    let result = if args.raw {
        hex_dump_raw_to_stdout(&args.input, args.offset, args.length)
    } else {
        hex_dump_records_to_stdout(&args.input, args.records, args.offset)
    };
    finish_dump(result)
}

/// Map a dump result to the CLI contract. A broken pipe on stdout (e.g.
/// `dump | head`) is a clean termination and exits `0` per L2-WRT-018; any
/// other writer error (disk full, permission denied) — now that the dump
/// propagates them instead of swallowing them — surfaces as a runtime error.
fn finish_dump(result: crate::error::MieResult<()>) -> Result<(), CliError> {
    match result {
        Ok(()) => Ok(()),
        Err(e) if e.is_broken_pipe() => {
            log_info!("dump: broken-pipe on stdout, exiting 0");
            Ok(())
        }
        Err(e) => Err(CliError::runtime(format_mie_error(e))),
    }
}

fn format_mie_error(e: MieError) -> String {
    e.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> ArgIter<'static> {
        let v: Vec<String> = values.iter().map(|s| s.to_string()).collect();
        // skip(0) is required to match the ArgIter alias's
        // Skip<IntoIter<...>> shape; clippy can't see past the alias.
        #[allow(clippy::iter_skip_zero)]
        v.into_iter().skip(0).peekable()
    }

    /// `--help` must propagate as `ParseError::HelpRequested`, never as
    /// `process::exit`. The whole point of the refactor.
    /// Requirements: L2-CLI-001
    #[test]
    fn parse_decode_help_returns_help_requested() {
        let mut it = args(&["--help"]);
        match parse_decode(&mut it) {
            Err(ParseError::HelpRequested) => {}
            other => panic!("expected HelpRequested, got {other:?}"),
        }
    }

    /// Requirements: L2-CLI-008, L3-RS-008
    #[test]
    fn parse_count_help_returns_help_requested() {
        let mut it = args(&["-h"]);
        match parse_count(&mut it) {
            Err(ParseError::HelpRequested) => {}
            other => panic!("expected HelpRequested, got {other:?}"),
        }
    }

    /// Requirements: L2-CLI-009
    #[test]
    fn parse_dump_help_returns_help_requested() {
        let mut it = args(&["--help"]);
        match parse_dump(&mut it) {
            Err(ParseError::HelpRequested) => {}
            other => panic!("expected HelpRequested, got {other:?}"),
        }
    }

    /// Parse errors should still surface as ParseError::Other, not panics
    /// or exits.
    /// Requirements: L2-CLI-005
    #[test]
    fn parse_decode_unknown_flag_returns_other() {
        let mut it = args(&["--nope"]);
        match parse_decode(&mut it) {
            Err(ParseError::Other(msg)) => assert!(msg.contains("--nope")),
            other => panic!("expected Other, got {other:?}"),
        }
    }

    /// Requirements: L2-CLI-001
    #[test]
    fn parse_decode_missing_input_returns_other() {
        let mut it = args(&[]);
        match parse_decode(&mut it) {
            Err(ParseError::Other(msg)) => assert!(msg.contains("input file")),
            other => panic!("expected Other, got {other:?}"),
        }
    }

    /// Happy path still produces a value.
    /// Requirements: L2-CLI-002
    #[test]
    fn parse_decode_minimal_ok() {
        let mut it = args(&["recording.mie"]);
        let parsed = parse_decode(&mut it).unwrap();
        assert_eq!(parsed.inputs, vec![PathBuf::from("recording.mie")]);
    }

    /// Regression test for the team's exact reproducer:
    /// `decode --include-rts 15 file.mie` previously consumed `file.mie`
    /// as another RT value (greedy multi-value). Now filter flags take
    /// exactly one value, so the positional input binds correctly.
    /// Requirements: L2-CLI-010
    #[test]
    fn filter_flag_does_not_eat_positional_input() {
        let mut it = args(&["--include-rts", "15", "file.mie"]);
        let parsed = parse_decode(&mut it).unwrap();
        assert_eq!(parsed.inputs, vec![PathBuf::from("file.mie")]);
        assert_eq!(parsed.include_rts, vec![15]);
    }

    /// Comma-separated values within a single flag.
    /// Requirements: L2-CLI-010
    #[test]
    fn filter_flag_accepts_comma_separated_values() {
        let mut it = args(&["--include-rts", "15,20,31", "file.mie"]);
        let parsed = parse_decode(&mut it).unwrap();
        assert_eq!(parsed.inputs, vec![PathBuf::from("file.mie")]);
        assert_eq!(parsed.include_rts, vec![15, 20, 31]);
    }

    /// Repeating a filter flag accumulates values.
    /// Requirements: L2-CLI-010
    #[test]
    fn filter_flag_repeats_accumulate() {
        let mut it = args(&["--include-rts", "15", "--include-rts", "31", "file.mie"]);
        let parsed = parse_decode(&mut it).unwrap();
        assert_eq!(parsed.inputs, vec![PathBuf::from("file.mie")]);
        assert_eq!(parsed.include_rts, vec![15, 31]);
    }

    /// `--flag=value` syntax with comma-separation.
    /// Requirements: L2-CLI-010
    #[test]
    fn filter_flag_accepts_eq_form() {
        let mut it = args(&["--include-rts=15,20", "file.mie"]);
        let parsed = parse_decode(&mut it).unwrap();
        assert_eq!(parsed.inputs, vec![PathBuf::from("file.mie")]);
        assert_eq!(parsed.include_rts, vec![15, 20]);
    }

    /// Sanity-check the same property for the other filter flags.
    /// Requirements: L2-CLI-010
    #[test]
    fn all_filter_flags_take_single_value() {
        let mut it = args(&[
            "--exclude-types",
            "SPURIOUS_DATA",
            "--include-buses",
            "A",
            "--exclude-subaddresses",
            "0,31",
            "rec.mie",
        ]);
        let parsed = parse_decode(&mut it).unwrap();
        assert_eq!(parsed.inputs, vec![PathBuf::from("rec.mie")]);
        assert_eq!(parsed.exclude_types, vec![0x20]);
        assert_eq!(parsed.include_buses, vec![crate::models::Bus::A]);
        assert_eq!(parsed.exclude_subaddresses, vec![0, 31]);
    }

    /// A single-value filter flag consumes exactly one value; any further
    /// tokens are positional inputs. With multi-file input (L2-MRG-001),
    /// `--include-rts 15 31 file.mie` parses as include-rts=[15] and inputs
    /// ["31", "file.mie"] — the stray "31" becomes a path (failing later at
    /// open time) rather than being silently absorbed as a second RT value.
    /// Requirements: L2-CLI-010, L2-MRG-001
    #[test]
    fn filter_flag_takes_single_value_rest_are_positional_inputs() {
        let mut it = args(&["--include-rts", "15", "31", "file.mie"]);
        let parsed = parse_decode(&mut it).unwrap();
        assert_eq!(parsed.include_rts, vec![15]);
        assert_eq!(
            parsed.inputs,
            vec![PathBuf::from("31"), PathBuf::from("file.mie")]
        );
    }

    /// Requirements: L2-CLI-012
    #[test]
    fn parse_decode_standard_tick_rate_hz_space_and_eq_forms() {
        let mut it = args(&["--standard-tick-rate-hz", "1000000", "rec.mie"]);
        let parsed = parse_decode(&mut it).unwrap();
        assert_eq!(parsed.inputs, vec![PathBuf::from("rec.mie")]);
        assert_eq!(parsed.standard_tick_rate_hz, Some(1_000_000.0));

        let mut it = args(&["--standard-tick-rate-hz=2.5e6", "rec.mie"]);
        let parsed = parse_decode(&mut it).unwrap();
        assert_eq!(parsed.standard_tick_rate_hz, Some(2_500_000.0));
    }

    /// Requirements: L2-CLI-012
    #[test]
    fn parse_decode_standard_tick_rate_hz_rejects_nonpositive() {
        for bad in ["0", "-1", "0.0"] {
            let mut it = args(&["--standard-tick-rate-hz", bad, "rec.mie"]);
            match parse_decode(&mut it) {
                Err(ParseError::Other(msg)) => {
                    assert!(
                        msg.contains("--standard-tick-rate-hz"),
                        "error should name the flag for {bad:?}: {msg}"
                    );
                }
                other => panic!("expected Other for {bad:?}, got {other:?}"),
            }
        }
    }

    /// Requirements: L2-CLI-012
    #[test]
    fn parse_decode_standard_tick_rate_hz_rejects_non_numeric() {
        let mut it = args(&["--standard-tick-rate-hz", "fast", "rec.mie"]);
        match parse_decode(&mut it) {
            Err(ParseError::Other(msg)) => assert!(msg.contains("--standard-tick-rate-hz")),
            other => panic!("expected Other, got {other:?}"),
        }
    }

    // ── --config plumbing through count/dump ──────────────────────────
    //
    // Regression: before, count and dump ignored --config entirely so a
    // malformed config file passed alongside those subcommands silently
    // succeeded. After the fix, all three subcommands load the config
    // up-front and surface parse errors uniformly.

    fn write_temp_file(suffix: &str, content: &[u8]) -> PathBuf {
        use std::io::Write;
        use std::sync::atomic::{AtomicU64, Ordering};
        static C: AtomicU64 = AtomicU64::new(0);
        let n = C.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let p = std::env::temp_dir().join(format!("mie-cli-test-{pid}-{n}{suffix}"));
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(content).unwrap();
        f.flush().unwrap();
        p
    }

    /// Requirements: L2-CLI-005, L2-CLI-011, L1-EXIT-008
    #[test]
    fn run_count_propagates_config_load_error() {
        let bad = write_temp_file(".toml", b"[decode]\ntime_format = \"potato\"\n");
        let globals = GlobalArgs {
            log_level: None,
            config: Some(bad.clone()),
        };
        // Input doesn't matter: config error fires before the file is opened.
        let result = run_count(globals, PathBuf::from("/no/such/recording.mie"));
        let _ = std::fs::remove_file(&bad);
        match result {
            Err(e) => {
                assert_eq!(e.code, exit_code::CONFIG, "config error should exit 5");
                assert!(
                    e.message.contains("Invalid time_format"),
                    "expected config error, got: {}",
                    e.message
                );
            }
            Ok(()) => panic!("expected config error, got Ok"),
        }
    }

    /// Requirements: L2-CLI-005, L2-CLI-011, L1-EXIT-008
    #[test]
    fn run_dump_propagates_config_load_error() {
        let bad = write_temp_file(".toml", b"[decode]\ntime_format = \"potato\"\n");
        let globals = GlobalArgs {
            log_level: None,
            config: Some(bad.clone()),
        };
        let dump_args = DumpArgs {
            input: PathBuf::from("/no/such/recording.mie"),
            ..Default::default()
        };
        let result = run_dump(globals, dump_args);
        let _ = std::fs::remove_file(&bad);
        match result {
            Err(e) => {
                assert_eq!(e.code, exit_code::CONFIG, "config error should exit 5");
                assert!(
                    e.message.contains("Invalid time_format"),
                    "expected config error, got: {}",
                    e.message
                );
            }
            Ok(()) => panic!("expected config error, got Ok"),
        }
    }

    /// Requirements: L2-WRT-018
    #[test]
    fn finish_dump_maps_broken_pipe_to_ok() {
        let broken = MieError::WriterError {
            destination: "stdout".to_string(),
            source: std::io::Error::new(std::io::ErrorKind::BrokenPipe, "pipe closed"),
        };
        assert!(
            finish_dump(Err(broken)).is_ok(),
            "broken pipe on dump stdout should exit 0"
        );
    }

    /// Requirements: L2-WRT-018
    #[test]
    fn finish_dump_propagates_real_write_error() {
        let disk_full = MieError::WriterError {
            destination: "stdout".to_string(),
            source: std::io::Error::other("No space left on device"),
        };
        let err = finish_dump(Err(disk_full)).unwrap_err();
        assert_eq!(err.code, exit_code::RUNTIME, "disk-full dump error exits 1");
    }

    /// Requirements: L2-CFG-005, L2-CLI-011, L1-EXIT-008
    #[test]
    fn run_count_propagates_missing_config_file() {
        let globals = GlobalArgs {
            log_level: None,
            config: Some(PathBuf::from("/no/such/config.toml")),
        };
        let result = run_count(globals, PathBuf::from("/no/such/recording.mie"));
        match result {
            Err(e) => {
                assert_eq!(
                    e.code,
                    exit_code::CONFIG,
                    "missing config file should exit 5"
                );
                assert!(
                    e.message.contains("Config file not found"),
                    "expected 'Config file not found' error, got: {}",
                    e.message
                );
            }
            Ok(()) => panic!("expected error, got Ok"),
        }
    }

    // ── Log-level validation ─────────────────────────────────────────
    //
    // Regression: --log-level NOPE used to be silently ignored (the
    // code did `if let Some(lvl) = Level::parse(s)` and never bothered
    // with the None branch). Now invalid values fail loudly:
    //   - CLI input fails at run() entry with exit 4 (usage error)
    //   - Config-file value fails at config load time with exit 5
    //     (configuration error)

    /// Requirements: L2-CLI-004
    #[test]
    fn apply_log_level_accepts_known_names() {
        for name in [
            "DEBUG", "INFO", "WARNING", "WARN", "ERROR", "CRITICAL", "off",
        ] {
            apply_log_level("--log-level", name)
                .unwrap_or_else(|e| panic!("expected {name} to parse, got: {e}"));
        }
    }

    /// Requirements: L2-CLI-004
    #[test]
    fn apply_log_level_rejects_unknown_names() {
        match apply_log_level("--log-level", "NOPE") {
            Err(msg) => {
                assert!(msg.contains("--log-level"));
                assert!(msg.contains("NOPE"));
                assert!(msg.contains("valid:"));
            }
            Ok(()) => panic!("expected error, got Ok"),
        }
    }

    /// Requirements: L2-CLI-004
    #[test]
    fn apply_log_level_includes_source_in_error() {
        let err = apply_log_level("[logging].level (in config)", "WHATEVER")
            .err()
            .unwrap();
        assert!(err.contains("[logging].level"));
        assert!(err.contains("WHATEVER"));
    }

    /// Requirements: L2-CFG-010, L2-CLI-011, L1-EXIT-008
    #[test]
    fn run_count_with_invalid_config_log_level_fails() {
        let bad = write_temp_file(".toml", b"[logging]\nlevel = \"NOPE\"\n");
        let globals = GlobalArgs {
            log_level: None,
            config: Some(bad.clone()),
        };
        let result = run_count(globals, PathBuf::from("/no/such/recording.mie"));
        let _ = std::fs::remove_file(&bad);
        match result {
            Err(e) => {
                assert_eq!(
                    e.code,
                    exit_code::CONFIG,
                    "config-level error should exit 5"
                );
                assert!(
                    e.message.contains("Invalid logging.level"),
                    "expected config-level error, got: {}",
                    e.message
                );
            }
            Ok(()) => panic!("expected error, got Ok"),
        }
    }

    /// Requirements: L2-CLI-004, L2-CLI-011, L1-EXIT-007
    #[test]
    fn run_count_with_invalid_cli_log_level_fails_via_resolve_config() {
        // The run() entry-point catches bad CLI levels first (exit 4),
        // but resolve_config — which is what the runners use — also
        // re-validates the CLI value. Test that path directly; a bad
        // CLI value is a usage error (exit 4).
        let globals = GlobalArgs {
            log_level: Some("NOPE".to_string()),
            config: None,
        };
        let result = run_count(globals, PathBuf::from("/no/such/recording.mie"));
        match result {
            Err(e) => {
                assert_eq!(e.code, exit_code::USAGE, "bad --log-level should exit 4");
                assert!(
                    e.message.contains("invalid --log-level"),
                    "expected CLI-level error, got: {}",
                    e.message
                );
            }
            Ok(()) => panic!("expected error, got Ok"),
        }
    }

    /// Combining input methods (positionals + --manifest / --glob) is a usage
    /// error at parse time.
    /// Requirements: L2-MRG-001
    #[test]
    fn decode_rejects_combined_input_methods() {
        for argv in [
            vec!["a.mie", "--manifest", "list.txt"],
            vec!["a.mie", "--glob", "*.mie"],
            vec!["--manifest", "list.txt", "--glob", "*.mie"],
        ] {
            let mut it = args(&argv);
            match parse_decode(&mut it) {
                Err(ParseError::Other(msg)) => {
                    assert!(msg.contains("only one input method"), "got: {msg}");
                }
                other => panic!("expected combined-method usage error, got {other:?}"),
            }
        }
    }

    /// More than MAX_MERGE_FILES resolved inputs is a usage error (exit 4),
    /// rejected before any file is opened.
    /// Requirements: L2-MRG-001
    #[test]
    fn resolve_inputs_rejects_over_cap() {
        let a = DecodeArgs {
            inputs: (0..=crate::merge::MAX_MERGE_FILES)
                .map(|i| PathBuf::from(format!("f{i}.mie")))
                .collect(),
            ..Default::default()
        };
        match resolve_inputs(&a) {
            Err(e) => {
                assert_eq!(e.code, exit_code::USAGE);
                assert!(
                    e.message.contains("too many input files"),
                    "got: {}",
                    e.message
                );
            }
            Ok(_) => panic!("expected over-cap usage error"),
        }
    }
}
