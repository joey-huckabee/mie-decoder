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

### Removed

- **`docs/FIELDS.md`** — the 3-line redirect stub kept for legacy
  external-link compatibility since the L2-DEC-015 / Documentation
  Initiative absorbed its content into `docs/MIE-FORMAT.md`. The
  stub has done its job; deleted. Active references in
  `docs/L1-REQ.md` (L1-OUT-001), `docs/L2-REQ.md` (L2-DEC-002,
  L2-SYN-025 rationale, error-code family rationale),
  `docs/MAINTAINER-GUIDE.md` (repo-tree listing), `CLAUDE.md`
  (Reference docs section), `docs/MIE-FORMAT.md` (the
  "absorbs FIELDS.md" note now reflects deletion), and
  `README.md` (repo-tree listing) updated to either point at
  `docs/MIE-FORMAT.md` directly or to drop the stub reference.
  ROADMAP historical mentions of FIELDS.md (Documentation
  Initiative recap, deferred-audit notes, etc.) preserved as-is
  since they describe past state accurately.

### Added

- **Conformance fixture: L2-DEC-015 borderline detection** (two
  cases). `timestamp-format-borderline-default` and
  `timestamp-format-borderline-n1` share a hand-crafted 5-record
  input where the multi-record probe genuinely changes the
  format choice cross-impl: at `--detect-records 1` both impls
  pick Standard and decode 1 row; at default `--detect-records 8`
  both impls pick IRIG (Decisive) and decode 4 rows. The two
  oracles are byte-identical across Rust and Python, pinning the
  cross-impl behavior at each N. Conformance case count: 22 → 24.
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

### Changed

- `docs/L2-REQ.md` L2-SYN-005 generalized in place: the original
  "two-record look-ahead" wording is now described as a special case
  of the configurable N-record rule with default `N = 2`. The
  generalization is non-breaking (existing files and configs continue
  to behave identically); the rationale for the in-place wording
  update is recorded in the L2-SYN-005 Rationale field.

### Fixed

- **Conformance fixture `timestamp-format-ambiguous-strict` was
  exercising the wrong code path.** Discovered while adding the
  lenient-mode companion fixture. The fixture's input bytes had a
  Type Word declaring `word_count = 7` (14 bytes) but the actual hex
  block contained 16 bytes per record. The mismatch caused
  `find_first_record`'s look-ahead to land on filler bytes mid-record
  and reject the candidate, producing `MieNoValidRecords` (exit 2)
  instead of the intended `MieTimestampFormatMismatch` (also exit
  2). The strict fixture's `expected_exit: 2` couldn't distinguish
  the two error paths, so the test passed for the wrong reason.

  Fix: change the Type Word's `word_count` field from 7 to 8 so the
  declared length matches the actual 16-byte record. The probe now
  reaches AMBIGUOUS classification correctly, and the strict fixture
  exercises the L2-DEC-016 path it was always meant to.

### Added

- **Conformance fixture: L2-DEC-016 lenient-mode WARN**
  (`timestamp-format-ambiguous-lenient`). Reuses the (now-fixed)
  ambiguous input bytes with default (lenient) mode. Asserts exit 0
  with a header-only CSV oracle, plus a stderr substring assertion
  that pins the lenient-mode WARN's score breakdown
  (`"Ambiguous: IRIG=4 STD=4"`). Companion to the strict fixture;
  together they pin both branches of L2-DEC-016 cross-impl.
- **Stderr substring assertions on both L2-DEC-016 conformance
  fixtures.** Strict fixture now asserts
  `expected_stderr_contains: "auto-detection is ambiguous"`
  (substring unique to the `MieTimestampFormatMismatch` error
  message); lenient asserts the WARN substring as above. These
  defend against future regressions that exit with the right code
  via the wrong code path — the same class of bug-in-test the strict
  fixture had until this commit.

Conformance case count: 21 → 22.

### Maintenance

- `docs/diagrams/dataflow.puml` `find_first_record` note updated:
  "(two-record look-ahead)" → "(L2-SYN-005 / L2-SYN-026; N defaults
  to 2, configurable via decode.lookahead_records)". The rendered
  `docs/diagrams/dataflow.svg` was regenerated to match (PlantUML
  1.2026.5, matching the pin in the `diagrams` CI job).
- `docs/ROADMAP.md` refreshed for the post-v1.1.0 / pre-v1.2.0 state:
  v1.1.0 release-status entry added; "Queued for the next release"
  section describes the `[Unreleased]` L2-SYN-026 work; the two
  Robustness-backlog items resolved in v1.1.0 / `[Unreleased]` are
  struck through with their resolution commits; "Shared Commitments"
  text updated from "two-record look-ahead" to the N-record wording;
  new "Deferred follow-ups" section enumerates small bounded items
  (three conformance fixtures + the Python coverage gate) so they
  don't get lost between sessions.

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

[Unreleased]: https://github.com/joey-huckabee/mie-decoder/compare/v1.1.0...HEAD
[1.1.0]: https://github.com/joey-huckabee/mie-decoder/compare/v1.0.0...v1.1.0
[1.0.0]: https://github.com/joey-huckabee/mie-decoder/releases/tag/v1.0.0
