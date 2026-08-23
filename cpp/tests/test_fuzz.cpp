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
//      xorshift64, same seed, same size range, same little-endian fill. All
//      three implementations therefore see THE SAME INPUTS. L1-ROB-001 is a
//      shared requirement, and testing it three different ways would produce
//      three incomparable results; this way a divergence on identical bytes is
//      detectable.
//
//   3. It is deterministic. Coverage-guided exploration means a required job
//      can fail on an input nobody's change produced, which is the concern
//      was raised against a blocking timed fuzz run when this was scoped.
//
// No committed corpus, for the same reason: the generator reproduces its inputs
// exactly, so a corpus would be storage for something already derivable.
//
// The default 256 iterations run in seconds and ride along with every `make
// check`. The nightly `fuzz.yml` burn-in sets MIE_FUZZ_ITERATIONS higher; since
// the PRNG is deterministic, a burn-in is a strict SUPERSET of the default run
// -- the first 256 inputs are identical.

#include <catch2/catch.hpp>

#include <cstdlib>
#include <string>
#include <vector>

#include "mie/error.hpp"
#include "mie/order.hpp"
#include "mie/reader.hpp"
#include "mie/source.hpp"
#include "temp_path.hpp"

namespace {

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
    const uint64_t seed = 0x0DDCD1ECDDC0DEC0ULL;
    uint64_t state = seed;
    const std::size_t iterations = iterations_from_env();

    for (std::size_t i = 0; i < iterations; ++i) {
        // 32 B (just above MIN_RECORD_BYTES_STANDARD, so record headers stay
        // reachable) to ~8 KB (so each iteration stays fast).
        const std::size_t size = 32 + static_cast<std::size_t>(xorshift64(state) % 8192);
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

        INFO("seed=0x0DDCD1ECDDC0DEC0 iteration=" << i << " size=" << size);

        const mie_test::TempFile input("mie-fuzz.mie", bytes);

        try {
            mie::MieFileReader reader;
            reader.open(input.str(), mie::ReaderOptions());
            mie::RecordIter iter = reader.iter();
            IterSource source(iter);

            // The canonical-order stage is on the fuzzed path deliberately:
            // random bytes readily decode to repeated or all-zero timestamps,
            // which is exactly the equal-timestamp run its max_sort_group cap
            // (L2-WRT-022) exists to bound. A small cap is used so the
            // cap-overflow branch is reached often rather than only on a
            // pathological input.
            mie::OrderedSource ordered(source, 8);

            mie::MieMessage message;
            uint64_t yielded = 0;
            while (ordered.next(message)) {
                ++yielded;
                // Defence in depth: if the walk ever fails to terminate, this
                // surfaces it as a failed assertion rather than a hung runner.
                REQUIRE(yielded < 100000);
            }
        } catch (const mie::MieError&) {  // NOLINT(bugprone-empty-catch)
            // Empty ON PURPOSE, and the NOLINT is the machine-readable form of
            // saying so. MieError is the documented error path: opening an
            // empty or unrecognisable file and losing sync mid-stream both land
            // here, and both are the decoder behaving as specified. Swallowing
            // it is the assertion -- what this test forbids is any OTHER
            // exception type, which escapes and fails the case.
        }
        // Any OTHER exception type escapes this block and fails the test. That
        // is the point: MieError is the contract, and anything else -- a
        // std::out_of_range from an unchecked index, a std::bad_alloc from a
        // length read straight off the wire -- is a defect.
    }
}
