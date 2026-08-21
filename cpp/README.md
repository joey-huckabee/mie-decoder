# mie-decoder — C++ implementation

The third implementation of the MIE decoder, alongside the Rust crate in
`../rust/` and the Python package in `../python/`. It exists so that
**SLES 12 SP5** is a first-class deployment target rather than a documented
exception: it compiles with the platform's own GCC 4.8.5, ships as a single
binary, and needs no interpreter or toolchain on the host (ADR-0001).

The same source builds with MSVC and Windows is a **shipping target**, not a
development convenience (ADR-0003).

> **Status: Phase 1 complete.** `decode` and `count` work on a single input,
> and the binary is held to the **same byte-exact CSV oracles** as the Rust and
> Python implementations — every shared conformance case that does not need the
> unported merge.
>
> **Not yet ported:** the multi-file merge (`--manifest`, `--glob`, several
> positional inputs, `--delta-scope`, `--collapse-duplicates`,
> `--collapse-window-us`) and `dump`. Those flags are **refused with a message
> saying the module is missing**, not reported as unknown flags, and the
> conformance runner skips their cases by name. Use the Rust or Python
> implementation for a merge.
>
> Ported and gated: `platform`, `text`, `optional`, `models`, `error`, `decode`,
> `sync`, `log`, `delta`, `reader`, `writer`, `toml`, `config`, `filter`,
> `order`, `cli`. Phase 2 adds `merge`, `dump`, and the remaining flags — and
> with them the CLI-surface-parity gate, which this build is deliberately
> excluded from while its flag set is a subset. See `../CHANGELOG.md` for what
> each landing covered and `../docs/adr/` for the decisions that shape the
> tree.

## Build & test

### Linux (authoritative — see ADR-0002)

```bash
make check                # build and run the suite, -Werror
make check SANITIZE=1     # AddressSanitizer + UBSan + LeakSanitizer
make check-valgrind       # memcheck, fails on any leak or invalid access
make check-gcc48          # the SLES 12 fidelity tier, in a container
make tidy                 # clang-tidy (needs compile_commands.json, see below)
make format-check         # clang-format, non-mutating
make versions             # print the resolved toolchain and build directory
make clean                # remove only this toolchain's artifacts
make clean-all            # remove every toolchain's artifacts
```

Flags are `-std=c++11 -Wall -Wextra -Werror`. Vendored headers come in via
`-isystem third_party` so they cannot trip `-Werror`.

Build output goes to `build/<machine>-<version>-<tier>`, keyed on the compiler
that produced it, so the modern, `gcc:4.8` and sanitizer tiers cannot overwrite
one another. Overwrite with `make BUILD_DIR=/somewhere/else` when needed.

`make tidy` needs a compilation database:

```bash
bear -- make -j"$(nproc)" all      # writes compile_commands.json
make tidy
```

### Windows / Visual Studio

**Visual Studio 2022 → File → Open → Folder → `cpp\`.** VS configures CMake,
wires up IntelliSense and the debugger, and picks the test target up in the Test
Explorer. No `.sln` or `.vcxproj` is checked in — hand-maintained project XML is
exactly the drift the shared source list exists to avoid.

From a command line:

```powershell
cmake -B build-msvc -S . -G "Visual Studio 17 2022" -A x64
cmake --build build-msvc --config Debug
ctest --test-dir build-msvc -C Debug --output-on-failure
```

Flags are `/std:c++14 /W4 /WX /permissive- /utf-8`. MSVC has no `/std:c++11` —
its floor is C++14 — so C++11-conformant source is compiled as C++14, which is
valid. **The compiler that actually enforces the C++11 boundary is GCC 4.8.5 in
the fidelity tier, not this one.**

### WSL2

Build on the **Linux-native filesystem** (`~/src/...`), not `/mnt/c`. Two
reasons, and the second is not obvious:

1. Cross-filesystem builds over DrvFs are slow and produce spurious rebuilds
   from mtime granularity.
2. **Rootless podman cannot bind-mount a `/mnt/c` path.** The container starts,
   the mount is silently empty, and `g++` reports "no input files" as though the
   source had vanished — so `make check-gcc48` fails in a way that looks like a
   code problem. Run it from a native path.

## Layout

```
include/mie/       public headers
src/               implementation
tests/             Catch2 v2 test suite
third_party/       vendored pinned single headers (see VENDORED.md)
LICENSES/          license texts for vendored dependencies
sources.txt        THE library source list, read by both builds
Makefile           authoritative on Linux
CMakeLists.txt     authoritative on Windows
```

Requirements and decisions live at the repository root, shared with the Rust and
Python implementations:

```
../docs/L1-REQ.md    system requirements
../docs/L2-REQ.md    architectural derivations
../docs/L3-REQ.md    implementation obligations (L3-CPP-* for this tree)
../docs/adr/         MADR-format architecture decision records
```

## Design rules

- **No runtime dependencies.** Argument parsing, CSV, TOML, logging and error
  types are hand-rolled, matching the Rust crate's single-dependency discipline.
  Catch2 is test-only and vendored (`VENDORED.md`).
- **The platform layer is the only thing that touches the OS.** Five concerns
  live behind `include/mie/platform.hpp` — mapping the input, atomic output,
  directory enumeration, binary stdout, path identity — and nothing else may
  include `<windows.h>` or `<sys/mman.h>`. Enforced by
  `../scripts/assert-platform-confined.sh`.
- **Never locale-sensitive.** `DELTA` is formatted `%.6f`, whose decimal
  separator is locale-dependent; the parsers classify characters with explicit
  ASCII ranges rather than `<cctype>`. Enforced by
  `../scripts/assert-locale-free.sh`.
- **RAII only**, no naked `new`/`delete`.
- **C++11, not C++14.** Two constructs MSVC accepts and GCC 4.8.5 rejects are
  banned by convention, because they produce "green on Windows, red on Linux":
  1. aggregate-initialising a class that has a default member initialiser
  2. passing a `const_iterator` to a container mutator

## CI tiers

| Tier | What it proves |
|---|---|
| modern g++ | fast feedback, plus the sanitizers and fuzzing GCC 4.8 cannot host |
| `gcc:4.8` container | C++11 conformance on the SLES 12 system compiler — **runs the full suite, not just a compile** |
| MSVC on `windows-2022` | the shipping Windows artifact, at `/W4 /WX /permissive-` |
| real SLES 12 SP5 | deployability. Not in CI — verified by hand on hardware |

`gcc:4.8` is a *proxy* for the target, not the target: Debian 7 "wheezy" with
glibc 2.13 against SUSE's 2.22. Older, not merely different — which makes it a
conservative floor rather than an approximation.
