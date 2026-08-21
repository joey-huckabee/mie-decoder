// SPDX-License-Identifier: Apache-2.0
//
// The executable entry point, and nothing else.
//
// Everything the CLI does lives in `mie/cli.hpp`, which takes a vector of
// arguments rather than `(argc, argv)`. That is what makes the exit-code
// contract testable in-process: the Catch2 suite links the library, so a test
// that had to spawn this binary to check an exit code would be a subprocess per
// case and could not run at all under the sanitizer or GCC 4.8.5 tiers.

#include <string>
#include <vector>

#include "mie/cli.hpp"
#include "mie/platform.hpp"

int main(int argc, char** argv) {
    // First, before anything writes a byte. On Windows the CRT otherwise
    // rewrites every newline into CRLF on the way out, which breaks byte-exact
    // output on that platform alone (L2-WRT-012).
    mie::platform::set_stdout_binary();

    std::vector<std::string> args;
    if (argc > 1) {
        args.reserve(static_cast<std::size_t>(argc - 1));
        for (int i = 1; i < argc; ++i) {
            args.push_back(std::string(argv[i]));
        }
    }
    return mie::cli::run(args);
}
