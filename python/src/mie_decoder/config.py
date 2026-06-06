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
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from mie_decoder.models import Bus, ErrorMode, MessageType, TimestampFormat

logger = logging.getLogger(__name__)

# Conditional import for TOML support:
# - Python 3.11+ has tomllib in the standard library
# - Python 3.10 requires the tomli package
try:
    import tomllib  # type: ignore[import-not-found]
except ModuleNotFoundError:
    try:
        import tomli as tomllib  # type: ignore[no-redef]
    except ModuleNotFoundError:
        tomllib = None  # type: ignore[assignment]


#: Map of message type names (case-insensitive) to MessageType enum values.
_TYPE_NAME_MAP: dict[str, int] = {m.name.upper(): m.value for m in MessageType}

#: Map of bus name strings to Bus enum values.
_BUS_NAME_MAP: dict[str, Bus] = {"A": Bus.A, "B": Bus.B}

#: Map of timestamp format names to TimestampFormat enum values.
_TIME_FORMAT_MAP: dict[str, TimestampFormat] = {
    "auto": TimestampFormat.AUTO,
    "irig": TimestampFormat.IRIG,
    "standard": TimestampFormat.STANDARD,
}

#: Map of error mode names to ErrorMode enum values.
_ERROR_MODE_MAP: dict[str, ErrorMode] = {
    "separate": ErrorMode.SEPARATE,
    "inline": ErrorMode.INLINE,
}


@dataclass
class FilterConfig:
    """Message filtering configuration.

    All filter lists use OR logic: a message is excluded if it matches
    ANY criterion.

    Attributes:
        exclude_types: Set of MessageType values to exclude from output.
            Empty set means no type filtering.
        exclude_rts: Set of RT addresses (0–31) to exclude from output.
            Empty set means no RT filtering.
        exclude_buses: Set of Bus values to exclude from output.
            Empty set means no bus filtering.
        exclude_subaddresses: Set of subaddresses (0–31) to exclude.
            Empty set means no subaddress filtering.
    """

    exclude_types: set[int] = field(default_factory=set)
    exclude_rts: set[int] = field(default_factory=set)
    exclude_buses: set[Bus] = field(default_factory=set)
    exclude_subaddresses: set[int] = field(default_factory=set)

    @property
    def is_active(self) -> bool:
        """True if any filter criteria are configured."""
        return bool(
            self.exclude_types
            or self.exclude_rts
            or self.exclude_buses
            or self.exclude_subaddresses
        )

    def should_exclude(self, message_type: int, rt: int, bus: Bus, subaddress: int) -> bool:
        """Test whether a message should be excluded from output.

        Args:
            message_type: The message type code from the Type Word.
            rt: The Remote Terminal address from the Command Word.
            bus: The bus identifier from the Type Word.
            subaddress: The subaddress from the Command Word.

        Returns:
            True if the message matches any exclusion criterion.
        """
        if self.exclude_types and message_type in self.exclude_types:
            return True
        if self.exclude_rts and rt in self.exclude_rts:
            return True
        if self.exclude_buses and bus in self.exclude_buses:
            return True
        if self.exclude_subaddresses and subaddress in self.exclude_subaddresses:
            return True
        return False


@dataclass
class DecoderConfig:
    """Complete decoder configuration.

    Attributes:
        log_level: Logging verbosity level name.
        time_format: Timestamp format (auto/irig/standard).
        strict: If True, raise on invalid records instead of skipping.
        error_mode: How errored messages appear in output.
        filters: Message filtering configuration.
        output_format: Output format name (csv for v1.0).
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

    def with_overrides(self, **kwargs: Any) -> DecoderConfig:
        """Return a new config with specified fields overridden.

        Only non-None values in kwargs are applied.

        Args:
            **kwargs: Field names and values to override.

        Returns:
            A new DecoderConfig with the overrides applied.
        """
        new_log = kwargs.get("log_level") or self.log_level
        new_tf = kwargs.get("time_format") or self.time_format
        new_strict = kwargs.get("strict") if kwargs.get("strict") is not None else self.strict
        new_em = kwargs.get("error_mode") or self.error_mode
        new_fmt = kwargs.get("output_format") or self.output_format
        # bool overrides need an explicit None check; `or` would let a
        # config-file True be reset to False by an omitted CLI flag.
        new_nc = (
            kwargs["no_clobber"]
            if kwargs.get("no_clobber") is not None
            else self.no_clobber
        )
        new_ap = (
            kwargs["allow_partial"]
            if kwargs.get("allow_partial") is not None
            else self.allow_partial
        )

        # Merge filter overrides — CLI adds to (not replaces) config file filters
        new_filters = FilterConfig(
            exclude_types=self.filters.exclude_types | set(kwargs.get("exclude_types") or []),
            exclude_rts=self.filters.exclude_rts | set(kwargs.get("exclude_rts") or []),
            exclude_buses=self.filters.exclude_buses | set(kwargs.get("exclude_buses") or []),
            exclude_subaddresses=self.filters.exclude_subaddresses | set(kwargs.get("exclude_subaddresses") or []),
        )

        return DecoderConfig(
            log_level=new_log,
            time_format=new_tf,
            strict=new_strict,
            error_mode=new_em,
            filters=new_filters,
            output_format=new_fmt,
            no_clobber=new_nc,
            allow_partial=new_ap,
        )


def _parse_type_names(names: list[str]) -> set[int]:
    """Parse a list of message type name strings into type code values.

    Args:
        names: List of type name strings (case-insensitive). Accepts
            both enum names (e.g., ``"BC_TO_RT"``) and hex codes
            (e.g., ``"0x02"``).

    Returns:
        Set of integer message type codes.

    Raises:
        ValueError: If a name is not recognized.
    """
    result: set[int] = set()
    for name in names:
        upper = name.strip().upper()
        if upper in _TYPE_NAME_MAP:
            result.add(_TYPE_NAME_MAP[upper])
        elif upper.startswith("0X"):
            try:
                result.add(int(upper, 16))
            except ValueError:
                raise ValueError(f"Invalid hex type code: {name!r}")
        else:
            valid = ", ".join(sorted(_TYPE_NAME_MAP.keys()))
            raise ValueError(
                f"Unknown message type name: {name!r}. "
                f"Valid names: {valid}"
            )
    return result


def _parse_bus_names(names: list[str]) -> set[Bus]:
    """Parse a list of bus name strings into Bus enum values.

    Args:
        names: List of bus name strings (case-insensitive, "A" or "B").

    Returns:
        Set of Bus enum values.

    Raises:
        ValueError: If a name is not "A" or "B".
    """
    result: set[Bus] = set()
    for name in names:
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
    with open(config_path, "rb") as f:
        data = tomllib.load(f)

    # ── Parse sections ─────────────────────────────────────────────
    logging_section = data.get("logging", {})
    decode_section = data.get("decode", {})
    filter_section = data.get("filter", {})
    output_section = data.get("output", {})

    # Logging level
    log_level = logging_section.get("level", "WARNING").upper()

    # Timestamp format
    tf_str = decode_section.get("time_format", "auto").lower()
    if tf_str not in _TIME_FORMAT_MAP:
        raise ValueError(
            f"Invalid time_format: {tf_str!r}. "
            f"Valid: auto, irig, standard"
        )
    time_format = _TIME_FORMAT_MAP[tf_str]

    # Strict mode
    strict = bool(decode_section.get("strict", False))

    # Error mode
    em_str = decode_section.get("error_mode", "separate").lower()
    if em_str not in _ERROR_MODE_MAP:
        raise ValueError(
            f"Invalid error_mode: {em_str!r}. Valid: separate, inline"
        )
    error_mode = _ERROR_MODE_MAP[em_str]

    # Filters
    exclude_types = _parse_type_names(filter_section.get("exclude_types", []))
    exclude_rts = set(filter_section.get("exclude_rts", []))
    exclude_buses = _parse_bus_names(filter_section.get("exclude_buses", []))
    exclude_subaddresses = set(filter_section.get("exclude_subaddresses", []))

    filters = FilterConfig(
        exclude_types=exclude_types,
        exclude_rts=exclude_rts,
        exclude_buses=exclude_buses,
        exclude_subaddresses=exclude_subaddresses,
    )

    # Output format
    output_format = output_section.get("format", "csv")
    # L2-WRT-017: refuse to overwrite existing destination.
    no_clobber = bool(output_section.get("no_clobber", False))
    # L1-023: --allow-partial / decode.allow_partial — turns
    # unrecoverable mid-file sync loss into a `.partial` commit + exit 0
    # instead of exit 3.
    allow_partial = bool(decode_section.get("allow_partial", False))

    config = DecoderConfig(
        log_level=log_level,
        time_format=time_format,
        strict=strict,
        error_mode=error_mode,
        filters=filters,
        output_format=output_format,
        no_clobber=no_clobber,
        allow_partial=allow_partial,
    )

    logger.debug("Loaded config: %s", config)
    return config
