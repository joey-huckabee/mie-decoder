//! Message filtering. Both `exclude_*` (negative) and `include_*`
//! (positive) filters are supported. A message passes if:
//!   - it matches no `exclude_*` set, AND
//!   - every active `include_*` set contains its value.
//!
//! Inactive sets (empty) are ignored on both sides.

use crate::models::{Bus, MieMessage};

#[derive(Debug, Clone, Default)]
pub struct FilterConfig {
    pub exclude_types: Vec<u8>,
    pub exclude_rts: Vec<u8>,
    pub exclude_buses: Vec<Bus>,
    pub exclude_subaddresses: Vec<u8>,

    pub include_types: Vec<u8>,
    pub include_rts: Vec<u8>,
    pub include_buses: Vec<Bus>,
    pub include_subaddresses: Vec<u8>,
}

impl FilterConfig {
    pub fn is_active(&self) -> bool {
        !self.exclude_types.is_empty()
            || !self.exclude_rts.is_empty()
            || !self.exclude_buses.is_empty()
            || !self.exclude_subaddresses.is_empty()
            || !self.include_types.is_empty()
            || !self.include_rts.is_empty()
            || !self.include_buses.is_empty()
            || !self.include_subaddresses.is_empty()
    }

    /// True if `msg` should be dropped from output.
    pub fn should_exclude(&self, msg: &MieMessage) -> bool {
        let message_type = msg.type_word.message_type;
        let bus = msg.type_word.bus;
        let (rt, subaddress) = match msg.command_word {
            Some(cw) => (Some(cw.rt), Some(cw.subaddress)),
            None => (None, None),
        };

        // Negative filters
        if self.exclude_types.contains(&message_type) {
            return true;
        }
        if let Some(rt) = rt {
            if self.exclude_rts.contains(&rt) {
                return true;
            }
        }
        if self.exclude_buses.contains(&bus) {
            return true;
        }
        if let Some(sa) = subaddress {
            if self.exclude_subaddresses.contains(&sa) {
                return true;
            }
        }

        // Positive filters: if active, value must be present.
        if !self.include_types.is_empty() && !self.include_types.contains(&message_type) {
            return true;
        }
        if !self.include_buses.is_empty() && !self.include_buses.contains(&bus) {
            return true;
        }
        if !self.include_rts.is_empty() {
            match rt {
                Some(rt) if self.include_rts.contains(&rt) => {}
                // SPURIOUS_DATA has no RT — drop when an include filter is set.
                _ => return true,
            }
        }
        if !self.include_subaddresses.is_empty() {
            match subaddress {
                Some(sa) if self.include_subaddresses.contains(&sa) => {}
                _ => return true,
            }
        }

        false
    }
}

/// Iterator adapter wrapping any `Iterator<Item = MieResult<MieMessage>>`.
pub struct Filtered<I> {
    inner: I,
    filters: FilterConfig,
}

impl<I, E> Iterator for Filtered<I>
where
    I: Iterator<Item = Result<MieMessage, E>>,
{
    type Item = Result<MieMessage, E>;
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.inner.next()? {
                Err(e) => return Some(Err(e)),
                Ok(msg) => {
                    if !self.filters.should_exclude(&msg) {
                        return Some(Ok(msg));
                    }
                }
            }
        }
    }
}

/// Extension trait: `iter.filter_messages(cfg)`.
pub trait FilterIterExt: Sized {
    fn filter_messages(self, filters: FilterConfig) -> Filtered<Self>;
}

impl<I, E> FilterIterExt for I
where
    I: Iterator<Item = Result<MieMessage, E>>,
{
    fn filter_messages(self, filters: FilterConfig) -> Filtered<Self> {
        Filtered {
            inner: self,
            filters,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::*;

    fn msg(rt: u8, sa: u8, bus: Bus, mt: u8) -> MieMessage {
        MieMessage {
            timestamp: Timestamp::Standard(StandardTimestamp {
                raw_value: 0,
                upper_word: 0,
                lower_word: 0,
            }),
            type_word: TypeWord {
                message_type: mt,
                bus,
                word_count: 5,
                error: false,
                raw: 0,
            },
            message_format: MessageFormat::Receive,
            command_word: Some(CommandWord {
                rt,
                direction: Direction::Receive,
                subaddress: sa,
                data_word_count: 1,
                raw: 0,
            }),
            command_word_2: None,
            status_word: None,
            status_word_2: None,
            data_words: DataWords::new(),
            error_word: None,
            delta: Some(0.0),
            file_offset: 0,
        }
    }

    /// Requirements: L2-FLT-001
    #[test]
    fn empty_config_is_inactive() {
        let cfg = FilterConfig::default();
        assert!(!cfg.is_active());
        assert!(!cfg.should_exclude(&msg(1, 1, Bus::A, 0x02)));
    }

    /// Requirements: L2-CFG-006
    #[test]
    fn exclude_by_rt() {
        let cfg = FilterConfig {
            exclude_rts: vec![31],
            ..Default::default()
        };
        assert!(cfg.should_exclude(&msg(31, 0, Bus::A, 0x02)));
        assert!(!cfg.should_exclude(&msg(15, 0, Bus::A, 0x02)));
    }

    /// Requirements: L2-CFG-006
    #[test]
    fn exclude_by_type_and_bus() {
        let cfg = FilterConfig {
            exclude_types: vec![0x20],
            exclude_buses: vec![Bus::B],
            ..Default::default()
        };
        assert!(cfg.should_exclude(&msg(1, 1, Bus::A, 0x20)));
        assert!(cfg.should_exclude(&msg(1, 1, Bus::B, 0x02)));
        assert!(!cfg.should_exclude(&msg(1, 1, Bus::A, 0x02)));
    }

    /// Requirements: L3-RS-010
    #[test]
    fn include_filters_drop_non_matches() {
        let cfg = FilterConfig {
            include_rts: vec![15],
            ..Default::default()
        };
        assert!(!cfg.should_exclude(&msg(15, 0, Bus::A, 0x02)));
        assert!(cfg.should_exclude(&msg(14, 0, Bus::A, 0x02)));
    }

    /// Requirements: L2-FLT-001
    #[test]
    fn iterator_adapter() {
        let msgs: Vec<Result<MieMessage, ()>> = vec![
            Ok(msg(15, 1, Bus::A, 0x02)),
            Ok(msg(31, 1, Bus::A, 0x02)),
            Ok(msg(0, 1, Bus::B, 0x02)),
        ];
        let cfg = FilterConfig {
            exclude_rts: vec![31],
            ..Default::default()
        };
        let filtered: Vec<_> = msgs
            .into_iter()
            .filter_messages(cfg)
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].rt(), Some(15));
        assert_eq!(filtered[1].rt(), Some(0));
    }
}
