---
status: accepted
date: 2026-08-18
decision-makers: Joey
---

# GNU make is authoritative on Linux; CMake drives the MSVC build

## Context and Problem Statement

The C++ implementation must build in two very different places:

1. **SLES 12 SP5 and the `gcc:4.8` fidelity container**, where ADR-0001 puts the
   floor at GCC 4.8.5.
2. **Visual Studio 2022 on Windows**, where the build has to be something a
   developer can open, debug and step through — and which produces a shipping
   artifact (ADR-0003).

One build description for both would be the obvious preference. The question is
whether it is achievable.

## Decision Drivers

* The fidelity tier must be able to build; it is the only thing that proves
  C++11 conformance on the target compiler
* Visual Studio integration should be real — IntelliSense, breakpoints, the Test
  Explorer — not "run this shell script and attach"
* Two file lists is the drift hazard, not two build *files*
* The sibling `background-file-mover` repository's Makefile already encodes the
  tier structure, the sanitizer switching and the toolchain-keyed build
  directory; reusing it is worth real money

## Considered Options

* **CMake everywhere**, dropping the Makefile
* **make everywhere**, with a hand-maintained `.sln` / `.vcxproj` for Visual
  Studio
* **make on Linux + CMake on Windows**, both reading one shared source list

## Decision Outcome

Chosen option: **make on Linux, CMake on Windows, one shared `cpp/sources.txt`.**

### Why not CMake everywhere

The `gcc:4.8` image is Debian 7 "wheezy" and ships **CMake 2.8**. A modern
`CMakeLists.txt` cannot be read by it at all — `cmake_minimum_required(VERSION
3.15)` fails outright. Making CMake universal therefore means publishing and
maintaining a *new* GCC 4.8.5 image with a backported CMake, which trades one
maintained artifact for another and puts a hand-built tool inside the tier whose
entire job is to be a faithful reproduction of the target.

It would also mean re-expressing every gate — the sanitizer tiers, the
toolchain-keyed build directory, coverage, tidy, the fidelity-container
invocation — as CMake presets, discarding a Makefile that already works.

### Why not a checked-in `.vcxproj`

MSBuild project XML is hand-maintained and drifts on every file added, and the
symptom is a link error on Windows only. It also gives up nothing in return:
Visual Studio 2022 opens a CMake folder natively with full IntelliSense and
debugger support, so the `.sln` buys no integration that CMake does not already
provide.

### How the drift is actually closed

The hazard was never "two build files" — it is **two file lists**. So there is
one: `cpp/sources.txt`, a newline-delimited list read by both builds.

- `cpp/Makefile` reads it with a `sed` pipeline inside `$(shell …)`.
- `cpp/CMakeLists.txt` reads it with `file(STRINGS …)` and a regex loop.
- `scripts/assert-sources-agree.sh` configures both and compares what each one
  **actually resolved** — not what the file appears to say. That distinction is
  the point: the two parse the same text with different tools, and a
  comment-stripping difference is exactly the kind of quiet divergence that
  produces a module present in one binary and missing from the other.

The platform backends (`platform_posix.cpp`, `platform_win32.cpp`) are
deliberately **not** in `sources.txt`: everything named there is compiled
unconditionally, and those two are mutually exclusive. Each carries an `#error`
guard so a mis-selection is one clear message rather than a cascade of
missing-symbol noise.

### Consequences

* Good: the fidelity tier keeps working with the tools the container has.
* Good: `File → Open → Folder → cpp\` in VS 2022 just works.
* Good: the Makefile's tier structure carries over from a repository where it is
  already proven.
* Bad: two build descriptions to keep in step for *flags*, not for files. The
  warning settings, the C++ standard and the preprocessor defines are stated
  twice and are not gate-checked. Changing one without the other is the live
  hazard this decision leaves open — mitigated only by both builds being run in
  CI at `-Werror` / `/WX`.
* Bad: a contributor has to know which build is authoritative where. Stated at
  the top of both files.

## Verification

`scripts/assert-sources-agree.sh` runs in CI and fails if the two builds resolve
different source sets. Both builds run the full Catch2 suite in CI — `make
check` on Linux (modern and GCC 4.8.5 tiers) and `ctest` on `windows-2022` — so
a flag divergence that changes behaviour surfaces as a test failure even though
the flags themselves are not compared.
