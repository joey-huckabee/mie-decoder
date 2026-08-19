// SPDX-License-Identifier: Apache-2.0
//
// Entry point for the mie-decoder C++ CLI.
//
// PHASE 0 SCOPE. This currently implements --version and --help only. The
// decode / count / dump subcommands land in Phase 1 and Phase 2 as the
// corresponding modules are ported; see docs/adr/0001 and the plan this tree
// was built from. The file exists this early so that the toolchain, both build
// descriptions, and every CI tier are proven end-to-end on a real linked
// executable before any porting effort is spent on them.
//
// Output goes through std::fwrite / std::fputs rather than <iostream>. Two
// reasons, both of which outlive Phase 0: the stream layer applies locale
// facets that would put a thousands separator or a comma decimal point into
// CSV output on a non-C locale host, and stdout has to be put into binary mode
// on Windows anyway, which mixes badly with an already-buffered std::cout.

#include "mie/platform.hpp"

#include <cstdio>
#include <cstring>
#include <string>

namespace {

// Kept in step with the joint repository tag by hand for now. It moves into a
// generated header when the C++ tree starts cutting its own cpp-vX.Y.Z tags
// (Phase 3); hard-coding it before then is honest about what the number is.
const char kVersion[] = "2.12.0";

const char kProgram[] = "mie-decoder";

// Exit codes are shared across all three implementations and are pinned by
// docs/ERROR-CATALOG.md. Only the two Phase 0 can reach are named here; the
// rest arrive with the code paths that produce them.
const int kExitSuccess = 0;
const int kExitError = 1;
const int kExitUsage = 4;

/// Write `text` and report whether it reached the stream.
///
/// The return value is checked at every call site rather than discarded. That
/// is not pedantry about a help message: `mie-decoder --help | head -1` closes
/// the pipe while this is still writing, and a program that ignores the failure
/// exits 0 having produced nothing. Reporting it is what makes the exit code
/// mean something in a shell pipeline.
bool emit(std::FILE* stream, const char* text) { return std::fputs(text, stream) >= 0; }

bool print_version() {
    // Format is pinned to the other two implementations: "mie-decoder 2.12.0".
    // A conformance case compares this string across implementations, so the
    // spacing is contract, not cosmetics.
    return emit(stdout, kProgram) && emit(stdout, " ") && emit(stdout, kVersion) &&
           emit(stdout, "\n");
}

bool print_help() {
    return emit(stdout,
                "mie-decoder \xE2\x80\x94 DDC MIL-STD-1553 MIE binary decoder\n"
                "\n"
                "USAGE:\n"
                "  mie-decoder [--log-level L] [--config PATH] <command> [options]\n"
                "\n"
                "COMMANDS:\n"
                "  decode <INPUT>... Decode MIE file(s) to CSV (2+ inputs "
                "\xE2\x86\x92 time-sorted merge)\n"
                "  count  <INPUT>    Print message count (no CSV)\n"
                "  dump   <INPUT>    Hex dump (raw or record-aware)\n"
                "\n"
                "GLOBAL OPTIONS:\n"
                "  --log-level LEVEL                     DEBUG|INFO|WARNING|WARN|ERROR|\n"
                "                                        CRITICAL|OFF (default WARNING;\n"
                "                                        case-insensitive; CRITICAL/OFF silence)\n"
                "  --config PATH                         TOML configuration file\n"
                "  -V, -v, --version                     Print version and exit\n"
                "  -h, --help                            Print this help and exit\n"
                "\n"
                "NOTE: this build implements --version and --help only. The decode,\n"
                "count and dump subcommands are not yet ported to C++; use the Rust or\n"
                "Python implementation for them.\n");
}

/// Case-insensitive ASCII comparison.
///
/// Hand-rolled with explicit ranges rather than using tolower(), which is
/// locale-sensitive: under tr_TR the uppercase of 'i' is not 'I', so a
/// locale-aware comparison would fail to recognise --VERSION on a Turkish host.
/// The same rule governs every parser in this tree and is checked by the
/// locale-free CI gate.
bool equals_ignoring_ascii_case(const char* a, const char* b) {
    while (*a != '\0' && *b != '\0') {
        char ca = *a;
        char cb = *b;
        if (ca >= 'A' && ca <= 'Z') {
            ca = static_cast<char>(ca - 'A' + 'a');
        }
        if (cb >= 'A' && cb <= 'Z') {
            cb = static_cast<char>(cb - 'A' + 'a');
        }
        if (ca != cb) {
            return false;
        }
        ++a;
        ++b;
    }
    return *a == *b;
}

}  // namespace

int main(int argc, char** argv) {
    // First, before anything writes a byte. On Windows the CRT otherwise
    // rewrites every newline into CRLF on the way out, which breaks byte-exact
    // output on that platform alone.
    mie::platform::set_stdout_binary();

    // --version and --help are answered before any other validation, matching
    // the other two implementations: asking a broken invocation for help must
    // produce help, not a usage error about the invocation.
    for (int i = 1; i < argc; ++i) {
        const char* arg = argv[i];
        if (equals_ignoring_ascii_case(arg, "--version") || std::strcmp(arg, "-V") == 0 ||
            std::strcmp(arg, "-v") == 0) {
            return print_version() ? kExitSuccess : kExitError;
        }
        if (equals_ignoring_ascii_case(arg, "--help") || std::strcmp(arg, "-h") == 0) {
            return print_help() ? kExitSuccess : kExitError;
        }
    }

    if (argc <= 1) {
        // Bare invocation prints help but still exits non-zero: a script that
        // calls the tool with no arguments has a bug, and exiting 0 would hide
        // it. A write failure does not change that verdict -- the usage error
        // is the more specific fact, so it wins.
        (void)print_help();
        return kExitUsage;
    }

    (void)emit(stderr, "error: this build implements --version and --help only\n");
    return kExitUsage;
}
