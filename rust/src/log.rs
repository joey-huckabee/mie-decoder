//! Tiny stderr logger. ~50 lines, no facade trait, no external crate.
//!
//! A single global level controls emission across all modules. The macros
//! `debug!`, `info!`, `warn!`, `error!` defined in this crate format with
//! `format!` only when the level passes the filter, so they're cheap when
//! disabled.

use std::sync::atomic::{AtomicU8, Ordering};

/// Log severity. Higher numeric value = more important.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum Level {
    Debug = 0,
    Info = 1,
    Warn = 2,
    Error = 3,
    Off = 4,
}

impl Level {
    pub fn parse(name: &str) -> Option<Self> {
        match name.to_ascii_uppercase().as_str() {
            "DEBUG" => Some(Self::Debug),
            "INFO" => Some(Self::Info),
            "WARNING" | "WARN" => Some(Self::Warn),
            "ERROR" => Some(Self::Error),
            "CRITICAL" | "OFF" => Some(Self::Off),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Debug => "DEBUG",
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
            Self::Off => "OFF",
        }
    }
}

/// Default to WARN, matching the Python CLI default.
static LEVEL: AtomicU8 = AtomicU8::new(Level::Warn as u8);

pub fn set_level(level: Level) {
    LEVEL.store(level as u8, Ordering::Relaxed);
}

#[inline]
pub fn current_level() -> Level {
    match LEVEL.load(Ordering::Relaxed) {
        0 => Level::Debug,
        1 => Level::Info,
        2 => Level::Warn,
        3 => Level::Error,
        _ => Level::Off,
    }
}

#[inline]
pub fn enabled(level: Level) -> bool {
    (level as u8) >= LEVEL.load(Ordering::Relaxed)
}

/// Internal write — used by macros. `args` is already-formatted message text.
pub fn _emit(level: Level, module: &str, args: std::fmt::Arguments<'_>) {
    if !enabled(level) {
        return;
    }
    let _ = std::io::Write::write_fmt(
        &mut std::io::stderr().lock(),
        format_args!("{} [{}] {}\n", level.label(), module, args),
    );
}

#[macro_export]
macro_rules! log_debug {
    ($($arg:tt)*) => {
        $crate::log::_emit($crate::log::Level::Debug, module_path!(), format_args!($($arg)*))
    };
}
#[macro_export]
macro_rules! log_info {
    ($($arg:tt)*) => {
        $crate::log::_emit($crate::log::Level::Info, module_path!(), format_args!($($arg)*))
    };
}
#[macro_export]
macro_rules! log_warn {
    ($($arg:tt)*) => {
        $crate::log::_emit($crate::log::Level::Warn, module_path!(), format_args!($($arg)*))
    };
}
#[macro_export]
macro_rules! log_error {
    ($($arg:tt)*) => {
        $crate::log::_emit($crate::log::Level::Error, module_path!(), format_args!($($arg)*))
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Requirements: L2-CLI-004
    #[test]
    fn level_parse() {
        assert_eq!(Level::parse("DEBUG"), Some(Level::Debug));
        assert_eq!(Level::parse("warning"), Some(Level::Warn));
        assert_eq!(Level::parse("warn"), Some(Level::Warn));
        // CRITICAL and OFF both map to Off (silence all output).
        assert_eq!(Level::parse("CRITICAL"), Some(Level::Off));
        assert_eq!(Level::parse("OFF"), Some(Level::Off));
        assert_eq!(Level::parse("off"), Some(Level::Off));
        assert_eq!(Level::parse("nope"), None);
    }

    /// Requirements: L1-LOG-001
    #[test]
    fn level_ordering() {
        assert!(Level::Debug < Level::Info);
        assert!(Level::Warn < Level::Error);
    }
}
