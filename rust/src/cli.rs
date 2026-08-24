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
use crate::order::OrderIterExt;
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
  -V, -v, --version                     Print version and exit
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
  --separate-errors                     Route errored/spurious records to a
                                        separate <stem>_errors.csv. Default:
                                        every record inline in the main CSV
                                        with ERROR/ERROR_CODE populated
  --no-clobber                          Refuse to overwrite an existing
                                        output file (L2-WRT-017)
  --allow-partial                       On unrecoverable mid-file sync
                                        loss, write a <output>.partial
                                        file and exit 0 instead of 3
                                        (L1-EXIT-004)
  --time-format auto|irig|standard      Default auto (case-insensitive)
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
  --delta-scope per-file|global         Scope DELTA is measured over in a
                                        multi-file merge (default per-file:
                                        each gap is to the previous same-key
                                        record from its OWN file, matching a
                                        single-file decode). global measures
                                        across the merged timeline. No effect
                                        on a single input. L2-MRG-005.
  --max-sort-group N                    Max consecutive same-TIME_STAMP records
                                        buffered to order rows by RT then MSG
                                        (range 1..=1048576, default 4096). Use 1
                                        to disable reordering and emit raw
                                        capture order. L2-WRT-022.
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
  --length N                            Bytes to dump, raw mode (decimal or 0xHEX)
  --records N                           Max records, record mode (decimal or 0xHEX)

EXAMPLES:
  mie-decoder decode rec.mie -o out.csv
  mie-decoder decode rec.mie --separate-errors --include-rts 15
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
    separate_errors: bool,
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
    /// `--delta-scope <SCOPE>`: DELTA measurement scope in a merge (L2-MRG-005).
    delta_scope: Option<crate::models::DeltaScope>,
    /// `--max-sort-group <N>`: cap on one buffered equal-timestamp run
    /// (L2-WRT-022); `1` disables canonical reordering.
    max_sort_group: Option<usize>,

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

#[must_use]
pub fn run(argv: Vec<String>) -> ExitCode {
    let mut iter = argv.into_iter().skip(1).peekable();

    // Pull global flags + --help / --version that may appear before the command.
    let mut globals = GlobalArgs::default();
    let cmd_token = match parse_global_flags(&mut iter, &mut globals) {
        Ok(token) => token,
        Err(code) => return code,
    };

    // Parse subcommand-specific args. Process control (printing help,
    // selecting an exit code) is decided inside `parse_subcommand`.
    let command = match parse_subcommand(&cmd_token, &mut iter) {
        Ok(c) => c,
        Err(code) => return code,
    };

    // Apply log level early so the version banner respects it. CLI
    // value if provided, else WARN default. An invalid CLI value
    // (e.g. `--log-level NOPE`) fails fast here via `die` (exit 4,
    // the usage class) instead of being silently ignored. The config file's level is layered
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

/// True for any accepted spelling of the version flag: the short `-V` / `-v`,
/// or a `--version` long flag in any letter case (`--version`, `--VERSION`,
/// `--Version`, …). Kept lenient on purpose so the two CLIs agree on every
/// spelling a user might reach for.
fn is_version_flag(arg: &str) -> bool {
    arg == "-V"
        || arg == "-v"
        || arg
            .strip_prefix("--")
            .is_some_and(|word| word.eq_ignore_ascii_case("version"))
}

/// Consume leading global flags (`--log-level`, `--config`) and `-h`/`-V`.
///
/// Returns `Ok(token)` with the first non-flag token (the subcommand), or
/// `Err(code)` when the caller (`run`) should return that exit code
/// immediately — help or version was printed, or the command line was a usage
/// error (including no subcommand at all).
fn parse_global_flags(
    iter: &mut ArgIter<'_>,
    globals: &mut GlobalArgs,
) -> Result<String, ExitCode> {
    loop {
        // Peek, because the first non-flag token is the SUBCOMMAND and belongs
        // to the caller. Split a clone so the joined and separated spellings
        // resolve here exactly as they do for subcommand flags -- globals are
        // parsed in this separate loop, so before the cursor each spelling had
        // a second, independent implementation.
        let Some(peeked) = iter.peek().cloned() else {
            eprint!("{HELP}");
            return Err(ExitCode::from(exit_code::USAGE));
        };
        let a = Arg::split(peeked);
        match a.name.as_str() {
            "-h" | "--help" if a.bare() => {
                print!("{HELP}");
                return Err(ExitCode::SUCCESS);
            }
            // `--version=1` is not a version request: it declines here and is
            // returned below as the subcommand token, reporting itself as an
            // unknown command.
            s if a.bare() && is_version_flag(s) => {
                println!("mie-decoder {VERSION}");
                return Err(ExitCode::SUCCESS);
            }
            "--log-level" => {
                iter.next(); // the flag token itself
                match a.value("--log-level", iter) {
                    Ok(v) => globals.log_level = Some(v),
                    Err(_) => return Err(die("--log-level requires a value")),
                }
            }
            "--config" => {
                iter.next();
                match a.value("--config", iter) {
                    // Its own message: a path, not a generic value.
                    Ok(v) => globals.config = Some(PathBuf::from(v)),
                    Err(_) => return Err(die("--config requires a path")),
                }
            }
            _ => {
                // `peek` just returned `Some`, so `next` cannot be `None`.
                // `unwrap_or_default` keeps the function total rather than
                // introducing a panic site (L1-ROB-001); an empty token would
                // fall through to the unknown-command arm, which is safe.
                return Ok(iter.next().unwrap_or_default());
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

/// Does this token look like an option rather than a value?
///
/// Used to decide whether the **separated** form may consume the next token.
/// Without this, `--mux-delimiter --no-mux` set the delimiter to the string
/// `"--no-mux"` and the `--no-mux` flag silently never ran — a wrong decode
/// that exited 0. Refusing turns that into a usage error, which is the whole
/// value of the check: the failure was silent, not merely inconsistent.
///
/// The rule is `argparse`'s, deliberately, because Python has always behaved
/// this way and the alternative was to make two implementations agree with
/// each other and disagree with the third. Its three exemptions are not
/// quirks to be tidied away — each keeps a real invocation working:
///
/// * **A lone `-`** is a value. `-o -` writes a file named `-` (L2-CLI-005).
/// * **A negative number** is a value, matched exactly as `argparse` does it
///   (`^-\d+$|^-\d*\.\d+$`). `--mux-field -1` is a documented feature —
///   negative indices count from the end — and `--collapse-window-us -5` must
///   still reach its own validator to be refused for being negative, rather
///   than being refused here for looking like a flag.
/// * **A token containing a space** is a value, since no option is spelled
///   that way.
///
/// Note what this does NOT cover: the joined form. `--mux-delimiter=--no-mux`
/// is unambiguous and stays legal in all three implementations, which is why
/// it is the documented way to pass a value that looks like a flag.
fn looks_like_option(token: &str) -> bool {
    if !token.starts_with('-') || token.chars().count() < 2 || token.contains(' ') {
        return false;
    }
    !is_negative_number(token)
}

/// `argparse`'s `^-\d+$|^-\d*\.\d+$`, hand-rolled because this crate has no
/// regex dependency and is not going to acquire one for four lines.
///
/// Deliberately narrow, matching the original: `-5` and `-.5` and `-5.5` are
/// numbers; `-5e3` and `-0x5` are not, because `argparse` says they are not.
/// Widening it here would re-open the divergence this closes.
fn is_negative_number(token: &str) -> bool {
    let Some(rest) = token.strip_prefix('-') else {
        return false;
    };
    if rest.is_empty() {
        return false;
    }
    match rest.split_once('.') {
        // `-\d+` — digits only, no point.
        None => rest.bytes().all(|b| b.is_ascii_digit()),
        // `-\d*\.\d+` — optional leading digits, then a point, then at least
        // one digit. A second point makes it not a number.
        Some((int_part, frac_part)) => {
            !frac_part.is_empty()
                && int_part.bytes().all(|b| b.is_ascii_digit())
                && frac_part.bytes().all(|b| b.is_ascii_digit())
        }
    }
}

fn next_value(name: &str, iter: &mut ArgIter<'_>) -> Result<String, String> {
    match iter.peek() {
        None => Err(format!("{name} requires a value")),
        Some(token) if looks_like_option(token) => Err(format!(
            "{name} requires a value, but the next argument is an option: {token}; \
             to pass it as a value, write {name}={token}"
        )),
        // `peek` returned `Some`, so `next` cannot be `None`.
        Some(_) => Ok(iter.next().unwrap_or_default()),
    }
}

/// One argument token, split at the first `=`.
///
/// `--flag=value` and `--flag value` are the same invocation, and every flag
/// that takes a value accepts both. Doing that split *once*, here, is what lets
/// a flag be handled by a single match arm: before this the two spellings were
/// two arms apiece — an exact-name arm and a `starts_with("--flag=")` arm that
/// re-sliced the token by a hard-coded prefix length — repeated at 26 sites. A
/// flag added with only the first arm would silently reject the joined form,
/// and no gate would have noticed: `cli-surface-parity` compares flag *names*.
///
/// Three rules, each load-bearing and each pinned by a test:
///
/// * **Only `--` tokens split.** A positional path may contain `=`
///   (`a=b.mie`), and `-o=out.csv` is not a spelling this CLI accepts — both
///   must survive untouched.
/// * **Only the first `=` separates**, so `--mux-delimiter==` sets the
///   delimiter to `=`.
/// * **`--flag=` is an empty value, not a missing one.** The flag's own
///   validator then decides: an empty filter list is fine, an empty delimiter
///   is not. C++ got this wrong in the other direction and reported an unknown
///   option where Rust and Python decoded normally.
///
/// `raw` is kept because the unknown-option message quotes the token as the
/// user typed it, and a flag that takes no value rejects `--flag=x` by falling
/// through to exactly that message.
struct Arg {
    /// The token exactly as it arrived.
    raw: String,
    /// The flag name: everything before the first `=`, or the whole token.
    name: String,
    /// The value that was joined with `=`, if any. `Some("")` for `--flag=`.
    inline: Option<String>,
}

impl Arg {
    fn split(raw: String) -> Self {
        // Single-dash and bare tokens are never split; see the type docs.
        if raw.starts_with("--")
            && let Some(at) = raw.find('=')
        {
            return Self {
                name: raw[..at].to_string(),
                inline: Some(raw[at + 1..].to_string()),
                raw,
            };
        }
        Self {
            name: raw.clone(),
            inline: None,
            raw,
        }
    }

    /// The value for a flag that requires one: the joined value if there was
    /// one, otherwise the next token.
    ///
    /// `name` is passed rather than read from `self.name` so the "requires a
    /// value" message names the long form even when the short one was used
    /// (`-o` reports `--output`), which is what the message did before.
    fn value(&self, name: &str, iter: &mut ArgIter<'_>) -> Result<String, String> {
        match &self.inline {
            Some(v) => Ok(v.clone()),
            None => next_value(name, iter),
        }
    }

    /// True when this token carried no `=`, which is the guard a value-less
    /// flag needs: `--no-mux=true` must not quietly set the flag and discard
    /// the value, so the arm declines and the token falls through to the
    /// unknown-option message carrying `raw`.
    fn bare(&self) -> bool {
        self.inline.is_none()
    }
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

    while let Some(token) = iter.next() {
        // One arm per flag: `Arg::split` has already resolved `--flag=value`
        // against `--flag value`, so neither spelling appears below.
        let a = Arg::split(token);
        match a.name.as_str() {
            "-o" | "--output" => {
                args.output = Some(PathBuf::from(a.value("--output", iter)?));
            }
            // Value-less flags decline a joined value rather than discard it;
            // `--no-mux=true` falls through to the unknown-option arm.
            "--separate-errors" if a.bare() => args.separate_errors = true,
            "--no-clobber" if a.bare() => args.no_clobber = true,
            "--allow-partial" if a.bare() => args.allow_partial = true,
            "--strict" if a.bare() => args.strict = Some(true),
            "--no-mux" if a.bare() => args.no_mux = true,
            "--collapse-duplicates" if a.bare() => args.collapse_duplicates = true,
            "--time-format" => {
                args.time_format = Some(parse_time_format_arg(&a.value("--time-format", iter)?)?);
            }
            "--detect-records" => {
                args.detect_records =
                    Some(parse_detect_records(&a.value("--detect-records", iter)?)?);
            }
            "--lookahead-records" => {
                args.lookahead_records = Some(parse_lookahead_records(
                    &a.value("--lookahead-records", iter)?,
                )?);
            }
            "--standard-tick-rate-hz" => {
                args.standard_tick_rate_hz = Some(parse_standard_tick_rate_hz(
                    &a.value("--standard-tick-rate-hz", iter)?,
                )?);
            }
            "--format" => args.output_format = Some(a.value("--format", iter)?),
            "--mux-delimiter" => {
                args.mux_delimiter = Some(parse_mux_delimiter(&a.value("--mux-delimiter", iter)?)?);
            }
            "--mux-field" => {
                args.mux_field = Some(parse_mux_field(&a.value("--mux-field", iter)?)?);
            }
            "--max-sort-group" => {
                args.max_sort_group =
                    Some(parse_max_sort_group(&a.value("--max-sort-group", iter)?)?);
            }
            "--delta-scope" => {
                args.delta_scope = Some(parse_delta_scope(&a.value("--delta-scope", iter)?)?);
            }
            "--collapse-window-us" => {
                args.collapse_window_us = Some(parse_collapse_window_us(
                    &a.value("--collapse-window-us", iter)?,
                )?);
            }
            "--manifest" => {
                args.manifest = Some(PathBuf::from(a.value("--manifest", iter)?));
            }
            "--glob" => args.glob = Some(a.value("--glob", iter)?),
            // Filter flags: each takes ONE value. Multiple values either
            // repeat the flag or comma-separate within one value:
            //   --include-rts 15
            //   --include-rts 15,20,31
            //   --include-rts 15 --include-rts 31
            // Any of those leaves trailing positionals like `file.mie`
            // free to bind to `args.inputs`.
            "--exclude-types" => push_filter(
                &a.value("--exclude-types", iter)?,
                &mut args.exclude_types,
                parse_type_filter,
            )?,
            "--include-types" => push_filter(
                &a.value("--include-types", iter)?,
                &mut args.include_types,
                parse_type_filter,
            )?,
            "--exclude-rts" => push_filter(
                &a.value("--exclude-rts", iter)?,
                &mut args.exclude_rts,
                |v| parse_u8_value(v, "--exclude-rts").map_err(Into::into),
            )?,
            "--include-rts" => push_filter(
                &a.value("--include-rts", iter)?,
                &mut args.include_rts,
                |v| parse_u8_value(v, "--include-rts").map_err(Into::into),
            )?,
            "--exclude-buses" => push_filter(
                &a.value("--exclude-buses", iter)?,
                &mut args.exclude_buses,
                parse_bus_filter,
            )?,
            "--include-buses" => push_filter(
                &a.value("--include-buses", iter)?,
                &mut args.include_buses,
                parse_bus_filter,
            )?,
            "--exclude-subaddresses" => push_filter(
                &a.value("--exclude-subaddresses", iter)?,
                &mut args.exclude_subaddresses,
                |v| parse_u8_value(v, "--exclude-subaddresses").map_err(Into::into),
            )?,
            "--include-subaddresses" => push_filter(
                &a.value("--include-subaddresses", iter)?,
                &mut args.include_subaddresses,
                |v| parse_u8_value(v, "--include-subaddresses").map_err(Into::into),
            )?,
            "-h" | "--help" if a.bare() => return Err(ParseError::HelpRequested),
            // `raw`, not `name`: the message quotes what was typed, so
            // `--no-mux=true` reports itself in full.
            _ if a.name.starts_with('-') => {
                return Err(format!("unknown decode option: {}", a.raw).into());
            }
            // Positional input path(s). One or more is accepted; more than one
            // resolved input triggers the time-sorted merge (L2-MRG-001).
            _ => args.inputs.push(PathBuf::from(a.raw)),
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
    for token in iter.by_ref() {
        // `count` has no value-taking flag today, but it goes through the same
        // cursor so that adding one cannot reintroduce a per-flag `=` arm.
        let a = Arg::split(token);
        match a.name.as_str() {
            "-h" | "--help" if a.bare() => return Err(ParseError::HelpRequested),
            _ if a.name.starts_with('-') => {
                return Err(format!("unknown count option: {}", a.raw).into());
            }
            _ => {
                if path.is_some() {
                    return Err(format!("unexpected positional argument: {}", a.raw).into());
                }
                path = Some(PathBuf::from(a.raw));
            }
        }
    }
    path.ok_or_else(|| ParseError::Other("count requires an input file".to_string()))
}

fn parse_dump(iter: &mut ArgIter<'_>) -> Result<DumpArgs, ParseError> {
    let mut args = DumpArgs::default();
    let mut input_seen = false;

    while let Some(token) = iter.next() {
        let a = Arg::split(token);
        match a.name.as_str() {
            "--raw" if a.bare() => args.raw = true,
            "--offset" => {
                args.offset = parse_int_value(&a.value("--offset", iter)?, "--offset")?;
            }
            "--length" => {
                args.length = Some(parse_int_value(&a.value("--length", iter)?, "--length")?);
            }
            "--records" => {
                args.records =
                    Some(parse_int_value(&a.value("--records", iter)?, "--records")? as u64);
            }
            "-h" | "--help" if a.bare() => return Err(ParseError::HelpRequested),
            _ if a.name.starts_with('-') => {
                return Err(format!("unknown dump option: {}", a.raw).into());
            }
            _ => {
                if input_seen {
                    return Err(format!("unexpected positional argument: {}", a.raw).into());
                }
                args.input = PathBuf::from(a.raw);
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
    TimestampFormat::from_name_ci(s)
        .ok_or_else(|| format!("invalid --time-format: {s:?}; valid: auto, irig, standard"))
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

/// `--max-sort-group` (L2-WRT-022): cap on one buffered equal-timestamp run.
/// Range-checked here so a bad value is a usage error (exit 4) rather than a
/// silent clamp, mirroring `--detect-records`.
fn parse_max_sort_group(s: &str) -> Result<usize, String> {
    let n: usize = s
        .trim()
        .parse()
        .map_err(|_| format!("invalid --max-sort-group: {s:?}; must be an integer"))?;
    if !(crate::order::MAX_SORT_GROUP_MIN..=crate::order::MAX_SORT_GROUP_MAX).contains(&n) {
        return Err(format!(
            "invalid --max-sort-group: {n}; valid range: [{}, {}]",
            crate::order::MAX_SORT_GROUP_MIN,
            crate::order::MAX_SORT_GROUP_MAX
        ));
    }
    Ok(n)
}

/// `--delta-scope` (L2-MRG-005). Shares `DeltaScope::from_name_ci` with the
/// config loader so the CLI and TOML accept exactly the same spellings.
fn parse_delta_scope(s: &str) -> Result<crate::models::DeltaScope, String> {
    crate::models::DeltaScope::from_name_ci(s.trim())
        .ok_or_else(|| format!("invalid --delta-scope: {s:?}. Valid: per-file, global"))
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
/// apply log-level precedence: config overrides the `run()` default; CLI
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
        // `--separate-errors` opts into the split-file mode; its absence leaves
        // the config value intact (the built-in default is now Inline).
        error_mode: if args.separate_errors {
            Some(ErrorMode::Separate)
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
        max_sort_group: args.max_sort_group,
        delta_scope: args.delta_scope,
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
        // L2-WRT-021: canonical row order is the LAST stage before the writer, so
        // the guarantee holds over exactly the rows that reach the CSV.
        let messages = readers[0]
            .iter()
            .filter_messages(cfg.filters.clone())
            .order_rows(cfg.max_sort_group);
        return Ok(write_messages(messages, output, cfg.error_mode, write_opts));
    }

    match crate::merge::MergedRecordIter::new(
        readers,
        cfg.standard_tick_rate_hz,
        cfg.allow_partial,
        cfg.strict,
    ) {
        Ok(merged) => {
            let merged = merged
                .collapse(cfg.collapse_duplicates, cfg.collapse_window_us)
                .delta_scope(cfg.delta_scope);
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
            // L2-WRT-021: order_rows sits after the merge and the filters, and
            // before `open_tail` — the tail is a terminal error that must stay
            // last, and the reorder stage would otherwise hold rows behind it.
            let result = write_messages(
                merged
                    .filter_messages(cfg.filters.clone())
                    .order_rows(cfg.max_sort_group)
                    .chain(open_tail),
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

    // One input → the single-file path; two or more → the time-sorted k-way
    // merge. DELTA is per-file on both paths unless `--delta-scope global` is
    // given (L2-MRG-005). Both feed the same writer. An incompatible-merge /
    // prime-time failure returns its exit code directly.
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
    let sync_losses: u64 = readers.iter().map(MieFileReader::sync_losses).sum();

    // L1-EXIT-010: report the empty-recording class only when *every* opened
    // input was a valid empty recording (so a merge that also drew rows from a
    // non-empty input stays `complete`). A single-file empty decode is the
    // common case; the writer has already produced a header-only CSV.
    let empty_recording = !readers.is_empty() && readers.iter().all(MieFileReader::empty_recording);

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
        let v: Vec<String> = values.iter().map(|s| (*s).to_string()).collect();
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

    /// `--time-format` accepts any letter case (auto/irig/standard), matching
    /// the config loader; an unrecognized value is a usage error.
    /// Requirements: L2-CLI-001
    #[test]
    fn time_format_arg_is_case_insensitive() {
        assert_eq!(
            parse_time_format_arg("IRIG").unwrap(),
            TimestampFormat::Irig
        );
        assert_eq!(
            parse_time_format_arg("Irig").unwrap(),
            TimestampFormat::Irig
        );
        assert_eq!(
            parse_time_format_arg("AUTO").unwrap(),
            TimestampFormat::Auto
        );
        assert_eq!(
            parse_time_format_arg("Standard").unwrap(),
            TimestampFormat::Standard
        );
        assert!(parse_time_format_arg("bogus").is_err());
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

    /// Every numeric `dump` argument accepts decimal and `0x` hex identically,
    /// and rejects a negative value (the parser is unsigned). Mirrors the Python
    /// `_nonneg_int` parity test.
    /// Requirements: L2-CLI-009
    #[test]
    fn parse_dump_numeric_args_accept_hex_and_reject_negative() {
        let d = parse_dump(&mut args(&[
            "f.mie",
            "--offset",
            "0x20",
            "--length",
            "16",
            "--records",
            "0x10",
        ]))
        .unwrap();
        assert_eq!(d.offset, 0x20);
        assert_eq!(d.length, Some(16));
        assert_eq!(d.records, Some(0x10));

        for flag in ["--offset", "--length", "--records"] {
            assert!(
                parse_dump(&mut args(&["f.mie", flag, "-1"])).is_err(),
                "{flag} should reject a negative value"
            );
        }
    }

    /// Parse errors should still surface as `ParseError::Other`, not panics
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
    /// The predicate itself, at the boundary, because the parse-level tests
    /// below cannot reach every shape cheaply. These values are argparse's,
    /// read out of `ArgumentParser._negative_number_matcher` rather than
    /// guessed: `-5e3` and `-0x5` really are options to argparse, and this
    /// must not "improve" on that.
    /// Requirements: L2-CLI-015
    #[test]
    fn looks_like_option_matches_argparse() {
        // Values -- consumed as the argument to a flag.
        for token in [
            "", "-", "-5", "-5.5", "-.5", "-0", "-00.5", "- x", "-1 2", "x", "a=b",
        ] {
            assert!(
                !looks_like_option(token),
                "{token:?} should be treated as a value"
            );
        }
        // Option-like -- refused as the argument to a flag.
        for token in [
            "-x",
            "-o",
            "-5e3",
            "-0x5",
            "-1a",
            "-.",
            "-..5",
            "-5.5.5",
            "--",
            "--foo",
            "--no-mux",
            "--flag=value",
        ] {
            assert!(
                looks_like_option(token),
                "{token:?} should be treated as an option"
            );
        }
    }

    /// The separated form must not swallow the following flag.
    ///
    /// This was the last cross-implementation divergence in flag parsing:
    /// `--mux-delimiter --no-mux` set the delimiter to the string "--no-mux"
    /// in Rust and C++ -- so `--no-mux` silently never ran and the decode
    /// succeeded with a wrong MUX column -- while Python refused it. A silent
    /// wrong answer is the worst of the three possible behaviours, so both
    /// moved to Python's.
    /// Requirements: L2-CLI-015
    #[test]
    fn a_flag_like_value_is_refused_in_the_separated_form() {
        for token in ["--no-mux", "--foo", "-o", "-x"] {
            let err = parse_decode(&mut args(&["--mux-delimiter", token, "rec.mie"])).unwrap_err();
            let ParseError::Other(msg) = err else {
                panic!("{token}: expected a usage error");
            };
            assert!(msg.contains("--mux-delimiter"), "{token}: got {msg:?}");
            // The message has to say how to get what the user probably wanted.
            assert!(
                msg.contains(&format!("--mux-delimiter={token}")),
                "{token}: message should suggest the joined form, got {msg:?}"
            );
        }
    }

    /// The joined form is unambiguous, so it still accepts a flag-like value
    /// -- and is what the refusal message points at.
    /// Requirements: L2-CLI-015
    #[test]
    fn the_joined_form_still_takes_a_flag_like_value() {
        let parsed = parse_decode(&mut args(&["--mux-delimiter=--no-mux", "rec.mie"])).unwrap();
        assert_eq!(parsed.mux_delimiter.as_deref(), Some("--no-mux"));
        // ...and it really is a delimiter, not the flag: `no_mux` stays unset.
        assert!(!parsed.no_mux);
    }

    /// The three exemptions, each of which keeps a real invocation working.
    /// Requirements: L2-CLI-005, L2-CLI-015
    #[test]
    fn the_exemptions_are_still_values() {
        // A lone `-` is a path, not stdout: `-o -` writes a file named `-`.
        let parsed = parse_decode(&mut args(&["-o", "-", "rec.mie"])).unwrap();
        assert_eq!(parsed.output, Some(PathBuf::from("-")));

        // A negative number: `--mux-field -1` counts from the end.
        let parsed = parse_decode(&mut args(&["--mux-field", "-1", "rec.mie"])).unwrap();
        assert_eq!(parsed.mux_field, Some(-1));

        // A negative number still reaches its OWN validator, so a flag that
        // forbids negatives refuses it for that reason and not for looking
        // like an option.
        let err = parse_decode(&mut args(&["--collapse-window-us", "-5", "rec.mie"])).unwrap_err();
        let ParseError::Other(msg) = err else {
            panic!("expected a usage error");
        };
        assert!(
            !msg.contains("is an option"),
            "should be refused by the validator, not the option guard: {msg:?}"
        );

        // A token containing a space is a value; no option is spelled so.
        let parsed = parse_decode(&mut args(&["--mux-delimiter", "- x", "rec.mie"])).unwrap();
        assert_eq!(parsed.mux_delimiter.as_deref(), Some("- x"));
    }

    /// Globals are a separate parse loop and need the guard too. Before it,
    /// `--log-level --config x` made Rust take "--config" as the level, then
    /// read "x" as the subcommand -- exit 4, but for an unrelated reason and
    /// with a message that named neither flag.
    /// Requirements: L2-CLI-015
    #[test]
    fn global_flags_refuse_a_flag_like_value() {
        let mut globals = GlobalArgs::default();
        let err = parse_global_flags(
            &mut args(&["--log-level", "--config", "x", "decode"]),
            &mut globals,
        )
        .unwrap_err();
        assert_eq!(err, ExitCode::from(exit_code::USAGE));
        assert!(
            globals.log_level.is_none(),
            "must not have consumed a value"
        );
    }

    /// Every valued flag, both spellings, compared structurally rather than by
    /// exit code — a spelling that parsed but dropped its value would still
    /// exit 0. `DecodeArgs` has no `PartialEq` (it is private, and deriving one
    /// only for a test is surface for its own sake), so `Debug` stands in: it
    /// renders every field, which is exactly the comparison wanted.
    ///
    /// This sweep is what the `Arg` cursor buys. Before it each of these flags
    /// carried two match arms, and the test could only have been written as a
    /// list of near-copies.
    /// Requirements: L2-CLI-015
    #[test]
    fn every_valued_flag_accepts_both_spellings() {
        let cases: &[(&str, &str)] = &[
            ("--output", "out.csv"),
            ("--time-format", "irig"),
            ("--format", "csv"),
            ("--detect-records", "4"),
            ("--lookahead-records", "2"),
            ("--standard-tick-rate-hz", "1000000"),
            ("--max-sort-group", "64"),
            ("--mux-delimiter", "_"),
            ("--mux-field", "0"),
            ("--delta-scope", "global"),
            ("--collapse-window-us", "10"),
            ("--manifest", "list.txt"),
            ("--glob", "*.mie"),
            ("--exclude-rts", "31"),
            ("--include-rts", "15"),
            ("--exclude-buses", "B"),
            ("--include-buses", "A"),
            ("--exclude-subaddresses", "30"),
            ("--include-subaddresses", "11"),
            ("--exclude-types", "RT_TO_RT"),
            ("--include-types", "BC_TO_RT"),
        ];

        for (flag, value) in cases {
            // --manifest and --glob are themselves input methods, so adding a
            // positional would fail the parse for an unrelated reason.
            let positional: &[&str] = if matches!(*flag, "--manifest" | "--glob") {
                &[]
            } else {
                &["rec.mie"]
            };

            let mut separated = vec![*flag, *value];
            separated.extend_from_slice(positional);
            let joined_flag = format!("{flag}={value}");
            let mut joined = vec![joined_flag.as_str()];
            joined.extend_from_slice(positional);

            let a = parse_decode(&mut args(&separated))
                .unwrap_or_else(|e| panic!("{flag} separated form failed: {e:?}"));
            let b = parse_decode(&mut args(&joined))
                .unwrap_or_else(|e| panic!("{flag} joined form failed: {e:?}"));
            assert_eq!(
                format!("{a:?}"),
                format!("{b:?}"),
                "{flag} spellings differ"
            );
        }
    }

    /// `--flag=` carries an EMPTY value, which the flag's own validator then
    /// accepts or rejects. It is not a malformed token.
    ///
    /// C++ had this wrong in the opposite direction: it required a character
    /// after the `=`, so `--exclude-rts=` was an "unknown option" (exit 4)
    /// where Rust and Python decoded normally (exit 0).
    /// Requirements: L2-CLI-015
    #[test]
    fn eq_form_with_empty_value_is_an_empty_value() {
        // Accepted: an empty filter excludes nothing, so the parse must match
        // the one that never passed the flag at all.
        let with = parse_decode(&mut args(&["--exclude-rts=", "rec.mie"])).unwrap();
        let without = parse_decode(&mut args(&["rec.mie"])).unwrap();
        assert!(with.exclude_rts.is_empty());
        assert_eq!(format!("{with:?}"), format!("{without:?}"));

        // Rejected: the validator refuses this, not the cursor.
        let err = parse_decode(&mut args(&["--mux-delimiter=", "rec.mie"])).unwrap_err();
        let ParseError::Other(msg) = err else {
            panic!("expected a usage error");
        };
        assert!(
            msg.contains("--mux-delimiter"),
            "message should name the flag, got {msg:?}"
        );
        assert!(
            !msg.contains("unknown"),
            "must reach the validator, not the unknown-option arm: {msg:?}"
        );
    }

    /// Only the first `=` separates; the remainder is the value verbatim.
    /// Requirements: L2-CLI-015
    #[test]
    fn only_the_first_equals_separates() {
        let parsed = parse_decode(&mut args(&["--mux-delimiter==", "rec.mie"])).unwrap();
        assert_eq!(parsed.mux_delimiter.as_deref(), Some("="));

        let parsed = parse_decode(&mut args(&["--glob=a=b*.mie"])).unwrap();
        assert_eq!(parsed.glob.as_deref(), Some("a=b*.mie"));
    }

    /// A flag that takes no value must REJECT a joined one rather than set
    /// itself and silently discard it, and the message quotes the token as it
    /// was typed.
    /// Requirements: L2-CLI-015
    #[test]
    fn a_valueless_flag_rejects_a_joined_value() {
        for token in [
            "--no-mux=true",
            "--no-mux=",
            "--separate-errors=1",
            "--strict=false",
        ] {
            let err = parse_decode(&mut args(&[token, "rec.mie"])).unwrap_err();
            let ParseError::Other(msg) = err else {
                panic!("{token}: expected a usage error");
            };
            assert!(
                msg.contains(token),
                "{token}: message should quote the whole token, got {msg:?}"
            );
        }
    }

    /// Splitting is confined to `--` tokens: a positional path may contain
    /// `=`, and `-o=value` is not a spelling this CLI accepts.
    /// Requirements: L2-CLI-015
    #[test]
    fn only_double_dash_tokens_are_split() {
        let parsed = parse_decode(&mut args(&["a=b.mie"])).unwrap();
        assert_eq!(parsed.inputs, vec![PathBuf::from("a=b.mie")]);

        let err = parse_decode(&mut args(&["-o=out.csv", "rec.mie"])).unwrap_err();
        let ParseError::Other(msg) = err else {
            panic!("expected a usage error");
        };
        assert!(msg.contains("-o=out.csv"), "got {msg:?}");
    }

    /// Globals are parsed in their own loop, so both spellings need proving
    /// there independently. `--version=1` is a value on a flag that takes
    /// none, so it is NOT a version request: it falls through as the
    /// subcommand token and is reported as an unknown command.
    /// Requirements: L2-CLI-015
    #[test]
    fn global_flags_accept_both_spellings() {
        for (argv, expected) in [
            (vec!["--log-level", "ERROR", "decode"], "ERROR"),
            (vec!["--log-level=ERROR", "decode"], "ERROR"),
            (vec!["--log-level=", "decode"], ""),
        ] {
            let mut globals = GlobalArgs::default();
            let sub = parse_global_flags(&mut args(&argv), &mut globals)
                .unwrap_or_else(|_| panic!("{argv:?} should yield a subcommand"));
            assert_eq!(sub, "decode", "{argv:?}");
            assert_eq!(globals.log_level.as_deref(), Some(expected), "{argv:?}");
        }

        for argv in [
            vec!["--config", "site.toml", "decode"],
            vec!["--config=site.toml", "decode"],
        ] {
            let mut globals = GlobalArgs::default();
            let sub = parse_global_flags(&mut args(&argv), &mut globals).unwrap();
            assert_eq!(sub, "decode");
            assert_eq!(globals.config, Some(PathBuf::from("site.toml")));
        }

        // Not a version request: handed back as the subcommand token.
        let mut globals = GlobalArgs::default();
        let sub = parse_global_flags(&mut args(&["--version=1"]), &mut globals).unwrap();
        assert_eq!(sub, "--version=1");
    }

    /// `dump` has its own loop as well.
    /// Requirements: L2-CLI-015
    #[test]
    fn dump_flags_accept_both_spellings() {
        let a = parse_dump(&mut args(&["rec.mie", "--records", "2", "--offset", "8"])).unwrap();
        let b = parse_dump(&mut args(&["rec.mie", "--records=2", "--offset=8"])).unwrap();
        assert_eq!(format!("{a:?}"), format!("{b:?}"));

        let err = parse_dump(&mut args(&["rec.mie", "--raw=true"])).unwrap_err();
        let ParseError::Other(msg) = err else {
            panic!("expected a usage error");
        };
        assert!(msg.contains("--raw=true"), "got {msg:?}");
    }

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
    /// `["31", "file.mie"]` — the stray "31" becomes a path (failing later at
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

    /// More than `MAX_MERGE_FILES` resolved inputs is a usage error (exit 4),
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
