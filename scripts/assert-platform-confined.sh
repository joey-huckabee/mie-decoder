#!/usr/bin/env bash
# Assert that operating-system headers appear ONLY in the platform backends.
#
# The C++ implementation's whole portability argument is that the decoder is
# OS-agnostic and that the five concerns which are not -- mapping the input,
# atomic output, directory enumeration, binary stdout, path identity -- live
# behind cpp/include/mie/platform.hpp. That argument is only true while it is
# enforced: one #include <windows.h> in decode.cpp compiles fine on the machine
# that added it and turns the SLES 12 build red weeks later, at which point the
# fix is a refactor rather than an edit.
#
# Run standalone, or via scripts/repo-hygiene.sh.
#
# Exit 0 when confined, 1 when a violation is found.

set -uo pipefail

CPP_DIR="${1:-cpp}"

if [ ! -d "$CPP_DIR" ]; then
    echo "assert-platform-confined: no such directory: $CPP_DIR" >&2
    exit 1
fi

# The two files permitted to reach the OS, plus platform_common.cpp -- which is
# named to match the pattern for exactly this reason but includes nothing
# platform-specific itself.
#
# Matched on basename, not on full path, so moving the backends between
# directories does not silently disable the gate.
is_backend() {
    case "$(basename "$1")" in
        platform_posix.cpp | platform_win32.cpp) return 0 ;;
        *) return 1 ;;
    esac
}

# Headers that mean "this translation unit talks to the operating system".
#
# <unistd.h> and <fcntl.h> are here alongside the obvious ones because they are
# how a POSIX-only shortcut usually enters a shared file -- an open() or a
# close() that looks harmless and does not compile on Windows at all.
OS_HEADERS='<windows\.h>|<sys/mman\.h>|<sys/stat\.h>|<sys/types\.h>|<unistd\.h>|<dirent\.h>|<fcntl\.h>|<io\.h>|<direct\.h>|<winbase\.h>|<fileapi\.h>'

violations=0

while IFS= read -r file; do
    if is_backend "$file"; then
        continue
    fi
    # Ignore commented-out includes: prose in a header comment that NAMES
    # <windows.h> to explain why it is absent must not trip the gate that
    # enforces its absence.
    hits=$(grep -nE "^[[:space:]]*#[[:space:]]*include[[:space:]]*($OS_HEADERS)" "$file" || true)
    if [ -n "$hits" ]; then
        echo "assert-platform-confined: FAIL $file includes an OS header outside the platform backends" >&2
        echo "$hits" | sed 's/^/    /' >&2
        violations=$((violations + 1))
    fi
done < <(find "$CPP_DIR/src" "$CPP_DIR/include" "$CPP_DIR/tests" "$CPP_DIR/fuzz" \
             -type f \( -name '*.cpp' -o -name '*.hpp' \) 2>/dev/null | sort)

if [ "$violations" -ne 0 ]; then
    echo "" >&2
    echo "OS headers belong in cpp/src/platform_posix.cpp or cpp/src/platform_win32.cpp." >&2
    echo "If a new OS capability is needed, add it to mie/platform.hpp and implement" >&2
    echo "it in BOTH backends -- that is what keeps the Windows binary a shipping" >&2
    echo "artifact rather than a build that happens to link." >&2
    exit 1
fi

echo "assert-platform-confined: OK (OS headers confined to the platform backends)"
exit 0
