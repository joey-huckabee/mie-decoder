// SPDX-License-Identifier: Apache-2.0
//
// Tests for the platform layer -- the only module that touches the OS, and
// therefore the only one whose behaviour can differ between the SLES 12 target
// and the shipped Windows binary.
//
// These deliberately do NOT use the platform layer to set up their own
// fixtures. Files are created and read back with <cstdio> so that a bug in
// AtomicFile cannot make an AtomicFile test pass by being consistently wrong in
// both directions.

#include "mie/platform.hpp"

#include <catch2/catch.hpp>

#include <algorithm>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <vector>

namespace {

namespace plat = mie::platform;

/// A directory the OS will let us write to, without using the layer under test.
std::string temp_root() {
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

/// A unique path under the temp directory. Counter-based rather than random so
/// a failing run names a file you can actually go and look at.
std::string temp_path(const std::string& leaf) {
    static int counter = 0;
    char suffix[32];
    // snprintf, not sprintf, and the result is checked. A test helper that can
    // silently truncate produces two fixtures with the same path, and the
    // resulting failure looks like a bug in the code under test.
    const int written = std::snprintf(suffix, sizeof(suffix), "-%d-%d",
                                      static_cast<int>(plat::process_id()), counter++);
    REQUIRE(written > 0);
    REQUIRE(static_cast<std::size_t>(written) < sizeof(suffix));
    return plat::path_join(temp_root(), std::string("mie-cpp-test-") + leaf + suffix);
}

void write_raw(const std::string& path, const std::string& contents) {
    std::FILE* f = std::fopen(path.c_str(), "wb");
    REQUIRE(f != 0);
    if (!contents.empty()) {
        REQUIRE(std::fwrite(contents.data(), 1, contents.size(), f) == contents.size());
    }
    // fclose is where a buffered write actually reports failure. A fixture that
    // never reached the disk would make the test it feeds fail for the wrong
    // reason, so this is asserted rather than ignored.
    REQUIRE(std::fclose(f) == 0);
}

/// Read a file back as raw bytes. Binary mode matters: the whole point of
/// several tests below is that no newline translation happened.
std::string read_raw(const std::string& path) {
    std::FILE* f = std::fopen(path.c_str(), "rb");
    if (f == 0) {
        return std::string();
    }
    std::string out;
    char buffer[4096];
    // Stop on the first SHORT read rather than on a zero one. A read that hits
    // the end of the file returns a positive count AND sets the EOF flag, so a
    // `while (n > 0)` loop calls fread once more with the stream already in EOF
    // state -- undefined-ish behaviour that clang-analyzer's unix.Stream check
    // reports, and it is right to. Stopping short also stops on a read error,
    // after which the stream position is indeterminate and there is nothing
    // trustworthy left to read.
    for (;;) {
        const std::size_t n = std::fread(buffer, 1, sizeof(buffer), f);
        out.append(buffer, n);
        if (n < sizeof(buffer)) {
            break;
        }
    }
    // Read-only: a close failure here cannot have lost data, so it is discarded
    // explicitly rather than asserted.
    (void)std::fclose(f);
    return out;
}

bool raw_exists(const std::string& path) {
    std::FILE* f = std::fopen(path.c_str(), "rb");
    if (f == 0) {
        return false;
    }
    (void)std::fclose(f);
    return true;
}

/// Removes a path if present, ignoring failure. Used in teardown, where a
/// missing file is the expected case as often as not -- several tests assert
/// that a file was NOT created and then clean up unconditionally.
void discard(const std::string& path) { (void)std::remove(path.c_str()); }

}  // namespace

// ---------------------------------------------------------------------------
// Path string helpers
// ---------------------------------------------------------------------------

TEST_CASE("path_parent and path_filename split at the final separator", "[platform][path]") {
    CHECK(plat::path_filename("/a/b/c.csv") == "c.csv");
    CHECK(plat::path_parent("/a/b/c.csv") == "/a/b");

    SECTION("a bare name has no parent") {
        CHECK(plat::path_parent("out.csv").empty());
        CHECK(plat::path_filename("out.csv") == "out.csv");
    }

    SECTION("the root separator survives as the parent") {
        // The parent of "/x" is "/", not "". Returning "" would make path_join
        // produce a relative path from an absolute one -- the temp file would
        // land in the working directory instead of beside the destination,
        // which silently breaks the single-filesystem rename guarantee.
        CHECK(plat::path_parent("/x") == "/");
    }
}

TEST_CASE("is_separator treats backslash as platform-specific", "[platform][path]") {
    CHECK(plat::is_separator('/'));
#if defined(_WIN32)
    CHECK(plat::is_separator('\\'));
#else
    // On POSIX a backslash is an ordinary filename character. Treating it as a
    // separator would truncate any path that legitimately contains one -- a bug
    // that cannot reproduce on Windows.
    CHECK_FALSE(plat::is_separator('\\'));
    CHECK(plat::path_filename("/tmp/od\\d.csv") == "od\\d.csv");
#endif
}

TEST_CASE("path_join collapses a doubled separator", "[platform][path]") {
    const std::string joined = plat::path_join("dir/", "/name");
    CHECK(plat::path_filename(joined) == "name");
    CHECK(joined.find("//") == std::string::npos);

    CHECK(plat::path_join("", "name") == "name");
    CHECK(plat::path_join("dir", "") == "dir");
}

// ---------------------------------------------------------------------------
// Temp-name construction (L3-WRT-001)
// ---------------------------------------------------------------------------

TEST_CASE("make_temp_name sits beside the destination and never repeats",
          "[platform][atomic][L3-WRT-001][L3-CPP-004]") {
    const std::string destination = plat::path_join("/some/dir", "out.csv");

    const std::string first = plat::make_temp_name(destination);
    const std::string second = plat::make_temp_name(destination);

    SECTION("it is co-located with the destination") {
        // Same-directory placement is what keeps the later rename on one
        // filesystem, and therefore atomic.
        CHECK(plat::path_parent(first) == plat::path_parent(destination));
    }

    SECTION("it is recognisable in a directory listing") {
        CHECK(plat::path_filename(first).find("out.csv.mie-decoder.tmp.") == 0);
    }

    SECTION("two calls in one process cannot collide") {
        // The counter component exists for exactly this: a coarse clock makes
        // two calls in one tick a real case, not a theoretical one.
        CHECK(first != second);
    }
}

TEST_CASE("make_temp_name handles a destination with no directory",
          "[platform][atomic][L3-CPP-004]") {
    const std::string name = plat::make_temp_name("out.csv");
    CHECK(plat::path_parent(name).empty());
    CHECK(name.find("out.csv.mie-decoder.tmp.") == 0);
}

// ---------------------------------------------------------------------------
// MappedFile
// ---------------------------------------------------------------------------

TEST_CASE("MappedFile exposes the file as a flat byte range", "[platform][mmap][L3-CPP-004]") {
    const std::string path = temp_path("map");
    const std::string contents("\x02\x24\x00\x00MIE", 7);
    write_raw(path, contents);

    plat::MappedFile mapped;
    plat::OsError err;
    REQUIRE(mapped.open(path, err));
    CHECK(err.ok());
    REQUIRE(mapped.is_open());
    REQUIRE(mapped.size() == contents.size());
    CHECK(std::memcmp(mapped.data(), contents.data(), contents.size()) == 0);

    mapped.close();
    CHECK_FALSE(mapped.is_open());
    discard(path);
}

TEST_CASE("MappedFile reports a missing path as an error", "[platform][mmap][L3-CPP-004]") {
    plat::MappedFile mapped;
    plat::OsError err;
    CHECK_FALSE(mapped.open(temp_path("absent"), err));
    CHECK_FALSE(err.ok());
    CHECK_FALSE(err.message.empty());
}

TEST_CASE("MappedFile rejects an empty file rather than mapping zero bytes",
          "[platform][mmap][L2-RDR-006][L3-CPP-004]") {
    // Callers reject empty inputs before reaching here, but the guard matters:
    // mmap of a zero-length range fails with EINVAL and CreateFileMapping
    // refuses outright, so without it the operator sees a confusing I/O error
    // instead of "the file is empty".
    const std::string path = temp_path("empty");
    write_raw(path, std::string());

    plat::MappedFile mapped;
    plat::OsError err;
    CHECK_FALSE(mapped.open(path, err));
    CHECK_FALSE(err.ok());
    discard(path);
}

TEST_CASE("MappedFile rejects a directory", "[platform][mmap][L3-CPP-004]") {
    // A directory stats as zero bytes on Windows. Opening before testing the
    // size is what keeps this reported as "not a regular file" rather than as
    // "empty" -- the distinction docs/ERROR-CATALOG.md pins for MieFileIoError.
    plat::MappedFile mapped;
    plat::OsError err;
    CHECK_FALSE(mapped.open(temp_root(), err));
    CHECK_FALSE(err.ok());
}

TEST_CASE("a moved-from MappedFile releases its mapping", "[platform][mmap][L3-CPP-004]") {
    const std::string path = temp_path("move");
    write_raw(path, std::string("payload"));

    plat::MappedFile source;
    plat::OsError err;
    REQUIRE(source.open(path, err));
    const uint8_t* original = source.data();

    const plat::MappedFile moved(static_cast<plat::MappedFile&&>(source));
    CHECK(moved.data() == original);
    CHECK(moved.size() == 7);
    // Interrogating a moved-from object is the ENTIRE POINT of this test, so the
    // analyzer's cplusplus.Move warning is suppressed here and nowhere else.
    // The source must not still believe it owns the mapping, or both objects
    // unmap the same range at scope exit -- a double-munmap that valgrind
    // catches only if something first makes the moved-from state observable.
    // NOLINTNEXTLINE(clang-analyzer-cplusplus.Move)
    CHECK_FALSE(source.is_open());

    discard(path);
}

// ---------------------------------------------------------------------------
// AtomicFile
// ---------------------------------------------------------------------------

TEST_CASE("AtomicFile commits through a temp file", "[platform][atomic][L2-WRT-015][L3-CPP-006]") {
    const std::string destination = temp_path("commit.csv");

    plat::AtomicFile file;
    plat::OsError err;
    REQUIRE(file.create(destination, err));

    const std::string temp = file.temp_path();
    CHECK(raw_exists(temp));
    CHECK_FALSE(raw_exists(destination));

    const std::string row("TIME_STAMP,RT,MSG\n");
    REQUIRE(file.write(row.data(), row.size(), err));
    REQUIRE(file.commit(err));

    CHECK(read_raw(destination) == row);
    CHECK_FALSE(raw_exists(temp));
    discard(destination);
}

TEST_CASE("AtomicFile replaces an existing destination",
          "[platform][atomic][L2-WRT-015][L3-CPP-006]") {
    // THE Windows trap, and the reason the backend uses MoveFileExW with
    // MOVEFILE_REPLACE_EXISTING: the MSVC CRT's rename() fails outright when the
    // destination exists. Rust's std::fs::rename uses MoveFileEx internally, so
    // a port written with plain rename() passes on Linux and is broken on the
    // shipped Windows binary. This test is what catches that.
    const std::string destination = temp_path("replace.csv");
    write_raw(destination, std::string("STALE"));

    plat::AtomicFile file;
    plat::OsError err;
    REQUIRE(file.create(destination, err));
    const std::string fresh("FRESH");
    REQUIRE(file.write(fresh.data(), fresh.size(), err));
    REQUIRE(file.commit(err));

    CHECK(read_raw(destination) == fresh);
    discard(destination);
}

TEST_CASE("AtomicFile leaves the destination untouched when aborted",
          "[platform][atomic][L2-WRT-015][L3-CPP-006]") {
    const std::string destination = temp_path("abort.csv");
    write_raw(destination, std::string("ORIGINAL"));

    std::string temp;
    {
        plat::AtomicFile file;
        plat::OsError err;
        REQUIRE(file.create(destination, err));
        temp = file.temp_path();
        const std::string partial("half a row");
        REQUIRE(file.write(partial.data(), partial.size(), err));
        // No commit: the destructor must clean up. This is the sync-loss path
        // without --allow-partial, where the destination must not be modified.
    }

    CHECK(read_raw(destination) == "ORIGINAL");
    CHECK_FALSE(raw_exists(temp));
    discard(destination);
}

TEST_CASE("commit_with_suffix writes the .partial file and spares the destination",
          "[platform][atomic][L3-WRT-002][L3-CPP-006]") {
    const std::string destination = temp_path("partial.csv");
    write_raw(destination, std::string("PREVIOUS"));

    plat::AtomicFile file;
    plat::OsError err;
    REQUIRE(file.create(destination, err));
    const std::string rows("partial rows\n");
    REQUIRE(file.write(rows.data(), rows.size(), err));
    REQUIRE(file.commit_with_suffix(".partial", err));

    CHECK(read_raw(destination + ".partial") == rows);
    CHECK(read_raw(destination) == "PREVIOUS");

    discard(destination);
    discard(destination + ".partial");
}

TEST_CASE("AtomicFile writes bytes verbatim, with no newline translation",
          "[platform][atomic][L2-WRT-012][L3-CPP-005]") {
    // On Windows the CRT rewrites a newline into CRLF unless the handle is
    // opened in binary mode, which would break every byte-exact CSV oracle on
    // that platform alone while Linux stayed green.
    const std::string destination = temp_path("newline.csv");

    plat::AtomicFile file;
    plat::OsError err;
    REQUIRE(file.create(destination, err));
    const std::string payload("a\nb\n");
    REQUIRE(file.write(payload.data(), payload.size(), err));
    REQUIRE(file.commit(err));

    const std::string round_tripped = read_raw(destination);
    CHECK(round_tripped.size() == 4);
    CHECK(round_tripped == payload);
    CHECK(round_tripped.find('\r') == std::string::npos);
    discard(destination);
}

TEST_CASE("AtomicFile survives payloads that straddle the buffer boundary",
          "[platform][atomic][L3-CPP-004]") {
    // Three shapes that each take a different branch: comfortably inside the
    // buffer, exactly at it, and a single write larger than it (which bypasses
    // buffering entirely).
    const std::string destination = temp_path("buffered.csv");

    plat::AtomicFile file;
    plat::OsError err;
    REQUIRE(file.create(destination, err));

    std::string expected;
    const std::string small(100, 'a');
    const std::string exact(plat::kWriteBufferSize, 'b');
    const std::string oversized(plat::kWriteBufferSize * 2 + 7, 'c');

    REQUIRE(file.write(small.data(), small.size(), err));
    expected += small;
    REQUIRE(file.write(exact.data(), exact.size(), err));
    expected += exact;
    REQUIRE(file.write(oversized.data(), oversized.size(), err));
    expected += oversized;
    REQUIRE(file.commit(err));

    CHECK(read_raw(destination).size() == expected.size());
    CHECK(read_raw(destination) == expected);
    discard(destination);
}

// ---------------------------------------------------------------------------
// Directory enumeration
// ---------------------------------------------------------------------------

TEST_CASE("list_directory returns entry names without the dot entries",
          "[platform][glob][L3-CPP-004]") {
    const std::string marker = temp_path("listed.mie");
    write_raw(marker, std::string("x"));

    std::vector<std::string> names;
    plat::OsError err;
    REQUIRE(plat::list_directory(temp_root(), names, err));

    // Findings are accumulated and asserted once, rather than asserted inside
    // the loop. The shared temp directory holds an unpredictable number of
    // unrelated files, so per-entry assertions make the suite's assertion count
    // depend on the host -- which turns a legitimate "157 vs 130" comparison
    // between two CI tiers into noise nobody can read.
    bool found_marker = false;
    bool saw_dot_entry = false;
    bool every_entry_is_a_bare_name = true;
    for (std::size_t i = 0; i < names.size(); ++i) {
        if (names[i] == "." || names[i] == "..") {
            saw_dot_entry = true;
        }
        // Names, not paths: --glob matches over the filename only.
        if (plat::path_filename(names[i]) != names[i]) {
            every_entry_is_a_bare_name = false;
        }
        if (names[i] == plat::path_filename(marker)) {
            found_marker = true;
        }
    }
    CHECK_FALSE(saw_dot_entry);
    CHECK(every_entry_is_a_bare_name);
    CHECK(found_marker);
    discard(marker);
}

TEST_CASE("list_directory reports a missing directory as an error",
          "[platform][glob][L3-CPP-004]") {
    std::vector<std::string> names;
    plat::OsError err;
    CHECK_FALSE(plat::list_directory(temp_path("no-such-dir"), names, err));
    CHECK_FALSE(err.ok());
}

// ---------------------------------------------------------------------------
// Path identity
// ---------------------------------------------------------------------------

TEST_CASE("paths_same_file detects an output aimed at its own input",
          "[platform][identity][L2-WRT-014][L3-CPP-008]") {
    const std::string path = temp_path("collide.mie");
    write_raw(path, std::string("data"));

    bool same = false;
    plat::OsError err;
    REQUIRE(plat::paths_same_file(path, path, same, err));
    CHECK(same);

    discard(path);
}

TEST_CASE("paths_same_file says no for two distinct files",
          "[platform][identity][L2-WRT-014][L3-CPP-008]") {
    const std::string input = temp_path("in.mie");
    const std::string output = temp_path("out.csv");
    write_raw(input, std::string("data"));
    write_raw(output, std::string("other"));

    bool same = true;
    plat::OsError err;
    REQUIRE(plat::paths_same_file(input, output, same, err));
    CHECK_FALSE(same);

    discard(input);
    discard(output);
}

TEST_CASE("paths_same_file tolerates an output that does not exist yet",
          "[platform][identity][L2-WRT-014][L3-CPP-008]") {
    // The normal case: the destination is about to be created. That must not be
    // an error, and it must not be reported as a collision.
    const std::string input = temp_path("in2.mie");
    const std::string output = temp_path("not-created-yet.csv");
    write_raw(input, std::string("data"));

    bool same = true;
    plat::OsError err;
    REQUIRE(plat::paths_same_file(input, output, same, err));
    CHECK_FALSE(same);

    discard(input);
}

TEST_CASE("paths_same_file fails when the input cannot be resolved",
          "[platform][identity][L2-WRT-014][L3-CPP-008]") {
    // Failing to resolve the INPUT is a real failure -- that path is about to
    // be opened -- where failing to resolve the output is routine.
    bool same = false;
    plat::OsError err;
    CHECK_FALSE(plat::paths_same_file(temp_path("absent.mie"), temp_path("out.csv"), same, err));
    CHECK_FALSE(err.ok());
}

TEST_CASE("canonical_path resolves an existing file", "[platform][identity][L3-CPP-008]") {
    const std::string path = temp_path("canon.mie");
    write_raw(path, std::string("data"));

    std::string canonical;
    plat::OsError err;
    REQUIRE(plat::canonical_path(path, canonical, err));
    CHECK_FALSE(canonical.empty());
    CHECK(plat::path_filename(canonical) == plat::path_filename(path));

    discard(path);
}

// ---------------------------------------------------------------------------
// Metadata and encoding
// ---------------------------------------------------------------------------

TEST_CASE("file_metadata reports size and regular-file status",
          "[platform][metadata][L3-CPP-004]") {
    const std::string path = temp_path("meta.mie");
    write_raw(path, std::string("1234567890"));

    uint64_t size = 0;
    bool is_regular = false;
    plat::OsError err;
    REQUIRE(plat::file_metadata(path, size, is_regular, err));
    CHECK(size == 10);
    CHECK(is_regular);

    SECTION("a directory is not a regular file") {
        REQUIRE(plat::file_metadata(temp_root(), size, is_regular, err));
        CHECK_FALSE(is_regular);
    }

    discard(path);
}

TEST_CASE("path_exists answers without reporting an error", "[platform][metadata][L3-CPP-004]") {
    const std::string path = temp_path("exists.mie");
    CHECK_FALSE(plat::path_exists(path));
    write_raw(path, std::string("x"));
    CHECK(plat::path_exists(path));
    discard(path);
}

TEST_CASE("remove_file deletes and then reports the absence", "[platform][metadata][L3-CPP-004]") {
    const std::string path = temp_path("remove.mie");
    write_raw(path, std::string("x"));

    plat::OsError err;
    REQUIRE(plat::remove_file(path, err));
    CHECK_FALSE(raw_exists(path));
    CHECK_FALSE(plat::remove_file(path, err));
    CHECK_FALSE(err.ok());
}

TEST_CASE("wide/narrow conversion round-trips ASCII on every platform",
          "[platform][encoding][L3-CPP-009]") {
    const std::string original("C:/logs/flight-01.mie");
    CHECK(plat::from_wide(plat::to_wide(original)) == original);
    CHECK(plat::to_wide(std::string()).empty());
    CHECK(plat::from_wide(std::wstring()).empty());
}

#if defined(_WIN32)
TEST_CASE("wide/narrow conversion round-trips non-ASCII UTF-8 on Windows",
          "[platform][encoding][L3-CPP-009]") {
    // Windows is a shipping target, so a non-ASCII path has to survive the trip
    // through the W entry points rather than being mangled by whatever the
    // active ANSI codepage happens to be. The POSIX backend does not convert at
    // all -- POSIX paths are opaque bytes -- so this property is Windows-only.
    const std::string original("C:/vuelos/a\xC3\xB1o-2026/registro.mie");
    const std::wstring wide = plat::to_wide(original);
    CHECK_FALSE(wide.empty());
    CHECK(plat::from_wide(wide) == original);
}
#endif

// ---------------------------------------------------------------------------
// Exhaustive path-helper properties
// ---------------------------------------------------------------------------
//
// The path helpers are pure string manipulation, so their input space can be
// swept properly rather than sampled. They are also the piece most likely to
// behave differently between platforms -- a backslash is a separator on Windows
// and an ordinary filename character on POSIX -- so the properties below are
// written to hold on both, with the platform-specific parts guarded.

TEST_CASE("parent and filename always partition a path", "[platform][path][exhaustive]") {
    // For any path, filename() is the tail after the last separator and
    // parent() is what precedes it. Reassembling them must recover the original
    // except for the separator itself -- which is the property a caller relies
    // on when it splits a destination to place a temp file beside it.
    const char* paths[] = {
        "",     "a",          "a.csv", "/",      "/a",
        "/a/b", "/a/b/c.csv", "a/b",   "a/b/",   "//a",
        "a//b", "./a",        "../a",  "/a/b/.", "/very/deep/path/with/many/segments/out.csv"};

    for (std::size_t i = 0; i < sizeof(paths) / sizeof(paths[0]); ++i) {
        const std::string path(paths[i]);
        const std::string parent = plat::path_parent(path);
        const std::string name = plat::path_filename(path);

        INFO("path = '" << path << "' parent = '" << parent << "' name = '" << name << "'");

        // The filename never contains a separator: that is what makes it a name.
        for (std::size_t k = 0; k < name.size(); ++k) {
            REQUIRE_FALSE(plat::is_separator(name[k]));
        }
        // The name is always a suffix of the path.
        REQUIRE(path.size() >= name.size());
        REQUIRE(path.compare(path.size() - name.size(), name.size(), name) == 0);
        // Everything before the name is the parent plus at most one separator.
        const std::size_t head = path.size() - name.size();
        REQUIRE(head >= parent.size());
        REQUIRE(head - parent.size() <= 1);
    }
}

TEST_CASE("joining a parent to a filename never doubles or loses a separator",
          "[platform][path][exhaustive]") {
    // Every combination of trailing separator on the directory and leading
    // separator on the name -- the four shapes a caller can produce by
    // concatenating configured strings.
    const char* dirs[] = {"dir", "dir/", "/abs", "/abs/", "/", ""};
    const char* names[] = {"out.csv", "/out.csv", ""};

    for (std::size_t d = 0; d < sizeof(dirs) / sizeof(dirs[0]); ++d) {
        for (std::size_t n = 0; n < sizeof(names) / sizeof(names[0]); ++n) {
            const std::string joined = plat::path_join(dirs[d], names[n]);
            INFO("dir = '" << dirs[d] << "' name = '" << names[n] << "' -> '" << joined << "'");

            // No doubled forward slash is ever introduced. A doubled separator
            // is harmless to open() but makes a path that no longer matches the
            // one an operator typed, which matters for the collision check.
            REQUIRE(joined.find("//") == std::string::npos);

            const std::string name(names[n]);
            if (!name.empty()) {
                // The name survives intact at the tail.
                const std::string bare = plat::is_separator(name[0]) ? name.substr(1) : name;
                REQUIRE(joined.size() >= bare.size());
                REQUIRE(joined.compare(joined.size() - bare.size(), bare.size(), bare) == 0);
            }
        }
    }
}

TEST_CASE("every byte value is classified as a separator consistently",
          "[platform][path][exhaustive]") {
    for (int i = 0; i < 256; ++i) {
        const char c = static_cast<char>(i);
        const bool sep = plat::is_separator(c);
#if defined(_WIN32)
        REQUIRE(sep == (c == '/' || c == '\\'));
#else
        // On POSIX a backslash is an ordinary filename character. Treating it
        // as a separator would truncate any path that legitimately contains
        // one -- a bug that cannot reproduce on Windows.
        REQUIRE(sep == (c == '/'));
#endif
    }
}

TEST_CASE("a filename with no separator is its own path", "[platform][path][exhaustive]") {
    // Sweeps every single-character filename, including the ones that are
    // separators on the other platform.
    for (int i = 1; i < 256; ++i) {
        const char c = static_cast<char>(i);
        if (plat::is_separator(c)) {
            continue;
        }
        const std::string name(1, c);
        INFO("byte = " << i);
        REQUIRE(plat::path_filename(name) == name);
        REQUIRE(plat::path_parent(name).empty());
    }
}

TEST_CASE("temp names stay unique across many consecutive calls",
          "[platform][atomic][exhaustive][L3-WRT-001]") {
    // The uniqueness guarantee is what stops two writers clobbering each other.
    // A thousand calls in a tight loop is the case a coarse clock breaks: if
    // the name depended on the timestamp alone, many of these would collide.
    const std::string destination = plat::path_join("/some/dir", "out.csv");
    std::vector<std::string> names;
    names.reserve(1000);
    for (int i = 0; i < 1000; ++i) {
        names.push_back(plat::make_temp_name(destination));
    }

    std::sort(names.begin(), names.end());
    CHECK(std::adjacent_find(names.begin(), names.end()) == names.end());

    // Every one of them is still beside the destination and still recognisable.
    for (std::size_t i = 0; i < names.size(); ++i) {
        REQUIRE(plat::path_parent(names[i]) == plat::path_parent(destination));
        REQUIRE(plat::path_filename(names[i]).find("out.csv.mie-decoder.tmp.") == 0);
    }
}

TEST_CASE("wide and narrow conversion round-trips every ASCII string",
          "[platform][encoding][exhaustive]") {
    // Every single-character ASCII string, plus the empty one. On POSIX the
    // conversion is a byte pass-through and this proves it loses nothing; on
    // Windows it is a real UTF-8/UTF-16 conversion and this proves the same.
    CHECK(plat::from_wide(plat::to_wide(std::string())).empty());
    for (int i = 1; i < 128; ++i) {
        const std::string s(1, static_cast<char>(i));
        if (plat::from_wide(plat::to_wide(s)) != s) {
            INFO("byte = " << i);
            REQUIRE(plat::from_wide(plat::to_wide(s)) == s);
        }
    }
    SUCCEED("every ASCII character survives the wide round-trip");
}
