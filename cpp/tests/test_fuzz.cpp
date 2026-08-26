// SPDX-License-Identifier: Apache-2.0
//
// L1-ROB-001: arbitrary bytes must not crash the decoder.
//
// WHY THIS SHAPE, AND NOT libFUZZER. The delivery plan called for libFuzzer
// targets. It is the wrong tool for THIS tree, for three reasons:
//
//   1. libFuzzer needs clang. The defining constraint here is GCC 4.8.5, the
//      SLES 12 system compiler (ADR-0001). A libFuzzer target would run on one
//      of the four C++ tiers and NOT on the one this implementation exists for.
//      A Catch2 test case runs on all of them -- modern g++, GCC 4.8.5, MSVC --
//      and again under ASan/UBSan and valgrind, which is where a genuine memory
//      fault on random input actually surfaces.
//
//   2. The generator below is byte-for-byte the one in
//      `rust/tests/integration.rs` and `python/tests/test_e2e.py`: same
//      xorshift64, same seed, same size range, same little-endian fill, same
//      draw order. All three implementations therefore see THE SAME INPUTS.
//      L1-ROB-001 is a shared requirement, and testing it three different ways
//      would produce three incomparable results; this way a divergence on
//      identical bytes is detectable.
//
//   3. It is deterministic. Coverage-guided exploration means a required job
//      can fail on an input nobody's change produced, which is the concern
//      was raised against a blocking timed fuzz run when this was scoped.
//
// No committed corpus, for the same reason: the generator reproduces its inputs
// exactly, so a corpus would be storage for something already derivable.
//
// THREE KNOBS, SHARED WITH THE RUST AND PYTHON HARNESSES. All three read the
// same environment variables and mean the same thing by them, because a burn-in
// scoped differently per language cannot be compared across languages -- which
// is the subject of `docs/FUZZING.md` section 1.
//
//   MIE_FUZZ_ITERATIONS   inputs to generate (default 256)
//   MIE_FUZZ_STREAM_LOGS  `1` / `true` -> leave the decoder's logger at WARN so
//                         its diagnostics stream; anything else -> LEVEL_OFF
//   MIE_FUZZ_SUMMARY      file to append this run's FUZZ-SUMMARY line to
//
// WHY THE LOG KNOB IS THE HARNESS'S JOB AND NOT THE RUNNER'S. The logger writes
// to the stderr file descriptor, which Catch2 does not redirect -- so no runner
// flag controls it in either direction. Before this knob existed a scheduled
// burn-in wrote roughly forty megabytes of WARN lines into every CI run whether
// or not anyone had asked for them.
//
// The default 256 iterations run in seconds and ride along with every `make
// check`. `make check-fuzz` runs ONLY this file's cases, which is what the
// nightly burn-in uses so its wall time measures fuzzing rather than the whole
// suite; `make check` is deliberately left as the single unparameterised
// command a developer runs. Since the PRNG is deterministic, a burn-in is a
// strict SUPERSET of the default run -- the first 256 inputs are identical.

#include <catch2/catch.hpp>

#include <cstdio>
#include <cstdlib>
#include <string>
#include <vector>

#include "mie/error.hpp"
#include "mie/log.hpp"
#include "mie/order.hpp"
#include "mie/reader.hpp"
#include "mie/source.hpp"
#include "mie/text.hpp"
#include "temp_path.hpp"

namespace {

/// The seed every fuzz harness in every implementation starts from.
const uint64_t FUZZ_SEED = 0x0DDCD1ECDDC0DEC0ULL;

/// The same xorshift64 the Rust and Python harnesses use. Not a good PRNG; a
/// REPRODUCIBLE one, which is the property that matters here.
uint64_t xorshift64(uint64_t& state) {
    uint64_t x = state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    state = x;
    return x;
}

/// Digits only, explicit ASCII range -- `<cctype>` reads the locale table and
/// is banned tree-wide (scripts/assert-locale-free.sh).
std::size_t iterations_from_env() {
    const char* raw = std::getenv("MIE_FUZZ_ITERATIONS");
    if (raw == 0) {
        return 256;
    }
    std::size_t value = 0;
    for (const char* p = raw; *p != '\0'; ++p) {
        if (*p < '0' || *p > '9') {
            return 256;  // unparseable: fall back rather than guess
        }
        value = value * 10 + static_cast<std::size_t>(*p - '0');
        if (value > 10000000) {
            return 10000000;
        }
    }
    return value == 0 ? 256 : value;
}

/// True when MIE_FUZZ_STREAM_LOGS asks for the decoder's diagnostics.
bool stream_logs_from_env() {
    const char* raw = std::getenv("MIE_FUZZ_STREAM_LOGS");
    if (raw == 0) {
        return false;
    }
    const std::string value(raw);
    return value == "1" || value == "true" || value == "TRUE";
}

/// Draw `size` bytes: eight at a time little-endian, then the tail one at a
/// time.
///
/// The draw ORDER is as much a part of the contract as the PRNG -- Rust and
/// Python consume the stream identically, which is what makes iteration N the
/// same bytes in all three implementations.
std::vector<uint8_t> fill_bytes(uint64_t& state, std::size_t size) {
    std::vector<uint8_t> bytes(size, 0);
    std::size_t j = 0;
    while (j + 8 <= size) {
        const uint64_t r = xorshift64(state);
        for (std::size_t k = 0; k < 8; ++k) {
            bytes[j + k] = static_cast<uint8_t>((r >> (8 * k)) & 0xFF);
        }
        j += 8;
    }
    while (j < size) {
        bytes[j] = static_cast<uint8_t>(xorshift64(state) & 0xFF);
        ++j;
    }
    return bytes;
}

/// Emit this harness's one-line summary.
///
/// The line has the same shape in all three implementations, so the burn-in
/// jobs produce artifacts that can be DIFFED rather than three differently
/// shaped pass messages ("1 passed" vs "2 passed" vs "484 test cases"). On
/// identical inputs the counters should be identical too; a difference is a
/// cross-implementation finding, and comparing them is the cheap precursor to a
/// real differential driver (docs/FUZZING.md 6.1).
///
/// Every field must be path-independent: the three harnesses name their temp
/// files differently, so anything derived from a path measures the harness
/// rather than the decoder.
///
/// Numbers go through `text::decimal` rather than `std::to_string`, which
/// routes through vsnprintf and would take its digits from the locale.
///
/// Built by successive `+=` rather than one concatenation chain: a six-term
/// chain is exactly the shape clang-format versions wrap differently, and the
/// local, WSL and CI toolchains here are three different versions.
void emit_summary(const std::string& harness, std::size_t iterations, const std::string& fields) {
    std::string line = "FUZZ-SUMMARY impl=cpp";
    line += " harness=" + harness;
    line += " seed=0x" + mie::text::hex_upper(FUZZ_SEED, 16);
    line += " iterations=" + mie::text::decimal(static_cast<uint64_t>(iterations));
    line += " " + fields;
    static_cast<void>(std::fputs(line.c_str(), stderr));
    static_cast<void>(std::fputc('\n', stderr));

    const char* target = std::getenv("MIE_FUZZ_SUMMARY");
    if (target == 0) {
        return;
    }
    // Binary append: the artifact is compared byte for byte against the Rust
    // and Python ones, and the Windows CRT would otherwise rewrite the newline.
    std::FILE* handle = std::fopen(target, "ab");
    if (handle == 0) {
        return;  // best effort -- a summary that cannot be written is not a failure
    }
    static_cast<void>(std::fwrite(line.data(), 1, line.size(), handle));
    static_cast<void>(std::fputc('\n', handle));
    static_cast<void>(std::fclose(handle));
}

/// Sets the logger level for a scope and restores it on the way out.
///
/// RAII rather than a call pair, for the reason `log_capture.hpp` gives about
/// its sink: Catch2 runs every case in ONE process, so a case that leaves the
/// global level changed breaks the cases after it, and a `FAIL` unwinding past
/// a restore call would leak it. That is not hypothetical -- silencing the
/// logger here without restoring made `test_log.cpp`'s "the default level is
/// WARN" fail with `4 == 2`, pointing at a file this harness never touches.
class ScopedLogLevel {
  public:
    explicit ScopedLogLevel(mie::log::Level level) : previous_(mie::log::current_level()) {
        mie::log::set_level(level);
    }

    ~ScopedLogLevel() { mie::log::set_level(previous_); }

  private:
    mie::log::Level previous_;
};

/// Adapts a RecordIter to the pipeline's MessageSource contract.
///
/// The CLI has its own copy of this; making the reader implement MessageSource
/// would give it a vtable it does not otherwise need.
class IterSource : public mie::MessageSource {
  public:
    explicit IterSource(mie::RecordIter& iter) : iter_(&iter) {}
    bool next(mie::MieMessage& out) { return iter_->next(out); }

  private:
    mie::RecordIter* iter_;
};

}  // namespace

TEST_CASE("arbitrary bytes never crash the decoder", "[fuzz][robustness][L1-ROB-001]") {
    const ScopedLogLevel log_level(stream_logs_from_env() ? mie::log::LEVEL_WARN
                                                          : mie::log::LEVEL_OFF);

    uint64_t state = FUZZ_SEED;
    const std::size_t iterations = iterations_from_env();
    uint64_t total_bytes = 0;
    uint64_t opened = 0;
    uint64_t records = 0;
    uint64_t iter_errors = 0;

    for (std::size_t i = 0; i < iterations; ++i) {
        // 32 B (just above MIN_RECORD_BYTES_STANDARD, so record headers stay
        // reachable) to ~8 KB (so each iteration stays fast).
        const std::size_t size = 32 + static_cast<std::size_t>(xorshift64(state) % 8192);
        const std::vector<uint8_t> bytes = fill_bytes(state, size);
        total_bytes += static_cast<uint64_t>(size);

        INFO("seed=0x0DDCD1ECDDC0DEC0 iteration=" << i << " size=" << size);

        const mie_test::TempFile input("mie-fuzz.mie", bytes);

        // Construction and iteration are caught SEPARATELY, matching the Python
        // harness and Rust's `if let Ok(reader)`. One combined try cannot tell
        // "the file was rejected" from "the walk hit a bad record", and those
        // are the two counters the summary line reports.
        mie::MieFileReader reader;
        bool open_ok = false;
        try {
            reader.open(input.str(), mie::ReaderOptions());
            open_ok = true;
        } catch (const mie::MieError&) {  // NOLINT(bugprone-empty-catch)
            // Empty ON PURPOSE, and the NOLINT is the machine-readable form of
            // saying so. Opening an empty or unrecognisable file lands here and
            // is the decoder behaving as specified. Swallowing it is the
            // assertion -- what this test forbids is any OTHER exception type,
            // which escapes and fails the case.
        }

        if (!open_ok) {
            continue;
        }
        ++opened;

        try {
            mie::RecordIter iter = reader.iter();
            IterSource source(iter);

            // The canonical-order stage is on the fuzzed path deliberately:
            // random bytes readily decode to repeated or all-zero timestamps,
            // which is exactly the equal-timestamp run its max_sort_group cap
            // (L2-WRT-022) exists to bound. A small cap is used so the
            // cap-overflow branch is reachable at all.
            mie::OrderedSource ordered(source, 8);

            mie::MieMessage message;
            uint64_t yielded = 0;
            while (ordered.next(message)) {
                ++yielded;
                ++records;
                // Defence in depth: if the walk ever fails to terminate, this
                // surfaces it as a failure rather than a hung runner.
                //
                // A plain `if` rather than REQUIRE: Catch2 decomposes and
                // counts every REQUIRE it evaluates, and this one sits in the
                // per-RECORD loop. The Rust and Python harnesses spend an
                // `assert!` and an `if` here, so a per-record Catch2 assertion
                // made the C++ harness slower than the other two at the same
                // work. FAIL still reports through Catch2 when it fires.
                if (yielded >= 100000) {
                    FAIL("iterator yielded over 100k items on a "
                         << size << "-byte input -- possible unbounded loop (iteration " << i
                         << ")");
                }
            }
        } catch (const mie::MieError&) {
            // Losing sync mid-stream is the documented error path. Counted, not
            // ignored: it is one of the numbers the three implementations are
            // expected to agree on.
            ++iter_errors;
        }
        // Any OTHER exception type escapes this block and fails the test. That
        // is the point: MieError is the contract, and anything else -- a
        // std::out_of_range from an unchecked index, a std::bad_alloc from a
        // length read straight off the wire -- is a defect.
    }

    std::string fields = "bytes=" + mie::text::decimal(total_bytes);
    fields += " opened=" + mie::text::decimal(opened);
    fields += " open_errors=" + mie::text::decimal(static_cast<uint64_t>(iterations) - opened);
    fields += " records=" + mie::text::decimal(records);
    fields += " iter_errors=" + mie::text::decimal(iter_errors);
    fields += " outcome=ok";
    emit_summary("reader", iterations, fields);

    // The loop above deliberately spends no per-record Catch2 assertion, so
    // without this the case would report "no assertions" on a clean run.
    REQUIRE(opened <= static_cast<uint64_t>(iterations));
}
