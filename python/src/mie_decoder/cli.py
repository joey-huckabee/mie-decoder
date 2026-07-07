"""Command-line interface for MIE-Decoder.

Provides the ``mie-decoder`` CLI command for decoding DDC MIL-STD-1553
MIE binary recording files into CSV format, and for hex-dumping raw
binary content with record boundary awareness.

Configuration is loaded from an optional TOML file and merged with
CLI arguments. CLI arguments always take precedence.

Usage::

    # Decode to stdout
    mie-decoder decode recording.mie

    # Decode with config file (--config is global: before the subcommand)
    mie-decoder --config my-config.toml decode recording.mie

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
from typing import TYPE_CHECKING, NoReturn

from mie_decoder import __version__
from mie_decoder.exceptions import (
    MieClobberRefusedError,
    MieDecoderError,
    MieFileError,
    MieHomogeneousPayloadError,
    MieIncompatibleMergeInputsError,
    MieInputOutputCollisionError,
    MieNonMonotonicInputError,
    MieNoValidRecordsError,
    MieTimestampFormatMismatchError,
    MieUnrecoverableSyncLossError,
    MieWriterError,
)
from mie_decoder.logger import configure_logging

if TYPE_CHECKING:
    from collections.abc import Iterator

    from mie_decoder.config import DecoderConfig
    from mie_decoder.models import ErrorMode, MieMessage
    from mie_decoder.reader import MieFileReader
    from mie_decoder.writer import WriteOptions, WriteOutcome

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
EXIT_MERGE_INCOMPATIBLE = 6  # merge inputs cannot share an absolute timeline (L1-EXIT-009)


def _log_safe(value: object) -> str:
    """Neutralize CR/LF in user-controlled values before logging.

    A crafted input path could otherwise embed a newline and forge or inject
    additional log lines (SonarQube S5145). Escaping the control characters
    keeps each logged value on a single line without altering the visible path.
    """
    return str(value).replace("\r", "\\r").replace("\n", "\\n")


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
        except ValueError as exc:
            raise ValueError(f"{flag} expected integer, got {tok!r}") from exc
        if not (0 <= n <= 255):
            raise ValueError(f"{flag} value out of range (0-255): {n}")
        out.append(n)
    return out


def _normalize_log_level(value: str) -> str:
    """Validate ``--log-level`` case-insensitively against the shared level
    set, returning the canonical uppercase name; raises ``ValueError`` on an
    invalid value.

    Uses the same vocabulary as the config-file ``logging.level`` key and
    the Rust CLI (case-insensitive, accepting ``WARN`` and ``OFF``) rather
    than argparse ``choices`` (which is case-sensitive and would reject
    ``warn`` / ``off`` and lowercase spellings).

    This is applied in ``main()`` *after* ``parse_args`` rather than as an
    argparse ``type=`` so that ``--version`` / ``--help`` short-circuit
    before the level is validated — matching the Rust CLI, which pulls those
    flags before applying the log level (so ``--log-level bogus --version``
    still prints the version instead of failing on the bad flag).
    """
    from mie_decoder.config import _VALID_LOG_LEVELS

    normalized = value.upper()
    if normalized not in _VALID_LOG_LEVELS:
        raise ValueError(
            f"argument --log-level: invalid log level {value!r}; valid: "
            "DEBUG, INFO, WARNING, WARN, ERROR, CRITICAL, OFF (case-insensitive)"
        )
    return normalized


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
        metavar="LEVEL",
        default=None,
        help=(
            "Set logging verbosity: DEBUG, INFO, WARNING (alias WARN), ERROR, "
            "CRITICAL, or OFF (case-insensitive; CRITICAL/OFF silence all "
            "output). Overrides config file. Validated after --version/--help."
        ),
    )
    # Global option (before the subcommand), matching the Rust CLI:
    # `mie-decoder --config site.toml decode rec.mie`. Applies to every
    # subcommand (decode/count use the full config; dump uses only
    # [logging] level).
    parser.add_argument(
        "--config",
        type=Path,
        default=None,
        metavar="PATH",
        help="Path to TOML configuration file. Global (place before the subcommand).",
    )

    subparsers = parser.add_subparsers(dest="command", help="Available commands")

    # ── decode subcommand ──────────────────────────────────────────
    decode_parser = subparsers.add_parser(
        "decode",
        help="Decode MIE binary file to CSV.",
    )
    decode_parser.add_argument(
        "inputs",
        type=Path,
        nargs="*",
        metavar="INPUT",
        help=(
            "Path(s) to MIE binary recording file(s). Give more than one to "
            "merge them into a single time-sorted CSV (requires calendar-locked "
            "IRIG inputs). Mutually exclusive with --manifest / --glob."
        ),
    )
    decode_parser.add_argument(
        "--manifest",
        type=Path,
        default=None,
        metavar="PATH",
        help=(
            "Read input paths from a file (one per line; blank lines and "
            "#-comments ignored). Mutually exclusive with positionals / --glob."
        ),
    )
    decode_parser.add_argument(
        "--glob",
        dest="glob",
        default=None,
        metavar="PATTERN",
        help=(
            "Expand a single-directory glob (e.g. 'dir/*.mie'); '*' and '?' "
            "wildcards over the filename only (no recursion). Mutually "
            "exclusive with positionals / --manifest."
        ),
    )
    decode_parser.add_argument(
        "-o",
        "--output",
        type=Path,
        default=None,
        help="Output CSV file path. If omitted, writes to stdout.",
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
        help=(
            "Exclude messages by RT address. Comma-separated, repeatable. Merges with config file."
        ),
    )
    decode_parser.add_argument(
        "--exclude-buses",
        action=_CommaSeparatedAppend,
        metavar="VAL",
        default=None,
        help=(
            "Exclude messages by bus (A, B). Comma-separated, repeatable. Merges with config file."
        ),
    )
    decode_parser.add_argument(
        "--exclude-subaddresses",
        action=_CommaSeparatedAppend,
        metavar="VAL",
        default=None,
        help=(
            "Exclude messages by subaddress. Comma-separated, repeatable. Merges with config file."
        ),
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
        "--inline-errors",
        action="store_true",
        default=False,
        help=(
            "Include errored/spurious messages inline in the main CSV with "
            "the ERROR/ERROR_CODE columns populated. Default (omitted): "
            "errors go to a separate <output>_errors.csv. Stdout output "
            "always uses inline mode (you cannot split stdout)."
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
    decode_parser.add_argument(
        "--strict",
        action="store_true",
        default=None,
        help=(
            "Raise on invalid records instead of skipping them. Overrides "
            "the config file (default: lenient). Mirrors the decode.strict "
            "config key and the Rust --strict flag."
        ),
    )
    decode_parser.add_argument(
        "--format",
        default=None,
        metavar="FORMAT",
        help=(
            "Output format (csv only at present). Overrides the "
            "output.format config key, matching the Rust --format flag."
        ),
    )
    decode_parser.add_argument(
        "--no-mux",
        action="store_true",
        default=None,
        help=(
            "Leave the MUX column empty (vendor-exact output). By default MUX "
            "is derived from the input file name (L2-WRT-020). Mirrors "
            "[mux] enabled = false."
        ),
    )
    decode_parser.add_argument(
        "--mux-delimiter",
        default=None,
        metavar="D",
        help="MUX field separator (default '.'). Mirrors the mux.delimiter key.",
    )
    decode_parser.add_argument(
        "--mux-field",
        type=int,
        default=None,
        metavar="N",
        help=(
            "0-based MUX field index; negative counts from the end "
            "(default 4). Mirrors the mux.field config key."
        ),
    )
    decode_parser.add_argument(
        "--collapse-duplicates",
        action="store_true",
        default=None,
        help=(
            "Collapse the same bus transaction witnessed by multiple recorders "
            "into one row (multi-file merge only). Off by default. Mirrors "
            "[merge] collapse_duplicates = true."
        ),
    )
    decode_parser.add_argument(
        "--collapse-window-us",
        type=int,
        default=None,
        metavar="N",
        help=(
            "Timestamp tolerance in microseconds for collapsing (default 0 = "
            "exact match). Mirrors the merge.collapse_window_us config key."
        ),
    )

    # ── count subcommand ───────────────────────────────────────────
    # Its own subcommand, matching the Rust CLI (`count <INPUT>`).
    # Counts valid records after applying the config file's [filter]
    # section; CLI filter flags are decode-only. Global --config applies.
    count_parser = subparsers.add_parser(
        "count",
        help="Print the message count (no CSV).",
    )
    count_parser.add_argument(
        "input",
        type=Path,
        help="Path to the MIE binary recording file.",
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
    # dump only consumes [logging] level from the global --config; the
    # other decode-time keys (time_format, filters, strict, etc.) don't
    # apply to a hex dump.

    return parser


def _apply_config_log_level(args: argparse.Namespace, config_log_level: str) -> None:
    """Apply log-level precedence: CLI > TOML > default.

    ``main()`` already configured logging with the CLI value (or the
    ``"WARNING"`` default) before the TOML config was loaded. If the
    user did not pass ``--log-level``, re-configure with the TOML
    value now. ``config_log_level`` falls back to ``"WARNING"`` when
    the file has no ``[logging]`` section, so this is a no-op in the
    common case. Mirrors ``resolve_config`` in ``rust/src/cli.rs``.
    """
    if args.log_level is None:
        configure_logging(config_log_level)


def _resolve_decode_inputs(args: argparse.Namespace) -> list[Path]:
    """Resolve the decode input set from exactly one method (positionals,
    ``--manifest``, or ``--glob``), enforcing mutual exclusivity and the
    ``MAX_MERGE_FILES`` cap (L2-MRG-001).

    Raises:
        ValueError: usage problems (no method / combined methods / empty
            resolution / over-cap) → the caller maps to exit 4.
        OSError: a manifest that cannot be read or a glob directory that does
            not exist → the caller maps to exit 1.
    """
    from mie_decoder.merge import MAX_MERGE_FILES, expand_glob, read_manifest

    methods = sum([bool(args.inputs), args.manifest is not None, args.glob is not None])
    if methods == 0:
        raise ValueError("decode requires an input file (positional, --manifest, or --glob)")
    if methods > 1:
        raise ValueError(
            "decode accepts only one input method: positional paths, "
            "--manifest, or --glob — not a combination"
        )

    if args.manifest is not None:
        try:
            paths = read_manifest(args.manifest)
        except UnicodeDecodeError as exc:
            # A non-text manifest is a runtime input error (exit 1), matching
            # the Rust reader's read_to_string failure — not a usage error.
            raise OSError(f"manifest {args.manifest} is not valid UTF-8 text") from exc
    elif args.glob is not None:
        paths = expand_glob(args.glob)
    else:
        paths = list(args.inputs)

    if not paths:
        if args.manifest is not None:
            raise ValueError(f"manifest {args.manifest} contains no input paths")
        if args.glob is not None:
            raise ValueError(f"--glob {args.glob!r} matched no files")
        raise ValueError("decode requires at least one input file")
    if len(paths) > MAX_MERGE_FILES:
        raise ValueError(
            f"too many input files: {len(paths)} (maximum is {MAX_MERGE_FILES}); "
            f"split the set into smaller batches"
        )
    return paths


def _merge_output_collision(output: Path, inputs: list[Path]) -> str | None:
    """Return an error message if a merge's output path resolves to one of its
    inputs (L2-WRT-014 across the input set), else None. ``Path.resolve`` is
    non-strict, so a not-yet-existing output resolves fine."""
    out_resolved = output.resolve()
    for inp in inputs:
        if inp.resolve() == out_resolved:
            return (
                f"output path {output} resolves to merge input {inp}; "
                f"choose a different output path"
            )
    return None


def _validate_int_range(value: int, flag: str, lo: int, hi: int) -> int:
    """Return ``value`` if within ``[lo, hi]``, else raise ``ValueError``.

    The bounds mirror the TOML load-time checks in ``config.load_config``;
    validating the CLI value here (post-parse) keeps an out-of-range value a
    usage error (the caller maps ``ValueError`` to EXIT_USAGE) rather than
    argparse's default exit code 2.
    """
    if not (lo <= value <= hi):
        raise ValueError(f"invalid {flag}: {value}; valid range: [{lo}, {hi}]")
    return value


def _validate_positive_finite(value: float, flag: str) -> float:
    """Return ``value`` if finite and strictly positive, else raise
    ``ValueError`` (L2-DEC-017 / L2-CLI-012)."""
    if not math.isfinite(value) or value <= 0.0:
        raise ValueError(f"invalid {flag}: {value}; must be a finite value greater than 0")
    return value


def _validate_nonempty(value: str, flag: str) -> str:
    """Return ``value`` if non-empty, else raise ``ValueError``."""
    if value == "":
        raise ValueError(f"invalid {flag}: must be a non-empty string")
    return value


def _simple_overrides(args: argparse.Namespace) -> dict[str, object]:
    """Build the passthrough CLI overrides that need no validation.

    A boolean flag flips a value on; its absence leaves the config value intact
    (there is no "off" form on the CLI). ``--inline-errors`` flips error_mode to
    INLINE; the default IS separate.
    """
    from mie_decoder.models import ErrorMode, TimestampFormat

    overrides: dict[str, object] = {}
    if args.time_format is not None:
        tf_map = {
            "auto": TimestampFormat.AUTO,
            "irig": TimestampFormat.IRIG,
            "standard": TimestampFormat.STANDARD,
        }
        overrides["time_format"] = tf_map[args.time_format]
    if args.inline_errors:
        overrides["error_mode"] = ErrorMode.INLINE
    if args.no_clobber:
        overrides["no_clobber"] = True
    if args.allow_partial:
        overrides["allow_partial"] = True
    if args.strict is not None:
        overrides["strict"] = args.strict
    if args.format is not None:
        overrides["output_format"] = args.format
    if args.no_mux:
        overrides["mux_enabled"] = False
    if args.mux_field is not None:
        overrides["mux_field"] = args.mux_field
    if args.collapse_duplicates:
        overrides["collapse_duplicates"] = True
    return overrides


def _filter_overrides(args: argparse.Namespace) -> dict[str, object]:
    """Parse the ``--exclude-*`` / ``--include-*`` filter values.

    types/buses parse via name-or-hex; rts/subaddresses via the u8 (0-255)
    parser mirroring the Rust CLI. Any bad value raises ``ValueError`` (the
    caller maps it to EXIT_USAGE). include_* are CLI-only (L3-PY-013).
    """
    from mie_decoder.config import _parse_bus_names, _parse_type_names

    overrides: dict[str, object] = {}
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
    return overrides


def _validated_numeric_overrides(args: argparse.Namespace) -> dict[str, object]:
    """Build overrides for the numeric/string args that carry range/format
    checks, raising ``ValueError`` (usage) on an invalid value. The bounds
    mirror the TOML load-time checks (L2-DEC-015 / L2-SYN-026 / L2-DEC-017 /
    L2-CLI-012)."""
    from mie_decoder.config import (
        DETECT_RECORDS_MAX,
        DETECT_RECORDS_MIN,
        LOOKAHEAD_RECORDS_MAX,
        LOOKAHEAD_RECORDS_MIN,
    )

    overrides: dict[str, object] = {}
    if args.detect_records is not None:
        overrides["detect_records"] = _validate_int_range(
            args.detect_records,
            "--detect-records",
            DETECT_RECORDS_MIN,
            DETECT_RECORDS_MAX,
        )
    if args.lookahead_records is not None:
        overrides["lookahead_records"] = _validate_int_range(
            args.lookahead_records,
            "--lookahead-records",
            LOOKAHEAD_RECORDS_MIN,
            LOOKAHEAD_RECORDS_MAX,
        )
    if args.standard_tick_rate_hz is not None:
        overrides["standard_tick_rate_hz"] = _validate_positive_finite(
            args.standard_tick_rate_hz, "--standard-tick-rate-hz"
        )
    if args.mux_delimiter is not None:
        overrides["mux_delimiter"] = _validate_nonempty(args.mux_delimiter, "--mux-delimiter")
    if args.collapse_window_us is not None:
        if args.collapse_window_us < 0:
            raise ValueError("--collapse-window-us must be a non-negative integer")
        overrides["collapse_window_us"] = args.collapse_window_us
    return overrides


def _build_decode_overrides(args: argparse.Namespace) -> dict[str, object]:
    """Assemble all CLI → config overrides, raising ``ValueError`` (which the
    caller maps to EXIT_USAGE) on any invalid value."""
    overrides: dict[str, object] = {}
    overrides.update(_simple_overrides(args))
    overrides.update(_filter_overrides(args))
    overrides.update(_validated_numeric_overrides(args))
    return overrides


def _open_reader(path: Path, config: DecoderConfig) -> MieFileReader:
    """Open one input file with reader options from ``config`` (mirrors
    ``open_reader`` in ``rust/src/cli.rs``). Raises ``MieFileError`` on a
    file/open failure (the caller maps it to EXIT_RUNTIME).
    """
    from mie_decoder.reader import MieFileReader

    return MieFileReader(
        path,
        time_format=config.time_format,
        strict=config.strict,
        detect_records=config.detect_records,
        lookahead_records=config.lookahead_records,
        standard_tick_rate_hz=config.standard_tick_rate_hz,
        mux_enabled=config.mux_enabled,
        mux_delimiter=config.mux_delimiter,
        mux_field=config.mux_field,
    )


def _check_merge_output_collision(
    args: argparse.Namespace,
    input_paths: list[Path],
    readers: list[MieFileReader],
) -> int | None:
    """For a merge (>1 input) writing to a file, reject an output path that
    resolves to one of the inputs (L2-WRT-014 across the set); a single input
    uses the writer's own input/output check. Returns an exit code to
    short-circuit on, or ``None`` to continue.
    """
    if len(readers) > 1 and args.output is not None:
        collision = _merge_output_collision(args.output, input_paths)
        if collision is not None:
            logger.error("%s", collision)
            print(f"Error: {collision}", file=sys.stderr)
            return EXIT_RUNTIME
    return None


def _build_message_stream(
    readers: list[MieFileReader],
    config: DecoderConfig,
    *,
    merge_requested: bool = False,
    open_dropped: bool = False,
) -> Iterator[MieMessage]:
    """Build the decoded-message stream: a single filtered reader, or the
    time-sorted k-way merge of several (global DELTA, L2-MRG-002/005).

    ``merge_requested`` routes by the *requested* input count (not the surviving
    reader count) so an --allow-partial merge that dropped an input at open time
    still uses the merge path. ``open_dropped`` appends a terminal after the good
    rows so the writer commits a `.partial` (L2-MRG-004). ``merge_readers``
    validates the input set eagerly, so an incompatible set raises
    ``MieIncompatibleMergeInputsError`` here before any output.
    """
    from mie_decoder.filters import apply_filters
    from mie_decoder.merge import merge_readers

    if not merge_requested:
        return apply_filters(readers[0], config.filters)
    merged = merge_readers(
        readers,
        standard_tick_rate_hz=config.standard_tick_rate_hz,
        allow_partial=config.allow_partial,
        strict=config.strict,
        collapse_duplicates=config.collapse_duplicates,
        collapse_window_us=config.collapse_window_us,
    )
    stream = apply_filters(merged, config.filters)
    if open_dropped:
        return _append_open_terminal(stream)
    return stream


def _append_open_terminal(stream: Iterator[MieMessage]) -> Iterator[MieMessage]:
    """Yield from ``stream`` then raise an unrecoverable-sync-loss terminal so the
    writer commits a `.partial` (L2-MRG-004). Used when a merge dropped an input
    at open time — that file contributed nothing (truncated at offset 0)."""
    yield from stream
    raise MieUnrecoverableSyncLossError(0, 0)


def _write_messages(
    messages: Iterator[MieMessage],
    output: Path | None,
    error_mode: ErrorMode,
    write_opts: WriteOptions,
) -> WriteOutcome:
    """Write the stream and log the outcome (mirrors ``write_messages`` in
    ``rust/src/cli.rs``). Separate mode with a file output writes the split
    CSVs; INLINE mode (or stdout, which cannot be split) writes one CSV."""
    from mie_decoder.models import ErrorMode
    from mie_decoder.writer import write_csv, write_csv_split

    t0 = time.perf_counter()
    if error_mode == ErrorMode.SEPARATE and output is not None:
        outcome = write_csv_split(messages, output=output, opts=write_opts)
        elapsed = time.perf_counter() - t0
        logger.info(
            "Wrote %d messages + %d errors to %s in %.3fs",
            outcome.normal_count,
            outcome.error_count,
            output,
            elapsed,
        )
    else:
        # INLINE mode, or stdout (can't split stdout).
        outcome = write_csv(messages, output=output, opts=write_opts)
        elapsed = time.perf_counter() - t0
        dest = str(output) if output else "stdout"
        logger.info(
            "Wrote %d messages to %s in %.3fs",
            outcome.normal_count,
            dest,
            elapsed,
        )
    return outcome


def _classify_decode_error(exc: Exception) -> int:
    """Map a decode-time exception to its exit code, emitting the same stderr
    and exit-class log lines as before. Mirrors the error arms of
    ``classify_decode_exit`` in ``rust/src/cli.rs``. Order matters: specific
    types precede the ``MieDecoderError`` base.
    """
    if isinstance(exc, MieIncompatibleMergeInputsError):
        # L1-EXIT-009 / L2-MRG-003: inputs cannot share an absolute timeline.
        logger.error("%s", exc)
        print(f"Error: {exc}", file=sys.stderr)
        logger.info("decode exit class: merge-incompatible")
        return EXIT_MERGE_INCOMPATIBLE
    if isinstance(exc, (MieInputOutputCollisionError, MieClobberRefusedError)):
        # File-safety preflight (L2-WRT-014/017). Generic runtime error.
        logger.error("%s", exc)
        print(f"Error: {exc}", file=sys.stderr)
        return EXIT_RUNTIME
    if isinstance(exc, MieNoValidRecordsError):
        # L1-EXIT-002 → no-records.
        logger.error("%s", exc)
        print(f"Error: {exc}", file=sys.stderr)
        logger.info("decode exit class: no-records")
        return EXIT_NO_RECORDS
    if isinstance(exc, MieHomogeneousPayloadError):
        # L2-SYN-018 + L1-EXIT-002: a single-byte pad, not an MIE recording —
        # same exit class as NoValidRecords.
        logger.error("%s", exc)
        print(f"Error: {exc}", file=sys.stderr)
        logger.info("decode exit class: no-records")
        return EXIT_NO_RECORDS
    if isinstance(exc, MieTimestampFormatMismatchError):
        # L2-DEC-016 + L1-EXIT-002: ambiguous IRIG-vs-Standard probe — another
        # "wrong file type" rejection. Only fires in strict mode.
        logger.error("%s", exc)
        print(f"Error: {exc}", file=sys.stderr)
        logger.info("decode exit class: no-records (timestamp-format-mismatch)")
        return EXIT_NO_RECORDS
    if isinstance(exc, MieUnrecoverableSyncLossError):
        # L1-EXIT-004 → exit 3 (allow_partial would have been caught inside the
        # writer and returned a WriteOutcome instead).
        logger.error("%s", exc)
        print(f"Error: {exc}", file=sys.stderr)
        logger.info(
            "decode exit class: partial-unrecoverable (sync_losses=%d); "
            "pass --allow-partial to preserve the rows decoded so far",
            exc.sync_losses,
        )
        return EXIT_SYNC_LOSS
    if isinstance(exc, BrokenPipeError):
        # L2-WRT-018 — usually handled inside the streaming writer; cover the
        # edge case where it escapes.
        logger.info("decode exit class: complete (broken-pipe on stdout)")
        return EXIT_OK
    if isinstance(exc, MieWriterError):
        logger.error("Write failed: %s", exc)
        print(f"Error writing output: {exc}", file=sys.stderr)
        return EXIT_RUNTIME
    if isinstance(exc, MieNonMonotonicInputError):
        # L2-MRG-006: a strict-mode merge hit an input that is not internally
        # time-sorted. Record-error class (exit 1).
        logger.error("%s", exc)
        print(f"Error: {exc}", file=sys.stderr)
        logger.info("decode exit class: non-monotonic-input (strict)")
        return EXIT_RUNTIME
    # Any remaining MieDecoderError (record errors, generic file errors).
    logger.error("Decode failed: %s", exc)
    print(f"Error: {exc}", file=sys.stderr)
    return EXIT_RUNTIME


def _classify_decode_success(outcome: WriteOutcome, readers: list[MieFileReader]) -> int:
    """Emit the L1-EXIT-005 exit-class summary for a successful run and return
    EXIT_OK (mirrors the ``Ok`` arm of Rust ``classify_decode_exit``)."""
    sync_losses = sum(r.sync_losses for r in readers)
    # L1-EXIT-010: report the empty-recording class only when *every* opened
    # input was a valid empty recording (so a merge that also drew rows from a
    # non-empty input stays "complete"). The writer has already produced a
    # header-only CSV.
    empty_recording = bool(readers) and all(r.empty_recording for r in readers)
    if outcome.partial is not None:
        cls = "partial-unrecoverable"
    elif empty_recording and outcome.normal_count == 0 and outcome.error_count == 0:
        cls = "empty-recording"
    elif sync_losses > 0:
        cls = "partial-recovered"
    else:
        cls = "complete"
    logger.info("decode exit class: %s (sync_losses=%d)", cls, sync_losses)
    return EXIT_OK


def _run_decode(args: argparse.Namespace) -> int:
    """Execute the decode subcommand.

    Loads configuration from file (if specified), merges with CLI
    arguments, configures filtering, and runs the decode pipeline.

    Args:
        args: Parsed CLI arguments.

    Returns:
        Exit code: 0 on success, 1 on error.
    """
    from mie_decoder.config import load_config

    # ── Load and merge configuration ───────────────────────────────
    try:
        config = load_config(args.config)
    except (FileNotFoundError, ValueError, RuntimeError) as exc:
        print(f"Config error: {exc}", file=sys.stderr)
        return EXIT_CONFIG

    _apply_config_log_level(args, config.log_level)

    try:
        overrides = _build_decode_overrides(args)
    except ValueError as exc:
        print(f"Error: {exc}", file=sys.stderr)
        return EXIT_USAGE

    config = config.with_overrides(**overrides)

    # L2-CFG-010 mirror of the Rust runtime check (rust/src/cli.rs): a config-file
    # output.format is validated at load time (exit 5), but a --format override
    # is applied after load, so re-check here. Non-csv is a runtime error
    # (exit 1), matching the Rust CLI.
    if config.output_format != "csv":
        print(
            f"Error: output format {config.output_format!r} not yet supported (only 'csv')",
            file=sys.stderr,
        )
        return EXIT_RUNTIME

    # ── Resolve the input set (positionals / --manifest / --glob) ──
    try:
        input_paths = _resolve_decode_inputs(args)
    except ValueError as exc:
        # Usage problems: no method, combined methods, empty resolution,
        # over-cap (L2-MRG-001).
        print(f"Error: {exc}", file=sys.stderr)
        return EXIT_USAGE
    except OSError as exc:
        # Manifest unreadable / glob directory missing.
        print(f"Error: {exc}", file=sys.stderr)
        return EXIT_RUNTIME

    # ── Open a reader per input ────────────────────────────────────
    # Under --allow-partial a *merge* tolerates a per-file OPEN failure (an
    # empty / unreadable / missing input): it drops that input with a WARN and
    # commits the batch as `.partial` (L2-MRG-004), mirroring a priming-time or
    # mid-file failure. A single-input decode is unaffected.
    merge_requested = len(input_paths) > 1
    readers: list[MieFileReader] = []
    open_dropped = False
    for p in input_paths:
        try:
            readers.append(_open_reader(p, config))
        except MieFileError as exc:
            if merge_requested and config.allow_partial:
                logger.warning(
                    "merge: input %s could not be opened; truncating it from the "
                    "merge (--allow-partial): %s",
                    _log_safe(p),
                    _log_safe(exc),
                )
                open_dropped = True
            else:
                logger.error("Failed to open input file: %s", exc)
                print(f"Error: {exc}", file=sys.stderr)
                return EXIT_RUNTIME

    for r in readers:
        logger.info("Opened %s (%d bytes)", r.path.name, r.file_size)

    collision_code = _check_merge_output_collision(args, input_paths, readers)
    if collision_code is not None:
        return collision_code

    from mie_decoder.writer import WriteOptions

    write_opts = WriteOptions(
        input_path=None if merge_requested else input_paths[0],
        no_clobber=config.no_clobber,
        allow_partial=config.allow_partial,
    )

    # ── Build the message stream and write it ──────────────────────
    # One input → the single-file path (per-file DELTA); two or more → the
    # time-sorted k-way merge (global DELTA, L2-MRG-002/005), which validates
    # eagerly. Build- and write-time decode failures (and a broken pipe on
    # stdout) map to exit codes via _classify_decode_error; a clean run is
    # classified by the cumulative sync-loss count (L1-EXIT-005).
    try:
        messages = _build_message_stream(
            readers, config, merge_requested=merge_requested, open_dropped=open_dropped
        )
        outcome = _write_messages(messages, args.output, config.error_mode, write_opts)
    except (MieDecoderError, BrokenPipeError) as exc:
        return _classify_decode_error(exc)

    return _classify_decode_success(outcome, readers)


def _run_count(args: argparse.Namespace) -> int:
    """Execute the count subcommand (L3-PY-010).

    Counts valid records after applying the config file's ``[filter]``
    section (CLI filter flags are decode-only), printing the integer
    count to stdout and a human-readable status line to stderr. Mirrors
    the Rust ``count`` subcommand.

    Returns:
        Exit code: 0 on success, 1 on a decode error, 5 on config error.
    """
    from mie_decoder.config import load_config
    from mie_decoder.filters import apply_filters
    from mie_decoder.reader import MieFileReader

    try:
        config = load_config(args.config)
    except (FileNotFoundError, ValueError, RuntimeError) as exc:
        print(f"Config error: {exc}", file=sys.stderr)
        return EXIT_CONFIG

    _apply_config_log_level(args, config.log_level)

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

    messages = apply_filters(reader, config.filters)
    try:
        t0 = time.perf_counter()
        count = sum(1 for _ in messages)
        elapsed = time.perf_counter() - t0
    except (
        MieNoValidRecordsError,
        MieHomogeneousPayloadError,
        MieTimestampFormatMismatchError,
    ) as exc:
        # Align with decode (L2-CLI-011): a wrong-file rejection maps to exit 2,
        # not the generic runtime exit 1. (An empty recording raises nothing —
        # the iterator simply yields zero records — so count prints 0 and exits
        # 0 per L1-EXIT-010.)
        logger.error("Count failed: %s", exc)
        print(f"Error: {exc}", file=sys.stderr)
        return EXIT_NO_RECORDS
    except MieDecoderError as exc:
        # Any other decode error during the count maps to a runtime failure
        # (exit 1), matching the Rust count subcommand.
        logger.error("Count failed: %s", exc)
        print(f"Error: {exc}", file=sys.stderr)
        return EXIT_RUNTIME

    logger.info("Counted %d messages in %.3fs", count, elapsed)
    # L3-PY-010: integer count to stdout (the machine-readable datum),
    # human-friendly status with path context to stderr (always emitted,
    # not gated by --log-level so an interactive operator sees context).
    print(count)
    if reader.empty_recording:
        print(
            f"no records in {reader.path.name} (empty recording — opens on "
            f"the end-of-records terminator)",
            file=sys.stderr,
        )
    else:
        print(f"counted {count} messages in {reader.path.name}", file=sys.stderr)
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

    # Validate --log-level here (not via an argparse `type=`) so --version
    # and --help short-circuit during parse_args before the level is
    # checked, matching the Rust CLI. An invalid value is a usage error (4).
    if args.log_level is not None:
        try:
            args.log_level = _normalize_log_level(args.log_level)
        except ValueError as exc:
            print(f"{parser.prog}: error: {exc}", file=sys.stderr)
            return EXIT_USAGE

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
    if args.command == "count":
        return _run_count(args)
    if args.command == "dump":
        return _run_dump(args)
    # No subcommand given — a usage error, not a runtime failure.
    parser.print_help()
    return EXIT_USAGE
