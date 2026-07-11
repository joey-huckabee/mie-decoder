# Changelog

All notable changes to MIE-Decoder are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Versioning model: **v1.0.0 is a joint cut** — both the Rust crate and
the Python package ship from a single repository tag (`v1.0.0`).
Subsequent releases may diverge in version via impl-prefixed tags
(`rust-vX.Y.Z`, `python-vX.Y.Z`); the cross-implementation conformance
contract (byte-exact CSV equivalence on shared behavior) holds at any
compatible version pair. See `docs/MAINTAINER-GUIDE.md` §11 for the
full release workflow.

## [Unreleased]

### Fixed

- **Version short flag parity across implementations.** Both CLIs now accept
  `-V`, `-v`, and `--version` (with any letter case in the long form). Previously
  Rust accepted only `-V` and Python only `-v`, so a flag that worked on one
  implementation failed on the other.
- **`--time-format` is now case-insensitive on both implementations.** The Python
  CLI previously rejected `--time-format IRIG` / `Auto` (argparse `choices` are
  case-sensitive) while Rust accepted them. Both CLIs and both config loaders now
  route every spelling through one shared case-insensitive parser per
  implementation, so the CLI and `[decode] time_format` can never disagree on
  which names are accepted. An unrecognized name is still rejected (CLI: usage
  error, exit 4; config file: config error, exit 5).
- **`dump` numeric arguments accept the same notation across both flags and
  implementations.** Python's `dump --records` was decimal-only while its own
  `--offset` / `--length` already accepted `0x` hex (and Rust accepted hex on all
  three) — an internal and cross-implementation inconsistency. All three now go
  through one shared parser per implementation that accepts decimal and `0x` hex
  (Python also accepts `0o` / `0b`) and rejects negative values, matching the
  Rust unsigned semantics. Invalid values are a usage error (exit 4).
- **Schema-invalid config values are now clean config errors on both
  implementations.** A TOML-valid but schema-invalid value — a non-string
  `time_format` / `error_mode` (e.g. `time_format = 1`), or a known section name
  assigned a scalar (`decode = true` instead of a `[decode]` header) — previously
  crashed the Python CLI with an unclassified `AttributeError` (exit 1), and the
  section-as-scalar case was silently ignored by the Rust loader (exit 0). Both
  now reject these as configuration errors (exit 5): Python validates section
  shape and string types at load time, and Rust rejects a known section name used
  as a scalar (the same silent-drop footgun as a section the loader never reads).
- **Python now caps a record's data-word payload at 32, matching Rust.** A
  crafted record whose Type Word claims more than 32 data words previously left
  Python's `MieMessage.data_words` unbounded, while Rust's `DataWords` inline
  buffer (`[u16; 32]`) truncates it — so a library consumer saw different payload
  lengths across implementations for such a record. `MieMessage` now truncates to
  32 on construction. No CSV impact (the writer emits at most 32 `WDnn` columns
  either way), and MIL-STD-1553B caps a transaction at 32 words, so conforming
  records are unaffected.
- **Standard-timestamp microsecond rounding is now bit-identical across
  implementations.** Python computed `int(x + 0.5)` while Rust used `f64::round`;
  for a value one ULP below a half-integer these differ by 1 µs (Python rounds
  `x + 0.5` up to `1.0`), a latent break of the byte-exact CSV contract on
  calibrated Standard timestamps (`--standard-tick-rate-hz`). Python now rounds
  half-away-from-zero via `floor(x) + (frac >= 0.5)`, matching `f64::round`
  exactly. Rate-gated and rare, but the two decoders can no longer disagree.
- **Removed two latent panic sites in the Rust decoder (L1-ROB-001).** The
  `unreachable!()` in the reader's timestamp decode and the `assert!` in the
  public `DataWords::from_slice` were both provably unreachable / uncallable past
  their guards, but they nicked the no-panic invariant. The reader now decodes a
  stray `Auto` format as IRIG (its documented fallback) instead of panicking, and
  `DataWords::from_slice` truncates an over-length slice to the 1553B 32-word cap
  (matching `from_iter_capped`) instead of asserting. The `#[deny]`-on-`warn`
  clippy gate is extended to `panic` / `unreachable` / `todo` / `unimplemented`
  (in addition to `unwrap` / `expect`) so the invariant is now enforced in CI.
- **Python merge output-collision guard now fires when `--allow-partial` drops a
  merge to a single surviving input.** The guard was gated on the surviving
  reader count, so a multi-input merge that lost all but one input to
  `--allow-partial` skipped it — and because the writer's own single-input check
  is bypassed for any requested merge, an output path pointing at one of the
  inputs could overwrite it in place. It is now gated on whether a merge was
  *requested* (checking the full requested input set), matching the Rust CLI.
- **Atomic CSV writers use a unique, exclusively-created temp file.** Both
  implementations named their temp file `<dest>.mie-decoder.tmp.<pid>` and opened
  it with truncation, so two writers targeting the same destination *within one
  process* could collide on the temp path and clobber each other before the
  atomic rename. The temp name now also carries a per-process monotonic counter
  and a nanosecond timestamp, and the file is created with exclusive-create
  (Rust `create_new` / Python mode `"x"`, i.e. `O_EXCL`) with retry — so
  concurrent same-destination writers can no longer share a temp file. Output
  content, permissions, and the atomic-rename behavior are unchanged.
- **The Rust config loader now rejects non-identifier section headers.** A
  section name with a hyphen, space, or quote (`[bad-section]`, `[bad section]`,
  `["bad"]`) was stored as an oddly-named section on Rust while Python's whitelist
  rejected it. Rust now applies the same simple-identifier check to section
  headers that it already applied to keys, so both reject the form (exit 5).
- **Python now warns for a root-level unknown scalar key, matching Rust.** A
  stray top-level key such as `bogus = true` loaded silently on Python
  (`_warn_unknown_keys` skipped non-table entries), while Rust logged
  `unknown TOML key: [] bogus`. Python now emits the same warning, honoring the
  documented WARN-and-continue contract for unknown keys. (Both still accept the
  config — this is observability only.)
- **Python's config array splitter now handles escaped quotes like Rust.** An
  array whose string element contained an escaped quote followed by a comma
  (e.g. `["a\", b"]`) was mis-split on the interior comma and rejected on Python,
  while Rust (which tracks the preceding backslash) accepted it. The Python
  splitter now mirrors Rust's `push_quoted_char`, so both treat the comma as
  inside the string. Added to the parity corpus and the fuzzer's value set.
- **Config numeric and string-escape grammars are now aligned, and a
  differential fuzzer guards config parsing.** Rust routed numeric literals
  through native `i64` / `f64` parsing, which accepts non-TOML forms (`08`, `1.`,
  `[01]`) that Python's `tomllib` rejects, and the two supported different string
  escape sets. Both now enforce one explicit grammar: numbers match
  `[+-]?(0|[1-9][0-9]*)(.[0-9]+)?([eE][+-]?[0-9]+)?` (no leading zeros / bare
  trailing dot / `0x`-`0o`-`0b` / `_`), and strings accept only the `\"` `\\`
  `\n` `\t` escapes on both. A new differential fuzzer
  (`tests/conformance/config_fuzz.py`) generates config documents and asserts
  both CLIs agree on accept/reject, so a divergence is caught in CI rather than
  reported after the fact.
- **Config parsing is now aligned by an explicit whitelist grammar on both
  implementations, closing a whole class of Rust↔Python divergences.** The two
  parsers accepted different TOML subsets — full-TOML `tomllib` on Python vs a
  minimal hand-rolled parser on Rust — so forms outside the flat `[section]` +
  `key = value` schema (inline tables, multi-line arrays, `1_000` / `0x08` /
  `0o` / `0b` numbers, date-times, quoted keys) were honored by Python but
  rejected by Rust. Rather than continue rejecting each divergent form one at a
  time, Python now validates every config line against the *same* flat grammar
  the Rust parser accepts — a whitelist run before `tomllib` — and Rust rejects
  non-identifier keys, so both refuse anything outside the subset with a config
  error (exit 5). A new differential parity corpus (config snippets run through
  both CLIs asserting identical accept/reject) guards the alignment in CI.
- **Dotted keys, dotted section headers, and array-of-tables headers are now
  rejected by both implementations.** `tomllib` nested a dotted key
  (`decode.strict = true`) or dotted header (`[output.no_clobber]`) and accepted
  an array-of-tables header (`[[decode]]`), while the Rust hand-rolled parser
  silently dropped the dotted key, stored a dotted header as a section literally
  named `output.no_clobber` (ignoring its keys), or misread `[[decode]]` — so the
  same config behaved differently on each implementation, and a mis-typed safety
  option such as `[output.no_clobber]` / `output.no_clobber = true` was silently
  ignored on Rust (dropping overwrite protection). Both now refuse these forms as
  config errors (exit 5); the config schema is the flat `[section]` +
  `key = value` form only.
- **The Rust config loader now rejects duplicate TOML keys and re-declared
  section headers.** A repeated `(section, key)`, or a `[section]` header
  declared more than once (even with different keys inside), previously kept the
  first value / silently merged the blocks on Rust while Python's `tomllib`
  rejected it — so a malformed config could decode differently on each
  implementation. Rust now raises a config error (exit 5) to match, per the TOML
  spec (which forbids defining a table twice). `CONFIG-REFERENCE.md` now
  documents the exact TOML subset the hand-rolled Rust parser accepts (and the
  full-TOML features it does not, e.g. multi-line arrays and `1_000_000`
  underscore separators).
- **The Rust config loader now honors the `[merge]` section.** Setting
  `[merge] collapse_duplicates` / `collapse_window_us` in a TOML config file had
  no effect on the Rust CLI — the section was silently ignored and even reported
  as an unknown key — while the Python CLI applied it. Cross-recorder duplicate
  collapsing (L2-MRG-007) can now be configured from a file on both
  implementations, exactly as `config/default.toml` and `CONFIG-REFERENCE.md`
  already document; the `--collapse-duplicates` / `--collapse-window-us` CLI
  flags still override it. A negative `collapse_window_us` is rejected at load
  time (exit 5). A new conformance case pins the config path to the CLI path.

## [2.7.0] — 2026-07-08

Minor release from a second round of team review. Adds a small public-API
accessor and two diagnostic/tooling improvements, plus documentation
clarifications. No change to decode behavior or the byte-exact CSV output — the
full Rust and Python suites and the 60-case conformance oracle pass unchanged.

### Added

- **`MieMessage.subaddress()` convenience accessor** in both implementations
  (Rust `subaddress(&self) -> Option<u8>`, Python `subaddress` property),
  mirroring the existing `rt()` — returns `None` for SPURIOUS_DATA. This is the
  additive public API that makes the release a minor bump; `cargo-semver-checks`
  confirms no breaking change.
- **`dump` now explains errored records.** The record-aware hex dump annotates
  each record with an explicit `error flag (bit 14): SET/clear`, a classified
  `Format:` line (e.g. `TRANSMIT`, `MODE_CODE_NO_DATA`), and — for errored
  records — an `Error: 0xNNNN → <DDC description>` line, so *why* a record is
  errored is legible at a glance. The classifier is guarded (an unclassifiable
  record degrades to `(unclassifiable)`, never throws) since `dump` runs on
  suspect files.
- **Conformance runner single-implementation modes** (`--python-only` /
  `--rust-only`). Because the byte-exact oracles are committed under
  `tests/conformance/expected/`, each implementation can now be validated on its
  own with no other toolchain present — e.g. a Python-only, air-gapped host runs
  `python tests/conformance/run.py --python-only` with no cargo. `--update-expected`
  still requires both implementations (it regenerates the oracles only when Rust
  and Python agree).

### Changed

- **Docs: default error routing made prominent.** Errored and SPURIOUS records
  go to `<output>_errors.csv` by default (separate mode); `--inline-errors` puts
  them in the main CSV and matches the DDC vendor tool. A README callout and
  cross-references now state this up front (it had surprised operators expecting
  an errored record in the main CSV).
- **Docs: `--glob` matches filenames, not recordings.** Documented that a glob
  which catches a non-recording (a `README`, a log) fails with exit 2 — and in a
  merge aborts before any output — so precise patterns (`dir/*.mie`) are the
  right default. (`docs/CLI-REFERENCE.md`, `docs/USER-GUIDE.md`.)
- **Docs: config schema no longer duplicated in the README.** The drifted TOML
  snippet (wrong filter defaults / logging level, missing keys) is replaced by
  links to the two authoritative sources, `config/default.toml` and
  `docs/CONFIG-REFERENCE.md`.
- **Docs: recorded why the Python wheel build uses `poetry -P` (not `-C`).**
  `poetry -C python build` doubles the source path on Windows (verified on Poetry
  2.3.4); `-P` (`--project`, Poetry ≥ 2.0) is the deliberate workaround.

### Fixed

- **README library examples can no longer silently rot.** The Rust example is now
  compiled in CI (as a `no_run` doctest via `include_str!` in `src/lib.rs`, plus a
  checked `examples/library_usage.rs`, with `cargo test --doc` added to CI and the
  pre-commit hook); the Python example is executed against a fixture by
  `tests/test_readme_examples.py`. Both would have caught the earlier
  `write_csv`-arity and `message.subaddress` breakages.

## [2.6.2] — 2026-07-07

Documentation and packaging release from a team review: corrects a license
declaration contradiction, hardens `dump` output on Windows, fixes the README
library examples, and consolidates the CLI reference. No functional change to
decode behavior or CSV output — the conformance oracle and full test suites are
unchanged.

### Changed

- **License declaration corrected to Apache-2.0.** The `LICENSE` file has always
  been the Apache License 2.0, but the package metadata and READMEs declared
  `MIT` — a contradiction that is legally ambiguous for downstream users and
  fails automated license scans. Every declaration (`rust/Cargo.toml`,
  `python/pyproject.toml`, the three READMEs, `rust/deny.toml`) now correctly
  states **Apache-2.0**, matching the `LICENSE` file. This corrects the declared
  license shown on crates.io / PyPI; the `LICENSE` text itself is unchanged.
- **`docs/ROADMAP.md` is now forward-looking only.** Removed ~1,840 lines of
  completed history (the "Team Review Backlog", "Production-Readiness Audit",
  "Architecture Audit", and "Documentation Initiative" sections) that duplicated
  the requirements docs and had begun minting requirement IDs which collided with
  real assignments (e.g. a proposed `L2-CONF-006` vs. the assigned
  public-library-API `L2-CONF-006`). Completed work is now tracked solely in
  `CHANGELOG.md`, the `L1/L2/L3-REQ.md` requirements, and git; the roadmap no
  longer mints `L2-*`/`L3-*` IDs.

### Added

- **`docs/CLI-REFERENCE.md`** — a complete per-flag reference for the `decode`,
  `count`, and `dump` subcommands and the global options: value, default, range,
  and config-key equivalent for every flag. The root README now links to it
  rather than duplicating an (already-incomplete) exhaustive flag table, giving
  the CLI parameter documentation a single home.

### Fixed

- **`dump` no longer crashes on a redirected stdout on Windows.** The
  record-aware dump annotation contains non-ASCII characters (box-drawing,
  en-dash, `§`); piping `mie-decoder dump file.mie > out.txt` on a stock Windows
  shell (cp1252 code page) previously raised `UnicodeEncodeError` and aborted the
  dump. The CLI now forces UTF-8 on stdout/stderr (encoding only — `newline`
  handling is untouched, so the byte-exact CSV contract is unaffected), matching
  the POSIX default and the raw UTF-8 the Rust build already emitted.
- **README library examples now compile / run.** The Rust example called
  `write_csv` with two arguments — it takes three (added
  `WriteOptions::default()`); the Python example printed `message.subaddress`,
  which does not exist (now `message.msg_label`). Both fixes were verified by
  compiling / running the examples against a real fixture.

## [2.6.1] — 2026-07-07

Maintainability release: resolves the SonarCloud findings from the v2.6.0
analysis. No change to the CLI, the public library API, or the byte-exact CSV
output — the full Rust and Python test suites and the 60-case
cross-implementation conformance oracle pass unchanged.

### Security

- **Log injection hardened (CWE-117).** User-controlled values written to the
  logs (notably input file paths) are now escaped for carriage-return and
  line-feed, so a crafted path can no longer forge or split log records.
  (SonarCloud `S5145`.)

### Changed

- **Cognitive-complexity sweep — behavior-preserving.** 28 functions across both
  implementations were brought under the complexity threshold by pure helper
  extraction, spanning config parsing, timestamp-format detection, the hex
  `dump`, filters, the CSV writers, sync validation, the multi-file merge, the
  CLI argument parsers and decode entrypoint, and the reader hot paths (the Rust
  decode loop and the Python `__iter__` generator). Internal plumbing only — a `MieMessage`
  DELTA-copy helper was factored out and the reader's per-format payload
  extraction was split by message family; no public API, CLI, or CSV change.
- **Minor code-health fixes.** Return-type precision on the merge DELTA copy,
  exception-logging hygiene (log-and-return instead of `logging.exception`
  outside the handler), removal of an unused timestamp-conversion parameter, and
  a redundant `isinstance` guard dropped. (SonarCloud `S5886`, `S5890`, `S8572`,
  `S1172`, `S2589`.)

## [2.6.0] — 2026-07-06

### Added

- **End-of-records terminator awareness (`0x0000`) and the empty-recording
  outcome.** DDC recorders cap the record stream with a null Type Word
  (`0x0000`) and, for a channel that captured no traffic, produce a file that is
  *only* that terminator (e.g. an unused MIL-STD-1553 station — literally the two
  bytes `00 00`). The decoder now understands the terminator: a file whose stream
  opens on it is recognized as a **valid but empty recording** and decodes to a
  **header-only CSV at exit 0** (new `empty-recording` exit class, with a WARN),
  instead of the previous `NoValidRecords` error (exit 2) that halted batch
  processing. `count` prints `0` and exits 0. The wrong-file guard is preserved —
  only a genuine `0x0000` lead word qualifies; arbitrary non-MIE bytes still exit
  2. New requirements **L1-EXIT-010**, **L2-RDR-021**, **L2-SYN-028**; L1-EXIT-002
  amended to carve out the empty case. Documented in `docs/MIE-FORMAT.md` §2.4.

### Fixed

- **The last record of every recording is no longer silently dropped.** Because
  each recording ends `…record, 00 00`, the final record's look-ahead follower is
  the terminator, which the N-record look-ahead (L2-SYN-005) treated as an invalid
  follower — so that record failed validation and was dropped. Single-record files
  failed entirely (`NoValidRecords`). The look-ahead now honors the terminator on
  the trusted-boundary path (forward decode / first-record detection) per
  L2-SYN-028; sync **recovery** stays strict so a mis-aligned candidate can't
  validate off a stray zero data word. This bug went unnoticed because the
  conformance fixtures never included a real terminator — now they do
  (`multi-record-then-terminator` pins all rows survive).
- **`count` now exits 2 (not 1) on a wrong-file input**, matching `decode` and the
  L2-CLI-011 exit-code table.

### Tests

- New Rust unit/CLI tests and Python pytest cases for the terminator (forward
  vs. recovery), single-record-then-terminator, last-record survival, empty
  recording (exit 0 + header-only CSV, `count` → 0), and the wrong-file guard.
- Six new cross-implementation conformance cases, including three fixtures
  carrying a real `00 00` terminator (`empty-recording`,
  `single-record-then-terminator`, `multi-record-then-terminator`) plus two
  `count`-path cross-checks — `count-no-valid-records` (both impls exit 2 on a
  wrong file, guarding the count/decode exit-code parity of L2-CLI-011) and
  `count-multi-record-then-terminator` (both impls count 3, proving the last
  record survives via the count path).

### Documentation

- **`docs/MIE-FORMAT.md` §2.3 now explains *why* the N-record look-ahead
  exists**, not just what it checks: the format has no per-record sync marker,
  so the Type Word's `word_count` is the only framing; a single Type Word
  passes by chance ~1 in 20 of the time, so the look-ahead chains consecutive
  self-consistent lengths into a synthetic sync check that drives the
  false-positive rate toward zero. §2.4 explains, as design rationale, why the
  end-of-records terminator is accepted as an end-of-chain on the forward
  paths but not during sync recovery. `docs/ARCHITECTURE.md` Phase 3 points to
  the deep rationale.

## [2.5.3] — 2026-06-28

### Fixed

- **Transmit mode codes carrying no data word are no longer dropped from the
  CSV.** A non-broadcast transmit mode code (the common MIL-STD-1553 mode codes
  0–15 — "transmit status word", "transmitter shutdown", "reset remote
  terminal", …) was unconditionally classified `MODE_CODE_TX_DATA`, which expects
  a Status **and** a data word; with no data word the record was too short for
  the L2-SYN-022 word-count capacity check and was **silently skipped in lenient
  mode** (the default), so these messages never reached the output. A no-data
  mode code now classifies as `MODE_CODE_NO_DATA` regardless of direction (the
  wire shape is `ModeCmd + Status` either way; the `CMD` column preserves the
  direction) and is written with empty `WD*` columns. Fixed in both
  implementations; the requirement `L2-MSG-004` (which had specified the buggy
  "classified by direction independent of word count") is corrected.

### Tests

- **End-to-end, cross-implementation conformance coverage for all 11 decoded
  message formats** (previously 4 had none: `RECEIVE_BROADCAST`,
  `RT_TO_RT_BROADCAST`, `MODE_CODE_BCAST_NO_DATA`, `MODE_CODE_BCAST_DATA`, plus an
  IRIG `MODE_CODE_RX_DATA` and the headline no-data-transmit regression). Every
  raw Type Word type / decoded format is now proven to decode to a byte-identical
  CSV row in both the Rust and Python CLIs.

### Documentation

- **`MIE-FORMAT.md` §6 now opens with an at-a-glance index of all 11 decoded
  message formats** — each format's source Type Word code, its identification
  rule, and a link to its per-format byte-shape subsection — plus a note on the
  two-layer (raw type code → decoded format) classification and how a flagged
  error record layers on top of any format. Consolidates a standalone
  message-types reference into the format authority (the raw type-code table
  already lived in §4.1 and the `0x01xx` / `0x20xx` error-code tables in §7).

## [2.5.2] — 2026-06-28

### Fixed

- **`merge --allow-partial` now writes a `.partial` for a per-file failure
  detected at *open* or *priming*, not only mid-file** (L2-MRG-004). Previously a
  bad input whose first record was unreadable / non-MIE (e.g. all-0xFF) was
  skipped during priming **without** arming the deferred-partial state, so a
  `good.mie + bad.mie --allow-partial` merge wrote a normal `out.csv` (no
  `.partial`) and reported success — silently losing the signal that an input had
  been dropped. An input that fails at *open* (empty / unreadable / missing)
  likewise aborted the batch even under `--allow-partial`. Both now drop the
  offending input with a WARN naming it, complete the merge from the rest, and
  commit the combined output as `<output>.partial` (and `<stem>_errors.partial`
  in separate mode), exit 0 — uniform with the existing mid-file behavior. New
  Rust + Python regression and CLI tests plus a cross-impl conformance case
  (`merge-allow-partial-priming`, which also implements the previously-declared
  `expected_partial` oracle in the conformance runner) pin it.

### Documentation

- **New `docs/DATA-SCENARIOS.md`** — a plain-language, scenario-indexed reference
  for how the tool handles every data condition (clean / error / spurious
  records, IRIG / Standard / freerun timestamps, empty / truncated / non-MIE
  files, multi-file merge including per-file `--allow-partial` and duplicate
  collapsing, output modes, filters, MUX), each with its CSV / log / exit
  outcome, plus a glossary that defines the jargon (including "oracle"). Linked
  from the docs index and USER-GUIDE.md; cross-references ERROR-CATALOG.md and
  MIE-FORMAT.md.

## [2.5.1] — 2026-06-28

### Fixed

- **Duplicate collapsing no longer faults or over-collapses on a lenient
  non-monotonic merge input** (`--collapse-duplicates`). When an input file is
  not internally time-sorted (L2-MRG-006) the merged stream can step backward;
  the dedup window's one-sided `us - survivor_us` gap then underflowed —
  **panicking the Rust CLI in debug builds (exit 101)** in `merge.rs` — and, in
  Python, bypassed the window check so a record could collapse against a survivor
  far outside `--collapse-window-us`. The window comparison now uses the
  **absolute** time distance (with saturating eviction in Rust), so collapsing is
  panic-free and never drops a row outside the window; on such known-bad order it
  is best-effort. Sorted input is unaffected (byte-identical output). Pinned by
  new Rust + Python regression tests and a cross-impl conformance case
  (`L2-MRG-007`).

## [2.5.0] — 2026-06-27

### Added

- **Cross-recorder duplicate collapsing in the multi-file merge** (opt-in,
  `--collapse-duplicates` / `[merge] collapse_duplicates`, both implementations).
  When several recorders witness the same MIL-STD-1553 transaction, the merge can
  now collapse those duplicate rows into one instead of inflating the message
  count. A duplicate is the same wire content (Type / Command / Status Words,
  data words, error word) seen by a **different** input within
  `--collapse-window-us` microseconds (default `0` = exact-µs match; widen it for
  recorders whose clocks differ slightly). Same-recorder repeats and single-file
  decodes are unaffected; de-duplication runs before the global-DELTA stage so
  DELTA reflects the deduped timeline, and the suppressed count is logged.
  **Off by default — the default never drops a row.** Adds a `[merge]` config
  section; specified by `L1-MRG-003` / `L2-MRG-007` and pinned by cross-impl
  conformance fixtures.
- **Rust public-API SemVer gate** (`cargo-semver-checks` CI job). Runs
  `cargo semver-checks check-release` against the latest release tag
  (`baseline-rev`, with `release-type: minor`), failing a PR that makes a
  **breaking** public-API change to the crate without a major version bump;
  additive changes pass. Completes the Rust CI tooling deferred in v2.4.0 — the
  v2.4.0 tag (the first under the `rust/` layout) is the baseline.
- **Bandit security gate** (`bandit` dev dependency + a `bandit` CI job). Runs
  `bandit -r src/mie_decoder` — security static analysis (SAST) over the Python
  package source — and blocks merge on any finding at the default
  severity/confidence. The initial scan is clean (no issues across the package).

## [2.4.0] — 2026-06-27

### Added

- **SonarCloud (SonarQube Cloud) analysis workflow**
  (`.github/workflows/sonarcloud.yml`). On pushes/PRs to `main` it scans both
  implementations — Rust coverage (LCOV via `cargo cov-lcov`) and Python
  coverage (Cobertura XML via `pytest --cov`) feed the SonarCloud Quality Gate —
  and exports BUG/VULNERABILITY findings to GitHub code scanning (SARIF). Project
  identity and token come from `SONAR_*` repository secrets; the workflow is a
  clean no-op when they are absent. A SonarCloud Quality Gate badge was added to
  the root `README.md`.
- **CodeQL security-scan workflow** (`.github/workflows/codeql.yml`). Analyzes
  the Rust crate and the Python package (both `build-mode: none`) and reports to
  GitHub code scanning. (CodeQL's Rust support is no-build only; macro-heavy
  files extract with reduced fidelity — a known upstream limitation.)
- **Pylint lint gate** (`pylint` dev dependency + a `pylint` CI job). Lints the
  Python package source (`src/mie_decoder`) and blocks merge below 10/10.
  Configuration lives in `python/pyproject.toml` `[tool.pylint.*]` — curated
  disables for the deliberate function-local-import pattern and the `too-many-*`
  complexity family already covered by SonarCloud, plus line length. Reaching a
  clean pass also removed several dead imports and added exception chaining
  (`raise ... from`) in `cli.py` / `config.py`. No behavior change (conformance
  unaffected).
- **Ruff lint + format gate** (`ruff` dev dependency + a `ruff` CI job running
  `ruff check` and `ruff format --check`). Covers the Python package **and
  tests**; config in `python/pyproject.toml` `[tool.ruff]` (line length 100, to
  match pylint). Adopting `ruff format` reformatted the package + tests to its
  Black-style canonical form (mechanical, no behavior change — conformance
  unaffected); `ruff check` also caught two real import bugs in the test suite
  (a missing `from pathlib import Path` and an unused one).
- **Vulture dead-code gate** (`vulture` dev dependency + a `vulture` CI job).
  Scans the package **and tests** together (so test-only usage counts); config
  in `python/pyproject.toml` `[tool.vulture]`. The initial pass removed a genuinely
  unused test fixture (`single_transmit_record`) and ignores three intentional
  names — the argparse `option_string` interface arg and the documented module
  constants `MAX_RECORD_BYTES` / `ALL_KNOWN_ERROR_CODES`. No behavior change.
- **Rust rustdoc gate** (`rust-doc` CI job): `cargo doc --no-deps` with
  `RUSTDOCFLAGS="-D warnings"`, failing on broken intra-doc links and other
  rustdoc lints. The initial pass fixed a broken intra-doc link in `reader.rs`.
- **Rust MSRV gate** (`rust-msrv` CI job): `cargo check --all-targets` on a
  pinned **1.88** toolchain, so the declared `rust-version` is actually enforced
  (the main `rust` job builds on stable and would otherwise mask a sub-MSRV
  feature). Adding this gate surfaced that the MSRV claim was already untrue —
  see Changed.
- **Rust supply-chain gate** (`cargo-deny` CI job): `cargo deny check` over the
  dependency tree — RustSec advisories, a license allow-list (MIT / Apache-2.0),
  duplicate/wildcard bans, and crates.io-only sources (config in
  `rust/deny.toml`). Its first run flagged a live advisory (see Security).
- **Rust `[lints]` table** in `rust/Cargo.toml`, so lint policy is enforced on a
  plain `cargo build` / `cargo clippy`, not just via CI flags:
  `unsafe_op_in_unsafe_fn` and `clippy::undocumented_unsafe_blocks` (the latter
  promotes the repo's `// SAFETY:` convention from a pre-commit grep to a real
  lint), plus `unreachable_pub`. Enforcing `unreachable_pub` tightened six
  crate-internal exit-code constants in `cli.rs` from `pub` to `pub(crate)`.

### Security

- **Upgraded `memmap2` 0.9.10 → 0.9.11** to resolve `RUSTSEC-2026-0186` (unsound
  unchecked pointer offset in `memmap2`, fixed upstream in 0.9.11), surfaced by
  the new `cargo-deny` gate. No API or behavior change (conformance unaffected).

### Changed

- **Rust MSRV raised from 1.85 to 1.88** (`rust-version` in `rust/Cargo.toml`).
  The sole dependency `memmap2` (≥0.9.7, and the locked 0.9.10) uses `let`-chains
  and so requires Rust 1.88, even though it under-declares its own MSRV as 1.63.
  The crate had therefore not actually built on 1.85 for several `memmap2`
  releases; 1.88 reflects the true floor. Edition 2024 itself still only requires
  1.85, so the L3 "MSRV 1.85 or newer" requirement remains satisfied.
- **Repository layout: the Rust crate now lives under `rust/`, mirroring
  `python/`.** The crate source (`src/`), integration tests
  (`tests/cli.rs`, `tests/integration.rs`), `Cargo.toml` / `Cargo.lock`, and the
  `.cargo/` coverage aliases moved from the repository root into `rust/`. The
  shared artifacts stay at the root: `config/default.toml`, the cross-impl
  `tests/conformance/` oracle, and `docs/`. **No runtime, CLI, or library-API
  change** — decoded output is byte-identical and the cross-implementation
  conformance contract is unaffected. Building from source now runs from the
  crate directory (`cd rust && cargo build`; cargo's `-C` flag is still
  unstable); CI, the pre-commit hook, the coverage wrapper
  (`scripts/coverage.sh`), the conformance runner, and the trace-matrix
  generator were updated to match, and documentation/source cross-references
  that named root paths (`src/…`, `tests/…`) were re-rooted under `rust/`.

- **Internal: decomposed the Python `_run_decode` CLI handler into focused
  helpers** (override building/validation, reader opening, message-stream
  building, writing, and the success/error exit-code classifiers), mirroring the
  existing decomposition of `run_decode` in `rust/src/cli.rs` and cutting the
  function's cognitive complexity well under the SonarCloud threshold. **No
  behavior change** — exit codes, stderr, and decoded output are identical
  (conformance unaffected); new direct unit tests (`python/tests/test_cli.py`)
  raise `cli.py` coverage.

### Documentation

- **Per-implementation READMEs.** Added `rust/README.md` and `python/README.md`
  holding the language-specific build, library-usage, development, and structure
  sections. The root `README.md` is slimmed to the shared content (project
  overview, CLI reference, configuration, error handling, supported formats) and
  links both implementation READMEs from the overview, the new Building section,
  Project Structure, and Development. `rust/README.md` also resolves the dangling
  `readme = "README.md"` in `rust/Cargo.toml`; `python/README.md` remains the
  package long-description referenced by `python/pyproject.toml`.

## [2.3.0] — 2026-06-23

### Added

- **MUX column populated from the input file name (L2-WRT-020, both
  implementations, behavior change).** The previously-always-empty `MUX` CSV
  column is now filled from a field of each input's file name, so a decoded CSV
  can carry the source/recorder identity that operators encode in the name (e.g.
  `full_loadout.draw.data.1553.aa.unused.mie_irig` → `MUX = aa`). The basename
  is split on a configurable delimiter (default `.`) and a configurable 0-based
  field index (default `4`; negative counts from the end) selects the value; an
  out-of-range/empty field leaves MUX empty. In a multi-file merge each row
  carries the MUX of the file it was decoded from. Configurable via the new
  `[mux]` config section (`enabled` / `delimiter` / `field`) and the new
  `--no-mux` / `--mux-delimiter` / `--mux-field` CLI flags (identical in both
  CLIs). Extraction is hand-rolled (delimiter + index, **no new dependency**).
  Pinned by the `mux-from-filename`, `mux-merge-per-file` (per-file values
  through the merge), and `mux-disabled` conformance cases plus unit tests.
  - **Behavior / vendor-compat change:** because this is **on by default**,
    default output is no longer byte-for-byte identical to the DDC vendor CSV
    when the input name carries the field. Pass **`--no-mux`** (or set
    `[mux] enabled = false`) to restore empty MUX for a vendor-exact diff. The
    other placeholder columns (`TERM_NAME`, `IM_GAP`, `RCV_GAP`, `XMT_GAP`)
    remain empty; the `MUX` column position is unchanged. `L2-WRT-013`,
    `VENDOR-CSV-DIFFS.md`, and the "Shared Commitments" / "Conventions" notes
    were updated accordingly.

### Fixed

- **Standard-timestamp mode-code records with data are no longer misclassified
  (L2-MSG-004, both implementations, behavior change).** The mode-code
  data-vs-no-data classifier used absolute word-count thresholds that assumed an
  IRIG timestamp (3 timestamp words): broadcast data iff `word_count > 5`,
  receive data iff `word_count >= 7`. A Standard timestamp is only 2 words, so a
  Standard broadcast-with-data record (`word_count = 5`) and a Standard
  receive-with-data record (`word_count = 6`) were classified as *no-data* — the
  data word was emitted under `STAT` instead of `WD01`. The thresholds are now
  derived from the resolved timestamp word count (broadcast data iff
  `word_count >= timestamp_words + 3`, receive data iff
  `word_count >= timestamp_words + 4`), so classification is correct under both
  formats. **IRIG output is byte-identical** (the IRIG thresholds are unchanged);
  only Standard mode-code-with-data records change. Pinned by the new
  `standard-mode-code-data` conformance case (the data word now appears in
  `WD01`) plus Standard-timestamp classifier unit tests in both implementations.

### Documentation

- Corrected the Rust coverage gate in `CONTRIBUTING.md`: it stated the
  `cargo cov-ci` floor was 70% line / 70% region, but the enforced value in
  `.cargo/config.toml` is **84% line / 83% region**. Updated the gate references
  and the stale headroom rationale.
- Corrected the look-ahead description in `docs/MIE-FORMAT.md`: it claimed the
  following Type Word during look-ahead "must also pass checks 1–4", but
  L2-SYN-005 and the implementation (`src/sync.rs`) only require a **plausible
  Type Word** (known message type + in-range word count). Reworded to match the
  normative requirement and the code.

## [2.2.0] — 2026-06-22

### Added

- **Within-file monotonicity detection in the multi-file merge (L2-MRG-006,
  both implementations, identical).** The time-sorted merge assumes each input
  is internally chronological (capture order). It now verifies this: when a
  record's absolute IRIG microsecond key steps strictly backward relative to the
  previous record from the **same** input (caused by sync-loss recovery or a
  day/year rollover), the merge detects it in O(1) per record. In the default
  (lenient) mode it logs a WARN once per offending input — naming the file and
  the backward step — and still emits every record in heap order (it never
  re-sorts, preserving the O(number-of-files) streaming guarantee). Previously
  such an input produced silently out-of-order merged rows with no diagnostic.
  A new `merge-non-monotonic-within-file` conformance case pins the cross-impl
  WARN + byte-identical output. A single-file decode is unaffected (the merge
  path only runs for two or more inputs).
- **The Python `decode` CLI gains `--strict` and `--format`, completing
  argument-surface parity with the Rust CLI.** `--strict` (raise on invalid
  records) was previously settable only via `[decode] strict = true` in a config
  file on the Python side, even though the Rust CLI and the docs already exposed
  it as a flag; it is now a `decode` flag in both, overriding the config like
  every other CLI override. `--format` (output format; `csv` only at present)
  likewise matches Rust: `csv` is accepted and any other value is a **runtime
  error (exit 1)** applied after config load, distinct from the config-file
  `output.format` validation (a config error, exit 5) that both already shared.
  With these, both CLIs accept an identical set of flags across `decode`,
  `count`, and `dump`. The `merge-non-monotonic-strict` conformance case now
  drives strict via the shared `--strict` arg (rather than a config file),
  directly proving both CLIs accept it.

### Changed

- **Strict mode now fails a merge whose input is not internally time-sorted
  (behavior change, both implementations).** Under `--strict` or
  `[decode] strict = true`, a within-file backward timestamp step
  (L2-MRG-006, above) is treated as a record error — `NonMonotonicInput` /
  `MieNonMonotonicInputError`, mapped to **exit 1** (the runtime/decode-error
  class, like other strict record failures) — rather than being silently
  accepted. Lenient mode (the default) is unchanged apart from the new WARN.
  Pinned by the `merge-non-monotonic-strict` conformance case.
- **The Rust `decode --help` now documents `--manifest` and `--glob`** (and
  shows `decode <INPUT>...` for the multi-input merge). Both flags were already
  accepted by the parser since v2.1.0 but were missing from the help text; no
  behavior change.

### Fixed

- **Python: `[filter] exclude_types` / `include_types` now accept integer codes
  and bound-check them, matching Rust (cross-impl parity).** A TOML config with
  a bare integer type code (e.g. `exclude_types = [2]`) previously crashed the
  Python CLI with an uncaught `AttributeError`, while the Rust CLI accepted it;
  Python now accepts integer codes (bounded to `0..=255`) like Rust. An
  out-of-range code (e.g. `"0x100"`) was previously **silently accepted** by
  Python — making the filter a no-op — whereas Rust rejected it; Python now
  rejects it too (a CLI usage error → exit 4, or a config error → exit 5),
  matching Rust. **Python: a non-string `[filter] exclude_buses` entry** (e.g.
  `exclude_buses = [1]`) likewise crashed with an uncaught `AttributeError`; it
  now raises a clean config error (exit 5) naming the problem, as Rust does.
  Found by a cross-implementation parity audit. Pinned by the new
  `config-exclude-types-int` (int code == name, byte-identical) and
  `usage-error-exclude-types-out-of-range` (exit 4 both impls) conformance
  cases, plus Python unit tests.

### Maintenance

- **CLI flag-surface parity is now gated against drift.** The conformance
  runner (`tests/conformance/run.py`) gained a `check_cli_surface` step that
  extracts the full long-flag set from each CLI's `--help` (top-level +
  `decode` / `count` / `dump`) and fails the run — naming the offending flag —
  if the Rust and Python surfaces diverge. A flag added to only one
  implementation now fails CI even when no conformance case exercises it. (A
  one-word Rust `--help` reword — `--flag=value` → `=value` — removes a phantom
  token so the check is exact.)

## [2.1.0] — 2026-06-21

### Added

- **Multi-file, time-sorted merge (L1-MRG, both implementations, identical).**
  The `decode` command now accepts more than one input and emits a single CSV
  whose records are in global time order. Inputs are supplied by **multiple
  positionals** (`decode a.mie b.mie -o out.csv`), a **`--manifest <file>`**
  (one path per line; blank lines and `#`-comments ignored), or a
  **`--glob <pattern>`** (single-directory `*`/`?` filename match, expanded by
  the tool so it works on Windows); the three methods are **mutually exclusive**.
  A single input behaves exactly as before. The merge is a streaming k-way heap
  merge keyed on absolute IRIG microseconds, so resident memory stays O(number
  of files) and O(1) in record count, and DELTA is recomputed on the unified
  timeline (L2-MRG-005). Time-sorted merge requires every input to be
  calendar-locked IRIG: a Standard-format input, a freerun-leading input, or a
  mixed-format set is rejected before any output with a new **exit code 6**
  (L1-EXIT-009 / L2-MRG-003). More than `MAX_MERGE_FILES` (256) inputs, or
  combining input methods, is a usage error (exit 4). Per-file failure follows
  the existing `--strict` / lenient / `--allow-partial` policy across the batch:
  `--allow-partial` truncates a failed file, completes the merge from the rest,
  and writes the combined output as `.partial` (L2-MRG-004). Rust uses
  `std::collections::BinaryHeap` and a hand-rolled glob matcher; Python uses
  `heapq` — no new dependency in either. Output is byte-identical across the two
  implementations (pinned by the `merge-ordered` conformance case).

## [2.0.1] — 2026-06-21

### Added

- **L2-SYN-027: RT-to-RT Command-Word `data_word_count` agreement check (both
  implementations).** An RT-to-RT or RT-to-RT-broadcast record whose two
  Command Words declare different `data_word_count` values is now rejected as
  corruption: strict mode surfaces a record error, lenient mode logs a WARN and
  skips the record. The bus protocol carries a single count for the transfer
  (`docs/MIE-FORMAT.md` §6.3), so a mismatch is internally inconsistent. This is
  a post-extract check mirroring the sibling L2-SYN-023 (Cmd2 direction).
  **Behavior change:** 2.0.0 silently accepted such records (emitting truncated
  data); they are now rejected. Valid DDC recordings always agree, so
  conformance is unaffected.

### Changed

- **The record-aware `dump` now logs its scan-stop anomalies (L2-CLI-013, both
  implementations).** Invalid `word_count`, truncated-record, and (Rust)
  offset-overflow stops are emitted through the logger at `WARN` — to stderr,
  subject to `--log-level` — in addition to the existing inline `!! …` note in
  the hex report. This makes the dump's diagnostics consistent with the
  reader's and visible on the normal log channel; the hex-report format is
  unchanged.

### Fixed

- **Reader: RT-to-RT payload extraction could read past the record extent
  (Python).** For RT-to-RT and RT-to-RT-broadcast records, the data-word count
  comes from the second Command Word (Cmd2), but the L2-SYN-022 capacity
  invariant is computed from Cmd1. A malformed record with a small Cmd1 count
  (passing the capacity check) and an over-claiming Cmd2 caused
  `_extract_payload` to read beyond the Type Word's declared extent — into the
  following record, or past EOF as a `struct.error` (caught by the L1-ROB-001
  fuzz harness). Payload reads are now bounded to the record extent so an
  over-claim can no longer overrun, matching the Rust reader (L2-DEC-009); the
  record is then rejected by the new L2-SYN-027 invariant. The Rust
  implementation was already bounded; this brings Python to parity. New
  regression tests in both implementations
  (`rt_to_rt_cmd2_overclaim_does_not_overrun`) plus an arbitrary-bytes
  robustness test for the `dump` subcommand in both implementations
  (`dump_arbitrary_bytes_never_panics`).

## [2.0.0] — 2026-06-18

A joint Rust + Python major release whose theme is **parity**: the two
tools now function the same way. The Python CLI gained the capabilities and
the exact argument surface of the Rust v2 CLI, and the Python writer now
streams in constant memory like Rust. Breaking changes are confined to the
Python CLI and the Python library API (see **Removed**); CSV and count output
are byte-for-byte unchanged, and the Rust CLI is unchanged. Both
implementations ship from the single tag `v2.0.0`.

### Added

- **Include filters in the Python CLI** — `--include-types`, `--include-rts`,
  `--include-buses`, `--include-subaddresses`, the positive complement of the
  exclude filters and the last filtering capability Python lacked. A message
  passes only if it matches no active exclude set and is contained in every
  active include set; SPURIOUS_DATA (no RT/SA) is dropped when an RT/SA include
  filter is active. Include filters are CLI-only overrides (no config-file
  key), matching Rust. Pinned by the new `L3-PY-013`; `L3-RS-010` was reworded
  from "Python is not required to expose equivalent CLI syntax" to require it.
- **A `count` subcommand in the Python CLI** (`mie-decoder count rec.mie`),
  matching the Rust `count` subcommand. Counts valid records after the config
  file's `[filter]` section, printing the integer to stdout and a status line
  to stderr (`L3-PY-010`).
- **The Python package root now exposes its decoder entry point** —
  `from mie_decoder import MieFileReader` (and `MieMessage`) now works without
  reaching into submodules, advertised via `__all__`. Previously the package
  root exposed only `__version__`, so `L3-PY-007` ("expose the decoder entry
  point as a typed callable importable from the package root") was unsatisfied
  in code and traced only through the conformance-runner requirement rather
  than a real root-API check. The re-export is additive (submodule paths are
  unchanged), and `L3-PY-007` now traces to a dedicated root-API test
  (`tests/test_package_api.py`); its verification method moved from Inspection
  to Test + Inspection. The requirement was also re-parented off the
  conformance-wiring requirement (`L2-CONF-002`) onto a new public-API-surface
  requirement, `L2-CONF-006` ("each maintained implementation SHALL expose a
  documented public library API with its decode entry point importable from
  the package/crate root", under `L1-CONF-001`), with a Rust counterpart
  `L3-RS-013` verifying the crate-root `pub use` re-exports — so both
  implementations' library surfaces are now pinned and tested.

### Changed

- **The Python CLI now shares one identical argument surface with the Rust
  CLI.** `--inline-errors` (a boolean flag; separate is the default) replaces
  `--error-mode {separate,inline}` (`L3-PY-011`); `--config` is now a global
  option placed *before* the subcommand
  (`mie-decoder --config site.toml decode rec.mie`) rather than a
  per-subcommand flag; and every filter flag takes one comma-separable,
  repeatable value (`--exclude-rts 15,31` ≡ `--exclude-rts 15 --exclude-rts 31`)
  instead of space-separated `nargs`, with RT/SA values bounded to u8 (0–255)
  exactly like Rust. The cross-implementation conformance suite dropped its
  per-impl argument translation: a single `args` vector now drives both CLIs,
  so the byte-for-byte conformance cases are a direct proof that the two tools
  accept the same arguments.
- **PY-streaming: the Python writer now streams in constant memory.** Both
  `write_csv` and `write_csv_split` previously collected every row into a list
  and materialized a full `pandas.DataFrame` before flushing, making Python
  decode memory `O(record_count)` — a multi-GB recording could exhaust RAM
  while the Rust CLI streamed the same input in constant memory. The writer
  now streams each row straight to the output handle through the
  standard-library `csv` module via two new primitives — `_AtomicCsvFile`
  (temp-file + `os.replace`, with `commit()` / `commit_partial()` /
  cleanup-on-failure) and `_StreamingCsvRowWriter` — ported from the Rust
  `AtomicCsvFile` / `CsvWriter` shapes. Python decode memory is now `O(1)` in
  the record count, matching Rust (`L3-PY-012` reworded from `O(record_count)`;
  verification raised from Inspection to a `tracemalloc` memory test). CSV
  output is unchanged, pinned by a new byte-exact golden characterization suite
  and the full conformance suite.

### Removed

- **The Python `decode --count` flag** (use the `count` subcommand) and the
  **`--error-mode` flag** (use `--inline-errors`; separate is the default).
  Python filter flags no longer accept space-separated values
  (`--exclude-rts 15 31`) — use commas or repeat the flag — and `--config` is
  no longer accepted after the subcommand.
- **The `pandas` runtime dependency**, leaving `tomli` (Python 3.10 only) as
  the Python package's sole runtime dependency — the same dependency-light
  story as the Rust crate.
- **The public Python helpers `mie_decoder.writer.messages_to_dataframe` and
  `dataframe_to_csv`.** Build a DataFrame from the public message stream
  instead: `pandas.DataFrame(map(message_to_row, MieFileReader(path)))`.

### Fixed

- **`logging.level = "OFF"` no longer crashes the Python CLI; it now silences
  all output, matching Rust.** Both implementations accepted `OFF` at config
  load, but Python then raised an uncaught `ValueError` when applying it
  (stdlib `logging` has no `OFF` level), while Rust correctly mapped it to
  "silence all" (`Level::Off`). Python now maps `OFF` to a level above
  `CRITICAL`, so a config with `logging.level = "OFF"` decodes cleanly and
  silently in both implementations. `OFF` was also added to the normative
  L2-CFG schema table, `CONFIG-REFERENCE.md`, `config/default.toml`, and both
  implementations' "invalid level" error messages (which under-reported the
  accepted set — Rust's also omitted `WARN`). Corrected the docs that claimed
  `CRITICAL` "behaves the same as `ERROR`": the decoder emits no
  `CRITICAL`-level messages, so `CRITICAL` (like `OFF`) suppresses all output.
  A new `log-level-off` conformance case pins the cross-impl behavior.
- **The `--log-level` CLI flag now accepts the same level set as the config
  file in both implementations.** The Python CLI previously accepted only
  `DEBUG`/`INFO`/`WARNING`/`ERROR`/`CRITICAL` and was case-sensitive (rejecting
  `WARN`, `OFF`, and lowercase like `debug`), while the Rust CLI already
  accepted all seven case-insensitively but its `--help` and invalid-value
  message under-reported the set (omitting `WARN`/`OFF`, and `CRITICAL` in the
  help). Both CLIs now accept `DEBUG`/`INFO`/`WARNING`/`WARN`/`ERROR`/
  `CRITICAL`/`OFF` case-insensitively (matching `logging.level`), with help,
  invalid-value text, and the README aligned. The Python change is additive
  (a superset of what it accepted before). `--version` and `--help` are also
  now honored even alongside an invalid `--log-level` in both CLIs (Python
  previously failed on the bad flag before reaching `--version`/`--help`); the
  level is validated after those flags short-circuit, matching Rust.

## [1.5.1] — 2026-06-15

### Changed

- **Separate-mode output now commits the main CSV before the errors CSV in
  both implementations.** Rust previously committed errors-then-main while
  Python committed main-then-errors — a cross-impl divergence in the
  mid-commit failure residue. Aligned Rust to Python's main-first order so
  that, since the two files are committed sequentially (each is atomic on its
  own, but there is no cross-file atomic rename), a failure of the second
  commit leaves the **primary `main.csv`** behind rather than an orphan
  errors file, and a failure of the first commit leaves neither file. This
  only affects the rare partial-failure path; successful writes are
  unchanged, as is all CSV content. The order is now pinned by a new
  requirement (`L2-WRT-019`) and verified by mid-commit-failure tests in both
  implementations (the failing commit is forced by making the destination a
  directory).

### Fixed

- **Corrected the Python `count` help and README, which claimed the count is
  printed to stderr.** Both implementations print the integer count to
  **stdout** (the machine-readable datum) and only the human-readable status
  summary to stderr (`L3-PY-010` / `L3-RS-008`); the Rust help and the
  `count-one` conformance oracle were already correct. Fixed the
  `--count` flag help string and the README `count` description to match.
- **Completed the reference configuration `config/default.toml`.** The file
  is advertised as a fully-commented starter config, but omitted four
  documented, parsed keys: `decode.allow_partial`, `decode.detect_records`,
  `decode.lookahead_records`, and `output.no_clobber`. Added all four with
  commented descriptions, valid ranges (`[1, 32]` for the two record-count
  knobs), CLI-override notes, and their default values, so the starter file
  now covers every key in `CONFIG-REFERENCE.md`. Guarded by a new test in
  each implementation: a completeness check that the file mentions every
  documented key (Rust) and a parity check that the shared file loads with
  the documented defaults (both Rust and Python).
- **The Rust `dump` subcommand no longer reports success after an output
  write failure.** `hex_dump_raw` / `hex_dump_records` discarded every
  `writeln!` / hex-line result with `let _ =` and unconditionally returned
  `Ok(())`, and the stdout `BufWriter` swallowed flush errors on drop — so a
  dump whose output hit disk-full or a permission error (e.g.
  `dump > out.txt`) exited `0` with truncated or empty output, violating
  `L2-WRT-018`. The writers now propagate every write and an explicit final
  flush as a `WriterError`; the CLI surfaces it as a runtime failure (exit
  `1`) while still treating a broken pipe on stdout (`dump | head`) as a
  clean exit `0`. The Python `dump` already propagated via `print`, so this
  aligns Rust to the existing behavior. Covered by new write-failure and
  broken-pipe tests in both `dump.rs` and `cli.rs`.
- Corrected a false atomicity guarantee in the docs and source comments for
  separate-mode output. `ARCHITECTURE.md` §8 previously claimed the main and
  errors CSVs "both either succeed atomically or neither appears" — implying
  cross-file atomicity that does not exist. Rewrote the §8 note and the
  misleading `src/writer.rs` commit-ordering comment (whose stated rationale
  was backwards) to describe the per-file, main-first guarantee honestly.

### Documentation

- Fixed two factually-wrong source comments / doc descriptions. (1) The
  `src/reader.rs` mmap `SAFETY` comment claimed the file is "moved into the
  closure" and that "the mmap holds it alive" — there is no closure, and
  `Mmap::map(&file)` borrows the file rather than owning it; rewrote it to
  state the real contract (the OS mapping outlives the dropped `File`; the
  input must not be mutated while mapped, per `L1-EXIT-006`). (2) Several
  reader/sync module docs and `MIE-FORMAT.md` still described a *fixed*
  "two-record look-ahead" although the depth has been configurable
  (`N`-record, default 2) since `L2-SYN-026`; reworded them (and the
  `CLAUDE.md` / `CONTRIBUTING.md` preservation notes) to say "N-record
  look-ahead (default 2)".
- Removed stale hardcoded conformance-suite case counts from the reference
  docs, extending the `9b47121` "no drift-prone counts" policy to the docs
  that earlier cleanup missed. `ARCHITECTURE.md` (§1 and the conformance
  section), `USER-GUIDE.md`, and `VENDOR-CSV-DIFFS.md` cited "19-case" /
  "20-case" suites — both wrong (the suite has grown) and guaranteed to
  re-stale each release — so they now refer to the conformance suite
  generically; the live count lives only in `tests/conformance/manifest.json`.
  Also reworded a drift-prone "`[Unreleased]` is empty as of the v1.3.0 cut"
  note in `ROADMAP.md` to be version-agnostic. (The `README.md` and
  `MAINTAINER-GUIDE.md` locations flagged in review were already clean;
  ROADMAP's historical per-release case counts are intentionally kept.)
- Scoped the "IRIG day-field decoding across DDC card models" ROADMAP item
  (Decode correctness) as **blocked on external data**: recorded what is
  already known (the bits 13–5 binary slice is per-spec; only day-of-year
  diverges, only on some card models), the sample set required to make
  progress (recording + vendor CSV + true date + model/firmware id per
  card model), and the diff-and-solve method for when ground-truth data is
  available. No behavior change — the v1.5.0 advisory WARN remains the
  interim treatment.
- Designed the **multi-file time-sorted merge** Planned feature (Rust v1.x)
  and documented how it works. `ROADMAP.md` carries the full design — the
  streaming k-way (min-heap) merge that keeps memory O(file-count) rather
  than O(record-count), the IRIG microseconds-from-start-of-year merge key,
  the hard absolute-time and single-year constraints (Standard counters /
  freerun IRIG / cross-year inputs are not cross-file orderable), the
  file-local error-classification requirement, deterministic tie-breaking,
  and the open CLI/failure-policy decisions. `ARCHITECTURE.md` §12 explains
  the streaming merge mechanism (clearly marked not-yet-implemented) so the
  memory model is understood before the feature lands. No behavior change —
  design/docs only.

## [1.5.0] — 2026-06-15

### Added

- One-time IRIG day-of-year advisory (ROADMAP PRA-9). Both readers now emit
  a single WARN per decode the first time a calendar-locked (non-freerun)
  IRIG record is decoded, pointing to the documented day-of-year
  firmware-discrepancy limitation (`docs/VENDOR-CSV-DIFFS.md` §5). Advisory
  only — not a decode failure; freerun records don't trigger it and
  `--log-level ERROR` silences it.
- Targeted `L2-DEC-009` payload-bounding test in both implementations
  (ROADMAP PRA-8): an over-declaring record before a valid one is rejected
  in strict mode and skipped in lenient mode, with the following record
  decoded intact at its true offset — proving extraction never overruns
  into the next record. `L2-DEC-009` is now Test + Inspection verified.
- Scheduled fuzz burn-in (ROADMAP PRA-5). The L1-ROB-001 no-panic
  harnesses now honor a `MIE_FUZZ_ITERATIONS` override (default 256,
  deterministic), and a new `.github/workflows/fuzz.yml` runs them daily
  (and on manual dispatch) at 25 000 iterations per implementation. Added
  an explicit `L1-SYN-002` cumulative-scan-bound test in both impls
  (`recovery_scan_is_forward_only_and_bounded`) asserting that repeated
  recoveries advance strictly forward and never re-traverse already-scanned
  bytes.

### Fixed

- **Forcing the wrong `--time-format` no longer silently emits garbage
  timestamps** (ROADMAP PRA-2, implements the previously-unimplemented
  half of `L2-DEC-013`). When an explicit `--time-format` /
  `decode.time_format` contradicts the recording — the detection probe is
  *Decisive* for the other format — strict mode now raises a
  timestamp-format mismatch (exit `2`) and lenient mode logs a WARN and
  proceeds with the forced format. Marginal/ambiguous recordings are not
  flagged, so an intentional override of a misdetection still works.
  Verified by new forced-mismatch tests in both implementations and the
  `forced-format-mismatch-strict` conformance case.

### Changed

- **CLI exit codes are now a granular, cross-impl-identical taxonomy**
  (ROADMAP PRA-1). Behavior change: CLI **usage errors** (unknown/invalid/
  missing flag or argument, bad flag value, no subcommand) now exit **4**,
  and **configuration errors** (config file missing, malformed, or invalid
  value) now exit **5**. Previously Rust exited `2` for usage errors
  (colliding with the no-records class) and Python was inconsistent
  (`1` vs `2`). The shipped meanings of `0`, `2` (no records), and `3`
  (unrecoverable sync loss) are unchanged; `1` is now specifically the
  runtime/decode-error class. `count`/`dump` inherit `0/1/2/4/5` but never
  `3`. Both implementations return identical codes for the same condition,
  verified by the new `usage-error-bad-flag-value` (4) and
  `config-error-invalid-value` (5) conformance cases. Spec: new
  `L1-EXIT-007`/`L1-EXIT-008` and the revised `L2-CLI-011` table; docs:
  `ERROR-CATALOG.md`, `USER-GUIDE.md`, `EXAMPLES.md`.

### Documentation

- Flagged the CHANGELOG compare-URL footer (version-bump checklist step 2)
  as the most-often-missed release step in `MAINTAINER-GUIDE.md` §11. It was
  silently skipped on the `1.4.0` and `1.4.1` cuts — the footer's
  `[Unreleased]` link kept pointing at `v1.3.0...HEAD` with no `1.4.0`/
  `1.4.1` entries — and was repaired during this cut. Added a concrete
  post-edit cross-check against `git tag --sort=-creatordate`.
- Surfaced the Python large-file memory ceiling for operators (ROADMAP
  PRA-4). New `USER-GUIDE.md` §10 "Performance and large recordings" gives
  the per-implementation memory table, the "~5 GB RAM per ~10 M records"
  planning rule, and the recommendation to use the Rust CLI for multi-GB /
  10M+-record recordings (byte-identical output). `PY-streaming` (the
  constant-memory Python writer that will remove the ceiling) is now an
  explicit Planned entry and the next Python work item.
- Trace-matrix generator now surfaces and counts L1-level test markers
  (ROADMAP PRA-3). The "L1 → L2" table gained a Test Artifacts column, so
  direct-L1-marked leaves (e.g. `L1-ROB-001`'s fuzz harness) list their
  tests instead of appearing untested, and the coverage denominator folds
  in the Test-verifiable L1 *leaves* (composite L1s stay excluded to avoid
  double-counting their L2/L3 children). Headline moved 118/134 → 124/140
  tested, 100% verified.
- Added a Production-Readiness Audit backlog (`PRA-1`–`PRA-9`) to
  `docs/ROADMAP.md`, capturing findings from a comment/docs hygiene sweep
  and a requirements deep analysis: CLI exit-code taxonomy alignment (open
  decision), the unimplemented `L2-DEC-013` forced-format validation,
  trace-matrix L1-marker coverage, Python large-file memory limits and
  `PY-streaming`, a fuzz CI burn-in plus the `L1-SYN-002` cumulative-scan
  test, normative-doc/source-comment staleness, and lower-priority
  test/diagnostic items. Items are unscheduled — no target version is
  assigned until each is planned for closure.
- Cleared version-anchored source comments (ROADMAP PRA-7): removed the
  "v2 redesign" framing from the `src/cli.rs` / `src/filter.rs` module
  docs, and reworded the "empty in v1.0" / "csv for v1.0" anchors in
  `python/.../writer.py`, `python/.../config.py`, and `src/config.rs` to
  describe current behavior — the empty vendor columns now cite
  `L2-WRT-013` and the output-format notes read "currently only csv".
- Removed drift-prone release versions and hardcoded counts from
  `CLAUDE.md`, `README.md`, and the requirements docs (ROADMAP PRA-6).
  These now live only in their source of truth — the conformance suite
  for case counts, the requirements docs / `TRACE-MATRIX.md` for
  requirement counts, and `git tag` / `CHANGELOG.md` for versions.

## [1.4.1] — 2026-06-14

Joint Rust + Python maintenance release: close the CI dev-tool gap, tighten
the coverage gates, and clear stale comments. No public API or decode-output
changes. Both implementations ship together from the `v1.4.1` repository tag.

### Added

- A `mypy` CI job and dev dependency. `python/pyproject.toml` declared
  `[tool.mypy] strict = true` but nothing installed or ran it; strict
  type-checking is now gated in CI (`poetry run mypy src`).

### Fixed

- Latent crash in the Python filter: `apply_filters` dereferenced
  `command_word.rt` / `.subaddress` on records with no Command Word
  (SPURIOUS_DATA), raising `AttributeError` whenever such a record reached
  the filter. RT/subaddress filters now treat a missing Command Word as
  "no match" (excludable only by type or bus), matching the Rust filter.
  Surfaced by the new strict mypy gate.

### Changed

- Coverage gates ratcheted from baseline-5pp to baseline-2pp: Rust
  `cov-ci` to 84% line / 83% region (from 70/70), Python `fail_under` to
  88% combined line+branch (from 85%, now config-driven in
  `[tool.coverage.report]`).
- mypy-strict cleanups across the Python package: a shared `ByteSource`
  buffer alias for the `mmap`-backed decode/sync helpers, an explicit
  `TextIO` stream type in `dump`, removal of stale `# type: ignore`
  comments, and minor annotation fixes. No runtime behavior change.

### Removed

- Stale "Deferred (Phase 7b)" comments in `decode.rs` / `decode.py` (the
  RT-to-RT Cmd2-direction and anomaly invariants shipped as
  L2-SYN-023/024/025) and a stale SPURIOUS_DATA "raises ValueError"
  docstring (0x20 classifies as `SPURIOUS_DATA`).

## [1.4.0] — 2026-06-14

Joint Rust + Python feature release. Adds opt-in **Standard-timestamp
tick calibration**: when an operator supplies the card's free-running
counter frequency, Standard-format records are converted to microseconds
and participate in `DELTA` tracking like IRIG records. Without a rate,
behavior is unchanged (empty `DELTA`), so all existing CSV output stays
byte-identical. Both implementations ship together from the `v1.4.0`
repository tag.

### Added

- New `decode.standard_tick_rate_hz` TOML key and `--standard-tick-rate-hz`
  CLI flag (both implementations). When set to a finite value `> 0`,
  Standard timestamps convert to microseconds as
  `round(raw_ticks × 1_000_000 / rate)` (half-away-from-zero, identical
  across implementations) and join per-RT/MSG `DELTA` tracking
  (L2-DEC-017, L2-CFG-011, L2-CLI-012).
- Float value support in the Rust hand-rolled TOML parser (`TomlValue::Float`,
  `TomlDoc::get_float`), required by the new key. No new crate dependency.
- Two cross-implementation conformance cases — `standard-tick-calibrated-cli`
  and `standard-tick-calibrated-toml` — sharing one oracle to prove the CLI
  and TOML paths produce byte-identical calibrated output.

### Changed

- L2-RDR-019 generalized: Standard-format records have an empty `DELTA`
  only when no tick rate is configured; with a valid rate they participate
  in `DELTA` on the same terms as IRIG. `Timestamp::to_microseconds` /
  `Timestamp.to_microseconds` now take an optional Standard tick rate.

## [1.3.0] — 2026-06-11

Joint Rust + Python hardening release. Adds precise sync-validation
failure APIs and strict-mode diagnostics, bounded DEBUG context logging,
production Rust unwrap/expect linting, Rust LCOV artifact publishing,
and complete verification coverage for all 131 active requirements.
Both implementations ship together from the `v1.3.0` repository tag.

### Added

- Additive detailed sync-validation APIs in both implementations:
  Rust `sync::validate_record_detailed(...) -> Result<(), ValidationFailure>`
  and Python `sync.validate_record_detailed(...) -> ValidationFailure | None`.
  Existing boolean `validate_record(...)` APIs remain unchanged.
- DEBUG-level validation context diagnostics capped at 32 bytes in both
  readers.
- Rust CI now uploads `lcov.info` as the `rust-lcov` workflow artifact.

### Changed

- Strict-mode IRIG-range and look-ahead failures now name the precise
  validation reason instead of the combined "IRIG-range or look-ahead"
  fallback detail.
- Rust production crates enable Clippy's `unwrap_used` and `expect_used`
  lints outside test builds; former production unwrap/expect sites now
  return defensive errors.
- Rust CLI acceptance coverage now pins both `--inline-errors` and the
  stdout-forces-inline behavior required by L3-RS-009.

### Removed

- Python's unconditional multi-line unknown-Type-Word stderr dump. The
  bounded DEBUG context diagnostic replaces it and respects log-level
  configuration.

### Maintenance

- Close the remaining partial traceability row with an L2-CONF-002
  conformance-runner wiring inspection test. All 131 active requirements
  are now verified.

## [1.2.0] — 2026-06-08

Configurable sync look-ahead with TOML + CLI controls, a Python
TOML `[logging] level` precedence fix, the new Python coverage
gate in CI (85% combined line+branch floor mirroring the Rust
70/70 model), retirement of the static-musl SLES 12 deployment
target, retirement of the `docs/FIELDS.md` redirect stub, and
three new cross-impl conformance fixtures (L2-DEC-015 borderline,
L2-DEC-016 lenient-mode WARN, L2-SYN-026 N>2 catches what N=2
misses). Both implementations ship together at v1.2.0 from a
single repository tag (`v1.2.0`), continuing the joint-cut model
established by v1.0.0.

### Added

- **Configurable N-record sync look-ahead** (L2-SYN-005, L2-SYN-026).
  `sync::validate_record` (Rust) / `sync.validate_record` (Python) now
  accept a look-ahead depth parameter `N`. The function checks `N − 1`
  subsequent records' Type Words after the candidate, advancing by each
  record's declared `word_count`. Default `N = 2` preserves the
  historical two-record look-ahead behavior; higher values catch wider
  classes of consecutive-same-shape corruption that previously defeated
  the validator (e.g., two adjacent fake-record headers that align on
  plausible Type Words). Configurable via `decode.lookahead_records` in
  TOML or `--lookahead-records N` on the CLI, range `[1, 32]`. See
  `docs/ARCHITECTURE.md` §3 Phase 3 for the design.
- New TOML key `decode.lookahead_records` with load-time range
  validation.
- New CLI flag `--lookahead-records N` with parse-time range
  validation, exposed in both the Rust and Python CLIs.
- **Python coverage gate in CI** (`python-coverage` job). Mirrors
  the Rust `cargo cov-ci` model: runs once on Linux/Python 3.12,
  not across the full matrix (coverage isn't platform- or
  interpreter-dependent). `pytest-cov ^7.0` added as a dev
  dependency; `[tool.coverage.run]` and `[tool.coverage.report]`
  sections added to `python/pyproject.toml` (branch coverage on,
  `__main__.py` excluded as the entry shim — parallel to Rust's
  `bin/mie-decoder.rs` exclusion). Floor is **85% combined
  line+branch**, set as `--cov-fail-under=85` in the CI job.
  Baseline at integration was 88.92% across the 245-case pytest
  suite, giving ~4 percentage points of headroom before drift
  starts failing the build (same ratchet model as the Rust 70/70
  floor's ~5pp headroom against its 74.81% baseline).
  `docs/MAINTAINER-GUIDE.md` §10 rewritten to cover both Rust and
  Python coverage workflows; cheat sheet (§3) gains the
  `poetry -C python run pytest --cov --cov-fail-under=85`
  invocation alongside `cargo cov-ci`; CI architecture table (§9)
  updated from five jobs to six.
- **Conformance fixture: L2-DEC-015 borderline detection** (two
  cases). `timestamp-format-borderline-default` and
  `timestamp-format-borderline-n1` share a hand-crafted 5-record
  input where the multi-record probe genuinely changes the format
  choice cross-impl: at `--detect-records 1` both impls pick
  Standard and decode 1 row; at default `--detect-records 8` both
  impls pick IRIG (Decisive) and decode 4 rows. The two oracles
  are byte-identical across Rust and Python, pinning the cross-
  impl behavior at each N.
- **Conformance fixture: L2-DEC-016 lenient-mode WARN**
  (`timestamp-format-ambiguous-lenient`). Reuses the (now-fixed)
  ambiguous input bytes with default (lenient) mode. Asserts
  exit 0 with a header-only CSV oracle, plus a stderr substring
  assertion that pins the lenient-mode WARN's score breakdown
  (`"Ambiguous: IRIG=4 STD=4"`). Companion to the strict
  fixture; together they pin both branches of L2-DEC-016
  cross-impl.
- **Stderr substring assertions on both L2-DEC-016 conformance
  fixtures.** Strict fixture now asserts
  `expected_stderr_contains: "auto-detection is ambiguous"`
  (substring unique to the `MieTimestampFormatMismatch` error
  message); lenient asserts the WARN substring as above. These
  defend against future regressions that exit with the right code
  via the wrong code path — the same class of bug-in-test the
  strict fixture had until v1.2.0's strict-fixture fix.
- **Conformance fixture: L2-SYN-026 N-record look-ahead value
  demonstration** (two cases). `lookahead-corruption-chain-n2`
  and `lookahead-corruption-chain-n4` share a hand-crafted
  5-record input (2 valid records, 32 bytes of 0xFF garbage, 1
  valid record at the end). At `--lookahead-records 2` both
  impls accept the file start, decode record 1, then sync-
  recover through the garbage and decode record 5 — 2 rows. At
  `--lookahead-records 4` both impls reject the file start
  (look-ahead chain reaches the garbage), scan forward, and
  accept only record 5 — 1 row. The contrast (2 rows vs 1 row)
  demonstrates the L2-SYN-026 value proposition: deeper look-
  ahead catches "valid prefix followed by corruption" patterns
  that defeat the default N=2 window. Both oracles byte-
  identical cross-impl.

Total conformance case count: 21 → 27 across this release.
Python test count: 242 → 248.

### Changed

- `docs/L2-REQ.md` L2-SYN-005 generalized in place: the original
  "two-record look-ahead" wording is now described as a special
  case of the configurable N-record rule with default `N = 2`.
  The generalization is non-breaking (existing files and configs
  continue to behave identically); the rationale for the
  in-place wording update is recorded in the L2-SYN-005
  Rationale field.

### Removed

- **Static-musl SLES 12 deployment target.** The Rust crate is
  no longer published with documentation or tooling for the
  `x86_64-unknown-linux-musl` cross-compile path. Native release
  builds (`cargo build --release`) are now the only documented
  artifact; deployers targeting older glibc hosts produce the
  static binary themselves out-of-tree if needed. Concrete
  changes: `docs/L3-REQ.md` L3-RS-007 marked *Withdrawn in
  v1.2.0* (the ID is reserved, not reused, so the trace matrix
  and historical references stay coherent); `README.md`,
  `CLAUDE.md`, `CONTRIBUTING.md`, `docs/MAINTAINER-GUIDE.md`,
  `docs/USER-GUIDE.md`, `docs/L1-REQ.md` rationale, and
  `.github/workflows/ci.yml` comments updated to remove musl /
  SLES references; historical CHANGELOG and ROADMAP entries
  describing the v1.0.0 musl scope preserved as-is. Trace matrix
  regenerated (active L3 count: 26 → 25; L3-RS subtotal:
  12 → 11).
- **`docs/FIELDS.md`** — the 3-line redirect stub kept for
  legacy external-link compatibility since the L2-DEC-015 /
  Documentation Initiative absorbed its content into
  `docs/MIE-FORMAT.md`. The stub has done its job; deleted. All
  active references repointed at `docs/MIE-FORMAT.md` directly
  or removed: `docs/L1-REQ.md` (L1-OUT-001), `docs/L2-REQ.md`
  (L2-DEC-002, L2-SYN-025 rationale, error-code family
  rationale) — repointed; `docs/MAINTAINER-GUIDE.md` (repo-tree
  listing), `CLAUDE.md` (Reference docs section), `README.md`
  (repo-tree listing) — stub row dropped; `docs/MIE-FORMAT.md` —
  the "absorbs FIELDS.md" note removed entirely and replaced
  with a direct "single source of truth" statement (no
  historical breadcrumb to the deleted predecessor). ROADMAP
  historical mentions of FIELDS.md (Documentation Initiative
  recap, deferred-audit notes, etc.) preserved as-is since they
  describe past state accurately.

### Fixed

- **Python: `[logging] level` in TOML config now honored**
  (regression of L2-CFG-003 precedence: CLI > TOML > default).
  `python/src/mie_decoder/config.py` was parsing
  `[logging] level` into `DecoderConfig.log_level` and
  validating it at load time, but `python/src/mie_decoder/cli.py`
  only called `configure_logging()` once at the top of `main()`
  with the CLI value (or `"WARNING"` default) — `_run_decode`
  then loaded the TOML but never re-applied the log level. Net
  effect: a TOML `[logging] level = "INFO"` was silently ignored
  unless the user also passed `--log-level` on the CLI. Rust
  applied the TOML value correctly via `resolve_config` in
  `src/cli.rs`. Fix introduces a small
  `_apply_config_log_level(args, config_log_level)` helper called
  immediately after `load_config(...)` in both `_run_decode` and
  `_run_dump`; it re-configures the logger from the TOML value
  when `--log-level` was not passed. The CLI value (when present)
  still wins because `main()` configured with it before the
  subcommand runner was entered. Also adds `--config` to the
  `dump` subparser so `mie-decoder dump file.mie --config
  foo.toml` can honor the TOML log level too (mirrors Rust, where
  `--config` is a global flag accepted by every subcommand). New
  Python e2e regressions in `python/tests/test_e2e.py`:
  `test_cli_toml_logging_level_is_honored_when_no_cli_override`
  (catches the original bug — fails without the fix),
  `test_cli_log_level_overrides_toml_logging_level` (pins the
  CLI-wins precedence), and
  `test_cli_dump_honors_toml_logging_level` (covers the
  dump-path fix). New cross-impl conformance fixture
  `log-level-from-toml-config` reuses the `basic-multi-record`
  input + oracle plus `configs/log-level-info.toml` and asserts
  the substring `"decode exit class"` appears on stderr — an
  INFO-level message both impls emit identically.
- **Conformance fixture `timestamp-format-ambiguous-strict` was
  exercising the wrong code path.** Discovered while adding the
  lenient-mode companion fixture. The fixture's input bytes had
  a Type Word declaring `word_count = 7` (14 bytes) but the
  actual hex block contained 16 bytes per record. The mismatch
  caused `find_first_record`'s look-ahead to land on filler
  bytes mid-record and reject the candidate, producing
  `MieNoValidRecords` (exit 2) instead of the intended
  `MieTimestampFormatMismatch` (also exit 2). The strict
  fixture's `expected_exit: 2` couldn't distinguish the two
  error paths, so the test passed for the wrong reason. Fix:
  change the Type Word's `word_count` field from 7 to 8 so the
  declared length matches the actual 16-byte record. The probe
  now reaches AMBIGUOUS classification correctly, and the
  strict fixture exercises the L2-DEC-016 path it was always
  meant to.

### Maintenance

- `docs/diagrams/dataflow.puml` `find_first_record` note
  updated: "(two-record look-ahead)" → "(L2-SYN-005 /
  L2-SYN-026; N defaults to 2, configurable via
  decode.lookahead_records)". The rendered
  `docs/diagrams/dataflow.svg` was regenerated to match
  (PlantUML 1.2026.5, matching the pin in the `diagrams` CI
  job).
- `docs/ROADMAP.md` refreshed for v1.2.0: v1.1.0 release-status
  entry added; "Queued for the next release" rewritten to
  summarize the full `[Unreleased]` contents (L2-SYN-026
  configurable look-ahead, FIELDS.md retirement, Python coverage
  gate, Python TOML log-level fix, three new cross-impl
  conformance fixtures, dataflow-diagram refresh); the two
  Robustness-backlog items resolved in v1.1.0 / v1.2.0 are
  struck through with their resolution commits; "Shared
  Commitments" text updated from "two-record look-ahead" to the
  N-record wording. A mid-cycle "Deferred follow-ups" section
  introduced during v1.2.0 development was removed before the
  release cut — every item it tracked shipped within v1.2.0.

## [1.1.0] — 2026-06-07

Stronger timestamp-format auto-detection via a multi-record probe,
plus a new ambiguous-detection error class. Both implementations
ship together at v1.1.0 from a single repository tag (`v1.1.0`),
continuing the joint-cut model established by v1.0.0.

### Added

- **Multi-record timestamp-format auto-detection** (L2-DEC-015). The
  IRIG-vs-Standard probe now walks up to *N* records (default `8`,
  configurable via `decode.detect_records` in TOML or `--detect-records N`
  on the CLI, range `1..=32`) and aggregates per-record scoring across the
  probe set rather than committing on the first record alone. The chosen
  format is still resolved before the first record is decoded and is final
  for the rest of the decode per L2-DEC-011 (no per-record re-detection).
  Strengthens detection on borderline files where the first record alone
  scores ambiguously between the two formats. See
  `docs/MIE-FORMAT.md` §5.3 for the per-record scoring signals and
  confidence thresholds.
- **`MieTimestampFormatMismatchError`** / `MieError::TimestampFormatMismatch`
  (L2-DEC-016). New file-level error variant raised when the L2-DEC-015
  probe completes with an aggregate score below the confidence floor
  (`max_score < 4`) OR a margin below `MIN_MARGIN = 3`. Strict mode only:
  lenient mode (the default) logs a single WARN with the score breakdown
  and proceeds with the chosen format, preserving backwards compatibility
  with borderline files that decoded acceptably under the previous
  single-record detection. Maps to CLI exit class `2` (`no-records`),
  same class as `NoValidRecords` and `HomogeneousPayload`.
- New TOML key `decode.detect_records` with load-time range validation.
- New CLI flag `--detect-records N` with parse-time range validation.

### Changed

- Auto-detection logs now include the per-format aggregated score
  breakdown plus the L2-DEC-016 confidence classification:
  - `Decisive` and `Marginal` outcomes log at INFO with score numbers
    plus a hint to `--time-format` on Marginal calls.
  - `Ambiguous` outcomes log at WARN (lenient) or ERROR + raise (strict).
- Conformance manifest schema validation in `tests/conformance/run.py` now
  checks field types in addition to field names. Rejects wrong scalar types
  (e.g. `"config": 12345`), wrong container types (e.g. `"rust_args": "a
  string"`), wrong list-element types (e.g. `"rust_args": [42]`), and invalid
  enum values (e.g. `"mode": "banana"`) with actionable error messages that
  name the offending field and the expected type.

### Fixed

- `tests/cli.rs::decode_emits_exit_class_summary_at_info_level` now surfaces
  in `docs/TRACE-MATRIX.md`. The L1 section of the matrix displays only L2
  children and rolled-up status, not direct L1 test markers, so the test's
  `L1-EXIT-005` tag alone was invisible. Added `L2-CLI-006` (stderr-only
  diagnostic obligation) to the `/// Requirements:` line — semantically
  correct (the summary line IS a human-readable stderr diagnostic) and the
  test now shows up under that L2's row.

### Maintenance

- `docs/MAINTAINER-GUIDE.md` §10 "220+ tests" updated to the actual count
  (236 as of v1.0.0).
- Spec additions: `L2-DEC-015` (multi-record probe) and `L2-DEC-016`
  (ambiguous-mismatch error class), both children of `L1-DEC-002`. See
  `docs/L2-REQ.md`.
- New conformance case `timestamp-format-ambiguous-strict` pins the
  cross-impl behavior on strict-mode ambiguous input: both Rust and
  Python raise their respective mismatch errors and exit `2` byte-for-byte
  equivalently. Conformance case count: 20 → 21.

## [1.0.0] — 2026-06-07

First joint release of the Rust crate and the Python package.
Both implementations ship from the same commit at v1.0.0.

### Highlights

- **Two implementations, one binary contract.** Rust crate at the
  repository root; Python package under `python/`. A 20-case
  conformance suite (`tests/conformance/`) holds the two to byte-exact
  CSV output (and matching exit code on negative cases).
- **DDC-vendor-compatible CSV.** Column names and ordering match DDC's
  own recording software output by spec (`L1-OUT-001`). See
  `docs/VENDOR-CSV-DIFFS.md` for the alignment statement and the five
  vendor-empty columns preserved as placeholders.
- **Streaming Rust writer (constant memory).** The Rust crate streams
  rows directly to a `BufWriter` — `O(1)` per record. Python remains
  pandas-buffered (`O(record_count)` memory) per `L3-PY-012`; a future
  Python-streaming feature is on the roadmap.
- **Static-musl Rust binary for SLES 12 deployment** via
  `x86_64-unknown-linux-musl`. Single self-contained binary, no glibc
  dependency.
- **Single external Rust dependency** (`memmap2`). Argument parsing,
  CSV writing, TOML loading, logging, and error types are all
  hand-rolled — see `CLAUDE.md` "Conventions worth preserving".

### Added — CLI

- `decode`, `count`, `dump` subcommands (Rust); `decode` with
  `--count` / `--dump` flags (Python). CLI shapes intentionally
  differ between impls per `L1-CLI-001` (capability parity, not
  exact spelling). See `docs/USER-GUIDE.md`.
- **Two-channel `count` output** (`L3-RS-008` / `L3-PY-010`): only
  the integer record count goes to stdout (pipeline-friendly:
  `n=$(mie-decoder count rec.mie)`); a human-readable
  `counted <N> messages in <basename>` status line goes to stderr.
- **Output safety subsystem** (`L1-OUT-002`):
  - `--no-clobber` refuses to overwrite an existing output
    (`L2-WRT-014`, `MieClobberRefusedError` / exit 1).
  - Input-equals-output rejection (`L2-WRT-016`,
    `MieInputOutputCollisionError` / exit 1).
  - Atomic write via `<stem>.<pid>.tmp` + `rename` (`L2-WRT-017`).
  - `--allow-partial` commits a partial decode to
    `<stem>.partial.csv` on unrecoverable sync loss (`L2-WRT-015`,
    exit 0); without it the partial is unlinked and the run exits 3.
- **Include filters** (`--include-types` / `--include-rts` /
  `--include-buses` / `--include-subaddresses`) as a Rust-only
  axis (`L3-RS-010`); exclude filters parity across both impls.
- **Exit-class taxonomy** (`L1-EXIT-005`): every decode emits a
  `decode exit class: <class>` INFO summary line and exits 0 / 1 /
  2 / 3 per `L1-EXIT-002`..`L1-EXIT-004`.
- **Inline error output mode** (`L2-ERR-011`): `--inline-errors`
  (Rust) / `--error-mode inline` (Python) keeps errored records
  in the main CSV with `ERROR` and `ERROR_CODE` columns populated
  rather than splitting them to `<stem>_errors.csv`.
- Hand-rolled **TOML config loader** with documented precedence
  (CLI flags > config file > built-in defaults); unknown-key
  warnings (`L2-CFG-009`); load-time validation (`L2-CFG-010`).
  See `docs/CONFIG-REFERENCE.md`.

### Added — decode pipeline

- **Four-phase sync strategy** (`docs/ARCHITECTURE.md` §3): header
  detection with diagnostic-rich failure (`diagnose_header_scan_failure`
  distinguishes `NoValidRecords` / `FirstRecordTruncated` /
  `HomogeneousPayload`), continuous per-record validation, two-record
  look-ahead confirmation, recovery scan with `MAX_SCAN_BYTES = 64 KB`.
- **Homogeneity-payload defense** (`L2-SYN-018`): rejects pathological
  inputs (all-zero, all-`0xFFFF`, etc.) that would otherwise pass shape
  validation. Maps to exit class 2.
- **First-record-truncated detection** (`L2-RDR-004`,
  `MieFirstRecordTruncatedError`): distinguishes a truncated initial
  record from a generic no-records error.
- **Structural invariants subsystem** (`L2-SYN-020`..`L2-SYN-025`):
  six rules check decoded records against MIL-STD-1553 transaction
  shape. Severity::Reject raises `MieRecordError`; Severity::AnomalyWarn
  logs and keeps the record. Errored records skip invariant checks
  (truncated payload by definition).
- **DELTA tracker** (`L2-RDR-016`..`L2-RDR-019`): per-RT/MSG key
  monotonicity tracking with non-monotonic-timestamp warnings.
- **Error pipeline**: DDC `0x01xx` hardware codes preserved verbatim;
  decoder-internal `0x20xx` codes (`0x2000` SPURIOUS_DATA continuation,
  `0x2001` standalone) assigned by classifying SPURIOUS_DATA records
  against the preceding record state. See `docs/ERROR-CATALOG.md`.

### Added — requirements traceability

- **L1 / L2 / L3 requirements docs** (`docs/L1-REQ.md`,
  `docs/L2-REQ.md`, `docs/L3-REQ.md`): 24 system requirements + 102
  architectural derivations + 26 implementation obligations,
  cross-linked by parent IDs and verification methods.
- **Auto-generated trace matrix** (`docs/TRACE-MATRIX.md`) produced by
  `scripts/build-trace-matrix.py`, gated in CI on every push.
- **Per-test requirement tagging** via `/// Requirements:` doc
  comments (Rust) and `@pytest.mark.requirement` markers (Python).

### Added — tooling

- **Cross-platform CI matrix** (`.github/workflows/ci.yml`): Rust on
  `ubuntu-latest` + `windows-latest`; Python on `ubuntu-latest`
  (3.10..3.14) + `windows-latest` (3.12, 3.14); conformance on both
  platforms; trace-matrix `--check`; PlantUML diagram drift gate.
- **`cargo-llvm-cov` coverage gate** at 70% line + region (Rust,
  Linux-only).
- **Pre-commit hook** (`.githooks/pre-commit`, installed via
  `bash scripts/install-hooks.sh`) mirroring the CI gates locally:
  whitespace/CRLF/merge-marker scans, file size cap, Cargo.lock
  parity, trace-matrix `--check`, `cargo fmt --check`, clippy,
  `cargo test --all-targets`, `dbg!()` scan, `// SAFETY:` comment
  requirement.
- **Conformance manifest schema enforcement**: typos in case fields
  (e.g. `rust_arg` for `rust_args`) fail fast with an actionable
  error and the list of allowed fields.

### Documentation

- `docs/USER-GUIDE.md` — end-to-end CLI walkthrough.
- `docs/EXAMPLES.md` — 11 runnable operator-task recipes.
- `docs/MIE-FORMAT.md` — comprehensive binary format reference with
  three worked hex-to-CSV decodes.
- `docs/ARCHITECTURE.md` (v2.0) — dual-implementation architecture
  with Rust↔Python module correspondence.
- `docs/CONFIG-REFERENCE.md` — normative TOML key reference.
- `docs/ERROR-CATALOG.md` — operator-facing error and exit-code
  reference.
- `docs/MAINTAINER-GUIDE.md` — repo layout, daily commands,
  workflows, CI architecture, release process.
- `docs/VENDOR-CSV-DIFFS.md` — alignment statement vs DDC vendor CSV.
- `docs/diagrams/{class,component,dataflow}.{puml,svg}` —
  dual-implementation PlantUML diagrams with committed rendered SVGs.

### Notes

- The Python package previously shipped at `1.1.0` from a pre-Rust-port
  lineage. As of this joint cut, the version aligns at `1.0.0`. This is
  a one-time downward alignment; future Python releases will increment
  forward from `1.0.0` per the impl-prefixed tagging scheme.
- The CHANGELOG starts here. Earlier history exists in `git log` but is
  not retroactively documented as separate entries.

[Unreleased]: https://github.com/joey-huckabee/mie-decoder/compare/v2.7.0...HEAD
[2.7.0]: https://github.com/joey-huckabee/mie-decoder/compare/v2.6.2...v2.7.0
[2.6.2]: https://github.com/joey-huckabee/mie-decoder/compare/v2.6.1...v2.6.2
[2.6.1]: https://github.com/joey-huckabee/mie-decoder/compare/v2.6.0...v2.6.1
[2.6.0]: https://github.com/joey-huckabee/mie-decoder/compare/v2.5.3...v2.6.0
[2.5.3]: https://github.com/joey-huckabee/mie-decoder/compare/v2.5.2...v2.5.3
[2.5.2]: https://github.com/joey-huckabee/mie-decoder/compare/v2.5.1...v2.5.2
[2.5.1]: https://github.com/joey-huckabee/mie-decoder/compare/v2.5.0...v2.5.1
[2.5.0]: https://github.com/joey-huckabee/mie-decoder/compare/v2.4.0...v2.5.0
[2.4.0]: https://github.com/joey-huckabee/mie-decoder/compare/v2.3.0...v2.4.0
[2.3.0]: https://github.com/joey-huckabee/mie-decoder/compare/v2.2.0...v2.3.0
[2.2.0]: https://github.com/joey-huckabee/mie-decoder/compare/v2.1.0...v2.2.0
[2.1.0]: https://github.com/joey-huckabee/mie-decoder/compare/v2.0.1...v2.1.0
[2.0.1]: https://github.com/joey-huckabee/mie-decoder/compare/v2.0.0...v2.0.1
[2.0.0]: https://github.com/joey-huckabee/mie-decoder/compare/v1.5.1...v2.0.0
[1.5.1]: https://github.com/joey-huckabee/mie-decoder/compare/v1.5.0...v1.5.1
[1.5.0]: https://github.com/joey-huckabee/mie-decoder/compare/v1.4.1...v1.5.0
[1.4.1]: https://github.com/joey-huckabee/mie-decoder/compare/v1.4.0...v1.4.1
[1.4.0]: https://github.com/joey-huckabee/mie-decoder/compare/v1.3.0...v1.4.0
[1.3.0]: https://github.com/joey-huckabee/mie-decoder/compare/v1.2.0...v1.3.0
[1.2.0]: https://github.com/joey-huckabee/mie-decoder/compare/v1.1.0...v1.2.0
[1.1.0]: https://github.com/joey-huckabee/mie-decoder/compare/v1.0.0...v1.1.0
[1.0.0]: https://github.com/joey-huckabee/mie-decoder/releases/tag/v1.0.0
