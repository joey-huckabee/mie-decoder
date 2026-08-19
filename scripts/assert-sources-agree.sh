#!/usr/bin/env bash
# Assert that the Makefile and CMakeLists.txt resolve the SAME library sources.
#
# Both read cpp/sources.txt, so in principle they cannot disagree. In practice
# they parse it with different tools -- a sed pipeline on one side, a CMake
# regex loop on the other -- and the failure mode of a subtle parsing
# difference is severe and quiet: a translation unit compiled into the Linux
# binary and missing from the Windows one, surfacing as a link error on one
# platform that reads like a toolchain problem rather than a bookkeeping one.
#
# This gate costs a second and closes that gap. It compares what each build
# actually resolves, not what the file appears to say -- which is the only
# comparison that would have caught a comment-stripping divergence.
#
# Run standalone, or via scripts/repo-hygiene.sh. Requires make; skips the CMake
# half (with a warning, not a failure) when cmake is unavailable, so a host
# without it can still run the rest of repo hygiene.
#
# Exit 0 when they agree, 1 when they do not.

set -uo pipefail

CPP_DIR="${1:-cpp}"

if [ ! -f "$CPP_DIR/sources.txt" ]; then
    echo "assert-sources-agree: no such file: $CPP_DIR/sources.txt" >&2
    exit 1
fi

if ! command -v make >/dev/null 2>&1; then
    echo "assert-sources-agree: make not found; cannot compare" >&2
    exit 1
fi

# The Makefile's own view, printed by a target rather than re-derived here.
# Re-implementing the sed pipeline in this script would make the gate agree with
# a copy of the build rather than with the build.
make_view=$(make -C "$CPP_DIR" -s print-lib-sources 2>/dev/null | tr ' ' '\n' | grep -v '^$' | sort)

if [ -z "$make_view" ]; then
    echo "assert-sources-agree: FAIL the Makefile resolved no library sources" >&2
    exit 1
fi

# The CMake view, obtained the same way -- by asking CMake, in a throwaway
# project that reuses the real parsing loop is not possible without duplicating
# it, so instead the real CMakeLists is configured and asked to report.
if ! command -v cmake >/dev/null 2>&1; then
    echo "assert-sources-agree: WARN cmake not found; checked the make side only" >&2
    echo "$make_view" | sed 's/^/    /'
    exit 0
fi

scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT

if ! cmake -B "$scratch" -S "$CPP_DIR" -DMIE_BUILD_TESTS=OFF >"$scratch/configure.log" 2>&1; then
    echo "assert-sources-agree: FAIL cmake configure failed" >&2
    tail -20 "$scratch/configure.log" >&2
    exit 1
fi

# MIE_LIB_SOURCES is written to the cache by CMakeLists.txt for this gate.
cmake_view=$(cmake -B "$scratch" -S "$CPP_DIR" -LA 2>/dev/null \
             | grep '^MIE_RESOLVED_SOURCES:' \
             | sed 's/^MIE_RESOLVED_SOURCES:[^=]*=//' \
             | tr ';' '\n' | grep -v '^$' | sort)

if [ -z "$cmake_view" ]; then
    echo "assert-sources-agree: FAIL cmake reported no library sources" >&2
    exit 1
fi

# The platform backend is chosen by the host, so the two views legitimately
# differ on exactly that one entry when this runs on a machine whose CMake picks
# a different backend from the Makefile's hard-coded POSIX one. Compare the
# portable set and check the backend separately.
make_portable=$(echo "$make_view" | grep -v 'platform_posix\.cpp\|platform_win32\.cpp')
cmake_portable=$(echo "$cmake_view" | grep -v 'platform_posix\.cpp\|platform_win32\.cpp')

if [ "$make_portable" != "$cmake_portable" ]; then
    echo "assert-sources-agree: FAIL the two builds resolve different sources" >&2
    echo "  only make:" >&2
    comm -23 <(echo "$make_portable") <(echo "$cmake_portable") | sed 's/^/    /' >&2
    echo "  only cmake:" >&2
    comm -13 <(echo "$make_portable") <(echo "$cmake_portable") | sed 's/^/    /' >&2
    exit 1
fi

backends=$(echo "$cmake_view" | grep -c 'platform_posix\.cpp\|platform_win32\.cpp')
if [ "$backends" -ne 1 ]; then
    echo "assert-sources-agree: FAIL cmake selected $backends platform backends, expected exactly 1" >&2
    exit 1
fi

count=$(echo "$make_portable" | wc -l | tr -d ' ')
echo "assert-sources-agree: OK (both builds resolve the same $count portable sources + 1 backend)"
exit 0
