"""Hex dump utility for MIE binary recording files.

Provides a command-line hex dump with MIE record boundary awareness.
Records are visually separated and annotated with decoded Type Word
fields, making it easy to identify record boundaries, message types,
and byte-level content.

Usage via CLI::

    mie-decoder dump recording.mie
    mie-decoder dump recording.mie --offset 0x48 --length 256
    mie-decoder dump recording.mie --records 10
"""

from __future__ import annotations

import logging
import sys
from pathlib import Path
from typing import TextIO

from mie_decoder.decode import (
    MIN_RECORD_BYTES,
    MIN_RECORD_WORDS,
    decode_command_word,
    decode_irig_timestamp,
    decode_type_word,
    is_valid_message_type,
    read_u16,
)
from mie_decoder.exceptions import MieFileEmptyError, MieFileNotFoundError
from mie_decoder.models import MessageType

logger = logging.getLogger(__name__)


#: Map of known type codes to human-readable names.
_TYPE_NAMES: dict[int, str] = {
    MessageType.MODE_COMMAND: "Mode Command",
    MessageType.BC_TO_RT: "BC→RT (Receive)",
    MessageType.RT_TO_BC: "RT→BC (Transmit)",
    MessageType.RT_TO_RT: "RT→RT",
    MessageType.BROADCAST_BC_TO_RT: "Broadcast BC→RT",
    MessageType.BROADCAST_RT_TO_RT: "Broadcast RT→RT",
    MessageType.SPURIOUS_DATA: "Spurious Data",
}


def hex_dump_raw(
    path: str | Path,
    start_offset: int = 0,
    length: int | None = None,
    stream: TextIO | None = None,
) -> None:
    """Print a raw hex dump of a binary file.

    Args:
        path: Path to the binary file.
        start_offset: Byte offset to begin the dump.
        length: Number of bytes to dump. None means dump to end of file.
        stream: Output stream. Defaults to sys.stdout.
    """
    fpath = Path(path)
    if not fpath.exists():
        raise MieFileNotFoundError(str(fpath))
    data = fpath.read_bytes()
    if len(data) == 0:
        raise MieFileEmptyError(str(fpath))

    out = stream if stream is not None else sys.stdout

    end = len(data) if length is None else min(start_offset + length, len(data))
    chunk = data[start_offset:end]

    print(f"File: {fpath.name} ({len(data)} bytes)", file=out)
    print(f"Range: 0x{start_offset:08X}–0x{end:08X}\n", file=out)

    for i in range(0, len(chunk), 16):
        addr = start_offset + i
        hex_part = " ".join(f"{b:02X}" for b in chunk[i:i + 16])
        ascii_part = "".join(
            chr(b) if 32 <= b < 127 else "." for b in chunk[i:i + 16]
        )
        print(
            f"  {addr:08X}  {hex_part:<48s}  |{ascii_part}|",
            file=out,
        )


def hex_dump_records(
    path: str | Path,
    max_records: int | None = None,
    start_offset: int = 0,
    stream: TextIO | None = None,
) -> None:
    """Print a record-aware hex dump of an MIE binary file.

    Each record is displayed with a header showing the decoded Type Word
    fields (message type, bus, word count, error flag), timestamp, and
    command word summary.

    Args:
        path: Path to the MIE binary file.
        max_records: Maximum number of records to dump. None for all.
        start_offset: Byte offset to begin scanning for records.
        stream: Output stream. Defaults to sys.stdout.
    """
    fpath = Path(path)
    if not fpath.exists():
        raise MieFileNotFoundError(str(fpath))
    data = fpath.read_bytes()
    if len(data) == 0:
        raise MieFileEmptyError(str(fpath))

    out = stream if stream is not None else sys.stdout
    file_len = len(data)
    offset = start_offset
    record_num = 0

    print(
        f"File: {fpath.name} ({file_len} bytes)",
        file=out,
    )
    print(
        f"Record dump starting at offset 0x{start_offset:08X}\n",
        file=out,
    )

    while offset + MIN_RECORD_BYTES <= file_len:
        if max_records is not None and record_num >= max_records:
            break

        type_raw = read_u16(data, offset)
        tw = decode_type_word(type_raw)

        if tw.word_count < MIN_RECORD_WORDS:
            print(
                f"  !! Invalid word_count={tw.word_count} at 0x{offset:08X}, stopping",
                file=out,
            )
            # L2-CLI-013: surface the scan-stop anomaly through the logger
            # (subject to the configured level), in addition to the inline note.
            logger.warning(
                "dump: invalid word_count=%d at 0x%X; stopping record scan",
                tw.word_count, offset,
            )
            break

        record_bytes = tw.word_count * 2
        if offset + record_bytes > file_len:
            print(
                f"  !! Truncated record at 0x{offset:08X} "
                f"({record_bytes} bytes needed, {file_len - offset} available)",
                file=out,
            )
            logger.warning(
                "dump: truncated record at 0x%X (%d bytes needed, %d available); "
                "stopping record scan",
                offset, record_bytes, file_len - offset,
            )
            break

        # Decode header fields for annotation
        type_name = _TYPE_NAMES.get(tw.message_type, f"UNKNOWN(0x{tw.message_type:02X})")
        ts_upper = read_u16(data, offset + 2)
        ts_middle = read_u16(data, offset + 4)
        ts_lower = read_u16(data, offset + 6)
        timestamp = decode_irig_timestamp(ts_upper, ts_middle, ts_lower)
        cmd_raw = read_u16(data, offset + 8)
        cmd = decode_command_word(cmd_raw)

        # Record header
        print(
            f"{'─'*72}\n"
            f"  Record #{record_num}  @  0x{offset:08X}  "
            f"({record_bytes} bytes, {tw.word_count} words)\n"
            f"  Type: 0x{tw.raw:04X}  →  {type_name}  "
            f"Bus {'B' if tw.bus else 'A'}  "
            f"{'ERROR' if tw.error else 'OK'}\n"
            f"  Time: {timestamp.format()}"
            f"{'  [FREERUN]' if timestamp.freerun else ''}\n"
            f"  Cmd:  0x{cmd.raw:04X}  →  RT{cmd.rt} SA{cmd.subaddress} "
            f"{'T' if cmd.direction else 'R'} WC={cmd.data_word_count}",
            file=out,
        )

        # Hex dump of the full record
        record_data = data[offset:offset + record_bytes]
        for i in range(0, len(record_data), 16):
            addr = offset + i
            hex_part = " ".join(f"{b:02X}" for b in record_data[i:i + 16])
            ascii_part = "".join(
                chr(b) if 32 <= b < 127 else "." for b in record_data[i:i + 16]
            )
            print(
                f"    {addr:08X}  {hex_part:<48s}  |{ascii_part}|",
                file=out,
            )

        print(file=out)
        offset += record_bytes
        record_num += 1

    print(
        f"{'─'*72}\n{record_num} records dumped.",
        file=out,
    )
