# MIE-Decoder Architecture

**Document ID:** MIE-ARCH-001
**Version:** 1.0.0

---

## Module Dependency Diagram

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
          ┌───────────────┐      ┌──────────────┐
          │  reader.rs    │      │  writer.rs   │
          │ MieFileReader │─────▶│ streaming CSV│
          └───────┬───────┘      └──────────────┘
                  │
          ┌───────┴───────┐
          │               │
          ▼               ▼
  ┌───────────────┐ ┌───────────┐
  │  decode.rs    │ │  sync.rs  │
  │ pure decode   │ │ validate  │
  │ + classify    │ │ find_first│
  └───────┬───────┘ │ recover   │
          │         └───────────┘
          ▼
  ┌───────────────────────────────┐
  │          models.rs            │
  │  Enums, structs, DataWords,   │
  │  error code constants         │
  └───────────────┬───────────────┘
                  │
  ┌───────────────┴───────────────┐
  │          error.rs             │    ┌───────────┐
  │  MieError enum + Display      │    │  dump.rs  │
  └───────────────────────────────┘    │ hex dump  │
                                       └───────────┘
```

The only external crate dependency is `memmap2`. All other concerns
(argument parsing, CSV emission, TOML parsing, logging, error type) are
implemented in this crate.

## Synchronization Strategy

The reader maintains sync through a four-phase approach:

### Phase 1: Initial Alignment (Header Detection)

Before decoding begins, \`find_first_record()\` scans from offset 0 to
find the first position that passes multi-point validation. This handles:

- Files starting directly with records (offset 0 returned immediately).
- Files with proprietary DDC headers containing ASCII equipment names,
  configuration data, or padding bytes.

The scan advances in 2-byte (word-aligned) steps and caps at 64 KB.

### Phase 2: Continuous Validation

At each record boundary, the reader validates before decoding **using the
same `sync::validate_record()` function as Phases 1 and 4**. There is one
validation path; the per-record loop, the header-skip scan, and the
recovery walk all share it. The full check set (see "Validation
Heuristics" below) is applied to every record, not just on first-record
discovery and post-recovery.

This is load-bearing: a weaker per-record check (e.g., type + word_count +
fit only) lets corrupt-but-plausible records — a record with valid Type
Word framing but an out-of-range IRIG hour, or one whose word count points
to garbage instead of the next valid Type Word — pass through and emit
garbage rows. The reader must use the strongest available heuristic on
every record.

### Phase 3: Look-Ahead Confirmation

\`validate_record()\` uses a two-record look-ahead: a candidate is
confirmed valid only if the NEXT record (at offset + word_count × 2)
also starts with a valid Type Word. This dramatically reduces false
positives from coincidental byte patterns. Because Phase 2 shares this
validator, look-ahead is applied to every record in normal forward
decode, not only when re-acquiring sync.

### Phase 4: Sync Recovery (Walk Forward)

If validation fails, \`recover_sync()\` scans forward in 2-byte steps
until it finds a valid record. If recovery fails within the scan
window (64 KB default), iteration stops.

### Validation Heuristics (applied in order, fast checks first)

1. Type Word message type (bits 0–6) ∈ VALID_MESSAGE_TYPES
2. Word count ∈ [min_wc, 63] (6-bit field maximum)
3. Record does not extend past EOF
4. IRIG timestamp fields in valid ranges (hour < 24, minute < 60, second < 60)
5. Next record's Type Word also has valid type and word count

### Performance

- All checks use O(1) bit operations on 16-bit words.
- No string allocations during scanning.
- Look-ahead reads only 2 bytes (the next Type Word).
- Scan advances 2 bytes per step (word-aligned).
- Maximum scan distance caps at 64 KB.

### Error Records and Sync

Error records (Type Word bit 14 set) and SPURIOUS_DATA continuations
are valid records with valid Type Words. They pass sync validation
normally. Sync loss only occurs when the DDC card writes truly corrupt
data (e.g., truncated mid-word, power loss during recording).

## Error Handling Pipeline

\`\`\`
  Record at offset N
        │
        ├── Type Word bit 14 = 0? ──── Normal decode ──── yield MieMessage
        │
        └── Type Word bit 14 = 1? ──── Error record:
                │                        1. Last word = Error Word (DDC code)
                │                        2. Validate error code against known set
                │                        3. Payload = truncated data words
                │                        4. yield MieMessage(error_word=code)
                │                        5. Set prev_was_error = True
                ▼
  Record at offset N + record_bytes
        │
        ├── Type = 0x20 (SPURIOUS_DATA)?
        │       │
        │       ├── prev_was_error = True ──── error_word = 0x2000 (continuation)
        │       └── prev_was_error = False ─── error_word = 0x2001 (standalone)
        │
        └── Type ≠ 0x20 ──── Normal decode (new transaction)
\`\`\`

## Error Mode Output

\`\`\`
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
\`\`\`

## Data Flow

\`\`\`
  .mie binary file
        │
        ▼
  find_first_record()          ←── sync.py: header detection
        │
        ▼
  detect_timestamp_format()    ←── decode.py: IRIG vs Standard
        │
        ▼
  ┌─── for each record ──────────────────────────────────────────┐
  │                                                              │
  │  validate (type, word_count, next record look-ahead)         │
  │     │                                                        │
  │     ├── valid ──── decode ──── classify format ──── extract   │
  │     │                             payload                    │
  │     │                                                        │
  │     └── invalid ── recover_sync() ── scan forward            │
  │                        │                                     │
  │                        ├── found ── continue at new offset   │
  │                        └── not found ── stop iteration       │
  │                                                              │
  │  apply_filters()       ←── filters.py: exclude by type/RT/   │
  │                             bus/SA                           │
  │                                                              │
  │  yield MieMessage                                            │
  └──────────────────────────────────────────────────────────────┘
        │
        ▼
  write_csv() or write_csv_split()     ←── writer.py: pandas CSV
        │
        ▼
  .csv output file(s)
\`\`\`

## Configuration Hierarchy

CLI arguments > config file > built-in defaults.

\`\`\`
  config/default.toml         Built-in reference config
        │
        ▼
  load_config(path)           Parse TOML, validate values
        │
        ▼
  config.with_overrides()     Merge CLI args on top
        │
        ▼
  DecoderConfig               Final merged configuration
    ├── log_level
    ├── time_format
    ├── strict
    ├── error_mode
    ├── filters
    │   ├── exclude_types
    │   ├── exclude_rts
    │   ├── exclude_buses
    │   └── exclude_subaddresses
    └── output_format
\`\`\`

## Error Type

The Python class hierarchy collapses to a single Rust enum, `error::MieError`.
A `kind()` method returns a `MieErrorKind` discriminant for callers that
need to branch on the failure mode without matching on the full enum.

```
MieError {
    FileNotFound      { path }                                 // file-level
    FileEmpty         { path }                                 // file-level
    FileIo            { path, source: io::Error }              // file-level
    InvalidTypeWord   { offset, raw_type_word, word_count }    // record-level
    UnknownTypeWord   { offset, raw_type_word, message_type }  // record-level
    RecordTruncated   { offset, record_bytes, available }      // record-level
    PayloadError      { offset, detail }                       // record-level
    UnknownErrorCode  { offset, error_code }                   // record-level
    WriterError       { destination, source: io::Error }       // output
}
```

`is_file_error()` and `is_record_error()` helpers mirror the two intermediate
classes in the Python tree.

## Logging Strategy

`log.rs` is a hand-rolled module — no `log` crate, no `env_logger`. A single
global `AtomicU8` holds the current level; the `log_debug!`, `log_info!`,
`log_warn!`, `log_error!` macros emit to stderr only when the level passes.
The level is set from the CLI `--log-level` flag (or the config file's
`logging.level`); CLI overrides config.

| Level | What gets logged |
|-------|-----------------|
| DEBUG | Per-record decode trace, CLI args, header-skip-zero info |
| INFO | File open, header detected, timestamp format auto-detect, decode complete with counts, sync recoveries, CSV row counts, progress every 100k msgs |
| WARN  | Sync loss, unknown error codes (lenient), freerun timestamps, unclassifiable records (lenient), stdout-forces-inline-mode |
| ERROR | No valid records found, unrecoverable sync loss, file/write failures |

## Streaming CSV (memory profile)

In contrast to the Python writer, which built a `pandas.DataFrame` in memory
before flushing, the Rust `writer::CsvWriter` writes each row directly to a
`BufWriter<File>` (or stdout). Memory use is constant, dominated by the
`BufWriter` capacity. Decoding a 10 GB recording uses the same memory as
decoding a 10 MB recording.

## Data Words container

`models::DataWords` replaces Python's `tuple[int, ...]` with a fixed-capacity
inline buffer:

```rust
pub struct DataWords {
    buf: [u16; 32],   // MIL-STD-1553B caps a transaction at 32 words
    len: u8,
}
```

This avoids one heap allocation per decoded record. For files with
millions of records this is a measurable win.

## Performance and Limits (Phase 8)

### Memory model

| Implementation | Per-record cost | Total memory | Notes |
|----------------|----------------|--------------|-------|
| **Rust** | ~0 bytes (inline `DataWords` + bounded log buffers) | O(1) in record count | Streams rows directly to a `BufWriter<File>`. The only growable per-decode allocation is `delta_tracker: HashMap<u32, u64>` whose keys are bounded by `RT × SA × direction` ≤ 32 × 32 × 2 = 2048. Tracked as `L3-RS-012`. |
| **Python** | One `dict` per row (~1–2 KB) | O(record_count) | The writer materialises the entire pandas `DataFrame` before flushing. Decoding a recording with 10 M records consumes ~5 GB RSS. Tracked as `L3-PY-012`. A future `PY-streaming` change will replace this with a chunked writer. |

### Operational limits (`L1-EXIT-006`, `L1-SYN-002`)

- **No concurrent modification.** The input file is opened with a
  read-only mmap. POSIX `ftruncate` on a mapped file can produce
  SIGBUS on subsequent access; Windows generally locks the file
  against truncate while mapped but mmap does not grow if the file
  is extended. The operator is responsible for exclusive access for
  the duration of the decode.
- **Sync-recovery scan bounds.** Each `recover_sync` invocation scans
  at most `MAX_SCAN_BYTES` (64 KB) forward from the failed boundary.
  Across a full decode, cumulative recovery scan distance is bounded
  by the file size — `recover_sync` starts at the current offset and
  every successful recovery advances the offset past the scanned
  region, so the iterator can never re-traverse already-scanned
  bytes. The combination guarantees terminating iteration even on
  pathological inputs.

### Fuzz harness (`L1-ROB-001`)

Both implementations carry a deterministic-PRNG fuzz harness that
feeds 256 random byte sequences (32 B – 8 KB each) through the
`MieFileReader → message iterator` path and asserts that every
outcome is either a successfully decoded message or a documented
decoder error variant — never a panic, `IndexError`, `struct.error`,
or unbounded iteration. The harnesses use a shared xorshift64 PRNG
with seed `0x0DDCD1ECDDC0DEC0`, so a failure in one implementation
is exactly reproducible against the other.

- Rust: `tests/integration.rs::fuzz_arbitrary_bytes_never_panic`
- Python: `python/tests/test_e2e.py::TestFuzzHarness`

The default-suite iteration count (256) is sized so the harness
completes in a few seconds per implementation. CI environments that
want a longer burn-in can override `ITERATIONS` via a separate
follow-on smoke test outside the default suite.
