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
    mie-decoder decode recording.mie --exclude-types SPURIOUS_DATA MODE_COMMAND

    # Decode only Bus A, excluding RT 31 (broadcast)
    mie-decoder decode recording.mie --exclude-buses B --exclude-rts 31

    # Hex dump
    mie-decoder dump recording.mie --records 10
"""

from __future__ import annotations

import argparse
import logging
import sys
import time
from pathlib import Path

from mie_decoder import __version__
from mie_decoder.exceptions import (
    MieClobberRefusedError,
    MieDecoderError,
    MieFileError,
    MieInputOutputCollisionError,
    MieWriterError,
)
from mie_decoder.logger import configure_logging

logger = logging.getLogger(__name__)


def build_parser() -> argparse.ArgumentParser:
    """Build the argument parser for the CLI.

    Returns:
        Configured ArgumentParser with ``decode`` and ``dump`` subcommands.
    """
    parser = argparse.ArgumentParser(
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
        help="Print message count to stderr instead of CSV output.",
    )
    decode_parser.add_argument(
        "--time-format",
        choices=["auto", "irig", "standard"],
        default=None,
        help="Timestamp format. Overrides config file. Default: auto.",
    )
    decode_parser.add_argument(
        "--exclude-types",
        nargs="+",
        metavar="TYPE",
        default=None,
        help=(
            "Exclude message types from output. Accepts names "
            "(MODE_COMMAND, BC_TO_RT, RT_TO_BC, RT_TO_RT, "
            "BROADCAST_BC_TO_RT, BROADCAST_RT_TO_RT, SPURIOUS_DATA) "
            "or hex codes (0x01, 0x02, etc.). Merges with config file."
        ),
    )
    decode_parser.add_argument(
        "--exclude-rts",
        nargs="+",
        type=int,
        metavar="RT",
        default=None,
        help="Exclude messages by RT address (0-31). Merges with config file.",
    )
    decode_parser.add_argument(
        "--exclude-buses",
        nargs="+",
        metavar="BUS",
        default=None,
        help="Exclude messages by bus (A, B). Merges with config file.",
    )
    decode_parser.add_argument(
        "--exclude-subaddresses",
        nargs="+",
        type=int,
        metavar="SA",
        default=None,
        help="Exclude messages by subaddress (0-31). Merges with config file.",
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

    return parser


def _run_decode(args: argparse.Namespace) -> int:
    """Execute the decode subcommand.

    Loads configuration from file (if specified), merges with CLI
    arguments, configures filtering, and runs the decode pipeline.

    Args:
        args: Parsed CLI arguments.

    Returns:
        Exit code: 0 on success, 1 on error.
    """
    from mie_decoder.config import DecoderConfig, load_config, _parse_type_names, _parse_bus_names
    from mie_decoder.filters import apply_filters
    from mie_decoder.reader import MieFileReader
    from mie_decoder.writer import WriteOptions, write_csv, write_csv_split
    from mie_decoder.models import ErrorMode, TimestampFormat

    # ── Load and merge configuration ───────────────────────────────
    try:
        config = load_config(args.config)
    except (FileNotFoundError, ValueError, RuntimeError) as exc:
        print(f"Config error: {exc}", file=sys.stderr)
        return 1

    # Build CLI overrides dict (only non-None values)
    overrides: dict = {}
    if args.time_format is not None:
        tf_map = {"auto": TimestampFormat.AUTO, "irig": TimestampFormat.IRIG, "standard": TimestampFormat.STANDARD}
        overrides["time_format"] = tf_map[args.time_format]
    if args.error_mode is not None:
        em_map = {"separate": ErrorMode.SEPARATE, "inline": ErrorMode.INLINE}
        overrides["error_mode"] = em_map[args.error_mode]
    if args.exclude_types is not None:
        try:
            overrides["exclude_types"] = _parse_type_names(args.exclude_types)
        except ValueError as exc:
            print(f"Error: {exc}", file=sys.stderr)
            return 1
    if args.exclude_rts is not None:
        overrides["exclude_rts"] = args.exclude_rts
    if args.exclude_buses is not None:
        try:
            overrides["exclude_buses"] = _parse_bus_names(args.exclude_buses)
        except ValueError as exc:
            print(f"Error: {exc}", file=sys.stderr)
            return 1
    if args.exclude_subaddresses is not None:
        overrides["exclude_subaddresses"] = args.exclude_subaddresses
    # CLI flag flips no_clobber on; absence leaves config value intact.
    if args.no_clobber:
        overrides["no_clobber"] = True

    config = config.with_overrides(**overrides)

    # ── Open file ──────────────────────────────────────────────────
    try:
        reader = MieFileReader(
            args.input,
            time_format=config.time_format,
            strict=config.strict,
        )
    except MieFileError as exc:
        logger.error("Failed to open input file: %s", exc)
        print(f"Error: {exc}", file=sys.stderr)
        return 1

    logger.info("Opened %s (%d bytes)", reader.path.name, reader.file_size)

    # ── Apply filters ──────────────────────────────────────────────
    messages = apply_filters(reader, config.filters)

    # ── Execute ────────────────────────────────────────────────────
    if args.count:
        t0 = time.perf_counter()
        count = sum(1 for _ in messages)
        elapsed = time.perf_counter() - t0
        logger.info("Counted %d messages in %.3fs", count, elapsed)
        print(f"{count} messages in {reader.path.name}", file=sys.stderr)
        return 0

    # WriteOptions populated once with both file-output safety checks
    # (L2-WRT-014 input/output collision and L2-WRT-017 no-clobber).
    # File-path destinations consume these; stdout output ignores them.
    write_opts = WriteOptions(
        input_path=args.input,
        no_clobber=config.no_clobber,
    )

    try:
        t0 = time.perf_counter()
        if config.error_mode == ErrorMode.SEPARATE and args.output is not None:
            normal_count, error_count = write_csv_split(
                messages, output=args.output, opts=write_opts,
            )
            elapsed = time.perf_counter() - t0
            logger.info(
                "Wrote %d messages + %d errors to %s in %.3fs",
                normal_count, error_count, args.output, elapsed,
            )
        else:
            # INLINE mode, or stdout (can't split stdout).
            count = write_csv(messages, output=args.output, opts=write_opts)
            elapsed = time.perf_counter() - t0
            dest = str(args.output) if args.output else "stdout"
            logger.info("Wrote %d messages to %s in %.3fs", count, dest, elapsed)
    except (MieInputOutputCollisionError, MieClobberRefusedError) as exc:
        # File-safety preflight (L2-WRT-014/017). Distinct exit code
        # (1) preserves L2-CLI-005 behavior; finer exit-class semantics
        # land with Phase 3.
        logger.error("%s", exc)
        print(f"Error: {exc}", file=sys.stderr)
        return 1
    except MieWriterError as exc:
        logger.error("Write failed: %s", exc)
        print(f"Error writing output: {exc}", file=sys.stderr)
        return 1
    except MieDecoderError as exc:
        logger.error("Decode failed: %s", exc)
        print(f"Error: {exc}", file=sys.stderr)
        return 1

    return 0


def _run_dump(args: argparse.Namespace) -> int:
    """Execute the dump subcommand.

    Args:
        args: Parsed CLI arguments.

    Returns:
        Exit code: 0 on success, 1 on error.
    """
    from mie_decoder.dump import hex_dump_raw, hex_dump_records

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
        return 1

    return 0


def main(argv: list[str] | None = None) -> int:
    """Entry point for the MIE-Decoder CLI.

    Args:
        argv: Command-line arguments. If ``None``, uses ``sys.argv[1:]``.

    Returns:
        Exit code: 0 on success, 1 on error.
    """
    parser = build_parser()
    args = parser.parse_args(argv)

    # Determine log level: CLI > config file > default
    # At this point we configure with CLI value or default;
    # config file level is applied in _run_decode after loading.
    log_level = args.log_level or "WARNING"
    configure_logging(log_level)

    logger.info("MIE-Decoder v%s", __version__)
    logger.debug("Arguments: %s", args)

    if args.command == "decode":
        # Re-configure logging if config file specifies a level
        # and CLI didn't override it
        return _run_decode(args)
    elif args.command == "dump":
        return _run_dump(args)
    else:
        parser.print_help()
        return 1
