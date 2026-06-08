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

### Changed

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

[Unreleased]: https://github.com/joey-huckabee/mie-decoder/compare/v1.0.0...HEAD
[1.0.0]: https://github.com/joey-huckabee/mie-decoder/releases/tag/v1.0.0
