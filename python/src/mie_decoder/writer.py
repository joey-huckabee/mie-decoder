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
import os
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable, TextIO

import pandas as pd

from mie_decoder.exceptions import (
    MieClobberRefusedError,
    MieInputOutputCollisionError,
    MieUnrecoverableSyncLossError,
    MieWriterError,
)
from mie_decoder.models import MieMessage

logger = logging.getLogger(__name__)


# ── Path identity check (L2-WRT-014) ───────────────────────────────────


def paths_refer_to_same_file(input_path: Path, output_path: Path) -> bool:
    """Test whether ``input_path`` and ``output_path`` resolve to the same file.

    Handles the common case where ``output_path`` does not yet exist by
    resolving the parent directory and comparing against the prospective
    full path. Symlink-safe via ``Path.resolve``.
    """
    try:
        input_resolved = Path(input_path).resolve(strict=True)
    except (OSError, RuntimeError):
        return False
    # Direct path: both exist.
    try:
        output_resolved = Path(output_path).resolve(strict=True)
        return input_resolved == output_resolved
    except (OSError, RuntimeError):
        pass
    # Output doesn't exist; resolve its parent and join the filename.
    op = Path(output_path)
    parent = op.parent if str(op.parent) else Path(".")
    try:
        parent_resolved = parent.resolve(strict=True)
    except (OSError, RuntimeError):
        return False
    if op.name == "":
        return False
    return input_resolved == parent_resolved / op.name


# ── WriteOptions and preflight (L2-WRT-014, L2-WRT-017) ────────────────


@dataclass(frozen=True)
class WriteOptions:
    """Output-side options for the L2-WRT-014/017 safety checks.

    Attributes:
        input_path: Input MIE file path. When set, ``write_csv`` and
            ``write_csv_split`` reject same-path output before opening
            any file.
        no_clobber: When True, refuse to overwrite an existing
            destination.
        allow_partial: When True, an unrecoverable mid-file sync loss
            commits the rows decoded so far as ``<destination>.partial``
            and returns success rather than propagating the error.
    """

    input_path: Path | None = None
    no_clobber: bool = False
    allow_partial: bool = False


@dataclass(frozen=True)
class PartialCommit:
    """Where the partial output landed when ``allow_partial`` converted
    an ``UnrecoverableSyncLoss`` into a successful exit.

    Attributes:
        main_path: Path of the committed main `.partial` file.
        errors_path: Path of the errors `.partial` file, if any
            errored/spurious rows were written before the sync loss.
        offset: Byte offset of the unrecoverable boundary.
        sync_losses: Cumulative recovery attempts when the loss
            became unrecoverable.
    """

    main_path: Path
    errors_path: Path | None
    offset: int
    sync_losses: int


@dataclass(frozen=True)
class WriteOutcome:
    """Result of a successful CSV write.

    ``partial`` is ``None`` for Complete / PartialRecovered decodes;
    the CLI distinguishes the two by querying
    ``MieFileReader.sync_losses`` post-iteration. ``partial`` is
    ``Some(PartialCommit)`` only when ``allow_partial`` fired.

    Attributes:
        normal_count: Number of normal messages written.
        error_count: Number of errored/spurious messages written.
        partial: Partial-commit info, if applicable.
    """

    normal_count: int
    error_count: int
    partial: PartialCommit | None


def _preflight_output(output: Path, opts: WriteOptions) -> None:
    """Raise per the L2-WRT-014 and L2-WRT-017 contracts.

    Runs before any output file is opened so existing destinations are
    never partially overwritten on a rejected configuration.
    """
    if opts.input_path is not None and paths_refer_to_same_file(opts.input_path, output):
        raise MieInputOutputCollisionError(str(output))
    if opts.no_clobber and output.exists():
        raise MieClobberRefusedError(str(output))


# ── Atomic CSV write helper (L2-WRT-015, L2-WRT-016) ───────────────────


def _make_temp_path(final_path: Path) -> Path:
    """Build the temp file path next to ``final_path``.

    Pattern: ``<destination>.mie-decoder.tmp.<pid>``. Co-located so
    ``os.replace`` is atomic (same filesystem). The PID suffix avoids
    collisions when multiple processes target the same destination
    directory.
    """
    return final_path.with_name(f"{final_path.name}.mie-decoder.tmp.{os.getpid()}")


def _write_dataframe_atomic(df: pd.DataFrame, dest: Path) -> None:
    """Write ``df`` to a temp file beside ``dest``, then ``os.replace``.

    On any failure during the write, the temp file is unlinked and the
    original ``dest`` (if it existed) is left untouched.
    """
    temp = _make_temp_path(dest)
    try:
        df.to_csv(temp, index=False, lineterminator="\n")
    except OSError as exc:
        if temp.exists():
            try:
                temp.unlink()
            except OSError:
                pass
        raise MieWriterError(str(dest), exc) from exc
    try:
        os.replace(temp, dest)
    except OSError as exc:
        if temp.exists():
            try:
                temp.unlink()
            except OSError:
                pass
        raise MieWriterError(str(dest), exc) from exc

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

    Low-level CSV writing. File-path destinations go through the
    atomic temp-file + ``os.replace`` pattern (L2-WRT-015). Text-stream
    destinations (including ``sys.stdout``) bypass the atomic path
    because they have no on-disk identity; a broken-pipe condition on
    such a destination is silently swallowed (L2-WRT-018).

    Args:
        df: DataFrame with columns matching :data:`CSV_HEADER`.
        output: Destination for CSV output. Accepts:
            - A file path (``str`` or ``Path``) — atomic write.
            - A text stream (e.g., ``sys.stdout``) — direct write.
            - ``None`` — writes to ``sys.stdout``.

    Raises:
        MieWriterError: If an I/O error occurs during writing.
    """
    if isinstance(output, (str, Path)):
        dest = Path(output)
        _write_dataframe_atomic(df, dest)
        logger.info("Wrote %d rows to %s", len(df), dest)
        return

    # Text-stream destination (TextIO or None → stdout).
    stream: TextIO = output if output is not None else sys.stdout
    dest_name = "stdout" if output is None else "<stream>"
    try:
        # Keep output byte-stable across platforms and aligned with the Rust
        # implementation. Pandas otherwise emits CRLF when writing on Windows.
        df.to_csv(stream, index=False, lineterminator="\n")
    except BrokenPipeError:
        # L2-WRT-018: downstream consumer closed early. Treat as success.
        logger.info("Stdout consumer closed early (broken pipe) — exit 0")
        return
    except OSError as exc:
        raise MieWriterError(dest_name, exc) from exc

    logger.info("Wrote %d rows to %s", len(df), dest_name)


def write_csv(
    messages: Iterable[MieMessage],
    output: str | Path | TextIO | None = None,
    opts: WriteOptions | None = None,
) -> WriteOutcome:
    """Write all messages (normal + errored) to a single CSV.

    Used for INLINE error mode. ERROR and ERROR_CODE columns are
    populated for errored and spurious records.

    Args:
        messages: Iterable of decoded MieMessage instances.
        output: Destination for CSV output (file path, stream, or None for stdout).
        opts: Output safety options. When ``output`` is a file path, the
            L2-WRT-014 input/output collision check, L2-WRT-017 no-clobber
            check, and L1-023 allow_partial handling are applied. Stream
            destinations ignore these (no on-disk identity, no partial).

    Returns:
        A WriteOutcome capturing counts and optional PartialCommit info.

    Raises:
        MieInputOutputCollisionError: Output path resolves to the same file as the input.
        MieClobberRefusedError: Output exists and ``opts.no_clobber`` is True.
        MieUnrecoverableSyncLossError: Lenient-mode mid-file sync loss
            exhausted recovery and ``opts.allow_partial`` is False.
        MieWriterError: If an I/O error occurs during writing.
    """
    if opts is None:
        opts = WriteOptions()
    if isinstance(output, (str, Path)):
        _preflight_output(Path(output), opts)

    # Collect rows in one pass so we can detect UnrecoverableSyncLoss
    # mid-stream and commit the rows-so-far as .partial when
    # allow_partial is set. (Python's writer materialises the full
    # DataFrame regardless; the streaming variant is tracked as
    # PY-streaming.)
    rows: list[dict[str, str]] = []
    partial_info: tuple[int, int] | None = None
    try:
        for i, msg in enumerate(messages):
            rows.append(message_to_row(msg))
            if (i + 1) % 10_000 == 0:
                logger.debug("Converted %d messages to rows", i + 1)
    except MieUnrecoverableSyncLossError as exc:
        if not opts.allow_partial:
            raise
        partial_info = (exc.offset, exc.sync_losses)

    df = pd.DataFrame(rows, columns=CSV_HEADER)

    if partial_info is not None and isinstance(output, (str, Path)):
        dest = Path(output)
        partial_path = dest.with_name(f"{dest.name}.partial")
        _write_dataframe_atomic(df, partial_path)
        offset, sync_losses = partial_info
        logger.warning(
            "Unrecoverable sync loss at 0x%X after %d recovery attempt(s); "
            "wrote %d rows to %s (--allow-partial)",
            offset, sync_losses, len(df), partial_path,
        )
        return WriteOutcome(
            normal_count=len(df),
            error_count=0,
            partial=PartialCommit(
                main_path=partial_path,
                errors_path=None,
                offset=offset,
                sync_losses=sync_losses,
            ),
        )

    dataframe_to_csv(df, output=output)
    return WriteOutcome(normal_count=len(df), error_count=0, partial=None)


def write_csv_split(
    messages: Iterable[MieMessage],
    output: str | Path,
    opts: WriteOptions | None = None,
) -> WriteOutcome:
    """Write normal messages to main CSV, errors to a separate file.

    Used for SEPARATE error mode (default). Normal messages go to
    ``output``, errored and spurious records go to
    ``<output_stem>_errors<output_suffix>``.

    Both files are written via the atomic temp + ``os.replace`` pattern.
    If the errors-file write fails after the main file has been
    committed, the main file remains; we accept this trade-off because
    atomically committing two files together is not possible without
    cross-file rename support.

    Args:
        messages: Iterable of decoded MieMessage instances.
        output: Path for the main CSV output file.
        opts: Output safety options. Both the main destination AND the
            derived errors path are checked against ``opts.no_clobber``.
            The L2-WRT-014 collision check applies to the main path.

    Returns:
        A tuple of (normal_count, error_count).

    Raises:
        MieInputOutputCollisionError: Main output collides with input.
        MieClobberRefusedError: Main or errors destination exists and
            ``opts.no_clobber`` is True.
        MieWriterError: If an I/O error occurs during writing.
    """
    if opts is None:
        opts = WriteOptions()
    output_path = Path(output)
    error_path = output_path.with_name(
        f"{output_path.stem}_errors{output_path.suffix}"
    )

    _preflight_output(output_path, opts)
    # The errors-file path also needs the no-clobber check; the
    # collision check is implicit since errors_path is derived from
    # output_path which was just checked.
    if opts.no_clobber and error_path.exists():
        raise MieClobberRefusedError(str(error_path))

    normal_rows: list[dict[str, str]] = []
    error_rows: list[dict[str, str]] = []
    partial_info: tuple[int, int] | None = None

    try:
        for msg in messages:
            row = message_to_row(msg)
            if msg.error_label:
                error_rows.append(row)
            else:
                normal_rows.append(row)
    except MieUnrecoverableSyncLossError as exc:
        if not opts.allow_partial:
            raise
        partial_info = (exc.offset, exc.sync_losses)

    normal_df = pd.DataFrame(normal_rows, columns=CSV_HEADER)

    if partial_info is not None:
        # Commit both files as .partial.
        main_partial = output_path.with_name(f"{output_path.name}.partial")
        _write_dataframe_atomic(normal_df, main_partial)
        errors_partial: Path | None = None
        if error_rows:
            error_df = pd.DataFrame(error_rows, columns=CSV_HEADER)
            errors_partial = error_path.with_name(f"{error_path.name}.partial")
            _write_dataframe_atomic(error_df, errors_partial)
        offset, sync_losses = partial_info
        logger.warning(
            "Unrecoverable sync loss at 0x%X after %d recovery attempt(s); "
            "wrote %d normal + %d error rows as partial to %s (--allow-partial)",
            offset, sync_losses, len(normal_df), len(error_rows), main_partial,
        )
        return WriteOutcome(
            normal_count=len(normal_df),
            error_count=len(error_rows),
            partial=PartialCommit(
                main_path=main_partial,
                errors_path=errors_partial,
                offset=offset,
                sync_losses=sync_losses,
            ),
        )

    dataframe_to_csv(normal_df, output=output_path)
    logger.info("Wrote %d normal messages to %s", len(normal_df), output_path)

    if error_rows:
        error_df = pd.DataFrame(error_rows, columns=CSV_HEADER)
        dataframe_to_csv(error_df, output=error_path)
        logger.info("Wrote %d error/spurious messages to %s", len(error_df), error_path)
    else:
        logger.info("No error/spurious messages — error file not created")

    return WriteOutcome(
        normal_count=len(normal_df),
        error_count=len(error_rows),
        partial=None,
    )
