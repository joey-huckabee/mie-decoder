// SPDX-License-Identifier: Apache-2.0
//
// Test-only helper: a filesystem path that removes itself.
//
// WHY THIS EXISTS. The suites here write real files, because the platform layer
// and the reader both take a path and there is no in-memory entry point worth
// inventing purely for tests. Cleaning those up with a call at the end of each
// test is one `REQUIRE` away from being skipped: an assertion failure unwinds,
// the cleanup line never runs, and the temp directory fills with fixtures from
// every failing run. Tying removal to a destructor makes that impossible rather
// than merely unlikely.
//
// The subtle half is CONSTRUCTION. A fixture that creates a file and then
// asserts something about it -- that the write succeeded, that the handle
// closed -- can throw from its own constructor, at which point its destructor
// never runs and the file it just created leaks. Holding the path as a MEMBER
// fixes that: members are destroyed when a constructor exits by exception, so
// the removal happens even though the object was never fully built.
//
// WHAT THE ISOLATION RULE ACTUALLY COVERS. test_platform.cpp creates and reads
// back its fixtures with <cstdio> rather than through AtomicFile, so that a bug
// in AtomicFile cannot make an AtomicFile test pass by being consistently wrong
// in both directions. That rule is about the WRITE-AND-READ-BACK path, and it
// is preserved here. Composing a path from `process_id()` is not part of it: a
// wrong process id produces a differently-named file, not a test that passes
// for the wrong reason. Separator handling is done locally anyway, because
// `path_join` IS under test.

#ifndef MIE_TESTS_TEMP_PATH_HPP
#define MIE_TESTS_TEMP_PATH_HPP

#include <catch2/catch.hpp>

#include <cstdio>
#include <cstdlib>
#include <string>
#include <vector>

#include "mie/platform.hpp"

namespace mie_test {

/// A directory the operating system will let us write to.
inline std::string temp_root() {
    const char* candidates[] = {"TMPDIR", "TEMP", "TMP"};
    for (std::size_t i = 0; i < sizeof(candidates) / sizeof(candidates[0]); ++i) {
        const char* value = std::getenv(candidates[i]);
        if (value != 0 && value[0] != '\0') {
            return std::string(value);
        }
    }
#if defined(_WIN32)
    return std::string("C:\\Windows\\Temp");
#else
    return std::string("/tmp");
#endif
}

/// The platform's preferred separator, spelled locally on purpose: `path_join`
/// is one of the functions under test, and building fixture paths with it would
/// let a bug in it hide itself.
inline char temp_separator() {
#if defined(_WIN32)
    return '\\';
#else
    return '/';
#endif
}

/// A distinct token for one fixture, as `-<pid>-<counter>`.
///
/// Both halves earn their place: the counter alone would collide with a
/// previous crashed run's leftovers, and the process id alone would collide
/// within one run. Counter-based rather than random so a failing run names a
/// file you can go and look at.
inline std::string unique_token() {
    static int counter = 0;
    char buffer[48];
    // snprintf, not sprintf, and the result is checked. A helper that silently
    // truncates hands two fixtures the same path, and the failure that produces
    // looks like a bug in the code under test.
    const int written =
        std::snprintf(buffer, sizeof(buffer), "-%llu-%d",
                      static_cast<unsigned long long>(mie::platform::process_id()), counter++);
    REQUIRE(written > 0);
    REQUIRE(static_cast<std::size_t>(written) < sizeof(buffer));
    return std::string(buffer);
}

/// `<temp>/mie-<leaf><token>` -- a unique path that does not exist yet.
inline std::string unique_temp_path(const std::string& leaf) {
    std::string out = temp_root();
    if (!out.empty() && out[out.size() - 1] != temp_separator()) {
        out += temp_separator();
    }
    out += "mie-";
    out += leaf;
    out += unique_token();
    return out;
}

/// A path that removes itself, along with any derived paths named through
/// `sibling()`.
///
/// The file is NOT created here. Several tests assert that the code under test
/// did *not* create it, and a helper that created it would make those
/// unwritable. Removing a path that never existed is a no-op, which is what
/// lets one type serve both cases.
class TempPath {
  public:
    /// A unique path built from `leaf`.
    explicit TempPath(const std::string& leaf) : path_(unique_temp_path(leaf)), siblings_() {}

    ~TempPath() {
        for (std::size_t i = 0; i < siblings_.size(); ++i) {
            (void)std::remove(siblings_[i].c_str());
        }
        (void)std::remove(path_.c_str());
    }

    const std::string& str() const { return path_; }

    /// A derived path the code under test creates -- `<dest>.partial`,
    /// `<dest>_errors.csv` -- registered for removal alongside this one.
    ///
    /// Returned BY VALUE. Handing back a reference into `siblings_` would be
    /// live only until the next call reallocated the vector, which is a dangling
    /// reference introduced by a helper whose whole purpose is to remove a
    /// footgun.
    std::string sibling(const std::string& suffix) {
        const std::string derived = path_ + suffix;
        siblings_.push_back(derived);
        return derived;
    }

  private:
    TempPath(const TempPath&);
    TempPath& operator=(const TempPath&);

    std::string path_;
    std::vector<std::string> siblings_;
};

/// A TempPath whose file exists, with the given contents.
///
/// The path is a MEMBER rather than a base or a local, which is the whole
/// point: the `REQUIRE`s below run after the file exists, and a member is
/// destroyed when a constructor exits by exception. Written with <cstdio>,
/// never through the platform layer -- see the isolation note above.
class TempFile {
  public:
    TempFile(const std::string& leaf, const std::string& contents) : path_(leaf) {
        write(contents.data(), contents.size());
    }

    TempFile(const std::string& leaf, const std::vector<uint8_t>& contents) : path_(leaf) {
        write(contents.empty() ? 0 : reinterpret_cast<const char*>(&contents[0]), contents.size());
    }

    const std::string& str() const { return path_.str(); }
    TempPath& path() { return path_; }

  private:
    TempFile(const TempFile&);
    TempFile& operator=(const TempFile&);

    void write(const char* bytes, std::size_t length) {
        std::FILE* handle = std::fopen(path_.str().c_str(), "wb");
        REQUIRE(handle != 0);
        if (length != 0) {
            const std::size_t written = std::fwrite(bytes, 1, length, handle);
            if (written != length) {
                // Close before asserting: REQUIRE throws, and leaking the handle
                // on the failure path is how a Windows test then fails to remove
                // the file it is complaining about.
                (void)std::fclose(handle);
                REQUIRE(written == length);
            }
        }
        // fclose is where a buffered write actually reports failure. A fixture
        // that never reached the disk would make the test it feeds fail for
        // entirely the wrong reason, so this is asserted rather than ignored.
        REQUIRE(std::fclose(handle) == 0);
    }

    TempPath path_;
};

}  // namespace mie_test

#endif  // MIE_TESTS_TEMP_PATH_HPP
