// SPDX-License-Identifier: Apache-2.0
//
// The configuration schema — the layer that decides what a parsed document
// MEANS. Grammar-level behaviour is in test_toml.cpp; nothing here re-tests it.
//
// THE PARITY CORPUS. The last section of this file is a port of
// `tests/conformance/config_parity.py`'s corpus, run directly against the
// loader. That harness drives the CLI, which C++ does not have yet, so joining
// it must wait for the CLI module — but the corpus is the accumulated record of
// every form the three implementations have had to agree on, and waiting to
// exercise it would mean building the loader blind to the very cases it exists
// to get right. Running it here now costs nothing and catches the divergence at
// the layer that causes it.
//
// Every `reject` snippet is a form Python's full `tomllib` ACCEPTS and the flat
// schema refuses. That asymmetry is the point: silently storing a
// `output.no_clobber = true` the schema will never read is a safety option that
// appears to be set and is not.

#include "mie/config.hpp"

#include <catch2/catch.hpp>

#include <string>
#include <vector>

#include "log_capture.hpp"
#include "mie/log.hpp"
#include "temp_path.hpp"

namespace {

using mie_test::LogCapture;
using mie_test::TempPath;

mie::DecoderConfig must_load(const std::string& text) {
    INFO("config: " << text);
    return mie::parse_into_config(text);
}

/// Load, requiring a ConfigError, and return its message.
std::string must_fail(const std::string& text) {
    INFO("config: " << text);
    try {
        mie::parse_into_config(text);
    } catch (const mie::ConfigError& error) {
        return error.message();
    }
    FAIL("expected a ConfigError");
    return std::string();
}

}  // namespace

// ---------------------------------------------------------------------------
// Defaults
// ---------------------------------------------------------------------------

TEST_CASE("a default configuration is the documented no-config behaviour", "[config][L2-CFG-001]") {
    // A default-constructed instance is a VALID configuration, not a
    // placeholder: running with no config file must not need a second code
    // path.
    const mie::DecoderConfig config;
    CHECK(config.log_level == "WARNING");
    CHECK(config.time_format == mie::TIMESTAMP_AUTO);
    CHECK_FALSE(config.strict);
    CHECK(config.error_mode == mie::ERROR_MODE_INLINE);
    CHECK(config.output_format == "csv");
    CHECK_FALSE(config.no_clobber);
    CHECK_FALSE(config.allow_partial);
    CHECK(config.detect_records == mie::decode::DEFAULT_DETECT_RECORDS);
    CHECK(config.lookahead_records == mie::sync::DEFAULT_LOOKAHEAD_RECORDS);
    CHECK_FALSE(config.standard_tick_rate_hz.has_value());
    CHECK(config.mux_enabled);
    CHECK(config.mux_field == mie::decode::DEFAULT_MUX_FIELD);
    CHECK_FALSE(config.collapse_duplicates);
    CHECK(config.collapse_window_us == 0);
    CHECK(config.delta_scope == mie::DELTA_SCOPE_PER_FILE);
    CHECK(config.max_sort_group == mie::DEFAULT_MAX_SORT_GROUP);
    CHECK_FALSE(config.filters.is_active());
}

TEST_CASE("an empty document leaves every default in place", "[config][L2-CFG-001]") {
    const mie::DecoderConfig config = must_load("");
    const mie::DecoderConfig defaults;
    CHECK(config.log_level == defaults.log_level);
    CHECK(config.detect_records == defaults.detect_records);
    CHECK(config.max_sort_group == defaults.max_sort_group);
}

// ---------------------------------------------------------------------------
// [logging]
// ---------------------------------------------------------------------------

TEST_CASE("logging.level accepts every spelling the logger does", "[config][L2-CLI-004]") {
    CHECK(must_load("[logging]\nlevel = \"DEBUG\"\n").log_level == "DEBUG");
    CHECK(must_load("[logging]\nlevel = \"warning\"\n").log_level == "WARNING");
    CHECK(must_load("[logging]\nlevel = \"WARN\"\n").log_level == "WARN");
    CHECK(must_load("[logging]\nlevel = \"critical\"\n").log_level == "CRITICAL");
    CHECK(must_load("[logging]\nlevel = \"off\"\n").log_level == "OFF");
}

TEST_CASE("an unknown logging.level is refused at load time", "[config][L2-CFG-010]") {
    // Validated here rather than when the level is first used, so the error
    // arrives while the operator is looking at the config file.
    CHECK(must_fail("[logging]\nlevel = \"LOUD\"\n").find("Invalid logging.level") == 0);
    must_fail("[logging]\nlevel = \"\"\n");
    must_fail("[logging]\nlevel = 1\n");
}

// ---------------------------------------------------------------------------
// [decode]
// ---------------------------------------------------------------------------

TEST_CASE("decode keys map onto their fields", "[config][L2-CFG-001]") {
    const mie::DecoderConfig config = must_load(
        "[decode]\n"
        "time_format = \"irig\"\n"
        "strict = true\n"
        "error_mode = \"separate\"\n"
        "allow_partial = true\n"
        "detect_records = 16\n"
        "lookahead_records = 4\n"
        "standard_tick_rate_hz = 1000000.0\n");

    CHECK(config.time_format == mie::TIMESTAMP_IRIG);
    CHECK(config.strict);
    CHECK(config.error_mode == mie::ERROR_MODE_SEPARATE);
    CHECK(config.allow_partial);
    CHECK(config.detect_records == 16);
    CHECK(config.lookahead_records == 4);
    REQUIRE(config.standard_tick_rate_hz.has_value());
    CHECK(config.standard_tick_rate_hz.value() == Approx(1000000.0));
}

TEST_CASE("the record-count ranges are enforced at both ends", "[config][L2-CFG-010]") {
    CHECK(must_load("[decode]\ndetect_records = 1\n").detect_records == 1);
    CHECK(must_load("[decode]\ndetect_records = 32\n").detect_records == 32);
    CHECK(must_fail("[decode]\ndetect_records = 0\n").find("Invalid decode.detect_records") == 0);
    must_fail("[decode]\ndetect_records = 33\n");
    must_fail("[decode]\ndetect_records = -1\n");

    CHECK(must_load("[decode]\nlookahead_records = 1\n").lookahead_records == 1);
    CHECK(must_load("[decode]\nlookahead_records = 32\n").lookahead_records == 32);
    must_fail("[decode]\nlookahead_records = 0\n");
    must_fail("[decode]\nlookahead_records = 33\n");
}

TEST_CASE("an integer key refuses a float", "[config][L2-CFG-010]") {
    // Deliberately asymmetric with the rate key below. `detect_records = 8.0`
    // is a mistake worth naming rather than rounding.
    CHECK(must_fail("[decode]\ndetect_records = 8.0\n") ==
          "[decode] detect_records must be an integer");
}

TEST_CASE("the tick rate accepts an integer as well as a float", "[config][L2-DEC-017]") {
    // The one place the asymmetry runs the other way: an operator writing a
    // frequency should not have to remember a decimal point.
    REQUIRE(
        must_load("[decode]\nstandard_tick_rate_hz = 1000000\n").standard_tick_rate_hz.has_value());
    CHECK(must_load("[decode]\nstandard_tick_rate_hz = 1000000\n").standard_tick_rate_hz.value() ==
          Approx(1000000.0));
}

TEST_CASE("the tick rate must be finite and positive", "[config][L2-DEC-017]") {
    // A non-finite rate would make every converted timestamp a NaN; a zero or
    // negative one would invert the timeline.
    CHECK(must_fail("[decode]\nstandard_tick_rate_hz = 0.0\n")
              .find("Invalid decode.standard_tick_rate_hz") == 0);
    must_fail("[decode]\nstandard_tick_rate_hz = -1.0\n");
    must_fail("[decode]\nstandard_tick_rate_hz = 0\n");
}

TEST_CASE("time_format and error_mode take only their documented spellings",
          "[config][L2-CFG-010]") {
    CHECK(must_load("[decode]\ntime_format = \"AUTO\"\n").time_format == mie::TIMESTAMP_AUTO);
    CHECK(must_load("[decode]\ntime_format = \"Standard\"\n").time_format ==
          mie::TIMESTAMP_STANDARD);
    must_fail("[decode]\ntime_format = \"gps\"\n");

    CHECK(must_load("[decode]\nerror_mode = \"INLINE\"\n").error_mode == mie::ERROR_MODE_INLINE);
    must_fail("[decode]\nerror_mode = \"merged\"\n");
}

// ---------------------------------------------------------------------------
// [output], [mux], [merge]
// ---------------------------------------------------------------------------

TEST_CASE("output keys map onto their fields", "[config][L2-WRT-017][L2-WRT-022]") {
    CHECK(must_load("[output]\nno_clobber = true\n").no_clobber);
    CHECK(must_load("[output]\nformat = \"csv\"\n").output_format == "csv");
    CHECK(must_load("[output]\nmax_sort_group = 64\n").max_sort_group == 64);
    CHECK(must_load("[output]\nmax_sort_group = 1\n").max_sort_group == 1);
    CHECK(must_load("[output]\nmax_sort_group = 1048576\n").max_sort_group == 1048576);

    must_fail("[output]\nformat = \"json\"\n");
    must_fail("[output]\nmax_sort_group = 0\n");
    must_fail("[output]\nmax_sort_group = 1048577\n");
    must_fail("[output]\nmax_sort_group = -1\n");
}

TEST_CASE("mux keys map onto their fields", "[config][L2-WRT-020]") {
    CHECK_FALSE(must_load("[mux]\nenabled = false\n").mux_enabled);
    CHECK(must_load("[mux]\ndelimiter = \"_\"\n").mux_delimiter == "_");
    CHECK(must_load("[mux]\nfield = 2\n").mux_field == 2);
    // Negative counts from the end, and an index past either end simply yields
    // an empty column rather than an error -- so it is NOT range-checked.
    CHECK(must_load("[mux]\nfield = -1\n").mux_field == -1);
    CHECK(must_load("[mux]\nfield = 999\n").mux_field == 999);

    CHECK(must_fail("[mux]\ndelimiter = \"\"\n").find("Invalid mux.delimiter") == 0);
}

TEST_CASE("merge keys map onto their fields", "[config][L2-MRG-005][L2-MRG-007]") {
    CHECK(must_load("[merge]\ncollapse_duplicates = true\n").collapse_duplicates);
    CHECK(must_load("[merge]\ncollapse_window_us = 100\n").collapse_window_us == 100);
    CHECK(must_load("[merge]\ndelta_scope = \"global\"\n").delta_scope == mie::DELTA_SCOPE_GLOBAL);
    CHECK(must_load("[merge]\ndelta_scope = \"per-file\"\n").delta_scope ==
          mie::DELTA_SCOPE_PER_FILE);
    CHECK(must_load("[merge]\ndelta_scope = \"Per-File\"\n").delta_scope ==
          mie::DELTA_SCOPE_PER_FILE);

    must_fail("[merge]\ndelta_scope = \"whole\"\n");
    must_fail("[merge]\ndelta_scope = 1\n");
    CHECK(
        must_fail("[merge]\ncollapse_window_us = -1\n").find("Invalid merge.collapse_window_us") ==
        0);
}

// ---------------------------------------------------------------------------
// [filter]
// ---------------------------------------------------------------------------

TEST_CASE("filter arrays accept names, hex codes and integers", "[config][L2-FLT-001]") {
    const mie::DecoderConfig config = must_load(
        "[filter]\n"
        "exclude_types = [\"BC_TO_RT\", \"0x04\", 8]\n"
        "exclude_rts = [0, 31]\n"
        "exclude_buses = [\"A\", \"b\"]\n"
        "exclude_subaddresses = [5]\n");

    REQUIRE(config.filters.exclude_types.size() == 3);
    CHECK(config.filters.exclude_types[0] == mie::MESSAGE_TYPE_BC_TO_RT);
    CHECK(config.filters.exclude_types[1] == mie::MESSAGE_TYPE_RT_TO_BC);
    CHECK(config.filters.exclude_types[2] == mie::MESSAGE_TYPE_RT_TO_RT);
    CHECK(config.filters.exclude_rts.size() == 2);
    REQUIRE(config.filters.exclude_buses.size() == 2);
    CHECK(config.filters.exclude_buses[0] == mie::BUS_A);
    CHECK(config.filters.exclude_buses[1] == mie::BUS_B);
    CHECK(config.filters.is_active());
}

TEST_CASE("an empty filter array is valid and inactive", "[config][L2-FLT-001]") {
    CHECK_FALSE(must_load("[filter]\nexclude_rts = []\n").filters.is_active());
}

TEST_CASE("filter entries are validated element by element", "[config][L2-CFG-010]") {
    must_fail("[filter]\nexclude_types = [\"NOT_A_TYPE\"]\n");
    must_fail("[filter]\nexclude_buses = [\"C\"]\n");
    must_fail("[filter]\nexclude_buses = [1]\n");
    // RT and subaddress are both five-bit fields on the wire.
    must_fail("[filter]\nexclude_rts = [32]\n");
    must_fail("[filter]\nexclude_rts = [-1]\n");
    must_fail("[filter]\nexclude_subaddresses = [32]\n");
    must_fail("[filter]\nexclude_rts = [\"5\"]\n");
    // The array itself must be an array.
    CHECK(must_fail("[filter]\nexclude_rts = 5\n") == "[filter] exclude_rts must be an array");
}

TEST_CASE("message type names resolve, and unknown ones do not", "[config]") {
    CHECK(mie::parse_type_name("BC_TO_RT") == mie::MESSAGE_TYPE_BC_TO_RT);
    CHECK(mie::parse_type_name("bc_to_rt") == mie::MESSAGE_TYPE_BC_TO_RT);
    CHECK(mie::parse_type_name("  RT_TO_RT  ") == mie::MESSAGE_TYPE_RT_TO_RT);
    CHECK(mie::parse_type_name("0x20") == mie::MESSAGE_TYPE_SPURIOUS_DATA);
    CHECK(mie::parse_type_name("0X20") == mie::MESSAGE_TYPE_SPURIOUS_DATA);
    CHECK_THROWS_AS(mie::parse_type_name("NOPE"), mie::ConfigError);
    CHECK_THROWS_AS(mie::parse_type_name("0xZZ"), mie::ConfigError);
    CHECK_THROWS_AS(mie::parse_type_name(""), mie::ConfigError);
}

TEST_CASE("bus names resolve case-insensitively", "[config]") {
    CHECK(mie::parse_bus_name("A") == mie::BUS_A);
    CHECK(mie::parse_bus_name("b") == mie::BUS_B);
    CHECK(mie::parse_bus_name(" B ") == mie::BUS_B);
    CHECK_THROWS_AS(mie::parse_bus_name("C"), mie::ConfigError);
    CHECK_THROWS_AS(mie::parse_bus_name(""), mie::ConfigError);
}

// ---------------------------------------------------------------------------
// Type errors and unknown keys
// ---------------------------------------------------------------------------

TEST_CASE("a wrong-typed value names the section, key and wanted type", "[config][L2-CFG-010]") {
    CHECK(must_fail("[decode]\nstrict = 1\n") == "[decode] strict must be a boolean");
    CHECK(must_fail("[decode]\ntime_format = true\n") == "[decode] time_format must be a string");
    CHECK(must_fail("[output]\nmax_sort_group = \"64\"\n") ==
          "[output] max_sort_group must be an integer");
    CHECK(must_fail("[output]\nmax_sort_group = true\n") ==
          "[output] max_sort_group must be an integer");
    CHECK(must_fail("[decode]\nstandard_tick_rate_hz = \"fast\"\n") ==
          "[decode] standard_tick_rate_hz must be a number");
}

TEST_CASE("an unknown key warns and is ignored", "[config][L2-CFG-009]") {
    // Non-fatal by design: a config written for a newer build must still load
    // on an older one, or a shared site config becomes un-upgradable.
    const LogCapture capture(mie::log::LEVEL_WARN);
    const mie::DecoderConfig config =
        must_load("[decode]\nexclude_subdresses = [1]\nstrict = true\n");
    CHECK(config.strict);
    CHECK(capture.contains("unknown config key 'decode.exclude_subdresses'"));
    CHECK(capture.contains("line 2"));
}

TEST_CASE("an unknown section's keys warn too", "[config][L2-CFG-009]") {
    const LogCapture capture(mie::log::LEVEL_WARN);
    must_load("[bogus]\nunknown_key = 1\n");
    CHECK(capture.contains("unknown config key 'bogus.unknown_key'"));
}

TEST_CASE("a known key produces no warning", "[config][L2-CFG-009]") {
    const LogCapture capture(mie::log::LEVEL_WARN);
    must_load("[decode]\nstrict = true\n[output]\nno_clobber = true\n");
    CHECK(capture.count_containing("unknown config key") == 0);
}

TEST_CASE("a section written as a scalar is an error, not an unknown key", "[config][L2-CFG-010]") {
    // `decode = true` instead of a `[decode]` header. Reporting it as an
    // unknown key would leave the operator staring at the word `decode` in
    // their file wondering why it was ignored.
    CHECK(must_fail("decode = true\n") == "Invalid [decode]: expected a table, not a value");
    must_fail("filter = 1\n");
    // A root-level key that is NOT a section name is merely unknown.
    const LogCapture capture(mie::log::LEVEL_WARN);
    must_load("stray = 1\n");
    CHECK(capture.contains("unknown config key 'stray'"));
}

TEST_CASE("is_known_section and is_known_key describe the schema", "[config]") {
    CHECK(mie::is_known_section("decode"));
    CHECK(mie::is_known_section("filter"));
    CHECK_FALSE(mie::is_known_section("decoding"));
    CHECK_FALSE(mie::is_known_section(""));

    CHECK(mie::is_known_key("decode", "strict"));
    CHECK(mie::is_known_key("filter", "exclude_rts"));
    CHECK_FALSE(mie::is_known_key("decode", "exclude_rts"));
    CHECK_FALSE(mie::is_known_key("nope", "strict"));
}

// ---------------------------------------------------------------------------
// Loading from disk
// ---------------------------------------------------------------------------

TEST_CASE("an absent path yields the defaults", "[config]") {
    const mie::DecoderConfig config = mie::load_config(mie::Optional<std::string>());
    CHECK(config.log_level == "WARNING");
}

TEST_CASE("a named config file is read", "[config][L2-CFG-001]") {
    const mie_test::TempFile file("cfg.toml", std::string("[decode]\nstrict = true\n"));
    const mie::DecoderConfig config = mie::load_config(mie::Optional<std::string>(file.str()));
    CHECK(config.strict);
}

TEST_CASE("a named config file that does not exist is an error", "[config]") {
    // Not silently ignored: an operator who named a file expects it to be used,
    // and falling back to defaults would decode with settings they did not ask
    // for and never see.
    const TempPath missing("absent.toml");
    CHECK_THROWS_AS(mie::load_config(mie::Optional<std::string>(missing.str())), mie::ConfigError);
}

TEST_CASE("a schema error from a file names the file", "[config]") {
    const mie_test::TempFile file("bad.toml", std::string("[decode]\ndetect_records = 99\n"));
    try {
        mie::load_config(mie::Optional<std::string>(file.str()));
        FAIL("expected a ConfigError");
    } catch (const mie::ConfigError& error) {
        // An operator with several config files needs to know which one.
        CHECK(error.message().find(file.str()) == 0);
        CHECK(error.message().find("detect_records") != std::string::npos);
    }
}

// ---------------------------------------------------------------------------
// Precedence
// ---------------------------------------------------------------------------

TEST_CASE("an override replaces the loaded value", "[config][L2-CFG-002]") {
    const mie::DecoderConfig base = must_load("[decode]\nstrict = false\ndetect_records = 4\n");
    mie::ConfigOverrides overrides;
    overrides.strict = true;
    overrides.detect_records = static_cast<std::size_t>(16);

    const mie::DecoderConfig merged = mie::with_overrides(base, overrides);
    CHECK(merged.strict);
    CHECK(merged.detect_records == 16);
}

TEST_CASE("an absent override leaves the loaded value alone", "[config][L2-CFG-002]") {
    // Absent means "not supplied", which is why the fields are Optional: a
    // sentinel could not tell `--strict=false` from no flag at all.
    const mie::DecoderConfig base = must_load("[decode]\nstrict = true\n");
    const mie::DecoderConfig merged = mie::with_overrides(base, mie::ConfigOverrides());
    CHECK(merged.strict);
}

TEST_CASE("an override can set a value back to its default", "[config][L2-CFG-002]") {
    const mie::DecoderConfig base = must_load("[decode]\nstrict = true\n");
    mie::ConfigOverrides overrides;
    overrides.strict = false;
    CHECK_FALSE(mie::with_overrides(base, overrides).strict);
}

TEST_CASE("filter overrides MERGE rather than replace", "[config][L2-CFG-002]") {
    // An operator adding one --exclude-rt flag does not expect to lose the
    // exclusions their config file already set.
    const mie::DecoderConfig base = must_load("[filter]\nexclude_rts = [9]\n");
    mie::ConfigOverrides overrides;
    overrides.filters.exclude_rts.push_back(5);

    const mie::DecoderConfig merged = mie::with_overrides(base, overrides);
    REQUIRE(merged.filters.exclude_rts.size() == 2);
    CHECK(merged.filters.exclude_rts[0] == 9);
    CHECK(merged.filters.exclude_rts[1] == 5);
}

TEST_CASE("every override field is wired", "[config][L2-CFG-002]") {
    // A field added to DecoderConfig and to ConfigOverrides but never applied
    // in with_overrides would be a CLI flag that silently does nothing. Each
    // one is set to something distinguishable from the default.
    mie::ConfigOverrides o;
    o.log_level = std::string("DEBUG");
    o.time_format = mie::TIMESTAMP_STANDARD;
    o.strict = true;
    o.error_mode = mie::ERROR_MODE_SEPARATE;
    o.output_format = std::string("csv");
    o.no_clobber = true;
    o.allow_partial = true;
    o.detect_records = static_cast<std::size_t>(3);
    o.lookahead_records = static_cast<std::size_t>(5);
    o.standard_tick_rate_hz = 2000.0;
    o.mux_enabled = false;
    o.mux_delimiter = std::string("|");
    o.mux_field = static_cast<int64_t>(7);
    o.collapse_duplicates = true;
    o.collapse_window_us = static_cast<uint64_t>(250);
    o.delta_scope = mie::DELTA_SCOPE_GLOBAL;
    o.max_sort_group = static_cast<std::size_t>(11);

    const mie::DecoderConfig merged = mie::with_overrides(mie::DecoderConfig(), o);
    CHECK(merged.log_level == "DEBUG");
    CHECK(merged.time_format == mie::TIMESTAMP_STANDARD);
    CHECK(merged.strict);
    CHECK(merged.error_mode == mie::ERROR_MODE_SEPARATE);
    CHECK(merged.no_clobber);
    CHECK(merged.allow_partial);
    CHECK(merged.detect_records == 3);
    CHECK(merged.lookahead_records == 5);
    REQUIRE(merged.standard_tick_rate_hz.has_value());
    CHECK(merged.standard_tick_rate_hz.value() == Approx(2000.0));
    CHECK_FALSE(merged.mux_enabled);
    CHECK(merged.mux_delimiter == "|");
    CHECK(merged.mux_field == 7);
    CHECK(merged.collapse_duplicates);
    CHECK(merged.collapse_window_us == 250);
    CHECK(merged.delta_scope == mie::DELTA_SCOPE_GLOBAL);
    CHECK(merged.max_sort_group == 11);
}

// ---------------------------------------------------------------------------
// The conformance parity corpus, run against the loader
// ---------------------------------------------------------------------------

TEST_CASE("the config parity corpus lands in the expected class", "[config][L2-CFG-010][parity]") {
    // Ported from tests/conformance/config_parity.py. That harness compares
    // exit codes from the Rust and Python CLIs; this runs the same snippets
    // against the loader directly, because the C++ CLI does not exist yet and
    // building the loader without this corpus would mean building it blind to
    // the exact forms the three implementations have had to be aligned on.
    //
    // When the CLI lands, C++ joins the real harness and this stays as the
    // fast, layer-local version of the same statement.
    struct Snippet {
        const char* name;
        const char* toml;
        bool accept;
    };
    static const Snippet corpus[] = {
        // --- valid flat forms -------------------------------------------
        {"flat-strict", "[decode]\nstrict = true\n", true},
        {"comment-only", "# just a comment\n", true},
        {"empty-file", "", true},
        {"string-value", "[decode]\ntime_format = \"irig\"\n", true},
        {"int-value", "[decode]\ndetect_records = 8\n", true},
        {"float-value", "[decode]\nstandard_tick_rate_hz = 1000000.0\n", true},
        {"bool-value", "[output]\nno_clobber = true\n", true},
        {"int-array", "[filter]\nexclude_rts = [0, 31]\n", true},
        {"string-array", "[filter]\nexclude_buses = [\"A\", \"B\"]\n", true},
        {"trailing-comment", "[decode]\nstrict = true  # yes\n", true},
        {"extra-whitespace", "[decode]\n  strict   =   true  \n", true},
        {"blank-lines", "[decode]\n\n\nstrict = true\n", true},
        {"negative-mux-field", "[mux]\nfield = -1\n", true},
        {"delta-scope-per-file", "[merge]\ndelta_scope = \"per-file\"\n", true},
        {"delta-scope-global", "[merge]\ndelta_scope = \"global\"\n", true},
        {"delta-scope-mixed-case", "[merge]\ndelta_scope = \"Per-File\"\n", true},
        {"max-sort-group-valid", "[output]\nmax_sort_group = 64\n", true},
        {"max-sort-group-min", "[output]\nmax_sort_group = 1\n", true},
        {"max-sort-group-max", "[output]\nmax_sort_group = 1048576\n", true},
        // An unknown key WARNs and is ignored; it does not fail the load.
        {"unknown-key-in-known-section", "[decode]\nnot_a_key = 1\n", true},
        {"array-escaped-quote", "[bogus]\nunknown_key = [\"a\\\", b\"]\n", true},

        // --- schema violations ------------------------------------------
        {"delta-scope-unknown", "[merge]\ndelta_scope = \"whole\"\n", false},
        {"delta-scope-non-string", "[merge]\ndelta_scope = 1\n", false},
        {"max-sort-group-zero", "[output]\nmax_sort_group = 0\n", false},
        {"max-sort-group-over", "[output]\nmax_sort_group = 1048577\n", false},
        {"max-sort-group-negative", "[output]\nmax_sort_group = -1\n", false},
        {"max-sort-group-bool", "[output]\nmax_sort_group = true\n", false},
        {"max-sort-group-string", "[output]\nmax_sort_group = \"64\"\n", false},

        // --- full TOML the flat schema cannot model ---------------------
        {"inline-table", "[decode]\nx = { a = 1 }\n", false},
        {"multiline-array", "[filter]\nexclude_rts = [\n  1,\n]\n", false},
        {"dotted-key", "[decode]\na.b = 1\n", false},
        {"quoted-key", "[decode]\n\"strict\" = true\n", false},
        {"dotted-section", "[output.no_clobber]\nx = 1\n", false},
        {"array-of-tables", "[[decode]]\nstrict = true\n", false},
        {"duplicate-section", "[decode]\nx = 1\n[decode]\ny = 2\n", false},
        {"duplicate-key", "[decode]\nstrict = true\nstrict = false\n", false},
        {"section-as-scalar", "decode = true\n", false},
        {"leading-zero-int", "[decode]\ndetect_records = 08\n", false},
        {"underscore-int", "[decode]\ndetect_records = 1_6\n", false},
        {"hex-int", "[decode]\ndetect_records = 0x10\n", false},
        {"bare-trailing-dot", "[decode]\nstandard_tick_rate_hz = 1.\n", false},
    };

    std::vector<std::string> failures;
    for (std::size_t i = 0; i < sizeof(corpus) / sizeof(corpus[0]); ++i) {
        const Snippet& snippet = corpus[i];
        bool accepted = false;
        try {
            mie::parse_into_config(snippet.toml);
            accepted = true;
        } catch (const mie::ConfigError&) {
            accepted = false;
        }
        if (accepted != snippet.accept) {
            failures.push_back(std::string(snippet.name) + ": got " +
                               (accepted ? "accept" : "reject") + ", expected " +
                               (snippet.accept ? "accept" : "reject"));
        }
    }

    for (std::size_t i = 0; i < failures.size(); ++i) {
        FAIL_CHECK(failures[i]);
    }
    CHECK(failures.empty());
}
