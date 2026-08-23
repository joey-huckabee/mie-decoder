// SPDX-License-Identifier: Apache-2.0

#include "mie/delta.hpp"

namespace mie {

uint32_t delta_key(uint8_t rt, uint8_t subaddress, bool transmit) {
    return (static_cast<uint32_t>(rt) << 16) | (static_cast<uint32_t>(subaddress) << 8) |
           static_cast<uint32_t>(transmit ? 1 : 0);
}

DeltaOutcome::DeltaOutcome()
    : kind(DELTA_NO_KEY), seconds(0.0), prev_us(0), curr_us(0), key(0), first_for_key(false) {}

Optional<double> DeltaOutcome::value() const {
    if (kind == DELTA_FIRST) {
        return Optional<double>(0.0);
    }
    if (kind == DELTA_ELAPSED) {
        return Optional<double>(seconds);
    }
    return Optional<double>();
}

DeltaTracker::DeltaTracker() : last_us_(), warned_keys_(), tick_rate_hz_() {}

DeltaTracker::DeltaTracker(const Optional<double>& tick_rate_hz)
    : last_us_(), warned_keys_(), tick_rate_hz_(tick_rate_hz) {}

DeltaOutcome DeltaTracker::observe(const CommandWord* command_word, const Timestamp& timestamp) {
    DeltaOutcome outcome;
    if (command_word == nullptr) {
        outcome.kind = DELTA_NO_KEY;
        return outcome;
    }

    uint64_t curr_us = 0;
    if (!timestamp.to_microseconds(tick_rate_hz_, curr_us)) {
        // No microsecond basis -- a Standard counter with no configured tick
        // rate. Nothing is recorded either: an entry here would hand the next
        // record a baseline that means nothing.
        outcome.kind = DELTA_UNCALIBRATED;
        return outcome;
    }

    const uint32_t key = delta_key(command_word->rt, command_word->subaddress,
                                   command_word->direction == DIRECTION_TRANSMIT);

    const std::map<uint32_t, uint64_t>::iterator found = last_us_.find(key);
    const bool seen_before = found != last_us_.end();
    const uint64_t prev_us = seen_before ? found->second : 0;

    // Unconditional, and deliberately so on the backward path too: the next
    // record for this key is measured from THIS one. Keeping the older, larger
    // value would report a gap that no pair of records in the file actually
    // has.
    if (seen_before) {
        found->second = curr_us;
    } else {
        last_us_.insert(std::make_pair(key, curr_us));
    }

    if (!seen_before) {
        outcome.kind = DELTA_FIRST;
        return outcome;
    }
    if (curr_us >= prev_us) {
        outcome.kind = DELTA_ELAPSED;
        outcome.seconds = static_cast<double>(curr_us - prev_us) / 1000000.0;
        return outcome;
    }

    outcome.kind = DELTA_BACKWARD;
    outcome.prev_us = prev_us;
    outcome.curr_us = curr_us;
    outcome.key = key;
    outcome.first_for_key = warned_keys_.insert(key).second;
    return outcome;
}

}  // namespace mie
