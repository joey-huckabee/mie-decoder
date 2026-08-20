// SPDX-License-Identifier: Apache-2.0
//
// EXHAUSTIVE verification of the locale-free text primitives (L3-CPP-007).
//
// Two properties are swept in full here, both over all 256 byte values:
//
//   1. CORRECTNESS -- each classifier matches an independently written range
//      expression, for every byte, including the high bytes that a single-byte
//      locale would classify as letters.
//
//   2. LOCALE INDEPENDENCE -- every classifier and every formatter produces
//      *identical* results under a hostile locale. This is the property the
//      whole module exists for, and asserting it over one hand-picked character
//      would leave the interesting ones untested: tr_TR redefines the case
//      mapping of 'i' and 'I' specifically, and de_DE redefines the decimal
//      separator.
//
// The locale is always restored before any assertion is evaluated, so a Catch2
// failure message cannot itself be formatted under the altered locale.

#include "mie/text.hpp"

#include <catch2/catch.hpp>

#include <clocale>
#include <cstdio>
#include <string>
#include <vector>

namespace {

namespace txt = mie::text;

/// Switch LC_ALL, returning whether the locale was available. Restored by the
/// caller; a host without the locale still runs the C-locale half of each test.
bool try_locale(const char* name) { return std::setlocale(LC_ALL, name) != 0; }

/// Snapshot every classifier's verdict for all 256 byte values, plus the case
/// mappings. Comparing two snapshots is how "the locale changed nothing" is
/// asserted over the whole byte space in one comparison.
struct ClassificationSnapshot {
    std::vector<unsigned char> digit;
    std::vector<unsigned char> alpha;
    std::vector<unsigned char> upper;
    std::vector<unsigned char> lower;
    std::vector<unsigned char> alnum;
    std::vector<unsigned char> blank;
    std::vector<char> to_lower;
    std::vector<char> to_upper;
    std::vector<int> hex_value;

    bool operator==(const ClassificationSnapshot& o) const {
        return digit == o.digit && alpha == o.alpha && upper == o.upper && lower == o.lower &&
               alnum == o.alnum && blank == o.blank && to_lower == o.to_lower &&
               to_upper == o.to_upper && hex_value == o.hex_value;
    }
};

ClassificationSnapshot snapshot() {
    ClassificationSnapshot s;
    for (int i = 0; i < 256; ++i) {
        const char c = static_cast<char>(i);
        s.digit.push_back(txt::is_ascii_digit(c) ? 1 : 0);
        s.alpha.push_back(txt::is_ascii_alpha(c) ? 1 : 0);
        s.upper.push_back(txt::is_ascii_upper(c) ? 1 : 0);
        s.lower.push_back(txt::is_ascii_lower(c) ? 1 : 0);
        s.alnum.push_back(txt::is_ascii_alnum(c) ? 1 : 0);
        s.blank.push_back(txt::is_ascii_blank(c) ? 1 : 0);
        s.to_lower.push_back(txt::ascii_lower(c));
        s.to_upper.push_back(txt::ascii_upper(c));
        s.hex_value.push_back(txt::ascii_hex_value(c));
    }
    return s;
}

/// snprintf with the result CHECKED. A truncated oracle would silently weaken
/// every comparison that uses it, which is worse than a failing test.
std::string fixed6_oracle(double v) {
    char buffer[512];
    const int n = std::snprintf(buffer, sizeof(buffer), "%.6f", v);
    REQUIRE(n > 0);
    REQUIRE(static_cast<std::size_t>(n) < sizeof(buffer));
    return std::string(buffer, static_cast<std::size_t>(n));
}

}  // namespace

// ---------------------------------------------------------------------------
// Classification correctness over every byte
// ---------------------------------------------------------------------------

TEST_CASE("every byte value classifies exactly as its ASCII range says",
          "[text][exhaustive][L3-CPP-007]") {
    for (int i = 0; i < 256; ++i) {
        const char c = static_cast<char>(i);
        // The expectations are written against the unsigned code point, which
        // is a different formulation from the implementation's signed-char
        // comparisons -- so a sign-extension bug on the high bytes shows up as
        // a mismatch rather than as two copies of the same mistake.
        const unsigned u = static_cast<unsigned>(i);
        const bool want_digit = u >= '0' && u <= '9';
        const bool want_upper = u >= 'A' && u <= 'Z';
        const bool want_lower = u >= 'a' && u <= 'z';

        INFO("byte = " << i);
        REQUIRE(txt::is_ascii_digit(c) == want_digit);
        REQUIRE(txt::is_ascii_upper(c) == want_upper);
        REQUIRE(txt::is_ascii_lower(c) == want_lower);
        REQUIRE(txt::is_ascii_alpha(c) == (want_upper || want_lower));
        REQUIRE(txt::is_ascii_alnum(c) == (want_upper || want_lower || want_digit));
        REQUIRE(txt::is_ascii_blank(c) == (u == ' ' || u == '\t'));
    }
}

TEST_CASE("no byte above 0x7F is ever a letter or digit", "[text][exhaustive][L3-CPP-007]") {
    // The specific failure a locale-aware classifier produces: under a
    // single-byte locale, bytes in 0x80..0xFF classify as alphabetic, and a
    // config or CLI parser built on that starts accepting keys it should
    // reject.
    for (int i = 0x80; i < 256; ++i) {
        const char c = static_cast<char>(i);
        INFO("byte = 0x" << std::hex << i);
        REQUIRE_FALSE(txt::is_ascii_alpha(c));
        REQUIRE_FALSE(txt::is_ascii_digit(c));
        REQUIRE_FALSE(txt::is_ascii_alnum(c));
        REQUIRE_FALSE(txt::is_ascii_blank(c));
        REQUIRE(txt::ascii_hex_value(c) == -1);
        // Case mapping must leave them untouched rather than folding them.
        REQUIRE(txt::ascii_lower(c) == c);
        REQUIRE(txt::ascii_upper(c) == c);
    }
}

TEST_CASE("case mapping is an involution on letters and identity elsewhere",
          "[text][exhaustive][L3-CPP-007]") {
    for (int i = 0; i < 256; ++i) {
        const char c = static_cast<char>(i);
        INFO("byte = " << i);
        // Round-tripping a letter through both mappings returns it.
        if (txt::is_ascii_upper(c)) {
            REQUIRE(txt::ascii_upper(txt::ascii_lower(c)) == c);
        } else if (txt::is_ascii_lower(c)) {
            REQUIRE(txt::ascii_lower(txt::ascii_upper(c)) == c);
        } else {
            // Everything else is left alone by both.
            REQUIRE(txt::ascii_lower(c) == c);
            REQUIRE(txt::ascii_upper(c) == c);
        }
    }
}

TEST_CASE("hex digit values are correct for every byte", "[text][exhaustive]") {
    for (int i = 0; i < 256; ++i) {
        const char c = static_cast<char>(i);
        int want = -1;
        if (i >= '0' && i <= '9') {
            want = i - '0';
        } else if (i >= 'a' && i <= 'f') {
            want = i - 'a' + 10;
        } else if (i >= 'A' && i <= 'F') {
            want = i - 'A' + 10;
        }
        const int got = txt::ascii_hex_value(c);
        if (got != want) {
            INFO("byte = " << i);
            REQUIRE(got == want);
        }
    }
    SUCCEED("all 256 byte values map to the right hex digit value");
}

// ---------------------------------------------------------------------------
// Locale independence, over the whole byte space
// ---------------------------------------------------------------------------

TEST_CASE("classification is byte-for-byte identical under a hostile locale",
          "[text][exhaustive][L3-CPP-007]") {
    // tr_TR is the interesting one: it maps 'i' to a dotted capital I and 'I'
    // to a dotless lowercase i. A classifier built on <cctype> changes its
    // answer for those two characters, and a config parser built on it stops
    // recognising a key. Comparing full snapshots proves nothing changed for
    // ANY byte, not just those two.
    const ClassificationSnapshot baseline = snapshot();

    const char* previous = std::setlocale(LC_ALL, 0);
    const std::string saved = previous != 0 ? std::string(previous) : std::string("C");

    const char* locales[] = {"tr_TR.UTF-8", "tr_TR", "de_DE.UTF-8", "de_DE", "C.UTF-8"};
    int exercised = 0;
    bool all_match = true;

    for (std::size_t i = 0; i < sizeof(locales) / sizeof(locales[0]); ++i) {
        if (!try_locale(locales[i])) {
            continue;
        }
        ++exercised;
        if (!(snapshot() == baseline)) {
            all_match = false;
        }
    }
    (void)std::setlocale(LC_ALL, saved.c_str());

    CHECK(all_match);
    if (exercised == 0) {
        // Reported, not skipped: a gate that could not run has not passed, and
        // on a host with no extra locales this case proves only that the C
        // locale works.
        WARN("no alternate locale available; locale-independence was not exercised");
    }
}

TEST_CASE("fixed6 is byte-for-byte identical under a comma-separator locale",
          "[text][exhaustive][delta][L3-CPP-007]") {
    // Swept rather than spot-checked: the separator appears in every value, so
    // a locale-sensitive formatter would corrupt the entire DELTA column, and
    // the sweep covers the magnitudes an actual gap takes (sub-microsecond
    // through hours).
    const double samples[] = {0.0,     0.000001, 0.5,  1.0,  1.2345,       59.999999, 3600.0,
                              86399.5, 1e6,      1e-7, -0.5, -1234.567890, 0.0000005, 2.0000005};
    const std::size_t count = sizeof(samples) / sizeof(samples[0]);

    std::vector<std::string> baseline;
    baseline.reserve(count);
    for (std::size_t i = 0; i < count; ++i) {
        baseline.push_back(txt::fixed6(samples[i]));
    }

    const char* previous = std::setlocale(LC_ALL, 0);
    const std::string saved = previous != 0 ? std::string(previous) : std::string("C");

    const char* locales[] = {"de_DE.UTF-8", "de_DE", "fr_FR.UTF-8", "fr_FR"};
    int exercised = 0;
    bool all_match = true;
    bool any_comma = false;

    for (std::size_t l = 0; l < sizeof(locales) / sizeof(locales[0]); ++l) {
        if (!try_locale(locales[l])) {
            continue;
        }
        ++exercised;
        for (std::size_t i = 0; i < count; ++i) {
            const std::string got = txt::fixed6(samples[i]);
            if (got != baseline[i]) {
                all_match = false;
            }
            if (got.find(',') != std::string::npos) {
                any_comma = true;
            }
        }
    }
    (void)std::setlocale(LC_ALL, saved.c_str());

    CHECK(all_match);
    CHECK_FALSE(any_comma);
    for (std::size_t i = 0; i < count; ++i) {
        INFO("sample index " << i);
        CHECK(baseline[i].find(',') == std::string::npos);
    }
    if (exercised == 0) {
        WARN("no comma-separator locale available; fixed6 normalisation was not exercised");
    }
}

TEST_CASE("integer formatting is identical under a hostile locale",
          "[text][exhaustive][L3-CPP-007]") {
    // A locale with digit grouping would insert separators into a printf-based
    // formatter's output. These are hand-rolled and must not care.
    std::vector<std::string> baseline;
    for (uint32_t v = 0; v <= 2000; ++v) {
        baseline.push_back(txt::decimal(v));
        baseline.push_back(txt::decimal_padded(v, 6));
        baseline.push_back(txt::hex_upper(v, 4));
    }

    const char* previous = std::setlocale(LC_ALL, 0);
    const std::string saved = previous != 0 ? std::string(previous) : std::string("C");

    const char* locales[] = {"de_DE.UTF-8", "de_DE", "tr_TR.UTF-8", "tr_TR"};
    bool all_match = true;
    for (std::size_t l = 0; l < sizeof(locales) / sizeof(locales[0]); ++l) {
        if (!try_locale(locales[l])) {
            continue;
        }
        std::size_t k = 0;
        for (uint32_t v = 0; v <= 2000; ++v) {
            if (txt::decimal(v) != baseline[k++]) {
                all_match = false;
            }
            if (txt::decimal_padded(v, 6) != baseline[k++]) {
                all_match = false;
            }
            if (txt::hex_upper(v, 4) != baseline[k++]) {
                all_match = false;
            }
        }
    }
    (void)std::setlocale(LC_ALL, saved.c_str());

    CHECK(all_match);
}

// ---------------------------------------------------------------------------
// fixed6 against snprintf
// ---------------------------------------------------------------------------

TEST_CASE("fixed6 matches snprintf across the magnitudes a DELTA takes",
          "[text][exhaustive][delta]") {
    // Independent oracle. Runs in the C locale, where snprintf and fixed6 must
    // agree exactly -- the normalisation step is a no-op there, so this checks
    // the rounding and digit generation rather than the separator.
    for (uint32_t micros = 0; micros <= 200000; micros += 7) {
        const double v = static_cast<double>(micros) / 1000000.0;
        const std::string want = fixed6_oracle(v);
        const std::string got = txt::fixed6(v);
        if (got != want) {
            INFO("micros = " << micros);
            REQUIRE(got == want);
        }
    }
    for (uint32_t seconds = 0; seconds <= 90000; seconds += 13) {
        const double v = static_cast<double>(seconds) + 0.5;
        const std::string want = fixed6_oracle(v);
        const std::string got = txt::fixed6(v);
        if (got != want) {
            INFO("seconds = " << seconds);
            REQUIRE(got == want);
        }
    }
    SUCCEED("fixed6 matches snprintf across sub-microsecond to day-scale gaps");
}

TEST_CASE("fixed6 always emits exactly six fractional digits", "[text][exhaustive][delta]") {
    // The structural property, independent of the value: every rendering has a
    // dot with exactly six digits after it. A width bug would shift the CSV
    // column contents without necessarily changing the leading digits.
    for (uint32_t micros = 0; micros <= 100000; micros += 3) {
        const std::string got = txt::fixed6(static_cast<double>(micros) / 1000000.0);
        const std::size_t dot = got.find('.');
        if (dot == std::string::npos || got.size() - dot - 1 != 6) {
            INFO("micros = " << micros << " rendered as " << got);
            REQUIRE(dot != std::string::npos);
            REQUIRE(got.size() - dot - 1 == 6);
        }
    }
    SUCCEED("every rendering carries exactly six fractional digits");
}

// ---------------------------------------------------------------------------
// Case-insensitive comparison
// ---------------------------------------------------------------------------

TEST_CASE("case-insensitive comparison is exhaustive over short permutations",
          "[text][exhaustive]") {
    // Every upper/lower permutation of an eight-character name -- 256 spellings
    // -- must compare equal to the canonical one, and a name differing in one
    // character must not.
    const std::string canonical = "IRIG_STD";
    // The shift is done in size_t, not in unsigned int. MSVC warns (C4334)
    // that a 32-bit shift is being widened to 64 bits and GCC does not, so
    // this is a portability difference the Windows tier catches and the
    // Linux tiers cannot.
    const std::size_t permutations = static_cast<std::size_t>(1) << canonical.size();
    for (std::size_t mask = 0; mask < permutations; ++mask) {
        std::string spelled = canonical;
        for (std::size_t bit = 0; bit < canonical.size(); ++bit) {
            if ((mask >> bit) & 1u) {
                spelled[bit] = txt::ascii_lower(spelled[bit]);
            }
        }
        if (!txt::equals_ignoring_ascii_case(spelled, canonical)) {
            INFO("spelling = " << spelled);
            REQUIRE(txt::equals_ignoring_ascii_case(spelled, canonical));
        }
    }
    SUCCEED("all 256 case permutations compare equal");
}

TEST_CASE("case-insensitive comparison rejects every single-character difference",
          "[text][exhaustive]") {
    const std::string canonical = "standard";
    for (std::size_t pos = 0; pos < canonical.size(); ++pos) {
        for (int repl = 0; repl < 256; ++repl) {
            const char c = static_cast<char>(repl);
            if (txt::ascii_lower(c) == txt::ascii_lower(canonical[pos])) {
                continue;  // same letter in some case -- must still match
            }
            std::string altered = canonical;
            altered[pos] = c;
            if (txt::equals_ignoring_ascii_case(altered, canonical)) {
                INFO("pos = " << pos << " byte = " << repl);
                REQUIRE_FALSE(txt::equals_ignoring_ascii_case(altered, canonical));
            }
        }
    }
    SUCCEED("no single-byte substitution compares equal");
}

TEST_CASE("trimming handles every blank arrangement", "[text][exhaustive]") {
    // Every combination of leading and trailing blanks up to four each, over
    // both blank characters -- the shapes a hand-edited TOML file produces.
    const char blanks[] = {' ', '\t'};
    for (std::size_t lead = 0; lead <= 4; ++lead) {
        for (std::size_t trail = 0; trail <= 4; ++trail) {
            for (std::size_t b = 0; b < 2; ++b) {
                const std::string padding_lead(lead, blanks[b]);
                const std::string padding_trail(trail, blanks[1 - b]);
                // Built by appending rather than by chained operator+, which
                // allocates a temporary for each link.
                std::string subject = padding_lead;
                subject += "value";
                subject += padding_trail;
                if (txt::trim_ascii_blank(subject) != "value") {
                    INFO("lead=" << lead << " trail=" << trail << " b=" << b);
                    REQUIRE(txt::trim_ascii_blank(subject) == "value");
                }
            }
        }
    }

    SECTION("an all-blank string trims to empty at every length") {
        for (std::size_t n = 0; n <= 16; ++n) {
            REQUIRE(txt::trim_ascii_blank(std::string(n, ' ')).empty());
            REQUIRE(txt::trim_ascii_blank(std::string(n, '\t')).empty());
        }
    }

    SECTION("interior blanks are preserved") {
        CHECK(txt::trim_ascii_blank("  a b\tc  ") == "a b\tc");
    }

    SECTION("newlines are not blanks and are not trimmed") {
        // is_ascii_blank deliberately excludes '\n'; trimming it would let a
        // line-oriented parser silently join lines.
        CHECK(txt::trim_ascii_blank(" \nvalue\n ") == "\nvalue\n");
    }
}
