//! Error types for the MIE-Decoder library.
//!
//! All fallible APIs return `Result<T, MieError>`. The single enum replaces
//! the Python class hierarchy; the `kind()` method returns a `MieErrorKind`
//! discriminant for callers that need to branch on the failure mode.

use std::fmt;
use std::io;
use std::path::PathBuf;

/// Single error type returned by all decoder operations.
#[derive(Debug)]
pub enum MieError {
    /// Specified MIE binary file does not exist.
    FileNotFound { path: PathBuf },

    /// Specified MIE binary file exists but is zero bytes.
    FileEmpty { path: PathBuf },

    /// File-level I/O failure (mmap, open, read).
    FileIo { path: PathBuf, source: io::Error },

    /// Type Word produced an invalid or zero word count.
    InvalidTypeWord {
        offset: u64,
        raw_type_word: u16,
        word_count: u16,
    },

    /// Type Word's message type code is not in the known set.
    UnknownTypeWord {
        offset: u64,
        raw_type_word: u16,
        message_type: u8,
    },

    /// Record extends beyond the end of the file.
    RecordTruncated {
        offset: u64,
        record_bytes: u64,
        available_bytes: u64,
    },

    /// Record's payload is inconsistent with Type Word / Command Word.
    PayloadError { offset: u64, detail: String },

    /// Errored record contains an unrecognized error code.
    UnknownErrorCode { offset: u64, error_code: u16 },

    /// File exists and is non-empty but contains no decodable MIE records
    /// within the initial 64 KB scan window. Typically means the file
    /// isn't an MIE recording at all (e.g., a TOML file mistakenly passed
    /// as input).
    NoValidRecords { path: PathBuf, scan_bytes: u64 },

    /// Output writer failed (CSV row, flush, etc).
    WriterError {
        destination: String,
        source: io::Error,
    },

    /// Output path resolves to the same file as the input. Per L2-WRT-014,
    /// decoding in-place is unsafe (the input is mmap-backed) and is
    /// rejected before any output file is opened.
    InputOutputCollision { path: PathBuf },

    /// `--no-clobber` was set (or `output.no_clobber = true` in config)
    /// and the output destination already exists. Per L2-WRT-017 the
    /// implementation refuses to overwrite rather than silently replacing.
    ClobberRefused { path: PathBuf },

    /// Mid-file sync loss in lenient mode that `recover_sync` could not
    /// reacquire within the scan window. Per L1-023 this maps to CLI
    /// exit code `3` by default, or to a `.partial` commit + exit `0`
    /// when `--allow-partial` is set. `sync_losses` is the cumulative
    /// recovery-attempt count for the decode invocation.
    UnrecoverableSyncLoss { offset: u64, sync_losses: u64 },
}

/// Discriminant identifying which variant of [`MieError`] occurred.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MieErrorKind {
    FileNotFound,
    FileEmpty,
    FileIo,
    InvalidTypeWord,
    UnknownTypeWord,
    RecordTruncated,
    PayloadError,
    UnknownErrorCode,
    NoValidRecords,
    WriterError,
    InputOutputCollision,
    ClobberRefused,
    UnrecoverableSyncLoss,
}

impl MieError {
    pub fn kind(&self) -> MieErrorKind {
        match self {
            Self::FileNotFound { .. } => MieErrorKind::FileNotFound,
            Self::FileEmpty { .. } => MieErrorKind::FileEmpty,
            Self::FileIo { .. } => MieErrorKind::FileIo,
            Self::InvalidTypeWord { .. } => MieErrorKind::InvalidTypeWord,
            Self::UnknownTypeWord { .. } => MieErrorKind::UnknownTypeWord,
            Self::RecordTruncated { .. } => MieErrorKind::RecordTruncated,
            Self::PayloadError { .. } => MieErrorKind::PayloadError,
            Self::UnknownErrorCode { .. } => MieErrorKind::UnknownErrorCode,
            Self::NoValidRecords { .. } => MieErrorKind::NoValidRecords,
            Self::WriterError { .. } => MieErrorKind::WriterError,
            Self::InputOutputCollision { .. } => MieErrorKind::InputOutputCollision,
            Self::ClobberRefused { .. } => MieErrorKind::ClobberRefused,
            Self::UnrecoverableSyncLoss { .. } => MieErrorKind::UnrecoverableSyncLoss,
        }
    }

    /// True if this error wraps an `io::Error` whose kind is `BrokenPipe`.
    /// Per L2-WRT-018 a broken-pipe condition on stdout output SHALL
    /// exit `0` with no error; the CLI driver uses this predicate.
    pub fn is_broken_pipe(&self) -> bool {
        matches!(
            self,
            Self::WriterError { source, .. } if source.kind() == io::ErrorKind::BrokenPipe
        )
    }

    /// True if this error originated at the file level (open/empty/io).
    pub fn is_file_error(&self) -> bool {
        matches!(
            self.kind(),
            MieErrorKind::FileNotFound | MieErrorKind::FileEmpty | MieErrorKind::FileIo
        )
    }

    /// True if this error is tied to a specific record byte offset.
    pub fn is_record_error(&self) -> bool {
        matches!(
            self.kind(),
            MieErrorKind::InvalidTypeWord
                | MieErrorKind::UnknownTypeWord
                | MieErrorKind::RecordTruncated
                | MieErrorKind::PayloadError
                | MieErrorKind::UnknownErrorCode
        )
    }
}

impl fmt::Display for MieError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FileNotFound { path } => {
                write!(f, "MIE file not found: {}", path.display())
            }
            Self::FileEmpty { path } => {
                write!(f, "MIE file is empty (0 bytes): {}", path.display())
            }
            Self::FileIo { path, source } => {
                write!(f, "I/O error on {}: {}", path.display(), source)
            }
            Self::InvalidTypeWord {
                offset,
                raw_type_word,
                word_count,
            } => write!(
                f,
                "Record error at offset 0x{offset:X}: \
                 Invalid Type Word 0x{raw_type_word:04X} with word_count={word_count} (minimum is 5)"
            ),
            Self::UnknownTypeWord {
                offset,
                raw_type_word,
                message_type,
            } => write!(
                f,
                "Record error at offset 0x{offset:X}: \
                 Unknown message type 0x{message_type:02X} in Type Word 0x{raw_type_word:04X}. \
                 Known types: 0x01, 0x02, 0x04, 0x08, 0x10, 0x18, 0x20."
            ),
            Self::RecordTruncated {
                offset,
                record_bytes,
                available_bytes,
            } => write!(
                f,
                "Record error at offset 0x{offset:X}: \
                 Record requires {record_bytes} bytes but only {available_bytes} bytes remain in file"
            ),
            Self::PayloadError { offset, detail } => {
                write!(f, "Record error at offset 0x{offset:X}: {detail}")
            }
            Self::UnknownErrorCode { offset, error_code } => write!(
                f,
                "Record error at offset 0x{offset:X}: \
                 Unknown error code 0x{error_code:04X}. \
                 Known DDC codes: 0x011E, 0x0120, 0x0136, 0x0140, 0x0150. \
                 Known decoder codes: 0x2000, 0x2001."
            ),
            Self::NoValidRecords { path, scan_bytes } => write!(
                f,
                "No valid MIE records found in {} (scanned first {scan_bytes} bytes). \
                 The file may not be an MIE recording, or the records may begin past the scan window.",
                path.display()
            ),
            Self::WriterError {
                destination,
                source,
            } => write!(f, "Failed to write to {destination}: {source}"),
            Self::InputOutputCollision { path } => write!(
                f,
                "Output path resolves to the same file as the input ({}); \
                 decoding in-place is unsafe with a memory-mapped reader. \
                 Choose a different output path.",
                path.display()
            ),
            Self::ClobberRefused { path } => write!(
                f,
                "Refusing to overwrite existing file {} \
                 (--no-clobber or output.no_clobber is set). \
                 Remove the file or unset the flag to proceed.",
                path.display()
            ),
            Self::UnrecoverableSyncLoss {
                offset,
                sync_losses,
            } => write!(
                f,
                "Unrecoverable mid-file sync loss at offset 0x{offset:X} \
                 after {sync_losses} recovery attempt(s); the decoder could \
                 not reacquire sync within the scan window. \
                 Pass --allow-partial to keep what was decoded as a .partial file."
            ),
        }
    }
}

impl std::error::Error for MieError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::FileIo { source, .. } | Self::WriterError { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Result type alias for decoder operations.
pub type MieResult<T> = std::result::Result<T, MieError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_includes_offset_in_hex() {
        let err = MieError::InvalidTypeWord {
            offset: 0xABCD,
            raw_type_word: 0x1234,
            word_count: 0,
        };
        let s = err.to_string();
        assert!(s.contains("0xABCD"));
        assert!(s.contains("0x1234"));
    }

    #[test]
    fn kind_classification() {
        let e = MieError::FileEmpty {
            path: PathBuf::from("/x"),
        };
        assert!(e.is_file_error());
        assert!(!e.is_record_error());

        let e = MieError::PayloadError {
            offset: 0,
            detail: "x".into(),
        };
        assert!(!e.is_file_error());
        assert!(e.is_record_error());
    }

    #[test]
    fn source_chain_for_io_errors() {
        let io_err = io::Error::new(io::ErrorKind::PermissionDenied, "nope");
        let e = MieError::FileIo {
            path: PathBuf::from("/x"),
            source: io_err,
        };
        assert!(std::error::Error::source(&e).is_some());
    }
}
