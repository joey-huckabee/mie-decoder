# MIE-Decoder Architecture

**Document ID:** MIE-ARCH-001
**Version:** 2.0.0

How the decoder is organized to turn an MIE binary recording into vendor-compatible CSV. Covers the module structure, the cross-implementation correspondence between the Rust and Python crates, the synchronization strategy, the error pipeline, the structural-invariants subsystem, the output-safety machinery, and the streaming-vs-buffered trade-offs.

Companion docs: [`MIE-FORMAT.md`](MIE-FORMAT.md) (binary format reference), [`ERROR-CATALOG.md`](ERROR-CATALOG.md) (every error class), [`MAINTAINER-GUIDE.md`](MAINTAINER-GUIDE.md) (how to add things), [`L1-REQ.md`](L1-REQ.md) / [`L2-REQ.md`](L2-REQ.md) / [`L3-REQ.md`](L3-REQ.md) (the spec).

---

## 1. Two implementations, one architecture

MIE-Decoder ships as a Rust crate (`src/`) and a Python package (`python/src/mie_decoder/`). They are independent implementations that satisfy the same shared specification and produce byte-identical CSV output (verified by 19 cross-implementation conformance fixtures). The module structure is intentionally aligned so the architecture description fits both.

| Concern | Rust module | Python module |
|---------|-------------|---------------|
| CLI / argument parsing | `src/cli.rs` | `python/src/mie_decoder/cli.py` |
| TOML configuration loader | `src/config.rs` | `python/src/mie_decoder/config.py` |
| Message filtering | `src/filter.rs` | `python/src/mie_decoder/filters.py` |
| Reader pipeline (mmap → records) | `src/reader.rs` | `python/src/mie_decoder/reader.py` |
| Pure decode (bit-level field extraction) | `src/decode.rs` | `python/src/mie_decoder/decode.py` |
| Sync helpers (validate, find first, recover) | `src/sync.rs` | `python/src/mie_decoder/sync.py` |
| Domain models + error code constants | `src/models.rs` | `python/src/mie_decoder/models.py` |
| Error types | `src/error.rs` (single enum) | `python/src/mie_decoder/exceptions.py` (class hierarchy) |
| CSV writer | `src/writer.rs` (streaming) | `python/src/mie_decoder/writer.py` (pandas) |
| Logging | `src/log.rs` (hand-rolled) | `python/src/mie_decoder/logger.py` (stdlib `logging`) |
| Hex dump | `src/dump.rs` | `python/src/mie_decoder/dump.py` |

Per L1-CONF-001 the two implementations must remain aligned on shared format and CSV semantics. Per-implementation requirements (`L3-PY-*` / `L3-RS-*`) cover the technology-specific obligations (pandas / tomllib for Python; memmap2 / streaming `BufWriter` for Rust). See [`L3-REQ.md`](L3-REQ.md) for the per-impl details.

---

## 2. Module dependency diagram (Rust shape; Python has the same topology)

```
┌──────────────────────────────────────────────────────────────┐
│                  src/bin/mie-decoder.rs                      │
│                (delegates to cli::run(argv))                 │
└──────────────────────────┬───────────────────────────────────┘
                           │
                           ▼
┌──────────────────────────────────────────────────────────────┐
│                          cli.rs                              │
│      (hand-rolled argparse: decode / count / dump)           │
└──────┬──────────┬──────────┬──────────┬──────────────────────┘
       │          │          │          │
       ▼          │          │          ▼
┌──────────┐      │          │  ┌──────────────┐
│  log.rs  │      │          │  │  config.rs   │
│ stderr   │      │          │  │ TOML loader  │
│ logger   │      │          │  │DecoderConfig │
└──────────┘      │          │  └──────────────┘
                  │          ▼
                  │  ┌──────────────┐
                  │  │  filter.rs   │
                  │  │ FilterConfig │
                  │  │ + Iter adptr │
                  │  └──────────────┘
                  ▼
          ┌───────────────┐      ┌───────────────────┐
          │  reader.rs    │      │    writer.rs      │
          │ MieFileReader │─────▶│ streaming CSV     │
          │ (Iterator)    │      │ + atomic commit   │
          └───────┬───────┘      │ + .partial path   │
                  │              └───────────────────┘
          ┌───────┴───────┐
          │               │
          ▼               ▼
  ┌───────────────┐ ┌────────────────────────┐
  │  decode.rs    │ │       sync.rs          │
  │ pure decode   │ │ validate_record,       │
  │ + classify    │ │ find_first_record,     │
  │ + invariants  │ │ recover_sync,          │
  │ + Severity    │ │ diagnose_header_scan,  │
  └───────┬───────┘ │ is_homogeneous_payload │
          │         └────────────────────────┘
          ▼
  ┌───────────────────────────────┐
  │          models.rs            │
  │  Enums, structs, DataWords,   │
  │  DDC + decoder error consts   │
  └───────────────┬───────────────┘
                  │
  ┌───────────────┴───────────────┐
  │          error.rs             │    ┌───────────┐
  │  MieError enum + Display      │    │  dump.rs  │
  │  + MieErrorKind discriminant  │    │ hex dump  │
  └───────────────────────────────┘    └───────────┘
```

The only external Rust runtime dependency is `memmap2` (per `L3-RS-002`). Argument parsing, CSV emission, TOML parsing, logging, and error types are all hand-rolled. The Python package depends on `pandas` for the CSV writer (`L3-PY-004`) and `tomllib` / `tomli` for config loading (`L3-PY-005`).

---

## 3. Synchronization strategy

The reader maintains sync through a four-phase approach, all sharing the same validation path per L2-SYN-014.

### Phase 1 — Initial alignment (header detection)

Before decoding begins, `find_first_record()` scans from offset 0 to find the first position that passes the full validation path. This handles:

- Files starting directly with records (offset 0 returned immediately).
- Files with proprietary DDC headers containing ASCII equipment names, configuration data, or padding bytes.

The scan advances in 2-byte (word-aligned) steps and caps at 64 KB (`MAX_SCAN_BYTES`, per L1-SYN-002 / L2-SYN-007).

When `find_first_record` returns None, the reader calls `diagnose_header_scan_failure` (sync module) to distinguish two cases (per L2-RDR-004):

- **`HomogeneousPayload`** — if `is_homogeneous_payload` reports byte-identical candidate records, the file is a single-byte pad (e.g., 0x20-fill that happens to parse as a SPURIOUS_DATA stream). Both modes reject.
- **`FirstRecordTruncated`** — if there's a structurally-valid Type Word at or after the header but its declared extent runs past EOF, surface this distinct class. Strict mode raises; lenient mode terminates cleanly with zero records emitted.
- **`NoValidRecords`** — otherwise the file isn't an MIE recording at all. Both modes raise.

### Phase 2 — Continuous validation

At each record boundary, the reader validates before decoding **using the same sync validation rules as Phases 1 and 4**. There is one validation path; the per-record loop, the header-skip scan, and the recovery walk all share it (L2-SYN-014). A weaker per-record check would let corrupt-but-plausible records pass through and emit garbage rows.

The public boolean `validate_record` API remains the compatibility surface for
scanners. The additive `validate_record_detailed` API returns a
`ValidationFailure` reason so readers can report a precise strict-mode failure
without duplicating validation logic. At DEBUG level, a validation failure also
emits one context line capped at 32 bytes.

### Phase 3 — N-record look-ahead

`validate_record` uses an N-record look-ahead: a candidate is confirmed valid only if the next `N − 1` records each start with a valid Type Word (message type in the known set, word count plausible). The walk advances by each candidate's declared `word_count` so it checks the *next records*, not the next 2-byte positions. This dramatically reduces false positives from coincidental byte patterns. When fewer than 2 bytes remain at any look-ahead position, the walk terminates without rejecting the original candidate — checks 1–5 alone are authoritative for records that don't exist in the file (L2-SYN-005, L2-SYN-026).

The look-ahead depth `N` is configurable via the `decode.lookahead_records` TOML key or the `--lookahead-records` CLI flag, range `[1, 32]`, default `2` (preserves the historical two-record look-ahead from earlier versions). Higher values catch wider classes of consecutive-same-shape corruption — for example, two adjacent fake-record headers that align on plausible Type Words can defeat `N = 2` but be caught at `N = 4`. The cost is small (one Type Word read per extra look-ahead record).

### Phase 4 — Sync recovery (walk forward)

If validation fails mid-file, `recover_sync` scans forward in 2-byte steps until it finds a valid record. If recovery fails within the per-recovery scan window (64 KB), the reader yields a terminal `Err(MieError::UnrecoverableSyncLoss)` / raises `MieUnrecoverableSyncLossError` (L1-EXIT-004). The cumulative recovery scan distance across a full decode is bounded by the file size — recovery scans don't re-traverse already-scanned bytes (L1-SYN-002).

### Validation heuristics (applied in order, fast checks first)

1. Type Word message type (bits 0–6) ∈ known set (L2-SYN-001).
2. Word count ∈ `[min_wc, 63]` (L2-SYN-002).
3. Record does not extend past EOF (L2-SYN-003).
4. IRIG timestamp fields in valid ranges; freerun exemption for day-of-year (L2-SYN-004 / L2-SYN-019).
5. Next record's Type Word also has valid type and word count (L2-SYN-005).

Plus the post-acceptance homogeneity defense (`is_homogeneous_payload`, L2-SYN-018) applied once at header detection.

### Performance

- All checks use O(1) bit operations on 16-bit words.
- No allocations during scanning.
- Look-ahead reads only 2 bytes (the next Type Word).
- Scan advances 2 bytes per step (word-aligned).
- Maximum scan distance caps at 64 KB per recovery.

### Error records, SPURIOUS_DATA, and sync

Error records (Type Word bit 14 set) and SPURIOUS_DATA records are valid records with valid Type Words — they pass sync validation normally (L2-SYN-017) and serve as eligible recovery anchor points. Sync loss only occurs when the DDC card writes truly corrupt data (truncated mid-word, power loss during recording).

---

## 4. Structural invariants subsystem

Beyond the five sync validation checks, the reader applies six **structural invariants** to every decoded record (L2-SYN-020 through L2-SYN-025). These catch corruption patterns where the Type Word + word count are structurally valid but the record's internal fields contradict each other.

Invariants are classified into two severity classes:

| Severity | Strict mode | Lenient mode | Used for |
|----------|-------------|--------------|----------|
| `Reject` | Surface `MieError::PayloadError` and stop | Log WARN and skip the record (advance past it without emission) | Internally inconsistent records that almost certainly indicate corruption |
| `AnomalyWarn` | Log WARN and continue emitting the record | Same | Patterns that may be legitimate (real-bus noise, undocumented vendor extensions) so outright rejection produces false negatives |

The six invariants:

| ID | Severity | What it catches |
|----|----------|-----------------|
| L2-SYN-020 | Reject | Type `0x02` (BC→RT) with Cmd direction = Transmit |
| L2-SYN-021 | Reject | Type `0x04` (RT→BC) with Cmd direction = Receive |
| L2-SYN-022 | Reject | Type Word `word_count` too small for the Cmd Word's declared payload |
| L2-SYN-023 | Reject | RT-to-RT records (`0x08` / `0x18`) where the second Cmd Word's direction isn't Receive |
| L2-SYN-024 | AnomalyWarn | Status Word's RT field doesn't match the Cmd Word's RT (possible multi-drop bus interference) |
| L2-SYN-025 | AnomalyWarn | Type Word bit 15 (reserved) is set (possible undocumented vendor extension) |

Implementation: a `WhichInvariant` enum names which specific invariant fired; the reader logs an L2-SYN diagnostic line containing the offset, the invariant name, and the raw bytes. The same enum is exposed in both crates so callers can branch on the specific failure rather than parsing the diagnostic string.

---

## 5. Error handling pipeline

```
  Record at offset N
        │
        ├── validate_record(N) fails?
        │       │
        │       ├── strict ──── raise (UnknownTypeWord / InvalidTypeWord /
        │       │                       RecordTruncated / PayloadError)
        │       └── lenient ──  recover_sync ──── if exhausted:
        │                                            raise UnrecoverableSyncLoss
        │
        ├── Type Word bit 14 = 1? ──── Error record path:
        │       │                        1. Last word = Error Word (DDC code)
        │       │                        2. Validate code against known set
        │       │                        3. Payload = truncated data words
        │       │                        4. Set prev_was_error = True
        │       │                        5. Yield MieMessage(error_word=code)
        │       │                                ↓
        │       ▼
        │  Record at offset N + record_bytes
        │       │
        │       ├── Type = 0x20 (SPURIOUS_DATA)?
        │       │       │
        │       │       ├── prev_was_error = True  → error_word = 0x2000 (continuation)
        │       │       └── prev_was_error = False → error_word = 0x2001 (standalone)
        │       │
        │       └── Type ≠ 0x20 ── reset prev_was_error, normal decode
        │
        └── Normal record:
                │
                ├── validate_structural_invariants (L2-SYN-020..023) ──── if fails:
                │       strict → raise PayloadError("L2-SYN-020..025: ...")
                │       lenient → log WARN, skip record (advance, continue)
                │
                ├── extract payload per message format
                │
                ├── validate_post_extract_invariants (L2-SYN-023 Cmd2 check)
                │       same strict/lenient policy
                │
                ├── detect_record_anomalies (L2-SYN-024 / 025)
                │       both modes log WARN; record still emitted
                │
                └── yield MieMessage
```

The "Error record path" branch and the "structural invariants" branch are independent — a record can be both errored (bit 14 set) AND fail an invariant. The reader checks bit 14 first; an errored record skips the invariant checks because its payload is truncated by definition.

For the full per-error-class behavior reference (when each fires, strict vs lenient, exit code), see [`ERROR-CATALOG.md`](ERROR-CATALOG.md).

---

## 6. Output safety subsystem

L1-OUT-002 obligates the writer to preserve output destination integrity: atomic writes, refuse-to-overwrite-input, partial cleanup on failure. The implementation:

```
write_csv(messages, dest_path, opts)
  │
  ├── L2-WRT-014: refuse if dest_path resolves to the same file as input
  │       (compared via canonical path; stdout is exempt)
  │
  ├── L2-WRT-017: if opts.no_clobber and dest_path exists → ClobberRefused
  │
  ├── L2-WRT-015: open temp file <dest>.mie-decoder.tmp.<pid> on the SAME
  │       filesystem as dest_path (so rename is atomic). Per L3-WRT-001.
  │
  ├── stream rows through BufWriter<File> wrapped around the temp
  │
  ├── normal completion → atomic rename(temp, dest_path)
  │
  └── failure path:
        ├── opts.allow_partial?
        │   └── yes → rename(temp, <dest_path>.partial); leave dest untouched.
        │            Exit class: complete (allow_partial). Per L3-WRT-002.
        │
        └── no → unlink(temp); leave dest untouched.
                Exit class: partial-unrecoverable (exit 3).
```

The atomic temp+rename is what guarantees a crash or kill mid-decode never produces a half-written destination file. The output destination is touched exactly once, by the final rename, and only when decoding completed cleanly. The L2-WRT-015 cleanup path also covers strict-mode errors raised during decoding — the temp file is unlinked before the exception propagates.

Stdout output (L2-WRT-007) bypasses the temp+rename machinery — it's a stream, not a file with a destination — and inherits broken-pipe-on-stdout semantics (L2-WRT-018: exit 0 with no error).

---

## 7. Error type

The Python class hierarchy and the Rust enum are kept in lockstep. Every variant in one language has a counterpart in the other.

### Rust — single `enum MieError`

```
MieError {
    // File-level (open / format-rejection)
    FileNotFound          { path }
    FileEmpty             { path }
    FileIo                { path, source: io::Error }
    NoValidRecords        { path, scan_bytes }
    HomogeneousPayload    { path, offset, sample_records }
    InputOutputCollision  { path }
    ClobberRefused        { path }

    // Record-level (per-record failures, carry offset)
    InvalidTypeWord       { offset, raw_type_word, word_count }
    UnknownTypeWord       { offset, raw_type_word, message_type }
    RecordTruncated       { offset, record_bytes, available_bytes }
    FirstRecordTruncated  { offset, record_bytes, available_bytes }
    PayloadError          { offset, detail }
    UnknownErrorCode      { offset, error_code }
    UnrecoverableSyncLoss { offset, sync_losses }

    // Output
    WriterError           { destination, source: io::Error }
}
```

`MieError::kind()` returns a `MieErrorKind` discriminant for callers that need to branch on the failure mode without matching on the full enum. `is_file_error()` and `is_record_error()` predicates mirror the two intermediate classes from the Python tree.

### Python — class hierarchy rooted at `MieDecoderError`

```
MieDecoderError                          (base, catches everything)
├── MieFileError
│   ├── MieFileNotFoundError
│   ├── MieFileEmptyError
│   ├── MieNoValidRecordsError
│   ├── MieHomogeneousPayloadError
│   ├── MieInputOutputCollisionError
│   └── MieClobberRefusedError
├── MieRecordError                       (carries `offset`)
│   ├── MieInvalidTypeWordError
│   ├── MieUnknownTypeWordError
│   ├── MieRecordTruncatedError
│   ├── MieFirstRecordTruncatedError
│   ├── MiePayloadError
│   ├── MieUnknownErrorCodeError
│   └── MieUnrecoverableSyncLossError
└── MieWriterError
```

`MieRecordError` is the Python analogue of `MieError::is_record_error()`; `MieFileError` corresponds to `MieError::is_file_error()` plus the non-classified file-shape rejections (NoValidRecords, HomogeneousPayload, InputOutputCollision, ClobberRefused).

For per-variant cause / lenient-vs-strict behavior / exit-code mapping, see [`ERROR-CATALOG.md`](ERROR-CATALOG.md).

---

## 8. Error-mode output

```
  --error-mode separate (default):        --error-mode inline:
  ┌──────────────────────┐                ┌──────────────────────┐
  │  main.csv            │                │  output.csv          │
  │  Normal messages     │                │  All messages        │
  │  ERROR col = empty   │                │  ERROR = ERROR|      │
  │  ERROR_CODE = empty  │                │         SPURIOUS|    │
  └──────────────────────┘                │         empty        │
  ┌──────────────────────┐                │  ERROR_CODE = 0x01xx │
  │  main_errors.csv     │                │              0x20xx  │
  │  Errored + spurious  │                │              empty   │
  │  ERROR = ERROR|      │                └──────────────────────┘
  │         SPURIOUS     │
  │  ERROR_CODE = codes  │
  └──────────────────────┘
```

In separate mode the errors file is **not created** if no error rows occur (lazy creation, per L2-ERR-008). The errors file inherits the same atomic-write contract — if both files would be produced, both are written via temp+rename and both either succeed atomically or neither appears at the destination.

Stdout output forces inline mode (you can't split stdout into two streams).

---

## 9. Data flow

```
  .mie binary file
        │
        ▼
  find_first_record  ←── sync: header detection (scans 64 KB, returns Option<offset>)
        │
        ├── None? ───── diagnose_header_scan_failure ────┐
        │                                                │
        │   ┌─ HomogeneousPayload → raise (exit 2)      │
        │   ├─ FirstRecordTruncated:                    │
        │   │    strict → raise (exit 1)                │
        │   │    lenient → terminate cleanly            │
        │   └─ otherwise → raise NoValidRecords (exit 2)│
        │                                                │
        ▼                                                │
  detect_timestamp_format ←── decode: IRIG vs Standard   │
        │                                                │
        ▼                                                │
  ┌─── for each record ──────────────────────────────────┐
  │                                                      │
  │  validate_record (5 checks + look-ahead)             │
  │     │                                                │
  │     ├── valid ──── classify_message_format           │
  │     │              + validate_structural_invariants  │
  │     │              + extract_payload                 │
  │     │              + validate_post_extract           │
  │     │              + detect_record_anomalies         │
  │     │              + compute DELTA                   │
  │     │              + yield MieMessage                │
  │     │                                                │
  │     └── invalid ── recover_sync ── scan forward      │
  │                        │                             │
  │                        ├── found → continue          │
  │                        └── exhausted → raise         │
  │                                       UnrecoverableSyncLoss
  │                                                      │
  │  apply_filters  ←── filter: exclude by type/RT/      │
  │                     bus/SA (or include on Rust)      │
  │                                                      │
  │  yield MieMessage                                    │
  └──────────────────────────────────────────────────────┘
        │
        ▼
  write_csv / write_csv_split  ←── writer: streaming row → BufWriter
        │                          + atomic temp + rename (L2-WRT-015)
        │                          + .partial on allow_partial (L2-WRT-016)
        │                          + no-clobber check (L2-WRT-017)
        │                          + I/O collision check (L2-WRT-014)
        ▼
  .csv output file(s) — exit class summary logged (L1-EXIT-005)
```

---

## 10. Configuration hierarchy

CLI arguments > config file > built-in defaults (L2-CFG-003). Filter arrays merge across the levels rather than replace (L2-CFG-004).

```
  config/default.toml         Built-in reference config
        │
        ▼
  load_config(path)           Parse TOML, validate at load time (L2-CFG-010),
        │                     WARN on unknown keys (L2-CFG-009)
        ▼
  config.with_overrides()     Merge CLI args on top; filter arrays union
        │
        ▼
  DecoderConfig               Final merged, fully-validated config
    ├── log_level             logging.level
    ├── time_format           decode.time_format
    ├── strict                decode.strict
    ├── error_mode            decode.error_mode
    ├── allow_partial         decode.allow_partial      (L2-WRT-016)
    ├── filters
    │   ├── exclude_types     filter.exclude_types
    │   ├── exclude_rts       filter.exclude_rts
    │   ├── exclude_buses     filter.exclude_buses
    │   └── exclude_subaddrs  filter.exclude_subaddresses
    ├── output_format         output.format
    └── no_clobber            output.no_clobber         (L2-WRT-017)
```

For the full schema reference (every key, its type, valid values, validation behavior, CLI override), see [`CONFIG-REFERENCE.md`](CONFIG-REFERENCE.md).

---

## 11. Logging strategy

Rust's `log.rs` is hand-rolled — no `log` crate, no `env_logger`. A single global `AtomicU8` holds the current level; the `log_debug!`, `log_info!`, `log_warn!`, `log_error!` macros emit to stderr only when the level passes. Python uses the stdlib `logging` module with the same five levels.

The level is set from the CLI `--log-level` flag or the config file's `logging.level`; CLI overrides config (L2-CFG-003).

| Level | What gets logged |
|-------|-----------------|
| DEBUG | Per-record decode trace, CLI parsed arguments, header-skip-zero (`first record at offset 0 (no header)`), record-class details |
| INFO | File open, header detected with size (L2-SYN-012), timestamp format auto-detect, sync recoveries (L2-SYN-013), decode complete with counts, **exit-class summary** (L1-EXIT-005), CSV row counts, progress every 100k msgs |
| WARN | Sync loss (L2-SYN-013), unknown error codes (lenient), freerun timestamps, structural invariant violations (lenient skip), L2-SYN anomalies (L2-SYN-024 status RT mismatch / L2-SYN-025 reserved bit set), non-monotonic timestamps (L2-RDR-017, once per RT/MSG), unclassifiable records (lenient), stdout-forces-inline-mode |
| ERROR | No valid records found, homogeneous-payload rejection, unrecoverable sync loss, file/write failures, first-record truncated (strict) |

Per L2-CLI-006, all diagnostics go to stderr — never mixed into CSV stdout.

---

## 12. Streaming CSV (memory profile)

The two implementations make different memory tradeoffs.

| Implementation | Per-record cost | Total memory | Notes |
|----------------|-----------------|--------------|-------|
| **Rust** | ~0 bytes (inline `DataWords` + bounded log buffers) | O(1) in record count | Streams rows directly to a `BufWriter<File>`. The only growable per-decode allocation is `delta_tracker: HashMap<u32, u64>` whose keys are bounded by `RT × SA × direction ≤ 32 × 32 × 2 = 2048`. Tracked as `L3-RS-012`. |
| **Python** | One `dict` per row (~1–2 KB) | O(record_count) | The writer materializes the entire `pandas.DataFrame` before flushing. Decoding a recording with 10 M records consumes ~5 GB RSS. Tracked as `L3-PY-012`. A future `PY-streaming` change will replace this with a chunked writer. |

For the Rust crate, decoding a 10 GB recording uses the same memory as decoding a 10 MB recording. The streaming property is load-bearing: changes to the writer that buffer rows (e.g., a `Vec<Row>` collection step) would break L3-RS-012 and must be rejected at review.

---

## 13. Data Words container

`models::DataWords` (Rust) replaces a `Vec<u16>` with a fixed-capacity inline buffer:

```rust
pub struct DataWords {
    buf: [u16; MAX_DATA_WORDS],   // MAX_DATA_WORDS = 32, the MIL-STD-1553B per-transaction maximum
    len: u8,
}
```

This avoids one heap allocation per decoded record. For files with millions of records this is a measurable win. The Python implementation uses a `tuple[int, ...]` (immutable, hashable, slightly cheaper than a list); the structural invariant is the same — at most 32 data words per record (L3-RS-005).

---

## 14. Operational limits

### Concurrent modification of the input (L1-EXIT-006)

The input file is opened with a read-only mmap (L2-RDR-020). POSIX `ftruncate` on a mapped file can produce SIGBUS on subsequent access; Windows generally locks the file against truncate while mapped but mmap does not grow if the file is extended. The operator is responsible for exclusive access for the duration of the decode (L1-EXIT-006 is an operational contract, not a behavior the program can enforce).

### Sync-recovery scan bounds (L1-SYN-002)

Each `recover_sync` invocation scans at most `MAX_SCAN_BYTES` (64 KB) forward from the failed boundary. Across a full decode, cumulative recovery scan distance is bounded by the file size — `recover_sync` starts at the current offset and every successful recovery advances the offset past the scanned region, so the iterator can never re-traverse already-scanned bytes. The combination guarantees terminating iteration even on pathological inputs.

### Fuzz harness (L1-ROB-001)

Both implementations carry a deterministic-PRNG fuzz harness that feeds 256 random byte sequences (32 B – 8 KB each) through the `MieFileReader → message iterator` path and asserts that every outcome is either a successfully decoded message or a documented decoder error variant — never a panic, `IndexError`, `struct.error`, or unbounded iteration. The harnesses use a shared xorshift64 PRNG with seed `0x0DDCD1ECDDC0DEC0`, so a failure in one implementation is exactly reproducible against the other.

- Rust: `tests/integration.rs::fuzz_arbitrary_bytes_never_panic`
- Python: `python/tests/test_e2e.py::TestFuzzHarness`

The default-suite iteration count (256) is sized so the harness completes in a few seconds per implementation. CI environments that want a longer burn-in can override via a separate follow-on smoke test outside the default suite.

---

## 15. Cross-implementation conformance

The 19-case suite under `tests/conformance/` materializes hex-text inputs into temporary `.mie` files, invokes both CLIs, and requires byte-identical CSV output (or matching exit code for negative cases). The suite runs in CI on every push and pull request (L1-CONF-001 / L2-CONF-005).

Adding a new case is a four-step operation: hex fixture under `inputs/`, oracle CSV under `expected/` (or `expected_exit` for negative cases), entry in `manifest.json`, run locally to verify cross-impl agreement. See [`MAINTAINER-GUIDE.md`](MAINTAINER-GUIDE.md) §6 for the full procedure.

The suite is the regression net: every cross-implementation drift caught in the wild becomes a permanent fixture so it can't silently regress.

---

## 16. See also

- [`MIE-FORMAT.md`](MIE-FORMAT.md) — binary format reference (what the decoder consumes).
- [`USER-GUIDE.md`](USER-GUIDE.md) — operator workflow.
- [`EXAMPLES.md`](EXAMPLES.md) — runnable cookbook.
- [`CONFIG-REFERENCE.md`](CONFIG-REFERENCE.md) — TOML schema.
- [`ERROR-CATALOG.md`](ERROR-CATALOG.md) — per-error-class reference.
- [`VENDOR-CSV-DIFFS.md`](VENDOR-CSV-DIFFS.md) — alignment vs vendor CSV.
- [`MAINTAINER-GUIDE.md`](MAINTAINER-GUIDE.md) — how to add things.
- [`L1-REQ.md`](L1-REQ.md) / [`L2-REQ.md`](L2-REQ.md) / [`L3-REQ.md`](L3-REQ.md) — normative spec.
- [`TRACE-MATRIX.md`](TRACE-MATRIX.md) — auto-generated forward trace.
