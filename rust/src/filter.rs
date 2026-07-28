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
        if let Some(rt) = rt
            && self.exclude_rts.contains(&rt)
        {
            return true;
        }
        if self.exclude_buses.contains(&bus) {
            return true;
        }
        if let Some(sa) = subaddress
            && self.exclude_subaddresses.contains(&sa)
        {
            return true;
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
///
/// Reports the same diagnostics as the Python `apply_filters` generator: the
/// active filter sets once at construction (INFO), each dropped record (DEBUG),
/// and a passed/excluded tally when the stream is finished (INFO). The tally is
/// emitted from `Drop` rather than at end-of-iteration so it still appears when
/// a consumer stops early — a broken pipe, `| head` — where an end-of-stream
/// hook would never run.
pub struct Filtered<I> {
    inner: I,
    filters: FilterConfig,
    passed: u64,
    excluded: u64,
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
                        self.passed += 1;
                        return Some(Ok(msg));
                    }
                    self.excluded += 1;
                    log_filtered_out(&msg);
                }
            }
        }
    }
}

impl<I> Drop for Filtered<I> {
    fn drop(&mut self) {
        if self.filters.is_active() {
            crate::log_info!(
                "Filter results: {} passed, {} excluded",
                self.passed,
                self.excluded
            );
        }
    }
}

/// One-time INFO summary of the configured filter sets. `none` for an inactive
/// (empty) set, matching the Python wording.
fn log_active_filters(f: &FilterConfig) {
    // Sorted so the line is stable regardless of the order values were parsed
    // in, and rendered identically on both implementations (Python holds these
    // as sets, whose repr order is not guaranteed).
    fn show<T: std::fmt::Display + Ord + Copy>(v: &[T]) -> String {
        if v.is_empty() {
            return "none".to_string();
        }
        let mut sorted: Vec<T> = v.to_vec();
        sorted.sort_unstable();
        let items: Vec<String> = sorted.iter().map(|x| x.to_string()).collect();
        format!("[{}]", items.join(", "))
    }
    crate::log_info!(
        "Filtering active: exclude_types={} exclude_rts={} exclude_buses={} \
         exclude_subaddresses={} include_types={} include_rts={} include_buses={} \
         include_subaddresses={}",
        show(&f.exclude_types),
        show(&f.exclude_rts),
        show(&f.exclude_buses),
        show(&f.exclude_subaddresses),
        show(&f.include_types),
        show(&f.include_rts),
        show(&f.include_buses),
        show(&f.include_subaddresses),
    );
}

/// DEBUG line for a message dropped by the filters.
fn log_filtered_out(msg: &MieMessage) {
    let (rt, sa) = match msg.command_word {
        Some(cw) => (cw.rt.to_string(), cw.subaddress.to_string()),
        None => ("-".to_string(), "-".to_string()),
    };
    crate::log_debug!(
        "Filtered out: offset=0x{:X} type=0x{:02X} RT{} SA{} Bus {}",
        msg.file_offset,
        msg.type_word.message_type,
        rt,
        sa,
        msg.type_word.bus
    );
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
        if filters.is_active() {
            log_active_filters(&filters);
        } else {
            crate::log_debug!("No filters active, passing all messages through");
        }
        Filtered {
            inner: self,
            filters,
            passed: 0,
            excluded: 0,
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
            mux: None,
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
