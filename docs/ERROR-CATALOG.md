# MIE-Decoder — Error Catalog

Operator-facing reference for every error and diagnostic the MIE-Decoder CLI and libraries can surface. Use this when:

- The CLI exited non-zero and you need to know what happened.
- The CSV `ERROR_CODE` column contains an unfamiliar code.
- You see a `WARN` line in the stderr log and need to know whether it's actionable.
- You're integrating one of the libraries (Rust crate or Python package) and need to know which error class to catch.

This doc covers the **observable surface**. Spec rationale lives in [`L1-REQ.md`](L1-REQ.md), [`L2-REQ.md`](L2-REQ.md), and [`L3-REQ.md`](L3-REQ.md); a forward trace from each requirement to its test artifacts lives in [`TRACE-MATRIX.md`](TRACE-MATRIX.md).

---

## 1. CLI exit codes

The four exit-code classes are pinned by L1-EXIT-001 through L1-EXIT-004 and L2-CLI-011. Every decode invocation logs a one-line `decode exit class:` summary (L1-EXIT-005) so the class is grep-able even when only stderr is captured.

| Code | Class | Triggering errors | What it means | Operator action |
|------|-------|-------------------|---------------|-----------------|
| **0** | `complete` | (none — decode finished normally) | Every record decoded without sync loss. | None. |
| **0** | `partial-recovered` | (none — decode finished after sync loss recovery) | At least one mid-file sync loss occurred and was recovered. INFO summary names the recovery count. | Investigate the recording source if recovery counts are high or trending up. |
| **0** | `complete` (`--allow-partial`) | `UnrecoverableSyncLoss` on the unrecoverable-but-tolerated path | An unrecoverable sync loss occurred but `--allow-partial` preserved the rows decoded so far as `<dest>.partial`. | Inspect the `.partial` output, then triage the recording. |
| **0** | `complete (broken-pipe on stdout)` | `BrokenPipeError` on stdout output | A downstream consumer closed early (e.g. `mie-decoder decode … \| head`). Not an error. | None. |
| **1** | (record / usage error) | `RecordTruncated`, `FirstRecordTruncated`, `PayloadError`, `InvalidTypeWord`, `UnknownTypeWord`, `UnknownErrorCode`, `WriterError` (non-broken-pipe), CLI usage errors, file I/O errors | Per-record validation failed in strict mode, the CLI was misused, or the output sink failed. | Read the stderr message; if it's a record error, lenient mode (`decode.strict = false`) usually skips and continues. |
| **2** | `no-records` | `NoValidRecords`, `HomogeneousPayload`, `TimestampFormatMismatch` (strict mode only) | The input file isn't an MIE recording at all (wrong file type, single-byte-pad, ambiguous timestamp format, etc.). No output file is created. | Verify the input path. If it's actually a recording, check that records begin within the first 64 KB and that the timestamp format is recognizable; pass `--time-format irig\|standard` to override auto-detection. |
| **3** | `partial-unrecoverable` | `UnrecoverableSyncLoss` without `--allow-partial` | A mid-file sync loss could not be recovered within the 64 KB scan window. | Re-run with `--allow-partial` to keep the rows decoded before the loss as `<dest>.partial`, then triage the recording. |

The `count` and `dump` subcommands inherit `0`, `1`, and `2` only — they don't write a streaming output that could be partial, so exit `3` cannot occur (L2-CLI-011).

---

## 2. Library exception / error hierarchy

The Python package exposes a class hierarchy rooted at `MieDecoderError`; the Rust crate exposes a single `MieError` enum with a `kind()` discriminant. The two are kept in lockstep — every variant has a counterpart in the other.

### Python (`mie_decoder.exceptions`)

```
Exception
└── MieDecoderError                  (catch-all base)
    ├── MieFileError                 (wrong file / file-system issue)
    │   ├── MieFileNotFoundError
    │   ├── MieFileEmptyError
    │   ├── MieNoValidRecordsError
    │   ├── MieHomogeneousPayloadError
    │   ├── MieTimestampFormatMismatchError
    │   ├── MieInputOutputCollisionError
    │   └── MieClobberRefusedError
    ├── MieRecordError               (per-record problem; carries an offset)
    │   ├── MieInvalidTypeWordError
    │   ├── MieUnknownTypeWordError
    │   ├── MieRecordTruncatedError
    │   ├── MieFirstRecordTruncatedError
    │   ├── MiePayloadError
    │   ├── MieUnknownErrorCodeError
    │   └── MieUnrecoverableSyncLossError
    └── MieWriterError               (output write failed)
```

### Rust (`mie_decoder::MieError`)

A single `enum MieError { … }` with the same 15 variants. `MieError::kind()` returns a `MieErrorKind` discriminant; `is_file_error()` / `is_record_error()` predicates mirror the Python class split.

```
MieError ├── FileNotFound             ── MieErrorKind::FileNotFound             (is_file_error)
         ├── FileEmpty                ── MieErrorKind::FileEmpty                (is_file_error)
         ├── FileIo                   ── MieErrorKind::FileIo                   (is_file_error)
         ├── NoValidRecords           ── MieErrorKind::NoValidRecords
         ├── HomogeneousPayload       ── MieErrorKind::HomogeneousPayload
         ├── TimestampFormatMismatch  ── MieErrorKind::TimestampFormatMismatch
         ├── InputOutputCollision     ── MieErrorKind::InputOutputCollision
         ├── ClobberRefused           ── MieErrorKind::ClobberRefused
         ├── InvalidTypeWord          ── MieErrorKind::InvalidTypeWord          (is_record_error)
         ├── UnknownTypeWord          ── MieErrorKind::UnknownTypeWord          (is_record_error)
         ├── RecordTruncated          ── MieErrorKind::RecordTruncated          (is_record_error)
         ├── FirstRecordTruncated     ── MieErrorKind::FirstRecordTruncated     (is_record_error)
         ├── PayloadError             ── MieErrorKind::PayloadError             (is_record_error)
         ├── UnknownErrorCode         ── MieErrorKind::UnknownErrorCode         (is_record_error)
         ├── UnrecoverableSyncLoss    ── MieErrorKind::UnrecoverableSyncLoss
         └── WriterError              ── MieErrorKind::WriterError
```

Python `mie_decoder.MieFileError` is the analogue of Rust's `MieError::is_file_error()` predicate; `MieRecordError` is the analogue of `is_record_error()`. The non-classified variants (`NoValidRecords`, `HomogeneousPayload`, `InputOutputCollision`, `ClobberRefused`, `UnrecoverableSyncLoss`, `WriterError`) inherit from `MieFileError` or `MieRecordError` in Python by the same rule the Rust predicate applies.

---

## 3. File-level errors

These fire before any record is decoded, or before the writer touches the destination. Catchable in Python as `MieFileError`; in Rust via `MieError::is_file_error()` for the I/O subset.

| Variant | When it fires | Exit | What to do |
|---------|---------------|------|------------|
| `MieFileNotFoundError` / `FileNotFound` | Input path does not exist (L2-RDR-005). | 1 | Check the path; verify mount / permissions. |
| `MieFileEmptyError` / `FileEmpty` | Input file exists but is zero bytes (L2-RDR-006). | 1 | Upstream recording or transfer failure; check the source. |
| (Python `OSError` / Rust `MieError::FileIo`) | Read or mmap fails (permission denied, disk error, etc.). | 1 | Inspect the underlying OS error; usually a filesystem permission or hardware issue. |
| `MieNoValidRecordsError` / `NoValidRecords` | The first 64 KB contain no valid MIE record at all (L1-EXIT-002). Typical cause: input isn't an MIE recording. | **2** | Verify the input is actually MIE; if it is, records may begin past the 64 KB scan window. |
| `MieHomogeneousPayloadError` / `HomogeneousPayload` | The first 4 candidate records are byte-identical in non-timestamp positions (L2-SYN-018). Typical cause: 0x20-padded or otherwise pathological single-byte file. | **2** | Verify the input file; almost always a wrong-file-type or corruption indicator. |
| `MieTimestampFormatMismatchError` / `TimestampFormatMismatch` | L2-DEC-015 multi-record probe completed with an L2-DEC-016 Ambiguous classification (max aggregate score < 4 OR margin < 3). **Strict mode only**: lenient mode logs a single WARN with the score breakdown and proceeds with the chosen format. Typical cause: the file genuinely isn't an MIE recording, OR the first N records score weakly enough that the probe can't pick a side. | **2** | First confirm the file is actually an MIE recording. If it is, pass `--time-format irig` or `--time-format standard` to force the choice. If a one-time decode is acceptable with the auto-picked format (IRIG on ties per L2-DEC-012), drop `--strict` to take the lenient path. |
| `MieInputOutputCollisionError` / `InputOutputCollision` | The output path resolves to the same file as the input (L2-WRT-014). | 1 | Choose a different output path; decoding in-place is unsafe under mmap. |
| `MieClobberRefusedError` / `ClobberRefused` | The output exists and `--no-clobber` / `output.no_clobber = true` is set (L2-WRT-017). | 1 | Remove the existing file or unset the flag. |

---

## 4. Record-level errors

These fire when a specific record fails decoding. Catchable in Python as `MieRecordError`; in Rust via `MieError::is_record_error()`. All variants carry the byte `offset` of the failing record (4-digit uppercase hex in formatted messages, matching DDC vendor conventions).

In **lenient mode** (default), most record errors result in the record being skipped with a `WARN` log line; the iterator continues. In **strict mode** (`decode.strict = true`), the error is raised and decoding stops.

| Variant | When it fires | Lenient | Strict |
|---------|---------------|---------|--------|
| `MieInvalidTypeWordError` / `InvalidTypeWord` | Type Word has zero or below-minimum word count (L2-SYN-002). | skip + WARN | raise |
| `MieUnknownTypeWordError` / `UnknownTypeWord` | Type Word's message type field is not in the known set (L2-SYN-001). | skip + WARN | raise |
| `MieRecordTruncatedError` / `RecordTruncated` | A non-first record's declared extent runs past EOF (L2-RDR-002, L2-RDR-003). | stop iteration cleanly | raise |
| `MieFirstRecordTruncatedError` / `FirstRecordTruncated` | The *first* record after header detection has a declared extent past EOF (L2-RDR-004). Distinct class so it doesn't get confused with mid-stream truncation. | terminate cleanly, zero records emitted | raise |
| `MiePayloadError` / `PayloadError` | Record's payload is internally inconsistent — IRIG range failure, structural invariant violation (L2-SYN-020/021/022/023), or generic extraction failure. | skip + WARN | raise |
| `MieUnknownErrorCodeError` / `UnknownErrorCode` | An errored record (Type Word bit 14 set) carries an Error Word value outside the known DDC + decoder set (L2-ERR-004). | log WARN, emit row with the unknown code | raise |
| `MieUnrecoverableSyncLossError` / `UnrecoverableSyncLoss` | After a sync loss, `recover_sync` scanned the full 64 KB window without finding a valid record (L2-SYN-011 / L1-EXIT-004). | terminal `Err` on the iterator; CLI exits **3** by default, or 0 + `.partial` with `--allow-partial` | same |

---

## 5. Writer errors

| Variant | When it fires | Exit |
|---------|---------------|------|
| `MieWriterError` / `WriterError` | An underlying `io::Error` from the CSV writer (disk full, permission denied, etc.). Preserves the source OS error message. | 1 |
| `MieWriterError` / `WriterError` (broken-pipe variant) | The stdout consumer closed before the writer flushed. Detected by `MieError::is_broken_pipe()` in Rust; raised as `BrokenPipeError` in Python. (L2-WRT-018) | **0** |

Disk-full and permission failures during the atomic temp-file write are surfaced via `WriterError` and the temp file is unlinked before the process exits (L2-WRT-015 / L2-WRT-016).

---

## 6. DDC hardware error codes (`0x01xx` family)

When the DDC card detects a bus error during recording, it sets bit 14 of the Type Word, truncates the payload, and appends a 16-bit Error Word containing one of these codes. The decoder copies that value into the CSV `ERROR_CODE` column.

| Code | Constant | Description (from DDC documentation) |
|------|----------|--------------------------------------|
| `0x011E` | `ERROR_MANCHESTER_PARITY` | Manchester encoding error, parity error, or bit-count error on the wire. |
| `0x0120` | `ERROR_NO_RESPONSE` | RT did not respond with a Status Word, or the response had too few data words. |
| `0x0136` | `ERROR_INVERTED_SYNC` | Inverted sync pattern detected on a data word (data word looks like a Cmd/Status word). |
| `0x0140` | `ERROR_TOO_MANY_WORDS` | More data words received than the Command Word specified. |
| `0x0150` | `ERROR_UNKNOWN_DDC` | Catch-all for DDC errors not in the above set (firmware variation, undocumented condition). |

Any `0x01xx` code outside this set surfaces as `MieUnknownErrorCodeError` / `MieError::UnknownErrorCode` in strict mode, and as a WARN log line in lenient mode. The CSV row still emits the unknown code so it's visible to analysts.

---

## 7. Decoder-assigned codes (`0x20xx` family)

When the DDC card emits a `SPURIOUS_DATA` record (message type `0x20`) — i.e., orphan data words without an associated Command Word — the decoder assigns one of two synthetic codes based on the record's relationship to the preceding record (L2-ERR-005 / L2-ERR-006).

| Code | Constant | Meaning |
|------|----------|---------|
| `0x2000` | `ERROR_SPURIOUS_CONTINUATION` | This SPURIOUS_DATA record immediately follows an errored record (Type Word bit 14 set on the previous decoded record). Most likely the tail of a transaction the bus cut short. |
| `0x2001` | `ERROR_SPURIOUS_STANDALONE` | Standalone SPURIOUS_DATA — the immediately preceding decoded record was not an error. Genuinely orphan fragment. |

The continuation flag resets on any decode-time corruption boundary between the error and the SPURIOUS — see L2-ERR-005's "immediately preceding *successfully decoded* record" clause.

---

## 8. Anomaly observations (WARN-only, no error class)

Two structural invariants emit `WARN` log lines but do **not** raise or skip the record. Both strict and lenient mode emit the row. These observations exist because the underlying behavior is sometimes legitimate (real-bus noise on a multi-drop bus, undocumented vendor extensions) so outright rejection would produce false negatives on real recordings.

| Invariant | When it fires | Log line |
|-----------|---------------|----------|
| **L2-SYN-024** (`STATUS_RT_MISMATCH`) | A Status Word's RT field doesn't match the Command Word's RT. Can happen on a multi-drop bus when a different RT responds to a broadcast. | `L2-SYN anomaly at 0x<offset>: Status RT = <N> does not match Cmd RT = <M> (raw Status = 0x<raw>); possible bus interference` |
| **L2-SYN-025** (`TYPE_WORD_RESERVED_BIT`) | Type Word bit 15 (reserved per `docs/MIE-FORMAT.md`) is set. May indicate undocumented vendor extension or wire corruption. | `L2-SYN anomaly at 0x<offset>: Type Word bit 15 (reserved) is set in raw 0x<raw>; possible undocumented vendor extension` |

If you see a high rate of either WARN in a recording, the recording itself is the thing to investigate — not the decoder.

---

## 9. Decision tree: "I got X, what now?"

**The CLI exited non-zero.** Look at the `decode exit class:` line in stderr first; it names the exit class. Then:

- `exit class: complete` (exit 0) — nothing failed; you may have hit `--allow-partial` and produced a `.partial`.
- `exit class: no-records` (exit 2) — verify the input file is actually MIE. If it is, check that records begin within the first 64 KB. If you see `HomogeneousPayload`, the file is a single-byte pad (e.g. zeros, 0x20-fill).
- `exit class: partial-unrecoverable` (exit 3) — the recording has unrecoverable corruption. Re-run with `--allow-partial` to capture what decoded before the loss, then triage the source.
- Otherwise (exit 1) — read the stderr error line; usually a per-record error or a file-safety preflight (input/output collision, no-clobber).

**The CSV `ERROR` column shows `ERROR`.** Look at `ERROR_CODE`:
- `011E`/`0120`/`0136`/`0140`/`0150` — a documented DDC bus error. See section 6.
- Anything else in `01xx` — undocumented DDC firmware variation; treat as bus error.

**The CSV `ERROR` column shows `SPURIOUS`.** Look at `ERROR_CODE`:
- `2000` — continuation of a preceding errored transaction. Usually paired with the row above.
- `2001` — orphan fragment. May indicate a recording-card hiccup.

**Stderr shows a `WARN` line.**
- `non-monotonic timestamp at …` — a record's timestamp went backwards relative to a prior record on the same RT/MSG (L2-RDR-017). `DELTA` is left empty. One WARN per RT/MSG per file.
- `L2-SYN anomaly at …` — see section 8. Not an error; the record was emitted.
- `L2-SYN structural invariant violation at …` — strict-mode would have rejected; lenient mode skipped the record. Check whether your recording has a corruption pattern worth investigating.
- `sync lost at …` / `sync recovered at …` — mid-file sync loss, recovered automatically. The `partial-recovered` exit-class summary counts these.

---

## 10. Trace links

Every error class and code in this catalog satisfies one or more L1/L2/L3 requirements. The mapping is preserved in `docs/TRACE-MATRIX.md`; the key sections are:

- **L1-ERR / L2-ERR-***: error-record decoding, SPURIOUS_DATA, separate vs inline output modes.
- **L1-EXIT-* / L2-CLI-011**: exit-code semantics and the `decode exit class:` summary.
- **L1-SYN / L2-SYN-***: validation invariants, header detection, sync recovery, the homogeneous-payload defense.
- **L1-OUT-002 / L2-WRT-014..017**: output destination integrity (collision detection, no-clobber, atomic write, partial preservation).
- **L1-ROB-001**: the fuzz harness verifying no error path panics on arbitrary input bytes.
