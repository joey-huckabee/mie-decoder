// SPDX-License-Identifier: Apache-2.0
//
// EXHAUSTIVE verification of the word decoders against the Rust implementation.
//
// The per-case tests in test_decode.cpp check the values a human thought to
// check. These check ALL of them: every one of the 65 536 possible Type Words,
// every one of the 65 536 possible Command Words, and every bit position of
// every IRIG and Standard timestamp field. There is no corner case left for a
// reviewer to have missed, because no case is selected.
//
// HOW IT WORKS
//
// `rust/examples/decode_digest.rs` sweeps the same inputs through the *Rust*
// decoders and prints an FNV-1a digest of the decoded fields. The constants
// below are those digests. This file recomputes them from the C++ decoders, so
// a single differing field in any of ~260 000 decodes changes the digest and
// fails the test.
//
// Regenerate after an intentional wire-format change:
//
//     cd rust && cargo run --release --example decode_digest
//
// A changed digest means the RUST decoder changed. That is a different problem
// from the C++ one drifting, and the fix is different too -- check which side
// moved before editing anything here.
//
// WHY THE DISCRIMINATION TESTS BELOW MATTER
//
// A digest test that cannot fail is worse than no test: it reports success
// forever. Every digest here is therefore accompanied by a case that feeds a
// deliberately-wrong decoding through the same hash and asserts the result
// differs. Without those, a bug in the hash -- or a hash that ignored its input
// -- would make this whole file a very convincing no-op.

#include "mie/decode.hpp"

#include <catch2/catch.hpp>

#include <cstdint>

namespace {

namespace dec = mie::decode;

/// FNV-1a, 64-bit. Must match rust/examples/decode_digest.rs byte for byte;
/// the offset basis and prime below are the standard ones.
class Fnv1a {
  public:
    Fnv1a() : state_(0xcbf29ce484222325ull) {}

    void byte(uint8_t b) {
        state_ ^= static_cast<uint64_t>(b);
        state_ *= 0x00000100000001B3ull;
    }

    void u16(uint16_t v) {
        byte(static_cast<uint8_t>(v & 0xFF));
        byte(static_cast<uint8_t>((v >> 8) & 0xFF));
    }

    void u32(uint32_t v) {
        u16(static_cast<uint16_t>(v & 0xFFFF));
        u16(static_cast<uint16_t>((v >> 16) & 0xFFFF));
    }

    uint64_t value() const { return state_; }

  private:
    uint64_t state_;
};

// Digests emitted by `cargo run --release --example decode_digest` against the
// Rust implementation at the time this file was written.
const uint64_t kTypeWordDigest = 0xC9E9E16B4E9C7B25ull;
const uint64_t kCommandWordDigest = 0x4EF6FD5E628BF325ull;
const uint64_t kIrigDigest = 0x62BFB17B4FC79D25ull;
const uint64_t kStandardDigest = 0x094680D7977C5B25ull;

/// The sweep is a template over the decode step so the same walk can be run
/// with the real decoder and with a deliberately-broken one. That is what makes
/// the discrimination tests meaningful: they exercise the identical hashing and
/// iteration, differing only in the values fed.
template <typename FeedTypeWord>
uint64_t sweep_type_words(FeedTypeWord feed) {
    // NOT const, despite what misc-const-correctness says. `feed` has a
    // dependent type, so clang-tidy analyses this template body without knowing
    // that feed takes its argument by non-const reference and mutates it --
    // making `h` const would not compile once instantiated.
    // NOLINTNEXTLINE(misc-const-correctness)
    Fnv1a h;
    for (uint32_t raw = 0; raw <= 0xFFFF; ++raw) {
        feed(h, static_cast<uint16_t>(raw));
    }
    return h.value();
}

void feed_real_type_word(Fnv1a& h, uint16_t raw) {
    const mie::TypeWord tw = dec::decode_type_word(raw);
    h.byte(tw.message_type);
    h.byte(static_cast<uint8_t>(tw.bus));
    h.u16(tw.word_count);
    h.byte(tw.error ? 1 : 0);
}

void feed_real_command_word(Fnv1a& h, uint16_t raw) {
    const mie::CommandWord cw = dec::decode_command_word(raw);
    h.byte(cw.rt);
    h.byte(static_cast<uint8_t>(cw.direction));
    h.byte(cw.subaddress);
    h.byte(cw.data_word_count);
}

void feed_real_irig(Fnv1a& h, uint16_t upper, uint16_t middle, uint16_t lower) {
    const mie::IrigTimestamp ts = dec::decode_irig_timestamp(upper, middle, lower);
    h.u16(ts.day);
    h.byte(ts.hour);
    h.byte(ts.minute);
    h.byte(ts.second);
    h.u32(ts.microsecond);
    h.byte(ts.freerun ? 1 : 0);
}

}  // namespace

// ---------------------------------------------------------------------------
// The digest actually discriminates
// ---------------------------------------------------------------------------

TEST_CASE("the digest changes when any decoded field changes", "[decode][exhaustive]") {
    // Run FIRST in this file by position, because every assertion below depends
    // on it. All four reference digests happen to end in the same byte as the
    // FNV offset basis, which is the kind of coincidence that should not be
    // taken on trust: if the hash ignored its input, every digest would match
    // and this file would be an elaborate no-op.
    const uint64_t real = sweep_type_words(feed_real_type_word);
    CHECK(real != Fnv1a().value());

    SECTION("a single wrong field in 65536 decodes is detected") {
        // One input out of 65 536 decodes with a word count one too high --
        // the smallest possible divergence.
        const uint64_t perturbed = sweep_type_words([](Fnv1a& h, uint16_t raw) {
            const mie::TypeWord tw = dec::decode_type_word(raw);
            h.byte(tw.message_type);
            h.byte(static_cast<uint8_t>(tw.bus));
            h.u16(raw == 0x8000 ? static_cast<uint16_t>(tw.word_count + 1) : tw.word_count);
            h.byte(tw.error ? 1 : 0);
        });
        CHECK(perturbed != real);
    }

    SECTION("a swapped field order is detected") {
        // Guards the case where both implementations decode correctly but this
        // file feeds the fields in a different order from the Rust generator,
        // which would make a passing digest meaningless.
        const uint64_t reordered = sweep_type_words([](Fnv1a& h, uint16_t raw) {
            const mie::TypeWord tw = dec::decode_type_word(raw);
            h.u16(tw.word_count);
            h.byte(tw.message_type);
            h.byte(static_cast<uint8_t>(tw.bus));
            h.byte(tw.error ? 1 : 0);
        });
        CHECK(reordered != real);
    }

    SECTION("the hash is sensitive to every byte value") {
        for (int b = 0; b < 256; ++b) {
            Fnv1a a;
            Fnv1a c;
            a.byte(static_cast<uint8_t>(b));
            c.byte(static_cast<uint8_t>((b + 1) & 0xFF));
            CHECK(a.value() != c.value());
        }
    }
}

// ---------------------------------------------------------------------------
// Exhaustive agreement with the Rust implementation
// ---------------------------------------------------------------------------

TEST_CASE("every Type Word decodes as Rust decodes it", "[decode][exhaustive][L3-CPP-004]") {
    // All 65 536 possible raw values. Not a sample.
    CHECK(sweep_type_words(feed_real_type_word) == kTypeWordDigest);
}

TEST_CASE("every Command Word decodes as Rust decodes it", "[decode][exhaustive][L3-CPP-004]") {
    Fnv1a h;
    for (uint32_t raw = 0; raw <= 0xFFFF; ++raw) {
        feed_real_command_word(h, static_cast<uint16_t>(raw));
    }
    CHECK(h.value() == kCommandWordDigest);
}

TEST_CASE("every IRIG field bit decodes as Rust decodes it", "[decode][exhaustive][L3-CPP-004]") {
    // Three sweeps, one per word, each covering that word's full range while
    // the others hold a mixed bit pattern. A single sweep over all three would
    // be 2^48 decodes; this covers every bit position of every field, which is
    // what a shift or mask error actually gets wrong. The fixed values must
    // match the Rust generator exactly.
    Fnv1a h;
    for (uint32_t upper = 0; upper <= 0xFFFF; ++upper) {
        feed_real_irig(h, static_cast<uint16_t>(upper), 0xA5A5, 0x5A5A);
    }
    for (uint32_t middle = 0; middle <= 0xFFFF; ++middle) {
        feed_real_irig(h, 0xA5A5, static_cast<uint16_t>(middle), 0x5A5A);
    }
    for (uint32_t lower = 0; lower <= 0xFFFF; ++lower) {
        feed_real_irig(h, 0xA5A5, 0x5A5A, static_cast<uint16_t>(lower));
    }
    CHECK(h.value() == kIrigDigest);
}

TEST_CASE("every Standard timestamp decodes as Rust decodes it",
          "[decode][exhaustive][L3-CPP-004]") {
    Fnv1a h;
    for (uint32_t upper = 0; upper <= 0xFFFF; ++upper) {
        // The complement gives the lower word a different pattern from the
        // upper, so a decoder that swapped the two would change the digest.
        const uint16_t u = static_cast<uint16_t>(upper);
        const mie::StandardTimestamp ts =
            dec::decode_standard_timestamp(u, static_cast<uint16_t>(~u));
        h.u32(ts.raw_value);
        h.u16(ts.upper_word);
        h.u16(ts.lower_word);
    }
    CHECK(h.value() == kStandardDigest);
}

// ---------------------------------------------------------------------------
// Bit-partition completeness -- self-contained, no reference needed
// ---------------------------------------------------------------------------
//
// The digests prove agreement with Rust. These prove something Rust cannot: that
// the fields PARTITION the raw word. Reassembling the raw value from the decoded
// fields must reproduce it exactly, for every input. That catches overlapping
// masks, a dropped bit, and a field read from the wrong position -- errors that
// both implementations could in principle share.

TEST_CASE("Type Word fields partition the raw word with nothing lost", "[decode][exhaustive]") {
    for (uint32_t raw32 = 0; raw32 <= 0xFFFF; ++raw32) {
        const uint16_t raw = static_cast<uint16_t>(raw32);
        const mie::TypeWord tw = dec::decode_type_word(raw);

        // Bit 15 is reserved and is not carried in a decoded field, so it is
        // taken from the raw word to complete the reconstruction.
        const uint16_t reserved = static_cast<uint16_t>(raw & 0x8000);
        const uint16_t rebuilt =
            static_cast<uint16_t>(static_cast<uint16_t>(tw.message_type) |
                                  static_cast<uint16_t>((tw.bus == mie::BUS_B ? 1u : 0u) << 7) |
                                  static_cast<uint16_t>(tw.word_count << 8) |
                                  static_cast<uint16_t>((tw.error ? 1u : 0u) << 14) | reserved);

        if (rebuilt != raw) {
            // Reported once rather than 65 536 times: a mask error fails for a
            // large fraction of inputs, and one legible failure is more useful
            // than a wall of them.
            INFO("raw = 0x" << std::hex << raw);
            REQUIRE(rebuilt == raw);
        }
    }
    SUCCEED("all 65536 Type Words reassemble exactly");
}

TEST_CASE("Type Word fields never exceed their bit widths", "[decode][exhaustive]") {
    for (uint32_t raw32 = 0; raw32 <= 0xFFFF; ++raw32) {
        const mie::TypeWord tw = dec::decode_type_word(static_cast<uint16_t>(raw32));
        if (tw.message_type > 0x7F || tw.word_count > 63) {
            INFO("raw = 0x" << std::hex << raw32);
            REQUIRE(tw.message_type <= 0x7F);
            REQUIRE(tw.word_count <= 63);
        }
    }
    SUCCEED("all 65536 Type Words stay within their field widths");
}

TEST_CASE("Command Word fields partition the raw word", "[decode][exhaustive]") {
    for (uint32_t raw32 = 0; raw32 <= 0xFFFF; ++raw32) {
        const uint16_t raw = static_cast<uint16_t>(raw32);
        const mie::CommandWord cw = dec::decode_command_word(raw);

        // The data-word-count field is the one place the decoded value is NOT
        // the raw field: a raw 0 means 32. Undoing that mapping is what makes
        // the reconstruction exact, and it is why 32 must map back to 0 rather
        // than to 32 & 0x1F -- which happens to also be 0, so the test would
        // pass either way. The dedicated assertion below covers the direction
        // this cannot.
        const uint16_t dwc_field =
            static_cast<uint16_t>(cw.data_word_count == 32 ? 0 : cw.data_word_count);
        const uint16_t rebuilt = static_cast<uint16_t>(
            static_cast<uint16_t>(cw.rt << 11) |
            static_cast<uint16_t>((cw.direction == mie::DIRECTION_TRANSMIT ? 1u : 0u) << 10) |
            static_cast<uint16_t>(cw.subaddress << 5) | dwc_field);

        if (rebuilt != raw) {
            INFO("raw = 0x" << std::hex << raw);
            REQUIRE(rebuilt == raw);
        }
    }
    SUCCEED("all 65536 Command Words reassemble exactly");
}

TEST_CASE("the data word count is always 1 to 32, never 0", "[decode][exhaustive]") {
    // The invariant the raw-0-means-32 mapping exists to produce. Checked over
    // the whole space because a decoder that dropped the mapping would emit 0
    // for exactly one input value in 32 -- easy to miss in a sample.
    int saw_32 = 0;
    for (uint32_t raw32 = 0; raw32 <= 0xFFFF; ++raw32) {
        const mie::CommandWord cw = dec::decode_command_word(static_cast<uint16_t>(raw32));
        if (cw.data_word_count < 1 || cw.data_word_count > 32) {
            INFO("raw = 0x" << std::hex << raw32);
            REQUIRE(cw.data_word_count >= 1);
            REQUIRE(cw.data_word_count <= 32);
        }
        if (cw.data_word_count == 32) {
            ++saw_32;
        }
    }
    // Exactly one in 32 raw values has a zero count field, and every one of
    // them must have become 32.
    CHECK(saw_32 == 65536 / 32);
}

TEST_CASE("IRIG fields partition their three words", "[decode][exhaustive]") {
    // Swept per word, matching the digest sweeps. Reassembly proves no bit of
    // any of the three words is dropped or double-counted.
    const uint16_t fixed_middle = 0xA5A5;
    const uint16_t fixed_lower = 0x5A5A;

    for (uint32_t upper32 = 0; upper32 <= 0xFFFF; ++upper32) {
        const uint16_t upper = static_cast<uint16_t>(upper32);
        const mie::IrigTimestamp ts = dec::decode_irig_timestamp(upper, fixed_middle, fixed_lower);
        // Bit 14 is RESERVED and belongs to no decoded field: freerun is bit 15,
        // day is bits 5-13 and hour is bits 0-4, which accounts for 15 of the
        // 16 bits. docs/MIE-FORMAT.md documents it as "Reserved for future
        // use". It is carried across from the raw word so the reconstruction is
        // exact -- the same treatment Type Word bit 15 gets.
        //
        // This is worth stating rather than quietly masking: the first draft of
        // this test assumed the three fields covered the word, and failed at
        // upper = 0x4000. The failure was the test's, not the decoder's, but
        // finding out which took reading the format spec -- which is precisely
        // what an exhaustive reconstruction is for.
        const uint16_t reserved_bit14 = static_cast<uint16_t>(upper & 0x4000);
        const uint16_t rebuilt_upper = static_cast<uint16_t>(
            static_cast<uint16_t>((ts.freerun ? 1u : 0u) << 15) | reserved_bit14 |
            static_cast<uint16_t>(ts.day << 5) | static_cast<uint16_t>(ts.hour));
        if (rebuilt_upper != upper) {
            INFO("upper = 0x" << std::hex << upper);
            REQUIRE(rebuilt_upper == upper);
        }
    }

    for (uint32_t middle32 = 0; middle32 <= 0xFFFF; ++middle32) {
        const uint16_t middle = static_cast<uint16_t>(middle32);
        const mie::IrigTimestamp ts = dec::decode_irig_timestamp(0xA5A5, middle, fixed_lower);
        const uint16_t rebuilt_middle = static_cast<uint16_t>(
            static_cast<uint16_t>(ts.minute << 10) | static_cast<uint16_t>(ts.second << 4) |
            static_cast<uint16_t>((ts.microsecond >> 16) & 0xF));
        if (rebuilt_middle != middle) {
            INFO("middle = 0x" << std::hex << middle);
            REQUIRE(rebuilt_middle == middle);
        }
    }

    for (uint32_t lower32 = 0; lower32 <= 0xFFFF; ++lower32) {
        const uint16_t lower = static_cast<uint16_t>(lower32);
        const mie::IrigTimestamp ts = dec::decode_irig_timestamp(0xA5A5, 0xA5A5, lower);
        if (static_cast<uint16_t>(ts.microsecond & 0xFFFF) != lower) {
            INFO("lower = 0x" << std::hex << lower);
            REQUIRE(static_cast<uint16_t>(ts.microsecond & 0xFFFF) == lower);
        }
    }
    SUCCEED("every bit of all three IRIG words is accounted for");
}

TEST_CASE("IRIG fields never exceed their bit widths", "[decode][exhaustive]") {
    // Bounded by the ENCODING, not by calendar sense: a day of 500 or an hour
    // of 31 is out of range semantically and is rejected by sync validation,
    // but the decoder's job is to report what the bits say. Confusing the two
    // is how a decoder starts silently clamping corrupt timestamps into
    // plausible ones.
    for (uint32_t upper32 = 0; upper32 <= 0xFFFF; ++upper32) {
        const mie::IrigTimestamp ts =
            dec::decode_irig_timestamp(static_cast<uint16_t>(upper32), 0xFFFF, 0xFFFF);
        REQUIRE(ts.day <= 0x1FF);
        REQUIRE(ts.hour <= 0x1F);
    }
    for (uint32_t middle32 = 0; middle32 <= 0xFFFF; ++middle32) {
        const mie::IrigTimestamp ts =
            dec::decode_irig_timestamp(0xFFFF, static_cast<uint16_t>(middle32), 0xFFFF);
        REQUIRE(ts.minute <= 0x3F);
        REQUIRE(ts.second <= 0x3F);
        REQUIRE(ts.microsecond <= 0xFFFFF);
    }
    SUCCEED("every IRIG field stays inside its encoded width");
}

TEST_CASE("Standard timestamps reassemble from their two words", "[decode][exhaustive]") {
    for (uint32_t upper32 = 0; upper32 <= 0xFFFF; ++upper32) {
        const uint16_t upper = static_cast<uint16_t>(upper32);
        const uint16_t lower = static_cast<uint16_t>(~upper);
        const mie::StandardTimestamp ts = dec::decode_standard_timestamp(upper, lower);
        const uint32_t rebuilt =
            (static_cast<uint32_t>(ts.upper_word) << 16) | static_cast<uint32_t>(ts.lower_word);
        if (rebuilt != ts.raw_value || ts.upper_word != upper || ts.lower_word != lower) {
            INFO("upper = 0x" << std::hex << upper);
            REQUIRE(rebuilt == ts.raw_value);
            REQUIRE(ts.upper_word == upper);
            REQUIRE(ts.lower_word == lower);
        }
    }
    SUCCEED("all 65536 Standard timestamps reassemble exactly");
}

// ---------------------------------------------------------------------------
// Exhaustive classification
// ---------------------------------------------------------------------------

TEST_CASE("classification accepts exactly the seven known type codes", "[decode][exhaustive]") {
    const mie::CommandWord cmd = dec::decode_command_word(0x797E);
    int accepted = 0;
    for (int code = 0; code <= 0xFF; ++code) {
        mie::MessageFormat fmt = mie::FORMAT_SPURIOUS_DATA;
        const bool ok = dec::classify_message_format(static_cast<uint8_t>(code), cmd, 36, 3, fmt);
        // Acceptance must agree with the type-code predicate for every byte;
        // a disagreement means the reader and the classifier would take
        // different views of the same record.
        REQUIRE(ok == mie::is_valid_message_type(static_cast<uint8_t>(code)));
        if (ok) {
            ++accepted;
        }
    }
    CHECK(accepted == 7);
}

TEST_CASE("mode-code classification is exhaustive over word count and both timestamp widths",
          "[decode][exhaustive][L2-MSG-004]") {
    // Every word count a six-bit field can hold, both timestamp widths, both
    // directions, broadcast and not. This is the full input space of the
    // mode-code shape decision -- the one place where an absolute threshold
    // instead of a relative one would misclassify silently.
    const uint16_t timestamp_widths[] = {2, 3};

    for (std::size_t w = 0; w < 2; ++w) {
        const uint16_t ts_words = timestamp_widths[w];
        for (uint16_t wc = 0; wc <= 63; ++wc) {
            // Non-broadcast, transmit: RT 5, subaddress 0.
            const mie::CommandWord tx =
                dec::decode_command_word(static_cast<uint16_t>((5 << 11) | (1 << 10)));
            const mie::CommandWord rx = dec::decode_command_word(static_cast<uint16_t>(5 << 11));
            const mie::CommandWord bcast =
                dec::decode_command_word(static_cast<uint16_t>(31 << 11));

            mie::MessageFormat fmt = mie::FORMAT_SPURIOUS_DATA;

            REQUIRE(dec::classify_message_format(mie::MESSAGE_TYPE_MODE_COMMAND, tx, wc, ts_words,
                                                 fmt));
            REQUIRE(fmt == (wc >= ts_words + 4 ? mie::FORMAT_MODE_CODE_TX_DATA
                                               : mie::FORMAT_MODE_CODE_NO_DATA));

            REQUIRE(dec::classify_message_format(mie::MESSAGE_TYPE_MODE_COMMAND, rx, wc, ts_words,
                                                 fmt));
            REQUIRE(fmt == (wc >= ts_words + 4 ? mie::FORMAT_MODE_CODE_RX_DATA
                                               : mie::FORMAT_MODE_CODE_NO_DATA));

            REQUIRE(dec::classify_message_format(mie::MESSAGE_TYPE_MODE_COMMAND, bcast, wc,
                                                 ts_words, fmt));
            REQUIRE(fmt == (wc >= ts_words + 3 ? mie::FORMAT_MODE_CODE_BCAST_DATA
                                               : mie::FORMAT_MODE_CODE_BCAST_NO_DATA));
        }
    }
    SUCCEED("every mode-code word count classifies identically under both timestamp widths");
}

TEST_CASE("the non-mode-code types ignore word count and timestamp width", "[decode][exhaustive]") {
    // Their format is fixed by the type code alone. Sweeping the other inputs
    // proves no accidental dependency crept in -- the kind of thing that would
    // make a Standard-format file classify differently from an IRIG one.
    const mie::CommandWord cmd = dec::decode_command_word(0x797E);
    const uint8_t codes[] = {
        mie::MESSAGE_TYPE_BC_TO_RT,           mie::MESSAGE_TYPE_RT_TO_BC,
        mie::MESSAGE_TYPE_RT_TO_RT,           mie::MESSAGE_TYPE_BROADCAST_BC_TO_RT,
        mie::MESSAGE_TYPE_BROADCAST_RT_TO_RT, mie::MESSAGE_TYPE_SPURIOUS_DATA};
    const mie::MessageFormat expected[] = {
        mie::FORMAT_RECEIVE,           mie::FORMAT_TRANSMIT,           mie::FORMAT_RT_TO_RT,
        mie::FORMAT_RECEIVE_BROADCAST, mie::FORMAT_RT_TO_RT_BROADCAST, mie::FORMAT_SPURIOUS_DATA};

    for (std::size_t i = 0; i < sizeof(codes) / sizeof(codes[0]); ++i) {
        for (uint16_t wc = 0; wc <= 63; ++wc) {
            for (uint16_t ts = 2; ts <= 3; ++ts) {
                mie::MessageFormat fmt = mie::FORMAT_SPURIOUS_DATA;
                REQUIRE(dec::classify_message_format(codes[i], cmd, wc, ts, fmt));
                REQUIRE(fmt == expected[i]);
            }
        }
    }
    SUCCEED("the six fixed-format types are stable across every word count");
}

// ---------------------------------------------------------------------------
// Exhaustive bounds on the primitive readers
// ---------------------------------------------------------------------------

TEST_CASE("read_u16 accepts exactly the in-bounds offsets", "[decode][exhaustive][L1-ROB-001]") {
    // Every offset from 0 past the end, for buffers of every small size. The
    // boundary is where a bounds check is wrong, and "every offset" leaves no
    // room for the off-by-one to hide.
    uint8_t buffer[16];
    for (std::size_t i = 0; i < sizeof(buffer); ++i) {
        buffer[i] = static_cast<uint8_t>(i);
    }

    for (std::size_t size = 0; size <= sizeof(buffer); ++size) {
        for (std::size_t offset = 0; offset <= size + 4; ++offset) {
            uint16_t value = 0;
            const bool ok = dec::read_u16(buffer, size, offset, value);
            const bool should = (size >= 2) && (offset <= size - 2);
            if (ok != should) {
                INFO("size = " << size << " offset = " << offset);
                REQUIRE(ok == should);
            }
            if (ok) {
                REQUIRE(value == static_cast<uint16_t>(buffer[offset] | (buffer[offset + 1] << 8)));
            }
        }
    }
    SUCCEED("read_u16 boundary is exact for every size and offset tested");
}

TEST_CASE("read_u16_array accepts exactly the in-bounds ranges",
          "[decode][exhaustive][L1-ROB-001]") {
    uint8_t buffer[16];
    for (std::size_t i = 0; i < sizeof(buffer); ++i) {
        buffer[i] = static_cast<uint8_t>(i * 7);
    }
    uint16_t out[16];

    for (std::size_t size = 0; size <= sizeof(buffer); ++size) {
        for (std::size_t offset = 0; offset <= size + 2; ++offset) {
            for (std::size_t count = 0; count <= 8; ++count) {
                const bool ok = dec::read_u16_array(buffer, size, offset, count, out);
                const bool should = (count == 0) || (offset <= size && size - offset >= count * 2);
                if (ok != should) {
                    INFO("size = " << size << " offset = " << offset << " count = " << count);
                    REQUIRE(ok == should);
                }
            }
        }
    }
    SUCCEED("read_u16_array boundary is exact for every size, offset and count tested");
}
