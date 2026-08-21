// SPDX-License-Identifier: Apache-2.0
//
// Merge tests: input resolution, the k-way order, and the rejections.
//
// The shared conformance oracles already prove the merged CSV is byte-correct.
// What they cannot reach is everything around it: a glob that matches nothing,
// a manifest full of comments, an input set over the cap, two recorders whose
// clocks disagree, a file that steps backward in time. Those are the conditions
// an operator running a batch actually hits, and none of them produces a CSV to
// compare against.

#include "mie/merge.hpp"

#include <catch2/catch.hpp>

#include <cstdio>
#include <string>
#include <vector>

#include "log_capture.hpp"
#include "mie/error.hpp"
#include "mie/reader.hpp"
#include "record_fixtures.hpp"
#include "temp_path.hpp"

namespace {

using mie::merge::glob_match;
using mie_test::TempFile;
using mie_test::TempPath;

/// A recording of `count` BC-to-RT records, one every 100 us from `start_us`.
std::vector<uint8_t> recording(uint32_t start_us, uint32_t count, uint8_t rt = 15) {
    std::vector<uint16_t> words;
    for (uint32_t i = 0; i < count; ++i) {
        mie_test::append(words, mie_test::bc_to_rt(rt, 11, 2, start_us + i * 100));
    }
    return mie_test::finish(words);
}

mie::ReaderOptions irig_options() {
    mie::ReaderOptions options;
    // Pinned rather than auto-detected: these fixtures are small, and an
    // ambiguous probe would make the test about detection instead of merging.
    options.time_format = mie::TIMESTAMP_IRIG;
    return options;
}

/// Drain a merge into the timestamps it produced, in order.
std::vector<uint64_t> merged_order(const std::vector<mie::MieFileReader*>& readers,
                                   const mie::merge::MergeOptions& options) {
    mie::merge::MergedSource source(readers, options);
    std::vector<uint64_t> out;
    mie::MieMessage message;
    while (source.next(message)) {
        out.push_back(message.timestamp.irig.to_total_microseconds());
    }
    return out;
}

}  // namespace

TEST_CASE("glob_match implements exactly the documented wildcards", "[merge][L3-CPP-019]") {
    SECTION("literals match whole strings only") {
        CHECK(glob_match("a.mie", "a.mie"));
        CHECK_FALSE(glob_match("a.mie", "xa.mie"));
        CHECK_FALSE(glob_match("a.mie", "a.mie.bak"));
        CHECK(glob_match("", ""));
        CHECK_FALSE(glob_match("", "a"));
    }

    SECTION("* matches any run, including an empty one") {
        CHECK(glob_match("*", ""));
        CHECK(glob_match("*", "anything"));
        CHECK(glob_match("*.mie", "a.mie"));
        CHECK(glob_match("*.mie", ".mie"));
        CHECK(glob_match("a*", "a"));
        CHECK(glob_match("a*c", "ac"));
        CHECK(glob_match("a*c", "abbbc"));
        CHECK_FALSE(glob_match("a*c", "abbb"));
    }

    SECTION("several stars backtrack correctly") {
        // The case a naive left-to-right matcher gets wrong: the first `*` must
        // give characters back so the later literals can line up.
        CHECK(glob_match("*a*b*", "xxayybzz"));
        CHECK(glob_match("*a*b", "ab"));
        CHECK_FALSE(glob_match("*a*b", "ba"));
        CHECK(glob_match("**", "abc"));
        CHECK(glob_match("*flight*.mie", "2026-flight-07.mie"));
    }

    SECTION("? matches exactly one character, never zero") {
        CHECK(glob_match("?", "a"));
        CHECK_FALSE(glob_match("?", ""));
        CHECK_FALSE(glob_match("?", "ab"));
        CHECK(glob_match("a?c", "abc"));
        CHECK_FALSE(glob_match("a?c", "ac"));
    }

    SECTION("? consumes a whole UTF-8 character, not one byte") {
        // The cross-implementation trap. Rust and Python match over Unicode
        // scalar values, so `a?c` matches "aXc" where X is one non-ASCII
        // character. A byte-wise matcher would need TWO `?` for a two-byte
        // character and would silently disagree on any non-ASCII filename.
        const std::string two_byte = "a\xC2\xA7";  // U+00A7 SECTION SIGN
        CHECK(glob_match("a?", two_byte));
        CHECK_FALSE(glob_match("a??", two_byte));

        const std::string four_byte = "\xF0\x9F\x9B\xA9";  // U+1F6E9, one character
        CHECK(glob_match("?", four_byte));
        CHECK(glob_match("*", four_byte));
    }

    SECTION("no other character is special") {
        CHECK(glob_match("a.b", "a.b"));
        CHECK_FALSE(glob_match("a.b", "axb"));  // '.' is a literal, not "any"
        CHECK(glob_match("[abc]", "[abc]"));    // no character classes
        CHECK(glob_match("{a,b}", "{a,b}"));    // no brace expansion
    }
}

TEST_CASE("read_manifest keeps order and ignores comments", "[merge][L3-CPP-019]") {
    SECTION("blank lines, comments and surrounding blanks are dropped") {
        const TempFile manifest("mie-manifest.txt",
                                std::string("# a comment\n"
                                            "\n"
                                            "   \n"
                                            "  first.mie  \n"
                                            "second.mie\n"
                                            "   # an indented comment\n"
                                            "third.mie"));  // no trailing newline
        std::vector<std::string> paths;
        mie::platform::OsError err;
        REQUIRE(mie::merge::read_manifest(manifest.str(), paths, err));
        REQUIRE(paths.size() == 3u);
        // Order is the FILE's order and is never sorted: a manifest is how an
        // operator states an order explicitly, so sorting it would discard the
        // one thing it is for.
        CHECK(paths[0] == "first.mie");
        CHECK(paths[1] == "second.mie");
        CHECK(paths[2] == "third.mie");
    }

    SECTION("CRLF line endings are handled") {
        // A manifest is a text file an operator may well have written on
        // Windows, and a trailing CR would become part of the filename.
        const TempFile manifest("mie-manifest-crlf.txt", std::string("a.mie\r\nb.mie\r\n"));
        std::vector<std::string> paths;
        mie::platform::OsError err;
        REQUIRE(mie::merge::read_manifest(manifest.str(), paths, err));
        REQUIRE(paths.size() == 2u);
        CHECK(paths[0] == "a.mie");
        CHECK(paths[1] == "b.mie");
    }

    SECTION("an empty manifest is not an error, just empty") {
        const TempFile manifest("mie-manifest-empty.txt", std::string("# only a comment\n"));
        std::vector<std::string> paths;
        mie::platform::OsError err;
        REQUIRE(mie::merge::read_manifest(manifest.str(), paths, err));
        CHECK(paths.empty());
    }

    SECTION("a missing manifest reports the I/O failure") {
        const TempPath absent("mie-manifest-absent.txt");
        std::vector<std::string> paths;
        mie::platform::OsError err;
        CHECK_FALSE(mie::merge::read_manifest(absent.str(), paths, err));
        CHECK_FALSE(err.ok());
    }
}

TEST_CASE("expand_glob returns matching files in a deterministic order", "[merge][L3-CPP-019]") {
    // Files are created straight in the temp root under one unique prefix,
    // rather than in a directory of their own. Making a directory is not one of
    // the five concerns the platform layer owns, and growing that layer to a
    // sixth for the benefit of a test would weaken the confinement argument
    // `scripts/assert-platform-confined.sh` exists to enforce.
    TempPath anchor("glob-anchor");
    const std::string base = anchor.str();
    mie::platform::OsError err;

    // Created out of alphabetical order on purpose: the result must not depend
    // on the order the filesystem enumerates them in, or two hosts would merge
    // the same directory differently.
    const char* leaves[] = {"-c.mie", "-a.mie", "-b.mie", "-notes.txt"};
    for (std::size_t i = 0; i < sizeof(leaves) / sizeof(leaves[0]); ++i) {
        const std::string path = anchor.also_remove(base + leaves[i]);
        std::FILE* handle = std::fopen(path.c_str(), "wb");
        REQUIRE(handle != NULL);
        REQUIRE(std::fputs("x", handle) >= 0);
        REQUIRE(std::fclose(handle) == 0);
    }

    SECTION("wildcards apply to the filename and results are sorted") {
        std::vector<std::string> found;
        REQUIRE(mie::merge::expand_glob(base + "-*.mie", found, err));
        REQUIRE(found.size() == 3u);
        CHECK(found[0] == base + "-a.mie");
        CHECK(found[1] == base + "-b.mie");
        CHECK(found[2] == base + "-c.mie");
    }

    SECTION("the separator the pattern used is the one that comes back") {
        // Regression pin. The prefix was rebuilt with a hardcoded '/', so a
        // Windows pattern returned `C:\dir/file.mie`. Windows OPENS that
        // happily, so every functional check passed and only the paths the tool
        // reported back -- in log lines and error messages -- were mangled.
        std::vector<std::string> found;
        REQUIRE(mie::merge::expand_glob(base + "-*.mie", found, err));
        REQUIRE_FALSE(found.empty());
        for (std::size_t i = 0; i < found.size(); ++i) {
            INFO(found[i]);
            // The result starts with the pattern's own directory text, byte for
            // byte, separator included.
            CHECK(found[i].compare(0, base.size(), base) == 0);
        }
    }

    SECTION("a glob matching nothing is empty, not a failure") {
        // The distinction matters: the CLI turns "matched nothing" into a usage
        // error naming the pattern, which it could not do if this reported I/O.
        std::vector<std::string> found;
        REQUIRE(mie::merge::expand_glob(base + "-*.nope", found, err));
        CHECK(found.empty());
        CHECK(err.ok());
    }

    SECTION("a missing directory is a failure") {
        std::vector<std::string> found;
        CHECK_FALSE(mie::merge::expand_glob(base + "-no-such-dir/*.mie", found, err));
    }
}

TEST_CASE("the merge interleaves inputs by timestamp", "[merge][L3-CPP-020]") {
    const TempFile a("mie-merge-a.mie", recording(100, 3));
    const TempFile b("mie-merge-b.mie", recording(150, 3));

    mie::MieFileReader reader_a;
    mie::MieFileReader reader_b;
    reader_a.open(a.str(), irig_options());
    reader_b.open(b.str(), irig_options());

    std::vector<mie::MieFileReader*> readers;
    readers.push_back(&reader_a);
    readers.push_back(&reader_b);

    const mie::merge::MergeOptions options;
    const std::vector<uint64_t> order = merged_order(readers, options);
    REQUIRE(order.size() == 6u);
    // Strictly non-decreasing, and actually interleaved rather than
    // concatenated -- a merge that returned all of A then all of B would also
    // be "sorted" if A happened to end before B began, so the fixtures overlap.
    for (std::size_t i = 1; i < order.size(); ++i) {
        CHECK(order[i - 1] <= order[i]);
    }
    const uint64_t base = order[0];
    CHECK(order[1] - base == 50u);
    CHECK(order[2] - base == 100u);
    CHECK(order[3] - base == 150u);
}

TEST_CASE("a merge rejects inputs that cannot share a timeline", "[merge][L3-CPP-021]") {
    // L2-MRG-003. Each of these resolves to a number that means something
    // different per file, so interleaving on it would be meaningless -- and
    // silently so, which is why it is a rejection rather than a warning.
    const TempFile good("mie-merge-good.mie", recording(100, 3));

    SECTION("a freerun-leading IRIG input") {
        std::vector<uint16_t> words;
        for (uint32_t i = 0; i < 3; ++i) {
            std::vector<uint16_t> record;
            record.push_back(mie_test::type_word(mie::MESSAGE_TYPE_BC_TO_RT, 8));
            mie_test::push_irig(record, 100 + i * 100, 10, 15, 54, 50, /*freerun=*/true);
            record.push_back(mie_test::command_word(15, mie::DIRECTION_RECEIVE, 11, 2));
            record.push_back(static_cast<uint16_t>(0x1100 + i));
            record.push_back(static_cast<uint16_t>(0x2200 + i));
            record.push_back(mie_test::status_word(15));
            mie_test::append(words, record);
        }
        const TempFile freerun("mie-merge-freerun.mie", mie_test::finish(words));

        mie::MieFileReader reader_a;
        mie::MieFileReader reader_b;
        reader_a.open(good.str(), irig_options());
        reader_b.open(freerun.str(), irig_options());
        std::vector<mie::MieFileReader*> readers;
        readers.push_back(&reader_a);
        readers.push_back(&reader_b);

        const mie::merge::MergeOptions options;
        try {
            // Constructing it IS the operation under test: the merge primes
            // itself here, which is where an input that cannot share a timeline
            // is rejected -- before a single row exists.
            const mie::merge::MergedSource source(readers, options);
            FAIL("expected an incompatible-merge rejection");
        } catch (const mie::MieError& error) {
            CHECK(error.kind() == mie::KIND_INCOMPATIBLE_MERGE_INPUTS);
            // The message must name the input, because an operator merging
            // forty files needs to know WHICH one to remove.
            CHECK(error.message().find("freerun") != std::string::npos);
        }
    }

    SECTION("a Standard-format input") {
        std::vector<uint16_t> words;
        for (uint32_t i = 0; i < 3; ++i) {
            std::vector<uint16_t> record;
            record.push_back(mie_test::type_word(mie::MESSAGE_TYPE_BC_TO_RT, 7));
            mie_test::push_standard(record, 1000 + i * 100);
            record.push_back(mie_test::command_word(15, mie::DIRECTION_RECEIVE, 11, 2));
            record.push_back(static_cast<uint16_t>(0x3300 + i));
            record.push_back(static_cast<uint16_t>(0x4400 + i));
            record.push_back(mie_test::status_word(15));
            mie_test::append(words, record);
        }
        const TempFile standard("mie-merge-standard.mie", mie_test::finish(words));

        mie::ReaderOptions standard_options;
        standard_options.time_format = mie::TIMESTAMP_STANDARD;

        mie::MieFileReader reader_a;
        mie::MieFileReader reader_b;
        reader_a.open(good.str(), irig_options());
        reader_b.open(standard.str(), standard_options);
        std::vector<mie::MieFileReader*> readers;
        readers.push_back(&reader_a);
        readers.push_back(&reader_b);

        const mie::merge::MergeOptions options;
        try {
            // Constructing it IS the operation under test: the merge primes
            // itself here, which is where an input that cannot share a timeline
            // is rejected -- before a single row exists.
            const mie::merge::MergedSource source(readers, options);
            FAIL("expected an incompatible-merge rejection");
        } catch (const mie::MieError& error) {
            CHECK(error.kind() == mie::KIND_INCOMPATIBLE_MERGE_INPUTS);
        }
    }
}

TEST_CASE("duplicate collapsing spans recorders but never one file", "[merge][L3-CPP-022]") {
    // L2-MRG-007. Two recorders on one bus see the same transaction; the same
    // content twice within ONE file is the bus really carrying it twice, and
    // dropping that would be losing data rather than de-duplicating it.
    mie::merge::DedupWindow window(10);

    mie::MieMessage message;
    message.type_word = mie::TypeWord(mie::MESSAGE_TYPE_BC_TO_RT, mie::BUS_A, 8, false, 0x0802);
    message.command_word = mie::CommandWord(15, mie::DIRECTION_RECEIVE, 11, 2, 0x7962);
    message.status_word = static_cast<uint16_t>(0x7800);
    const uint16_t payload[2] = {0x1234, 0xABCD};
    message.data_words = mie::DataWords::from_words(payload, 2);

    SECTION("the same content from another input within the window collapses") {
        CHECK_FALSE(window.is_duplicate(1000, 0, message));
        CHECK(window.is_duplicate(1005, 1, message));
    }

    SECTION("the same content from the SAME input never collapses") {
        mie::merge::DedupWindow same_file(10);
        CHECK_FALSE(same_file.is_duplicate(1000, 0, message));
        CHECK_FALSE(same_file.is_duplicate(1005, 0, message));
    }

    SECTION("outside the window it is a distinct record") {
        mie::merge::DedupWindow narrow(10);
        CHECK_FALSE(narrow.is_duplicate(1000, 0, message));
        CHECK_FALSE(narrow.is_duplicate(1011, 1, message));
    }

    SECTION("different payload is never a duplicate") {
        mie::merge::DedupWindow other(10);
        CHECK_FALSE(other.is_duplicate(1000, 0, message));
        mie::MieMessage changed(message);
        const uint16_t different[2] = {0x1234, 0x0000};
        changed.data_words = mie::DataWords::from_words(different, 2);
        CHECK_FALSE(other.is_duplicate(1002, 1, changed));
    }

    SECTION("a backward step neither collapses wrongly nor underflows") {
        // A lenient non-monotonic input (L2-MRG-006) can step backward. The
        // window compares ABSOLUTE distance: a one-sided subtraction would wrap
        // to an enormous unsigned value and read as "outside the window" only
        // by luck.
        mie::merge::DedupWindow backward(10);
        CHECK_FALSE(backward.is_duplicate(1000, 0, message));
        CHECK(backward.is_duplicate(995, 1, message));
    }
}

TEST_CASE("a non-monotonic input warns once and keeps going", "[merge][L3-CPP-023]") {
    // L2-MRG-006: inputs are assumed internally time-sorted. A backward step
    // means the merged output may be out of order for that input, which the
    // operator has to be told -- once. Repeating it per record would bury the
    // message in its own output.
    std::vector<uint16_t> words;
    mie_test::append(words, mie_test::bc_to_rt(15, 11, 2, 500));
    mie_test::append(words, mie_test::bc_to_rt(15, 11, 2, 600));
    mie_test::append(words, mie_test::bc_to_rt(15, 11, 2, 200));  // steps backward
    mie_test::append(words, mie_test::bc_to_rt(15, 11, 2, 300));
    mie_test::append(words, mie_test::bc_to_rt(15, 11, 2, 100));  // and again
    const TempFile backward("mie-merge-backward.mie", mie_test::finish(words));
    const TempFile forward("mie-merge-forward.mie", recording(1000, 2));

    mie::MieFileReader reader_a;
    mie::MieFileReader reader_b;
    reader_a.open(backward.str(), irig_options());
    reader_b.open(forward.str(), irig_options());
    std::vector<mie::MieFileReader*> readers;
    readers.push_back(&reader_a);
    readers.push_back(&reader_b);

    SECTION("lenient mode warns exactly once for that input") {
        const mie_test::LogCapture capture(mie::log::LEVEL_WARN);
        mie::merge::MergeOptions options;
        options.strict = false;
        const std::vector<uint64_t> order = merged_order(readers, options);
        CHECK(order.size() == 7u);

        std::size_t warnings = 0;
        const std::vector<std::string>& lines = mie_test::captured_lines();
        for (std::size_t i = 0; i < lines.size(); ++i) {
            if (lines[i].find("not internally time-sorted") != std::string::npos) {
                warnings += 1;
            }
        }
        CHECK(warnings == 1u);
    }

    SECTION("strict mode fails the batch") {
        mie::merge::MergeOptions options;
        options.strict = true;
        mie::merge::MergedSource source(readers, options);
        mie::MieMessage message;
        bool raised = false;
        try {
            while (source.next(message)) {
                // drain
            }
        } catch (const mie::MieError& error) {
            raised = true;
            CHECK(error.kind() == mie::KIND_NON_MONOTONIC_INPUT);
        }
        CHECK(raised);
    }
}

TEST_CASE("an empty input contributes nothing rather than failing", "[merge][L3-CPP-020]") {
    // A valid but empty recording is not a broken one (L1-EXIT-010): the stream
    // opens on the end-of-records terminator. In a merge it simply has no
    // records to offer.
    const std::vector<uint16_t> nothing;
    const TempFile empty("mie-merge-empty.mie", mie_test::finish(nothing));
    const TempFile full("mie-merge-full.mie", recording(100, 3));

    mie::MieFileReader reader_a;
    mie::MieFileReader reader_b;
    reader_a.open(empty.str(), irig_options());
    reader_b.open(full.str(), irig_options());
    std::vector<mie::MieFileReader*> readers;
    readers.push_back(&reader_a);
    readers.push_back(&reader_b);

    const mie::merge::MergeOptions options;
    CHECK(merged_order(readers, options).size() == 3u);
}
