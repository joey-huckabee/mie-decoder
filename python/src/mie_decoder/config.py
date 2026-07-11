"""Configuration loading and management for MIE-Decoder.

Loads configuration from TOML files and merges with CLI arguments.
CLI arguments always take precedence over file-based configuration.

Configuration sources (in priority order, highest first):
    1. CLI arguments (``--log-level``, ``--time-format``, ``--exclude-types``, etc.)
    2. User-specified config file (``--config path/to/config.toml``)
    3. Built-in defaults (equivalent to ``config/default.toml``)

Usage::

    from mie_decoder.config import DecoderConfig, load_config

    # Load from file
    config = load_config("my-config.toml")

    # Override with CLI args
    config = config.with_overrides(log_level="DEBUG", exclude_types=["SPURIOUS_DATA"])
"""

from __future__ import annotations

import logging
import math
import re
from collections.abc import Sequence
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from mie_decoder.models import (
    Bus,
    ErrorMode,
    MessageType,
    TimestampFormat,
    parse_timestamp_format,
)

logger = logging.getLogger(__name__)

# Conditional import for TOML support:
# - Python 3.11+ has tomllib in the standard library
# - Python 3.10 requires the tomli package
try:
    import tomllib  # type: ignore[import-not-found]
except ModuleNotFoundError:
    try:
        import tomli as tomllib
    except ModuleNotFoundError:
        tomllib = None


#: Map of message type names (case-insensitive) to MessageType enum values.
_TYPE_NAME_MAP: dict[str, int] = {m.name.upper(): m.value for m in MessageType}

#: Accepted logging.level values (case-insensitive). Mirrors the Rust
#: log::Level::parse table so both implementations reject the same
#: inputs at config load time.
_VALID_LOG_LEVELS: frozenset[str] = frozenset(
    {
        "DEBUG",
        "INFO",
        "WARNING",
        "WARN",
        "ERROR",
        "CRITICAL",
        "OFF",
    }
)

#: L2-CFG-009 schema membership. Any [section] key not in this set
#: triggers an unknown-key WARN at load time.
_KNOWN_SHARED_KEYS: frozenset[tuple[str, str]] = frozenset(
    {
        ("logging", "level"),
        ("decode", "time_format"),
        ("decode", "strict"),
        ("decode", "error_mode"),
        ("decode", "allow_partial"),
        ("decode", "detect_records"),
        ("decode", "lookahead_records"),
        ("decode", "standard_tick_rate_hz"),
        ("output", "format"),
        ("output", "no_clobber"),
        ("mux", "enabled"),
        ("mux", "delimiter"),
        ("mux", "field"),
        ("merge", "collapse_duplicates"),
        ("merge", "collapse_window_us"),
        ("filter", "exclude_types"),
        ("filter", "exclude_rts"),
        ("filter", "exclude_buses"),
        ("filter", "exclude_subaddresses"),
    }
)

#: L2-DEC-015 valid range for ``decode.detect_records``. Values outside
#: this range are rejected at config-load time with a clear error.
DETECT_RECORDS_MIN: int = 1
DETECT_RECORDS_MAX: int = 32

#: L2-SYN-026 valid range for ``decode.lookahead_records``. Same shape
#: as DETECT_RECORDS_MIN/_MAX — both configurable record-count knobs
#: share their valid range for consistency.
LOOKAHEAD_RECORDS_MIN: int = 1
LOOKAHEAD_RECORDS_MAX: int = 32


def _require_bool(section: str, key: str, value: object) -> bool:
    """Validate that `value` is a real ``bool`` (not coerced from an
    int/str). Per L2-CFG-010 schema validations apply at load time.
    """
    # NOTE: ``isinstance(True, int)`` is True in Python, but
    # ``isinstance(0, bool)`` is False — the bool check is sufficient
    # here, no special-case needed.
    if not isinstance(value, bool):
        raise ValueError(
            f"Invalid [{section}] {key}: expected boolean, got {type(value).__name__} ({value!r})"
        )
    return value


def _require_table(data: dict[str, Any], section: str) -> dict[str, Any]:
    """Return the ``[section]`` table (or an empty dict if absent).

    Raises ``ValueError`` if the name is present but is not a table — e.g.
    ``decode = true`` written instead of a ``[decode]`` header. Without this the
    downstream ``.get(...)`` on a scalar leaks an ``AttributeError`` (which the
    CLI does not classify as a config error). Matches the Rust loader, which
    rejects a known section name assigned a scalar value (L2-CFG-010).
    """
    value = data.get(section, {})
    if not isinstance(value, dict):
        raise ValueError(f"Invalid [{section}]: expected a table, got {type(value).__name__}")
    return value


#: A simple identifier (section name or key): letters, digits, underscores.
_IDENT_RE = re.compile(r"^[A-Za-z0-9_]+$")

#: A numeric literal the flat schema accepts, matching the Rust
#: `is_toml_number_literal` grammar: `[+-]? (0 | [1-9][0-9]*) (.[0-9]+)?
#: ([eE][+-]?[0-9]+)?`. Rejects leading zeros (`08`, `01`), a bare trailing dot
#: (`1.`), and `0x`/`0o`/`0b` / underscore forms that `tomllib` and native Rust
#: parsing disagree on.
_NUMBER_RE = re.compile(r"^[+-]?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?$")


def _basic_string_accepted(tok: str) -> bool:
    """A double-quoted string using only the escapes the Rust parser supports
    (``\\"`` ``\\\\`` ``\\n`` ``\\t``).

    Rejects other escapes (``\\r``, ``\\uXXXX``) and an unescaped inner quote,
    mirroring `rust/src/config.rs::parse_string` exactly — `tomllib` accepts the
    full TOML escape set, so a naive regex would silently diverge from Rust.
    """
    if len(tok) < 2 or tok[0] != '"' or tok[-1] != '"':
        return False
    inner = tok[1:-1]
    i = 0
    while i < len(inner):
        if inner[i] == "\\":
            if i + 1 >= len(inner) or inner[i + 1] not in '"\\nt':
                return False
            i += 2
        elif inner[i] == '"':
            return False  # unescaped quote inside the string
        else:
            i += 1
    return True


def _scalar_accepted(tok: str) -> bool:
    """A single scalar value the flat schema accepts: a number, a boolean, or a
    basic string with the Rust-supported escapes."""
    tok = tok.strip()
    return bool(_NUMBER_RE.match(tok)) or tok in ("true", "false") or _basic_string_accepted(tok)


def _strip_toml_comment(line: str) -> str:
    """Drop a trailing ``#`` comment, preserving ``#`` inside a quoted string.

    Mirrors the Rust ``strip_comment`` so both parsers see the same value text.
    """
    in_quote = False
    prev_backslash = False
    for i, ch in enumerate(line):
        if in_quote:
            if ch == "\\" and not prev_backslash:
                prev_backslash = True
                continue
            if ch == '"' and not prev_backslash:
                in_quote = False
            prev_backslash = False
        elif ch == '"':
            in_quote = True
        elif ch == "#":
            return line[:i]
    return line


def _split_array_items(inner: str) -> list[str]:
    """Split array element text on top-level commas, respecting quoted strings.

    A backslash escapes the next character (so an escaped ``\\"`` does not close
    the string), mirroring `rust/src/config.rs::split_array_items` /
    ``push_quoted_char`` — otherwise ``["a\\", b"]`` would be mis-split on the
    comma *inside* the string and rejected where Rust accepts it.
    """
    items: list[str] = []
    buf: list[str] = []
    in_quote = False
    prev_backslash = False
    for ch in inner:
        if in_quote:
            buf.append(ch)
            if ch == "\\" and not prev_backslash:
                prev_backslash = True  # next char is escaped; stays quoted
            else:
                if ch == '"' and not prev_backslash:
                    in_quote = False
                prev_backslash = False
        elif ch == ",":
            items.append("".join(buf))
            buf = []
        else:
            if ch == '"':
                in_quote = True
            buf.append(ch)
    items.append("".join(buf))
    return [item for item in items if item.strip()]


def _value_accepted(value: str) -> bool:
    """True if ``value`` is a scalar or a single-line array of scalars the flat
    schema accepts. Rejects inline tables, multi-line arrays, date-times, and
    ``1_000`` / ``0x08`` numeric forms that ``tomllib`` would accept but the Rust
    value parser does not."""
    value = value.strip()
    if value.startswith("[") and value.endswith("]"):
        return all(_scalar_accepted(item) for item in _split_array_items(value[1:-1]))
    return _scalar_accepted(value)


def _reject_unsupported_toml_forms(text: str) -> None:
    """Reject any config line outside the flat ``[section]`` + ``key = value``
    schema, so Python's acceptance matches the minimal Rust parser exactly.

    This is a **whitelist**: a line must be blank, a comment, a flat ``[section]``
    header, or ``key = value`` with a simple-identifier key and a scalar / single-
    line-array value. ``tomllib`` is a full TOML parser that would otherwise
    *honor* forms the Rust hand-rolled parser rejects — dotted keys / headers,
    array-of-tables, inline tables, ``1_000`` / ``0x08`` numbers, date-times,
    multi-line arrays — so a config would behave differently across the two
    implementations (and a dotted/mis-typed safety option like ``no_clobber``
    could be silently ignored on Rust). Rejecting anything outside the subset up
    front keeps the two aligned by construction (exit 5, L2-CFG-010).
    """
    for lineno, raw in enumerate(text.splitlines(), start=1):
        line = _strip_toml_comment(raw).strip()
        if not line:
            continue
        if line.startswith("[["):
            raise ValueError(f"line {lineno}: array-of-tables headers ([[...]]) are not supported")
        if line.startswith("["):
            if not line.endswith("]"):
                raise ValueError(f"line {lineno}: malformed section header: {line!r}")
            header = line[1:-1].strip()
            if "." in header:
                raise ValueError(
                    f"line {lineno}: dotted section headers ([a.b]) are not "
                    "supported; use a flat [section] header"
                )
            if not _IDENT_RE.match(header):
                raise ValueError(
                    f"line {lineno}: unsupported section header [{header}]; "
                    "use a flat [section] name (letters, digits, underscore)"
                )
            continue
        if "=" not in line:
            raise ValueError(f"line {lineno}: expected 'key = value' or a [section] header")
        raw_key, value = line.split("=", 1)
        key = raw_key.strip()
        if "." in key and not key.startswith('"'):
            raise ValueError(
                f"line {lineno}: dotted keys (a.b = ...) are not supported; use a [section] header"
            )
        if not _IDENT_RE.match(key):
            raise ValueError(
                f"line {lineno}: unsupported key {key!r}; keys must be simple "
                "identifiers (letters, digits, underscore)"
            )
        if not _value_accepted(value):
            raise ValueError(
                f"line {lineno}: unsupported value for {key!r}: {value.strip()!r}; only "
                "strings, plain numbers, booleans, and single-line arrays are allowed"
            )


def _require_rt_sa_range(  # pylint: disable=redefined-outer-name
    field: str, values: object
) -> set[int]:
    """Validate a list of RT or subaddress values: each must be an int
    in [0, 31] per the L2-CFG schema reference.
    """
    if not isinstance(values, list):
        raise ValueError(f"Invalid filter.{field}: expected array, got {type(values).__name__}")
    out: set[int] = set()
    for v in values:
        if isinstance(v, bool) or not isinstance(v, int):
            raise ValueError(
                f"Invalid filter.{field} entry: expected integer, got {type(v).__name__} ({v!r})"
            )
        if not (0 <= v <= 31):
            raise ValueError(f"filter.{field} value out of MIL-STD-1553 range [0, 31]: {v}")
        out.add(v)
    return out


#: Map of bus name strings to Bus enum values.
_BUS_NAME_MAP: dict[str, Bus] = {"A": Bus.A, "B": Bus.B}

#: Map of error mode names to ErrorMode enum values.
_ERROR_MODE_MAP: dict[str, ErrorMode] = {
    "separate": ErrorMode.SEPARATE,
    "inline": ErrorMode.INLINE,
}


@dataclass
class FilterConfig:
    """Message filtering configuration.

    Both ``exclude_*`` (negative) and ``include_*`` (positive) filters
    are supported, mirroring the Rust ``FilterConfig``. A message passes
    if it matches no active ``exclude_*`` set AND every active
    ``include_*`` set contains its value. Empty (inactive) sets are
    ignored on both sides; excludes are checked first and take
    precedence.

    Attributes:
        exclude_types: MessageType values to exclude. Empty = no filter.
        exclude_rts: RT addresses (0–31) to exclude. Empty = no filter.
        exclude_buses: Bus values to exclude. Empty = no filter.
        exclude_subaddresses: subaddresses (0–31) to exclude. Empty = no
            filter.
        include_types: when non-empty, only these MessageType values pass.
        include_rts: when non-empty, only these RT addresses pass.
        include_buses: when non-empty, only these Bus values pass.
        include_subaddresses: when non-empty, only these subaddresses pass.
    """

    exclude_types: set[int] = field(default_factory=set)
    exclude_rts: set[int] = field(default_factory=set)
    exclude_buses: set[Bus] = field(default_factory=set)
    exclude_subaddresses: set[int] = field(default_factory=set)

    include_types: set[int] = field(default_factory=set)
    include_rts: set[int] = field(default_factory=set)
    include_buses: set[Bus] = field(default_factory=set)
    include_subaddresses: set[int] = field(default_factory=set)

    @property
    def is_active(self) -> bool:
        """True if any filter criteria are configured."""
        return bool(
            self.exclude_types
            or self.exclude_rts
            or self.exclude_buses
            or self.exclude_subaddresses
            or self.include_types
            or self.include_rts
            or self.include_buses
            or self.include_subaddresses
        )

    def should_exclude(
        self, message_type: int, rt: int | None, bus: Bus, subaddress: int | None
    ) -> bool:
        """Test whether a message should be excluded from output.

        Args:
            message_type: The message type code from the Type Word.
            rt: The Remote Terminal address from the Command Word, or
                ``None`` for records with no Command Word (SPURIOUS_DATA).
            bus: The bus identifier from the Type Word.
            subaddress: The subaddress from the Command Word, or ``None``
                for records with no Command Word (SPURIOUS_DATA).

        Returns:
            True if the message matches any ``exclude_*`` criterion, or
            fails any active ``include_*`` criterion. A ``None``
            ``rt``/``subaddress`` never matches an RT/subaddress exclude
            filter, and is always dropped when an RT/subaddress include
            filter is active (SPURIOUS_DATA has no RT/SA). Mirrors the
            Rust ``FilterConfig::should_exclude`` behavior.
        """
        # Negative filters take precedence over positive ones.
        return self._matches_exclude(message_type, rt, bus, subaddress) or self._fails_include(
            message_type, rt, bus, subaddress
        )

    def _matches_exclude(
        self, message_type: int, rt: int | None, bus: Bus, subaddress: int | None
    ) -> bool:
        """Whether the message matches any active ``exclude_*`` set."""
        return bool(
            (self.exclude_types and message_type in self.exclude_types)
            or (self.exclude_rts and rt in self.exclude_rts)
            or (self.exclude_buses and bus in self.exclude_buses)
            or (self.exclude_subaddresses and subaddress in self.exclude_subaddresses)
        )

    def _fails_include(
        self, message_type: int, rt: int | None, bus: Bus, subaddress: int | None
    ) -> bool:
        """Whether the message is absent from any active ``include_*`` set. A
        ``None`` rt/subaddress (SPURIOUS_DATA) fails an active RT/SA include."""
        return bool(
            (self.include_types and message_type not in self.include_types)
            or (self.include_buses and bus not in self.include_buses)
            or (self.include_rts and (rt is None or rt not in self.include_rts))
            or (
                self.include_subaddresses
                and (subaddress is None or subaddress not in self.include_subaddresses)
            )
        )


@dataclass
class DecoderConfig:
    """Complete decoder configuration.

    Attributes:
        log_level: Logging verbosity level name.
        time_format: Timestamp format (auto/irig/standard).
        strict: If True, raise on invalid records instead of skipping.
        error_mode: How errored messages appear in output.
        filters: Message filtering configuration.
        output_format: Output format name (currently only ``csv``).
        no_clobber: L2-WRT-017. Refuse to overwrite an existing
            destination. Defaults to False (overwrite permitted).
    """

    log_level: str = "WARNING"
    time_format: TimestampFormat = TimestampFormat.AUTO
    strict: bool = False
    error_mode: ErrorMode = ErrorMode.SEPARATE
    filters: FilterConfig = field(default_factory=FilterConfig)
    output_format: str = "csv"
    no_clobber: bool = False
    allow_partial: bool = False
    #: L2-DEC-015: number of records the timestamp-format auto-detect
    #: probe walks before committing to IRIG vs Standard. Range
    #: [DETECT_RECORDS_MIN, DETECT_RECORDS_MAX]. Default 8.
    detect_records: int = 8
    #: L2-SYN-026: total number of records sync.validate_record checks
    #: (1 candidate + N-1 look-ahead). Range
    #: [LOOKAHEAD_RECORDS_MIN, LOOKAHEAD_RECORDS_MAX]. Default 2,
    #: preserving the historical two-record look-ahead.
    lookahead_records: int = 2
    #: L2-DEC-017: optional Standard-counter tick rate in Hz. None (the
    #: default) keeps the historical empty-DELTA behavior for Standard
    #: records; a finite, strictly-positive value enables tick->microsecond
    #: conversion and DELTA participation. Validated at load time.
    standard_tick_rate_hz: float | None = None
    #: L2-WRT-020: populate the MUX column from a field of the input file
    #: name. Enabled by default ([mux] enabled = false / --no-mux disables
    #: it for vendor-exact output). The name is split on mux_delimiter and the
    #: mux_field-th field (0-based; negative counts from the end) becomes MUX.
    mux_enabled: bool = True
    mux_delimiter: str = "."
    mux_field: int = 4

    #: L2-MRG-007: collapse the same bus transaction witnessed by multiple
    #: recorders into one row, in a multi-file merge. Off by default (loss-free).
    #: collapse_window_us is the timestamp tolerance in microseconds (0 = exact).
    collapse_duplicates: bool = False
    collapse_window_us: int = 0

    def with_overrides(self, **kwargs: Any) -> DecoderConfig:
        """Return a new config with specified fields overridden.

        Only non-None values in kwargs are applied.

        Args:
            **kwargs: Field names and values to override.

        Returns:
            A new DecoderConfig with the overrides applied.
        """
        return DecoderConfig(
            log_level=self._override_or(kwargs, "log_level"),
            time_format=self._override_or(kwargs, "time_format"),
            strict=self._override_present(kwargs, "strict"),
            error_mode=self._override_or(kwargs, "error_mode"),
            filters=self._merge_filter_overrides(kwargs),
            output_format=self._override_or(kwargs, "output_format"),
            no_clobber=self._override_present(kwargs, "no_clobber"),
            allow_partial=self._override_present(kwargs, "allow_partial"),
            detect_records=self._override_present(kwargs, "detect_records"),
            lookahead_records=self._override_present(kwargs, "lookahead_records"),
            standard_tick_rate_hz=self._override_present(kwargs, "standard_tick_rate_hz"),
            mux_enabled=self._override_present(kwargs, "mux_enabled"),
            mux_delimiter=self._override_or(kwargs, "mux_delimiter"),
            mux_field=self._override_present(kwargs, "mux_field"),
            collapse_duplicates=self._override_present(kwargs, "collapse_duplicates"),
            collapse_window_us=self._override_present(kwargs, "collapse_window_us"),
        )

    def _override_or(self, kwargs: dict[str, Any], name: str) -> Any:
        """Override resolution for enum / non-empty-string fields: a falsy or
        absent override keeps the current value (``kwargs.get(name) or self.name``)."""
        return kwargs.get(name) or getattr(self, name)

    def _override_present(self, kwargs: dict[str, Any], name: str) -> Any:
        """Override resolution for bool / int / float fields: only an explicit
        non-``None`` override replaces the current value, so an omitted CLI flag
        never resets a config-file value (e.g. ``no_clobber = true``)."""
        value = kwargs.get(name)
        return value if value is not None else getattr(self, name)

    def _merge_filter_overrides(self, kwargs: dict[str, Any]) -> FilterConfig:
        """CLI filters ADD to (not replace) config-file filters (Rust parity);
        ``include_*`` are CLI-only but merge the same way for symmetry."""
        return FilterConfig(
            exclude_types=self.filters.exclude_types | set(kwargs.get("exclude_types") or []),
            exclude_rts=self.filters.exclude_rts | set(kwargs.get("exclude_rts") or []),
            exclude_buses=self.filters.exclude_buses | set(kwargs.get("exclude_buses") or []),
            exclude_subaddresses=(
                self.filters.exclude_subaddresses | set(kwargs.get("exclude_subaddresses") or [])
            ),
            include_types=self.filters.include_types | set(kwargs.get("include_types") or []),
            include_rts=self.filters.include_rts | set(kwargs.get("include_rts") or []),
            include_buses=self.filters.include_buses | set(kwargs.get("include_buses") or []),
            include_subaddresses=(
                self.filters.include_subaddresses | set(kwargs.get("include_subaddresses") or [])
            ),
        )


def _parse_type_names(names: Sequence[object]) -> set[int]:
    """Parse message-type identifiers into type code values.

    Each element is either a **string** (an enum name like ``"BC_TO_RT"`` or a
    hex code like ``"0x02"``) or an **integer** type code. Mirrors the Rust
    ``parse_type_value`` (``rust/src/config.rs``): integer and hex codes are bounded
    to a ``u8`` (``0..=255``); anything else is rejected with a ``ValueError``
    (so the caller maps it to a config error / usage error, never a crash).

    Args:
        names: Sequence of type identifiers (strings and/or integers). Comes
            either from a TOML array (which may mix strings and integers) or
            from the comma-separated CLI flag (always strings).

    Returns:
        Set of integer message type codes.

    Raises:
        ValueError: an unrecognized name, a code outside ``0..=255``, or an
            element that is neither a string nor an integer.
    """
    return {_parse_type_code(name) for name in names}


def _parse_type_code(name: object) -> int:
    """Parse one message-type identifier (an int code, or a string) to a u8."""
    # bool is an int subclass; a TOML boolean is not a valid type code.
    if isinstance(name, bool):
        raise ValueError(f"Invalid message type code: {name!r}")
    if isinstance(name, int):
        if not 0 <= name <= 255:
            raise ValueError(f"Type code out of range: {name}")
        return name
    if not isinstance(name, str):
        raise ValueError(
            "exclude_types/include_types entries must be strings or "
            f"integers, got {type(name).__name__}"
        )
    return _parse_type_code_str(name)


def _parse_type_code_str(name: str) -> int:
    """Parse a type-identifier string: an enum name (``BC_TO_RT``) or a ``0x`` hex code."""
    upper = name.strip().upper()
    if upper in _TYPE_NAME_MAP:
        return _TYPE_NAME_MAP[upper]
    if upper.startswith("0X"):
        try:
            code = int(upper, 16)
        except ValueError as exc:
            raise ValueError(f"Invalid hex type code: {name!r}") from exc
        if not 0 <= code <= 255:
            raise ValueError(f"Invalid hex type code: {name!r}")
        return code
    valid = ", ".join(sorted(_TYPE_NAME_MAP.keys()))
    raise ValueError(f"Unknown message type name: {name!r}. Valid names: {valid}")


def _parse_bus_names(names: Sequence[object]) -> set[Bus]:
    """Parse bus identifiers into Bus enum values.

    Each element must be a **string** (``"A"`` or ``"B"``, case-insensitive) —
    matching the Rust ``parse_bus_value`` (``rust/src/config.rs``), which rejects a
    non-string entry rather than crashing. Comes from a TOML array (which may
    contain non-strings) or the CLI flag (always strings).

    Args:
        names: Sequence of bus identifiers.

    Returns:
        Set of Bus enum values.

    Raises:
        ValueError: an entry that is not a string, or not "A"/"B".
    """
    result: set[Bus] = set()
    for name in names:
        if not isinstance(name, str):
            raise ValueError("exclude_buses entries must be strings")
        upper = name.strip().upper()
        if upper in _BUS_NAME_MAP:
            result.add(_BUS_NAME_MAP[upper])
        else:
            raise ValueError(f"Invalid bus name: {name!r}. Valid: A, B")
    return result


def load_config(path: str | Path | None = None) -> DecoderConfig:
    """Load configuration from a TOML file.

    Args:
        path: Path to the TOML configuration file. If None, returns
            the built-in defaults.

    Returns:
        A populated DecoderConfig.

    Raises:
        FileNotFoundError: If the specified config file does not exist.
        ValueError: If the config file contains invalid values.
        RuntimeError: If TOML parsing is unavailable (Python 3.10
            without the ``tomli`` package installed).
    """
    if path is None:
        logger.debug("No config file specified, using defaults")
        return DecoderConfig()

    config_path = Path(path)
    if not config_path.exists():
        raise FileNotFoundError(f"Config file not found: {config_path}")

    if tomllib is None:
        raise RuntimeError(
            "TOML parsing requires Python 3.11+ or the 'tomli' package. "
            "Install with: pip install tomli"
        )

    logger.info("Loading config from %s", config_path)
    text = config_path.read_text(encoding="utf-8")
    _reject_unsupported_toml_forms(text)
    data = tomllib.loads(text)

    # Each `[section]` is validated by its own loader (L2-CFG-010: validate at
    # load time) so no single function carries the whole schema. The loaders
    # run in the original order — all validation first, then the unknown-key
    # WARN, then assembly — so error precedence is unchanged.
    decode_section = _require_table(data, "decode")
    output_section = _require_table(data, "output")

    log_level = _load_logging_level(_require_table(data, "logging"))
    time_format = _load_time_format(decode_section)
    strict = _require_bool("decode", "strict", decode_section.get("strict", False))
    error_mode = _load_error_mode(decode_section)
    filters = _load_filter_section(_require_table(data, "filter"))
    output_format = _load_output_format(output_section)
    no_clobber = _require_bool("output", "no_clobber", output_section.get("no_clobber", False))
    allow_partial = _require_bool(
        "decode", "allow_partial", decode_section.get("allow_partial", False)
    )
    detect_records = _require_int_range(
        "decode.detect_records",
        decode_section.get("detect_records", 8),
        DETECT_RECORDS_MIN,
        DETECT_RECORDS_MAX,
    )
    lookahead_records = _require_int_range(
        "decode.lookahead_records",
        decode_section.get("lookahead_records", 2),
        LOOKAHEAD_RECORDS_MIN,
        LOOKAHEAD_RECORDS_MAX,
    )
    standard_tick_rate_hz = _load_standard_tick_rate(decode_section)
    mux_enabled, mux_delimiter, mux_field = _load_mux_section(_require_table(data, "mux"))
    collapse_duplicates, collapse_window_us = _load_merge_section(_require_table(data, "merge"))

    _warn_unknown_keys(data)

    config = DecoderConfig(
        log_level=log_level,
        time_format=time_format,
        strict=strict,
        error_mode=error_mode,
        filters=filters,
        output_format=output_format,
        no_clobber=no_clobber,
        allow_partial=allow_partial,
        detect_records=detect_records,
        lookahead_records=lookahead_records,
        standard_tick_rate_hz=standard_tick_rate_hz,
        mux_enabled=mux_enabled,
        mux_delimiter=mux_delimiter,
        mux_field=mux_field,
        collapse_duplicates=collapse_duplicates,
        collapse_window_us=collapse_window_us,
    )

    logger.debug("Loaded config: %s", config)
    return config


def _load_logging_level(logging_section: dict[str, Any]) -> str:
    """`[logging] level` — validated against the known level names."""
    log_level_raw = logging_section.get("level", "WARNING")
    if not isinstance(log_level_raw, str):
        raise ValueError(
            f"Invalid [logging] level: expected string, got {type(log_level_raw).__name__}"
        )
    log_level = log_level_raw.upper()
    if log_level not in _VALID_LOG_LEVELS:
        raise ValueError(
            f"Invalid [logging] level: {log_level_raw!r}. "
            f"Valid: DEBUG, INFO, WARNING, WARN, ERROR, CRITICAL, OFF"
        )
    return log_level


def _load_time_format(decode_section: dict[str, Any]) -> TimestampFormat:
    """`[decode] time_format`."""
    raw = decode_section.get("time_format", "auto")
    if not isinstance(raw, str):
        raise ValueError(f"Invalid decode.time_format: expected string, got {type(raw).__name__}")
    return parse_timestamp_format(raw)


def _load_error_mode(decode_section: dict[str, Any]) -> ErrorMode:
    """`[decode] error_mode`."""
    raw = decode_section.get("error_mode", "separate")
    if not isinstance(raw, str):
        raise ValueError(f"Invalid decode.error_mode: expected string, got {type(raw).__name__}")
    em_str = raw.lower()
    if em_str not in _ERROR_MODE_MAP:
        raise ValueError(f"Invalid error_mode: {em_str!r}. Valid: separate, inline")
    return _ERROR_MODE_MAP[em_str]


def _load_filter_section(filter_section: dict[str, Any]) -> FilterConfig:
    """`[filter]` exclude arrays (RT/SA values validated to [0, 31])."""
    return FilterConfig(
        exclude_types=_parse_type_names(filter_section.get("exclude_types", [])),
        exclude_rts=_require_rt_sa_range("exclude_rts", filter_section.get("exclude_rts", [])),
        exclude_buses=_parse_bus_names(filter_section.get("exclude_buses", [])),
        exclude_subaddresses=_require_rt_sa_range(
            "exclude_subaddresses", filter_section.get("exclude_subaddresses", [])
        ),
    )


def _load_output_format(output_section: dict[str, Any]) -> str:
    """`[output] format` — `csv` is currently the only supported value (L2-CFG-010)."""
    output_format: str = output_section.get("format", "csv")
    if output_format != "csv":
        raise ValueError(f"Invalid output.format: {output_format!r}. Valid: csv")
    return output_format


def _load_standard_tick_rate(decode_section: dict[str, Any]) -> float | None:
    """`[decode] standard_tick_rate_hz` (L2-DEC-017): when present, a real,
    strictly-positive frequency. Accept int or float (not bool); reject
    non-finite or non-positive values so a bad rate can't silently produce
    garbage microseconds."""
    if "standard_tick_rate_hz" not in decode_section:
        return None
    raw_hz = decode_section["standard_tick_rate_hz"]
    if isinstance(raw_hz, bool) or not isinstance(raw_hz, (int, float)):
        raise ValueError(f"Invalid decode.standard_tick_rate_hz: {raw_hz!r}; must be a number")
    hz = float(raw_hz)
    if not math.isfinite(hz) or hz <= 0.0:
        raise ValueError(
            f"Invalid decode.standard_tick_rate_hz: {hz}. Must be a finite value greater than 0"
        )
    return hz


def _load_mux_section(mux_section: dict[str, Any]) -> tuple[bool, str, int]:
    """`[mux]` MUX-from-filename configuration (L2-WRT-020).

    The caller validates the section is a table (see :func:`_require_table`).
    """
    mux_enabled = _require_bool("mux", "enabled", mux_section.get("enabled", True))
    mux_delimiter_raw = mux_section.get("delimiter", ".")
    if not isinstance(mux_delimiter_raw, str) or mux_delimiter_raw == "":
        raise ValueError(
            f"Invalid mux.delimiter: {mux_delimiter_raw!r}; must be a non-empty string"
        )
    mux_field_raw = mux_section.get("field", 4)
    if isinstance(mux_field_raw, bool) or not isinstance(mux_field_raw, int):
        raise ValueError(f"Invalid mux.field: {mux_field_raw!r}; must be an integer")
    return mux_enabled, mux_delimiter_raw, mux_field_raw


def _load_merge_section(merge_section: dict[str, Any]) -> tuple[bool, int]:
    """`[merge]` cross-recorder duplicate collapsing (L2-MRG-007).

    The caller validates the section is a table (see :func:`_require_table`).
    """
    collapse_duplicates = _require_bool(
        "merge", "collapse_duplicates", merge_section.get("collapse_duplicates", False)
    )
    collapse_window_us_raw = merge_section.get("collapse_window_us", 0)
    if (
        isinstance(collapse_window_us_raw, bool)
        or not isinstance(collapse_window_us_raw, int)
        or collapse_window_us_raw < 0
    ):
        raise ValueError(
            f"Invalid merge.collapse_window_us: {collapse_window_us_raw!r}; "
            "must be a non-negative integer"
        )
    return collapse_duplicates, collapse_window_us_raw


def _require_int_range(key: str, value: object, lo: int, hi: int) -> int:
    """Validate a TOML integer within ``[lo, hi]`` at load time (L2-CFG-010),
    rejecting bools and non-integers. ``key`` names the offending TOML key."""
    if isinstance(value, bool) or not isinstance(value, int):
        raise ValueError(f"Invalid {key}: {value!r}; must be an integer")
    if value < lo or value > hi:
        raise ValueError(f"Invalid {key}: {value}. Valid range: [{lo}, {hi}]")
    return value


def _warn_unknown_keys(data: dict[str, Any]) -> None:
    """L2-CFG-009: WARN on unknown `[section] key` entries so typos surface to
    the operator instead of being silently dropped. Non-fatal."""
    for section_name, section_dict in data.items():
        if not isinstance(section_dict, dict):
            continue
        for key in section_dict:
            if (section_name, key) not in _KNOWN_SHARED_KEYS:
                logger.warning("unknown TOML key: [%s] %s", section_name, key)
