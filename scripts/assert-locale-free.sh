#!/usr/bin/env bash
# Assert that the C++ implementation never becomes locale-sensitive.
#
# WHY THIS GATE EXISTS
#
# The DELTA column is formatted to six decimal places -- "{:.6}" in Rust,
# f"{d:.6f}" in Python, "%.6f" here. printf's decimal separator is chosen by the
# active LC_NUMERIC locale, so on a host configured for de_DE or fr_FR a C++
# build that has called setlocale emits "1,234500" where the other two
# implementations emit "1.234500". Every conformance oracle then fails, on that
# host only, with a diff that looks like a decoding bug.
#
# The same hazard runs through the parsers. tolower()/toupper()/isdigit() are
# locale-sensitive: under tr_TR the lowercase of 'I' is a dotless i, so a
# case-insensitive comparison of "--VERSION" or a TOML key stops matching. The
# hand-rolled TOML and CLI parsers must classify characters with explicit ASCII
# ranges instead.
#
# The rule is therefore: this program never calls setlocale, and never uses the
# <cctype> classification functions. Both halves are checked here.
#
# Run standalone, or via scripts/repo-hygiene.sh.
#
# Exit 0 when clean, 1 when a violation is found.

set -uo pipefail

CPP_DIR="${1:-cpp}"

if [ ! -d "$CPP_DIR" ]; then
    echo "assert-locale-free: no such directory: $CPP_DIR" >&2
    exit 1
fi

violations=0

check_pattern() {
    local pattern="$1"
    local explanation="$2"
    local hits
    # Strip line comments before matching, so prose that NAMES a banned call in
    # order to explain why it is banned does not trip the gate enforcing it.
    # Whole-line block comments are handled by the leading-* filter.
    while IFS= read -r file; do
        hits=$(sed -e 's://.*::' "$file" \
               | grep -nE "^[[:space:]]*[^*]*$pattern" \
               | grep -vE '^[0-9]+:[[:space:]]*\*' || true)
        if [ -n "$hits" ]; then
            echo "assert-locale-free: FAIL $file -- $explanation" >&2
            echo "$hits" | sed 's/^/    /' >&2
            violations=$((violations + 1))
        fi
    done < <(find "$CPP_DIR/src" "$CPP_DIR/include" -type f \( -name '*.cpp' -o -name '*.hpp' \) \
                 2>/dev/null | sort)
}

# cpp/tests is DELIBERATELY not scanned. The suite has to call setlocale in
# order to prove the property this gate protects: one test switches LC_NUMERIC
# to a comma-separator locale, formats a DELTA value, and asserts a dot comes
# back. Scanning the tests would make that test unwritable, leaving the
# guarantee asserted by a grep and demonstrated by nothing.
#
# The tests are also not shipped, so a locale call there cannot reach an
# operator's CSV.

# setlocale in any form. The program must run in the "C" locale it starts in.
check_pattern 'setlocale[[:space:]]*\(' \
    'calls setlocale; the decoder must stay in the C locale so %.6f emits a dot'

# <cctype> classification. std::isdigit and friends read the locale table.
check_pattern '\b(std::)?(isalpha|isdigit|isalnum|isspace|isupper|islower|toupper|tolower|isxdigit|ispunct)[[:space:]]*\(' \
    'uses a locale-sensitive <cctype> function; classify with explicit ASCII ranges instead'

if [ "$violations" -ne 0 ]; then
    echo "" >&2
    echo "Replace locale-sensitive calls with explicit ASCII range tests, e.g." >&2
    echo "    c >= '0' && c <= '9'          instead of isdigit(c)" >&2
    echo "    c >= 'A' && c <= 'Z'          instead of isupper(c)" >&2
    echo "This is the same rule the Rust and Python implementations follow by" >&2
    echo "construction; C++ is the only one of the three where it has to be enforced." >&2
    exit 1
fi

echo "assert-locale-free: OK (no setlocale, no locale-sensitive character classification)"
exit 0
