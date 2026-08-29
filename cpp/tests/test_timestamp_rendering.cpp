// SPDX-License-Identifier: Apache-2.0
//
// Operator-selectable TIME_STAMP rendering (v3.0.0).
//
// Mirrors rust/src/models.rs, rust/tests/cli.rs and
// python/tests/test_timestamp_rendering.py. The leap-year matrix is the centre
// of this file: the same day-of-year lands one day apart either side of a leap
// day, which is the entire reason the calendar renderings need a year at all,
// and the defect class the rest of the feature exists to refuse.

#include <catch2/catch.hpp>

#include <string>
#include <vector>

#include "log_capture.hpp"
#include "mie/cli.hpp"
#include "mie/config.hpp"
#include "mie/models.hpp"
#include "mie/writer.hpp"
#include "record_fixtures.hpp"
#include "temp_path.hpp"

namespace {

using mie_test::TempFile;
using mie_test::TempPath;

typedef std::vector<std::string> Args;

mie::IrigTimestamp sample_irig() { return mie::IrigTimestamp(192, 15, 54, 50, 456225, false); }

mie::TimeRender render_of(mie::OutputTimeFormat format, int year, int offset_minutes = 0) {
    mie::TimeRender render;
    render.format = format;
    render.year = year;
    render.utc_offset_minutes = offset_minutes;
    return render;
}

/// A well-formed recording whose records carry day-of-year 192.
///
/// Built here rather than with `bc_to_rt`, whose fixture day is 10: day 10
/// falls before the leap day, so it renders identically in a leap and a common
/// year and the CLI cases below would not discriminate between them. Day 192 is
/// July 10 in a leap year and July 11 in a common one, which is the difference
/// these tests exist to see.
///
/// Two records, not one: entry validation confirms a candidate by looking
/// ahead, so a single-record file exercises a different path.
std::vector<uint8_t> recording_on_day_192() {
    std::vector<uint16_t> words;
    for (uint32_t i = 0; i < 2; ++i) {
        std::vector<uint16_t> record;
        // Type(1) + IRIG(3) + Cmd(1) + 2 data + Status(1) = 8 words.
        record.push_back(mie_test::type_word(mie::MESSAGE_TYPE_BC_TO_RT, 8));
        mie_test::push_irig(record, 456225, 192);
        record.push_back(mie_test::command_word(15, mie::DIRECTION_RECEIVE, 11, 2));
        record.push_back(static_cast<uint16_t>(0x1100 + i));
        record.push_back(static_cast<uint16_t>(0x1200 + i));
        record.push_back(mie_test::status_word(15));
        mie_test::append(words, record);
    }
    return mie_test::finish(words);
}

int run_capturing(const Args& argv, std::string& out, std::string& err) {
    const TempPath out_path("mie-ts-out");
    const TempPath err_path("mie-ts-err");
    int code = 0;
    {
        std::FILE* out_handle = std::fopen(out_path.str().c_str(), "wb");
        std::FILE* err_handle = std::fopen(err_path.str().c_str(), "wb");
        REQUIRE(out_handle != NULL);
        REQUIRE(err_handle != NULL);
        code = mie::cli::run(argv, mie::cli::Streams(out_handle, err_handle));
        REQUIRE(std::fclose(out_handle) == 0);
        REQUIRE(std::fclose(err_handle) == 0);
    }
    REQUIRE(mie_test::read_file(out_path.str(), out));
    REQUIRE(mie_test::read_file(err_path.str(), err));
    return code;
}

/// The second line of a CSV capture -- the first data row.
std::string first_row(const std::string& csv) {
    const std::string::size_type header_end = csv.find('\n');
    REQUIRE(header_end != std::string::npos);
    const std::string::size_type row_end = csv.find('\n', header_end + 1);
    REQUIRE(row_end != std::string::npos);
    return csv.substr(header_end + 1, row_end - header_end - 1);
}

bool starts_with(const std::string& value, const std::string& prefix) {
    return value.size() >= prefix.size() && value.compare(0, prefix.size(), prefix) == 0;
}

}  // namespace

// ---------------------------------------------------------------------------
// Calendar arithmetic (L2-WRT-025)
// ---------------------------------------------------------------------------

TEST_CASE("the leap-year rule is proleptic Gregorian", "[models][L2-WRT-025]") {
    const int leap_years[] = {1996, 2000, 2004, 2020, 2024, 2028, 1600};
    for (std::size_t i = 0; i < sizeof(leap_years) / sizeof(leap_years[0]); ++i) {
        INFO("year = " << leap_years[i]);
        CHECK(mie::is_leap_year(leap_years[i]));
    }
    const int common_years[] = {1900, 1999, 2001, 2025, 2026, 2027, 2100, 2200, 2300};
    for (std::size_t i = 0; i < sizeof(common_years) / sizeof(common_years[0]); ++i) {
        INFO("year = " << common_years[i]);
        CHECK(!mie::is_leap_year(common_years[i]));
    }
}

TEST_CASE("day-of-year 192 shifts by one across leap and common years",
          "[models][L2-WRT-025][L2-WRT-026]") {
    // The example from the format documentation, and the hazard in one line.
    int month = 0;
    int day = 0;

    REQUIRE(mie::day_of_year_to_month_day(2024, 192, month, day));
    CHECK(month == 7);
    CHECK(day == 10);

    REQUIRE(mie::day_of_year_to_month_day(2026, 192, month, day));
    CHECK(month == 7);
    CHECK(day == 11);
}

TEST_CASE("day-of-year resolves at month boundaries", "[models][L2-WRT-025]") {
    struct Case {
        int year;
        int day_of_year;
        int month;
        int day_of_month;
    };
    const Case cases[] = {
        // Common year: day 59 is Feb 28, day 60 is Mar 1.
        {2026, 1, 1, 1},
        {2026, 31, 1, 31},
        {2026, 32, 2, 1},
        {2026, 59, 2, 28},
        {2026, 60, 3, 1},
        {2026, 365, 12, 31},
        // Leap year: 60 is the leap day itself, and everything after shifts.
        {2024, 59, 2, 28},
        {2024, 60, 2, 29},
        {2024, 61, 3, 1},
        {2024, 366, 12, 31},
    };
    for (std::size_t i = 0; i < sizeof(cases) / sizeof(cases[0]); ++i) {
        INFO("year " << cases[i].year << " day " << cases[i].day_of_year);
        int month = 0;
        int day = 0;
        REQUIRE(mie::day_of_year_to_month_day(cases[i].year, cases[i].day_of_year, month, day));
        CHECK(month == cases[i].month);
        CHECK(day == cases[i].day_of_month);
    }
}

TEST_CASE("day 366 has no date in a common year", "[models][L2-WRT-026]") {
    int month = 0;
    int day = 0;
    CHECK(!mie::day_of_year_to_month_day(2026, 366, month, day));
    CHECK(!mie::day_of_year_to_month_day(1900, 366, month, day));
    CHECK(mie::day_of_year_to_month_day(2024, 366, month, day));

    const int years[] = {2024, 2026};
    for (std::size_t i = 0; i < 2; ++i) {
        CHECK(!mie::day_of_year_to_month_day(years[i], 0, month, day));
        CHECK(!mie::day_of_year_to_month_day(years[i], -1, month, day));
        CHECK(!mie::day_of_year_to_month_day(years[i], 367, month, day));
        CHECK(!mie::day_of_year_to_month_day(years[i], 100000, month, day));
    }
}

TEST_CASE("every day of both year kinds resolves in ascending order", "[models][L2-WRT-025]") {
    // Exhaustive rather than spot-checked: this catches an off-by-one in the
    // accumulator that boundary cases alone would miss.
    const int years[] = {1, 4, 100, 400, 1900, 2000, 2024, 2026, 9999};
    for (std::size_t y = 0; y < sizeof(years) / sizeof(years[0]); ++y) {
        const int year = years[y];
        const bool leap = mie::is_leap_year(year);
        const int length = leap ? 366 : 365;
        int per_month[13] = {0};
        int previous_month = 0;
        int previous_day = 0;

        for (int doy = 1; doy <= length; ++doy) {
            int month = 0;
            int day = 0;
            INFO("year " << year << " day-of-year " << doy);
            REQUIRE(mie::day_of_year_to_month_day(year, doy, month, day));
            const bool ascending =
                month > previous_month || (month == previous_month && day > previous_day);
            CHECK(ascending);
            previous_month = month;
            previous_day = day;
            per_month[month] += 1;
        }

        const int common_lengths[12] = {31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31};
        for (int index = 0; index < 12; ++index) {
            const int expected = common_lengths[index] + ((index == 1 && leap) ? 1 : 0);
            INFO("year " << year << " month " << (index + 1));
            CHECK(per_month[index + 1] == expected);
        }
    }
}

TEST_CASE("a UTC offset renders as Z at zero and signed HH:MM otherwise", "[models][L2-WRT-025]") {
    CHECK(mie::format_utc_offset(0) == "Z");
    CHECK(mie::format_utc_offset(-300) == "-05:00");
    CHECK(mie::format_utc_offset(330) == "+05:30");
    CHECK(mie::format_utc_offset(60) == "+01:00");
    CHECK(mie::format_utc_offset(-1) == "-00:01");
    CHECK(mie::format_utc_offset(1439) == "+23:59");
}

// ---------------------------------------------------------------------------
// The three renderings (L2-WRT-025)
// ---------------------------------------------------------------------------

TEST_CASE("the default rendering is unaffected by a year or an offset",
          "[models][L2-WRT-011][L2-WRT-025]") {
    // Byte-identical to what pre-v3.0.0 emitted -- that is what keeps a no-flag
    // decode vendor-diffable (L1-OUT-004). A year and an offset are inert here
    // (L2-WRT-026 clause 5).
    const mie::IrigTimestamp ts = sample_irig();
    const std::string expected = "192:15:54:50.456225";
    CHECK(ts.format() == expected);

    std::string rendered;
    CHECK(ts.format_with(mie::TimeRender(), rendered) == mie::CALENDAR_OK);
    CHECK(rendered == expected);

    CHECK(ts.format_with(render_of(mie::OUTPUT_TIME_DOY, 2024, -300), rendered) ==
          mie::CALENDAR_OK);
    CHECK(rendered == expected);
}

TEST_CASE("the calendar renderings resolve against the given year", "[models][L2-WRT-025]") {
    struct Case {
        mie::OutputTimeFormat format;
        int year;
        int offset;
        const char* expected;
    };
    const Case cases[] = {
        {mie::OUTPUT_TIME_ISO, 2024, 0, "2024-07-10T15:54:50.456225Z"},
        {mie::OUTPUT_TIME_ISO, 2026, 0, "2026-07-11T15:54:50.456225Z"},
        {mie::OUTPUT_TIME_ISO, 2026, -300, "2026-07-11T15:54:50.456225-05:00"},
        {mie::OUTPUT_TIME_ISO, 2026, 330, "2026-07-11T15:54:50.456225+05:30"},
        {mie::OUTPUT_TIME_DOM, 2024, 0, "10:15:54:50.456225"},
        {mie::OUTPUT_TIME_DOM, 2026, 0, "11:15:54:50.456225"},
    };
    for (std::size_t i = 0; i < sizeof(cases) / sizeof(cases[0]); ++i) {
        INFO(cases[i].expected);
        std::string rendered;
        REQUIRE(sample_irig().format_with(
                    render_of(cases[i].format, cases[i].year, cases[i].offset), rendered) ==
                mie::CALENDAR_OK);
        CHECK(rendered == cases[i].expected);
    }
}

TEST_CASE("every rendering emits exactly six microsecond digits",
          "[models][L2-DEC-014][L2-WRT-025]") {
    // The wider ISO cell does not relax L2-DEC-014.
    const uint32_t micros[] = {0, 1, 999999, 1000000, 1234567};
    const mie::OutputTimeFormat formats[] = {mie::OUTPUT_TIME_DOY, mie::OUTPUT_TIME_ISO,
                                             mie::OUTPUT_TIME_DOM};
    for (std::size_t m = 0; m < sizeof(micros) / sizeof(micros[0]); ++m) {
        const mie::IrigTimestamp ts(192, 15, 54, 50, micros[m], false);
        for (std::size_t f = 0; f < 3; ++f) {
            std::string rendered;
            REQUIRE(ts.format_with(render_of(formats[f], 2026), rendered) == mie::CALENDAR_OK);
            INFO(rendered);
            const std::string::size_type dot = rendered.rfind('.');
            REQUIRE(dot != std::string::npos);
            std::size_t digits = 0;
            for (std::string::size_type i = dot + 1;
                 i < rendered.size() && rendered[i] >= '0' && rendered[i] <= '9'; ++i) {
                digits += 1;
            }
            CHECK(digits == 6);
        }
    }
}

TEST_CASE("output-time-format names parse case-insensitively", "[models][L2-CLI-018][L2-CFG-012]") {
    struct Case {
        const char* name;
        mie::OutputTimeFormat expected;
    };
    const Case accepted[] = {
        {"doy", mie::OUTPUT_TIME_DOY}, {"DOY", mie::OUTPUT_TIME_DOY}, {"Iso", mie::OUTPUT_TIME_ISO},
        {"ISO", mie::OUTPUT_TIME_ISO}, {"dom", mie::OUTPUT_TIME_DOM}, {"DoM", mie::OUTPUT_TIME_DOM},
    };
    for (std::size_t i = 0; i < sizeof(accepted) / sizeof(accepted[0]); ++i) {
        mie::OutputTimeFormat parsed = mie::OUTPUT_TIME_DOY;
        INFO(accepted[i].name);
        REQUIRE(mie::output_time_format_from_name(accepted[i].name, parsed));
        CHECK(parsed == accepted[i].expected);
    }

    const char* const rejected[] = {"", "day", "iso8601", "doy ", "elapsed"};
    for (std::size_t i = 0; i < sizeof(rejected) / sizeof(rejected[0]); ++i) {
        mie::OutputTimeFormat parsed = mie::OUTPUT_TIME_DOY;
        INFO(rejected[i]);
        CHECK(!mie::output_time_format_from_name(rejected[i], parsed));
    }

    CHECK(!mie::output_time_format_needs_calendar(mie::OUTPUT_TIME_DOY));
    CHECK(mie::output_time_format_needs_calendar(mie::OUTPUT_TIME_ISO));
    CHECK(mie::output_time_format_needs_calendar(mie::OUTPUT_TIME_DOM));
    CHECK(std::string(mie::output_time_format_name(mie::OUTPUT_TIME_DOY)) == "doy");
    CHECK(std::string(mie::output_time_format_name(mie::OUTPUT_TIME_ISO)) == "iso");
    CHECK(std::string(mie::output_time_format_name(mie::OUTPUT_TIME_DOM)) == "dom");
}

// ---------------------------------------------------------------------------
// Refusals (L2-WRT-026)
// ---------------------------------------------------------------------------

TEST_CASE("a calendar rendering refuses rather than approximating", "[models][L2-WRT-026]") {
    const mie::OutputTimeFormat formats[] = {mie::OUTPUT_TIME_ISO, mie::OUTPUT_TIME_DOM};
    const mie::IrigTimestamp leap_day(366, 15, 54, 50, 456225, false);

    for (std::size_t i = 0; i < 2; ++i) {
        std::string rendered;

        // No year resolved at all.
        mie::TimeRender no_year;
        no_year.format = formats[i];
        CHECK(sample_irig().format_with(no_year, rendered) == mie::CALENDAR_MISSING_YEAR);

        // Day 366 against a common year has no date to render.
        CHECK(leap_day.format_with(render_of(formats[i], 2026), rendered) ==
              mie::CALENDAR_NO_SUCH_DAY);

        // The same record renders fine once the year actually has that day.
        CHECK(leap_day.format_with(render_of(formats[i], 2024), rendered) == mie::CALENDAR_OK);
    }

    // `doy` never needs a calendar and so never refuses.
    std::string rendered;
    CHECK(leap_day.format_with(mie::TimeRender(), rendered) == mie::CALENDAR_OK);
}

TEST_CASE("a Standard counter refuses the calendar renderings",
          "[models][L2-WRT-025][L2-WRT-026]") {
    // A free-running counter has no epoch, so no year places it on a calendar,
    // and emitting hex into a column the operator asked to be ISO-8601 would be
    // its own kind of lie.
    const mie::Timestamp ts =
        mie::Timestamp::from_standard(mie::StandardTimestamp(100000, 0x0001, 0x86A0));

    std::string rendered;
    REQUIRE(ts.format_with(mie::TimeRender(), rendered) == mie::CALENDAR_OK);
    CHECK(rendered == "0x000186A0");

    CHECK(ts.format_with(render_of(mie::OUTPUT_TIME_ISO, 2026), rendered) ==
          mie::CALENDAR_NOT_LOCKED);
    CHECK(ts.format_with(render_of(mie::OUTPUT_TIME_DOM, 2026), rendered) ==
          mie::CALENDAR_NOT_LOCKED);
}

TEST_CASE("a freerun IRIG timestamp refuses the calendar renderings", "[models][L2-WRT-026]") {
    // The most dangerous of the three: freerun fields are calendar-shaped but
    // not calendar-anchored, so they would render as an ordinary date.
    const mie::IrigTimestamp freerun(192, 15, 54, 50, 456225, true);

    std::string rendered;
    REQUIRE(freerun.format_with(mie::TimeRender(), rendered) == mie::CALENDAR_OK);
    CHECK(rendered == "192:15:54:50.456225");

    CHECK(freerun.format_with(render_of(mie::OUTPUT_TIME_ISO, 2026), rendered) ==
          mie::CALENDAR_FREERUN);
    CHECK(freerun.format_with(render_of(mie::OUTPUT_TIME_DOM, 2026), rendered) ==
          mie::CALENDAR_FREERUN);

    // Calendar-locked, the same instant renders fine -- so the refusal is about
    // the freerun bit and nothing else.
    CHECK(sample_irig().format_with(render_of(mie::OUTPUT_TIME_ISO, 2026), rendered) ==
          mie::CALENDAR_OK);
}

// ---------------------------------------------------------------------------
// CLI surface (L2-CLI-018 / L2-CLI-019)
// ---------------------------------------------------------------------------

TEST_CASE("the default decode rendering is unchanged", "[cli][L2-WRT-011][L2-WRT-025]") {
    const TempFile input("mie-ts-in", recording_on_day_192());
    Args argv;
    argv.push_back("decode");
    argv.push_back(input.str());

    std::string out;
    std::string err;
    REQUIRE(run_capturing(argv, out, err) == mie::cli::EXIT_OK);
    CHECK(starts_with(first_row(out), "192:15:54:50.456225,"));
}

TEST_CASE("the calendar renderings reach the CSV", "[cli][L2-WRT-025][L2-CLI-018]") {
    const TempFile input("mie-ts-in", recording_on_day_192());

    struct Case {
        const char* format;
        const char* year;
        const char* extra;
        const char* expected_prefix;
    };
    const Case cases[] = {
        {"iso", "2026", "", "2026-07-11T15:54:50.456225Z,"},
        {"iso", "2024", "", "2024-07-10T15:54:50.456225Z,"},
        {"dom", "2026", "", "11:15:54:50.456225,"},
        {"dom", "2024", "", "10:15:54:50.456225,"},
        {"iso", "2026", "--utc-offset=-05:00", "2026-07-11T15:54:50.456225-05:00,"},
    };

    for (std::size_t i = 0; i < sizeof(cases) / sizeof(cases[0]); ++i) {
        Args argv;
        argv.push_back("decode");
        argv.push_back(input.str());
        argv.push_back("--output-time-format");
        argv.push_back(cases[i].format);
        argv.push_back("--year");
        argv.push_back(cases[i].year);
        if (cases[i].extra[0] != 0) {
            argv.push_back(cases[i].extra);
        }

        std::string out;
        std::string err;
        INFO(cases[i].format << " " << cases[i].year << " " << cases[i].extra);
        REQUIRE(run_capturing(argv, out, err) == mie::cli::EXIT_OK);
        CHECK(starts_with(first_row(out), cases[i].expected_prefix));
    }
}

TEST_CASE("a calendar rendering with no year is a usage error", "[cli][L2-WRT-026][L2-CLI-018]") {
    // Refused before the output is opened, naming BOTH ways to supply a year.
    const TempFile input("mie-ts-in", recording_on_day_192());
    const char* const formats[] = {"iso", "dom"};

    for (std::size_t i = 0; i < 2; ++i) {
        const TempPath output("mie-ts-out");
        Args argv;
        argv.push_back("decode");
        argv.push_back(input.str());
        argv.push_back("-o");
        argv.push_back(output.str());
        argv.push_back("--output-time-format");
        argv.push_back(formats[i]);

        std::string out;
        std::string err;
        INFO(formats[i]);
        CHECK(run_capturing(argv, out, err) == mie::cli::EXIT_USAGE);
        CHECK(err.find("--year") != std::string::npos);
        CHECK(err.find("[output] year") != std::string::npos);
        CHECK(!mie::platform::path_exists(output.str()));
    }
}

TEST_CASE("doy ignores a year and an offset rather than rejecting them", "[cli][L2-WRT-026]") {
    const TempFile input("mie-ts-in", recording_on_day_192());
    Args argv;
    argv.push_back("decode");
    argv.push_back(input.str());
    argv.push_back("--output-time-format");
    argv.push_back("doy");
    argv.push_back("--year");
    argv.push_back("2024");
    argv.push_back("--utc-offset=-05:00");

    std::string out;
    std::string err;
    REQUIRE(run_capturing(argv, out, err) == mie::cli::EXIT_OK);
    CHECK(starts_with(first_row(out), "192:15:54:50.456225,"));
}

TEST_CASE("the new flag values are validated at parse time", "[cli][L2-CLI-018]") {
    const TempFile input("mie-ts-in", recording_on_day_192());

    struct Case {
        const char* flag;
        const char* value;
    };
    const Case bad[] = {
        {"--output-time-format", "elapsed"},
        {"--output-time-format", ""},
        {"--year", "0"},
        {"--year", "10000"},
        {"--year", "-1"},
        {"--year", "twenty"},
        {"--utc-offset", "+24:00"},
        {"--utc-offset", "+05:60"},
        {"--utc-offset", "+5:00"},
        {"--utc-offset", "0500"},
        {"--utc-offset", "PST"},
    };

    for (std::size_t i = 0; i < sizeof(bad) / sizeof(bad[0]); ++i) {
        Args argv;
        argv.push_back("decode");
        argv.push_back(input.str());
        argv.push_back(bad[i].flag);
        argv.push_back(bad[i].value);

        std::string out;
        std::string err;
        INFO(bad[i].flag << " " << bad[i].value);
        CHECK(run_capturing(argv, out, err) == mie::cli::EXIT_USAGE);
        CHECK(err.find(bad[i].flag) != std::string::npos);
    }
}

TEST_CASE("the retired --time-format flag names both replacements", "[cli][L2-CLI-019]") {
    // Unlike --inline-errors this flag has a successor -- two of them -- so the
    // diagnostic must say which is which. That is the one question a generic
    // "unknown option" cannot answer.
    const TempFile input("mie-ts-in", recording_on_day_192());
    const TempPath output("mie-ts-out");

    Args argv;
    argv.push_back("decode");
    argv.push_back(input.str());
    argv.push_back("-o");
    argv.push_back(output.str());
    argv.push_back("--time-format");
    argv.push_back("irig");

    std::string out;
    std::string err;
    CHECK(run_capturing(argv, out, err) == mie::cli::EXIT_USAGE);
    CHECK(err.find("--time-format") != std::string::npos);
    CHECK(err.find("--input-time-format") != std::string::npos);
    CHECK(err.find("--output-time-format") != std::string::npos);
    CHECK(!mie::platform::path_exists(output.str()));
}

TEST_CASE("help advertises the timestamp flags", "[cli][L2-CLI-018]") {
    // The cross-implementation parity gate scrapes --help, so a flag the parser
    // accepts but help omits fails the conformance run.
    Args argv;
    argv.push_back("decode");
    argv.push_back("--help");

    std::string out;
    std::string err;
    REQUIRE(run_capturing(argv, out, err) == mie::cli::EXIT_OK);
    const std::string help = out + err;
    const char* const flags[] = {"--input-time-format", "--output-time-format", "--year",
                                 "--utc-offset"};
    for (std::size_t i = 0; i < 4; ++i) {
        INFO(flags[i]);
        CHECK(help.find(flags[i]) != std::string::npos);
    }
}

// ---------------------------------------------------------------------------
// Config surface (L2-CFG-012)
// ---------------------------------------------------------------------------

TEST_CASE("the retired config key is rejected, not ignored", "[config][L2-CFG-012]") {
    // The generic unknown-key rule only WARNs, which for a *rename* would
    // silently discard a forced format and revert to auto-detection.
    mie::DecoderConfig config;
    bool threw = false;
    try {
        config = mie::parse_into_config("[decode]\ntime_format = \"irig\"\n");
    } catch (const mie::ConfigError& e) {
        threw = true;
        const std::string message(e.what());
        CHECK(message.find("decode.input_time_format") != std::string::npos);
        CHECK(message.find("output.output_time_format") != std::string::npos);
    }
    CHECK(threw);
}

TEST_CASE("the three output keys load and validate", "[config][L2-CFG-012]") {
    const mie::DecoderConfig config = mie::parse_into_config(
        "[output]\noutput_time_format = \"iso\"\nyear = 2024\nutc_offset = \"-05:00\"\n");
    CHECK(config.output_time_format == mie::OUTPUT_TIME_ISO);
    REQUIRE(config.year.has_value());
    CHECK(config.year.value() == 2024);
    CHECK(config.utc_offset_minutes == -300);

    const char* const rejected[] = {
        "[output]\noutput_time_format = \"elapsed\"\n",
        "[output]\noutput_time_format = 1\n",
        "[output]\nyear = 0\n",
        "[output]\nyear = 10000\n",
        "[output]\nyear = \"2026\"\n",
        "[output]\nutc_offset = \"+24:00\"\n",
        "[output]\nutc_offset = 5\n",
    };
    for (std::size_t i = 0; i < sizeof(rejected) / sizeof(rejected[0]); ++i) {
        INFO(rejected[i]);
        bool threw = false;
        try {
            mie::parse_into_config(rejected[i]);
        } catch (const mie::ConfigError&) {
            threw = true;
        }
        CHECK(threw);
    }
}

TEST_CASE("a UTC offset parses only in its exact shape", "[config][L2-CFG-012]") {
    int minutes = 0;
    REQUIRE(mie::parse_utc_offset("Z", minutes));
    CHECK(minutes == 0);
    REQUIRE(mie::parse_utc_offset("z", minutes));
    CHECK(minutes == 0);
    REQUIRE(mie::parse_utc_offset("-05:00", minutes));
    CHECK(minutes == -300);
    REQUIRE(mie::parse_utc_offset("+05:30", minutes));
    CHECK(minutes == 330);
    REQUIRE(mie::parse_utc_offset("+23:59", minutes));
    CHECK(minutes == 1439);

    const char* const rejected[] = {"+24:00", "+05:60", "+5:00", "0500",  "PST",
                                    "",       "+05:0",  "05:00", "+05-00"};
    for (std::size_t i = 0; i < sizeof(rejected) / sizeof(rejected[0]); ++i) {
        INFO(rejected[i]);
        CHECK(!mie::parse_utc_offset(rejected[i], minutes));
    }
}

// ---------------------------------------------------------------------------
// Advisory level (L2-LOG-002)
// ---------------------------------------------------------------------------

TEST_CASE("the day-of-year advisory escalates under a calendar rendering",
          "[cli][L2-LOG-001][L2-LOG-002]") {
    // INFO under `doy`, where a skewed day is visibly a day number; WARNING
    // under a calendar rendering, where the same skew is resolved into
    // something that reads as a fact.
    //
    // Captured through the logger's sink hook rather than from the CLI's
    // reporting streams: `run()` reports on the streams it is handed, but log
    // lines go to the sink, which defaults to the process's real stderr.
    const TempFile input("mie-ts-in", recording_on_day_192());

    {
        mie_test::LogCapture capture(mie::log::LEVEL_DEBUG);
        Args argv;
        argv.push_back("--log-level");
        argv.push_back("INFO");
        argv.push_back("decode");
        argv.push_back(input.str());

        std::string out;
        std::string err;
        REQUIRE(run_capturing(argv, out, err) == mie::cli::EXIT_OK);
        REQUIRE(capture.count_containing("day-of-year") == 1);
        CHECK(capture.count_containing("INFO") >= 1);
        CHECK(capture.count_containing("WARN IRIG day-of-year") == 0);
    }

    // A calendar rendering, at the DEFAULT level: present, and at WARN.
    {
        mie_test::LogCapture capture(mie::log::LEVEL_DEBUG);
        Args argv;
        argv.push_back("decode");
        argv.push_back(input.str());
        argv.push_back("--output-time-format");
        argv.push_back("iso");
        argv.push_back("--year");
        argv.push_back("2026");

        std::string out;
        std::string err;
        REQUIRE(run_capturing(argv, out, err) == mie::cli::EXIT_OK);
        REQUIRE(capture.count_containing("day-of-year") == 1);
        CHECK(capture.count_containing("WARN") >= 1);
    }

    // The opt-out still wins, under the calendar rendering too.
    {
        mie_test::LogCapture capture(mie::log::LEVEL_DEBUG);
        Args argv;
        argv.push_back("--no-irig-day-advisory");
        argv.push_back("decode");
        argv.push_back(input.str());
        argv.push_back("--output-time-format");
        argv.push_back("iso");
        argv.push_back("--year");
        argv.push_back("2026");

        std::string out;
        std::string err;
        REQUIRE(run_capturing(argv, out, err) == mie::cli::EXIT_OK);
        CHECK(capture.count_containing("day-of-year") == 0);
    }
}
