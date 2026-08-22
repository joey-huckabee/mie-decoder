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

## 1. `-o -` means three different things

**Status:** a divergence with no gate; found while closing the `dump` character
question.

| | `decode … -o -` | `decode …` (no `-o`) |
|---|---|---|
| Rust | creates a **file named `-`** | stdout |
| Python | creates a **file named `-`** | stdout |
| C++ | **stdout** | stdout |

The C++ behaviour is mine, from the CLI port: its `--help` says
`'-' writes to stdout`, and it does. Rust and Python take `-` as an ordinary
filename. The surface-parity gate compares flag *names*, not semantics, so
nothing caught it.

Both readings are defensible. `-` for stdout is a strong Unix convention, and
`-o -` is more explicit than omitting the flag. Against it: all three already
write to stdout when `-o` is omitted, so `-` adds nothing but a special case —
and a special case that silently changes the meaning of a path is exactly the
kind of thing that surprises someone whose file really is called `-`.

**Recommendation: drop it from C++**, matching the shipped behaviour of the
other two. Adding it to all three would be adding a special case for a
capability that already exists; removing it from one restores parity and loses
nothing. Either way it should be decided rather than left as an accident of
which implementation was written last.

## 2. Should a merge be allowed to overwrite a *non-input* file without `--no-clobber`?

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

## 3. Coverage and fuzz gates for the C++ tree

**Status:** unbuilt; needs a threshold, not a design.

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

---

## 4. SonarCloud: `main` is red, and C++ analysis is still deferred

**Status:** pre-existing; deferred deliberately.

`main` ships red on SonarCloud (S2083 / S8707 on `reader.py`'s `open()`), which
is roadmapped rather than suppressed. C++ analysis (build-wrapper based) was
deferred by the delivery plan on the grounds that adding a language to a red
gate obscures both problems.

**Needs deciding:** whether to resolve the Python findings first and then add
C++, or add C++ analysis now and accept two red sources at once.

**Recommendation:** resolve the Python findings first. The reason for deferring
has not changed, and the C++ tree already carries clang-tidy, cppcheck, ASan,
UBSan, LSan and Valgrind — so the marginal analysis value of adding Sonar now is
lower than the cost of a gate nobody can read.

---

## 5. Phase 3: release artifacts and the versioning scheme

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
