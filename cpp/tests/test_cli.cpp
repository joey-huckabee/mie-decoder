// SPDX-License-Identifier: Apache-2.0
//
// CLI tests: argument parsing, precedence, and the exit-code contract.
//
// The shared conformance oracles already prove the CSV bytes are right. What
// they cannot reach is everything that happens BEFORE a decode starts -- a
// mistyped flag, a value out of range, a flag belonging to an unported module
// -- because a case that never produces a CSV has no oracle to compare against.
// Those failures are exactly the ones an operator hits first, so they are
// tested here, where `run()` is callable in-process and the exit code is an
// ordinary return value.

#include "mie/cli.hpp"

#include <catch2/catch.hpp>

#include <cstdio>
#include <string>
#include <vector>

#include "log_capture.hpp"
#include "mie/log.hpp"
#include "temp_path.hpp"

namespace {

using mie_test::TempFile;
using mie_test::TempPath;

typedef std::vector<std::string> Args;

/// Build an argument vector without C++11 initializer-list-into-vector, which
/// reads badly at every call site once there are six of them.
Args args(const std::string& a0, const std::string& a1 = std::string(),
          const std::string& a2 = std::string(), const std::string& a3 = std::string(),
          const std::string& a4 = std::string(), const std::string& a5 = std::string()) {
    Args out;
    out.push_back(a0);
    const std::string rest[5] = {a1, a2, a3, a4, a5};
    for (std::size_t i = 0; i < 5; ++i) {
        if (!rest[i].empty()) {
            out.push_back(rest[i]);
        }
    }
    return out;
}

std::vector<uint8_t> le_bytes(const std::vector<uint16_t>& words) {
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
/// The word count is the count for the WHOLE record -- type word, timestamp,
/// command, payload and status -- not the payload alone. Writing the payload
/// count here produces a file whose first record ends four words early, and the
/// reader then reads the command word as the next type word and reports a sync
/// loss that has nothing to do with what is being tested.
uint16_t type_word(uint8_t message_type, uint16_t word_count) {
    return static_cast<uint16_t>((message_type & 0x7F) | ((word_count & 0x3F) << 8));
}

/// Command Word: RT in 11-15, direction in 10, subaddress in 5-9, data word
/// count in 0-4.
uint16_t command_word(uint8_t rt, uint8_t subaddress, uint8_t data_word_count) {
    return static_cast<uint16_t>(((rt & 0x1F) << 11) | ((subaddress & 0x1F) << 5) |
                                 (data_word_count & 0x1F));
}

void push_irig(std::vector<uint16_t>& words, uint32_t microsecond) {
    REQUIRE(microsecond < 1000000u);
    const uint16_t upper = static_cast<uint16_t>(15 | (10 << 5));
    uint16_t middle = static_cast<uint16_t>((microsecond >> 16) & 0xF);
    middle = static_cast<uint16_t>(middle | (50 << 4));
    middle = static_cast<uint16_t>(middle | (54 << 10));
    words.push_back(upper);
    words.push_back(middle);
    words.push_back(static_cast<uint16_t>(microsecond & 0xFFFF));
}

/// A minimal well-formed recording: two BC-to-RT records and the terminator.
///
/// Two, not one: entry validation confirms a candidate by looking ahead, so a
/// single-record file exercises a different path than the one these tests are
/// about. The payload varies with the record's time because L2-SYN-018 rejects
/// an input whose leading records are byte-identical outside the timestamp.
std::vector<uint8_t> valid_recording() {
    const uint8_t kRt = 15;
    const uint8_t kSubaddress = 11;
    const uint8_t kDataWords = 2;
    std::vector<uint16_t> words;
    for (uint32_t i = 0; i < 2; ++i) {
        const uint32_t microsecond = 100 + i * 100;
        // BC-to-RT layout: Type, IRIG, Command, payload, Status.
        words.push_back(type_word(0x01, static_cast<uint16_t>(6 + kDataWords)));
        push_irig(words, microsecond);
        words.push_back(command_word(kRt, kSubaddress, kDataWords));
        for (uint8_t w = 0; w < kDataWords; ++w) {
            words.push_back(static_cast<uint16_t>(0x1100 + (microsecond & 0xFF) + w));
        }
        words.push_back(static_cast<uint16_t>((kRt & 0x1F) << 11));  // status word
    }
    words.push_back(0x0000);  // end-of-records terminator
    return le_bytes(words);
}

/// Run the CLI with its reporting streams pointed at temporary files, and
/// return what it wrote to each.
///
/// No file-descriptor juggling: `run()` takes the streams it reports on, which
/// is the whole reason that seam exists.
int run_capturing(const Args& argv, std::string& out, std::string& err) {
    const TempPath out_path("mie-cli-out");
    const TempPath err_path("mie-cli-err");
    int code = 0;
    {
        std::FILE* out_handle = std::fopen(out_path.str().c_str(), "wb");
        std::FILE* err_handle = std::fopen(err_path.str().c_str(), "wb");
        REQUIRE(out_handle != NULL);
        REQUIRE(err_handle != NULL);
        code = mie::cli::run(argv, mie::cli::Streams(out_handle, err_handle));
        // Checked, not voided: a failed close can mean the bytes never
        // reached the file, and a test that then read an empty capture would
        // report a CLI bug that does not exist.
        REQUIRE(std::fclose(out_handle) == 0);
        REQUIRE(std::fclose(err_handle) == 0);
    }
    REQUIRE(mie_test::read_file(out_path.str(), out));
    REQUIRE(mie_test::read_file(err_path.str(), err));
    return code;
}

int run_capturing_stdout(const Args& argv, std::string& out) {
    std::string ignored;
    return run_capturing(argv, out, ignored);
}

}  // namespace

TEST_CASE("version and help are answered before anything else", "[cli][L3-CPP-014]") {
    SECTION("every accepted spelling exits 0") {
        const char* spellings[] = {"--version", "-V", "-v", "--VERSION", "--help", "-h", "--HELP"};
        for (std::size_t i = 0; i < sizeof(spellings) / sizeof(spellings[0]); ++i) {
            std::string out;
            INFO(spellings[i]);
            REQUIRE(run_capturing_stdout(args(spellings[i]), out) == mie::cli::EXIT_OK);
            REQUIRE_FALSE(out.empty());
        }
    }

    SECTION("help wins over an otherwise broken invocation") {
        // An operator whose command line is wrong is the one most likely to ask
        // for help. Validating first would answer the question they did not ask.
        std::string out;
        REQUIRE(run_capturing_stdout(args("decode", "--nonsense", "--help"), out) ==
                mie::cli::EXIT_OK);
        REQUIRE(out.find("USAGE") != std::string::npos);
    }

    SECTION("the help text names every implemented subcommand") {
        const std::string help = mie::cli::help_text();
        REQUIRE(help.find("decode") != std::string::npos);
        REQUIRE(help.find("count") != std::string::npos);
    }
}

TEST_CASE("a bare invocation is a usage error, not a success", "[cli][L3-CPP-014]") {
    // Exit 0 here would let a script that forgot to pass its arguments look
    // like it had done its job.
    REQUIRE(mie::cli::run(Args()) == mie::cli::EXIT_USAGE);
}

TEST_CASE("unknown commands and options are usage errors", "[cli][L3-CPP-014]") {
    REQUIRE(mie::cli::run(args("frobnicate")) == mie::cli::EXIT_USAGE);
    REQUIRE(mie::cli::run(args("decode", "--frobnicate", "x.mie")) == mie::cli::EXIT_USAGE);
    REQUIRE(mie::cli::run(args("count", "--frobnicate", "x.mie")) == mie::cli::EXIT_USAGE);
    // No input at all.
    REQUIRE(mie::cli::run(args("decode")) == mie::cli::EXIT_USAGE);
    REQUIRE(mie::cli::run(args("count")) == mie::cli::EXIT_USAGE);
}

TEST_CASE("a valued flag given no value is a usage error", "[cli][L3-CPP-014]") {
    // The trailing-flag case: `--output` as the last token has nothing after
    // it. Reading past the end would be a crash; treating the absence as an
    // empty value would write to a file named "".
    const char* flags[] = {"--output",
                           "--config",
                           "--log-level",
                           "--time-format",
                           "--format",
                           "--detect-records",
                           "--lookahead-records",
                           "--standard-tick-rate-hz",
                           "--max-sort-group",
                           "--mux-delimiter",
                           "--mux-field",
                           "--exclude-types",
                           "--include-types",
                           "--exclude-rts",
                           "--include-rts",
                           "--exclude-buses",
                           "--include-buses",
                           "--exclude-subaddresses",
                           "--include-subaddresses"};
    for (std::size_t i = 0; i < sizeof(flags) / sizeof(flags[0]); ++i) {
        INFO(flags[i]);
        REQUIRE(mie::cli::run(args("decode", "in.mie", flags[i])) == mie::cli::EXIT_USAGE);
    }
    REQUIRE(mie::cli::run(args("decode", "in.mie", "-o")) == mie::cli::EXIT_USAGE);
}

TEST_CASE("--flag=value and --flag value are the same thing", "[cli][L3-CPP-014]") {
    // Both spellings go through one cursor rather than being written out per
    // flag, so this sweep is what proves the cursor -- not twenty near-copies.
    const TempFile input("mie-cli-forms.mie", valid_recording());

    struct Case {
        const char* flag;
        const char* value;
    };
    const Case cases[] = {
        {"--time-format", "irig"},
        {"--format", "csv"},
        {"--detect-records", "4"},
        {"--lookahead-records", "2"},
        {"--standard-tick-rate-hz", "1000000"},
        {"--max-sort-group", "64"},
        {"--mux-delimiter", "_"},
        {"--mux-field", "0"},
        {"--exclude-rts", "31"},
        {"--include-rts", "15"},
        {"--exclude-buses", "B"},
        {"--include-buses", "A"},
        {"--exclude-subaddresses", "30"},
        {"--include-subaddresses", "11"},
        {"--exclude-types", "RT_TO_RT"},
        {"--include-types", "BC_TO_RT"},
    };

    for (std::size_t i = 0; i < sizeof(cases) / sizeof(cases[0]); ++i) {
        const TempPath separated("mie-cli-sep.csv");
        const TempPath joined("mie-cli-join.csv");
        INFO(cases[i].flag);

        const Args split_form =
            args("decode", input.str(), "-o", separated.str(), cases[i].flag, cases[i].value);
        const Args joined_form = args("decode", input.str(), "-o", joined.str(),
                                      std::string(cases[i].flag) + "=" + cases[i].value);

        REQUIRE(mie::cli::run(split_form) == mie::cli::EXIT_OK);
        REQUIRE(mie::cli::run(joined_form) == mie::cli::EXIT_OK);

        // Same bytes, not merely the same exit code: a form that parsed but
        // dropped the value would still exit 0.
        std::string a;
        std::string b;
        REQUIRE(mie_test::read_file(separated.str(), a));
        REQUIRE(mie_test::read_file(joined.str(), b));
        REQUIRE(a == b);
    }
}

TEST_CASE("numeric flags reject trailing junk", "[cli][L3-CPP-014]") {
    // `strtoll` stops at the first non-digit and reports success, so "4x" would
    // silently become 4. A typo must be refused, not rounded off.
    const char* bad[] = {"4x", "", "  ", "0x4", "4.5", "--", "1e3", "4 "};
    for (std::size_t i = 0; i < sizeof(bad) / sizeof(bad[0]); ++i) {
        INFO(bad[i]);
        REQUIRE(mie::cli::run(args("decode", "in.mie", "--detect-records", bad[i])) ==
                mie::cli::EXIT_USAGE);
    }
}

TEST_CASE("bounded flags reject out-of-range values", "[cli][L3-CPP-014]") {
    static const char* const out_of_range[][2] = {
        {"--detect-records", "0"},     {"--detect-records", "33"}, {"--lookahead-records", "0"},
        {"--lookahead-records", "33"}, {"--max-sort-group", "0"},  {"--max-sort-group", "1048577"},
        {"--exclude-rts", "32"},       {"--include-rts", "-1"},    {"--exclude-subaddresses", "99"},
    };
    for (std::size_t i = 0; i < sizeof(out_of_range) / sizeof(out_of_range[0]); ++i) {
        INFO(out_of_range[i][0] << " " << out_of_range[i][1]);
        REQUIRE(mie::cli::run(args("decode", "in.mie", out_of_range[i][0], out_of_range[i][1])) ==
                mie::cli::EXIT_USAGE);
    }

    SECTION("the boundaries themselves are accepted") {
        const TempFile input("mie-cli-bounds.mie", valid_recording());
        static const char* const ok[][2] = {
            {"--detect-records", "1"},    {"--detect-records", "32"},
            {"--lookahead-records", "1"}, {"--lookahead-records", "32"},
            {"--max-sort-group", "1"},    {"--max-sort-group", "1048576"},
        };
        for (std::size_t i = 0; i < sizeof(ok) / sizeof(ok[0]); ++i) {
            const TempPath out("mie-cli-bound.csv");
            INFO(ok[i][0] << " " << ok[i][1]);
            REQUIRE(mie::cli::run(args("decode", input.str(), "-o", out.str(), ok[i][0],
                                       ok[i][1])) == mie::cli::EXIT_OK);
        }
    }
}

TEST_CASE("--standard-tick-rate-hz must be a positive number", "[cli][L3-CPP-014]") {
    // A zero or negative rate would divide by zero or run the clock backwards;
    // NaN compares false against every bound, so it has to be rejected by the
    // positive test rather than by a range check.
    const char* bad[] = {"0", "-1", "-0.5", "abc", "", "nan", "1.0.0"};
    for (std::size_t i = 0; i < sizeof(bad) / sizeof(bad[0]); ++i) {
        INFO(bad[i]);
        REQUIRE(mie::cli::run(args("decode", "in.mie", "--standard-tick-rate-hz", bad[i])) ==
                mie::cli::EXIT_USAGE);
    }
}

TEST_CASE("--mux-delimiter rejects an empty value", "[cli][L3-CPP-014]") {
    // Splitting on "" has no meaningful answer; accepting it would produce a
    // MUX column derived from nothing.
    REQUIRE(mie::cli::run(args("decode", "in.mie", "--mux-delimiter=")) == mie::cli::EXIT_USAGE);
}

TEST_CASE("--mux-field accepts a negative index", "[cli][L3-CPP-014]") {
    // Unlike the other integer flags this one is deliberately unbounded below:
    // a negative index counts back from the end of the filename.
    const TempFile input("mie-cli-mux.mie", valid_recording());
    const TempPath out("mie-cli-mux.csv");
    REQUIRE(mie::cli::run(args("decode", input.str(), "-o", out.str(), "--mux-field", "-1")) ==
            mie::cli::EXIT_OK);
}

TEST_CASE("a bad filter value is a usage error, not a config error", "[cli][L3-CPP-014]") {
    // Regression pin. `parse_type_name` and `parse_bus_name` are shared with
    // the config loader and signal with ConfigError; reached through a CLI flag
    // that must become exit 4, not exit 5, and above all not an uncaught
    // exception -- which is what it was, aborting the process.
    REQUIRE(mie::cli::run(args("decode", "in.mie", "--exclude-types", "0x100")) ==
            mie::cli::EXIT_USAGE);
    REQUIRE(mie::cli::run(args("decode", "in.mie", "--include-types", "NOT_A_TYPE")) ==
            mie::cli::EXIT_USAGE);
    REQUIRE(mie::cli::run(args("decode", "in.mie", "--exclude-buses", "C")) ==
            mie::cli::EXIT_USAGE);
    REQUIRE(mie::cli::run(args("decode", "in.mie", "--include-buses", "")) == mie::cli::EXIT_USAGE);
}

TEST_CASE("filter lists tolerate whitespace and a trailing comma", "[cli][L3-CPP-014]") {
    const TempFile input("mie-cli-list.mie", valid_recording());
    const TempPath tidy("mie-cli-tidy.csv");
    const TempPath messy("mie-cli-messy.csv");

    REQUIRE(mie::cli::run(args("decode", input.str(), "-o", tidy.str(), "--exclude-rts",
                               "1,2,3")) == mie::cli::EXIT_OK);
    REQUIRE(mie::cli::run(args("decode", input.str(), "-o", messy.str(), "--exclude-rts",
                               " 1 , 2 ,3 ,")) == mie::cli::EXIT_OK);

    std::string a;
    std::string b;
    REQUIRE(mie_test::read_file(tidy.str(), a));
    REQUIRE(mie_test::read_file(messy.str(), b));
    REQUIRE(a == b);
}

TEST_CASE("an invalid --log-level is refused rather than ignored", "[cli][L3-CPP-014]") {
    // Silently falling back to the default would leave an operator who asked
    // for DEBUG staring at a quiet run and concluding nothing happened.
    REQUIRE(mie::cli::run(args("--log-level", "LOUD", "count", "in.mie")) == mie::cli::EXIT_USAGE);

    const char* accepted[] = {"DEBUG", "INFO", "WARNING", "WARN", "ERROR", "CRITICAL", "OFF"};
    const TempFile input("mie-cli-level.mie", valid_recording());
    for (std::size_t i = 0; i < sizeof(accepted) / sizeof(accepted[0]); ++i) {
        std::string out;
        INFO(accepted[i]);
        REQUIRE(run_capturing_stdout(args("--log-level", accepted[i], "count", input.str()), out) ==
                mie::cli::EXIT_OK);
    }
    mie::log::set_level(mie::log::LEVEL_OFF);
}

TEST_CASE("unported flags say what is missing, not that the flag is unknown", "[cli][L3-CPP-015]") {
    // An operator porting a working Rust invocation should learn that the merge
    // is not built yet, rather than doubting that they spelled the flag right.
    const char* deferred[] = {"--manifest", "--glob", "--delta-scope", "--collapse-duplicates",
                              "--collapse-window-us"};
    for (std::size_t i = 0; i < sizeof(deferred) / sizeof(deferred[0]); ++i) {
        INFO(deferred[i]);
        REQUIRE(mie::cli::run(args("decode", "in.mie", deferred[i])) == mie::cli::EXIT_USAGE);
        // The `--flag=value` spelling must be caught too, or half of them slip
        // through and are reported as an unknown option.
        REQUIRE(mie::cli::run(args("decode", "in.mie", std::string(deferred[i]) + "=x")) ==
                mie::cli::EXIT_USAGE);
    }

    SECTION("dump and multi-input decode say the same") {
        REQUIRE(mie::cli::run(args("dump", "in.mie")) == mie::cli::EXIT_USAGE);
        REQUIRE(mie::cli::run(args("decode", "a.mie", "b.mie")) == mie::cli::EXIT_USAGE);
        REQUIRE(mie::cli::run(args("count", "a.mie", "b.mie")) == mie::cli::EXIT_USAGE);
    }
}

TEST_CASE("exit codes classify the failure", "[cli][L3-CPP-016]") {
    SECTION("a missing input is a runtime error") {
        const TempPath absent("mie-cli-absent.mie");
        REQUIRE(mie::cli::run(args("decode", absent.str(), "-o", "out.csv")) ==
                mie::cli::EXIT_RUNTIME);
        // count agrees with decode: a script's "is this readable?" check must
        // not depend on which subcommand it happened to use.
        REQUIRE(mie::cli::run(args("count", absent.str())) == mie::cli::EXIT_RUNTIME);
    }

    SECTION("a file that is not a recording is exit 2 on both subcommands") {
        std::vector<uint8_t> junk;
        junk.reserve(512);
        for (std::size_t i = 0; i < 512; ++i) {
            junk.push_back(static_cast<uint8_t>(0xA5));
        }
        const TempFile input("mie-cli-junk.mie", junk);
        const TempPath out("mie-cli-junk.csv");
        REQUIRE(mie::cli::run(args("decode", input.str(), "-o", out.str())) ==
                mie::cli::EXIT_NO_RECORDS);
        REQUIRE(mie::cli::run(args("count", input.str())) == mie::cli::EXIT_NO_RECORDS);
    }

    SECTION("a missing config file is exit 5") {
        const TempPath absent("mie-cli-absent.toml");
        REQUIRE(mie::cli::run(args("--config", absent.str(), "count", "in.mie")) ==
                mie::cli::EXIT_CONFIG);
    }

    SECTION("malformed config is exit 5") {
        const TempFile config("mie-cli-bad.toml", std::string("[decode\nstrict = true\n"));
        REQUIRE(mie::cli::run(args("--config", config.str(), "count", "in.mie")) ==
                mie::cli::EXIT_CONFIG);
    }

    SECTION("an unsupported output format is a runtime error") {
        const TempFile input("mie-cli-fmt.mie", valid_recording());
        const TempPath out("mie-cli-fmt.csv");
        REQUIRE(mie::cli::run(args("decode", input.str(), "-o", out.str(), "--format",
                                   "parquet")) == mie::cli::EXIT_RUNTIME);
    }
}

TEST_CASE("count writes the integer alone to stdout", "[cli][L3-CPP-017]") {
    // `n=$(mie-decoder count x.mie)` must not need to strip prose, so the
    // human-readable sentence goes to stderr and stdout carries the number and
    // a newline -- nothing else.
    const TempFile input("mie-cli-count.mie", valid_recording());
    std::string out;
    REQUIRE(run_capturing_stdout(args("count", input.str()), out) == mie::cli::EXIT_OK);
    REQUIRE(out == "2\n");
}

TEST_CASE("config supplies defaults that the CLI overrides", "[cli][L3-CPP-018]") {
    const TempFile input("mie-cli-prec.mie", valid_recording());

    SECTION("a presence-only flag left off does not clobber the config value") {
        // The trap this pins: contributing `false` for an absent flag would make
        // `--no-clobber`'s absence mean "allow overwriting", silently undoing a
        // safety setting the operator had put in their config file.
        const TempFile config("mie-cli-clobber.toml", std::string("[output]\nno_clobber = true\n"));
        const TempPath out("mie-cli-clobber.csv");

        REQUIRE(mie::cli::run(args("--config", config.str(), "decode", input.str(), "-o",
                                   out.str())) == mie::cli::EXIT_OK);
        // Second run over the same destination: no_clobber from the config must
        // still be in force even though no flag was passed either time.
        REQUIRE(mie::cli::run(args("--config", config.str(), "decode", input.str(), "-o",
                                   out.str())) != mie::cli::EXIT_OK);
    }

    SECTION("a config value is used when no flag contradicts it") {
        const TempFile config(
            "mie-cli-detect.toml",
            std::string("[decode]\ndetect_records = 4\ntime_format = \"irig\"\n"));
        const TempPath out("mie-cli-detect.csv");
        REQUIRE(mie::cli::run(args("--config", config.str(), "decode", input.str(), "-o",
                                   out.str())) == mie::cli::EXIT_OK);
    }

    SECTION("an invalid config value is exit 5, and the CLI equivalent is exit 4") {
        // Same bad number, two sources, two different exit codes -- which is
        // the distinction that tells an operator where to go and fix it.
        const TempFile config("mie-cli-range.toml", std::string("[decode]\ndetect_records = 99\n"));
        REQUIRE(mie::cli::run(args("--config", config.str(), "count", input.str())) ==
                mie::cli::EXIT_CONFIG);
        REQUIRE(mie::cli::run(args("decode", input.str(), "--detect-records", "99")) ==
                mie::cli::EXIT_USAGE);
    }
}

TEST_CASE("--separate-errors on stdout forces inline, and says so", "[cli][L3-CPP-017]") {
    // You cannot split stdout, so the flag has to be ignored there. Ignoring it
    // SILENTLY is the problem: an operator who asked for a separate errors file
    // and got one combined stream would only discover it by reading the output.
    const TempFile input("mie-cli-split-stdout.mie", valid_recording());
    const mie_test::LogCapture capture(mie::log::LEVEL_WARN);
    std::string out;
    std::string err;
    REQUIRE(run_capturing(args("decode", input.str(), "-o", "-", "--separate-errors"), out, err) ==
            mie::cli::EXIT_OK);
    REQUIRE(out.find("TIME_STAMP") == 0);

    bool warned = false;
    const std::vector<std::string>& lines = mie_test::captured_lines();
    for (std::size_t i = 0; i < lines.size(); ++i) {
        if (lines[i].find("forces inline error mode") != std::string::npos) {
            warned = true;
        }
    }
    REQUIRE(warned);
}

TEST_CASE("-o - selects stdout", "[cli][L3-CPP-017]") {
    const TempFile input("mie-cli-stdout-dest.mie", valid_recording());
    std::string out;
    REQUIRE(run_capturing_stdout(args("decode", input.str(), "-o", "-"), out) == mie::cli::EXIT_OK);
    REQUIRE(out.find("TIME_STAMP") == 0);
    // A single trailing newline, not a CRLF pair, on every host (L2-WRT-012).
    REQUIRE(out.find("\r\n") == std::string::npos);
}
