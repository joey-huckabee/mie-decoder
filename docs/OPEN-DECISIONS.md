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

## 1. `dump` report characters diverge between Rust and Python

**Status:** blocking a conformance gate on `dump` output.

The three implementations' `dump` reports are not byte-identical, and nothing
gates them. Found while adding a `dump` mode to the conformance runner during
the C++ port.

| Element | Rust and C++ | Python |
|---|---|---|
| Section rule | `-` × 72 | `─` (U+2500) × 72 |
| Annotation arrow | `->` | `→` (U+2192) |
| Type label | `BC->RT (Receive)` | `BC→RT (Receive)` |

`rust/src/dump.rs`'s own header comment claims it "mirrors the Python `dump.py`
output format closely enough for diffing", which is false on every one of those
lines — so this is drift, not an intended difference, and one side has to move.

**Recommendation: align Python to ASCII.** `dump` is the tool of last resort,
reached for on consoles that may not be UTF-8 — the SLES 12 SP5 default console
and Windows `cmd` at a legacy codepage both qualify. The C++ tree is held
locale-free by rule (`scripts/assert-locale-free.sh`) precisely because
environment-dependent rendering is a real hazard in this program. Two of three
already emit ASCII.

**Against:** the box-drawing rule reads better where it renders, and changing
Python alters a shipped implementation's visible output.

**Once decided:** change whichever side moves; re-add `"dump"` to
`ALLOWED_MODES` in `tests/conformance/run.py` and route it through the existing
stdout-comparison path (about five lines — `count` already uses it); add dump
cases to `manifest.json` and generate oracles with `--update-expected`, which
refuses to write unless all three agree. Full detail in `ROADMAP.md`.

---

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

## 3. Differential config-parser checks are Rust-vs-Python only

**Status:** the work is scoped and ready; only the *shape* needs a call.

`config_parity.py`, `config_fuzz.py` and `config_path_parity.py` compare two
parsers' accept/reject behaviour against each other. The C++ TOML and config
parsers are held to the same grammar only by their own unit tests
(`test_toml.cpp`, `test_config.cpp`), which cannot catch a divergence *from the
other two* — exactly the class of bug the fuzzer exists to find, and exactly how
the `?`-versus-byte glob difference and the `dump` character drift were missed
until something compared implementations directly.

The question is what "agreement" means with three parsers:

- **All-pairs.** Any two disagreeing fails. Strictest, and the most likely to
  produce a failure that is really "Python's `tomllib` is stricter than a
  hand-rolled parser can practically be".
- **Majority.** Two out of three define correct. Tempting, and wrong: it would
  let a genuine two-way bug outvote the correct implementation.
- **Reference implementation.** Nominate one (Rust) as the oracle and hold the
  others to it. Clearest to reason about; makes the reference's quirks
  normative.

**Recommendation: all-pairs**, on the grounds that a divergence between any two
implementations is a bug regardless of which is right, and the runner should say
which pair disagreed rather than silently pick a winner.

---

## 4. Coverage and fuzz gates for the C++ tree

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

## 5. SonarCloud: `main` is red, and C++ analysis is still deferred

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

## 6. Phase 3: release artifacts and the versioning scheme

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

## 7. Remaining test-fixture duplication (cheap, no judgement needed)

`cpp/tests/record_fixtures.hpp` now holds the on-the-wire record builders, and
`test_cli.cpp` and `test_merge.cpp` use it. `test_reader.cpp`, `test_sync.cpp`
and `test_decode.cpp` still carry their own copies plus the specialised builders
their cases need.

Listed here only so it is not forgotten; it needs no decision, just a quiet PR.
