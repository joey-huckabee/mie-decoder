//! MIE-Decoder: parser for DDC MIL-STD-1553 MIE binary recording files.
//!
//! See [`docs/ARCHITECTURE.md`](../docs/ARCHITECTURE.md) for the module
//! diagram and synchronization strategy.

pub mod cli;
pub mod config;
pub mod decode;
pub mod dump;
pub mod error;
pub mod filter;
pub mod log;
pub mod models;
pub mod reader;
pub mod sync;
pub mod writer;

pub use reader::{MieFileReader, ReaderOptions};

pub use error::{MieError, MieErrorKind, MieResult};
pub use models::{
    Bus, CommandWord, DataWords, Direction, ErrorMode, IrigTimestamp, MessageFormat, MessageType,
    MieMessage, StandardTimestamp, Timestamp, TimestampFormat, TypeWord,
};
