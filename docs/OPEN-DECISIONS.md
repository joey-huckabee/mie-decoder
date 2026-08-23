# Open decisions

Questions that need a call from the maintainer before the work they gate can
land. Each entry states what was found, what the options cost, and what happens
next once it is decided — so a decision can be made from this page without
re-deriving the evidence.

This is **not** a backlog. Work that is merely queued lives in
[`ROADMAP.md`](ROADMAP.md); this file holds only items where the next step
depends on a judgement rather than on effort. Delete an entry when it is
decided, recording the outcome wherever it belongs (an ADR, a requirement, or
the changelog).

---

## 1. Should a merge be allowed to overwrite a *non-input* file without `--no-clobber`?

**Status:** not blocking; raised by the collision fix.

A merge now refuses an output that resolves to one of its own inputs, matching
Rust and Python. Separately, `--no-clobber` is off by default, so a merge will
silently overwrite an unrelated existing CSV.

That is consistent with the single-file path and with most CLI tools, and
changing it would be a breaking change to the default. But a merge is typically
a long batch job whose output is expensive to reproduce, which is the case where
a silent overwrite hurts most.

**Recommendation: leave it.** Consistency with the single-file path is worth
more than the marginal safety, and `--no-clobber` exists for operators who want
it. Recorded here because it was noticed while fixing the input-collision gap,
not because it is believed wrong.

---

## 2. Coverage and fuzz gates for the C++ tree — COVERAGE DONE, FUZZ OPEN

**Status:** the coverage gate is built and wired. The fuzz targets are still
unwritten, and that half still needs the decision below.

The delivery plan listed a gcov coverage gate and libFuzzer targets (record
decoder/sync over arbitrary bytes, the TOML config parser, CLI argv) as Phase 1
items. Neither is wired up. The Python coverage gate sits at 92%.

**Needs deciding:** the C++ coverage threshold, and whether the fuzz job runs a
committed-corpus replay only (fast, deterministic, safe for a required check) or
also a timed exploratory run (finds more, but a required job that fails on a new
finding makes an unrelated PR red).

**Recommendation:** corpus replay as a required gate, a timed run as a scheduled
job that opens an issue rather than blocking a merge. Coverage at 85% initially,
raised once the number is known rather than chosen in advance.

**Coverage, as built.** `make -C cpp coverage` gates on 90% lines and 76%
branches, wired as a required `cpp-ci.yml` job. CI measures 90.9% lines, 96.6%
functions and 81.5% branches over the Catch2 suite; the WSL2 host measures 91.1%
/ 96.6% / 76.5% for the same code, because gcov branch counts follow the
conditionals the COMPILER emits and so move with its version — the conformance
suite is deliberately excluded, as it is for Rust and Python, because it drives
each CLI out-of-process and counting it would measure a different thing in each
implementation.

Both floors sit at what the suite measures today, so nothing can regress, and
both are meant to rise as tests land. That is this entry's own recommendation
followed: a number chosen after measuring rather than before. The 85% it
originally guessed at would have been *below* the line coverage that already
existed and so would have gated nothing.

The branch figure deserves naming rather than burying: **81.5% on CI, 76.5% on
the older host — several hundred uncovered branch outcomes either way**,
concentrated in `reader.cpp`, `writer.cpp` and `merge.cpp`. The floor is pinned
to the lower figure so it holds on every supported compiler; pinning it to CI's
number would make a local run pass while CI failed. Part of that gap is metric rather than test quality — gcov counts
every conditional the compiler emits, so it reads lower than the llvm-cov
*regions* the Rust gate uses — but not all of it. Raising it is real work that
has not been done.

**Fuzz remains open.** No `cpp/fuzz/` directory, no targets, no job. The shape
question above (corpus replay as a required gate, timed run as a scheduled one)
is unchanged and still needs a call.

---

## 3. SonarCloud does not analyse the C++ tree — RESOLVED

**Status:** done. C++ is analysed by SonarCloud and by CodeQL. Kept here rather
than deleted because the reasoning below -- particularly the correction -- is
what made the decision, and the "measure before making it blocking" caveat is
still live.

**Correction.** The previous version of this entry said `main` "ships red on
SonarCloud ... roadmapped rather than suppressed". That was true when it was
first written, and had stopped being true before this file existed. The two
`reader.py` findings (`pythonsecurity:S2083`, `pythonsecurity:S8707`) are
**suppressed** — per-rule and per-file, in `.github/workflows/sonarcloud.yml`,
with the taint flow and reasoning written out beside the exclusion — and `main`
is green. `docs/ROADMAP.md` has carried the resolution the whole time. The claim
was repeated from a stale note rather than checked against the workflow file or
the run history.

**Also settled since.** The six issues SonarCloud had open — four `rust:S1612`,
one `rust:S3776`, one `python:S9073` — are fixed, and the reason `cargo clippy`
reported none of the Rust ones is understood and documented: clippy's `pedantic`
and `nursery` groups are allow-by-default, so `-D warnings` never sees them
while Sonar's rules do. See `CONTRIBUTING.md` step 11.

**What was open, and what was done.** `sonar.sources` was `rust/src,python/src`;
the C++ tree was not analysed at all. That was deferred by the delivery plan on
the grounds that "adding a language to a red gate obscures both problems" — a
premise that stopped holding once the gate went green.

Both analysers now cover it:

* **SonarCloud** — `cpp/src` and `cpp/include` are sources, `cpp/tests` are
  tests, and the CFamily analyser reads the **compilation database** that `bear`
  writes, rather than using the build-wrapper. The database is what clang-tidy
  already consumes, so Sonar sees the flags the real build uses instead of a
  second description that could drift from `sources.txt`. Vendored Catch2 and
  the per-toolchain `build/` trees are excluded.
* **CodeQL** — `c-cpp` joins `rust` and `python`, with `build-mode: manual` and
  an explicit `make -C cpp all`. Not `autobuild`: this tree has a specific build
  and guessing at it is how an extractor silently analyses less than you think.

**Needs deciding:** whether it is worth adding. C/C++ analysis needs SonarCloud's
build-wrapper around a real compile, which is a meaningful addition to CI. The
tree already carries clang-tidy 20, cppcheck, ASan, UBSan, LSan, Valgrind and a
GCC 4.8.5 full-suite tier, so the marginal defect-finding value is genuinely
lower here than it was for Python — the question is whether Sonar's taint and
security rules add something those do not.

**Measured.** The first branch analysis of `main` reported **404 findings, all
from `cpp/`**: 402 code smells, 2 vulnerabilities, 0 bugs. Rust and Python
contribute none. An earlier note here and in the CHANGELOG said "zero findings";
that came from a pull-request analysis, which only examines changed files, and no
C++ file changed in that PR.

Both vulnerabilities are suppressed with the reasoning recorded in
`.github/workflows/sonarcloud.yml`, scoped per-rule and per-file as the Python
entries are: `cpp:S2083` on `dump.cpp` (a path-injection rule matching a function
that touches no path — the tainted value is report text bound for stdout) and
`cpp:S2612` on `platform_posix.cpp` (the 0644 temp-file mode, which is stricter
than the 0666 Rust and Python request, so tightening C++ alone would diverge the
three).

The 402 code smells do not fail the gate — maintainability on new code is A — and
are being worked separately. They concentrate in a few rules; `cpp:S3230` alone
is 176 of them.

**Recommendation, as taken:** added, and now measured. The C++ tree joins an
existing `sonar.qualitygate.wait=true` job, so if the first analysis surfaces a
large pile of style findings the answer is to scope C++ to the security rules,
not to unpick the integration. The rules worth having are the security and taint
ones, which no other gate in this repository runs; clang-tidy 20, cppcheck, ASan,
UBSan, LSan and Valgrind already cover the rest.

---

## 4. Phase 3: release artifacts and the versioning scheme

**Status:** not started; needs direction before any of it is built.

`CLAUDE.md` anticipates impl-prefixed tags (`rust-vX.Y.Z`, `python-vX.Y.Z`,
`cpp-vX.Y.Z`) while every release so far has been a joint cut from one tag.

**Needs deciding:**

- Does the C++ implementation ship in the next joint cut, or take its own
  `cpp-v*` tag first?
- Linux artifact built in the `gcc:4.8` container (runs on glibc 2.13 and up,
  so SLES 12 SP5 is covered) — confirm that is the intended build host.
- Windows artifact: MSVC Release x64. Is a static CRT wanted so the binary does
  not depend on the VC++ redistributable being present?
- Real SLES 12 SP5 deployment verification is **manual and out of CI** — the
  `gcc:4.8` container is Debian 7 / glibc 2.13, a conservative proxy that proves
  C++11 conformance, not deployability. Who does that check, and against what?

---
