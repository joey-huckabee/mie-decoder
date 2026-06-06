"""CSV output writer for decoded MIE messages using pandas.

Produces CSV output matching the column layout used by DDC's recording
software, enabling direct comparison between MIE-Decoder output and
vendor-generated CSV files.

Output Column Definitions:

    TIME_STAMP
        IRIG-format timestamp of the first word of this message on the
        1553 bus, formatted as ``DAY:HH:MM:SS.uuuuuu``. The DAY field
        is the day-of-year (1–366). Hours, minutes, and seconds are
        zero-padded to two digits. Microseconds are zero-padded to six
        digits, giving microsecond-level resolution.

    RT
        Remote Terminal address (0–30). Identifies which RT participated
        in this bus transaction. Address 31 is reserved for broadcast.

    MSG
        Message identifier combining the subaddress and transfer
        direction in the format ``<Subaddress><T|R>``. For example,
        ``11R`` means Subaddress 11, Receive (BC→RT); ``22T`` means
        Subaddress 22, Transmit (RT→BC). Subaddresses 0 and 31 denote
        mode code messages per MIL-STD-1553B.

    WD01 through WD32
        Raw 16-bit data words in uppercase hexadecimal (e.g., ``0400``,
        ``CA22``). Words are in bus wire order. Columns beyond the
        actual data word count for this message are empty strings.
        The maximum is 32 data words per MIL-STD-1553B.

    STAT
        Raw 16-bit MIL-STD-1553 Status Word in uppercase hexadecimal.
        Returned by the RT to indicate message acceptance, busy status,
        subsystem flag, etc. Bits 15–11 echo the RT address.

    CMD
        Raw 16-bit MIL-STD-1553 Command Word in uppercase hexadecimal.
        Sent by the Bus Controller to initiate the transaction. Contains
        the RT address, T/R bit, subaddress, and word count.

    MUX
        Multiplexer label or subchannel identifier. Derived from
        external configuration (TMATS or recording software setup).
        Not decoded from the binary record. Empty in v1.0.

    TERM_NAME
        Terminal or equipment name associated with the RT/SA combination.
        Derived from external configuration. Empty in v1.0.

    BUS
        Redundant bus identifier: ``A`` or ``B``. MIL-STD-1553 defines
        two redundant buses for fault tolerance; this field indicates
        which bus the message was captured on.

    DELTA
        Inter-arrival time in seconds (six decimal places) between this
        message and the most recent prior message sharing the same
        Remote Terminal address (RT) and message identifier (MSG).

        The MSG identifier is the combination of Subaddress and Direction
        (e.g., ``11T`` for Subaddress 11, Transmit; ``22R`` for
        Subaddress 22, Receive). Messages are grouped by the composite
        key ``<RT>:<MSG>`` — for example, all messages to RT 15 SA 11
        Receive are tracked independently from RT 15 SA 11 Transmit,
        and independently from RT 30 SA 11 Receive.

        For the first occurrence of any RT/MSG combination in a
        recording file, DELTA is ``0.000000``.

        This metric directly reveals the Bus Controller's scheduling
        rate for each unique message type. A consistent DELTA of
        approximately 0.016 seconds indicates the BC is polling that
        message at a 60 Hz minor frame rate. A consistent DELTA of
        approximately 0.033 seconds indicates a 30 Hz rate. Jitter or
        drift in DELTA values across a recording can indicate bus
        loading anomalies, missed scheduling cycles, BC priority
        changes, or intermittent RT response failures.

    IM_GAP
        Inter-message gap. Not decoded from the binary record in v1.0.
        Empty string.

    RCV_GAP
        Receive gap. Not decoded from the binary record in v1.0.
        Empty string.

    XMT_GAP
        Transmit gap. Not decoded from the binary record in v1.0.
        Empty string.
"""

from __future__ import annotations

import logging
import sys
from pathlib import Path
from typing import Iterable, TextIO

import pandas as pd

from mie_decoder.exceptions import MieWriterError
from mie_decoder.models import MieMessage

logger = logging.getLogger(__name__)

#: Maximum number of data word columns in the CSV output.
MAX_DATA_WORDS: int = 32

#: CSV column definitions in output order. Each entry is (column_name, description).
#: The canonical column order is defined here and used by all output functions.
CSV_COLUMNS: list[tuple[str, str]] = [
    ("TIME_STAMP", "IRIG timestamp DAY:HH:MM:SS.uuuuuu"),
    ("RT", "Remote Terminal address 0-30"),
    ("MSG", "Message identifier: <Subaddress><T|R>"),
    *[(f"WD{i:02d}", f"Data word {i} (hex)") for i in range(1, MAX_DATA_WORDS + 1)],
    ("STAT", "MIL-STD-1553 Status Word (hex)"),
    ("CMD", "MIL-STD-1553 Command Word (hex)"),
    ("MUX", "Multiplexer label (external config, empty in v1.0)"),
    ("TERM_NAME", "Terminal name (external config, empty in v1.0)"),
    ("BUS", "Bus identifier: A or B"),
    ("DELTA", "Seconds since prior message with same RT+MSG"),
    ("ERROR", "Error label: empty=normal, ERROR=bit14, SPURIOUS=type 0x20"),
    ("ERROR_CODE", "DDC error code (0x01xx) or decoder code (0x20xx)"),
    ("IM_GAP", "Inter-message gap (empty in v1.0)"),
    ("RCV_GAP", "Receive gap (empty in v1.0)"),
    ("XMT_GAP", "Transmit gap (empty in v1.0)"),
]

#: Ordered list of column names for CSV header row.
CSV_HEADER: list[str] = [name for name, _ in CSV_COLUMNS]


def message_to_row(msg: MieMessage) -> dict[str, str]:
    """Convert a single decoded message to a dict of CSV field strings.

    Args:
        msg: A fully decoded MieMessage instance.

    Returns:
        A dict keyed by column name (matching :data:`CSV_HEADER`) with
        string values. Handles all message types including errored
        records and SPURIOUS_DATA (where command_word is None).
    """
    row: dict[str, str] = {
        "TIME_STAMP": msg.timestamp.format(),
        "RT": str(msg.rt) if msg.rt is not None else "",
        "MSG": msg.msg_label,
        "STAT": f"{msg.status_word:04X}" if msg.status_word is not None else "",
        "CMD": f"{msg.command_word.raw:04X}" if msg.command_word is not None else "",
        "MUX": "",
        "TERM_NAME": "",
        "BUS": msg.bus.name,
        "DELTA": f"{msg.delta:.6f}" if msg.delta is not None else "",
        "ERROR": msg.error_label,
        "ERROR_CODE": f"{msg.error_word:04X}" if msg.error_word is not None else "",
        "IM_GAP": "",
        "RCV_GAP": "",
        "XMT_GAP": "",
    }

    for i in range(1, MAX_DATA_WORDS + 1):
        col = f"WD{i:02d}"
        idx = i - 1
        if idx < len(msg.data_words):
            row[col] = f"{msg.data_words[idx]:04X}"
        else:
            row[col] = ""

    return row


def messages_to_dataframe(messages: Iterable[MieMessage]) -> pd.DataFrame:
    """Convert an iterable of decoded messages to a pandas DataFrame.

    Builds a DataFrame with columns matching :data:`CSV_HEADER` and
    one row per message. All values are strings to preserve hex
    formatting on output.

    Args:
        messages: Iterable of decoded MieMessage instances.

    Returns:
        A DataFrame with string-typed columns in CSV_HEADER order.
    """
    rows: list[dict[str, str]] = []
    for i, msg in enumerate(messages):
        rows.append(message_to_row(msg))
        if (i + 1) % 10_000 == 0:
            logger.debug("Converted %d messages to rows", i + 1)

    logger.debug("Building DataFrame from %d rows", len(rows))
    df = pd.DataFrame(rows, columns=CSV_HEADER)
    return df


def dataframe_to_csv(
    df: pd.DataFrame,
    output: str | Path | TextIO | None = None,
) -> None:
    """Write a DataFrame to CSV format.

    This is the low-level CSV writing function. It writes the DataFrame
    produced by :func:`messages_to_dataframe` to the specified output
    destination using :meth:`pandas.DataFrame.to_csv`.

    Args:
        df: DataFrame with columns matching :data:`CSV_HEADER`.
        output: Destination for CSV output. Accepts:
            - A file path (``str`` or ``Path``) — writes to that file.
            - A text stream (e.g., ``sys.stdout``) — writes directly.
            - ``None`` — writes to ``sys.stdout``.

    Raises:
        MieWriterError: If an I/O error occurs during writing.
    """
    dest: str | Path | TextIO = output if output is not None else sys.stdout
    dest_name = str(output) if output is not None else "stdout"

    try:
        # Keep output byte-stable across platforms and aligned with the Rust
        # implementation. Pandas otherwise emits CRLF when writing on Windows.
        df.to_csv(dest, index=False, lineterminator="\n")
    except OSError as exc:
        raise MieWriterError(dest_name, exc) from exc

    logger.info("Wrote %d rows to %s", len(df), dest_name)


def write_csv(
    messages: Iterable[MieMessage],
    output: str | Path | TextIO | None = None,
) -> int:
    """Write all messages (normal + errored) to a single CSV.

    Used for INLINE error mode. ERROR and ERROR_CODE columns are
    populated for errored and spurious records.

    Args:
        messages: Iterable of decoded MieMessage instances.
        output: Destination for CSV output.

    Returns:
        The number of messages written.

    Raises:
        MieWriterError: If an I/O error occurs during writing.
    """
    df = messages_to_dataframe(messages)
    dataframe_to_csv(df, output=output)
    return len(df)


def write_csv_split(
    messages: Iterable[MieMessage],
    output: str | Path,
) -> tuple[int, int]:
    """Write normal messages to main CSV, errors to a separate file.

    Used for SEPARATE error mode (default). Normal messages go to
    ``output``, errored and spurious records go to
    ``<output_stem>_errors<output_suffix>``.

    Args:
        messages: Iterable of decoded MieMessage instances.
        output: Path for the main CSV output file.

    Returns:
        A tuple of (normal_count, error_count).

    Raises:
        MieWriterError: If an I/O error occurs during writing.
    """
    output_path = Path(output)
    error_path = output_path.with_name(
        f"{output_path.stem}_errors{output_path.suffix}"
    )

    normal_rows: list[dict[str, str]] = []
    error_rows: list[dict[str, str]] = []

    for msg in messages:
        row = message_to_row(msg)
        if msg.error_label:
            error_rows.append(row)
        else:
            normal_rows.append(row)

    normal_df = pd.DataFrame(normal_rows, columns=CSV_HEADER)
    dataframe_to_csv(normal_df, output=output_path)
    logger.info("Wrote %d normal messages to %s", len(normal_df), output_path)

    if error_rows:
        error_df = pd.DataFrame(error_rows, columns=CSV_HEADER)
        dataframe_to_csv(error_df, output=error_path)
        logger.info("Wrote %d error/spurious messages to %s", len(error_df), error_path)
    else:
        logger.info("No error/spurious messages — error file not created")

    return len(normal_df), len(error_rows)
