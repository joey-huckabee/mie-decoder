//! Hand-rolled argument parser and CLI dispatch.
//!
//! Surface (v2 redesign):
//!
//! ```text
//! mie-decoder [--log-level L] [--config PATH] <command> [opts...]
//! ```
//!
//! Commands: `decode`, `count`, `dump`.

use std::path::PathBuf;
use std::process::ExitCode;

use crate::config::{ConfigOverrides, load_config, parse_bus_name, parse_type_name};
use crate::dump::{hex_dump_raw_to_stdout, hex_dump_records_to_stdout};
use crate::error::MieError;
use crate::filter::FilterIterExt;
use crate::log::{self, Level};
use crate::models::{ErrorMode, TimestampFormat};
use crate::reader::{MieFileReader, ReaderOptions};
use crate::writer::{write_csv, write_csv_split};
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
  --time-format auto|irig|standard      Default auto
  --strict                              Raise on invalid records
  --format csv                          Output format (csv only at present)
  --exclude-types T1 [T2 ...]           Names or 0xNN hex codes
  --exclude-rts N1 [N2 ...]
  --exclude-buses A|B [...]
  --exclude-subaddresses N1 [N2 ...]
  --include-types T1 [T2 ...]
  --include-rts N1 [N2 ...]
  --include-buses A|B [...]
  --include-subaddresses N1 [N2 ...]

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
    Decode(DecodeArgs),
    Count(PathBuf),
    Dump(DumpArgs),
}

#[derive(Debug, Default)]
struct DecodeArgs {
    input: PathBuf,
    output: Option<PathBuf>,
    inline_errors: bool,
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

    // Parse subcommand-specific args.
    let command = match cmd_token.as_str() {
        "decode" => match parse_decode(&mut iter) {
            Ok(c) => Command::Decode(c),
            Err(e) => return die(&e),
        },
        "count" => match parse_count(&mut iter) {
            Ok(p) => Command::Count(p),
            Err(e) => return die(&e),
        },
        "dump" => match parse_dump(&mut iter) {
            Ok(c) => Command::Dump(c),
            Err(e) => return die(&e),
        },
        "-h" | "--help" => {
            print!("{HELP}");
            return ExitCode::SUCCESS;
        }
        other => return die(&format!("Unknown command: {other:?}")),
    };

    // Apply log level (CLI > config > default).
    let level_str = globals.log_level.clone().unwrap_or_else(|| {
        // Default applied if no config or CLI sets it.
        "WARNING".to_string()
    });
    if let Some(lvl) = Level::parse(&level_str) {
        log::set_level(lvl);
    }

    log_info!("mie-decoder v{VERSION}");

    let result = match command {
        Command::Decode(args) => run_decode(globals, args),
        Command::Count(input) => run_count(input),
        Command::Dump(args) => run_dump(args),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
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

/// Collect a multi-value arg until we hit something that starts with `-`.
fn collect_multi(iter: &mut ArgIter<'_>) -> Vec<String> {
    let mut out = Vec::new();
    while let Some(p) = iter.peek() {
        if p.starts_with('-') {
            break;
        }
        out.push(iter.next().unwrap());
    }
    out
}

fn parse_decode(iter: &mut ArgIter<'_>) -> Result<DecodeArgs, String> {
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
            "--exclude-types" => {
                for v in collect_multi(iter) {
                    args.exclude_types.push(parse_type_name(&v).map_err(|e| e.0)?);
                }
            }
            "--include-types" => {
                for v in collect_multi(iter) {
                    args.include_types.push(parse_type_name(&v).map_err(|e| e.0)?);
                }
            }
            "--exclude-rts" => {
                for v in collect_multi(iter) {
                    args.exclude_rts.push(parse_u8_value(&v, "--exclude-rts")?);
                }
            }
            "--include-rts" => {
                for v in collect_multi(iter) {
                    args.include_rts.push(parse_u8_value(&v, "--include-rts")?);
                }
            }
            "--exclude-buses" => {
                for v in collect_multi(iter) {
                    args.exclude_buses.push(parse_bus_name(&v).map_err(|e| e.0)?);
                }
            }
            "--include-buses" => {
                for v in collect_multi(iter) {
                    args.include_buses.push(parse_bus_name(&v).map_err(|e| e.0)?);
                }
            }
            "--exclude-subaddresses" => {
                for v in collect_multi(iter) {
                    args.exclude_subaddresses
                        .push(parse_u8_value(&v, "--exclude-subaddresses")?);
                }
            }
            "--include-subaddresses" => {
                for v in collect_multi(iter) {
                    args.include_subaddresses
                        .push(parse_u8_value(&v, "--include-subaddresses")?);
                }
            }
            "-h" | "--help" => {
                print!("{HELP}");
                std::process::exit(0);
            }
            s if s.starts_with('-') => return Err(format!("unknown decode option: {s}")),
            _ => {
                if input_seen {
                    return Err(format!("unexpected positional argument: {arg}"));
                }
                args.input = PathBuf::from(arg);
                input_seen = true;
            }
        }
    }

    if !input_seen {
        return Err("decode requires an input file".to_string());
    }
    Ok(args)
}

fn parse_count(iter: &mut ArgIter<'_>) -> Result<PathBuf, String> {
    let mut path: Option<PathBuf> = None;
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                std::process::exit(0);
            }
            s if s.starts_with('-') => return Err(format!("unknown count option: {s}")),
            _ => {
                if path.is_some() {
                    return Err(format!("unexpected positional argument: {arg}"));
                }
                path = Some(PathBuf::from(arg));
            }
        }
    }
    path.ok_or_else(|| "count requires an input file".to_string())
}

fn parse_dump(iter: &mut ArgIter<'_>) -> Result<DumpArgs, String> {
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
                args.records =
                    Some(parse_int_value(&s["--records=".len()..], "--records")? as u64);
            }
            "-h" | "--help" => {
                print!("{HELP}");
                std::process::exit(0);
            }
            s if s.starts_with('-') => return Err(format!("unknown dump option: {s}")),
            _ => {
                if input_seen {
                    return Err(format!("unexpected positional argument: {arg}"));
                }
                args.input = PathBuf::from(arg);
                input_seen = true;
            }
        }
    }

    if !input_seen {
        return Err("dump requires an input file".to_string());
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

fn run_decode(globals: GlobalArgs, args: DecodeArgs) -> Result<(), String> {
    // Load config (with file precedence < CLI overrides).
    let cfg = load_config(globals.config.as_deref()).map_err(|e| e.0)?;

    // Re-apply log level: config overrides default; CLI overrides config.
    if let Some(lvl) = Level::parse(&cfg.log_level) {
        log::set_level(lvl);
    }
    if let Some(s) = &globals.log_level {
        if let Some(lvl) = Level::parse(s) {
            log::set_level(lvl);
        }
    }

    let cfg = cfg.with_overrides(ConfigOverrides {
        time_format: args.time_format,
        strict: args.strict,
        error_mode: if args.inline_errors {
            Some(ErrorMode::Inline)
        } else {
            None
        },
        output_format: args.output_format.clone(),
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

    let reader = MieFileReader::with_options(
        &args.input,
        ReaderOptions {
            strict: cfg.strict,
            time_format: cfg.time_format,
        },
    )
    .map_err(|e| format_mie_error(e))?;

    log_info!(
        "opened {} ({} bytes)",
        reader.path().display(),
        reader.file_size()
    );

    // Build the iterator chain: reader → filter
    let filter_cfg = cfg.filters.clone();

    if cfg.error_mode == ErrorMode::Separate {
        let Some(ref output) = args.output else {
            // Stdout cannot be split; force inline behavior with a warning.
            crate::log_warn!("stdout output forces inline error mode");
            let messages = reader.iter().filter_messages(filter_cfg);
            write_csv(messages, None).map_err(format_mie_error)?;
            return Ok(());
        };
        let messages = reader.iter().filter_messages(filter_cfg);
        let (n, e) = write_csv_split(messages, output).map_err(format_mie_error)?;
        log_info!("wrote {n} messages + {e} errors to {}", output.display());
    } else {
        let messages = reader.iter().filter_messages(filter_cfg);
        write_csv(messages, args.output.as_deref()).map_err(format_mie_error)?;
    }

    Ok(())
}

fn run_count(input: PathBuf) -> Result<(), String> {
    let reader = MieFileReader::new(&input).map_err(format_mie_error)?;
    let mut count: u64 = 0;
    for item in reader.iter() {
        match item {
            Ok(_) => count += 1,
            Err(e) => return Err(format_mie_error(e)),
        }
    }
    eprintln!("{count} messages in {}", reader.path().display());
    Ok(())
}

fn run_dump(args: DumpArgs) -> Result<(), String> {
    if args.raw {
        hex_dump_raw_to_stdout(&args.input, args.offset, args.length).map_err(format_mie_error)
    } else {
        hex_dump_records_to_stdout(&args.input, args.records, args.offset).map_err(format_mie_error)
    }
}

fn format_mie_error(e: MieError) -> String {
    e.to_string()
}
