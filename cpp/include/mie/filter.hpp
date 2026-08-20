// SPDX-License-Identifier: Apache-2.0
//
// Message filter configuration.
//
// DATA ONLY, FOR NOW. This header carries the filter *sets* and the predicate
// that decides whether one message survives them. The pipeline stage that
// applies it to a record stream -- the counterpart of Rust's
// `FilterIterExt::filter_messages` -- lands with the rest of the filter module.
//
// It exists ahead of that stage because `config` populates these sets from a
// `[filter]` section, and a config loader that had to invent its own container
// for them would be the second definition of what a filter is.
//
// Mirrors the data half of `rust/src/filter.rs` and
// `python/src/mie_decoder/filters.py`.

#ifndef MIE_FILTER_HPP
#define MIE_FILTER_HPP

#include <cstdint>
#include <vector>

#include "mie/models.hpp"

namespace mie {

/// Which messages to keep.
///
/// EXCLUDE and INCLUDE are both supported, and they are not opposites. An
/// empty include set means "no include constraint", not "include nothing" --
/// otherwise a config that set only `exclude_rts` would drop every record.
/// Where both are present the include set is checked first and exclusion still
/// applies, so the narrower rule wins.
struct FilterConfig {
    std::vector<uint8_t> exclude_types;
    std::vector<uint8_t> exclude_rts;
    std::vector<Bus> exclude_buses;
    std::vector<uint8_t> exclude_subaddresses;

    std::vector<uint8_t> include_types;
    std::vector<uint8_t> include_rts;
    std::vector<Bus> include_buses;
    std::vector<uint8_t> include_subaddresses;

    FilterConfig();

    /// True when any set is populated, so the caller can skip the stage
    /// entirely rather than run a predicate that cannot reject anything.
    bool is_active() const;
};

}  // namespace mie

#endif  // MIE_FILTER_HPP
