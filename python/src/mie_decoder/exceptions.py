"""Custom exceptions for the MIE-Decoder package.

All exceptions inherit from :class:`MieDecoderError`, allowing callers
to catch the full family with a single ``except`` clause while still
being able to discriminate specific failure modes.

Exception hierarchy::

    MieDecoderError
    ├── MieFileError
    │   ├── MieFileNotFoundError
    │   ├── MieFileEmptyError
    │   ├── MieNoValidRecordsError
    │   ├── MieInputOutputCollisionError
    │   └── MieClobberRefusedError
    ├── MieRecordError
    │   ├── MieInvalidTypeWordError
    │   ├── MieUnknownTypeWordError
    │   ├── MieRecordTruncatedError
    │   ├── MiePayloadError
    │   ├── MieUnknownErrorCodeError
    │   └── MieUnrecoverableSyncLossError
    └── MieWriterError
"""

from __future__ import annotations


class MieDecoderError(Exception):
    """Base exception for all MIE-Decoder errors.

    All exceptions raised by the MIE-Decoder library inherit from this
    class, enabling callers to catch any decoder-related error with a
    single handler::

        try:
            for msg in MieFileReader("recording.mie"):
                process(msg)
        except MieDecoderError as exc:
            logger.error("Decoder failure: %s", exc)
    """


class MieFileError(MieDecoderError):
    """Base exception for file-level errors.

    Raised when the input file cannot be opened, is missing, or is
    structurally invalid at the file level (as opposed to individual
    record-level corruption).
    """


class MieFileNotFoundError(MieFileError):
    """Raised when the specified MIE binary file does not exist.

    Attributes:
        path: The file path that was not found.
    """

    def __init__(self, path: str) -> None:
        self.path = path
        super().__init__(f"MIE file not found: {path}")


class MieNoValidRecordsError(MieFileError):
    """Raised when the input file contains no decodable MIE records.

    Per L1-021 the CLI maps this to exit code ``2``. Typically means the
    file is not an MIE recording at all (e.g., a TOML file passed as
    input by mistake) or the records begin past the 64 KB header scan
    window.

    Attributes:
        path: Path that was scanned.
        scan_bytes: Number of bytes scanned before giving up.
    """

    def __init__(self, path: str, scan_bytes: int) -> None:
        self.path = path
        self.scan_bytes = scan_bytes
        super().__init__(
            f"No valid MIE records found in {path} "
            f"(scanned first {scan_bytes} bytes). "
            f"The file may not be an MIE recording, or the records may "
            f"begin past the scan window."
        )


class MieFileEmptyError(MieFileError):
    """Raised when the specified MIE binary file exists but is zero bytes.

    An empty file cannot contain any valid records and indicates either
    a recording that was started but never received data, or a file
    transfer error.

    Attributes:
        path: The file path that was empty.
    """

    def __init__(self, path: str) -> None:
        self.path = path
        super().__init__(f"MIE file is empty (0 bytes): {path}")


class MieInputOutputCollisionError(MieFileError):
    """Raised when the output path resolves to the same file as the input.

    Per L2-WRT-014, decoding in-place is unsafe with a memory-mapped
    reader. The implementation rejects this configuration before any
    output file is opened.

    Attributes:
        path: The colliding path (both input and output resolve here).
    """

    def __init__(self, path: str) -> None:
        self.path = path
        super().__init__(
            f"Output path resolves to the same file as the input ({path}); "
            f"decoding in-place is unsafe with a memory-mapped reader. "
            f"Choose a different output path."
        )


class MieClobberRefusedError(MieFileError):
    """Raised when ``--no-clobber`` is set and the destination exists.

    Per L2-WRT-017, the implementation refuses to overwrite an existing
    file when the no-clobber flag is in effect (CLI ``--no-clobber`` or
    config ``output.no_clobber = true``).

    Attributes:
        path: The destination path that already exists.
    """

    def __init__(self, path: str) -> None:
        self.path = path
        super().__init__(
            f"Refusing to overwrite existing file {path} "
            f"(--no-clobber or output.no_clobber is set). "
            f"Remove the file or unset the flag to proceed."
        )


class MieRecordError(MieDecoderError):
    """Base exception for record-level decoding errors.

    Raised when an individual binary record within a valid file cannot
    be decoded. Includes the byte offset of the problematic record for
    diagnostic purposes.

    Attributes:
        offset: Byte offset of the record in the source file.
        detail: Human-readable description of the error.
    """

    def __init__(self, offset: int, detail: str) -> None:
        self.offset = offset
        self.detail = detail
        super().__init__(f"Record error at offset 0x{offset:X}: {detail}")


class MieInvalidTypeWordError(MieRecordError):
    """Raised when a Type Word produces an invalid or zero word count.

    A word count below the minimum record size (5 words = Type Word +
    3 Timestamp words + Command Word) indicates either file corruption
    or an unsupported record type.

    Attributes:
        offset: Byte offset of the record.
        raw_type_word: The raw 16-bit Type Word value.
        word_count: The decoded word count that was invalid.
    """

    def __init__(self, offset: int, raw_type_word: int, word_count: int) -> None:
        self.raw_type_word = raw_type_word
        self.word_count = word_count
        super().__init__(
            offset,
            f"Invalid Type Word 0x{raw_type_word:04X} with word_count={word_count} "
            f"(minimum is 5)",
        )


class MieUnknownTypeWordError(MieRecordError):
    """Raised when a Type Word contains an unrecognized message type code.

    The DDC MIE format defines seven message type codes in bits 0–6 of
    the Type Word: 0x01 (Mode Command), 0x02 (BC→RT), 0x04 (RT→BC),
    0x08 (RT→RT), 0x10 (Broadcast BC→RT), 0x18 (Broadcast RT→RT), and
    0x20 (Spurious Data). Any other value indicates either file
    corruption, a firmware version producing an unknown format, or an
    undocumented DDC extension.

    When encountered, the decoder prints diagnostic information to the
    console including the byte offset, raw Type Word value, and
    surrounding context to assist in reverse engineering the unknown
    format.

    Attributes:
        offset: Byte offset of the record in the source file.
        raw_type_word: The raw 16-bit Type Word value.
        message_type: The unrecognized message type code (bits 0–6).
    """

    def __init__(self, offset: int, raw_type_word: int, message_type: int) -> None:
        self.raw_type_word = raw_type_word
        self.message_type = message_type
        super().__init__(
            offset,
            f"Unknown message type 0x{message_type:02X} in Type Word "
            f"0x{raw_type_word:04X}. Known types: 0x01 (Mode Command), "
            f"0x02 (BC→RT), 0x04 (RT→BC), 0x08 (RT→RT), "
            f"0x10 (Broadcast BC→RT), 0x18 (Broadcast RT→RT), "
            f"0x20 (Spurious Data).",
        )


class MieRecordTruncatedError(MieRecordError):
    """Raised when a record extends beyond the end of the file.

    This typically occurs at the end of a recording that was terminated
    mid-write. The reader can optionally skip truncated final records
    rather than raising this exception.

    Attributes:
        offset: Byte offset of the record start.
        record_bytes: Expected record size in bytes.
        available_bytes: Actual bytes remaining in the file.
    """

    def __init__(self, offset: int, record_bytes: int, available_bytes: int) -> None:
        self.record_bytes = record_bytes
        self.available_bytes = available_bytes
        super().__init__(
            offset,
            f"Record requires {record_bytes} bytes but only "
            f"{available_bytes} bytes remain in file",
        )


class MiePayloadError(MieRecordError):
    """Raised when a record's payload cannot be extracted.

    Indicates that the data words, status word, or command word within
    a record are inconsistent with the Type Word's declared word count
    or the Command Word's declared data word count.

    Attributes:
        offset: Byte offset of the record.
    """


class MieWriterError(MieDecoderError):
    """Raised when CSV or other output writing fails.

    Wraps underlying I/O errors with decoder-specific context.

    Attributes:
        destination: Description of the output destination (path or "stdout").
    """

    def __init__(self, destination: str, cause: Exception) -> None:
        self.destination = destination
        self.cause = cause
        super().__init__(f"Failed to write to {destination}: {cause}")


class MieUnrecoverableSyncLossError(MieRecordError):
    """Raised in lenient mode when sync recovery exhausts mid-file.

    Per L1-023 the CLI maps this to exit code ``3`` by default, or to a
    ``.partial`` commit + exit ``0`` when ``--allow-partial`` is set.
    Carries the cumulative recovery-attempt count for the decode
    invocation so the operator can correlate with sync-loss WARN logs.

    Attributes:
        offset: Byte offset of the record that triggered the
            unrecoverable loss.
        sync_losses: Cumulative recovery attempts when the loss became
            unrecoverable.
    """

    def __init__(self, offset: int, sync_losses: int) -> None:
        self.sync_losses = sync_losses
        super().__init__(
            offset,
            f"Unrecoverable mid-file sync loss after {sync_losses} "
            f"recovery attempt(s); the decoder could not reacquire sync "
            f"within the scan window. Pass --allow-partial to keep what "
            f"was decoded as a .partial file.",
        )


class MieUnknownErrorCodeError(MieRecordError):
    """Raised when an errored record contains an unrecognized error code.

    DDC hardware error codes occupy the 0x01xx range. MIE-Decoder custom
    codes occupy the 0x20xx range. Any error code outside these known
    sets indicates either an undocumented DDC firmware version or file
    corruption.

    Attributes:
        offset: Byte offset of the record in the source file.
        error_code: The unrecognized 16-bit error code value.
    """

    def __init__(self, offset: int, error_code: int) -> None:
        self.error_code = error_code
        super().__init__(
            offset,
            f"Unknown error code 0x{error_code:04X}. "
            f"Known DDC codes: 0x011E, 0x0120, 0x0136, 0x0140, 0x0150. "
            f"Known decoder codes: 0x2000, 0x2001.",
        )
