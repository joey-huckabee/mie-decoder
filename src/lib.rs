//! MIE-Decoder: parser for DDC MIL-STD-1553 MIE binary recording files.
//!
//! See [`docs/ARCHITECTURE.md`](../docs/ARCHITECTURE.md) for the module
//! diagram and synchronization strategy.

pub mod decode;
pub mod error;
pub mod models;
pub mod sync;

pub use error::{MieError, MieErrorKind, MieResult};
pub use models::{
    Bus, CommandWord, DataWords, Direction, ErrorMode, IrigTimestamp, MessageFormat, MessageType,
    MieMessage, StandardTimestamp, Timestamp, TimestampFormat, TypeWord,
};
