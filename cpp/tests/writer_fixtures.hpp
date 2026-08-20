// SPDX-License-Identifier: Apache-2.0
//
// Test-only fixtures shared by the two writer suites.
//
// `test_writer_rows.cpp` asserts on formatted strings and needs no filesystem;
// `test_writer.cpp` asserts on files that were actually written. They want the
// same three records, so those live here rather than being copied — a fixture
// that drifts between two suites makes them disagree for reasons that have
// nothing to do with the code under test.

#ifndef MIE_TESTS_WRITER_FIXTURES_HPP
#define MIE_TESTS_WRITER_FIXTURES_HPP

#include <cstddef>
#include <string>
#include <vector>

#include "mie/models.hpp"

namespace mie_test {

/// Split on `delimiter`, keeping empty fields — a CSV row is mostly empty
/// cells, so a splitter that drops them would hide every column-shift bug.
inline std::vector<std::string> split(const std::string& text, char delimiter) {
    std::vector<std::string> parts;
    std::string current;
    for (std::size_t i = 0; i < text.size(); ++i) {
        if (text[i] == delimiter) {
            parts.push_back(current);
            current.clear();
        } else {
            current += text[i];
        }
    }
    parts.push_back(current);
    return parts;
}

/// Drop trailing CR and LF.
inline std::string chomp(const std::string& text) {
    std::string out(text);
    while (!out.empty() && (out[out.size() - 1] == '\n' || out[out.size() - 1] == '\r')) {
        out.erase(out.size() - 1);
    }
    return out;
}

/// A clean BC-to-RT record with two data words.
inline mie::MieMessage sample(uint32_t microsecond = 100) {
    mie::MieMessage message;
    message.timestamp =
        mie::Timestamp::from_irig(mie::IrigTimestamp(192, 15, 54, 50, microsecond, false));
    message.type_word = mie::TypeWord(mie::MESSAGE_TYPE_BC_TO_RT, mie::BUS_A, 8, false, 0x0802);
    message.message_format = mie::FORMAT_RECEIVE;
    message.command_word = mie::CommandWord(15, mie::DIRECTION_RECEIVE, 11, 2, 0x7962);
    message.status_word = static_cast<uint16_t>(0x7800);
    const uint16_t words[2] = {0x1234, 0xABCD};
    message.data_words = mie::DataWords::from_words(words, 2);
    message.delta = 0.25;
    return message;
}

/// SPURIOUS_DATA: no Command Word, so no RT, no MSG and no DELTA. On bus B, so
/// the BUS column proves it reads the Type Word rather than the Command Word.
inline mie::MieMessage spurious() {
    mie::MieMessage message;
    message.timestamp = mie::Timestamp::from_irig(mie::IrigTimestamp(192, 15, 54, 50, 200, false));
    message.type_word =
        mie::TypeWord(mie::MESSAGE_TYPE_SPURIOUS_DATA, mie::BUS_B, 6, false, 0x0620);
    message.message_format = mie::FORMAT_SPURIOUS_DATA;
    message.error_word = mie::ERROR_SPURIOUS_STANDALONE;
    return message;
}

/// An errored record, carrying a DELTA because errored records still take part
/// in tracking (L2-RDR-016).
inline mie::MieMessage errored() {
    mie::MieMessage message;
    message.timestamp = mie::Timestamp::from_irig(mie::IrigTimestamp(192, 15, 54, 50, 300, false));
    message.type_word = mie::TypeWord(mie::MESSAGE_TYPE_BC_TO_RT, mie::BUS_A, 8, true, 0x4802);
    message.message_format = mie::FORMAT_RECEIVE;
    message.command_word = mie::CommandWord(6, mie::DIRECTION_RECEIVE, 2, 2, 0x3042);
    message.error_word = mie::ERROR_MANCHESTER_PARITY;
    message.delta = 0.0;
    return message;
}

}  // namespace mie_test

#endif  // MIE_TESTS_WRITER_FIXTURES_HPP
