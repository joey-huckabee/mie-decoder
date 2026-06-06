# MIE-Decoder Requirements

**Document ID:** MIE-REQ-001
**Version:** 2.0.0

## Scope And Ownership

This repository contains maintained Rust and Python implementations of
MIE-Decoder. Requirements are divided into three ownership classes:

- **Shared (`L1-*`, `L2-*`)** requirements define observable MIE decoding,
  validation, configuration, filtering, and CSV behavior. Both implementations
  SHALL satisfy them.
- **Python (`PY-*`)** requirements apply only to the implementation under
  `python/`.
- **Rust (`RS-*`)** requirements apply only to the implementation at the
  repository root.

Shared behavior is normative even when implementation architecture or CLI
syntax differs. For example, both implementations provide message counting and
inline error output, but Rust exposes `count` and `--inline-errors` while
Python exposes `decode --count` and `--error-mode inline`.

Version 2.0 retains version 1 requirement IDs only when their meaning remains
compatible. Python-only version 1 requirements are reallocated to `PY-*`; Rust
implementation constraints use `RS-*`. Retired and reallocated IDs are listed
at the end of this document.

The cross-implementation suite under `tests/conformance/` verifies selected
shared requirements against byte-exact CSV oracles. Implementation-specific
behavior remains covered by each implementation's own tests.

---

## Non-Requirements (Out of Scope)

These items are explicitly OUT of scope for MIE-Decoder. They are
recorded here so future requests do not get folded into the existing
requirements set without separate analysis.

| ID | Statement |
|----|-----------|
| NR-001 | The MIE Decoder SHALL NOT implement decode functionality for IRIG 106 Chapter 10 / 1553 data. MIE files use a DDC proprietary record format that is distinct from IRIG 106 Chapter 10 packet formats — they differ in file framing, timestamp encoding, metadata layout, and the sub-format conventions used to carry MIL-STD-1553 wire data. Any future request to add IRIG 106 1553 decode SHALL be treated as a new capability requiring separate requirements, design analysis, architecture review, and approval. It SHALL NOT be added as an extension of the existing MIE Decoder. |

---

## L1 - Shared System Requirements

| ID | Requirement |
|----|-------------|
| L1-001 | Each implementation SHALL decode DDC MIE binary recording files containing MIL-STD-1553 bus-monitor captures. |
| L1-002 | Each implementation SHALL produce CSV output matching the DDC vendor-compatible column layout and field formatting defined in `docs/FIELDS.md`. |
| L1-003 | Each implementation SHALL decode IRIG timestamps with microsecond resolution and SHALL support Standard free-running timestamps. |
| L1-004 | Each implementation SHALL correctly decode all supported MIL-STD-1553 message formats and preserve bus wire order. |
| L1-005 | Each implementation SHALL compute per-RT/MSG inter-arrival time (`DELTA`) in seconds when the source timestamp has a known microsecond basis. Where no microsecond basis is available, `DELTA` SHALL be empty. |
| L1-006 | Each implementation SHALL support Bus A and Bus B recordings. |
| L1-007 | Each implementation SHALL provide CLI capabilities for decoding, message counting, configuration, filtering, timestamp selection, logging, and diagnostic dump output. CLI syntax MAY differ. |
| L1-008 | Each implementation SHALL handle truncated final records without crashing. |
| L1-011 | Each implementation SHALL provide configurable diagnostic logging and accept DEBUG, INFO, WARNING, ERROR, and CRITICAL level names. |
| L1-013 | Each implementation SHALL support strict and lenient handling of invalid records. |
| L1-015 | Each implementation SHALL detect proprietary file headers, validate every record, and recover from word-aligned mid-file sync loss. |
| L1-016 | Each implementation SHALL decode DDC error records and SPURIOUS_DATA records and SHALL support separate and inline error output. |
| L1-017 | Each implementation SHALL support TOML configuration with CLI values taking precedence over configuration values and built-in defaults. |
| L1-018 | Each implementation SHALL support message exclusion by type, RT address, bus, and subaddress. |
| L1-019 | Shared CSV and decoding behavior SHALL remain aligned through the cross-implementation conformance suite. |
| L1-020 | Each implementation SHALL provide actionable file, record, configuration, and output errors and SHALL return a non-zero CLI exit code on failure. |
| L1-021 | A decode invocation that finds no valid records SHALL exit with code `2` and SHALL NOT create an output file. |
| L1-022 | A decode invocation that recovers from one or more mid-file sync losses SHALL exit with code `0`, SHALL log an INFO summary naming the recovery count, and SHALL complete the CSV normally. |
| L1-023 | A decode invocation that suffers unrecoverable mid-file sync loss SHALL exit with code `3` by default and SHALL NOT preserve a partial output file. An optional `--allow-partial` CLI flag (and equivalent `decode.allow_partial` configuration key) SHALL downgrade the exit to `0`, log a WARN, and preserve the partial output with a `.partial` suffix appended to the configured destination. |
| L1-024 | The decode command SHALL log a one-line summary on exit naming the exit class: `complete`, `partial-recovered`, `partial-unrecoverable`, or `no-records`. |
| L1-025 | The input file SHALL NOT be modified, truncated, or extended during decoding. Behavior under concurrent modification is implementation-defined and MAY cause termination (POSIX mmap + ftruncate can produce SIGBUS; Windows file locking generally prevents writers but mmap does not grow with extensions). This is an operational contract: implementations make a read-only mmap and depend on the operator for exclusive access. |
| L1-026 | Sync recovery scanning SHALL be bounded. Per-recovery scan distance SHALL NOT exceed `MAX_SCAN_BYTES` (64 KB). Across a full decode invocation, cumulative recovery scan distance SHALL NOT exceed the file size — recovery scans SHALL NOT re-traverse already-scanned bytes. (Satisfied by the existing implementation: `recover_sync` starts at the current offset and each successful recovery advances the offset past the scanned region.) |
| L1-027 | For arbitrary input bytes within the size limits of `usize`, no implementation SHALL panic, segfault, or enter an unbounded loop. All failures SHALL surface as a documented decoder error variant (`MieError` in Rust, a `MieDecoderError` subclass in Python). Verified by a per-implementation deterministic-PRNG fuzz harness. |

---

## L2 - Shared Behavioral Requirements

### L2-DEC - Binary Decoding

| ID | Parent | Requirement |
|----|--------|-------------|
| L2-DEC-001 | L1-001 | A 16-bit Type Word SHALL decode `message_type` from bits 0-6, bus from bit 7, `word_count` from bits 8-13, and error flag from bit 14. |
| L2-DEC-002 | L1-003 | A 3-word IRIG timestamp SHALL decode day, hour, minute, second, microsecond, and freerun fields according to `docs/FIELDS.md`. |
| L2-DEC-002a | L1-002 | IRIG timestamp text SHALL emit exactly six microsecond digits regardless of the decoded value. A microsecond value >= 1_000_000 SHALL be considered unreachable given L2-SYN-004 validation, but if encountered (defensive path) the implementation SHALL truncate to six digits and SHALL log a WARN naming the offending record offset. The formatter SHALL NOT emit more than six microsecond digits under any circumstance. |
| L2-DEC-003 | L1-003 | IRIG decoding SHALL decode the freerun flag from bit 15 of the upper timestamp word. |
| L2-DEC-004 | L1-004 | A 16-bit Command Word SHALL decode RT address, T/R direction, subaddress, and data-word count, where a raw count of zero means 32 words. |
| L2-DEC-007 | L1-003 | A Standard timestamp SHALL decode as a 32-bit free-running counter. |
| L2-DEC-008 | L1-001 | All 16-bit words SHALL be read as little-endian values. |
| L2-DEC-009 | L1-004 | Payload extraction SHALL remain bounded by the Type Word's declared record extent and SHALL NOT consume bytes from a following record. |
| L2-DEC-010 | L1-001 | Decoded records SHALL retain their source byte offset and raw Type and Command Word values where present, in the internal record representation (e.g., `MieMessage`). Surfacing these fields in CSV output is not required by L2-WRT-001 and is reserved for future debug-only output or a programmatic API. |
| L2-DEC-011 | L1-003 | Timestamp-format detection SHALL be file-level: the format is resolved on the first valid record and used unchanged for every subsequent record in the same decode invocation. Per-record re-detection is not permitted. |
| L2-DEC-012 | L1-003 | When IRIG and Standard score equally during auto-detection, IRIG SHALL be selected. Flight-test recordings overwhelmingly use IRIG; this tie-break preserves the most common path. |
| L2-DEC-013 | L1-017 | An explicit `--time-format` CLI flag or `decode.time_format` configuration value SHALL bypass auto-detection and force the chosen format. The chosen format SHALL still be validated against the first record's word count to detect obviously-wrong selections, surfacing a distinct error class in strict mode and a WARN in lenient mode. |

### L2-SYN - Synchronization And Validation

| ID | Parent | Requirement |
|----|--------|-------------|
| L2-SYN-001 | L1-015 | Record validation SHALL reject unknown message types. |
| L2-SYN-002 | L1-015 | Record validation SHALL reject word counts below the timestamp-format minimum or above 63. |
| L2-SYN-003 | L1-015 | Record validation SHALL reject records extending past end-of-file. |
| L2-SYN-004 | L1-015 | IRIG validation SHALL reject hour values >= 24, minute values >= 60, second values >= 60, day-of-year values < 1 or > 366, and microsecond values > 999_999. |
| L2-SYN-004a | L1-015 | When the IRIG freerun flag (bit 15 of the upper timestamp word) is set, the day-of-year range constraint of L2-SYN-004 SHALL NOT apply because the card's free-running oscillator is not calendar-locked. Hour, minute, second, and microsecond constraints still apply. |
| L2-SYN-005 | L1-015 | Record validation SHALL confirm that the next record boundary contains a plausible Type Word when at least 2 bytes are available at `offset + (word_count × 2)`. When fewer than 2 bytes remain after the candidate record, look-ahead SHALL be skipped and validation checks 1 through 4 (type, word count, fits-in-file, IRIG range) SHALL be authoritative. |
| L2-SYN-006 | L1-015 | Header detection SHALL scan from offset zero in 2-byte, word-aligned increments. |
| L2-SYN-007 | L1-015 | Header detection SHALL cap its scan at 64 KB. |
| L2-SYN-008 | L1-015 | Header detection SHALL report when no valid record is found within the scan window. |
| L2-SYN-009 | L1-015 | Sync recovery SHALL scan forward from an invalid boundary in 2-byte, word-aligned increments. |
| L2-SYN-010 | L1-015 | Sync recovery SHALL cap its scan at 64 KB from the invalid boundary. |
| L2-SYN-011 | L1-015 | Sync recovery SHALL report when no valid record is found within the scan window. |
| L2-SYN-012 | L1-011 | Header detection SHALL log detected header size at INFO level. |
| L2-SYN-013 | L1-011 | Sync recovery SHALL log sync loss at WARNING and successful recovery at INFO. |
| L2-SYN-014 | L1-015 | Header detection, continuous decoding, and sync recovery SHALL use the same full record-validation path. |
| L2-SYN-015 | L1-015 | Lenient mode SHALL skip invalid records and continue from a recovered boundary when possible. |
| L2-SYN-016 | L1-013 | Strict mode SHALL stop and surface an error on invalid record validation. |
| L2-SYN-017 | L1-016 | Valid error records and SPURIOUS_DATA records SHALL remain eligible record boundaries during validation and recovery. |
| L2-SYN-018 | L1-015 | Header detection SHALL apply additional defenses against homogeneous-payload inputs. When the first N candidate records (with N >= 4) share identical bytes in payload positions (i.e., excluding positions where the timestamp word naturally varies), the implementation SHALL reject the input with a distinct error class. This defends against pathological files padded with a single byte value (such as 0x20-fill, where `0x20 0x20` parses as a valid SPURIOUS_DATA Type Word and the look-ahead heuristic alone admits the stream). |
| L2-SYN-INV-001 | L1-013, L1-015 | Records with Type Word message type `0x02` (BC→RT) SHALL have a Command Word with `direction = Receive`. Strict mode SHALL surface a record error; lenient mode SHALL log a WARN and skip the record (advance to the next record boundary without emission). |
| L2-SYN-INV-002 | L1-013, L1-015 | Records with Type Word message type `0x04` (RT→BC) SHALL have a Command Word with `direction = Transmit`. Strict mode SHALL surface a record error; lenient mode SHALL log a WARN and skip the record. |
| L2-SYN-INV-003 | L1-013, L1-015 | Type Word `word_count` SHALL be at least `1 (TypeWord) + ts_words + 1 (CommandWord) + payload_words(format, Cmd.data_word_count)`, where `payload_words` is the per-format declared payload size (e.g., `data_word_count + 1` for `Receive` and `Transmit`, `1` for `ModeCodeNoData`, etc.). A record whose Type Word declares a smaller capacity than the Command Word's declared payload is internally inconsistent. Strict mode SHALL surface a record error; lenient mode SHALL log a WARN and skip the record. |
| L2-SYN-INV-004 | L1-013, L1-015 | For RT-to-RT (`0x08`) and Broadcast RT-to-RT (`0x18`) records, the second Command Word's `direction` field SHALL be `Receive`. The first Command Word goes to the transmitting RT (direction = Transmit) and the second to the receiving RT (direction = Receive); a second Command Word with direction = Transmit is internally inconsistent. Strict mode SHALL surface a record error; lenient mode SHALL log a WARN and skip the record. |
| L2-SYN-INV-005 | L1-016 | When a record carries a Status Word, the implementation SHOULD verify that `Status.rt == Cmd.rt`. On mismatch, the implementation SHALL log a WARN naming the offset, both RTs, and the raw Status Word, and SHALL continue emitting the record in both strict and lenient mode. This is an anomaly-class observation (Severity::AnomalyWarn) rather than a corruption rejection because real-bus RT response interference on a multi-drop bus can produce a status word from a different RT than the command targeted; rejecting on this case would produce false negatives on real recordings. |
| L2-SYN-INV-006 | L1-015 | Type Word bit 15 is reserved (`docs/FIELDS.md` lists it as "Reserved for future use. Should be 0."). When a record's Type Word has bit 15 set, the implementation SHALL log a WARN naming the offset and the raw Type Word, and SHALL continue emitting the record in both strict and lenient mode. This is an anomaly-class observation (Severity::AnomalyWarn) rather than a corruption rejection because the bit may be used by undocumented vendor extensions; rejecting it would prevent decoding such recordings. |

Phase 7 invariant severity classes:

- **Severity::Reject** — Strict mode aborts with a record error class (e.g., `MieError::PayloadError`). Lenient mode logs a WARN and skips the record (advances past it without emission). Applies to L2-SYN-INV-001 through L2-SYN-INV-004.
- **Severity::AnomalyWarn** — Both strict and lenient modes log a WARN and continue emitting the record. Used when the bus-protocol or vendor-spec ambiguity makes outright rejection unsafe (real-bus noise, undocumented extensions). Applies to L2-SYN-INV-005 and L2-SYN-INV-006.

### L2-RDR - Reader Behavior

| ID | Parent | Requirement |
|----|--------|-------------|
| L2-RDR-002 | L1-008 | Lenient mode SHALL stop cleanly at a truncated final record. |
| L2-RDR-003 | L1-013 | Strict mode SHALL surface a truncation error when a readable Type Word declares a record extent beyond end-of-file. |
| L2-RDR-004 | L1-013 | Header detection followed by a first-record truncation (the first valid Type Word's declared extent runs past EOF) SHALL surface a distinct error class in strict mode (e.g., `MieError::FirstRecordTruncated`) and SHALL terminate cleanly with zero records emitted in lenient mode. This is the post-header counterpart to L2-RDR-002/003. |
| L2-RDR-005 | L1-020 | Opening a missing input file SHALL surface a file-not-found error. |
| L2-RDR-006 | L1-020 | Opening an empty input file SHALL surface an empty-file error. |
| L2-RDR-007 | L1-004 | Receive records SHALL extract Data Words before Status Word. |
| L2-RDR-008 | L1-004 | Transmit records SHALL extract Status Word before Data Words. |
| L2-RDR-009 | L1-005 | `DELTA` SHALL be calculated against the most recent prior message sharing the same RT and MSG identifier. |
| L2-RDR-010 | L1-005 | The first occurrence of each RT/MSG key SHALL have `DELTA` equal to `0.000000`. |
| L2-RDR-009a | L1-005 | Errored records (Type Word bit 14 set) SHALL participate in `DELTA` tracking — they update the per-RT/MSG cursor and SHALL receive a `DELTA` computed against the prior message sharing the same key. |
| L2-RDR-009b | L1-005 | When a record's timestamp is older than the prior message for the same RT/MSG key, `DELTA` SHALL be empty and the implementation SHALL log a WARN. The WARN SHALL be emitted at most once per RT/MSG key per decoded file to avoid log flooding. |
| L2-RDR-009c | L1-005 | `SPURIOUS_DATA` records have no RT/MSG key and SHALL have an empty `DELTA`; they SHALL NOT update any per-key cursor. |
| L2-RDR-009d | L1-005 | Standard-format timestamps have no known microsecond tick rate. Records carrying a Standard timestamp SHALL have an empty `DELTA` and SHALL NOT participate in per-key tracking until a future tick-rate calibration feature is configured. |
| L2-RDR-015 | L1-015 | Every record SHALL pass the full shared validation path before decoding. |

### L2-MSG - Message Semantics

| ID | Parent | Requirement |
|----|--------|-------------|
| L2-MSG-001 | L1-004 | The decoder SHALL classify all 10 supported MIL-STD-1553 transaction formats plus SPURIOUS_DATA. The supported transaction formats are: (1) BC→RT Receive, (2) RT→BC Transmit, (3) RT-to-RT, (4) Receive Broadcast (BC→RT broadcast), (5) RT-to-RT Broadcast, (6) Mode Code Transmit with data, (7) Mode Code Receive with data, (8) Mode Code with no data, (9) Mode Code Broadcast with no data, (10) Mode Code Broadcast with data. SPURIOUS_DATA is the 11th classification and represents records lacking a Command Word. |
| L2-MSG-002 | L1-006 | Bus SHALL be represented as `A` or `B` in CSV output. |
| L2-MSG-003 | L1-004 | A decoded message SHALL expose an MSG label in `<subaddress><T\|R>` form when a Command Word is present. |

### L2-ERR - Error Record Handling

| ID | Parent | Requirement |
|----|--------|-------------|
| L2-ERR-001 | L1-016 | Type Word bit 14 SHALL identify an errored record. |
| L2-ERR-002 | L1-016 | The final word of an errored record SHALL be decoded as its DDC Error Word. |
| L2-ERR-003 | L1-016 | Known DDC Error Word values SHALL be recognized. |
| L2-ERR-004 | L1-013 | Strict mode SHALL reject unknown DDC Error Word values. |
| L2-ERR-005 | L1-016 | SPURIOUS_DATA immediately following an errored record SHALL use decoder code `0x2000`. "Immediately following" refers to the immediately preceding *successfully decoded* record, not the immediately preceding error record. A classification failure or unrecoverable validation error between an error record and a SPURIOUS_DATA record SHALL reset the continuation flag — the corruption itself is treated as a boundary, and the SPURIOUS_DATA SHALL fall through to `L2-ERR-006` (standalone, `0x2001`). |
| L2-ERR-006 | L1-016 | Standalone SPURIOUS_DATA SHALL use decoder code `0x2001`. |
| L2-ERR-007 | L1-002 | CSV output SHALL include `ERROR` and `ERROR_CODE` columns. |
| L2-ERR-008 | L1-016 | Separate mode SHALL write normal messages to the main CSV and errored/spurious messages to `<stem>_errors<suffix>`, where `<stem>` is the destination filename up to and excluding the final `.`, and `<suffix>` is the final `.` and extension (or empty if the destination has no extension). Examples: `out.csv` → `out_errors.csv`; `out` → `out_errors`; `data.bar.csv` → `data.bar_errors.csv`. |
| L2-ERR-010 | L1-002 | CSV `ERROR` SHALL be empty, `ERROR`, or `SPURIOUS` as appropriate; `ERROR_CODE` SHALL contain the corresponding uppercase hexadecimal code. |
| L2-ERR-011 | L1-016 | Inline mode SHALL write normal, errored, and spurious messages to one CSV. |

### L2-WRT - CSV Output

| ID | Parent | Requirement |
|----|--------|-------------|
| L2-WRT-001 | L1-002 | CSV columns SHALL appear in this order: `TIME_STAMP`, `RT`, `MSG`, `WD01`-`WD32`, `STAT`, `CMD`, `MUX`, `TERM_NAME`, `BUS`, `DELTA`, `ERROR`, `ERROR_CODE`, `IM_GAP`, `RCV_GAP`, `XMT_GAP`. |
| L2-WRT-002 | L1-002 | Unused Data Word columns and unavailable fields SHALL be empty. |
| L2-WRT-003 | L1-002 | Data Words, Status Word, Command Word, and Error Word SHALL use 4-character uppercase hexadecimal without a `0x` prefix. |
| L2-WRT-004 | L1-002 | `DELTA` SHALL use exactly six decimal places when populated, and SHALL be an empty CSV cell when no `DELTA` is computed (see L2-RDR-009a through L2-RDR-009d). |
| L2-WRT-007 | L1-002 | CSV output SHALL support a file destination and stdout. |
| L2-WRT-011 | L1-002 | IRIG timestamp text SHALL use `DAY:HH:MM:SS.uuuuuu`. |
| L2-WRT-012 | L1-002 | CSV output SHALL use LF (`\n`) line endings on every supported platform. |
| L2-WRT-013 | L1-002 | CSV output SHALL preserve the currently-empty vendor compatibility columns. |
| L2-WRT-014 | L1-020 | The decode output path SHALL NOT resolve to the same canonical path as the input file. Implementations SHALL surface a distinct error class (e.g., `MieError::InputOutputCollision` / `MieOutputPathError`) before opening the output. Stdout output is exempt because it has no filesystem identity. |
| L2-WRT-015 | L1-020 | File output SHALL be written via a temporary file in the destination's directory, then renamed atomically over the destination on successful completion. The temp file name SHALL be `<destination>.mie-decoder.tmp.<pid>` so concurrent decoders writing to the same directory do not collide. The temp file SHALL live on the same filesystem as the destination so the rename is atomic. |
| L2-WRT-016 | L1-023 | On a decode failure that triggers the default `partial-unrecoverable` exit class (L1-023), the temp file SHALL be unlinked before the process exits. When `--allow-partial` is in effect, the temp file SHALL instead be renamed to `<destination>.partial` so the operator can inspect it; in that case the original `<destination>` SHALL remain untouched. |
| L2-WRT-017 | L1-020 | Overwrite of an existing destination SHALL succeed by default. An optional `--no-clobber` CLI flag (and equivalent `output.no_clobber` configuration key) SHALL refuse the overwrite and surface a distinct error class. |
| L2-WRT-018 | L1-020 | A broken-pipe condition on stdout output (downstream consumer closed early) SHALL exit `0` with no error. Disk-full and permission errors SHALL surface as a writer error preserving the underlying OS error message. |

### L2-CFG - Configuration And Filtering

| ID | Parent | Requirement |
|----|--------|-------------|
| L2-CFG-001 | L1-017 | TOML configuration SHALL support logging level, timestamp format, strict mode, error mode, exclusion filters, and output format. |
| L2-CFG-003 | L1-017 | Configuration precedence SHALL be CLI values over configuration-file values over built-in defaults. |
| L2-CFG-004 | L1-017 | CLI filter arguments SHALL merge with configuration-file filters. |
| L2-CFG-005 | L1-017 | The CLI SHALL accept a TOML configuration file path. |
| L2-CFG-006 | L1-018 | Exclusion filters SHALL support message type, RT address, bus, and subaddress. |
| L2-CFG-007 | L1-018 | Type filters SHALL accept documented symbolic names and hexadecimal type codes. |
| L2-CFG-008 | L1-017 | The configuration schema and key names demonstrated by `config/default.toml` SHALL remain supported. Implementations MAY add additional keys under namespaces that do not collide with shared keys (e.g., Rust-only `filter.include_*` keys per `RS-010`); such additional keys SHALL be ignored or warned by implementations that do not support them. |
| L2-CFG-009 | L1-017 | Unknown top-level TOML keys SHALL produce a WARN at load time naming the offending `[section] key`, but SHALL NOT fail the load. This permits forward-compatible additions to the schema without breaking older configs. |
| L2-CFG-010 | L1-017 | All schema validations (type, range, enum membership, unknown-key detection) SHALL apply at configuration load time, not at use time. A loaded `DecoderConfig` SHALL represent already-validated state; consumers SHALL NOT perform additional validation. |

#### L2-CFG Schema Reference

The table below pins the accepted TOML keys, their types, valid ranges,
and unknown-value handling. This schema is normative for `L2-CFG-001`,
`L2-CFG-008`, `L2-CFG-009`, and `L2-CFG-010`.

| Key | Type | Range / Enum | Unknown-value handling |
|-----|------|--------------|------------------------|
| `logging.level` | string | one of `DEBUG`/`INFO`/`WARNING`/`WARN`/`ERROR`/`CRITICAL` (case-insensitive) | reject at load time |
| `decode.time_format` | string | one of `auto`/`irig`/`standard` | reject at load time |
| `decode.strict` | bool | TOML boolean only (not coerced from strings) | reject non-bool |
| `decode.error_mode` | string | one of `separate`/`inline` | reject at load time |
| `decode.allow_partial` | bool | TOML boolean only (see L1-023) | reject non-bool |
| `output.format` | string | `csv` is the only valid value in v1 | reject at load time |
| `output.no_clobber` | bool | TOML boolean only (see L2-WRT-017) | reject non-bool |
| `filter.exclude_types` | array of string\|int | per-element validated against `L2-CFG-007` | reject at load time |
| `filter.exclude_rts` | array of int | each in `[0, 31]` (1553 RT range) | reject out-of-range at load time |
| `filter.exclude_buses` | array of string | each in `{A, B}` | reject at load time |
| `filter.exclude_subaddresses` | array of int | each in `[0, 31]` (1553 subaddress range) | reject out-of-range at load time |
| Any unknown `[section] key` | — | — | WARN at load time per L2-CFG-009 |

### L2-FLT - Filtering

| ID | Parent | Requirement |
|----|--------|-------------|
| L2-FLT-001 | L1-018 | Filtering SHALL omit messages matching configured exclusion criteria and yield all other messages unchanged. |
| L2-FLT-002 | L1-018 | Exclusion criteria SHALL use OR logic across configured type, RT, bus, and subaddress filters. |

### L2-CLI - Shared CLI Capabilities

| ID | Parent | Requirement |
|----|--------|-------------|
| L2-CLI-001 | L1-007 | Decode capability SHALL accept one input path. |
| L2-CLI-002 | L1-007 | Decode capability SHALL accept an optional output path. |
| L2-CLI-004 | L1-011 | The CLI SHALL accept a configurable logging level. |
| L2-CLI-005 | L1-020 | Successful commands SHALL return exit code zero; usage or runtime failures SHALL return non-zero. |
| L2-CLI-005a | L1-021, L1-022, L1-023 | Decode exit codes SHALL follow L1-021 through L1-023: `0` on a complete or recovered decode (and on `--allow-partial` partials), `1` on usage or configuration failure, `2` on no-valid-records, `3` on unrecoverable mid-file sync loss without `--allow-partial`. The `count` and `dump` commands inherit `0`, `1`, and `2` but SHALL NOT produce exit `3` because they do not write a streaming output that could be partial. |
| L2-CLI-006 | L1-020 | Human-readable diagnostics SHALL be written to stderr rather than mixed into CSV stdout. |
| L2-CLI-008 | L1-007 | The CLI SHALL provide message-counting capability without requiring CSV output. |
| L2-CLI-009 | L1-007 | The CLI SHALL provide raw and record-aware diagnostic dump capability. |
| L2-CLI-010 | L1-007 | The CLI SHALL accept timestamp-format selection, TOML configuration, and shared exclusion filters. |

### L2-CONF - Cross-Implementation Conformance

| ID | Parent | Requirement |
|----|--------|-------------|
| L2-CONF-001 | L1-019 | Shared conformance inputs SHALL be stored as reviewable hexadecimal text rather than committed `.mie` binary recordings. |
| L2-CONF-002 | L1-019 | The conformance runner SHALL invoke both maintained CLIs and require byte-identical CSV output. |
| L2-CONF-003 | L1-019 | Each implementation's output SHALL match the checked-in CSV oracle. |
| L2-CONF-004 | L1-019 | Expected CSV oracles SHALL be updated only after both implementations agree. |
| L2-CONF-005 | L1-019 | CI SHALL run the conformance suite on every push and pull request. |

---

## Python Implementation Requirements

These requirements apply only to `python/`.

| ID | Parent | Requirement |
|----|--------|-------------|
| PY-001 | L1-001 | The Python implementation SHALL support Python `>=3.10,<3.15`. |
| PY-002 | L1-001 | Python dependencies and packaging SHALL be managed by Poetry with a committed `python/poetry.lock`. |
| PY-003 | L1-001 | The Python package SHALL use the `src/mie_decoder` layout and expose the `mie-decoder` console script. |
| PY-004 | L2-WRT-001 | Python CSV generation SHALL use pandas and SHALL explicitly request LF line endings. |
| PY-005 | L2-CFG-001 | Python TOML parsing SHALL use `tomllib` on Python 3.11+ and `tomli` on Python 3.10. |
| PY-006 | L1-020 | Python errors SHALL inherit from `MieDecoderError`, with file and record subclasses retaining typed details. |
| PY-007 | L1-001 | Python public APIs SHALL use type annotations and documented public interfaces. |
| PY-008 | L1-019 | The Python test suite SHALL run under Python 3.10, 3.11, 3.12, 3.13, and 3.14 in CI. |
| PY-009 | L1-001 | The Python reader SHALL use read-only memory-mapped file access. |
| PY-010 | L2-CLI-008 | Python message counting SHALL remain available through `decode --count`. |
| PY-011 | L2-ERR-011 | Python inline error output SHALL remain available through `--error-mode inline`. |
| PY-012 | L1-027 | Python decode memory usage SHALL be O(record_count) — the writer materializes a `pandas.DataFrame` for the full record stream before writing. A future PY-streaming feature will replace this with a chunked writer; until that lands, the operator SHALL be aware that decoding very large recordings consumes proportional memory. |

---

## Rust Implementation Requirements

These requirements apply only to the root Rust crate.

| ID | Parent | Requirement |
|----|--------|-------------|
| RS-001 | L1-001 | The Rust crate SHALL use edition 2024 with MSRV 1.85 or newer. |
| RS-002 | L1-001 | The Rust crate SHALL keep `memmap2` as its only external runtime dependency unless an additional dependency is explicitly justified. |
| RS-003 | L1-001 | The Rust reader SHALL use read-only memory-mapped file access. |
| RS-004 | L2-WRT-001 | Rust CSV writing SHALL stream records directly to a `Write` implementation without collecting all messages or rows. |
| RS-005 | L1-004 | Rust `DataWords` SHALL use a fixed-capacity inline buffer sized for the MIL-STD-1553 limit of 32 words. |
| RS-006 | L1-020 | Rust fallible APIs SHALL return `Result<T, MieError>` and retain structured details for file, record, and writer failures. |
| RS-007 | L1-001 | The Rust production build SHALL support static `x86_64-unknown-linux-musl` deployment for SLES 12. |
| RS-008 | L2-CLI-008 | Rust message counting SHALL remain available through the `count` subcommand. |
| RS-009 | L2-ERR-011 | Rust inline error output SHALL remain available through `--inline-errors`. |
| RS-010 | L1-018 | Rust MAY additionally provide include filters without requiring equivalent Python CLI syntax. |
| RS-011 | L1-019 | Rust CI SHALL enforce formatting, Clippy warnings-as-errors, all-target tests, and the configured line and region coverage floors. |
| RS-012 | L1-027 | Rust decode memory usage SHALL be O(1) in the record count. The writer streams rows directly to a `BufWriter` (per RS-004) and the `DataWords` payload buffer is inline-fixed (per RS-005); the only per-record allocation is `String` for log messages and the per-key `HashMap` entry in `delta_tracker`, which is bounded by the count of distinct RT/MSG keys (≤ 32 × 32 × 2). |

---

## Verification And Traceability

### Shared Conformance Cases

| Case | Primary Shared Requirements |
|------|-----------------------------|
| `basic-multi-record` | L1-001, L1-002, L1-004 through L1-006; L2-DEC-001, L2-DEC-002, L2-DEC-004, L2-DEC-008; L2-RDR-007 through L2-RDR-010; L2-MSG-002, L2-MSG-003; L2-WRT-001 through L2-WRT-004, L2-WRT-007, L2-WRT-011 through L2-WRT-013 |
| `header-and-sync-recovery` | L1-015; L2-SYN-001 through L2-SYN-003, L2-SYN-005 through L2-SYN-007, L2-SYN-009, L2-SYN-014, L2-SYN-015; L2-RDR-015 |
| `errors-inline` | L1-016; L2-ERR-001 through L2-ERR-003, L2-ERR-005, L2-ERR-007, L2-ERR-010, L2-ERR-011; L2-RDR-009c |
| `exclude-subaddress` | L1-018; L2-CFG-006; L2-FLT-001, L2-FLT-002; L2-CLI-010 |
| `config-filter` | L1-017, L1-018; L2-CFG-001, L2-CFG-005, L2-CFG-006, L2-CFG-008; L2-FLT-001, L2-FLT-002 |
| `delta-tracking-irig` | L1-005; L2-RDR-009, L2-RDR-010, L2-RDR-009a; L2-WRT-004 |

### Implementation Evidence

| Area | Python Evidence | Rust Evidence |
|------|-----------------|---------------|
| Binary decoding and models | `python/tests/test_decode.py`, `python/tests/test_models.py` | Unit tests in `src/decode.rs`, `src/models.rs` |
| Reader, validation, and recovery | `python/tests/test_sync.py`, `python/tests/test_e2e.py` | Unit tests in `src/sync.rs`, `src/reader.rs`; `tests/integration.rs` |
| Errors, filtering, and configuration | `python/tests/test_exceptions.py`, `python/tests/test_config.py` | Unit tests in `src/error.rs`, `src/filter.rs`, `src/config.rs`, `src/cli.rs` |
| CSV behavior | `python/tests/test_e2e.py` | Unit tests in `src/writer.rs`; `tests/integration.rs` |
| Cross-implementation contract | `tests/conformance/run.py` and checked-in fixtures/oracles | Same shared conformance suite |
| Build and compatibility policy | `python/pyproject.toml`, `python/poetry.lock`, `.github/workflows/ci.yml` | `Cargo.toml`, `Cargo.lock`, `.cargo/config.toml`, `.github/workflows/ci.yml` |

Requirements not mapped to a conformance case remain mandatory and are
verified by implementation-specific tests, build metadata, or review. When a
new behavior is intended to be shared, add or update a conformance case where
practical.

## Version 1 ID Reallocation

Version 1 requirements that specified one implementation's technology are no
longer shared requirements:

| Version 1 ID | Version 2 Allocation |
|--------------|----------------------|
| `L1-009` memory-mapped I/O | `PY-009`, `RS-003` |
| `L1-010` pandas CSV generation | `PY-004` |
| `L1-012` Python exception hierarchy | `PY-006`; Rust error allocation is `RS-006` |
| `L1-014` Python/Poetry project policy | `PY-001` through `PY-003`, `PY-007`, `PY-008` |
| `L2-DEC-005` and `L3-010` Python `struct` usage | Shared little-endian behavior is `L2-DEC-008`; Python mechanism is verified by Python tests and review |
| `L2-CFG-002` Python `tomllib`/`tomli` selection | `PY-005` |
| `L2-WRT-005`, `L2-WRT-006`, and `L3-009` pandas/DataFrame design | `PY-004` |
| `L2-CLI-003` Python `--count` syntax | `PY-010`; shared counting capability is `L2-CLI-008` |
| `L2-ERR-009` Python `--error-mode` syntax | `PY-011`; Rust inline syntax is `RS-009` |
| Remaining `L3-*` Python implementation details | `PY-*` requirements or implementation-specific tests and review |

Any version 1 ID absent from the version 2 requirement tables is retired.
