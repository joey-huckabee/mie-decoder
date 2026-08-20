// SPDX-License-Identifier: Apache-2.0
//
// Row-level CSV formatting.
//
// Split from test_writer.cpp, which covers whole-file writes: everything here
// goes through `format_row` and needs no filesystem at all. That is why
// `format_row` is public — every column decision lives in it, and a test that
// asserts on a string is both faster and clearer than one that asserts on a
// file it just wrote.

#include <catch2/catch.hpp>

#include <string>
#include <vector>

#include "mie/models.hpp"
#include "mie/text.hpp"
#include "mie/writer.hpp"
#include "writer_fixtures.hpp"

namespace {

using mie_test::chomp;
using mie_test::errored;
using mie_test::sample;
using mie_test::split;
using mie_test::spurious;

/// "01".."32" -- the two-digit form the WDnn column names use.
std::string two_digits(std::size_t value) {
    const std::string out = mie::text::decimal(static_cast<uint64_t>(value));
    return out.size() < 2 ? "0" + out : out;
}

}  // namespace

// ---------------------------------------------------------------------------
// The column contract
// ---------------------------------------------------------------------------

TEST_CASE("the vendor block precedes the decoder-added columns",
          "[writer][L1-OUT-001][L2-WRT-001]") {
    const std::vector<std::string> cols = split(chomp(mie::CSV_HEADER), ',');

    REQUIRE(cols.size() == mie::TOTAL_COLUMN_COUNT);
    REQUIRE(cols.size() - mie::VENDOR_COLUMN_COUNT == 2);

    // The vendor block ends at XMT_GAP...
    CHECK(cols[mie::VENDOR_COLUMN_COUNT - 1] == "XMT_GAP");
    // ...and the gap columns sit at their vendor indices (1-based 42/43/44),
    // which is exactly what the pre-v2.10.0 interleaved layout got wrong. Every
    // column NAME still matched then, so a name-only check passed while every
    // positional comparison past DELTA was wrong. Hence the index assertions.
    CHECK(cols[41] == "IM_GAP");
    CHECK(cols[42] == "RCV_GAP");
    CHECK(cols[43] == "XMT_GAP");
    // Decoder additions strictly at the tail.
    CHECK(cols[44] == "ERROR");
    CHECK(cols[45] == "ERROR_CODE");

    // And neither may appear anywhere inside the vendor block.
    for (std::size_t i = 0; i < mie::VENDOR_COLUMN_COUNT; ++i) {
        CHECK(cols[i] != "ERROR");
        CHECK(cols[i] != "ERROR_CODE");
    }
}

TEST_CASE("the header names every vendor column in order", "[writer][L2-WRT-001]") {
    const std::vector<std::string> cols = split(chomp(mie::CSV_HEADER), ',');
    CHECK(cols[0] == "TIME_STAMP");
    CHECK(cols[1] == "RT");
    CHECK(cols[2] == "MSG");
    for (std::size_t i = 0; i < mie::MAX_DATA_WORDS; ++i) {
        CHECK(cols[3 + i] == "WD" + two_digits(i + 1));
    }
    CHECK(cols[35] == "STAT");
    CHECK(cols[36] == "CMD");
    CHECK(cols[37] == "MUX");
    CHECK(cols[38] == "TERM_NAME");
    CHECK(cols[39] == "BUS");
    CHECK(cols[40] == "DELTA");
}

TEST_CASE("a row has exactly as many cells as the header", "[writer][L2-WRT-001]") {
    // The property that catches a missing or extra comma anywhere in
    // format_row, which no per-column assertion would notice.
    const std::size_t expected = split(chomp(mie::CSV_HEADER), ',').size();
    CHECK(split(chomp(mie::format_row(sample())), ',').size() == expected);
    CHECK(split(chomp(mie::format_row(spurious())), ',').size() == expected);
    CHECK(split(chomp(mie::format_row(errored())), ',').size() == expected);

    mie::MieMessage full = sample();
    uint16_t words[mie::MAX_DATA_WORDS];
    for (std::size_t i = 0; i < mie::MAX_DATA_WORDS; ++i) {
        words[i] = static_cast<uint16_t>(0x1000 + i);
    }
    full.data_words = mie::DataWords::from_words(words, mie::MAX_DATA_WORDS);
    CHECK(split(chomp(mie::format_row(full)), ',').size() == expected);
}

// ---------------------------------------------------------------------------
// Row content
// ---------------------------------------------------------------------------

TEST_CASE("a clean record fills the vendor columns it has", "[writer][L2-WRT-003]") {
    const std::vector<std::string> c = split(chomp(mie::format_row(sample())), ',');

    CHECK(c[0] == "192:15:54:50.000100");
    CHECK(c[1] == "15");
    CHECK(c[2] == "11R");
    CHECK(c[3] == "1234");
    CHECK(c[4] == "ABCD");
    // The payload is two words, so the other thirty cells are present and
    // empty. A short payload must not shift STAT left.
    for (std::size_t i = 5; i < 3 + mie::MAX_DATA_WORDS; ++i) {
        CHECK(c[i].empty());
    }
    CHECK(c[35] == "7800");  // STAT
    CHECK(c[36] == "7962");  // CMD -- the RAW word, not the decoded fields
    CHECK(c[37].empty());    // MUX, unset on this record
    CHECK(c[38].empty());    // TERM_NAME, always empty (L2-WRT-013)
    CHECK(c[39] == "A");     // BUS
    CHECK(c[40] == "0.250000");
    CHECK(c[41].empty());  // IM_GAP
    CHECK(c[42].empty());  // RCV_GAP
    CHECK(c[43].empty());  // XMT_GAP
    CHECK(c[44].empty());  // ERROR -- clean
    CHECK(c[45].empty());  // ERROR_CODE
}

TEST_CASE("a full 32-word payload fills every data column", "[writer][L2-DEC-004]") {
    mie::MieMessage message = sample();
    uint16_t words[mie::MAX_DATA_WORDS];
    for (std::size_t i = 0; i < mie::MAX_DATA_WORDS; ++i) {
        words[i] = static_cast<uint16_t>(0x1000 + i);
    }
    message.data_words = mie::DataWords::from_words(words, mie::MAX_DATA_WORDS);

    const std::vector<std::string> c = split(chomp(mie::format_row(message)), ',');
    CHECK(c[3] == "1000");
    CHECK(c[34] == "101F");
    CHECK(c[35] == "7800");
}

TEST_CASE("a spurious record leaves RT, MSG and DELTA empty", "[writer][L2-RDR-018][L2-ERR-010]") {
    const std::vector<std::string> c = split(chomp(mie::format_row(spurious())), ',');
    CHECK(c[1].empty());   // RT -- no Command Word
    CHECK(c[2].empty());   // MSG
    CHECK(c[36].empty());  // CMD
    CHECK(c[39] == "B");   // BUS travels on the Type Word, not the Command Word
    CHECK(c[40].empty());  // DELTA -- no key to measure against
    CHECK(c[44] == "SPURIOUS");
    CHECK(c[45] == "2001");
}

TEST_CASE("an errored record carries its label and code", "[writer][L2-ERR-010]") {
    const std::vector<std::string> c = split(chomp(mie::format_row(errored())), ',');
    CHECK(c[44] == "ERROR");
    CHECK(c[45] == "011E");
    // DELTA is 0.000000, not empty: an errored record still takes part in
    // tracking (L2-RDR-016), and this is its first sighting.
    CHECK(c[40] == "0.000000");
}

TEST_CASE("every row ends with a bare newline", "[writer][L2-WRT-012][L3-CPP-005]") {
    // Byte-exact output on every host. A CRLF here would break every
    // conformance oracle on Windows alone.
    const std::string row = mie::format_row(sample());
    REQUIRE(row.size() >= 2);
    CHECK(row[row.size() - 1] == '\n');
    CHECK(row.find('\r') == std::string::npos);
}

TEST_CASE("DELTA is always six decimal places", "[writer][L2-DEC-014]") {
    mie::MieMessage message = sample();

    message.delta = 0.0;
    CHECK(split(chomp(mie::format_row(message)), ',')[40] == "0.000000");

    message.delta = 1.5;
    CHECK(split(chomp(mie::format_row(message)), ',')[40] == "1.500000");

    message.delta = 0.000001;
    CHECK(split(chomp(mie::format_row(message)), ',')[40] == "0.000001");

    // Absent is an EMPTY cell, not 0.000000: "no gap yet" and "no honest gap"
    // are different facts and the column has to keep them apart.
    message.delta = mie::Optional<double>();
    CHECK(split(chomp(mie::format_row(message)), ',')[40].empty());
}

// ---------------------------------------------------------------------------
// MUX and quoting
// ---------------------------------------------------------------------------

TEST_CASE("csv_quote applies RFC4180 minimal quoting", "[writer][L2-WRT-020]") {
    // Matching Python's csv.QUOTE_MINIMAL, which is what keeps MUX output
    // byte-identical across the three implementations rather than merely
    // similar.
    CHECK(mie::csv_quote("plain") == "plain");
    CHECK(mie::csv_quote("").empty());
    CHECK(mie::csv_quote("has,comma") == "\"has,comma\"");
    CHECK(mie::csv_quote("has\"quote") == "\"has\"\"quote\"");
    CHECK(mie::csv_quote("has\nnewline") == "\"has\nnewline\"");
    CHECK(mie::csv_quote("has\rreturn") == "\"has\rreturn\"");
    CHECK(mie::csv_quote("a,b\"c") == "\"a,b\"\"c\"");
}

TEST_CASE("a MUX value needing quotes is quoted in place", "[writer][L2-WRT-020]") {
    mie::MieMessage message = sample();
    message.mux.reset(new std::string("BUS,7"));
    const std::string row = mie::format_row(message);
    CHECK(row.find("\"BUS,7\"") != std::string::npos);
    // The embedded comma must not become a new cell -- splitting naively finds
    // one more field than the header has, which is the bug quoting prevents.
    CHECK(split(chomp(row), ',').size() == mie::TOTAL_COLUMN_COUNT + 1);
}

TEST_CASE("a plain MUX value lands in its column", "[writer][L2-WRT-020]") {
    mie::MieMessage message = sample();
    message.mux.reset(new std::string("BUS7"));
    CHECK(split(chomp(mie::format_row(message)), ',')[37] == "BUS7");
}
