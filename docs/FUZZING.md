# MIE-Decoder Fuzzing

**What this page is.** The single map of *what is fuzzed*, *by which
implementation*, *under which driver*, and *what is deliberately not fuzzed yet*.
Read it before adding a fuzz harness, before interpreting a burn-in log, and
before concluding that two implementations disagree.

**The one rule that governs this page.** A fuzz surface is either exercised by
**all three** implementations or by **none**. A surface that only one tree
fuzzes is a parity gap, and it is tracked in [section 5](#5-parity-gaps) until it
is closed — not left as a virtue of the tree that happens to have it. The gaps
are listed there precisely so that "Rust already does it" can never be the end of
the conversation.

Related: [`MAINTAINER-GUIDE.md`](MAINTAINER-GUIDE.md) for CI architecture,
[`ERROR-CATALOG.md`](ERROR-CATALOG.md) for the exit codes a fuzz finding is
classified by, [`DATA-SCENARIOS.md`](DATA-SCENARIOS.md) for what the decoder is
*supposed* to do with each kind of bad input.

---

## 1. Two architectures, and why the results look different

The repository contains two structurally different kinds of fuzzing. Almost
every surprise about "why don't the three implementations agree?" traces back to
which of the two a given surface uses.

### A. In-process robustness harnesses (per-language)

Each implementation carries its own harness, written in its own test framework,
running in its own process:

| | location | framework |
|---|---|---|
| Rust | `rust/tests/integration.rs` | libtest (`#[test]`) |
| Python | `python/tests/test_e2e.py` | pytest |
| C++ | `cpp/tests/test_fuzz.cpp` | Catch2 |

They share a **generator**: the same xorshift64 PRNG, the same seed
(`0x0DDCD1ECDDC0DEC0`), the same size bands, the same little-endian fill. So all
three see the *same bytes*.

They do **not** share an **assertion**. Each one asserts only a local, negative
property — *this process did not crash*:

- Rust: `std::panic::catch_unwind` — the only thing it can distinguish is
  panic / no panic. That "nothing but a `MieError` escapes" is a compile-time
  guarantee of the `Result<T, MieError>` signature, so the harness does not need
  to check it.
- Python: `except MieDecoderError: pass` ahead of a bare `except Exception:` that
  fails the test — a runtime assertion that the exception *type* is documented.
- C++: `catch (const mie::MieError&)` with an empty body; any other exception
  type escapes and fails the case.

**No per-record comparison happens.** Three implementations can each
independently prove "I did not crash" while producing entirely different decoded
output on the same bytes. That is the central limitation of this architecture,
and closing it properly is [section 6.1](#6-what-could-be-added).

What each harness *does* now share is a **summary line**. Every harness ends by
writing one `FUZZ-SUMMARY` record of counters — inputs, bytes generated, readers
opened, records yielded, errors — defined to mean the same thing in all three,
and the burn-in's `fuzz-compare` job diffs them all-pairs
(`scripts/compare-fuzz-summaries.py`). It is a coarse check: it would not catch
two implementations decoding the same record to different field values. It does
catch any disagreement about how many records a file yields, how many were
rejected, or how many inputs opened at all — and it costs one line per harness.

Every counter must be **path-independent**. The harnesses name their temp files
differently, so anything derived from a path measures the harness rather than the
decoder. The dump harness counts output *lines* for exactly this reason: it
counted bytes first, and Rust and Python disagreed by a constant offset that
turned out to be the length of the input path in the dump header.

Three consequences of the per-language architecture bit hard enough to be worth
recording, all fixed in v2.15.1:

1. **The jobs were not scoped alike.** Rust and Python ran *only* their
   harnesses; the C++ job ran `make -C cpp check` — the whole suite, from a cold
   build. Its wall time was therefore mostly not fuzzing, and Catch2 reports no
   per-case duration without `-d yes`, so the fuzz portion could not be
   recovered from the total. The burn-in now uses `make -C cpp check-fuzz`,
   which runs the `[fuzz]` cases only; `make check` is deliberately left as the
   single unparameterised command a developer runs.
2. **The C++ WARN stream was contaminated.** Because the whole suite ran, the
   job log interleaved `mie_decoder::writer`, `::config` and `::order`
   diagnostics from ordinary test cases with the fuzz harness's, so per-message
   counts from that log were not comparable to Rust's. Fixed by the same change.
3. **Log capture worked in exactly one of the three.** Rust's `log::emit` writes
   through `std::io::stderr().lock()`, and libtest's capture only intercepts the
   `print!` / `eprint!` macros — so `--nocapture` was a no-op for the crate and
   its diagnostics always streamed. C++ writes to the stderr fd, which Catch2
   does not redirect. pytest *does* capture. The result was that two jobs wrote
   tens of megabytes of WARN lines into every scheduled run while the third
   showed nothing, and the workflow's `stream_logs` input controlled only the
   third. Silencing is now the harness's job, driven by `MIE_FUZZ_STREAM_LOGS`
   ([section 3](#3-where-each-harness-runs)).

### B. Shared differential drivers (cross-implementation)

`tests/conformance/` holds the other architecture, and it is the one that
actually answers "do the three agree?". A single Python driver:

1. generates one input,
2. runs it through **every registered implementation's CLI** as a subprocess,
3. classifies each outcome, and
4. compares **all pairs** — any two disagreeing is a finding.

The comparator lives in `tests/conformance/differential.py`. It is deliberately
*not* majority-rule (two implementations sharing a bug would outvote the correct
one) and *not* reference-implementation (that would make one tree's quirks
normative). It reports the split — `accept: Rust, Python | reject: C++` — without
naming a winner, because which side is right is a maintainer's judgement.

This is the architecture the binary-input surfaces should converge on. It is
already in use for the config parser (section 4.4).

---

## 2. Inventory: what is fuzzed today

| # | Surface | Rust | Python | C++ | Architecture | Cross-impl compared? |
|---|---------|:----:|:------:|:---:|--------------|:--------------------:|
| 1 | Reader / binary record stream | yes | yes | yes | in-process | summary counters |
| 2 | `dump` (record-aware + raw hex) | yes | yes | **no** | in-process | summary counters |
| 3 | Merge input resolution (manifest, glob) | yes | partial | **no** | in-process | **no** |
| 4 | Config-parser documents (TOML) | yes | yes | yes | shared differential | **yes**, per input |
| 5 | CLI argument vectors | no | no | no | — | — |

"Summary counters" means the run *totals* are compared, not each input's output
— see [section 1.A](#a-in-process-robustness-harnesses-per-language). Surface 4
is the only one where a divergence is attributed to the specific input that
caused it.

Row 5 is listed to record that it is *absent everywhere*, which under the rule in
the header means it is not a parity gap — it is a future item
([section 6](#6-what-could-be-added)).

Adjacent but **not fuzzing** — exhaustive rather than generated, and worth
knowing about because it is the strongest cross-implementation check in the tree:
`cpp/tests/test_decode_exhaustive.cpp` sweeps every possible Type Word, every
possible Command Word and every timestamp field bit through the C++ decoders and
compares an FNV-1a digest against constants produced by
`rust/examples/decode_digest.rs`. Python has no counterpart.

---

## 3. Where each harness runs

| Driver | Default effort | Deep run | Workflow |
|---|---|---|---|
| In-process byte harnesses | `MIE_FUZZ_ITERATIONS` unset -> 256 | `MIE_FUZZ_ITERATIONS` override | `ci.yml` (Rust/Python, default), `cpp-ci.yml` (C++, default, across every tier), `fuzz.yml` (all three, both platforms, burn-in) |
| Merge-resolution harnesses | fixed iteration count in the test body | none | `ci.yml` (as part of the ordinary suite) |
| Config-parser fuzz | `MIE_CONFIG_FUZZ_ITERS` unset -> 100, `MIE_CONFIG_FUZZ_SEED` unset -> a fixed seed | env override | `differential.yml` only |

The byte harnesses share three environment variables, and mean the same thing by
each. That is load-bearing rather than tidy: a burn-in scoped differently per
language cannot be compared across languages.

| Variable | Default | Effect |
|---|---|---|
| `MIE_FUZZ_ITERATIONS` | 256 | Inputs to generate. Unparseable or zero falls back rather than guessing. |
| `MIE_FUZZ_STREAM_LOGS` | unset (silent) | `1` / `true` leaves the decoder's logger at WARN so its diagnostics stream. |
| `MIE_FUZZ_SUMMARY` | unset | File to append the run's `FUZZ-SUMMARY` line to. |

How the log level is set differs by language, and the difference is deliberate.
C++ uses an RAII guard, because Catch2 runs its cases sequentially in one
process and leaving the level at `OFF` broke `test_log.cpp` two files away.
Python uses a context manager for the same reason. Rust sets it and does **not**
restore it: libtest runs that binary's tests in parallel threads, so a scope
guard would restore the level while a sibling test was still running — the race
would be worse than the leak, and no other test in that binary asserts on
logging.

Three things about the table matter more than they look:

- **`fuzz.yml` runs C++ in its least-instrumented configuration.** The burn-in
  step is `make -C cpp check-fuzz` — the default `-O2` tier. The ASan/UBSan,
  Valgrind and GCC 4.8.5 tiers live in `cpp-ci.yml` and none of them set
  `MIE_FUZZ_ITERATIONS`, so the *sanitized* builds only ever see the default
  iteration count. A deep sweep against an uninstrumented binary will not notice
  an out-of-bounds read that does not happen to fault.
- **The burn-in covers Windows as well as Linux.** The C++ platform layer is
  entirely different code there — file mapping, path identity, binary stdout
  (ADR-0003) — and mapping a file is the first thing every harness does with its
  garbage input. Rust's and Python's mapping paths differ by platform too.
  `ci.yml` and `cpp-ci.yml` already build and test all three on Windows, but only
  at the 256-iteration default.
- **`differential.yml` is the only job that runs all three *in one job*.** It has
  no path filter, on purpose: the config fuzz checks skip themselves when fewer
  than two implementations are selected, so the `--skip cpp` and `--only cpp`
  invocations in the other workflows do not exercise them. `fuzz.yml` reaches the
  same comparison a different way — one job per implementation, then a
  `fuzz-compare` job over the uploaded summaries — because the three harnesses
  are in-process and have no single CLI a shared driver could call.

---

## 4. The surfaces in detail

### 4.1 Reader / binary record stream

**Requirement:** L1-ROB-001.

Generates a random byte sequence in a size band that keeps record headers
reachable while keeping each iteration fast, writes a temp file, opens it with
the reader, and drains the iterator through the canonical-order stage
(`order_rows` / `OrderedSource`) with a deliberately small `max_sort_group` cap.
The property asserted is *no crash*, plus a defence-in-depth bound that fails if
the iterator yields an implausible number of records (a stand-in for an
unbounded loop).

Random bytes are a good generator for this surface because they reach the
recovery paths densely: the sync-loss, freerun-timestamp, unknown-error-code,
reserved-bit and structural-invariant branches all fire in volume.

Two claims the harnesses' own comments make that a burn-in does **not** bear out,
and which should be read sceptically until fixed:

- The `max_sort_group` cap branch is described as "reached often". Across a full
  burn-in the Rust log contains no `mie_decoder::order` diagnostics at all —
  random bytes essentially never produce enough consecutive records with an equal
  decoded timestamp to hit a small cap.
- The harnesses are described as directly comparable. That holds for the
  *generator* but not for the *reporting*; see section 1.A.

### 4.2 `dump`

**Requirements:** L1-ROB-001, L2-CLI-009.

Same generator, a much smaller size band so the truncation and loop-guard paths
are hit densely, including zero-length inputs that exercise the empty-file
rejection. Drives both the record-aware dump and the raw hex dump into a
throwaway sink.

The property this guards is specific and worth stating: the record dump reads
headers under a bounds guard and slices to the record extent for the body — it
never reads payload by a Command Word's `data_word_count`, so it has no
over-claim/overrun class of its own. The harness exists to keep it that way.

**C++ has no equivalent.** See [section 5](#5-parity-gaps).

### 4.3 Merge input resolution

**Requirements:** L1-ROB-001, L2-MRG-001.

Feeds arbitrary bytes to the manifest reader, which must return a parsed list or
a documented decoding error and nothing else.

Rust's harness goes further than Python's: it also treats the same bytes
(lossily) as a glob *pattern* and drives the hand-rolled `glob_match` and
`expand_glob` with it. Since the glob matcher is hand-rolled in all three trees
— and the C++ one deliberately advances `?` by a whole UTF-8 character so it
agrees with Rust's and Python's scalar-value matching — this is exactly the kind
of surface where three independent implementations drift.

The two harnesses also use **different PRNGs** (Rust: the shared xorshift64;
Python: `random.Random`), so unlike every other surface they do not even see the
same inputs.

### 4.4 Config-parser documents

**This is the surface that already works the way the others should.**

`tests/conformance/config_fuzz.py` generates small TOML-ish documents from
palettes chosen to hit historically divergent ground — leading zeros, bare
trailing dots, hex/octal/binary literals, underscore separators, string escapes,
inline tables, datetimes, dotted and quoted keys, non-ASCII identifiers and
non-ASCII digits — writes each to a file, and runs **every** implementation's CLI
against it with `--config`. It compares the accept/reject verdict all-pairs and,
on a divergence, prints the generating document verbatim and indented, ready to
paste into `config_parity.py` as a pinned regression.

Two design choices are worth copying:

1. **It compares a verdict, not a message.** The implementations legitimately
   differ in wording, and a rejection may surface as a config error or a usage
   error depending on where the value was caught. What they must agree on is
   whether the document is *admissible*.
2. **The seed alone is not the reproducer.** The failing document is printed in
   full, because a seed does not survive a change to the generator.

The non-ASCII entries in the palettes are load-bearing: they are what keeps
Python's `re.ASCII` flags from being silently dropped, since a bare `\w` or `\d`
is Unicode-aware and would accept literals that Rust's `is_ascii_alphanumeric` /
`is_ascii_digit` refuse.

---

## 5. Parity gaps

Under the rule at the top of this page, each row is a defect, not a feature of
the tree in the "has it" column.

| Gap | Has it | Missing from | Why it matters |
|---|---|---|---|
| `dump` fuzz harness | Rust, Python | **C++** | C++ is the tree with manual bounds arithmetic and no borrow checker; it is the one that most needs this harness, and it is the one without it. |
| Merge glob-pattern fuzz | Rust | **Python, C++** | The glob matcher is hand-rolled three times and must agree on UTF-8 `?` advancement. Nothing generated tests that agreement. |
| Merge manifest fuzz | Rust, Python | **C++** | Same surface, one implementation short. |
| Shared PRNG for merge fuzz | Rust | **Python** | Python uses `random.Random`, so the two harnesses do not see the same inputs and cannot be compared even by hand. |
| Exhaustive decode digest | Rust (oracle), C++ | **Python** | The strongest cross-impl check in the tree covers two of three decoders. |
| Sanitized deep run | — | **C++** | The burn-in raises iterations only on the uninstrumented `-O2` build; ASan/UBSan/Valgrind stay at the default count. |
| Per-input differential comparison of fuzzed *binary* input | — | **all three** | The `FUZZ-SUMMARY` counters compare run totals, which catches a disagreement about how many records a file yields but not two implementations decoding the same record to different values. See [6.1](#6-what-could-be-added). |
| Summary line from the merge-resolution harnesses | — | **all three** | Surface 3 has no `FUZZ-SUMMARY` output, so `fuzz-compare` cannot see it at all. |

---

## 6. What could be added

Listed as candidates, not commitments. Each is written so that adopting it means
adopting it in **all three** implementations at once.

**6.1 Per-input differential record-stream fuzzing (highest value).** Promote
surface 1 to the section 4.4 architecture: a shared driver in
`tests/conformance/` generates bytes once, runs `decode` through all three CLIs,
and compares an exit-code class plus the CSV bytes all-pairs. The machinery —
`differential.py`, the `ImplSpec` CLI prefixes, the all-pairs comparator —
already exists and is already wired into a workflow that runs all three.

The `FUZZ-SUMMARY` comparison is the cheap version of this and is already in
place; what it cannot do is attribute a divergence to an input, or notice two
implementations decoding the same record to different *values* as long as they
agree on the count. A per-input driver gets both, at the cost of three
subprocesses per input.

**6.2 Structure-aware generation.** Uniform random bytes reach recovery paths
densely but reach *valid* record paths almost never, which is why the
canonical-order cap branch stays unvisited. A generator that emits well-formed
records and then mutates them — flip a Type Word bit, truncate a payload, corrupt
one Command Word, repeat a timestamp — would exercise the decode and ordering
paths that random noise cannot reach. This is a change to the *generator* and
should be made in one place, which argues for doing 6.1 first.

**6.3 CLI argument-vector fuzzing.** Generated `argv` arrays over the real flag
surface, compared all-pairs on exit-code class. Deliberately **not** implemented
today in any tree, so under the rule at the top of this page it is a clean
addition rather than a gap. The `cli-surface-parity` gate already proves the
three expose the same *flags*; this would test that they agree on the same
*combinations*, including mutually-exclusive ones and malformed values.

**6.4 Merge-topology fuzzing.** Generated sets of small input files with
overlapping and inverted timelines, mixed timestamp formats, and duplicate
records, compared across implementations. The merge heap, the DELTA scope rules
and the duplicate-collapse window are three interacting behaviours that the
curated fixtures test one at a time.

**6.5 Corpus-guided fuzzing (libFuzzer / AFL++).** Explicitly considered and
rejected for the C++ tree when it was scoped, for reasons recorded in
`cpp/tests/test_fuzz.cpp`: libFuzzer requires clang, which would mean the fuzz
target ran on one C++ tier and not on the GCC 4.8.5 tier the implementation
exists for; and coverage-guided exploration is non-deterministic, so a required
job can fail on an input nobody's change produced. If it is ever revisited it
should be as a **non-blocking** job alongside the deterministic harnesses, not as
a replacement for them.

---

## 7. Running and triaging

```bash
# Deep run of the in-process byte harnesses (all three honour the same knobs)
MIE_FUZZ_ITERATIONS=25000 cargo test --test integration fuzz_arbitrary_bytes_never_panic
MIE_FUZZ_ITERATIONS=25000 poetry -C python run pytest tests/test_e2e.py::TestFuzzHarness -s
MIE_FUZZ_ITERATIONS=25000 make -C cpp check-fuzz

# See the decoder's own diagnostics while it runs (any of the three)
MIE_FUZZ_ITERATIONS=256 MIE_FUZZ_STREAM_LOGS=1 make -C cpp check-fuzz

# C++ deep run WITH instrumentation -- what fuzz.yml does not currently do
MIE_FUZZ_ITERATIONS=25000 make -C cpp check SANITIZE=1

# Reproduce the cross-implementation comparison locally: point every harness at
# ONE summary file, then compare. The file is appended to, so order is free.
export MIE_FUZZ_SUMMARY=/tmp/fuzz-summary.txt && rm -f "$MIE_FUZZ_SUMMARY"
(cd rust && cargo test --test integration -- fuzz_arbitrary_bytes_never_panic \
    dump_arbitrary_bytes_never_panics)
poetry -C python run pytest tests/test_e2e.py::TestFuzzHarness
make -C cpp check-fuzz
mkdir -p /tmp/fz/local && cp "$MIE_FUZZ_SUMMARY" /tmp/fz/local/
python scripts/compare-fuzz-summaries.py /tmp/fz

# Deep run of the differential config fuzzer, across every implementation
(cd rust && cargo build) && (cd cpp && make all)
MIE_CONFIG_FUZZ_ITERS=5000 poetry -C python run python ../tests/conformance/run.py
```

**Triaging an in-process finding.** The failure prints the seed, the iteration
index and the input size, and the harnesses share a generator — so the same
iteration index reproduces the same bytes in the other two trees. Run the other
two at the same iteration count before assuming the defect is local. The
harnesses also emit their `FUZZ-SUMMARY` line on the failure path, with
`outcome=panic` / `outcome=error` in place of `outcome=ok`, so a run that died at
iteration 24 000 still reports the 24 000 iterations' worth of counters it had.

**Triaging a `fuzz-compare` failure.** The report names the split without naming
a winner, in the style of `tests/conformance/differential.py`. Reproduce with the
local recipe above at the burn-in's iteration count, then bisect by iteration:
the generator is deterministic, so halving `MIE_FUZZ_ITERATIONS` tells you
whether the divergence is in the first half.

**Triaging a differential finding.** The failure prints the generating input
verbatim. Pin it as a regression case first (`config_parity.py` for a config
document), *then* decide which side is wrong. The comparator deliberately does
not tell you — a divergence is the finding, and picking a winner is a separate
judgement that should be recorded in the fix.

**When a fuzz finding is not a bug.** Random input legitimately produces
`MieError` / `MieDecoderError` in volume; that is the documented response and is
what the harnesses are written to accept. A finding is a bug when the escape is
an *undocumented* exception type, a panic, a hang, or — for the differential
drivers — an *agreement* failure, regardless of whether either side crashed.

---

## 8. Rules for adding a fuzz harness

1. **All three or none.** If a surface is worth fuzzing in one tree it is worth
   fuzzing in the other two. Land the third before the first is merged, or record
   the gap in section 5 with an owner.
2. **Deterministic.** A fixed seed and a fixed default iteration count, with an
   environment override for depth. A required job must not fail on an input
   nobody's change produced.
3. **One generator.** If the harnesses are meant to see the same inputs, they
   must share a PRNG *and* a consumption order. Two different PRNGs is not a
   smaller version of the same test.
4. **Print the reproducer, not just the seed.** A seed stops reproducing the
   moment the generator changes.
5. **Prefer the differential driver.** An in-process harness proves a negative
   about one implementation. A differential driver proves a positive about all of
   them. Reach for `tests/conformance/` unless the surface genuinely has no CLI
   expression.
6. **Instrument the deep run.** A high iteration count against an uninstrumented
   binary buys less than a low count against a sanitized one. Do both.
