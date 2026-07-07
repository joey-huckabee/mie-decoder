"""CSV output writer for decoded MIE messages.

Streams rows straight to the output handle through the standard-library
``csv`` module — no DataFrame or full-file buffering, so decode memory is
O(1) in the record count (L3-PY-012). Produces CSV output matching the
column layout used by DDC's recording software, enabling direct
comparison between MIE-Decoder output and vendor-generated CSV files.

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
        Not decoded from the binary record; emitted as an empty column
        to preserve the vendor CSV layout (L2-WRT-013).

    TERM_NAME
        Terminal or equipment name associated with the RT/SA combination.
        Derived from external configuration; not decoded, so emitted as
        an empty column to preserve the vendor CSV layout (L2-WRT-013).

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
        Inter-message gap. Not decoded from the binary record; emitted as
        an empty column to preserve the vendor CSV layout (L2-WRT-013).

    RCV_GAP
        Receive gap. Not decoded from the binary record; emitted as an
        empty column to preserve the vendor CSV layout (L2-WRT-013).

    XMT_GAP
        Transmit gap. Not decoded from the binary record; emitted as an
        empty column to preserve the vendor CSV layout (L2-WRT-013).
"""

from __future__ import annotations

import csv
import logging
import os
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable, TextIO

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
    ("MUX", "Multiplexer label (external config, empty by spec L2-WRT-013)"),
    ("TERM_NAME", "Terminal name (external config, empty by spec L2-WRT-013)"),
    ("BUS", "Bus identifier: A or B"),
    ("DELTA", "Seconds since prior message with same RT+MSG"),
    ("ERROR", "Error label: empty=normal, ERROR=bit14, SPURIOUS=type 0x20"),
    ("ERROR_CODE", "DDC error code (0x01xx) or decoder code (0x20xx)"),
    ("IM_GAP", "Inter-message gap (empty by spec L2-WRT-013)"),
    ("RCV_GAP", "Receive gap (empty by spec L2-WRT-013)"),
    ("XMT_GAP", "Transmit gap (empty by spec L2-WRT-013)"),
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
        "MUX": msg.mux or "",
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


# ── Streaming primitives (PY-streaming, L3-PY-012) ─────────────────────
#
# These mirror the Rust writer's `AtomicCsvFile` and `CsvWriter` so both
# implementations stream rows straight to the output handle with no
# per-record buffering — memory is O(1) in the record count. The
# byte image they produce is pinned by the golden characterization
# tests (tests/test_writer_streaming_golden.py).


class _AtomicCsvFile:
    """Temp-file + ``os.replace`` atomic writer (L2-WRT-015/016).

    Mirrors the Rust ``AtomicCsvFile``. Opens a temp file beside the
    destination (same directory → ``os.replace`` is atomic on one
    filesystem). Callers write through :attr:`stream`. :meth:`commit`
    renames the temp over the destination; :meth:`commit_partial`
    renames it to ``<destination>.partial``. If the writer is closed
    without committing (decode failed or was interrupted), the temp
    file is unlinked and a pre-existing destination is left untouched.

    Usable as a context manager: an uncommitted writer is cleaned up on
    ``__exit__`` so the failure path leaves no temp behind.
    """

    def __init__(self, final_path: Path) -> None:
        self._final = final_path
        self._temp = _make_temp_path(final_path)
        try:
            # newline="" stops the csv module's terminator being
            # re-translated by the text layer, so output is LF-only.
            # The stream is owned by this object and closed in commit() /
            # cleanup(), so a `with` block does not fit its lifecycle.
            self._stream: TextIO = open(  # pylint: disable=consider-using-with
                self._temp, "w", newline="", encoding="utf-8"
            )
        except OSError as exc:
            raise MieWriterError(str(final_path), exc) from exc
        self._committed = False

    @property
    def stream(self) -> TextIO:
        """The underlying text stream (the open temp file)."""
        return self._stream

    def commit(self) -> None:
        """Flush, close, and atomically rename the temp over the destination."""
        self._close_stream()
        try:
            os.replace(self._temp, self._final)
        except OSError as exc:
            self._cleanup_temp()
            raise MieWriterError(str(self._final), exc) from exc
        self._committed = True

    def commit_partial(self) -> Path:
        """Rename the temp to ``<destination>.partial`` instead of over the
        destination (L2-WRT-016 ``--allow-partial`` branch). The original
        destination, if any, is left untouched. Returns the path written."""
        self._close_stream()
        partial = self._final.with_name(f"{self._final.name}.partial")
        try:
            os.replace(self._temp, partial)
        except OSError as exc:
            self._cleanup_temp()
            raise MieWriterError(str(partial), exc) from exc
        self._committed = True
        return partial

    def _close_stream(self) -> None:
        if not self._stream.closed:
            self._stream.flush()
            self._stream.close()

    def _cleanup_temp(self) -> None:
        try:
            if self._temp.exists():
                self._temp.unlink()
        except OSError:
            pass

    def close(self) -> None:
        """Close the stream; unlink the temp if it was never committed."""
        if not self._stream.closed:
            self._stream.close()
        if not self._committed:
            self._cleanup_temp()

    def __enter__(self) -> _AtomicCsvFile:
        return self

    def __exit__(self, *exc_info: object) -> None:
        self.close()


class _StreamingCsvRowWriter:
    """Streaming CSV row writer (mirrors the Rust ``CsvWriter``).

    Writes the header row on construction, then one CSV row per message
    via ``csv.DictWriter``. Retains no per-record buffer beyond the
    underlying stream's, so memory is O(1) in the record count
    (L3-PY-012 / L3-RS-012). ``lineterminator="\\n"`` keeps output
    byte-stable across platforms and aligned with the Rust writer.

    ``BrokenPipeError`` is allowed to propagate (the stdout consumer
    closed early); callers map it to a clean exit per L2-WRT-018. Other
    ``OSError``\\ s (disk full, permission) are wrapped as
    :class:`MieWriterError`.
    """

    def __init__(self, stream: TextIO, destination: str) -> None:
        self._destination = destination
        self._writer = csv.DictWriter(stream, fieldnames=CSV_HEADER, lineterminator="\n")
        self._rows_written = 0
        try:
            self._writer.writeheader()
        except BrokenPipeError:
            raise
        except OSError as exc:
            raise MieWriterError(destination, exc) from exc

    def write_message(self, msg: MieMessage) -> None:
        """Write one decoded message as a CSV row."""
        try:
            self._writer.writerow(message_to_row(msg))
        except BrokenPipeError:
            raise
        except OSError as exc:
            raise MieWriterError(self._destination, exc) from exc
        self._rows_written += 1

    @property
    def rows_written(self) -> int:
        """Number of data rows written so far."""
        return self._rows_written


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
            check, and L1-EXIT-004 allow_partial handling are applied. Stream
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

    # File-path destination vs. text-stream (TextIO or None → stdout): the two
    # differ enough (atomic temp + preflight + .partial vs. straight streaming)
    # to warrant separate helpers.
    if isinstance(output, (str, Path)):
        return _write_csv_to_file(messages, Path(output), opts)
    stream: TextIO = output if output is not None else sys.stdout
    dest_name = "stdout" if output is None else "<stream>"
    return _write_csv_to_stream(messages, stream, dest_name, opts)


def _write_csv_to_file(
    messages: Iterable[MieMessage], dest: Path, opts: WriteOptions
) -> WriteOutcome:
    """Stream rows into an atomic temp file (constant memory), then commit() over
    the destination — or commit_partial() to ``<dest>.partial`` on an
    allow_partial sync loss."""
    _preflight_output(dest, opts)
    partial_info: tuple[int, int] | None = None
    with _AtomicCsvFile(dest) as atomic:
        writer = _StreamingCsvRowWriter(atomic.stream, str(dest))
        try:
            for msg in messages:
                writer.write_message(msg)
        except MieUnrecoverableSyncLossError as exc:
            if not opts.allow_partial:
                raise
            partial_info = (exc.offset, exc.sync_losses)

        count = writer.rows_written
        if partial_info is None:
            atomic.commit()
            logger.info("Wrote %d rows to %s", count, dest)
            return WriteOutcome(normal_count=count, error_count=0, partial=None)

        partial_path = atomic.commit_partial()
        offset, sync_losses = partial_info
        logger.warning(
            "Unrecoverable sync loss at 0x%X after %d recovery attempt(s); "
            "wrote %d rows to %s (--allow-partial)",
            offset,
            sync_losses,
            count,
            partial_path,
        )
        return WriteOutcome(
            normal_count=count,
            error_count=0,
            partial=PartialCommit(
                main_path=partial_path,
                errors_path=None,
                offset=offset,
                sync_losses=sync_losses,
            ),
        )


def _write_csv_to_stream(
    messages: Iterable[MieMessage], stream: TextIO, dest_name: str, opts: WriteOptions
) -> WriteOutcome:
    """Stream rows straight to a text sink. No on-disk identity, so no preflight,
    no atomic temp, and no ``.partial`` — rows already sent are what the consumer
    has seen."""
    writer = _StreamingCsvRowWriter(stream, dest_name)
    try:
        for msg in messages:
            writer.write_message(msg)
    except BrokenPipeError:
        # L2-WRT-018: downstream consumer closed early. Treat as success.
        logger.info("Stdout consumer closed early (broken pipe) — exit 0")
        return WriteOutcome(normal_count=writer.rows_written, error_count=0, partial=None)
    except MieUnrecoverableSyncLossError:
        if not opts.allow_partial:
            raise
        # Rows decoded so far are already in the stream; nothing to roll back.
        logger.debug("Unrecoverable sync loss on stream output (--allow-partial)")

    logger.info("Wrote %d rows to %s", writer.rows_written, dest_name)
    return WriteOutcome(normal_count=writer.rows_written, error_count=0, partial=None)


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
    error_path = output_path.with_name(f"{output_path.stem}_errors{output_path.suffix}")

    _preflight_output(output_path, opts)
    # The errors-file path also needs the no-clobber check; the
    # collision check is implicit since errors_path is derived from
    # output_path which was just checked.
    if opts.no_clobber and error_path.exists():
        raise MieClobberRefusedError(str(error_path))

    # Stream into the main temp file eagerly; the errors temp is created
    # lazily on the first error row so a clean decode never leaves an
    # empty errors CSV behind. Both stay O(1) in the record count.
    main_atomic = _AtomicCsvFile(output_path)
    errors_atomic: _AtomicCsvFile | None = None
    partial_info: tuple[int, int] | None = None
    try:
        main_writer = _StreamingCsvRowWriter(main_atomic.stream, str(output_path))
        error_writer: _StreamingCsvRowWriter | None = None

        try:
            for msg in messages:
                if msg.error_label:
                    if error_writer is None:
                        errors_atomic = _AtomicCsvFile(error_path)
                        error_writer = _StreamingCsvRowWriter(errors_atomic.stream, str(error_path))
                    error_writer.write_message(msg)
                else:
                    main_writer.write_message(msg)
        except MieUnrecoverableSyncLossError as exc:
            if not opts.allow_partial:
                raise
            partial_info = (exc.offset, exc.sync_losses)

        normal_count = main_writer.rows_written
        error_count = error_writer.rows_written if error_writer is not None else 0
        return _commit_split_outputs(
            main_atomic,
            errors_atomic,
            output_path,
            error_path,
            normal_count,
            error_count,
            partial_info,
        )
    finally:
        # Unlink any temp that was never committed (failure path). After a
        # successful commit/commit_partial these are no-ops.
        main_atomic.close()
        if errors_atomic is not None:
            errors_atomic.close()


def _commit_split_outputs(
    main_atomic: _AtomicCsvFile,
    errors_atomic: _AtomicCsvFile | None,
    output_path: Path,
    error_path: Path,
    normal_count: int,
    error_count: int,
    partial_info: tuple[int, int] | None,
) -> WriteOutcome:
    """Commit the split outputs. ``partial_info is None`` is the normal path
    (atomic rename over each destination, MAIN first per L2-WRT-019 so a failed
    errors commit never leaves an orphan errors file); a tuple is the
    ``--allow-partial`` path (rename each temp to its ``.partial``)."""
    if partial_info is None:
        main_atomic.commit()
        logger.info("Wrote %d normal messages to %s", normal_count, output_path)
        if errors_atomic is not None:
            errors_atomic.commit()
            logger.info("Wrote %d error/spurious messages to %s", error_count, error_path)
        else:
            logger.info("No error/spurious messages — error file not created")
        return WriteOutcome(normal_count=normal_count, error_count=error_count, partial=None)

    # Partial path: commit each file as .partial (errors first, then main,
    # mirroring the Rust writer).
    errors_partial: Path | None = None
    if errors_atomic is not None:
        errors_partial = errors_atomic.commit_partial()
    main_partial = main_atomic.commit_partial()
    offset, sync_losses = partial_info
    logger.warning(
        "Unrecoverable sync loss at 0x%X after %d recovery attempt(s); "
        "wrote %d normal + %d error rows as partial to %s (--allow-partial)",
        offset,
        sync_losses,
        normal_count,
        error_count,
        main_partial,
    )
    return WriteOutcome(
        normal_count=normal_count,
        error_count=error_count,
        partial=PartialCommit(
            main_path=main_partial,
            errors_path=errors_partial,
            offset=offset,
            sync_losses=sync_losses,
        ),
    )
