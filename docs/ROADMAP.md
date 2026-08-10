# MIE-Decoder Roadmap

> **This roadmap is forward-looking only.** Completed work is not tracked here —
> it lives in `CHANGELOG.md` (release history), `docs/L1-REQ.md` /
> `docs/L2-REQ.md` / `docs/L3-REQ.md` (the normative requirements), and
> `docs/TRACE-MATRIX.md` (verification status), all backed by git history.
>
> **Do not mint requirement IDs (`L2-*`, `L3-*`) in this file.** An ID is born
> only when its requirement is written in `docs/L2-REQ.md` / `docs/L3-REQ.md`;
> describe intended work in prose and let the requirement process own the IDs.
> The former "Team Review Backlog", "Production-Readiness Audit", "Architecture
> Audit", and "Documentation Initiative" sections — all describing shipped work —
> were removed on 2026-07-07 for exactly this reason: provisional IDs minted here
> had begun to collide with real assignments (e.g. this file proposed
> `L2-CONF-006` for a conformance case while `docs/L2-REQ.md` had since assigned
> `L2-CONF-006` to the public-library-API requirement).

## Queued for the next release (`[Unreleased]`)

`[Unreleased]` is emptied at each release cut; whatever sits above the
most recent dated section in `CHANGELOG.md` is the live queue. Future work
accumulates here; when ready to cut a release, follow the version-bump
checklist in `docs/MAINTAINER-GUIDE.md` section 11.

## Planned

| Version | Feature |
|---------|---------|
| 3.0 | Data word decoders, additional per-message-type CSVs. |
| 4.0 | Apache Parquet output. |

## Merge follow-ups

The multi-file time-sorted merge has shipped (see `CHANGELOG.md`); the items
below are candidate refinements on top of it, **none built yet**. The first two
groups are next-release candidates (investigate-then-schedule); `--order file`
is distant-future. None has a committed version.

**Recorder-identity-aware merge.** The `MUX` column is already populated from a
configurable filename field (delimiter + 0-based index). Two related ideas could
reuse that same parsed identity:

- **Per-recorder DELTA — partly delivered in v2.11.0.** Merged DELTA is now
  selectable between `per-file` (the default) and `global` via `--delta-scope` /
  `[merge] delta_scope` (L2-MRG-005), which covers the common case: one file per
  recorder. What remains is the *identity* half of the original idea — keying
  DELTA on a recorder parsed from the file name (the MUX mechanism) rather than
  on the input file itself, which would matter only if one recorder's output were
  split across several files, or several recorders' output combined into one.
- **Identity-based residual tiebreak.** Equal-timestamp rows now order by
  `RT` then `MSG` (L1-OUT-003), so the input's position in the resolved list is
  no longer the *primary* tiebreak. It survives as the **residual** one: two
  records that share a timestamp *and* an RT *and* a MSG still fall back to
  `(file_index, within-file sequence)` — which for a genuine cross-recorder
  duplicate means "whichever file you listed first wins". A future release could
  make that residual order key on the parsed recorder identity instead, so the
  survivor is chosen by which recorder it came from rather than by argument
  order. (`--collapse-duplicates` already addresses the common case by emitting
  one row instead of choosing between two.)
- **Optional `TERM_NAME` from the filename** — the same delimiter+index
  mechanism (a second configurable field) could populate the still-empty
  `TERM_NAME` column, if a terminal-name field is encoded in the name.
- **Richer locator** — if delimiter+index proves insufficient for some naming
  scheme, a hand-rolled wildcard locator (reusing the `--glob` matcher) could be
  added without a new dependency.

**Cross-recorder de-duplication refinements.** Collapsing duplicate transactions
seen by multiple recorders already ships behind `--collapse-duplicates` /
`[merge] collapse_duplicates` (off by default, loss-free; window via
`--collapse-window-us`). Possible follow-ups, none built:

- **Witness annotation** — an optional column recording how many / which
  recorders saw a collapsed event (changes the CSV schema, so it would be its
  own opt-in).
- **Clock-skew alignment** — estimate / correct a per-recorder time offset
  rather than relying on a fixed `--collapse-window-us` tolerance.
- **Survivor-selection policy** — prefer the cleanest copy (e.g. the non-error
  read) rather than always keeping the first in heap order.
- **Deduped `count`** — collapsing applies to the `decode` CSV path today; a
  deduped `count` mode could follow.

**`--order file` (non-time merge).** A distant-future opt-in that would
concatenate inputs in CLI/manifest order **without** time-sorting, for sets that
are not calendar-locked IRIG (Standard counters, freerun, or mixed-format).
Today such sets are hard-rejected (exit 6) because they cannot share an absolute
timeline; `--order file` would let an operator explicitly accept a non-time
ordering. It is gated behind an explicit flag precisely so the default can never
silently emit a misleadingly "sorted" CSV — the operator must opt out of the
time guarantee. Output rows would carry their source-file order; DELTA would be
per file (no global timeline exists in this mode). Not scheduled; recorded here
so the request isn't folded into the time-merge contract without separate design.

## Decode correctness

- **IRIG day-field decoding across DDC card models.** Known limitation in
  v1.0.0 (carried from Python). The bit layout for the day-of-year field
  appears to vary between firmware versions; needs reverse-engineering
  across a sample set with cross-references against vendor CSV.
  - **Status: blocked on external data** — cannot proceed without real
    sample recordings. The v1.5.0 PRA-9 work only made the discrepancy
    *visible* (a one-time advisory WARN); the actual decode fix is deferred
    until ground-truth data is available.
  - **What's known.** The decoder extracts day-of-year as a 9-bit binary
    integer from Upper-Word bits 13–5 (`(upper >> 5) & 0x1FF`), which is
    correct per the DDC specification. Hour, minute, second, microsecond,
    and the freerun bit all decode correctly and match vendor CSV on every
    observed card model. Only the day-of-year field diverges, and only on
    *some* models — suggesting the firmware encodes that field differently
    (leading hypotheses: BCD rather than binary, or a different field width
    / bit offset). Freerun recordings are unaffected (the field carries no
    calendar meaning when the internal oscillator is running).
  - **What we need to collect** (the external dependency): for each of
    several DDC card models / firmware revisions — (1) a real `.mie`
    recording, (2) the vendor-generated CSV for that *same* file (the
    oracle), and (3) the known real calendar date the recording was made.
    A model/firmware identifier per sample is needed to tell a per-model
    encoding from a universal mis-slice.
  - **What we're looking for / method.** For each sample, pull the raw
    16-bit Upper Word and tabulate decoded-day vs vendor-day vs true-day.
    Solve for the transform that maps our value to the vendor's
    (BCD-decode of the bits? a shifted/widened bit window? an offset?), and
    determine whether it correlates with card model/firmware. Outcome:
    either a single corrected extraction or a model-keyed decode, landed as
    a spec'd requirement with byte-exact conformance fixtures so output
    matches vendor CSV. Until then the advisory WARN stays.

## SonarCloud security findings on the input path (deferred, gate is red)

**`main` currently fails the SonarCloud quality gate**, knowingly, as of the
v2.12.0 cut. `new_security_rating` is 5 where the gate requires 1. Every other
condition passes (reliability 1, maintainability 1, coverage 94.7%, duplication
0.0%, hotspots reviewed 100%), and all 51 non-Sonar CI checks pass.

Two findings, both on the `open(self._path, "rb")` in
`python/src/mie_decoder/reader.py` that landed with the `MieFileIoError`
conversion:

- **`pythonsecurity:S2083`** (blocker) — "Change this code to not construct the
  path from user-controlled data."
- **`pythonsecurity:S8707`** (major) — the agentic path-injection rule, the same
  one already excluded for `config.py`.

**Why this was not simply excluded at the cut.** The exclusion in
`.github/workflows/sonarcloud.yml` is scoped to one rule in one file, and its
comment says that was deliberate: *"any other finding there, and this rule
anywhere else, still fails the build."* The gate caught a new occurrence in a
new file exactly as designed. Widening it silently would spend that design, and
`S2083` has never been suppressed anywhere in this repository — a first blocker
suppression is a decision to take deliberately, not as a step in a release.

**Note the cost being carried.** v2.11.1 was cut *specifically* to stop releases
merging against a failing gate: before it, `sonar.qualitygate.wait` was set only
for pushes to the default branch, so four PRs merged green while `main` was red.
That fix works — the gate is blocking, and this entry exists because it blocked.
Merging anyway restores the very state v2.11.1 removed, and it stays until one
of the options below is taken. Anyone reading a red `main` should find this
section rather than assume the gate is broken again.

**The two real options:**

1. **Extend the exclusion to `reader.py` for both rules.** The false-positive
   argument is identical to the documented `config.py` one: mie-decoder is an
   operator-run CLI, the input path *is* the interface, and the process holds
   exactly the permissions of the user who typed the command — there is no
   sandbox to escape and no privilege boundary to cross. Cheapest, and
   consistent with reasoning already written down. Requires accepting the first
   `S2083` suppression, and `CONFIG-REFERENCE.md`'s "Trust boundary" section
   should be extended to cover the input file so the two stay in step.
2. **Harden rather than suppress.** Mirror what `config.py` already does and
   require a *regular file* before opening, so a directory or a character device
   is rejected up front rather than at the `OSError`. This is the repo's own
   established answer to this rule class and is a genuine improvement, but it
   needs Rust parity, tests on both sides, and a conformance case — and it may
   not clear the `S2083` taint path regardless, since the path still flows from
   an argument to an open.

## Diagram rendering: make the SVG guard real (deferred)

The `diagrams` CI job re-renders `docs/diagrams/*.puml` and byte-diffs the
result against the committed `*.svg`. It has never actually verified anything,
for three independent reasons found on 2026-08-09. The job still runs and is
harmless; what follows is what it would take to make it mean something.

- **PlantUML names its output after the diagram, not the source file.** Every
  source opens `@startuml MIE-Decoder Class Diagram`, so `-o docs/diagrams`
  writes `MIE-Decoder Class Diagram.svg` and never touches the tracked
  `class.svg`. `git diff --exit-code` only inspects tracked files, so the
  untracked renders are invisible and the step passes unconditionally. Fix:
  render into a scratch directory and map `@startuml` names back to source
  basenames explicitly.
- **PlantUML exits 0 on a failed render.** `component.puml` crashes in the
  smetana layout engine (`java.lang.IllegalStateException` in
  `smetana.core.JUtils.qsort`, reached from `dot_mincross`) on stable 1.2026.5
  and 1.2026.6, emitting a truncated ~14 KB SVG where a whole one is ~60 KB —
  and the process still returns 0. Any fix has to scan the render output for
  exceptions, and should assert each expected file was rewritten, rather than
  trusting the exit code. Upstream bug worth reporting with `component.puml` as
  the repro.
- **The pinned version has never matched the committed SVGs, and the version
  that does can't be pinned.** CI pins stable `1.2026.5`; all three committed
  SVGs carry `<?plantuml 1.2026.7beta11?>`, a build published only under
  PlantUML's rolling `snapshot` pre-release, whose assets are named
  `plantuml-SNAPSHOT.jar` and are overwritten in place. It is the only build
  tested here that renders all three diagrams without crashing, and it is
  precisely the one a URL cannot pin. Resolving this means either moving
  `component.puml` off smetana onto Graphviz `dot` (which CI already installs
  for `dataflow.puml`) so a stable release suffices, or waiting for the
  smetana fix to reach a stable release.

A fourth problem constrains whatever is built: **the render is not reproducible
across machines.** Re-rendering the *unchanged* `component.puml` and
`dataflow.puml` with the *same* jar on a different host shifted the canvas
(3350×1490 → 3370×1537 and 3871 → 3844 wide), because PlantUML measures text
with the JVM's font metrics and those differ by platform and font set. So a
byte-diff guard can only ever hold if one fixed environment renders every
committed SVG — a pinned container image, or CI as the sole renderer. A guard
that instead asserts *"the render completes without error and produces every
expected file"* is weaker but environment-independent, and would have caught
two of the three defects above.

## Performance refinements (deferred)

Surfaced by a source review and **verified real but deliberately deferred** —
these are per-record heap churn, not unbounded growth. Both implementations
already stream in constant memory (the design guarantee that matters), so these
would trade code churn in the hottest paths for a marginal, unprofiled gain.
Recorded here so the option isn't lost; revisit only if profiling shows
per-record allocation is a real bottleneck.

- **Rust per-row allocations.** The writer/merge path allocates a `String` DELTA
  key and a `Vec<u16>` dedup key, and formats `format!`/`msg_label()` strings per
  row. Candidate: borrow or intern the keys and reuse scratch buffers so a decode
  makes O(1) allocations rather than O(rows).
- **Python double-build on the error path.** The reader constructs a `MieMessage`
  twice for an errored record. Candidate: build it once and reuse.

Neither changes CSV output, so both are guarded by the existing byte-exact
conformance oracle if attempted.

## Config ergonomics (deferred)

The two config parsers are aligned on a **whitelist** of the flat `[section]` +
`key = value` schema: anything outside it is a config error (exit 5) on both
implementations, and a differential parity corpus keeps them aligned. When that
whitelist was drawn, a few *readable* full-TOML number forms were **rejected on
both** for strictness and consistency:

- underscore digit separators — `standard_tick_rate_hz = 1_000_000`;
- `0x` / `0o` / `0b` integer prefixes — e.g. `detect_records = 0x08`.

These are genuinely handy for a human writing a config, and `tomllib` already
accepts them; the decision to reject was about keeping the Rust hand-rolled
parser minimal and the two implementations identical, not because the forms are
harmful. A future release could instead **accept them on both** — teach the Rust
number parser to strip `_` and honor the `0x` / `0o` / `0b` prefixes, widen the
Python whitelist's value grammar to match, and flip the corresponding
`config_parity.py` snippets from `reject` to `accept`. It is an additive
ergonomics change with no effect on `config/default.toml` (which uses plain
numbers); not scheduled, recorded here so the option isn't lost. Any change must
keep the two parsers byte-for-byte aligned via the parity corpus.

## Shared Commitments

- **`config/default.toml` and TOML config support remain a first-class feature.** The Rust build ships a hand-rolled TOML loader for our config schema; the file format and key names are stable.
- **CSV column layout matches DDC vendor output byte-for-byte.** No reordering or renaming of columns, including the vendor placeholder columns (`MUX`, `TERM_NAME`, `IM_GAP`, `RCV_GAP`, `XMT_GAP`). `TERM_NAME`/`IM_GAP`/`RCV_GAP`/`XMT_GAP` remain empty. As of L2-WRT-020 the `MUX` *cell* is populated from the input file name by default (its column position is unchanged); `--no-mux` / `[mux] enabled = false` restores empty MUX for a byte-for-byte vendor diff.
- **Sync recovery semantics preserved.** N-record look-ahead (default `N = 2` per L2-SYN-005, configurable via L2-SYN-026), 64 KB scan cap, error records and SPURIOUS_DATA continuations remain valid records that pass validation.
- **One validation implementation.** Header skip, normal forward decode, and post-loss recovery share the same validation rules through the boolean compatibility wrapper or the detailed failure API. There is no weaker fast path.
- **Cross-implementation conformance.** Text-based fixtures under
  `tests/conformance/` exercise shared decoding, recovery, filtering, config,
  error, and CSV behavior against byte-exact output oracles in CI.

## Out of Scope (Pinned)

### IRIG 106 1553 decode support is out of scope for MIE Decoder

See `docs/L1-REQ.md` NR-001. MIE files use a DDC proprietary
record format that is distinct from IRIG 106 Chapter 10 1553 packet
formats. Adding IRIG 106 1553 decode is a new capability — separate
requirements, design analysis, architecture review, and approval —
not an incremental extension of MIE-Decoder. Any inbound feature
request that says "just add IRIG 106 support" SHALL be redirected
to a new requirements + design review.
