// SPDX-License-Identifier: Apache-2.0
//
// Per-RT/MSG DELTA tracking (L2-RDR-009/010/017/018/019, L3-RDR-001).
//
// Mirrors the Rust unit tests in `rust/src/delta.rs` and the Python ones in
// `python/tests/test_delta.py` case for case, so a divergence between the three
// implementations shows up as a failing test on one side rather than as a
// conformance-oracle mismatch later.
//
// The last case is the one that could not exist before the extraction: while
// the packed tracking key lived inside the reader and the display key lived in
// models, there was nowhere both were in scope to be compared.

#include "mie/delta.hpp"

#include <catch2/catch.hpp>

#include <cstddef>
#include <map>
#include <string>
#include <utility>

#include "mie/models.hpp"

namespace {

/// A Command Word carrying just the three fields the DELTA key uses.
mie::CommandWord cmd(uint8_t rt, uint8_t subaddress, bool transmit) {
    return mie::CommandWord(rt, transmit ? mie::DIRECTION_TRANSMIT : mie::DIRECTION_RECEIVE,
                            subaddress, 2, 0);
}

/// An IRIG timestamp `micros` microseconds into day 10.
mie::Timestamp at(uint32_t micros) {
    return mie::Timestamp::from_irig(mie::IrigTimestamp(
        10, 0, 0, static_cast<uint8_t>(micros / 1000000), micros % 1000000, false));
}

/// The absolute microseconds `at(micros)` decodes to.
///
/// IRIG timestamps are absolute -- day 10 alone contributes 864 000 000 000
/// microseconds -- so a test that expects to see its own argument back is
/// asserting against a number the tracker never had. Derived rather than
/// written out, so the two cannot disagree.
uint64_t us(uint32_t micros) {
    uint64_t value = 0;
    REQUIRE(at(micros).to_microseconds(mie::Optional<double>(), value));
    return value;
}

/// Observe with a Command Word, keeping it alive across the call.
///
/// `observe` takes a pointer because that is C++'s spelling of "an optional
/// reference", and `&cmd(...)` would be taking the address of a temporary --
/// ill-formed, not merely dangerous. One helper beats a named local at every
/// call site.
mie::DeltaOutcome observe(mie::DeltaTracker& tracker, uint8_t rt, uint8_t subaddress, bool transmit,
                          const mie::Timestamp& timestamp) {
    const mie::CommandWord command = cmd(rt, subaddress, transmit);
    return tracker.observe(&command, timestamp);
}

/// The DELTA the column would show, or -1.0 for an empty cell. Keeps the
/// assertions below readable without an Optional dance at every site.
double column(const mie::DeltaOutcome& outcome) {
    const mie::Optional<double> value = outcome.value();
    return value.has_value() ? value.value() : -1.0;
}

}  // namespace

TEST_CASE("the first sighting of a key is zero", "[delta][L2-RDR-010]") {
    mie::DeltaTracker tracker;
    const mie::DeltaOutcome outcome = observe(tracker, 3, 5, false, at(0));
    CHECK(outcome.kind == mie::DELTA_FIRST);
    CHECK(column(outcome) == Approx(0.0));
}

TEST_CASE("a later record reports the gap in seconds", "[delta][L2-RDR-009]") {
    mie::DeltaTracker tracker;
    observe(tracker, 3, 5, false, at(0));
    const mie::DeltaOutcome outcome = observe(tracker, 3, 5, false, at(250000));
    CHECK(outcome.kind == mie::DELTA_ELAPSED);
    CHECK(column(outcome) == Approx(0.25));
}

TEST_CASE("keys are tracked independently", "[delta][L2-RDR-009]") {
    // Interleaved traffic from two terminals. A single cursor would make every
    // gap the inter-record spacing rather than the per-key period -- a
    // plausible-looking wrong answer.
    mie::DeltaTracker tracker;
    CHECK(column(observe(tracker, 3, 5, false, at(0))) == Approx(0.0));
    CHECK(column(observe(tracker, 9, 1, false, at(100000))) == Approx(0.0));
    CHECK(column(observe(tracker, 3, 5, false, at(200000))) == Approx(0.2));
    CHECK(column(observe(tracker, 9, 1, false, at(300000))) == Approx(0.2));
}

TEST_CASE("direction is part of the key", "[delta][L2-RDR-009]") {
    // Same RT, same subaddress, opposite direction: two different messages on
    // the bus, so two independent periods.
    mie::DeltaTracker tracker;
    CHECK(column(observe(tracker, 3, 5, false, at(0))) == Approx(0.0));
    CHECK(column(observe(tracker, 3, 5, true, at(100000))) == Approx(0.0));
    CHECK(column(observe(tracker, 3, 5, false, at(400000))) == Approx(0.4));
}

TEST_CASE("a backward step reports no gap and flags only the first", "[delta][L2-RDR-017]") {
    mie::DeltaTracker tracker;
    observe(tracker, 3, 5, false, at(500000));

    const mie::DeltaOutcome first = observe(tracker, 3, 5, false, at(100000));
    CHECK(first.kind == mie::DELTA_BACKWARD);
    CHECK(first.prev_us == us(500000));
    CHECK(first.curr_us == us(100000));
    CHECK(first.first_for_key);
    CHECK_FALSE(first.value().has_value());

    // Reported against the PREVIOUS record, not the high-water mark.
    const mie::DeltaOutcome second = observe(tracker, 3, 5, false, at(50000));
    CHECK(second.kind == mie::DELTA_BACKWARD);
    CHECK(second.prev_us == us(100000));
    CHECK_FALSE(second.first_for_key);
}

TEST_CASE("the cursor advances across a backward step", "[delta][L2-RDR-017]") {
    // The recovery is measured from the last record SEEN, not from the
    // high-water mark -- otherwise the gap reported would be one no pair of
    // records in the file actually has.
    mie::DeltaTracker tracker;
    observe(tracker, 3, 5, false, at(500000));
    observe(tracker, 3, 5, false, at(100000));
    const mie::DeltaOutcome outcome = observe(tracker, 3, 5, false, at(600000));
    CHECK(outcome.kind == mie::DELTA_ELAPSED);
    CHECK(column(outcome) == Approx(0.5));
}

TEST_CASE("each key gets its own first-backward flag", "[delta][L2-RDR-017]") {
    mie::DeltaTracker tracker;
    observe(tracker, 3, 5, false, at(500000));
    observe(tracker, 9, 1, false, at(500000));

    const mie::DeltaOutcome a = observe(tracker, 3, 5, false, at(100000));
    const mie::DeltaOutcome b = observe(tracker, 9, 1, false, at(100000));
    CHECK(a.kind == mie::DELTA_BACKWARD);
    CHECK(a.first_for_key);
    CHECK(b.kind == mie::DELTA_BACKWARD);
    CHECK(b.first_for_key);
}

TEST_CASE("a record with no Command Word is never tracked", "[delta][L2-RDR-018]") {
    mie::DeltaTracker tracker;
    const mie::DeltaOutcome outcome = tracker.observe(0, at(0));
    CHECK(outcome.kind == mie::DELTA_NO_KEY);
    CHECK_FALSE(outcome.value().has_value());
    // And it left no cursor behind for a real key to trip over.
    CHECK(observe(tracker, 3, 5, false, at(0)).kind == mie::DELTA_FIRST);
}

TEST_CASE("an uncalibrated Standard counter is not tracked", "[delta][L2-RDR-019]") {
    mie::DeltaTracker tracker;
    const mie::Timestamp ticks =
        mie::Timestamp::from_standard(mie::StandardTimestamp(1000, 0, 1000));
    const mie::DeltaOutcome outcome = observe(tracker, 3, 5, false, ticks);
    CHECK(outcome.kind == mie::DELTA_UNCALIBRATED);
    CHECK_FALSE(outcome.value().has_value());
}

TEST_CASE("a calibrated Standard counter is tracked like IRIG", "[delta][L2-DEC-017][L2-RDR-019]") {
    mie::DeltaTracker tracker((mie::Optional<double>(1000000.0)));
    const mie::Timestamp zero = mie::Timestamp::from_standard(mie::StandardTimestamp(0, 0, 0));
    const mie::Timestamp one_second =
        mie::Timestamp::from_standard(mie::StandardTimestamp(1000000, 0x000F, 0x4240));

    CHECK(column(observe(tracker, 3, 5, false, zero)) == Approx(0.0));
    CHECK(column(observe(tracker, 3, 5, false, one_second)) == Approx(1.0));
}

TEST_CASE("the packed key and the display key agree",
          "[delta][L2-RDR-009][L2-MSG-003][L3-RDR-001]") {
    // If one ever collapses two distinct messages that the other keeps apart,
    // DELTA means different things on the single-file and merge paths. This
    // could not be written while the two representations lived in different
    // modules; it is the check whose absence motivated the extraction.
    std::map<uint32_t, std::string> packed_to_display;
    std::map<std::string, uint32_t> display_to_packed;

    for (uint8_t rt = 0; rt < 32; ++rt) {
        for (uint8_t subaddress = 0; subaddress < 32; ++subaddress) {
            for (int t = 0; t < 2; ++t) {
                const bool transmit = t == 1;
                const uint32_t packed = mie::delta_key(rt, subaddress, transmit);

                mie::MieMessage message;
                message.command_word = cmd(rt, subaddress, transmit);
                message.message_format = mie::FORMAT_RECEIVE;
                const std::string display = message.delta_key();

                const std::pair<std::map<uint32_t, std::string>::iterator, bool> p =
                    packed_to_display.insert(std::make_pair(packed, display));
                CHECK(p.first->second == display);

                const std::pair<std::map<std::string, uint32_t>::iterator, bool> d =
                    display_to_packed.insert(std::make_pair(display, packed));
                CHECK(d.first->second == packed);
            }
        }
    }

    // Computed in size_t, not in unsigned int and then widened: the product is
    // small here, but a multiplication that wraps BEFORE the widening is a real
    // class of bug and clang-tidy is right to refuse to distinguish this case
    // from one that matters.
    const std::size_t expected = static_cast<std::size_t>(32) * 32 * 2;
    CHECK(packed_to_display.size() == expected);
    CHECK(display_to_packed.size() == expected);
}
