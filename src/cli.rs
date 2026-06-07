//! Hand-rolled argument parser and CLI dispatch.
//!
//! Surface (v2 redesign):
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
use crate::{log_error, log_info};

const VERSION: &str = env!("CARGO_PKG_VERSION");

const HELP: &str = "\
mie-decoder — DDC MIL-STD-1553 MIE binary decoder

USAGE:
  mie-decoder [--log-level L] [--config PATH] <command> [options]

COMMANDS:
  decode <INPUT>   Decode an MIE file to CSV
  count  <INPUT>   Print message count (no CSV)
  dump   <INPUT>   Hex dump (raw or record-aware)

GLOBAL OPTIONS:
  --log-level DEBUG|INFO|WARNING|ERROR  (default WARNING)
  --config PATH                         TOML configuration file
  -V, --version                         Print version and exit
  -h, --help                            Print this help and exit

DECODE OPTIONS:
  -o, --output PATH                     Output CSV (default stdout)
  --inline-errors                       Errors inline in main CSV
                                        (default: separate <stem>_errors.csv)
  --no-clobber                          Refuse to overwrite an existing
                                        output file (L2-WRT-017)
  --allow-partial                       On unrecoverable mid-file sync
                                        loss, write a <output>.partial
                                        file and exit 0 instead of 3
                                        (L1-EXIT-004)
  --time-format auto|irig|standard      Default auto
  --strict                              Raise on invalid records
  --format csv                          Output format (csv only at present)
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
  are equivalent. The `--flag=value` form also works.

DUMP OPTIONS:
  --raw                                 Raw hex dump (no record parsing)
  --offset N                            Start offset (decimal or 0xHEX)
  --length N                            Bytes to dump (raw mode)
  --records N                           Max records to dump (record mode)

EXAMPLES:
  mie-decoder decode rec.mie -o out.csv
  mie-decoder decode rec.mie --inline-errors --include-rts 15
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
    input: PathBuf,
    output: Option<PathBuf>,
    inline_errors: bool,
    no_clobber: bool,
    allow_partial: bool,
    time_format: Option<TimestampFormat>,
    strict: Option<bool>,
    output_format: Option<String>,

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

// ── Top-level entry ───────────────────────────────────────────────────

pub fn run(argv: Vec<String>) -> ExitCode {
    let mut iter = argv.into_iter().skip(1).peekable();

    // Pull global flags + --help / --version that may appear before the command.
    let mut globals = GlobalArgs::default();
    let cmd_token = loop {
        match iter.peek().map(String::as_str) {
            Some("-h") | Some("--help") => {
                print!("{HELP}");
                return ExitCode::SUCCESS;
            }
            Some("-V") | Some("--version") => {
                println!("mie-decoder {VERSION}");
                return ExitCode::SUCCESS;
            }
            Some("--log-level") => {
                iter.next();
                match iter.next() {
                    Some(v) => globals.log_level = Some(v),
                    None => return die("--log-level requires a value"),
                }
            }
            Some(s) if s.starts_with("--log-level=") => {
                let v = iter.next().unwrap();
                globals.log_level = Some(v["--log-level=".len()..].to_string());
            }
            Some("--config") => {
                iter.next();
                match iter.next() {
                    Some(v) => globals.config = Some(PathBuf::from(v)),
                    None => return die("--config requires a path"),
                }
            }
            Some(s) if s.starts_with("--config=") => {
                let v = iter.next().unwrap();
                globals.config = Some(PathBuf::from(&v["--config=".len()..]));
            }
            Some(_) => break iter.next(),
            None => {
                eprint!("{HELP}");
                return ExitCode::from(2);
            }
        }
    };

    let Some(cmd_token) = cmd_token else {
        eprint!("{HELP}");
        return ExitCode::from(2);
    };

    // Parse subcommand-specific args. Process control (printing help,
    // selecting an exit code) is decided HERE — the parse helpers only
    // signal "user wanted help" via ParseError::HelpRequested.
    let command = match cmd_token.as_str() {
        "decode" => match parse_decode(&mut iter) {
            Ok(c) => Command::Decode(Box::new(c)),
            Err(ParseError::HelpRequested) => {
                print!("{HELP}");
                return ExitCode::SUCCESS;
            }
            Err(ParseError::Other(e)) => return die(&e),
        },
        "count" => match parse_count(&mut iter) {
            Ok(p) => Command::Count(p),
            Err(ParseError::HelpRequested) => {
                print!("{HELP}");
                return ExitCode::SUCCESS;
            }
            Err(ParseError::Other(e)) => return die(&e),
        },
        "dump" => match parse_dump(&mut iter) {
            Ok(c) => Command::Dump(Box::new(c)),
            Err(ParseError::HelpRequested) => {
                print!("{HELP}");
                return ExitCode::SUCCESS;
            }
            Err(ParseError::Other(e)) => return die(&e),
        },
        "-h" | "--help" => {
            print!("{HELP}");
            return ExitCode::SUCCESS;
        }
        other => return die(&format!("Unknown command: {other:?}")),
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

    // Decode returns a Result<ExitCode, String> so it can choose exit
    // codes 2 (no-records) and 3 (partial-unrecoverable) without
    // routing through the generic exit-1 error path. Count/Dump still
    // use the simpler Result<(), String> contract and map to either 0
    // (Ok) or 1 (Err) via map_simple_to_exit below.
    let result: Result<ExitCode, String> = match command {
        Command::Decode(args) => run_decode(globals, *args),
        Command::Count(input) => run_count(globals, input).map(|()| ExitCode::SUCCESS),
        Command::Dump(args) => run_dump(globals, *args).map(|()| ExitCode::SUCCESS),
    };

    match result {
        Ok(code) => code,
        Err(e) => {
            log_error!("{e}");
            eprintln!("Error: {e}");
            ExitCode::from(1)
        }
    }
}

fn die(msg: &str) -> ExitCode {
    eprintln!("Error: {msg}\n\n{HELP}");
    ExitCode::from(2)
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

fn parse_decode(iter: &mut ArgIter<'_>) -> Result<DecodeArgs, ParseError> {
    let mut args = DecodeArgs::default();
    let mut input_seen = false;

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
            "--format" => {
                args.output_format = Some(next_value("--format", iter)?);
            }
            s if s.starts_with("--format=") => {
                args.output_format = Some(s["--format=".len()..].to_string());
            }
            // Filter flags: each takes ONE value. Multiple values either
            // repeat the flag or comma-separate within one value:
            //   --include-rts 15
            //   --include-rts 15,20,31
            //   --include-rts 15 --include-rts 31
            // Any of those leaves a trailing positional like `file.mie`
            // free to bind to `args.input`. Both space- and `=`-form
            // value syntax are accepted.
            "--exclude-types" => {
                for v in split_csv(&next_value("--exclude-types", iter)?) {
                    args.exclude_types
                        .push(parse_type_name(&v).map_err(|e| e.0)?);
                }
            }
            s if s.starts_with("--exclude-types=") => {
                for v in split_csv(&s["--exclude-types=".len()..]) {
                    args.exclude_types
                        .push(parse_type_name(&v).map_err(|e| e.0)?);
                }
            }
            "--include-types" => {
                for v in split_csv(&next_value("--include-types", iter)?) {
                    args.include_types
                        .push(parse_type_name(&v).map_err(|e| e.0)?);
                }
            }
            s if s.starts_with("--include-types=") => {
                for v in split_csv(&s["--include-types=".len()..]) {
                    args.include_types
                        .push(parse_type_name(&v).map_err(|e| e.0)?);
                }
            }
            "--exclude-rts" => {
                for v in split_csv(&next_value("--exclude-rts", iter)?) {
                    args.exclude_rts.push(parse_u8_value(&v, "--exclude-rts")?);
                }
            }
            s if s.starts_with("--exclude-rts=") => {
                for v in split_csv(&s["--exclude-rts=".len()..]) {
                    args.exclude_rts.push(parse_u8_value(&v, "--exclude-rts")?);
                }
            }
            "--include-rts" => {
                for v in split_csv(&next_value("--include-rts", iter)?) {
                    args.include_rts.push(parse_u8_value(&v, "--include-rts")?);
                }
            }
            s if s.starts_with("--include-rts=") => {
                for v in split_csv(&s["--include-rts=".len()..]) {
                    args.include_rts.push(parse_u8_value(&v, "--include-rts")?);
                }
            }
            "--exclude-buses" => {
                for v in split_csv(&next_value("--exclude-buses", iter)?) {
                    args.exclude_buses
                        .push(parse_bus_name(&v).map_err(|e| e.0)?);
                }
            }
            s if s.starts_with("--exclude-buses=") => {
                for v in split_csv(&s["--exclude-buses=".len()..]) {
                    args.exclude_buses
                        .push(parse_bus_name(&v).map_err(|e| e.0)?);
                }
            }
            "--include-buses" => {
                for v in split_csv(&next_value("--include-buses", iter)?) {
                    args.include_buses
                        .push(parse_bus_name(&v).map_err(|e| e.0)?);
                }
            }
            s if s.starts_with("--include-buses=") => {
                for v in split_csv(&s["--include-buses=".len()..]) {
                    args.include_buses
                        .push(parse_bus_name(&v).map_err(|e| e.0)?);
                }
            }
            "--exclude-subaddresses" => {
                for v in split_csv(&next_value("--exclude-subaddresses", iter)?) {
                    args.exclude_subaddresses
                        .push(parse_u8_value(&v, "--exclude-subaddresses")?);
                }
            }
            s if s.starts_with("--exclude-subaddresses=") => {
                for v in split_csv(&s["--exclude-subaddresses=".len()..]) {
                    args.exclude_subaddresses
                        .push(parse_u8_value(&v, "--exclude-subaddresses")?);
                }
            }
            "--include-subaddresses" => {
                for v in split_csv(&next_value("--include-subaddresses", iter)?) {
                    args.include_subaddresses
                        .push(parse_u8_value(&v, "--include-subaddresses")?);
                }
            }
            s if s.starts_with("--include-subaddresses=") => {
                for v in split_csv(&s["--include-subaddresses=".len()..]) {
                    args.include_subaddresses
                        .push(parse_u8_value(&v, "--include-subaddresses")?);
                }
            }
            "-h" | "--help" => return Err(ParseError::HelpRequested),
            s if s.starts_with('-') => {
                return Err(format!("unknown decode option: {s}").into());
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
        return Err("decode requires an input file".to_string().into());
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
            "invalid {source}: {value:?}; valid: DEBUG, INFO, WARNING, ERROR, CRITICAL"
        )),
    }
}

/// Load `--config` (or the built-in defaults if none was specified) and
/// apply log-level precedence: config overrides the run() default; CLI
/// overrides config. Used by every subcommand so a malformed config
/// file is rejected uniformly regardless of whether you ran `decode`,
/// `count`, or `dump`.
fn resolve_config(globals: &GlobalArgs) -> Result<DecoderConfig, String> {
    let cfg = load_config(globals.config.as_deref()).map_err(|e| e.0)?;

    // The config file's log_level is validated at load time (see
    // config::parse_into_config), so apply_log_level cannot fail here
    // unless someone constructed a DecoderConfig manually with a bogus
    // string. Treat that case the same as an invalid CLI value.
    apply_log_level("[logging].level (in config)", &cfg.log_level)?;

    if let Some(s) = &globals.log_level {
        apply_log_level("--log-level", s)?;
    }

    Ok(cfg)
}

/// Open the input file and configure the reader from `cfg`. The
/// String-flavored error type is what every subcommand runner returns,
/// so the conversion is folded in here.
fn open_reader(path: &Path, cfg: &DecoderConfig) -> Result<MieFileReader, String> {
    MieFileReader::with_options(
        path,
        ReaderOptions {
            strict: cfg.strict,
            time_format: cfg.time_format,
        },
    )
    .map_err(format_mie_error)
}

fn run_decode(globals: GlobalArgs, args: DecodeArgs) -> Result<ExitCode, String> {
    let cfg = resolve_config(&globals)?;

    let cfg = cfg.with_overrides(ConfigOverrides {
        time_format: args.time_format,
        strict: args.strict,
        error_mode: if args.inline_errors {
            Some(ErrorMode::Inline)
        } else {
            None
        },
        output_format: args.output_format.clone(),
        // CLI flag flips no_clobber on; absence leaves the config value
        // intact (Some(false) would clobber a `true` from the config).
        no_clobber: if args.no_clobber { Some(true) } else { None },
        // Same precedence pattern for --allow-partial.
        allow_partial: if args.allow_partial { Some(true) } else { None },
        exclude_types: args.exclude_types,
        exclude_rts: args.exclude_rts,
        exclude_buses: args.exclude_buses,
        exclude_subaddresses: args.exclude_subaddresses,
        include_types: args.include_types,
        include_rts: args.include_rts,
        include_buses: args.include_buses,
        include_subaddresses: args.include_subaddresses,
        log_level: globals.log_level.clone(),
    });

    if cfg.output_format != "csv" {
        return Err(format!(
            "output format {:?} not yet supported (only 'csv')",
            cfg.output_format
        ));
    }

    let reader = open_reader(&args.input, &cfg)?;
    log_info!(
        "opened {} ({} bytes)",
        reader.path().display(),
        reader.file_size()
    );

    // Build the iterator chain once. Exactly one of the three branches
    // below consumes it.
    let messages = reader.iter().filter_messages(cfg.filters.clone());

    // WriteOptions populated once with both file-output safety checks
    // (input/output collision per L2-WRT-014 and no-clobber per
    // L2-WRT-017) and L1-EXIT-004 allow_partial.
    let write_opts = WriteOptions {
        input_path: Some(args.input.clone()),
        no_clobber: cfg.no_clobber,
        allow_partial: cfg.allow_partial,
    };

    // Stdout is `output == None`; the spec's path-safety checks are
    // file-only so they are bypassed here. Separate mode requires a
    // file path; stdout in that case forces inline behavior with a
    // WARN, matching the v1 behavior.
    let write_result = if cfg.error_mode == ErrorMode::Separate {
        match args.output.as_ref() {
            None => {
                // Stdout can't be split; force inline behavior with
                // a warning and ignore file-only write options.
                crate::log_warn!("stdout output forces inline error mode");
                write_csv(messages, None, WriteOptions::default())
            }
            Some(output) => write_csv_split(messages, output, write_opts).inspect(|outcome| {
                log_info!(
                    "wrote {} messages + {} errors to {}",
                    outcome.normal_count,
                    outcome.error_count,
                    output.display()
                );
            }),
        }
    } else {
        write_csv(messages, args.output.as_deref(), write_opts)
    };

    // After the iterator is exhausted, the reader's atomic sync_losses
    // counter is authoritative for the L1-EXIT-005 exit-class summary
    // (Complete vs PartialRecovered). Querying here is safe because
    // the iterator (which borrows the reader) was already moved into
    // and consumed by write_csv* above.
    let sync_losses = reader.sync_losses();

    Ok(classify_decode_exit(write_result, sync_losses))
}

/// Map a writer-side result + the reader's sync-loss count to an
/// `ExitCode` per L1-EXIT-002 through L1-EXIT-005 and L2-CLI-011. Emits the
/// one-line exit-class summary required by L1-EXIT-005 in every branch.
fn classify_decode_exit(
    r: crate::error::MieResult<crate::writer::WriteOutcome>,
    sync_losses: u64,
) -> ExitCode {
    match r {
        Ok(outcome) => {
            let class = if outcome.partial.is_some() {
                "partial-unrecoverable"
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
            ExitCode::from(2)
        }
        Err(e @ MieError::HomogeneousPayload { .. }) => {
            // L2-SYN-018 + L1-EXIT-002: semantically a "wrong file
            // type" rejection (the input is a single-byte pad rather
            // than an MIE recording), same exit-code class as
            // NoValidRecords.
            log_error!("{e}");
            eprintln!("Error: {e}");
            log_info!("decode exit class: no-records");
            ExitCode::from(2)
        }
        Err(e @ MieError::UnrecoverableSyncLoss { .. }) => {
            log_error!("{e}");
            eprintln!("Error: {e}");
            log_info!(
                "decode exit class: partial-unrecoverable (sync_losses={sync_losses}); \
                 pass --allow-partial to preserve the rows decoded so far"
            );
            ExitCode::from(3)
        }
        Err(e) => {
            log_error!("{e}");
            eprintln!("Error: {e}");
            ExitCode::from(1)
        }
    }
}

fn run_count(globals: GlobalArgs, input: PathBuf) -> Result<(), String> {
    let cfg = resolve_config(&globals)?;
    let reader = open_reader(&input, &cfg)?;

    // Apply config's filters to the count, matching decode's behavior.
    // A user who wants raw counts can omit [filter] from their config.
    let filter_cfg = cfg.filters.clone();
    let mut count: u64 = 0;
    for item in reader.iter().filter_messages(filter_cfg) {
        match item {
            Ok(_) => count += 1,
            Err(e) => return Err(format_mie_error(e)),
        }
    }
    eprintln!("{count} messages in {}", reader.path().display());
    Ok(())
}

fn run_dump(globals: GlobalArgs, args: DumpArgs) -> Result<(), String> {
    // dump only consumes log_level from config; time_format / strict /
    // filters don't apply to a hex view. We still call resolve_config
    // so a malformed config errors out consistently with the other
    // subcommands.
    let _cfg = resolve_config(&globals)?;

    if args.raw {
        hex_dump_raw_to_stdout(&args.input, args.offset, args.length).map_err(format_mie_error)
    } else {
        hex_dump_records_to_stdout(&args.input, args.records, args.offset).map_err(format_mie_error)
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
        assert_eq!(parsed.input, PathBuf::from("recording.mie"));
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
        assert_eq!(parsed.input, PathBuf::from("file.mie"));
        assert_eq!(parsed.include_rts, vec![15]);
    }

    /// Comma-separated values within a single flag.
    /// Requirements: L2-CLI-010
    #[test]
    fn filter_flag_accepts_comma_separated_values() {
        let mut it = args(&["--include-rts", "15,20,31", "file.mie"]);
        let parsed = parse_decode(&mut it).unwrap();
        assert_eq!(parsed.input, PathBuf::from("file.mie"));
        assert_eq!(parsed.include_rts, vec![15, 20, 31]);
    }

    /// Repeating a filter flag accumulates values.
    /// Requirements: L2-CLI-010
    #[test]
    fn filter_flag_repeats_accumulate() {
        let mut it = args(&["--include-rts", "15", "--include-rts", "31", "file.mie"]);
        let parsed = parse_decode(&mut it).unwrap();
        assert_eq!(parsed.input, PathBuf::from("file.mie"));
        assert_eq!(parsed.include_rts, vec![15, 31]);
    }

    /// `--flag=value` syntax with comma-separation.
    /// Requirements: L2-CLI-010
    #[test]
    fn filter_flag_accepts_eq_form() {
        let mut it = args(&["--include-rts=15,20", "file.mie"]);
        let parsed = parse_decode(&mut it).unwrap();
        assert_eq!(parsed.input, PathBuf::from("file.mie"));
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
        assert_eq!(parsed.input, PathBuf::from("rec.mie"));
        assert_eq!(parsed.exclude_types, vec![0x20]);
        assert_eq!(parsed.include_buses, vec![crate::models::Bus::A]);
        assert_eq!(parsed.exclude_subaddresses, vec![0, 31]);
    }

    /// Old greedy-form usage now produces a clear error rather than
    /// silently misbinding the filename.
    /// Requirements: L2-CLI-010
    #[test]
    fn filter_flag_old_greedy_form_fails_loudly() {
        // `--include-rts 15 31 file.mie` used to put 15+31 in include
        // and "file.mie" as input. Now: 15 binds to include, "31" gets
        // taken as the positional input, "file.mie" is unexpected.
        // The user gets an error instead of silent miswiring.
        let mut it = args(&["--include-rts", "15", "31", "file.mie"]);
        match parse_decode(&mut it) {
            Err(ParseError::Other(msg)) => {
                assert!(msg.contains("unexpected positional"));
            }
            other => panic!("expected Other(unexpected positional...), got {other:?}"),
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

    /// Requirements: L2-CLI-005
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
            Err(msg) => assert!(
                msg.contains("Invalid time_format"),
                "expected config error, got: {msg}"
            ),
            Ok(()) => panic!("expected config error, got Ok"),
        }
    }

    /// Requirements: L2-CLI-005
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
            Err(msg) => assert!(
                msg.contains("Invalid time_format"),
                "expected config error, got: {msg}"
            ),
            Ok(()) => panic!("expected config error, got Ok"),
        }
    }

    /// Requirements: L2-CFG-005
    #[test]
    fn run_count_propagates_missing_config_file() {
        let globals = GlobalArgs {
            log_level: None,
            config: Some(PathBuf::from("/no/such/config.toml")),
        };
        let result = run_count(globals, PathBuf::from("/no/such/recording.mie"));
        match result {
            Err(msg) => assert!(
                msg.contains("Config file not found"),
                "expected 'Config file not found' error, got: {msg}"
            ),
            Ok(()) => panic!("expected error, got Ok"),
        }
    }

    // ── Log-level validation ─────────────────────────────────────────
    //
    // Regression: --log-level NOPE used to be silently ignored (the
    // code did `if let Some(lvl) = Level::parse(s)` and never bothered
    // with the None branch). Now invalid values fail loudly:
    //   - CLI input fails at run() entry with exit 2 (usage error)
    //   - Config-file value fails at config load time with exit 1
    //     (runtime error)

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

    /// Requirements: L2-CFG-010
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
            Err(msg) => assert!(
                msg.contains("Invalid logging.level"),
                "expected config-level error, got: {msg}"
            ),
            Ok(()) => panic!("expected error, got Ok"),
        }
    }

    /// Requirements: L2-CLI-004
    #[test]
    fn run_count_with_invalid_cli_log_level_fails_via_resolve_config() {
        // The run() entry-point catches bad CLI levels first (exit 2),
        // but resolve_config — which is what the runners use — also
        // re-validates the CLI value. Test that path directly.
        let globals = GlobalArgs {
            log_level: Some("NOPE".to_string()),
            config: None,
        };
        let result = run_count(globals, PathBuf::from("/no/such/recording.mie"));
        match result {
            Err(msg) => assert!(
                msg.contains("invalid --log-level"),
                "expected CLI-level error, got: {msg}"
            ),
            Ok(()) => panic!("expected error, got Ok"),
        }
    }
}
