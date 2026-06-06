# MIE-Decoder Roadmap

| Version | Feature |
|---------|---------|
| **Python v1.1.0** | Sync recovery, error handling, config, and filtering. Maintained at `python/`. _(current Python release)_ |
| **Rust v1.0.0** | Rust port. CLI redesign (`--inline-errors`, `--include-*` filters, `count` subcommand, `--format csv` forward-compat). Streaming CSV writer (constant memory). Static musl build for SLES 12. _(current Rust release)_ |
| Python next | Continue feature and robustness work while preserving shared MIE format and CSV behavior. |
| Rust v1.1 | Multi-file input, time-sorted merge to single CSV |
| Rust v2.0 | Data word decoders, additional per-message-type CSVs |
| Rust v3.0 | Apache Parquet output |

The two implementations may release independently. Shared format semantics,
fixtures, and vendor-compatible CSV behavior should remain aligned.

## Shared Commitments

- **`config/default.toml` and TOML config support remain a first-class feature.** The Rust build ships a hand-rolled TOML loader for our config schema; the file format and key names are stable.
- **CSV column layout matches DDC vendor output byte-for-byte.** No reordering or renaming of columns, including currently-empty columns (`MUX`, `TERM_NAME`, `IM_GAP`, `RCV_GAP`, `XMT_GAP`).
- **Sync recovery semantics preserved.** Two-record look-ahead, 64 KB scan cap, error records and SPURIOUS_DATA continuations remain valid records that pass validation.
- **One validation path.** Header skip, normal forward decode, and post-loss recovery all share `sync::validate_record`. There is no weaker fast path.
- **Cross-implementation conformance.** Text-based fixtures under
  `tests/conformance/` exercise shared decoding, recovery, filtering, config,
  error, and CSV behavior against byte-exact output oracles in CI.

## Robustness & validation backlog

Items surfaced during the Rust v1.0.0 review. These are not regressions —
they are known gaps that could harden decode quality further. Tracked
here so they don't get dropped.

### Documentation

- **Comprehensive `docs/REQUIREMENTS.md` refresh.** Separate shared
  behavioral requirements from Python- and Rust-specific implementation
  traceability. The semantic requirements remain relevant to both
  implementations, while tooling and module references need explicit
  implementation ownership.
- **Refresh `docs/diagrams/*.puml`.** Class, dataflow, and component
  diagrams currently describe the Python implementation. Add clearly labeled
  Python and Rust architecture diagrams rather than replacing one with the
  other.

### Validation strength

- **Stronger timestamp-format auto-detection.** Today's scoring uses T/R
  consistency, word-count plausibility, and IRIG range checks against
  only the first record. On ambiguous recordings a wrong choice produces
  garbage timestamps for the whole file. Possible improvements: probe
  more than the first record, expose a per-format confidence score, or
  add a hard-fail "format mismatch detected mid-file" diagnostic.
- **Multi-record look-ahead.** Today's two-record look-ahead can be
  defeated by two consecutive same-shape corruptions. An N-record
  (configurable) look-ahead would catch a wider class of failures at the
  cost of small additional per-record reads.
- **T/R consistency check during decode.** For Type Word 0x02 (BC→RT),
  the Command Word direction bit should be Receive; for 0x04, Transmit.
  Today this is used during auto-detect but not enforced afterwards.
  Adding it as a sixth validation check would catch additional corruption
  modes.
- **Type Word ↔ Command Word capacity consistency.** A record whose
  Type Word claims `word_count = N` but whose Command Word claims
  `data_word_count = M` such that the implied payload can't fit in
  `N` words is internally inconsistent. Today this is mitigated by
  bounding payload reads to the Type Word's record extent (so we
  don't leak past the record), but the resulting CSV row has empty
  data words rather than being flagged as malformed. A future
  validation pass could detect the inconsistency in both lenient
  mode (log + skip) and strict mode (raise `MieValidationError`).
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
  free-running counter; tick rate is card-dependent and not encoded in
  the file. Today `to_total_microseconds()` returns raw counter ticks.
  A future option could accept an external calibration constant (TMATS
  field or CLI flag) and emit true microseconds.

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

- **Defer `decode` output-file creation until after first record
  validates.** Today `write_csv(messages, Some(path))` opens the
  output file and writes the CSV header BEFORE the iterator yields
  its first item. If the very first item is an error (e.g.
  `MieError::NoValidRecords` on a non-MIE input), the program
  correctly returns exit 1 and prints a clear error, but a
  header-only CSV is still left on disk. Cosmetic, not a correctness
  bug, but mildly confusing. Fix: peek the iterator (or capture the
  first item) before opening the output file; on Err, return without
  touching the filesystem. Same applies to `write_csv_split`.

### Validation strength (cont.)

- **Reject pathological-regular inputs that pass the 5-check
  heuristic.** Surfaced while writing the no-valid-records regression
  test: a file padded with 0x20 (ASCII space) bytes parses as a
  contiguous stream of "valid" SPURIOUS_DATA records. The byte pair
  `0x20 0x20` is a Type Word with `message_type = 0x20`
  (SPURIOUS_DATA, valid), `word_count = 32` (valid), and the
  two-record look-ahead sees more identical bytes ahead and accepts
  them too. Possible mitigations: detect homogeneous byte patterns
  in the first N records, require some entropy in payload bytes, or
  add a stronger initial header-detection heuristic for the file
  start specifically.

### Diagnostics

- **Negative DELTA reporting.** Per-RT/MSG delta is a signed `f64`
  difference. Out-of-order timestamps produce negative deltas silently.
  Worth a WARN-level log when this occurs, gated to avoid log-flooding
  on a chronically out-of-order file.
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
