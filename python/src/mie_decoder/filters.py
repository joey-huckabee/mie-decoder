"""Message filtering for decoded MIE messages.

Provides a generator wrapper that filters decoded messages based on
:class:`~mie_decoder.config.FilterConfig` criteria. Filtering is
applied after decoding and before CSV output, so filtered messages
do not appear in the output and are not counted.

Usage::

    from mie_decoder.config import FilterConfig
    from mie_decoder.filters import apply_filters
    from mie_decoder.reader import MieFileReader

    config = FilterConfig(exclude_types={0x20})  # drop spurious data
    reader = MieFileReader("recording.mie")
    for msg in apply_filters(reader, config):
        print(msg.timestamp.format())
"""

from __future__ import annotations

import logging
from typing import Iterable, Iterator

from mie_decoder.config import FilterConfig
from mie_decoder.models import MieMessage

logger = logging.getLogger(__name__)


def apply_filters(
    messages: Iterable[MieMessage],
    filters: FilterConfig,
) -> Iterator[MieMessage]:
    """Apply exclusion filters to a stream of decoded messages.

    This is a generator wrapper that yields only messages not matching
    any exclusion criterion. If no filters are active, all messages
    pass through with zero overhead.

    Args:
        messages: Iterable of decoded MieMessage instances (typically
            from :class:`~mie_decoder.reader.MieFileReader`).
        filters: Filter configuration specifying which messages to
            exclude.

    Yields:
        MieMessage instances that do not match any exclusion criterion.
    """
    if not filters.is_active:
        logger.debug("No filters active, passing all messages through")
        yield from messages
        return

    excluded_count = 0
    passed_count = 0

    logger.info(
        "Filtering active: exclude_types=%s exclude_rts=%s "
        "exclude_buses=%s exclude_subaddresses=%s "
        "include_types=%s include_rts=%s "
        "include_buses=%s include_subaddresses=%s",
        filters.exclude_types or "none",
        filters.exclude_rts or "none",
        filters.exclude_buses or "none",
        filters.exclude_subaddresses or "none",
        filters.include_types or "none",
        filters.include_rts or "none",
        filters.include_buses or "none",
        filters.include_subaddresses or "none",
    )

    for msg in messages:
        # SPURIOUS_DATA records carry no Command Word, so rt/subaddress are
        # None and only type/bus filters can match them (mirrors the Rust
        # filter). Guarding here also avoids an AttributeError on None.
        cw = msg.command_word
        rt = cw.rt if cw is not None else None
        subaddress = cw.subaddress if cw is not None else None
        if filters.should_exclude(
            message_type=msg.type_word.message_type,
            rt=rt,
            bus=msg.type_word.bus,
            subaddress=subaddress,
        ):
            excluded_count += 1
            logger.debug(
                "Filtered out: offset=0x%X type=0x%02X RT%s SA%s Bus %s",
                msg.file_offset,
                msg.type_word.message_type,
                rt if rt is not None else "-",
                subaddress if subaddress is not None else "-",
                msg.type_word.bus.name,
            )
            continue

        passed_count += 1
        yield msg

    logger.info(
        "Filter results: %d passed, %d excluded",
        passed_count,
        excluded_count,
    )
