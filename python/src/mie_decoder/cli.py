"""Command-line interface for MIE-Decoder.

Provides the ``mie-decoder`` CLI command for decoding DDC MIL-STD-1553
MIE binary recording files into CSV format, and for hex-dumping raw
binary content with record boundary awareness.

Configuration is loaded from an optional TOML file and merged with
CLI arguments. CLI arguments always take precedence.

Usage::

    # Decode to stdout
    mie-decoder decode recording.mie

    # Decode with config file
    mie-decoder decode recording.mie --config my-config.toml

    # Decode excluding spurious data and mode codes
    mie-decoder decode recording.mie --exclude-types SPURIOUS_DATA,MODE_COMMAND

    # Decode only RT 15 (include filter), excluding Bus B
    mie-decoder decode recording.mie --include-rts 15 --exclude-buses B

    # Hex dump
    mie-decoder dump recording.mie --records 10
"""

from __future__ import annotations

import argparse
import logging
import math
import sys
import time
from pathlib import Path
from typing import NoReturn

from mie_decoder import __version__
from mie_decoder.exceptions import (
    MieClobberRefusedError,
    MieDecoderError,
    MieFileError,
    MieHomogeneousPayloadError,
    MieInputOutputCollisionError,
    MieNoValidRecordsError,
    MieTimestampFormatMismatchError,
    MieUnrecoverableSyncLossError,
    MieWriterError,
)
from mie_decoder.logger import configure_logging

logger = logging.getLogger(__name__)

# Process exit codes — the normative contract pinned by L2-CLI-011 /
# L1-EXIT-002..006. Mirrors the Rust `cli::exit_code` module so both
# implementations return identical codes for the same condition.
EXIT_OK = 0  # complete / recovered / --allow-partial partial
EXIT_RUNTIME = 1  # runtime / decode error (I/O, writer, strict record failures)
EXIT_NO_RECORDS = 2  # input is not an MIE recording
EXIT_SYNC_LOSS = 3  # unrecoverable mid-file sync loss without --allow-partial
EXIT_USAGE = 4  # CLI usage error (bad/unknown/missing flag or argument)
EXIT_CONFIG = 5  # configuration error (missing/malformed/invalid config)


class _UsageErrorParser(argparse.ArgumentParser):
    """``ArgumentParser`` that exits with :data:`EXIT_USAGE` on a usage error.

    argparse defaults to exit code 2 for command-line usage errors, but in
    this tool exit 2 means "no valid records" (L2-CLI-011), so usage errors
    are remapped to 4 to avoid the collision and match the Rust CLI. The
    subclass propagates to subparsers automatically (argparse builds them
    with ``type(self)``).
    """

    def error(self, message: str) -> NoReturn:
        self.print_usage(sys.stderr)
        self.exit(EXIT_USAGE, f"{self.prog}: error: {message}\n")


class _CommaSeparatedAppend(argparse.Action):
    """Collect comma-separated, repeatable filter values into a flat list.

    Mirrors the Rust filter syntax (``split_csv``): each occurrence takes
    ONE value, split on commas with each token trimmed and empties
    dropped. ``--include-rts 15,31`` and
    ``--include-rts 15 --include-rts 31`` are equivalent. Tokens are
    collected as raw strings; per-filter conversion/validation happens in
    the override-building step.
    """

    def __call__(
        self,
        parser: argparse.ArgumentParser,
        namespace: argparse.Namespace,
        values: object,
        option_string: str | None = None,
    ) -> None:
        current = getattr(namespace, self.dest, None) or []
        tokens = [t.strip() for t in str(values).split(",") if t.strip()]
        setattr(namespace, self.dest, list(current) + tokens)


def _parse_u8_list(values: list[str], flag: str) -> list[int]:
    """Parse RT/subaddress filter tokens to ints, mirroring the Rust CLI.

    Each token is decimal or ``0x``-prefixed hex and must fit in a u8
    (0–255) — the same bound the Rust CLI applies (``parse_u8_value``).
    The tighter MIL-STD-1553 [0, 31] range is enforced only on the
    config-file path, not here, so the two CLIs accept the same inputs.
    """
    out: list[int] = []
    for tok in values:
        s = tok.strip()
        try:
            n = int(s, 16) if s[:2].lower() == "0x" else int(s)
        except ValueError:
            raise ValueError(f"{flag} expected integer, got {tok!r}")
        if not (0 <= n <= 255):
            raise ValueError(f"{flag} value out of range (0-255): {n}")
        out.append(n)
    return out


def build_parser() -> argparse.ArgumentParser:
    """Build the argument parser for the CLI.

    Returns:
        Configured ArgumentParser with ``decode`` and ``dump`` subcommands.
    """
    parser = _UsageErrorParser(
        prog="mie-decoder",
        description=(
            "Decode DDC MIL-STD-1553 MIE binary recording files "
            "into CSV format, or dump raw/record hex content."
        ),
    )
    parser.add_argument(
        "-v",
        "--version",
        action="version",
        version=f"%(prog)s {__version__}",
    )
    parser.add_argument(
        "--log-level",
        choices=["DEBUG", "INFO", "WARNING", "ERROR", "CRITICAL"],
        default=None,
        help="Set logging verbosity. Overrides config file.",
    )

    subparsers = parser.add_subparsers(dest="command", help="Available commands")

    # ── decode subcommand ──────────────────────────────────────────
    decode_parser = subparsers.add_parser(
        "decode",
        help="Decode MIE binary file to CSV.",
    )
    decode_parser.add_argument(
        "input",
        type=Path,
        help="Path to the MIE binary recording file.",
    )
    decode_parser.add_argument(
        "-o",
        "--output",
        type=Path,
        default=None,
        help="Output CSV file path. If omitted, writes to stdout.",
    )
    decode_parser.add_argument(
        "--config",
        type=Path,
        default=None,
        help="Path to TOML configuration file.",
    )
    decode_parser.add_argument(
        "--count",
        action="store_true",
        default=False,
        help="Print the message count to stdout (with a status summary on "
        "stderr) instead of writing CSV.",
    )
    decode_parser.add_argument(
        "--time-format",
        choices=["auto", "irig", "standard"],
        default=None,
        help="Timestamp format. Overrides config file. Default: auto.",
    )
    # Filter flags take ONE value each, comma-separable and repeatable
    # (`--exclude-rts 15,31` == `--exclude-rts 15 --exclude-rts 31`),
    # matching the Rust CLI exactly. exclude_* merge with the config file;
    # include_* are CLI-only (L3-PY-013 / L3-RS-010).
    decode_parser.add_argument(
        "--exclude-types",
        action=_CommaSeparatedAppend,
        metavar="VAL",
        default=None,
        help=(
            "Exclude message types from output. Comma-separated, repeatable. "
            "Accepts names (MODE_COMMAND, BC_TO_RT, RT_TO_BC, RT_TO_RT, "
            "BROADCAST_BC_TO_RT, BROADCAST_RT_TO_RT, SPURIOUS_DATA) "
            "or hex codes (0x01, 0x02, etc.). Merges with config file."
        ),
    )
    decode_parser.add_argument(
        "--exclude-rts",
        action=_CommaSeparatedAppend,
        metavar="VAL",
        default=None,
        help="Exclude messages by RT address. Comma-separated, repeatable. Merges with config file.",
    )
    decode_parser.add_argument(
        "--exclude-buses",
        action=_CommaSeparatedAppend,
        metavar="VAL",
        default=None,
        help="Exclude messages by bus (A, B). Comma-separated, repeatable. Merges with config file.",
    )
    decode_parser.add_argument(
        "--exclude-subaddresses",
        action=_CommaSeparatedAppend,
        metavar="VAL",
        default=None,
        help="Exclude messages by subaddress. Comma-separated, repeatable. Merges with config file.",
    )
    decode_parser.add_argument(
        "--include-types",
        action=_CommaSeparatedAppend,
        metavar="VAL",
        default=None,
        help=(
            "Include only these message types (same syntax as --exclude-types). "
            "Comma-separated, repeatable. CLI-only (no config-file key)."
        ),
    )
    decode_parser.add_argument(
        "--include-rts",
        action=_CommaSeparatedAppend,
        metavar="VAL",
        default=None,
        help="Include only these RT addresses. Comma-separated, repeatable. CLI-only.",
    )
    decode_parser.add_argument(
        "--include-buses",
        action=_CommaSeparatedAppend,
        metavar="VAL",
        default=None,
        help="Include only these buses (A, B). Comma-separated, repeatable. CLI-only.",
    )
    decode_parser.add_argument(
        "--include-subaddresses",
        action=_CommaSeparatedAppend,
        metavar="VAL",
        default=None,
        help="Include only these subaddresses. Comma-separated, repeatable. CLI-only.",
    )
    decode_parser.add_argument(
        "--error-mode",
        choices=["separate", "inline"],
        default=None,
        help=(
            "How to handle errored/spurious messages. "
            "'separate' (default): errors go to <output>_errors.csv. "
            "'inline': errors included in main CSV with ERROR/ERROR_CODE columns."
        ),
    )
    decode_parser.add_argument(
        "--no-clobber",
        action="store_true",
        default=False,
        help=(
            "Refuse to overwrite an existing output file (L2-WRT-017). "
            "Mirrors the output.no_clobber config key."
        ),
    )
    decode_parser.add_argument(
        "--allow-partial",
        action="store_true",
        default=False,
        help=(
            "On unrecoverable mid-file sync loss, write <output>.partial "
            "and exit 0 instead of exit 3 (L1-EXIT-004). Mirrors the "
            "decode.allow_partial config key."
        ),
    )
    decode_parser.add_argument(
        "--detect-records",
        type=int,
        default=None,
        metavar="N",
        help=(
            "Number of records the timestamp-format auto-detect probe "
            "walks before committing to IRIG vs Standard (range 1..=32, "
            "default 8). L2-DEC-015. Mirrors the decode.detect_records "
            "config key."
        ),
    )
    decode_parser.add_argument(
        "--lookahead-records",
        type=int,
        default=None,
        metavar="N",
        help=(
            "Total records checked by sync validation per call "
            "(1 candidate + N-1 look-ahead, range 1..=32, default 2). "
            "L2-SYN-026. Mirrors the decode.lookahead_records config "
            "key."
        ),
    )
    decode_parser.add_argument(
        "--standard-tick-rate-hz",
        type=float,
        default=None,
        metavar="HZ",
        help=(
            "Standard-counter frequency in Hz. When set, Standard "
            "timestamps are converted to microseconds and join DELTA "
            "tracking; must be > 0 (default: unset -> empty DELTA for "
            "Standard records). L2-DEC-017. Mirrors the "
            "decode.standard_tick_rate_hz config key."
        ),
    )

    # ── dump subcommand ────────────────────────────────────────────
    dump_parser = subparsers.add_parser(
        "dump",
        help="Hex dump MIE binary file with record annotations.",
    )
    dump_parser.add_argument(
        "input",
        type=Path,
        help="Path to the MIE binary file.",
    )
    dump_parser.add_argument(
        "--raw",
        action="store_true",
        default=False,
        help="Raw hex dump without record parsing.",
    )
    dump_parser.add_argument(
        "--offset",
        type=lambda x: int(x, 0),
        default=0,
        help="Start offset in bytes (supports 0x hex notation).",
    )
    dump_parser.add_argument(
        "--length",
        type=lambda x: int(x, 0),
        default=None,
        help="Number of bytes to dump (raw mode). Default: all.",
    )
    dump_parser.add_argument(
        "--records",
        type=int,
        default=None,
        help="Max number of records to dump (record mode). Default: all.",
    )
    # dump only consumes [logging] level from the TOML — the other
    # decode-time keys (time_format, filters, strict, etc.) don't
    # apply to a hex dump. Mirrors Rust where --config is a global
    # flag accepted by every subcommand.
    dump_parser.add_argument(
        "--config",
        type=Path,
        default=None,
        help="Path to TOML configuration file (only [logging] level applies to dump).",
    )

    return parser


def _apply_config_log_level(args: argparse.Namespace, config_log_level: str) -> None:
    """Apply log-level precedence: CLI > TOML > default.

    ``main()`` already configured logging with the CLI value (or the
    ``"WARNING"`` default) before the TOML config was loaded. If the
    user did not pass ``--log-level``, re-configure with the TOML
    value now. ``config_log_level`` falls back to ``"WARNING"`` when
    the file has no ``[logging]`` section, so this is a no-op in the
    common case. Mirrors ``resolve_config`` in ``src/cli.rs``.
    """
    if args.log_level is None:
        configure_logging(config_log_level)


def _run_decode(args: argparse.Namespace) -> int:
    """Execute the decode subcommand.

    Loads configuration from file (if specified), merges with CLI
    arguments, configures filtering, and runs the decode pipeline.

    Args:
        args: Parsed CLI arguments.

    Returns:
        Exit code: 0 on success, 1 on error.
    """
    from mie_decoder.config import (
        DETECT_RECORDS_MAX,
        DETECT_RECORDS_MIN,
        LOOKAHEAD_RECORDS_MAX,
        LOOKAHEAD_RECORDS_MIN,
        DecoderConfig,
        load_config,
        _parse_type_names,
        _parse_bus_names,
    )
    from mie_decoder.filters import apply_filters
    from mie_decoder.reader import MieFileReader
    from mie_decoder.writer import WriteOptions, write_csv, write_csv_split
    from mie_decoder.models import ErrorMode, TimestampFormat

    # ── Load and merge configuration ───────────────────────────────
    try:
        config = load_config(args.config)
    except (FileNotFoundError, ValueError, RuntimeError) as exc:
        print(f"Config error: {exc}", file=sys.stderr)
        return EXIT_CONFIG

    _apply_config_log_level(args, config.log_level)

    # Build CLI overrides dict (only non-None values)
    overrides: dict[str, object] = {}
    if args.time_format is not None:
        tf_map = {"auto": TimestampFormat.AUTO, "irig": TimestampFormat.IRIG, "standard": TimestampFormat.STANDARD}
        overrides["time_format"] = tf_map[args.time_format]
    if args.error_mode is not None:
        em_map = {"separate": ErrorMode.SEPARATE, "inline": ErrorMode.INLINE}
        overrides["error_mode"] = em_map[args.error_mode]
    # Filter overrides. types/buses parse via name-or-hex; rts/subaddresses
    # via the u8 (0-255) parser mirroring the Rust CLI. Any bad value is a
    # usage error (exit 4). include_* are CLI-only (L3-PY-013).
    try:
        if args.exclude_types is not None:
            overrides["exclude_types"] = _parse_type_names(args.exclude_types)
        if args.exclude_rts is not None:
            overrides["exclude_rts"] = _parse_u8_list(args.exclude_rts, "--exclude-rts")
        if args.exclude_buses is not None:
            overrides["exclude_buses"] = _parse_bus_names(args.exclude_buses)
        if args.exclude_subaddresses is not None:
            overrides["exclude_subaddresses"] = _parse_u8_list(
                args.exclude_subaddresses, "--exclude-subaddresses"
            )
        if args.include_types is not None:
            overrides["include_types"] = _parse_type_names(args.include_types)
        if args.include_rts is not None:
            overrides["include_rts"] = _parse_u8_list(args.include_rts, "--include-rts")
        if args.include_buses is not None:
            overrides["include_buses"] = _parse_bus_names(args.include_buses)
        if args.include_subaddresses is not None:
            overrides["include_subaddresses"] = _parse_u8_list(
                args.include_subaddresses, "--include-subaddresses"
            )
    except ValueError as exc:
        print(f"Error: {exc}", file=sys.stderr)
        return EXIT_USAGE
    # CLI flag flips no_clobber on; absence leaves config value intact.
    if args.no_clobber:
        overrides["no_clobber"] = True
    if args.allow_partial:
        overrides["allow_partial"] = True
    if args.detect_records is not None:
        # L2-DEC-015: validate range at parse time so an out-of-range
        # value surfaces before the config layer is even consulted.
        # The TOML form is range-checked in config.load_config.
        if not (DETECT_RECORDS_MIN <= args.detect_records <= DETECT_RECORDS_MAX):
            print(
                f"Error: invalid --detect-records: {args.detect_records}; "
                f"valid range: [{DETECT_RECORDS_MIN}, {DETECT_RECORDS_MAX}]",
                file=sys.stderr,
            )
            return EXIT_USAGE
        overrides["detect_records"] = args.detect_records
    if args.lookahead_records is not None:
        # L2-SYN-026: parse-time range check mirrors the TOML
        # load-time check in config.load_config.
        if not (LOOKAHEAD_RECORDS_MIN <= args.lookahead_records <= LOOKAHEAD_RECORDS_MAX):
            print(
                f"Error: invalid --lookahead-records: {args.lookahead_records}; "
                f"valid range: [{LOOKAHEAD_RECORDS_MIN}, {LOOKAHEAD_RECORDS_MAX}]",
                file=sys.stderr,
            )
            return EXIT_USAGE
        overrides["lookahead_records"] = args.lookahead_records
    if args.standard_tick_rate_hz is not None:
        # L2-DEC-017 / L2-CLI-012: parse-time validation mirrors the TOML
        # load-time check in config.load_config — a finite, strictly-
        # positive frequency.
        hz = args.standard_tick_rate_hz
        if not math.isfinite(hz) or hz <= 0.0:
            print(
                f"Error: invalid --standard-tick-rate-hz: {hz}; "
                f"must be a finite value greater than 0",
                file=sys.stderr,
            )
            return EXIT_USAGE
        overrides["standard_tick_rate_hz"] = hz

    config = config.with_overrides(**overrides)

    # ── Open file ──────────────────────────────────────────────────
    try:
        reader = MieFileReader(
            args.input,
            time_format=config.time_format,
            strict=config.strict,
            detect_records=config.detect_records,
            lookahead_records=config.lookahead_records,
            standard_tick_rate_hz=config.standard_tick_rate_hz,
        )
    except MieFileError as exc:
        logger.error("Failed to open input file: %s", exc)
        print(f"Error: {exc}", file=sys.stderr)
        return EXIT_RUNTIME

    logger.info("Opened %s (%d bytes)", reader.path.name, reader.file_size)

    # ── Apply filters ──────────────────────────────────────────────
    messages = apply_filters(reader, config.filters)

    # ── Execute ────────────────────────────────────────────────────
    if args.count:
        t0 = time.perf_counter()
        count = sum(1 for _ in messages)
        elapsed = time.perf_counter() - t0
        logger.info("Counted %d messages in %.3fs", count, elapsed)
        # L3-PY-010: integer count to stdout (the machine-readable data),
        # human-friendly status with path context to stderr (always
        # emitted, not gated by --log-level so an interactive operator
        # sees it without having to opt into INFO logging).
        print(count)
        print(f"counted {count} messages in {reader.path.name}", file=sys.stderr)
        return EXIT_OK

    # WriteOptions populated once with all three file-output safety
    # checks (L2-WRT-014 collision, L2-WRT-017 no-clobber, L1-EXIT-004
    # allow_partial). File-path destinations consume these; stdout
    # output ignores them.
    write_opts = WriteOptions(
        input_path=args.input,
        no_clobber=config.no_clobber,
        allow_partial=config.allow_partial,
    )

    try:
        t0 = time.perf_counter()
        if config.error_mode == ErrorMode.SEPARATE and args.output is not None:
            outcome = write_csv_split(
                messages, output=args.output, opts=write_opts,
            )
            elapsed = time.perf_counter() - t0
            logger.info(
                "Wrote %d messages + %d errors to %s in %.3fs",
                outcome.normal_count, outcome.error_count, args.output, elapsed,
            )
        else:
            # INLINE mode, or stdout (can't split stdout).
            outcome = write_csv(messages, output=args.output, opts=write_opts)
            elapsed = time.perf_counter() - t0
            dest = str(args.output) if args.output else "stdout"
            logger.info(
                "Wrote %d messages to %s in %.3fs",
                outcome.normal_count, dest, elapsed,
            )
    except (MieInputOutputCollisionError, MieClobberRefusedError) as exc:
        # File-safety preflight (L2-WRT-014/017). Generic runtime error.
        logger.error("%s", exc)
        print(f"Error: {exc}", file=sys.stderr)
        return EXIT_RUNTIME
    except MieNoValidRecordsError as exc:
        # L1-EXIT-002 → no-records.
        logger.error("%s", exc)
        print(f"Error: {exc}", file=sys.stderr)
        logger.info("decode exit class: no-records")
        return EXIT_NO_RECORDS
    except MieHomogeneousPayloadError as exc:
        # L2-SYN-018 + L1-EXIT-002: semantically a "wrong file type"
        # rejection (single-byte pad, not an MIE recording), same
        # exit-code class as NoValidRecords.
        logger.error("%s", exc)
        print(f"Error: {exc}", file=sys.stderr)
        logger.info("decode exit class: no-records")
        return EXIT_NO_RECORDS
    except MieTimestampFormatMismatchError as exc:
        # L2-DEC-016 + L1-EXIT-002: ambiguous timestamp format is
        # semantically another "wrong file type" rejection — the
        # probe could not confidently distinguish IRIG from Standard,
        # so we treat the file the same way we'd treat an
        # unrecognized stream. Same exit class (2) as NoValidRecords /
        # HomogeneousPayload. Only fires in strict mode; lenient mode
        # uses the chosen format and continues with a WARN.
        logger.error("%s", exc)
        print(f"Error: {exc}", file=sys.stderr)
        logger.info("decode exit class: no-records (timestamp-format-mismatch)")
        return EXIT_NO_RECORDS
    except MieUnrecoverableSyncLossError as exc:
        # L1-EXIT-004 → exit 3 (allow_partial would have caught this
        # inside the writer and returned a WriteOutcome instead).
        logger.error("%s", exc)
        print(f"Error: {exc}", file=sys.stderr)
        logger.info(
            "decode exit class: partial-unrecoverable (sync_losses=%d); "
            "pass --allow-partial to preserve the rows decoded so far",
            exc.sync_losses,
        )
        return EXIT_SYNC_LOSS
    except BrokenPipeError:
        # L2-WRT-018 — already handled inside the streaming writer for
        # stream destinations, but cover the edge case where it escapes.
        logger.info("decode exit class: complete (broken-pipe on stdout)")
        return EXIT_OK
    except MieWriterError as exc:
        logger.error("Write failed: %s", exc)
        print(f"Error writing output: {exc}", file=sys.stderr)
        return EXIT_RUNTIME
    except MieDecoderError as exc:
        logger.error("Decode failed: %s", exc)
        print(f"Error: {exc}", file=sys.stderr)
        return EXIT_RUNTIME

    # L1-EXIT-005 exit-class summary. Distinguish complete from
    # partial-recovered via reader.sync_losses, partial-committed
    # via outcome.partial.
    sync_losses = reader.sync_losses
    if outcome.partial is not None:
        cls = "partial-unrecoverable"
    elif sync_losses > 0:
        cls = "partial-recovered"
    else:
        cls = "complete"
    logger.info("decode exit class: %s (sync_losses=%d)", cls, sync_losses)
    return EXIT_OK


def _run_dump(args: argparse.Namespace) -> int:
    """Execute the dump subcommand.

    Args:
        args: Parsed CLI arguments.

    Returns:
        Exit code: 0 on success, 1 on error.
    """
    from mie_decoder.config import load_config
    from mie_decoder.dump import hex_dump_raw, hex_dump_records

    # dump only consumes log_level from config (time_format, strict,
    # filters, etc. don't apply to a raw / record hex dump). Load so
    # the TOML [logging] level is honored — same precedence as decode.
    # Mirrors the Rust dump path's resolve_config call.
    try:
        config = load_config(args.config)
    except (FileNotFoundError, ValueError, RuntimeError) as exc:
        print(f"Config error: {exc}", file=sys.stderr)
        return EXIT_CONFIG
    _apply_config_log_level(args, config.log_level)

    try:
        if args.raw:
            hex_dump_raw(
                args.input,
                start_offset=args.offset,
                length=args.length,
            )
        else:
            hex_dump_records(
                args.input,
                max_records=args.records,
                start_offset=args.offset,
            )
    except MieFileError as exc:
        print(f"Error: {exc}", file=sys.stderr)
        return EXIT_RUNTIME

    return EXIT_OK


def main(argv: list[str] | None = None) -> int:
    """Entry point for the MIE-Decoder CLI.

    Args:
        argv: Command-line arguments. If ``None``, uses ``sys.argv[1:]``.

    Returns:
        Process exit code per L2-CLI-011: 0 success; 1 runtime/decode
        error; 2 no valid records; 3 unrecoverable sync loss; 4 CLI usage
        error; 5 configuration error.
    """
    parser = build_parser()
    args = parser.parse_args(argv)

    # Determine log level: CLI > TOML > default. main() configures
    # with the CLI value (or the "WARNING" default) so any logging
    # in main() / parsing has a level; the subcommand runners
    # re-configure from TOML via _apply_config_log_level after the
    # config file is loaded.
    log_level = args.log_level or "WARNING"
    configure_logging(log_level)

    logger.info("MIE-Decoder v%s", __version__)
    logger.debug("Arguments: %s", args)

    if args.command == "decode":
        return _run_decode(args)
    elif args.command == "dump":
        return _run_dump(args)
    else:
        # No subcommand given — a usage error, not a runtime failure.
        parser.print_help()
        return EXIT_USAGE
