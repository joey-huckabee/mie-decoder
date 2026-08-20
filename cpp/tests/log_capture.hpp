// SPDX-License-Identifier: Apache-2.0
//
// Test-only helper: capture emitted log lines instead of writing them to
// stderr.
//
// Shared by test_log.cpp and test_reader.cpp. The reader is the module that
// TALKS -- sync and decode are pure by rule, so every operator-facing WARN
// about a freerun timestamp, a non-monotonic clock or a recovered sync loss is
// emitted from the reader and is part of its contract. Asserting that contract
// needs the lines, and reading the process's real stderr from inside the same
// process is neither portable nor reliable.
//
// NOT included in the shipped library. The sink hook it uses exists on the
// logger for this purpose and defaults to stderr.

#ifndef MIE_TESTS_LOG_CAPTURE_HPP
#define MIE_TESTS_LOG_CAPTURE_HPP

#include <cstddef>
#include <string>
#include <vector>

#include "mie/log.hpp"

namespace mie_test {

/// The captured lines. A function-local static rather than a namespace-scope
/// object: the sink is a plain function pointer, so it cannot capture, and a
/// function-local static also sidesteps the static-initialisation-order
/// question entirely.
inline std::vector<std::string>& captured_lines() {
    static std::vector<std::string> lines;
    return lines;
}

inline void capture_sink(const char* text, std::size_t length) {
    captured_lines().push_back(std::string(text, length));
}

/// Installs the capturing sink at `level`, and restores both on destruction.
///
/// RAII rather than a call pair, and that is load-bearing: a failing REQUIRE
/// unwinds, and a leaked sink would send every later test case in the binary
/// into this buffer. The resulting failures would point anywhere but at the
/// test that leaked.
class LogCapture {
  public:
    explicit LogCapture(mie::log::Level level) : previous_(mie::log::current_level()) {
        captured_lines().clear();
        mie::log::set_level(level);
        mie::log::set_sink(&capture_sink);
    }

    ~LogCapture() {
        mie::log::set_sink(0);
        mie::log::set_level(previous_);
    }

    const std::vector<std::string>& lines() const { return captured_lines(); }
    std::size_t count() const { return captured_lines().size(); }

    /// How many captured lines contain `needle`.
    ///
    /// Substring rather than equality on purpose: these messages carry offsets
    /// and counts that a test should not have to restate, and pinning the exact
    /// wording of a diagnostic makes every future clarification a test failure.
    /// The conformance manifest asserts stderr the same way.
    std::size_t count_containing(const std::string& needle) const {
        std::size_t hits = 0;
        const std::vector<std::string>& all = captured_lines();
        for (std::size_t i = 0; i < all.size(); ++i) {
            if (all[i].find(needle) != std::string::npos) {
                hits += 1;
            }
        }
        return hits;
    }

    bool contains(const std::string& needle) const { return count_containing(needle) > 0; }

  private:
    LogCapture(const LogCapture&);
    LogCapture& operator=(const LogCapture&);

    mie::log::Level previous_;
};

}  // namespace mie_test

#endif  // MIE_TESTS_LOG_CAPTURE_HPP
