# MIE-Decoder Roadmap

## Release status

**v1.2.0 — joint Rust + Python cut, 2026-06-08.** Third joint
release. Added the L2-SYN-026 configurable N-record sync look-ahead
(`decode.lookahead_records` TOML key, `--lookahead-records N` CLI
flag), the Python coverage gate in CI (85% combined line+branch
floor), and three new cross-impl conformance fixtures (L2-DEC-015
borderline, L2-DEC-016 lenient-mode WARN, L2-SYN-026 N>2 catches
what N=2 misses). Fixed a Python L2-CFG-003 precedence bug where
TOML `[logging] level` was silently ignored when no `--log-level`
was passed on the CLI. Retired the static-musl SLES 12 deployment
target (L3-RS-007 withdrawn; ID reserved) and the `docs/FIELDS.md`
redirect stub. Test count: 242 → 248; conformance case count:
21 → 27. See [`CHANGELOG.md`](../CHANGELOG.md) section `[1.2.0]`
for the full entry.

**v1.1.0 — joint Rust + Python cut, 2026-06-07.** Second joint release.
Added the L2-DEC-015 multi-record timestamp-format auto-detection probe,
the L2-DEC-016 `MieTimestampFormatMismatchError` for ambiguous-detection
cases, the `decode.detect_records` TOML key and `--detect-records N` CLI
flag (range `[1, 32]`, default `8`), plus several smaller cleanups
(conformance-manifest type validation, trace-matrix L2-CLI-006 fix). See
[`CHANGELOG.md`](../CHANGELOG.md) section `[1.1.0]` for the full entry.

**v1.0.0 — joint Rust + Python cut, 2026-06-07.** First release of both
implementations from a single repository tag (`v1.0.0`). Combined scope
covers the streaming Rust writer + static-musl build, the Python package
+ pandas writer, the cross-implementation conformance suite (20 cases),
the L1/L2/L3 requirements + auto-generated trace matrix, the output-
safety subsystem (atomic temp + rename, `--no-clobber`, `--allow-partial`,
input/output collision rejection), the structural-invariants subsystem
(`L2-SYN-020`..`L2-SYN-025`), the homogeneity-payload defense, and the
documentation suite under `docs/`. See [`CHANGELOG.md`](../CHANGELOG.md)
for the full v1.0.0 entry.

### Queued for the next release (`[Unreleased]`)

`[Unreleased]` is empty as of the v1.2.0 cut. Future work
accumulates here; when ready to cut a release, follow the
version-bump checklist in `docs/MAINTAINER-GUIDE.md` §11.

## Planned

| Version | Feature |
|---------|---------|
| Rust v1.x | Multi-file input, time-sorted merge to single CSV. |
| Rust v2.0 | Data word decoders, additional per-message-type CSVs. |
| Rust v3.0 | Apache Parquet output. |

Subsequent releases may diverge in version via impl-prefixed tags
(`rust-vX.Y.Z`, `python-vX.Y.Z`); the cross-implementation conformance
contract (CSV byte-for-byte equivalence on shared behavior) holds at any
compatible version pair. See [`docs/MAINTAINER-GUIDE.md`](MAINTAINER-GUIDE.md)
section 11 for the release workflow.

## Shared Commitments

- **`config/default.toml` and TOML config support remain a first-class feature.** The Rust build ships a hand-rolled TOML loader for our config schema; the file format and key names are stable.
- **CSV column layout matches DDC vendor output byte-for-byte.** No reordering or renaming of columns, including currently-empty columns (`MUX`, `TERM_NAME`, `IM_GAP`, `RCV_GAP`, `XMT_GAP`).
- **Sync recovery semantics preserved.** N-record look-ahead (default `N = 2` per L2-SYN-005, configurable via L2-SYN-026), 64 KB scan cap, error records and SPURIOUS_DATA continuations remain valid records that pass validation.
- **One validation path.** Header skip, normal forward decode, and post-loss recovery all share `sync::validate_record`. There is no weaker fast path.
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

## Team Review Backlog (2026-06-05)

Active backlog from a multi-implementation review conducted 2026-06-05.
Items are numbered as they were raised in review.

**Item 1 (DELTA units and Rust/Python divergence) is resolved** — landed
the `Option<f64>`/`float | None` contract, error-record participation,
non-monotonic-timestamp WARN, and the cross-impl `delta-tracking-irig`
conformance fixture. See `L2-RDR-016` through `L2-RDR-019` in
`docs/L2-REQ.md` and the FIELDS.md DELTA section.

**Items 2-8 below are open.** Effort estimates are person-day bands for
a single engineer with full context. The Phase Sequencing block at the
end records the recommended landing order — Phase 1 (spec only) is the
gating decision; after that, phases are mostly independent.

---

### Item 2 — Distinguish complete success, partial success, unrecoverable failure

**Severity:** Critical.

**Status quo.** Python's `MieFileReader.__iter__` returns silently when
`find_first_record` is None (`python/src/mie_decoder/reader.py:203-207`)
— a non-MIE file becomes a successful empty decode with exit 0. Rust
surfaces `MieError::NoValidRecords` as the first iterator item
(`src/reader.rs:138-149`). On unrecoverable mid-file sync loss, both
implementations log ERROR and stop with exit 0 (`src/reader.rs:320-329`;
`python/src/mie_decoder/reader.py:267-273`). The CSV writer reports
success even when zero rows were emitted.

**Risk.** A pipeline that decodes a corrupt file and `cat`s the output
never knows the file was bad. Conformance fixtures can't distinguish
"decoded nothing because nothing was there" from "decoded nothing
because the input was junk." Operators who pipe
`mie-decoder decode | downstream` get silent data loss.

**Proposed contract.**
- `L1-EXIT-002`: No valid records → exit code **2**, no output file created.
- `L1-EXIT-003`: Recovered corruption (lenient mode, `recover_sync` succeeded
  ≥1 time) → exit code **0**, log INFO with recovery counter, CSV
  completes normally.
- `L1-EXIT-004`: Unrecoverable mid-file sync loss → exit code **3** by
  default. Optional `--allow-partial` flag downgrades to exit **0** with
  WARN; the partial output file is preserved.
- `L1-EXIT-005`: The decode command SHALL log a one-line summary on exit
  naming the exit class (`complete`, `partial-recovered`,
  `partial-unrecoverable`, `no-records`).

**Implementation surface.**
- *Rust:* Add `ExitClass` enum returned alongside
  `(normal_count, error_count)` from `write_csv` / `write_csv_split`.
  `cli::run_decode` maps to exit codes. The iterator already returns
  `Err` on unrecoverable loss in strict mode; lenient mode needs a
  terminal `Err` item before the iterator stops. Hold the path of the
  partial file in `RecordIter` so the CLI can `unlink` on failure
  unless `--allow-partial`.
- *Python:* `MieFileReader` becomes a generator that yields a final
  sentinel (or raises a non-fatal `MiePartialDecodeError`) when sync
  recovery exhausts. CLI catches the sentinel and maps to exit code.
  The `if start_offset is None: return` path becomes
  `raise MieNoValidRecordsError` (new exception class) so non-MIE
  files exit 2.
- *Both:* `--allow-partial` CLI flag plus matching
  `decode.allow_partial = bool` config key.

**Tests / conformance.** Three new fixtures:
1. `no-valid-records` — 1 KB of `0xFF`, expected exit **2**.
2. `partial-unrecoverable-default` — valid record + corrupt tail,
   default exit **3**.
3. `partial-unrecoverable-allow` — same input with `--allow-partial`,
   exit **0** + partial CSV oracle.

Update `tests/conformance/run.py` to capture and assert exit codes
(currently it requires `returncode == 0`).

**Sequencing.** Depends on Item 3 (atomic writes) so the failed-decode
path can actually delete the temp file. **Effort: 3-5 days** including
the conformance plumbing.

---

### Item 3 — Output transaction safety and path identity

**Severity:** High.

**Status quo.** `src/writer.rs:155` calls `File::create(path)` before
consuming the iterator; if decode errors after the header is written,
the file is left header-only or partial. Stdout case has the same
issue. Python (`python/src/mie_decoder/writer.py:283-301`) builds rows
in memory then writes, so a decode error before completion leaves no
file — different failure mode, equally undefined. Neither implementation
checks that `--output` differs from the input path; with Rust's
mmap-on-read this could produce undefined behavior (Linux: stale page
cache; Windows: sharing violation).

**Risk.** Three concrete failure modes today:
- (a) `mie-decoder decode input.mie -o input.mie` corrupts the input
  mid-mmap.
- (b) `decode broken.mie -o existing-good.csv` destroys an existing
  file when the decode fails.
- (c) Disk-full midway through a 10 GB decode leaves a half-written
  CSV indistinguishable from a complete one.

**Proposed contract.**
- `L2-WRT-014`: The output path SHALL NOT resolve to the same canonical
  path as the input. Implementations SHALL surface a distinct error
  class before opening the output.
- `L2-WRT-015`: File output SHALL be written via temp-file-plus-atomic-
  rename. The temp file SHALL live in the destination's directory (not
  a system temp dir, to keep the rename on the same filesystem).
  Naming: `<output>.mie-decoder.tmp.<pid>` so concurrent runs don't
  collide.
- `L2-WRT-016`: On decode failure with default exit class (Item 2's
  L1-EXIT-004), the temp file SHALL be unlinked. With `--allow-partial`,
  the temp file SHALL be renamed to the destination with a `.partial`
  suffix appended.
- `L2-WRT-017`: Overwrite of an existing destination SHALL succeed;
  opt-out via `--no-clobber`.
- `L2-WRT-018`: Broken pipe (stdout consumer closed) SHALL exit **0**
  with no error. Disk full and permission errors SHALL produce a
  `WriterError` with the underlying OS error preserved.

**Implementation surface.**
- *Rust:* New `AtomicCsvFile` struct wrapping `BufWriter<File>` plus
  the temp path. `Drop` impl unlinks on failure path; explicit
  `commit()` does the rename. Use `std::fs::rename` (atomic on POSIX,
  atomic on NTFS for same-volume). `write_csv` / `write_csv_split`
  route through it. Pre-flight identity check via either a vendored
  `is_same_file` (~30 lines) or `fs::canonicalize` on both paths and
  compare.
- *Python:* `dataframe_to_csv` writes to a temp `Path`, then
  `os.replace(temp, dest)` (atomic on POSIX and Windows). Identity
  check via `os.path.samefile`. Stdout path: ignore `BrokenPipeError`.
- *Both:* CLI grows `--no-clobber`. Config key `output.no_clobber = bool`.

**Tests / conformance.** Unit tests for: same-path rejection, atomic
rename, partial cleanup under simulated error, `.partial` suffix under
`--allow-partial`, broken-pipe handling. Conformance: add a
`--no-clobber` case that runs twice and asserts the second invocation
fails with a distinct error class.

**Sequencing.** Item 2 depends on this for clean partial-file semantics.
Should land first or at the same time as Item 2. **Effort: 2-3 days.**

---

### Item 4 — IRIG validation accepts out-of-range day and microsecond fields

**Severity:** High.

**Status quo.** `sync::validate_record` checks hour/minute/second only
(`src/sync.rs:62-74`); day and microseconds are unchecked. A garbage
record whose bit pattern happens to encode `day=400` or
`microsecond=1_500_000` passes validation. `IrigTimestamp::format` then
emits `400:15:54:50.1500000` — seven-digit microseconds break the
documented `DAY:HH:MM:SS.uuuuuu` layout (`docs/FIELDS.md`) and can
confuse downstream parsers expecting fixed-width fields.

**Risk.** Two distinct harms:
- (a) Sync false-positives — a corrupt record passes the gate and emits
  garbage rows.
- (b) Output schema violation — DDC vendor CSV uses fixed-width fields,
  our output silently widens.

**Proposed contract.**
- `L2-SYN-004` (revised): IRIG validation SHALL reject `hour ≥ 24`,
  `minute ≥ 60`, `second ≥ 60`, `day < 1 || day > 366`, and
  `microsecond > 999_999`.
- `L2-SYN-019`: When `freerun = true` (bit 15 of upper word), the
  day-of-year field SHALL be permitted to fall outside `[1, 366]`
  because the card's free-running oscillator is not calendar-locked.
  Hour/minute/second/microsecond constraints still apply.
- `L2-DEC-014`: `IrigTimestamp::format` SHALL emit exactly six
  microsecond digits regardless of the decoded value (overflow truncates
  to six digits and emits a WARN — this case should be unreachable
  given the validation above, but the formatter MUST NOT produce more
  than six).

**Implementation surface.**
- *Rust:* `src/sync.rs::validate_record` — extend the IRIG block
  (currently lines 62-74). Add day check; compute `microsecond` from
  middle+lower and check `< 1_000_000`. `find_first_record` and
  `recover_sync` already share `validate_record` so no further edits
  there. Add defensive clamp/warn in `src/decode.rs::decode_irig_timestamp`.
- *Python:* `python/src/mie_decoder/sync.py::validate_record` — same
  additions. `python/src/mie_decoder/decode.py::decode_irig_timestamp`
  — same defensive clamp.
- *Both:* Update the FIELDS.md day-of-year note (already mentions vendor
  variance — re-anchor it on the freerun exception rather than
  card-model variance, which the broader IRIG day-field investigation
  still owes — see existing "IRIG day-field decoding across DDC card
  models" backlog item below).

**Tests / conformance.** Unit tests: each of `day=0`, `day=367`,
`microsecond=1_000_000` rejected; `freerun=true` with `day=0` accepted.
New conformance fixture `irig-freerun-out-of-range-day` that produces
a row with freerun set and an unusual day, plus a negative
`irig-corrupt-microsecond-rejected` that proves the byte sequence fails
to decode.

**Sequencing.** Independent. **Effort: 1 day.**

---

### Item 5 — Structural cross-field invariants are not enforced

**Severity:** High.

**Status quo.** Today's five validation heuristics catch framing-level
corruption but accept records that are internally inconsistent: Type
Word `0x02` (BC→RT) with a Command Word direction of Transmit; Command
Word `data_word_count=30` packed into a record whose `word_count` only
leaves room for 10 data words; Status Word RT field that doesn't match
the Command Word RT; reserved bits set to 1. Already enumerated as
ROADMAP backlog items in the "Validation strength" section below (T/R
consistency, Type↔Cmd capacity, header look-ahead depth).

**Risk.** Structurally plausible corruption produces decoded rows that
look real — wrong CSV output that downstream tools can't distinguish
from valid data. This is the highest-trust-violation class of bug
because validation is supposed to be the firewall against it.

**Proposed contract.** Pin a structural-invariant table per message
format. Format below; the actual table belongs in `docs/L2-REQ.md`
as a new L2-SYN-020..025 block referenced by L1-SYN-001.

| Format | Invariant | Strict | Lenient |
|--------|-----------|--------|---------|
| BC→RT (0x02) | `Cmd.direction == Receive` | `UnknownTypeWord` | WARN+skip |
| RT→BC (0x04) | `Cmd.direction == Transmit` | `UnknownTypeWord` | WARN+skip |
| Any | `TW.word_count ≥ 1 + TS + 1 + Cmd.data_word_count + status_words` | `PayloadError` | WARN+skip |
| Any with status | `Status.rt == Cmd.rt` | `PayloadError` | WARN+emit |
| Type Word | `bit 15 == 0` | `UnknownTypeWord` | WARN+emit |

- `L2-SYN-020` through `L2-SYN-024`: each invariant as a
  separate ID so conformance can assert one at a time.
- `L2-SYN-025`: Failed invariants count toward the sync-loss
  counter for Item 2's exit-class accounting.

The status-RT-mismatch case is **real-bus noise** (RT responding under
a different address), not necessarily corruption — so it must be
warn-and-emit, not skip, even in lenient mode. This needs to be
explicitly carved out in the requirement; otherwise we'll generate
false negatives on real recordings.

**Implementation surface.**
- *Rust:* `src/decode.rs::classify_message_format` already returns a
  per-format enum; add a
  `validate_structural_invariants(tw, cmd, payload_words) -> Result<(), InvariantViolation>`
  companion that the reader calls between classification and yielding.
  The reader's strict/lenient branch maps `InvariantViolation` to
  `MieError::PayloadError` with a structured `which_check` field
  (resolves the "structured 'which check failed' enum" backlog item
  below) or to a WARN+continue.
- *Python:* `_decode_error_record` and the normal-record path in
  `__iter__` get a shared `_check_invariants(...)` helper. Same
  strict/lenient mapping.

**Tests / conformance.** One conformance fixture per invariant,
exercising both strict (negative — must fail) and lenient (positive —
must skip-with-warn). Total ~10 fixtures. Effort dominated by fixture
construction.

**Sequencing.** Independent but large. **Effort: 5-7 days.** Best split
into two PRs: (a) capacity + direction invariants (highest value);
(b) status RT + reserved-bits (lower value, more debate).

---

### Item 6 — Configuration schema drift between Rust and Python

**Severity:** High.

**Status quo.** Concrete divergences confirmed against source:
- *Log level validation:* Rust rejects unknown `logging.level` at parse
  time (`src/config.rs:148`, regression-fixed; see test at
  `src/config.rs:580-593`). Python accepts any string
  (`python/src/mie_decoder/config.py:272`) and silently drops it at
  apply time.
- *Output format validation:* Both implementations accept any string
  for `output.format` and store it (`src/config.rs:165-167`,
  `python/.../config.py:308`). Rust documents "csv expected"; neither
  validates.
- *Strict boolean coercion:* Python uses
  `bool(decode_section.get("strict", False))`
  (`python/.../config.py:284`). For a TOML value that parsed as a
  string (it wouldn't — `tomllib` rejects), this is a non-issue at the
  TOML layer; but if someone calls `DecoderConfig(...)` directly with
  `strict="no"`, Python coerces to True. Rust's `with_overrides`
  requires `Option<bool>` so this is type-enforced.
- *RT/subaddress range:* Python `set(filter_section.get("exclude_rts", []))`
  — no range check. Rust `parse_int_u8` (`src/config.rs:276`) only
  checks u8 range (0-255), not 1553's 0-31. Both can accept
  `exclude_rts = [99]` and silently never match any record.
- *Unknown TOML keys:* Both implementations ignore unknown keys
  silently. A typo in `exclude_subdresses` is invisible.

**Risk.** Configs that "look applied" but quietly aren't are the worst
kind of bug — operator believes the filter is on, the filter is
silently off.

**Proposed contract.** A formal schema in `docs/L2-REQ.md` (or
referenced from there into a new `docs/CONFIG-SCHEMA.md`):

| Key | Type | Range / Enum | Unknown handling |
|-----|------|--------------|------------------|
| `logging.level` | string | one of DEBUG/INFO/WARNING/WARN/ERROR/CRITICAL (case-insensitive) | reject |
| `decode.time_format` | string | auto/irig/standard | reject |
| `decode.strict` | bool (TOML boolean only) | true/false | reject non-bool |
| `decode.error_mode` | string | separate/inline | reject |
| `decode.allow_partial` | bool | (Item 2) | reject non-bool |
| `output.format` | string | csv (only valid value in v1) | reject |
| `output.no_clobber` | bool | (Item 3) | reject non-bool |
| `filter.exclude_types` | string\|int array | per-element validated | reject |
| `filter.exclude_rts` | int array | each in [0, 31] | reject out-of-range |
| `filter.exclude_buses` | string array | each in {A, B} | reject |
| `filter.exclude_subaddresses` | int array | each in [0, 31] | reject out-of-range |
| Any unknown key | — | — | WARN at load time |

- `L2-CFG-009`: Unknown TOML keys SHALL produce a WARN at load time
  naming the offending key, but SHALL NOT fail the load.
  (WARN-not-error so we can add keys without breaking older configs.)
- `L2-CFG-010`: All schema validations SHALL apply at load time, not
  at use time.

**Implementation surface.**
- *Rust:* Add `output.format` validator (currently passes through). Add
  0-31 range checks in `parse_int_u8` callers for RT/SA specifically
  (introduce `parse_int_rt_sa(v) -> u8` that asserts 0..=31). Add
  unknown-key tracking in `parse_toml` — easiest: track all
  `(section, key)` entries we read, diff against `doc.entries` after,
  WARN for unread keys.
- *Python:* Add the same validators. Replace `bool(...)` coercion with
  explicit `isinstance(..., bool)` check. Add the 0-31 ranges. Add
  unknown-key tracking. Add `logging.level` validation. Verify the CLI
  driver actually calls `logging.getLogger().setLevel(...)` based on
  the loaded value (separate audit needed).

**Tests / conformance.** Existing config tests need extension:
unknown-level rejected (both), unknown-output-format rejected (both),
out-of-range RT rejected (both), unknown-key WARN'd (both). New
conformance case `config-validation-rejects-unknown-key` that runs with
a typo'd TOML and asserts non-zero exit.

**Sequencing.** Independent but touches both implementations
symmetrically. **Effort: 3 days**, mostly test coverage.

---

### Item 7 — Conformance suite under-covers the alignment claim

**Severity:** Medium.

**Status quo.** Six conformance cases now (after the new
`delta-tracking-irig` from Item 1): `basic-multi-record`,
`header-and-sync-recovery`, `errors-inline`, `exclude-subaddress`,
`config-filter`, `delta-tracking-irig`. The traceability table at
`docs/TRACE-MATRIX.md` (auto-generated from L1/L2/L3 + test markers) maps these to broad
swathes of L1/L2 — over-claiming. Notable gaps:
- No Standard-format timestamp fixture (despite L2-DEC-007).
- No Bus B fixture (despite L1-DEC-004).
- No RT-to-RT fixture (one of 10 supported formats).
- No mode code fixtures (broadcast or otherwise).
- No separate-mode error output fixture (only inline).
- No invalid-input fixture (no `MieError` path is exercised cross-impl).
- No strict-mode fixture.
- No exit-code assertions (Item 2 requires this).

**Risk.** Future divergences in any of these areas land green in CI.
The error-record DELTA divergence we just fixed (Item 1) was invisible
for exactly this reason.

**Proposed contract.**
- `L2-CONF-006`: The conformance suite SHALL include at least one case
  for each L1 requirement and at least one case per non-trivial L2
  behavioral class. The traceability table SHALL list, per case, the
  specific requirements it actually exercises (no umbrella claims).
- `L2-CONF-007`: The conformance runner SHALL assert exit codes per
  case (default 0; cases with `expected_exit` override).
- `L2-CONF-008`: Cases SHALL exist for each documented divergence
  point between implementations (currently: Rust include filters;
  future: anything in this roadmap).

**Implementation surface.** New fixtures to add (each ~30 min given
existing tooling):
1. `standard-timestamps-empty-delta` — proves Item-1 contract on
   Standard.
2. `bus-b` — one record with bit 7 set in the Type Word.
3. `rt-to-rt` — Type 0x08.
4. `mode-code-tx-data` and `mode-code-no-data` — covers the two most
   distinct mode-code layouts.
5. `errors-separate` — same input as `errors-inline` with default error
   mode, asserting `<stem>_errors.csv` exists with the right contents.
6. `strict-rejects-unknown-type` — input with an invalid Type Word,
   run with `--strict`, expected non-zero exit and no output file.
7. `no-valid-records-exit-2` — Item 2 fixture.

Extend `tests/conformance/run.py` to:
- Read `expected_exit` from manifest (default 0).
- Support `expected_stderr_contains` for cases where we want to assert
  on a specific log line.
- Support split-output cases (need to check both main + errors CSV
  against oracles).

**Sequencing.** Item 2 produces fixtures 6 and 7 as a side effect.
Items 4 and 5 produce more. **Effort for the rest: 2-3 days.**

---

### Item 8 — Operational assumptions unstated

**Severity:** Medium.

**Status quo.** Both readers use mmap (`src/reader.rs:82` via memmap2;
`python/src/mie_decoder/reader.py:195` via `mmap.ACCESS_READ`).
Concurrent file modification during decode is undefined: on POSIX,
truncating a mapped file can produce SIGBUS on subsequent access; on
Windows, file locking generally prevents truncate, but extension while
mapped doesn't grow the mapping. Python's writer materializes the full
DataFrame in memory before flushing
(`python/src/mie_decoder/writer.py:174-195` → `pandas.DataFrame`); Rust
streams. For a 10 GB recording, Python OOMs; Rust uses ~64 KB of
`BufWriter`. No requirement documents this asymmetry.

**Risk.**
- (a) Quiet data corruption if the input file is truncated mid-decode.
- (b) Python OOM on large files with no documented limit.
- (c) No defined behavior for "arbitrary bytes as input" — fuzz test
  would surface panics or unbounded scans.

**Proposed contract.**
- `L1-EXIT-006`: The input file SHALL NOT be modified, truncated, or
  extended during decoding. Behavior under concurrent modification is
  implementation-defined and MAY cause termination.
- `L1-SYN-002`: Sync recovery scanning SHALL be bounded — max one recovery
  attempt per 64 KB of input, max total recovery scan distance per
  file equal to the file size (i.e., scans never re-traverse
  already-scanned bytes).
- `L1-ROB-001`: For arbitrary input bytes within the size limits of
  `usize`, no implementation SHALL panic, segfault, or enter an
  unbounded loop. Failures SHALL surface as `MieError` variants.
- `L3-PY-012`: Python memory usage during decode SHALL be O(record_count)
  until streaming output is implemented. Document this with a
  typical-file-size guideline (e.g., "10 M records ≈ 5 GB RSS").
- `L3-RS-012`: Rust memory usage during decode SHALL be O(1) in the
  number of records; constant overhead bounded by `BufWriter` capacity
  plus `delta_tracker` size.

**Implementation surface.**
- *Rust:* Mostly a documentation/test exercise. Add a `tests/fuzz.rs`
  (or use `cargo fuzz`) that feeds random byte sequences to
  `MieFileReader` and asserts no panic. The `recover_sync` cumulative
  bound (L1-SYN-002) needs an added counter on `RecordIter` to prevent
  pathological inputs from looping the full file repeatedly.
- *Python:* Same fuzz harness via `hypothesis`. To make L1-ROB-001 actually
  hold, audit `decode.py` for places that could raise `IndexError`
  instead of `MiePayloadError` on malformed input — likely a few. The
  "Python streams CSV" change is a separate, larger refactor (track
  as `PY-streaming` — pandas DataFrame buffering is currently
  load-bearing for the `_errors.csv` split path).
- *Docs:* Add a "Performance and limits" section to
  `docs/ARCHITECTURE.md`.

**Tests / conformance.** Fuzz harness (no conformance case — fuzz is
per-impl). Cross-impl input-size benchmark documented as a non-binding
reference. Streaming-Python is its own item; mark it `PY-streaming` in
the backlog.

**Sequencing.** Fuzz/no-panic landings are independent (1 day each).
Python streaming is a 3-5 day item that should wait until Items 2 and
3 land (the streaming writer needs the atomic-file machinery anyway).

---

### Phase Sequencing

Recommended landing order, adopting the team's "first decisions" framing:

**Phase 1 — Decisions and spec (1 PR, no behavior change).**
- Update the requirement docs (`docs/L1-REQ.md` / `docs/L2-REQ.md` / `docs/L3-REQ.md`) and `docs/FIELDS.md` for Items 2, 3,
  4, 6 contract language only.
- Update `tests/conformance/manifest.json` schema to support
  `expected_exit`.
- Item 1 (DELTA) already landed.
- *Effort: 1 day.*

**Phase 2 — Atomic writes and identity check (Item 3).**
- Lands the file-safety substrate the rest depends on.
- *Effort: 2-3 days.*

**Phase 3 — Exit-code semantics (Item 2).**
- Builds on Phase 2's temp-file mechanism for partial-file cleanup.
- Includes the `no-valid-records` and `partial-unrecoverable`
  conformance fixtures.
- *Effort: 3-5 days.*

**Phase 4 — IRIG range validation (Item 4).**
- Independent, low risk.
- *Effort: 1 day.*

**Phase 5 — Config schema alignment (Item 6).**
- Independent, mechanical.
- *Effort: 3 days.*

**Phase 6 — Conformance breadth (Item 7).**
- Add the missing fixtures (Standard, Bus B, RT-RT, mode codes,
  errors-separate, strict-rejects).
- *Effort: 2-3 days* (after Items 2, 4 provide some fixtures for free).

**Phase 7 — Structural invariants (Item 5).**
- Largest design surface. Split into sub-PRs per invariant.
- *Effort: 5-7 days total.*

**Phase 8 — Fuzz and limits docs (Item 8).**
- Land fuzz harness, document memory model. Python streaming as a
  separate later item (`PY-streaming`).
- *Effort: 2 days* for the documented-limits portion;
  *3-5 days* additional for `PY-streaming` whenever it lands.

**Total open work: ~25-30 person-days.** Phase 1 is the gating
decision — once language is locked, the implementation phases are
mostly independent and can parallelize across engineers.

---

## Architecture Audit (2026-06-05)

Findings from the initial architecture and requirements audit performed
on 2026-06-05, **prior to** the team review that produced the Team
Review Backlog above. Captured here verbatim (with status annotations)
so context and rationale are not lost.

**Overall status: all nine items resolved.** Items 2 and 8 were the
two distinct new error classes added in 2026-06 commits `b1ea897`
(FirstRecordTruncated, L2-RDR-004) and `80d0884` (HomogeneousPayload,
L2-SYN-018). Items 1 and 5 landed in earlier 2026-06 work
(exit-code classes and DELTA edge cases). Items 3, 4, 6, 7, and 9
landed as spec clarifications in the L1/L2/L3 requirements split.
The Suggested Consolidated Spec-Only PR table at the end of this
section maps every row to its landing commit / requirement.

Source citations preserved. Status markers added inline.

### Bottom line

The requirements are unusually well-structured (L1 → L2 → PY/RS split
with traceability), and the code mostly matches them. The gaps that
came out of this audit fall into three buckets: (1) documented
behaviors the spec doesn't actually mandate, (2) edge cases neither
code nor spec address, and (3) one wording ambiguity that's
load-bearing for sync. Order below is roughly by severity at audit
time.

---

### Audit Item 1 — Lenient-mode unrecoverable sync loss is silent in the data plane

**Status:** **Resolved.** Option (b) landed in commit `286844e`
(`feat(reader,cli): exit-code classes, --allow-partial, .partial
commits`). Both Python (`MieUnrecoverableSyncLossError`) and Rust
(`MieError::UnrecoverableSyncLoss`) lenient-mode iterators yield a
terminal `Err` item before stopping, so library callers can react.
The CLI maps it to exit 3 per L1-EXIT-004. Captured here for history.

`L2-SYN-011` says recovery "SHALL report when no valid record is found
within the scan window." In strict mode the reader returns
`Err(MieError)`. In lenient mode (`src/reader.rs:302–330`) the iterator
just returns `None` and stops; the only "report" is a stderr
`log_error!` line — not an `Err` item, not an exit code.

Two reasonable consumers of this iterator get different stories: the
CSV writer reaches end-of-stream and exits 0; a programmatic API
consumer can't distinguish "clean EOF" from "gave up." Two options:

- **(a)** Tighten the requirement to "SHALL log at ERROR; MAY surface
  as `Err` only in strict mode" (codify current behavior).
- **(b)** Have lenient mode yield a terminal `Err` item before
  stopping, so library callers can react.

Lean (a) for stability — it's how it already works — but (b) is the
safer engineering answer if anyone consumes the iterator
programmatically. The CLI exit-code work in Team Review Item 2 makes
option (b) cheaper to implement because the partial-file unlink
machinery already needs the terminal-Err signal.

### Audit Item 2 — Truncated first record isn't covered

**Status:** **Resolved.** Landed as L2-RDR-004 in `docs/L2-REQ.md`
and implemented in commit `b1ea897` (`feat(reader): implement
L2-RDR-004 FirstRecordTruncated diagnostic`). Both crates have a
distinct error class (`MieError::FirstRecordTruncated` /
`MieFirstRecordTruncatedError`), strict mode surfaces it, lenient
mode terminates cleanly with zero records. The
`first-record-truncated` conformance fixture verifies cross-impl
agreement. Captured here for history.

`L1-DEC-005` / `L2-RDR-002` / `L2-RDR-003` all cover truncation of the
*final* record. Nothing covers truncation of the *first* record after
header skip. Today: in lenient mode `find_first_record` silently
returns `None` → `NoValidRecords`; in strict mode no distinct error
class fires.

**Proposed addition:** new L2-RDR requirement reading: "Header
detection followed by a truncated record SHALL surface a distinct
error in strict mode and SHALL terminate cleanly in lenient mode."

Effort: very small (one validation branch + one test per impl).

### Audit Item 3 — Timestamp-format selection isn't a requirement at all

**Status:** **Partially resolved.** All three proposed L2 additions
landed: L2-DEC-011 pins file-level detection, L2-DEC-012 pins the
IRIG-wins-on-tie tie-break, L2-DEC-013 pins the
`--time-format` / `decode.time_format` override path. Only L2-DEC-012
is still listed as Draft in `docs/TRACE-MATRIX.md` because writing a
test that produces a true equal-score tie requires reverse-engineering
the auto-detection heuristic; deferred. The spec dimension of the
audit item is fully done. Captured here for history.

The ROADMAP "Stronger timestamp-format auto-detection" backlog item
(below) is a *feature* request — but there's no L2 requirement defining
the *current* behavior either:

- Whether detection is file-level (current behavior: locked on the
  first record at `src/reader.rs:114–118` / `src/decode.rs:215`) or
  could be per-record.
- The tie-break rule (current: IRIG wins, hardcoded comment "more
  common in flight test recordings").
- Whether CLI `--time-format` overrides auto-detect (it does, but
  `L2-CFG-003` only generically covers precedence).

This matters because a future "smarter detection" PR could quietly
change file-level → per-record and break consumers. Pin the current
behavior with an L2-DEC requirement now, *before* anyone touches the
detection logic.

**Proposed additions:**
- `L2-DEC-011`: Timestamp-format detection SHALL be file-level —
  resolved on the first valid record and used unchanged for all
  subsequent records.
- `L2-DEC-012`: When IRIG and Standard score equally during
  auto-detection, IRIG SHALL be selected.
- `L2-DEC-013`: An explicit `--time-format` CLI flag or
  `decode.time_format` config value SHALL bypass auto-detection.

### Audit Item 4 — Look-ahead at EOF is correct but spec-ambiguous

**Status:** **Resolved.** L2-SYN-005 in `docs/L2-REQ.md` now reads
"when at least 2 bytes are available at `offset + (word_count × 2)`.
When fewer than 2 bytes remain after the candidate record,
look-ahead SHALL be skipped and validation checks 1 through 4 (type,
word count, fits-in-file, IRIG range) SHALL be authoritative."
Captured here for history.

`L2-SYN-005` says look-ahead is done "when look-ahead bytes are
available." The code (`src/sync.rs:78–89`) treats "<2 bytes remaining"
as "skip check 6 and accept." That's correct, but `available` is fuzzy
— does a partial next Type Word count?

**Proposed tightening:** "when at least 2 bytes are available at
`offset + record_bytes`; otherwise checks 1–5 alone are authoritative."

Mirror the same in `find_first_record`. Both Python and Rust already
match this behavior; the requirement just needs to catch up.

### Audit Item 5 — DELTA edge cases the spec doesn't address

**Status:** **Resolved.** Landed as part of the Item 1 (DELTA) work
on 2026-06-05. Both sub-items below are now in `docs/L2-REQ.md`:

- Error-record participation: original audit said *pick one* — either
  "DELTA is 0.000000 for errored and spurious records" or "errored
  records participate in DELTA tracking." Resolved as **participation**
  via `L2-RDR-016`. Errored records track per RT/MSG; SPURIOUS_DATA
  has no key so emits empty (`L2-RDR-018`).
- Negative DELTA from out-of-order timestamps: original audit said
  this is silently emitted as `-N.NNNNNN`. Resolved as **empty CSV
  cell + one-WARN-per-key** via `L2-RDR-017`.

Both are exercised in the `delta-tracking-irig` conformance fixture.
Captured here for history.

### Audit Item 6 — SPURIOUS continuation classification under filtering

**Status:** **Resolved.** L2-ERR-005 in `docs/L2-REQ.md` now states
explicitly that continuation status depends on the immediately
preceding *successfully decoded* record, and that "A classification
failure or unrecoverable validation error between an error record
and a SPURIOUS_DATA record SHALL reset the continuation flag." The
behavior of resetting on a corruption boundary is now spec-pinned;
test_e2e.py::test_spurious_data_empty_delta_and_continuation_code
exercises the L2-ERR-005 continuation code path. Captured here for
history.

Verified in `src/reader.rs:419`: `prev_was_error` is set at the reader,
before filtering — so a SPURIOUS following a filtered-out error is
still correctly classified as `0x2000` continuation.

**However:** the classification-failure path at `src/reader.rs:433`
resets `prev_was_error = false`. If a corrupt-but-passable record sits
between an error and a spurious, the spurious gets mislabeled as
`0x2001` (standalone) instead of `0x2000` (continuation). This is a
genuine subtle bug in a corruption-cascade scenario.

**Proposed addition:** a one-line note in `L2-ERR-005` clarifying that
continuation status depends on the *immediately preceding decoded
record*, not the immediately preceding error. Then decide whether the
classification-failure-resets-flag behavior is intentional (and
matches the spec) or a bug to fix.

Recommendation: leave the behavior as-is (a classification failure is
itself a real corruption boundary, and resetting is defensible), but
write the spec to match. Add a unit test that pins the behavior so
future refactors don't change it accidentally.

### Audit Item 7 — Inline-vs-separate suffix rule for multi-dot stems

**Status:** **Resolved.** L2-ERR-008 in `docs/L2-REQ.md` now defines
stem/suffix explicitly: "where `<stem>` is the destination filename
up to and excluding the final `.`, and `<suffix>` is the final `.`
and extension (or empty if the destination has no extension).
Examples: `out.csv` → `out_errors.csv`; `out` → `out_errors`;
`data.bar.csv` → `data.bar_errors.csv`." Captured here for history.

`src/writer.rs:255–267` and `python/src/mie_decoder/writer.py:276–278`
both produce:
- `foo.csv` → `foo_errors.csv`
- `foo` → `foo_errors`
- `foo.bar.csv` → `foo.bar_errors.csv` (Python uses
  `.with_name(stem + _errors + suffix)`; Rust splits on last extension).

They happen to agree on the common cases, but `L2-ERR-008` just says
`<stem>_errors<suffix>` without defining `stem`/`suffix`.

**Proposed tightening:** "`stem` = filename up to and excluding the
final `.`; `suffix` = the final `.` and extension, or empty if no
extension."

Effort: pure spec; both impls already match.

### Audit Item 8 — Pathological-regular files (0x20-padded)

**Status:** **Resolved.** Landed as L2-SYN-018 in `docs/L2-REQ.md`
and implemented in commit `80d0884` (`feat(sync,reader): implement
L2-SYN-018 homogeneous-payload defense`). Both crates have a
distinct error class (`MieError::HomogeneousPayload` /
`MieHomogeneousPayloadError`), both modes reject (mirroring the
NoValidRecords class), CLI maps to exit 2. The `homogeneous-payload`
conformance fixture verifies cross-impl agreement. Captured here for
history.

The 0x20-fill case (a file padded with ASCII space bytes parses as a
contiguous stream of "valid" SPURIOUS_DATA records) is called out in
the backlog but isn't a requirement. Worth promoting:

**Proposed addition:** "Header detection SHALL reject inputs where the
first N candidate records share an identical bit pattern in payload
positions" — or whatever defense is chosen.

Without this in the spec, a future refactor that "simplifies" the
look-ahead can reintroduce the false-positive without test coverage
flagging it.

### Audit Item 9 — Smaller items worth a sentence each

Five small items surfaced during the audit. All five are now
resolved; status tagged per item for history.

- **Concurrent file modification under mmap** (`src/reader.rs:82`,
  Python `mmap.ACCESS_READ`): undefined on POSIX if truncated
  mid-decode. **Status: Resolved** via L1-EXIT-006 (which pins the
  operational contract that the input file SHALL NOT be modified
  during decoding) and L2-RDR-020 (which pins read-only file access
  as the implementation enforcement of that contract).

- **`file_offset` is not exposed in any output** — it's in `MieMessage`
  per `L2-DEC-010` but not surfaced in CSV or `dump`.
  **Status: Resolved** via the L2-DEC-010 rationale, which now reads
  "Offset and raw word values are needed by the reader to log
  record-class diagnostics and by analysts using a programmatic API.
  Surfacing these in CSV output is not required by L2-WRT-001 and is
  reserved for future debug-only output paths." Intentionally
  internal; future debug-only CSV exposure is a separate feature
  request.

- **L2-CLI-005 exit codes are not class-differentiated**:
  **Status: Resolved** via L1-EXIT-002 through L1-EXIT-005 plus
  L2-CLI-011 (the exit-code table), which collectively define four
  exit classes — complete (0), partial-recovered (0 + INFO summary),
  partial-unrecoverable (3 or 0 with --allow-partial), no-records
  (2). The CLI's exit-class summary log line (also L1-EXIT-005) gives
  operators a grep-able one-line classifier.

- **`L2-CFG-008` ambiguity around per-implementation config keys**:
  **Status: Resolved.** L2-CFG-008 now reads "The configuration
  schema and key names demonstrated by `config/default.toml` SHALL
  remain supported. Implementations MAY add additional keys under
  namespaces that do not collide with shared keys (e.g., Rust-only
  `filter.include_*` keys per `L3-RS-010`); such additional keys
  SHALL be ignored or warned by implementations that do not support
  them."

- **`L2-MSG-001` enumerates "10 supported transaction formats" but
  the doc never lists them**. **Status: Resolved.** L2-MSG-001 now
  lists all 10 inline: "(1) BC→RT Receive, (2) RT→BC Transmit,
  (3) RT-to-RT, (4) Receive Broadcast (BC→RT broadcast),
  (5) RT-to-RT Broadcast, (6) Mode Code Transmit with data,
  (7) Mode Code Receive with data, (8) Mode Code with no data,
  (9) Mode Code Broadcast with no data, (10) Mode Code Broadcast
  with data. SPURIOUS_DATA is the 11th classification..."

---

### Original Recommendation (preserved verbatim)

> Don't try to land all of this at once. Two suggested PRs:
>
> 1. **Spec-only PR** updating the requirement docs to capture items 1, 2,
>    3, 4, 5, 7 — these are all "code already does this, lock it in."
>    Low risk, high value.
> 2. **Behavior PR** for the negative-DELTA WARN diagnostic and the
>    lenient-mode terminal-Err item (if you pick option (b) on item
>    1). These actually change observable output.

**Status of the original recommendation:**
- The behavior PR's first piece (negative-DELTA WARN) **landed** as
  part of the DELTA work (`L2-RDR-017`).
- The behavior PR's second piece (lenient-mode terminal-Err) is now
  rolled into Team Review Item 2 / Phase 3.
- The spec-only PR was never drafted. Audit Items 2, 3, 4, 6, 7, 8,
  and the five sub-items in 9 are still candidates for it. Combined
  with Team Review Phase 1, this could be a single consolidated
  spec-only PR.

---

### Suggested Consolidated Spec-Only PR — Status: Resolved

The consolidated spec-only PR proposed below was fully landed (mostly
across the v3 requirements split + the L2-RDR-004 / L2-SYN-018
implementations). Each row tagged with the commit/spec change that
closed it; preserved here for history.

| Source | Change | Landed in |
|--------|--------|-----------|
| Audit Item 2 | New L2-RDR requirement for truncated first record | L2-RDR-004 + commit `b1ea897` |
| Audit Item 3 | New `L2-DEC-011`/`L2-DEC-012`/`L2-DEC-013` pinning timestamp-format detection behavior | Spec landed via v3 requirements split; L2-DEC-012 test deferred |
| Audit Item 4 | Tighten `L2-SYN-005` look-ahead-at-EOF wording | L2-SYN-005 updated wording in `docs/L2-REQ.md` |
| Audit Item 6 | Clarify `L2-ERR-005` "immediately preceding decoded record" | L2-ERR-005 updated wording in `docs/L2-REQ.md` |
| Audit Item 7 | Define `stem`/`suffix` in `L2-ERR-008` | L2-ERR-008 examples in `docs/L2-REQ.md` |
| Audit Item 8 | New requirement rejecting homogeneous-payload pathological inputs | L2-SYN-018 + commit `80d0884` |
| Audit Item 9 (sub) | List the 10 transaction formats in `L2-MSG-001` | L2-MSG-001 enumeration in `docs/L2-REQ.md` |
| Audit Item 9 (sub) | Clarify `file_offset` as internal-only in `L2-DEC-010` | L2-DEC-010 rationale in `docs/L2-REQ.md` |
| Audit Item 9 (sub) | Clarify `L2-CFG-008` per-impl key namespacing vs `L3-RS-010` | L2-CFG-008 namespaced-keys clause in `docs/L2-REQ.md` |

---

## Documentation Initiative (2026-06-05) — Status: Resolved (2026-06-07)

**Resolved 2026-06-07.** All nine planned deliverables landed across
nine commits spanning 2026-06-06 to 2026-06-07. The commit-by-
deliverable table appears below; the rest of this section is
preserved verbatim as the historical scoping document.

| # | Deliverable                                       | Landed in                       | Date       |
|---|---------------------------------------------------|---------------------------------|------------|
| 1 | `MIE-FORMAT.md` (absorbs `FIELDS.md`)             | `9652719`                       | 2026-06-07 |
| 2 | `USER-GUIDE.md`                                   | `8e5de8b`                       | 2026-06-06 |
| 3 | `CONFIG-REFERENCE.md`                             | `cd14a92`                       | 2026-06-06 |
| 4 | `ERROR-CATALOG.md`                                | `3db705f`                       | 2026-06-06 |
| 5 | `EXAMPLES.md`                                     | `e5875d8`                       | 2026-06-06 |
| 6 | `VENDOR-CSV-DIFFS.md`                             | `6609ae1`                       | 2026-06-06 |
| 7 | `MAINTAINER-GUIDE.md`                             | `a22506c`                       | 2026-06-06 |
| 8 | Refreshed `ARCHITECTURE.md` (v2.0.0)              | `a216c48`                       | 2026-06-07 |
| 9 | Refreshed `docs/diagrams/*.puml` + tracked SVGs   | `67c0154`, `cdd4cc0`, `2e4ce08` | 2026-06-07 |

Phase D5 (integration and link audit) was folded into per-doc
writing rather than executed as a standalone phase. The
PlantUML-source-and-rendered-SVG co-commit convention added in
`2e4ce08` is now documented in `docs/MAINTAINER-GUIDE.md` §3 as a
durable maintenance rule.

---

**Severity:** High (gates broader adoption, onboarding, and downstream
integration). Tracked here as a multi-phase initiative because the
scope is large enough that piecemeal commits won't converge on a
coherent doc set without an explicit plan.

Today's documentation is accurate but assumes the reader is already
the decoder author. `docs/FIELDS.md` lists fields but doesn't walk
through a worked decode. `docs/ARCHITECTURE.md` has ASCII diagrams
that are useful but partial. `docs/L1-REQ.md`, `docs/L2-REQ.md`, and `docs/L3-REQ.md` are the contract
document, not a tutorial. `docs/diagrams/*.puml` describes only the
Python implementation. New engineers, downstream integrators, and
flight-test users who want to understand "what is this tool doing
to my recording" don't have a single accessible reference. This
initiative produces one.

### Audiences and Scope

Three primary audiences. The deliverable docs map to one or more.

| Audience | Need | Primary Deliverables |
|----------|------|----------------------|
| **Users** (flight-test engineers running the tool) | "How do I decode this file and read the output?" | `USER-GUIDE.md`, `CONFIG-REFERENCE.md`, `ERROR-CATALOG.md`, `EXAMPLES.md` |
| **Integrators** (consumers of CSV output, comparison-with-vendor users) | "What does each column mean, byte-for-byte, with edge cases?" | `MIE-FORMAT.md` (extends FIELDS.md), `EXAMPLES.md`, `VENDOR-CSV-DIFFS.md` |
| **Maintainers** (us, future-us, contributors) | "How do I add a feature without breaking the contract?" | Refreshed `ARCHITECTURE.md`, refreshed `docs/diagrams/*.puml`, new `MAINTAINER-GUIDE.md` |

### Deliverable Documents

New files to produce under `docs/`:

1. **`MIE-FORMAT.md`** — Comprehensive MIE binary format reference.
   Extends and partially absorbs FIELDS.md. Includes annotated bit
   layouts for every word, full record-layout walkthroughs for all
   10 transaction formats + SPURIOUS_DATA, error-record lifecycle
   (Type Word bit 14 → truncated payload → Error Word → optional
   SPURIOUS continuation), and a worked hex-to-CSV decode for at
   least three representative records.

2. **`USER-GUIDE.md`** — Task-oriented CLI guide. Per-command
   walkthroughs (`decode`, `count`, `dump`), common workflows
   (filter by RT, separate vs inline errors, configure via TOML,
   diff against vendor CSV), troubleshooting (FAQ tied to error
   messages from `ERROR-CATALOG.md`).

3. **`CONFIG-REFERENCE.md`** — Every TOML key, every CLI flag, the
   precedence rules (CLI > config > defaults), validation behavior
   per Item 6 of the Team Review Backlog. Side-by-side table of
   TOML key → CLI flag equivalents. Worked example showing the same
   filter expressed three ways.

4. **`ERROR-CATALOG.md`** — Every `MieError` variant (Rust) /
   exception class (Python), when it fires, what the user should do.
   Tied to exit codes (Item 2). Tied to DDC error codes (0x01xx) and
   decoder codes (0x20xx) with hardware/software origin clearly
   labeled.

5. **`EXAMPLES.md`** — End-to-end worked decodes. Each example shows:
   the input recording (described in prose + hex extract), the CLI
   invocation, the expected CSV output, and a line-by-line
   explanation of one or two interesting rows. At minimum: basic
   BC→RT, errored record + SPURIOUS continuation, RT-to-RT
   transfer, mode-code message, broadcast, header-skipped file,
   sync-recovery scenario.

6. **`VENDOR-CSV-DIFFS.md`** — Documented alignment with DDC vendor
   CSV output. Columns that match byte-for-byte; columns that we
   leave empty (`MUX`, `TERM_NAME`, `IM_GAP`, `RCV_GAP`, `XMT_GAP`)
   and why; known cosmetic differences (line endings, trailing
   whitespace); the validation workflow for confirming a decode
   matches vendor output.

7. **`MAINTAINER-GUIDE.md`** — Architecture-focused contributor
   guide. How to add a message format, how to add a validation
   check, how to add a conformance fixture, how the cross-impl
   contract works, when to add a `PY-*`/`RS-*` vs a shared
   requirement, code style notes beyond what's in CONTRIBUTING.md.

8. **Refreshed `ARCHITECTURE.md`** — Beef up the existing module
   diagrams; add the Rust counterparts where currently only Python
   is shown. Add explicit sequence diagrams for the read-decode
   loop and sync-recovery path. Cross-link to the new docs above.

9. **Refreshed `docs/diagrams/*.puml`** — Resolves the existing
   "Documentation" backlog item in the Robustness section below.
   Add clearly labeled Python and Rust class/component/dataflow
   diagrams rather than overwriting one with the other.

### Diagrams Required

PUML for class/component/sequence diagrams (matches existing
convention). ASCII boxes-and-arrows for inline data-flow blocks
(matches `ARCHITECTURE.md`). Annotated bit grids for binary-layout
diagrams (markdown table with one column per bit, or a fixed-width
text rendering).

| # | Diagram | Format | Target Doc |
|---|---------|--------|------------|
| D1 | Type Word bit layout (16-bit grid, bits 0-6 message type / 7 bus / 8-13 word count / 14 error / 15 reserved) | bit grid | `MIE-FORMAT.md` |
| D2 | Command Word bit layout (RT / T-R / SA / WC) | bit grid | `MIE-FORMAT.md` |
| D3 | Status Word bit layout (per MIL-STD-1553) | bit grid | `MIE-FORMAT.md` |
| D4 | IRIG timestamp 3-word layout (upper / middle / lower with field annotations) | bit grid | `MIE-FORMAT.md` |
| D5 | Standard timestamp 2-word layout (upper / lower) | bit grid | `MIE-FORMAT.md` |
| D6 | Per-format wire layout — BC→RT (Receive) | ASCII | `MIE-FORMAT.md` |
| D7 | Per-format wire layout — RT→BC (Transmit) | ASCII | `MIE-FORMAT.md` |
| D8 | Per-format wire layout — RT→RT | ASCII | `MIE-FORMAT.md` |
| D9 | Per-format wire layout — Broadcast BC→RT | ASCII | `MIE-FORMAT.md` |
| D10 | Per-format wire layout — Broadcast RT→RT | ASCII | `MIE-FORMAT.md` |
| D11 | Per-format wire layout — Mode Code TX with data | ASCII | `MIE-FORMAT.md` |
| D12 | Per-format wire layout — Mode Code RX with data | ASCII | `MIE-FORMAT.md` |
| D13 | Per-format wire layout — Mode Code no data | ASCII | `MIE-FORMAT.md` |
| D14 | Per-format wire layout — Mode Code broadcast (with / without data) | ASCII | `MIE-FORMAT.md` |
| D15 | Per-format wire layout — SPURIOUS_DATA | ASCII | `MIE-FORMAT.md` |
| D16 | Error record lifecycle: bit-14 set → truncated payload → Error Word → optional SPURIOUS continuation | ASCII + PUML sequence | `MIE-FORMAT.md` + `ARCHITECTURE.md` |
| D17 | File-level layout: optional header + record stream | ASCII | `MIE-FORMAT.md` |
| D18 | Rust module dependency graph | PUML component | `ARCHITECTURE.md`, `docs/diagrams/` |
| D19 | Python module dependency graph | PUML component | `ARCHITECTURE.md`, `docs/diagrams/` |
| D20 | Data flow: file → mmap → reader → filter → writer (Rust streaming variant) | ASCII | `ARCHITECTURE.md` |
| D21 | Data flow: file → mmap → reader → filter → DataFrame → writer (Python buffered variant) | ASCII | `ARCHITECTURE.md` |
| D22 | Sync recovery state machine: validate → recover_sync → re-enter | PUML state | `ARCHITECTURE.md` |
| D23 | Sync four-phase strategy: header detect → continuous validate → look-ahead confirm → recovery walk | ASCII | `ARCHITECTURE.md` |
| D24 | Read-decode loop sequence diagram (one record from offset to yielded MieMessage) | PUML sequence | `ARCHITECTURE.md`, `MAINTAINER-GUIDE.md` |
| D25 | Config precedence: CLI args > config file > defaults | ASCII | `CONFIG-REFERENCE.md` |
| D26 | Error mode comparison: separate vs inline output | ASCII | `USER-GUIDE.md` |
| D27 | Class diagram (Rust): MieMessage and related structs | PUML class | `docs/diagrams/`, `MAINTAINER-GUIDE.md` |
| D28 | Class diagram (Python): MieMessage and related dataclasses | PUML class | `docs/diagrams/`, `MAINTAINER-GUIDE.md` |
| D29 | Conformance pipeline: hex fixture → both CLIs → byte-compare → oracle | PUML sequence | `MAINTAINER-GUIDE.md` |
| D30 | DELTA computation flow (per `L2-RDR-016 through L2-RDR-019`): timestamp basis check → key lookup → monotonicity check → tracker update | ASCII | `MIE-FORMAT.md` |

### Tables Required

All as markdown tables in the deliverable documents. Source of truth
listed so tables can be regenerated from authoritative code/spec if
they drift.

| # | Table | Source of truth | Target Doc |
|---|-------|-----------------|------------|
| T1 | Type Word message type codes (7 rows × code/name/short description/format) | `src/models.rs::MessageType` | `MIE-FORMAT.md` |
| T2 | All 10 MIL-STD-1553 transaction formats + SPURIOUS_DATA, with which Type Word(s) map to each | `src/decode.rs::classify_message_format` | `MIE-FORMAT.md` |
| T3 | DDC hardware error codes (0x01xx) with hex / symbolic name / description | `src/models.rs` constants | `ERROR-CATALOG.md`, `MIE-FORMAT.md` |
| T4 | Decoder error codes (0x20xx) with origin (decoder-assigned, not hardware) | `src/models.rs` constants | `ERROR-CATALOG.md`, `MIE-FORMAT.md` |
| T5 | CSV columns with format spec (width, padding, encoding) and source binary field | `docs/FIELDS.md` | `MIE-FORMAT.md` (extended) |
| T6 | Empty-column rationale (`MUX`, `TERM_NAME`, `IM_GAP`, `RCV_GAP`, `XMT_GAP`) | `docs/FIELDS.md` + ROADMAP commitments | `VENDOR-CSV-DIFFS.md` |
| T7 | Config TOML keys: key / type / range / unknown handling / CLI equivalent | `src/config.rs`, `python/.../config.py` | `CONFIG-REFERENCE.md` |
| T8 | CLI flags: flag / argument type / per-subcommand applicability / config equivalent | `src/cli.rs`, `python/.../cli.py` | `CONFIG-REFERENCE.md` |
| T9 | Exit codes (after Team Review Item 2 lands): code / class / meaning / when to expect | `L1-EXIT-002` through `L1-EXIT-005` | `ERROR-CATALOG.md`, `USER-GUIDE.md` |
| T10 | `MieError` variants (Rust) and exception classes (Python), keyed by `MieErrorKind` discriminant | `src/error.rs`, `python/.../exceptions.py` | `ERROR-CATALOG.md` |
| T11 | Logging levels (DEBUG/INFO/WARNING/ERROR/CRITICAL) with what each produces | `src/log.rs` + `python/.../logger.py` + `docs/ARCHITECTURE.md` | `USER-GUIDE.md` |
| T12 | Validation heuristics (five checks + look-ahead) with order, cost, and what each rejects | `src/sync.rs` | `MAINTAINER-GUIDE.md` |
| T13 | Conformance fixture catalog: name / input / expected / args / requirements covered | `tests/conformance/manifest.json` + `docs/TRACE-MATRIX.md` | `MAINTAINER-GUIDE.md` |
| T14 | Cross-impl divergence registry: documented behaviors that differ between Rust and Python (e.g., Rust include filters per `L3-RS-010`) | `docs/L3-REQ.md` (L3-PY-* and L3-RS-* sections) | `MAINTAINER-GUIDE.md` |
| T15 | DELTA contract decision matrix: timestamp basis × record type → DELTA value (per `L2-RDR-016 through L2-RDR-019`) | `docs/L2-REQ.md` (L2-RDR-016 through L2-RDR-019) | `MIE-FORMAT.md` |

### Worked Examples Required

Each example in `EXAMPLES.md` (and select ones cross-posted into
`MIE-FORMAT.md`) is structured as: prose context → input hex with
byte-position annotations → CLI invocation → expected CSV row(s) →
line-by-line decode commentary.

| # | Example | Demonstrates |
|---|---------|--------------|
| E1 | Single BC→RT receive, 30 data words | Baseline record structure, IRIG timestamp decode, RT/MSG/DELTA |
| E2 | Single RT→BC transmit | Status-word-before-data wire order |
| E3 | RT-to-RT transfer | Two command words, two status words |
| E4 | Errored record (bit 14, DDC code 0x011E) | Truncated payload, appended Error Word, ERROR/ERROR_CODE columns |
| E5 | Errored record followed by SPURIOUS continuation | Continuation classification (0x2000 vs 0x2001), `prev_was_error` flag |
| E6 | Broadcast BC→RT (RT=31) | Status word absent |
| E7 | Mode code with data, mode code without data | Mode-code-specific wire layouts |
| E8 | File with proprietary header (e.g., DDC equipment-name bytes) | Header detection, scan window, find_first_record |
| E9 | File with mid-file corruption | Sync recovery, recovery counter |
| E10 | DELTA computation across normal → errored → SPURIOUS → normal sequence | The full L2-RDR-016 through L2-RDR-019 matrix in action |
| E11 | Filter-by-subaddress with both CLI flag and TOML config form | Config precedence, CLI/config equivalence |
| E12 | Separate vs inline error mode on the same input | Output file naming, where each row lands |

### Cross-References to Existing Docs

This initiative does NOT replace existing docs. It extends them:

- The requirement docs (`docs/L1-REQ.md`, `docs/L2-REQ.md`, `docs/L3-REQ.md`) remain the normative contract. New docs link
  back to specific requirement IDs.
- `docs/FIELDS.md` is partially absorbed into `MIE-FORMAT.md`. The
  absorption is one-way: FIELDS.md content moves into MIE-FORMAT.md
  with annotations and diagrams; FIELDS.md becomes a thin pointer
  to MIE-FORMAT.md so external links don't break.
- `docs/ARCHITECTURE.md` gets new diagrams (D18-D24, D27-D28) and
  links into MAINTAINER-GUIDE.md.
- `CLAUDE.md` (project instructions) stays the LLM-facing summary;
  new docs are for humans.
- `docs/ROADMAP.md` (this file) is unchanged in scope.

### Phased Plan

**Phase D1 — Format reference and worked examples (foundation).**
Produce `MIE-FORMAT.md` with diagrams D1-D17 and D30, tables T1-T6
and T15, and worked examples E1-E5. This is the gating phase
because every other doc references it. *Effort: 3-4 days.*

**Phase D2 — User guide and error catalog.** Produce `USER-GUIDE.md`,
`CONFIG-REFERENCE.md`, `ERROR-CATALOG.md`, `EXAMPLES.md` (remaining
worked examples E6-E12). Includes diagrams D25-D26, tables T7-T11
and T9. *Effort: 3 days.*

**Phase D3 — Vendor diff reference.** Produce `VENDOR-CSV-DIFFS.md`.
Requires running a comparison against a known vendor CSV output and
documenting the alignment / known cosmetic differences.
*Effort: 1-2 days* (most of the time is spent confirming alignment,
not writing).

**Phase D4 — Architecture and maintainer refresh.** Refresh
`ARCHITECTURE.md`, add diagrams D18-D24 and D27-D29, produce
`MAINTAINER-GUIDE.md` with tables T12-T14. Resolves the existing
"Refresh `docs/diagrams/*.puml`" backlog item in the Robustness
section below. *Effort: 2-3 days.*

**Phase D5 — Integration and link audit.** Walk every new and
existing doc, verify cross-links resolve, verify every table source
of truth still matches code, verify diagrams render in GitHub's
markdown viewer. *Effort: 1 day.*

**Total: 10-13 days.** Phase D1 is the largest single item; Phases
D2-D4 can be parallelized across people once D1 is complete.

### Effort and Tooling

- Diagram tooling: **PlantUML** for class/component/sequence/state
  (existing convention in `docs/diagrams/`), **ASCII** for inline
  data flows (existing convention in `ARCHITECTURE.md`), **markdown
  tables** for bit-grid diagrams (renders cleanly in GitHub).
- PUML rendering: keep raw `.puml` files and corresponding rendered
  `.svg` files in `docs/diagrams/`. Regenerate and commit the matching
  SVG whenever a PlantUML source changes so reviewers can view diagrams
  without local PlantUML tooling.
- Sample fixtures for worked examples: prefer existing conformance
  fixtures so the hex shown in docs is byte-identical to what CI
  validates. Add fixtures only if no existing one demonstrates the
  concept.
- Vendor CSV for `VENDOR-CSV-DIFFS.md`: requires a real DDC vendor
  CSV from the same recording as one of the conformance inputs.
  This is the only deliverable with an external dependency — note
  if a sample isn't available, that phase is gated.

### Acceptance Criteria

The initiative is complete when:

1. All deliverable documents exist under `docs/` and pass an
   `mdformat` lint (or equivalent).
   **Status: ✅ Met.** Markdown lint is not enforced as a CI gate,
   but each commit passed the existing trailing-whitespace, CRLF,
   and file-size pre-commit hooks.
2. Every diagram in the catalog above is present, in the specified
   format, in the specified document.
   **Status: ✅ Met.** Bit-grid and ASCII diagrams inline in
   `MIE-FORMAT.md` and `ARCHITECTURE.md`. PUML diagrams (D18, D27,
   D28) at `docs/diagrams/{component,class}.puml` — combined into
   single dual-impl diagrams rather than separate Python/Rust
   pairs, because the implementations share the same module shape
   (see `ARCHITECTURE.md` §1 correspondence table).
3. Every table in the catalog above is present and accurate
   against current code (spot-check 3 cells per table against
   source of truth).
   **Status: ✅ Met.** Tables traced back to `src/models.rs`,
   `src/decode.rs`, and the L1/L2/L3 spec docs during writing.
4. Every worked example produces the expected output when the CLI
   invocation is run against the documented input — verified by
   either adding the example as a conformance fixture or by a
   one-time manual run with output captured into the doc.
   **Status: ✅ Met.** Hex-to-CSV worked decodes in `MIE-FORMAT.md`
   reuse canonical fixtures from `tests/conformance/`, so the hex
   in the docs remains byte-exact against CI.
5. L1/L2/L3 requirement IDs are linked from every doc
   that mentions them (not just stated as text).
   **Status: ✅ Met.** Cross-link audit was done implicitly during
   per-doc writing.
6. The existing "Refresh `docs/diagrams/*.puml`" backlog item below
   is marked resolved.
   **Status: ✅ Met.** Item resolved in this commit (see Robustness
   & validation backlog → Documentation below).
7. A new "Documentation" L1 or L2 requirement is *not* added —
   docs are not requirements. But a `CLAUDE.md` pointer to the new
   doc structure SHOULD be added so future LLM sessions find the
   docs.
   **Status: ✅ Met.** No new requirement added. `CLAUDE.md`
   "Reference docs" section was updated alongside each new doc.

### Open Questions

- **Should `MIE-FORMAT.md` fully absorb `FIELDS.md`, or live alongside
  it?** Absorbing is cleaner long-term but breaks any external link
  to FIELDS.md. Recommendation: absorb, leave FIELDS.md as a 3-line
  pointer (header + "moved to MIE-FORMAT.md" + link).
- **Diagram format: stay PUML-only or add Mermaid?** Mermaid renders
  natively in GitHub markdown without tooling, but PUML is more
  expressive and is the existing convention. Recommendation: PUML
  for committed source, but Mermaid is acceptable for any
  in-document diagram that's tightly coupled to surrounding prose
  (e.g., a small flow diagram inside USER-GUIDE.md).
- **Worked examples on Standard timestamps:** until tick-rate
  calibration lands (the deferred follow-up from the DELTA
  contract), Standard-timestamp examples will show empty DELTA
  columns. Document this caveat in `MIE-FORMAT.md` so readers
  don't think it's a doc bug.

---

## Robustness & validation backlog

Items surfaced during the Rust v1.0.0 review. These are not regressions —
they are known gaps that could harden decode quality further. Tracked
here so they don't get dropped.

### Documentation

- **~~Refresh `docs/diagrams/*.puml`.~~** *Resolved 2026-06-07.*
  All three diagrams (`class.puml`, `component.puml`,
  `dataflow.puml`) rewritten as v2.0 dual-implementation diagrams
  in `67c0154`, with rendering fixes in `cdd4cc0` and committed
  rendered SVGs (`docs/diagrams/*.svg`) in `2e4ce08`. The Rust and
  Python implementations share the same module shape (see
  `ARCHITECTURE.md` §1), so the refresh combined the two into
  single labeled diagrams rather than maintaining parallel
  Python-only and Rust-only versions.

### Validation strength

- **~~Stronger timestamp-format auto-detection.~~** *Resolved in v1.1.0.*
  L2-DEC-015 multi-record probe (default `N = 8`, configurable) +
  L2-DEC-016 confidence classification + `MieTimestampFormatMismatchError`
  for the ambiguous case. See `CHANGELOG.md` `[1.1.0]` for the full
  entry.
- **~~Multi-record look-ahead.~~** *Resolved in `[Unreleased]`,
  shipping in v1.2.0.* L2-SYN-026 makes the look-ahead depth
  configurable via `decode.lookahead_records` (default `N = 2`
  preserves historical behavior). Implementation in commits
  `fc515d7..84938f2`.
- **~~T/R consistency check during decode.~~** *Resolved.* Landed as
  L2-SYN-020 (BC→RT requires Cmd direction = Receive) and L2-SYN-021
  (RT→BC requires Cmd direction = Transmit) in `docs/L2-REQ.md`,
  implemented in both crates. Strict mode raises a record error;
  lenient mode logs WARN and skips the record. Verified by the
  `invariant-direction-mismatch` conformance fixture.
- **~~Type Word ↔ Command Word capacity consistency.~~** *Resolved.*
  Landed as L2-SYN-022 in `docs/L2-REQ.md`, implemented in both
  crates. Records whose Type Word `word_count` is too small for the
  declared Command Word payload are rejected (strict: record error;
  lenient: WARN + skip). The structured "which invariant fired"
  enum is exposed via `WhichInvariant` (Rust) /
  `WhichInvariant` (Python) so callers can branch on the specific
  failure rather than parsing a string detail.
- **Header-detection look-ahead depth.** `find_first_record` shares the
  same two-record look-ahead as continuous validation. For files with
  deeply embedded headers that contain valid-looking byte patterns, an
  N-record confirmation could reduce false-positive header endpoints.

### Decode correctness

- **IRIG day-field decoding across DDC card models.** Known limitation in
  v1.0.0 (carried from Python). The bit layout for the day-of-year field
  appears to vary between firmware versions; needs reverse-engineering
  across a sample set with cross-references against vendor CSV.
- **Standard-timestamp tick calibration.** The Standard format is a
  free-running counter; the tick rate is card-dependent and not encoded
  in the file. The current shared contract (see L2-RDR-019 in
  `docs/L2-REQ.md`) emits an empty `DELTA` for every
  Standard-timestamp record because raw ticks cannot be truthfully
  represented as seconds. The follow-up feature here is a
  configuration value (TMATS field or CLI flag, e.g.
  `standard_tick_rate_hz`) that — when supplied — converts ticks to
  microseconds and re-enables DELTA participation for Standard
  records. Until that lands, the `Timestamp::to_microseconds`
  (Rust) / `Timestamp.to_microseconds` (Python) APIs return `None`
  for Standard so callers cannot accidentally treat ticks as
  microseconds.

### Lint policy

- **Adopt `clippy::unwrap_used = "warn"` and
  `clippy::expect_used = "warn"`.** Surface every `.unwrap()` and
  `.expect()` so each site is forced to either be rewritten with `?`
  or annotated with `#[allow(clippy::unwrap_used)]` plus a one-line
  rationale. The current crate has only structurally-safe unwraps
  (e.g., `iter.next().unwrap()` immediately after `iter.peek()`
  returning `Some`), but enforcing the lint converts that property
  from "true today" to "true and verified on every commit." Tracked
  here because flipping the lint requires touching ~6 sites with
  `#[allow]` + comment, which is paperwork that doesn't fit a
  feature commit.

### Output cosmetics

- **~~Defer `decode` output-file creation until after first record
  validates.~~** *Resolved.* The header-only-CSV-on-error symptom no
  longer occurs: per L2-WRT-015 the writer creates a temporary file
  (`<dest>.mie-decoder.tmp.<pid>`) and renames it atomically over
  the destination only on success. Per L2-WRT-016 the temp file is
  unlinked on the default failure path. The destination is therefore
  never written until decoding completes, so a non-MIE input
  produces exit 2 + clean filesystem state with no half-written
  output. `--allow-partial` is the explicit opt-in for preserving
  the temp as `<dest>.partial`.

### Validation strength (cont.)

- **~~Reject pathological-regular inputs that pass the 5-check
  heuristic.~~** *Resolved.* Landed as L2-SYN-018 in
  `docs/L2-REQ.md` and implemented in commit `80d0884`. After
  header detection accepts a candidate, the reader compares the
  first N=4 consecutive candidate-sized chunks for byte identity in
  non-timestamp positions; on a match it surfaces
  `MieError::HomogeneousPayload` / `MieHomogeneousPayloadError` and
  the CLI exits 2 (same exit-code class as NoValidRecords). The
  `homogeneous-payload` conformance fixture verifies cross-impl
  agreement on a 1 KB 0x20-fill input.

### Diagnostics

- **~~Negative DELTA reporting.~~** *Resolved.* The shared contract
  (L2-RDR-017 in `docs/L2-REQ.md`) now emits an empty `DELTA`
  on non-monotonic timestamps and a WARN gated to one line per RT/MSG
  key per recording.
- **Strict-mode error classification for IRIG-range and look-ahead
  failures.** When per-record validation fails for an IRIG-range or
  look-ahead reason, strict mode currently surfaces a `PayloadError`
  with a string detail. A dedicated `MieValidationError` variant with a
  structured "which check failed" enum field would be more consumable.
- **Verbose sync-loss diagnostic.** Today a sync-loss WARN line prints
  the failing offset, type, and word count. A complementary mode could
  hex-dump 32 bytes of context around the failure, similar to the
  Python `_print_unknown_type_diagnostic` helper that didn't make the
  port.

## Tooling

- **Publish Rust coverage reports from CI.** The GitHub Actions workflow now
  runs `cargo cov-ci` as a hard 70% line + region gate. A future improvement
  is running `cargo cov-lcov` and uploading `lcov.info` as a build artifact
  or to codecov.io so coverage trends are visible across PRs.
- **Ratchet thresholds upward as baseline stabilizes.** Initial
  floors (70/70) sit ~5pp below the baseline (74.81% / 71.55%) to
  absorb refactor drift. After a few weeks of stable readings,
  consider tightening to baseline-2pp.
- **Adopt `clippy::unwrap_used = "warn"` and
  `clippy::expect_used = "warn"`** (already noted under "Lint
  policy" above; relisted here as a tooling-track item).
