// SPDX-License-Identifier: Apache-2.0
//
// Test-only builders for MIE records on the wire.
//
// These encode the binary format -- Type Word bit layout, the IRIG triple, the
// Command Word -- and several suites need them. Copies had begun to accumulate,
// which is the drift `writer_fixtures.hpp` already exists to prevent: two
// suites encoding the format slightly differently disagree for reasons that
// have nothing to do with the code under test, and the one that is WRONG is the
// one that keeps passing.
//
// `docs/MIE-FORMAT.md` is the normative description; this is that description
// expressed as code, and nothing here may drift from it.
//
// Not every suite has been migrated yet -- `test_reader.cpp`, `test_sync.cpp`
// and `test_decode.cpp` still carry their own copies, along with the
// specialised builders their cases need.

#ifndef MIE_TESTS_RECORD_FIXTURES_HPP
#define MIE_TESTS_RECORD_FIXTURES_HPP

#include <catch2/catch.hpp>

#include <cstddef>
#include <vector>

#include "mie/models.hpp"

namespace mie_test {

/// Words to little-endian bytes, which is how the card writes them.
inline std::vector<uint8_t> le_bytes(const std::vector<uint16_t>& words) {
    std::vector<uint8_t> out;
    out.reserve(words.size() * 2);
    for (std::size_t i = 0; i < words.size(); ++i) {
        out.push_back(static_cast<uint8_t>(words[i] & 0xFF));
        out.push_back(static_cast<uint8_t>((words[i] >> 8) & 0xFF));
    }
    return out;
}

/// Type Word: type in bits 0-6, bus in 7, word count in 8-13, error in 14.
///
/// The word count covers the WHOLE record -- type word, timestamp, command,
/// payload and status -- not the payload alone. Writing the payload count
/// produces a file whose first record ends early, and the reader then reads the
/// command word as the next type word and reports a sync loss that has nothing
/// to do with what is being tested.
inline uint16_t type_word(uint8_t message_type, uint16_t word_count, bool error = false,
                          mie::Bus bus = mie::BUS_A) {
    uint16_t value = static_cast<uint16_t>(message_type & 0x7F);
    if (bus == mie::BUS_B) {
        value = static_cast<uint16_t>(value | 0x0080);
    }
    value = static_cast<uint16_t>(value | ((word_count & 0x3F) << 8));
    if (error) {
        value = static_cast<uint16_t>(value | 0x4000);
    }
    return value;
}

/// Command Word: RT in 11-15, direction in 10, subaddress in 5-9, data word
/// count in 0-4 -- where a raw 0 means 32, so 32 is written as 0.
inline uint16_t command_word(uint8_t rt, mie::Direction direction, uint8_t subaddress,
                             uint8_t data_word_count) {
    uint16_t value = static_cast<uint16_t>((rt & 0x1F) << 11);
    if (direction == mie::DIRECTION_TRANSMIT) {
        value = static_cast<uint16_t>(value | 0x0400);
    }
    value = static_cast<uint16_t>(value | ((subaddress & 0x1F) << 5));
    return static_cast<uint16_t>(value | (data_word_count & 0x1F));
}

/// Status Word carrying the RT address in bits 11-15. Matching the Command
/// Word's RT is what keeps the L2-SYN-024 anomaly check quiet.
inline uint16_t status_word(uint8_t rt) { return static_cast<uint16_t>((rt & 0x1F) << 11); }

/// The three IRIG words. Defaults sit inside every validated range, so a test
/// that cares about one field can vary that field alone.
inline void push_irig(std::vector<uint16_t>& words, uint32_t microsecond = 0, uint16_t day = 10,
                      uint8_t hour = 15, uint8_t minute = 54, uint8_t second = 50,
                      bool freerun = false) {
    // The microsecond field is 20 bits -- a high nibble in the middle word plus
    // the whole lower word -- so anything from 1048576 up silently wraps into a
    // DIFFERENT valid time. A fixture written with 1250000 encodes as 201936,
    // decodes without complaint, and quietly turns a forward gap into a
    // backward one. Caught here rather than debugged there.
    REQUIRE(microsecond < 1000000u);
    uint16_t upper = static_cast<uint16_t>(hour & 0x1F);
    upper = static_cast<uint16_t>(upper | ((day & 0x1FF) << 5));
    if (freerun) {
        upper = static_cast<uint16_t>(upper | 0x8000);
    }
    uint16_t middle = static_cast<uint16_t>((microsecond >> 16) & 0xF);
    middle = static_cast<uint16_t>(middle | ((second & 0x3F) << 4));
    middle = static_cast<uint16_t>(middle | ((minute & 0x3F) << 10));

    words.push_back(upper);
    words.push_back(middle);
    words.push_back(static_cast<uint16_t>(microsecond & 0xFFFF));
}

/// The two Standard timestamp words, upper first.
inline void push_standard(std::vector<uint16_t>& words, uint32_t ticks) {
    words.push_back(static_cast<uint16_t>((ticks >> 16) & 0xFFFF));
    words.push_back(static_cast<uint16_t>(ticks & 0xFFFF));
}

/// Payload words derived from the record's own time value.
///
/// Not decoration. L2-SYN-018 rejects an input whose first four candidate
/// records are byte-identical OUTSIDE the timestamp -- the 0x20-fill defence --
/// and the check deliberately ignores the timestamp triple, so four copies of
/// one record with only the clock advancing trips it. That is the check
/// working. Real bus traffic does not repeat a payload byte for byte either.
inline uint16_t payload_base(uint16_t family, uint32_t time_value) {
    return static_cast<uint16_t>(family | (time_value & 0xFF));
}

/// BC-to-RT: Type, IRIG, Cmd(Receive), data..., Status.
inline std::vector<uint16_t> bc_to_rt(uint8_t rt, uint8_t subaddress, uint8_t data_words,
                                      uint32_t microsecond = 0) {
    const uint16_t fill = payload_base(0x1100, microsecond);
    std::vector<uint16_t> words;
    words.push_back(type_word(mie::MESSAGE_TYPE_BC_TO_RT, static_cast<uint16_t>(6 + data_words)));
    push_irig(words, microsecond);
    words.push_back(command_word(rt, mie::DIRECTION_RECEIVE, subaddress, data_words));
    for (uint8_t i = 0; i < data_words; ++i) {
        words.push_back(static_cast<uint16_t>(fill + i));
    }
    words.push_back(status_word(rt));
    return words;
}

/// RT-to-BC: Type, IRIG, Cmd(Transmit), Status, data... -- the status comes
/// BEFORE the payload here, which is the whole difference from BC-to-RT.
inline std::vector<uint16_t> rt_to_bc(uint8_t rt, uint8_t subaddress, uint8_t data_words,
                                      uint32_t microsecond = 0) {
    const uint16_t fill = payload_base(0x2200, microsecond);
    std::vector<uint16_t> words;
    words.push_back(type_word(mie::MESSAGE_TYPE_RT_TO_BC, static_cast<uint16_t>(6 + data_words)));
    push_irig(words, microsecond);
    words.push_back(command_word(rt, mie::DIRECTION_TRANSMIT, subaddress, data_words));
    words.push_back(status_word(rt));
    for (uint8_t i = 0; i < data_words; ++i) {
        words.push_back(static_cast<uint16_t>(fill + i));
    }
    return words;
}

/// Append the end-of-records terminator and flatten to bytes.
///
/// A null Type Word ends the stream (there is no file header, and records start
/// at byte 0). A fixture without it decodes its records and then reports a sync
/// loss on whatever follows, which turns every test using it into a test of the
/// recovery path.
inline std::vector<uint8_t> finish(const std::vector<uint16_t>& words) {
    std::vector<uint16_t> all(words);
    all.push_back(0x0000);
    return le_bytes(all);
}

/// Concatenate record word-vectors.
inline void append(std::vector<uint16_t>& into, const std::vector<uint16_t>& record) {
    into.insert(into.end(), record.begin(), record.end());
}

}  // namespace mie_test

#endif  // MIE_TESTS_RECORD_FIXTURES_HPP
