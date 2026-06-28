# MIE-Decoder — Level 2 Requirements

## Purpose

This document establishes the Level 2 (L2) SHALL-statement requirements for MIE-Decoder. L2 requirements are architectural derivations of the L1 requirements documented in `L1-REQ.md`: they specify *how* each L1 obligation is structurally satisfied, without yet prescribing implementation details (those belong to L3).

Every L2 requirement traces to exactly one L1 parent via the `**Parent**:` field. When an L2 is motivated by multiple L1 obligations, the primary parent is declared in `**Parent**:` and the supporting L1s are mentioned in prose. L3 requirements derive from these L2s.

## Conventions

L2 identifiers follow the format `L2-<CATEGORY>-<NNN>`. Each L2 declares its parent L1 explicitly. Metadata fields (Statement, Rationale, Verification Method) carry the same semantics as in `L1-REQ.md`.

L2s are organized by category. Full forward trace tables appear in `TRACE-MATRIX.md`. ID numbering is monotone within each category; gaps reflect retired identifiers and are never reused.

**Status and verification artifacts** are tracked in [`docs/TRACE-MATRIX.md`](TRACE-MATRIX.md), regenerated from test markers and parent links by `scripts/build-trace-matrix.py`. This file holds only the spec content above.

## Table of categories

| Code      | Title                                       |
|-----------|---------------------------------------------|
| `DEC`     | Binary decoding                             |
| `SYN`     | Synchronization, validation, invariants     |
| `RDR`     | Reader behavior                             |
| `MSG`     | Message semantics                           |
| `ERR`     | Error record handling                       |
| `WRT`     | CSV output and output destination integrity |
| `CFG`     | Configuration                               |
| `FLT`     | Filtering                                   |
| `CLI`     | Shared CLI capabilities                     |
| `CONF`    | Cross-implementation conformance            |

(Per-category and total requirement counts are intentionally omitted — they
drift as requirements are added. The requirement entries below, and the
auto-generated [`TRACE-MATRIX.md`](TRACE-MATRIX.md), are the source of truth.)

---

## L2-DEC: Binary decoding

#### L2-DEC-001

**Parent**: L1-DEC-001
**Statement**: A 16-bit Type Word SHALL decode `message_type` from bits 0-6, bus from bit 7, `word_count` from bits 8-13, and the errored-record flag from bit 14.
**Rationale**: The Type Word is the single bit-field that drives record framing, classification, and the error path. Its layout is fixed by the DDC MIE format and shared across both implementations.
**Verification Method**: Test (T)

#### L2-DEC-002

**Parent**: L1-DEC-002
**Statement**: A 3-word IRIG timestamp SHALL decode day-of-year, hour, minute, second, microsecond, and the freerun flag according to `docs/MIE-FORMAT.md`.
**Rationale**: The IRIG packing is the DDC-specific timestamp convention. Both implementations must extract the same six fields from the same three input words.
**Verification Method**: Test (T)

#### L2-DEC-003

**Parent**: L1-DEC-002
**Statement**: IRIG decoding SHALL decode the freerun flag from bit 15 of the upper timestamp word.
**Rationale**: The freerun flag indicates that the card's IRIG clock is not calendar-locked. Downstream validation (L2-SYN-019) relaxes the day-of-year constraint when this bit is set; misreading the bit would either reject valid free-run recordings or accept invalid calendar-locked ones.
**Verification Method**: Test (T)

#### L2-DEC-004

**Parent**: L1-DEC-003
**Statement**: A 16-bit Command Word SHALL decode RT address, T/R direction, subaddress, and data-word count, where a raw count of zero means 32 words.
**Rationale**: The 32-word special case is from the MIL-STD-1553 specification and must be honored to correctly size payloads on full-length transactions.
**Verification Method**: Test (T)

#### L2-DEC-007

**Parent**: L1-DEC-002
**Statement**: A Standard timestamp SHALL decode as a 32-bit free-running counter.
**Rationale**: Standard timestamps lack a calibrated tick rate; the decoder surfaces the raw counter and defers any time-domain interpretation. The 32-bit-counter shape distinguishes Standard from IRIG (which is 48 bits of structured fields).
**Verification Method**: Test (T)

#### L2-DEC-008

**Parent**: L1-DEC-001
**Statement**: All 16-bit words SHALL be read as little-endian values.
**Rationale**: DDC MIE files are written by x86 hardware as little-endian. Both implementations target little-endian decode regardless of host endianness.
**Verification Method**: Inspection (I), Test (T)

#### L2-DEC-009

**Parent**: L1-DEC-003
**Statement**: Payload extraction SHALL remain bounded by the Type Word's declared record extent and SHALL NOT consume bytes from a following record.
**Rationale**: A Command Word with `data_word_count = 32` declares a payload that may exceed the Type Word's declared extent on a malformed or truncated record. The decoder respects the Type Word extent as authoritative to avoid overrunning into the next record. Extraction is bounded to the record (`record_end = offset + word_count * 2`, already validated to fit the file): Rust slices the buffer (`let record_data = &self.data[..record_end]`) and Python passes `record_end` into `_extract_payload`, whose `_r16`/`_read_n` helpers return `None`/`()` on an out-of-bounds read — so an over-claim yields empty/partial data rather than reading past the record. There are **two** over-claim cases: (a) a single Command Word declaring more payload than the Type Word holds, which the L2-SYN-022 capacity invariant catches *before* extraction (it is computed from Cmd1); and (b) an RT-to-RT record whose **Cmd2** (the transmit command, which carries the data-word count) over-claims while Cmd1 stays small — the capacity invariant cannot see Cmd2, so the record-bounded reads are what let extraction complete safely (this was the regression behind the L1-ROB-001 fuzz `struct.error` in the Python reader). In case (b) the record-bounded read is the byte-level guarantee of this requirement; the over-claim itself is a Cmd1/Cmd2 `data_word_count` disagreement, so the post-extract **L2-SYN-027** invariant then rejects the record (strict errors, lenient skips). Two targeted tests cover both cases in both implementations: `payload_extraction_does_not_overrun_into_next_record` (case a — strict rejects, lenient decodes the successor intact) and `rt_to_rt_cmd2_overclaim_does_not_overrun` (case b — extraction completes without overrun, then L2-SYN-027 rejects, and the successor decodes intact at its true offset).
**Verification Method**: Test (T), Inspection (I)

#### L2-DEC-010

**Parent**: L1-DEC-001
**Statement**: Decoded records SHALL retain their source byte offset and raw Type and Command Word values where present, in the internal record representation.
**Rationale**: Offset and raw word values are needed by the reader to log record-class diagnostics and by analysts using a programmatic API. Surfacing these in CSV output is not required by L2-WRT-001 and is reserved for future debug-only output paths.
**Verification Method**: Inspection (I), Test (T)

#### L2-DEC-011

**Parent**: L1-DEC-002
**Statement**: Timestamp-format detection SHALL be file-level: the format is resolved once at the start of the decode invocation (by the bounded multi-record probe of L2-DEC-015) and used unchanged for every subsequent record in the same decode invocation. Per-record re-detection is not permitted.
**Rationale**: Mid-file re-detection would silently produce mixed time bases in one CSV, defeating the time-series semantics of `DELTA` (see L1-DLT-001). File-level resolution makes the contract simple to reason about.
**Verification Method**: Test (T)

#### L2-DEC-012

**Parent**: L1-DEC-002
**Statement**: When IRIG and Standard format detection score equally during auto-detection, IRIG SHALL be selected.
**Rationale**: Flight-test recordings overwhelmingly use IRIG; this tie-break preserves the most common path. Inverting the tie-break would silently break the dominant operational use case.
**Verification Method**: Test (T)

#### L2-DEC-013

**Parent**: L1-CFG-001
**Statement**: An explicit `--time-format` CLI flag or `decode.time_format` configuration value SHALL bypass auto-detection and force the chosen format for the entire decode. The forced format SHALL nonetheless be sanity-checked against the L2-DEC-015 detection probe before iteration begins: when the probe is **Decisive** (per L2-DEC-016) for the *other* format, the forced selection is an obviously-wrong selection and SHALL surface the timestamp-format-mismatch class (`MieError::TimestampFormatMismatch` / `MieTimestampFormatMismatchError`, shared with L2-DEC-016, exit code `2`) in strict mode, or a single WARN in lenient mode after which decoding proceeds with the forced format. A **Marginal** or **Ambiguous** probe SHALL NOT be flagged — those are exactly the cases where forcing is the legitimate operator override of a detection the heuristic cannot make confidently.
**Rationale**: Operators sometimes know the format ahead of time (e.g., from the recording campaign's documentation) and want to skip the auto-detect heuristic — but a typo such as `--time-format standard` on an IRIG recording would otherwise emit garbage timestamps for the whole file silently. Gating the check on a *Decisive* probe catches that mistake while never overriding the operator in the marginal/ambiguous cases where forcing exists precisely to correct a misdetection; lenient mode preserves the forced result with a warning so an intentional override still works.
**Verification Method**: Test (T)

#### L2-DEC-014

**Parent**: L1-OUT-001
**Statement**: IRIG timestamp text SHALL emit exactly six microsecond digits regardless of the decoded value. A microsecond value greater than or equal to 1,000,000 SHALL be considered unreachable given L2-SYN-004 validation, but if encountered on the defensive path the implementation SHALL truncate to six digits and SHALL log a WARN naming the offending record offset. The formatter SHALL NOT emit more than six microsecond digits under any circumstance.
**Rationale**: A seven-digit microsecond field would silently shift every downstream column in the vendor-compatible CSV by one character. The defensive truncate-plus-warn is a belt-and-braces guard against any path that bypasses L2-SYN-004 validation.
**Verification Method**: Test (T)

#### L2-DEC-015

**Parent**: L1-DEC-002
**Statement**: Auto-detection of timestamp format SHALL probe up to the first `N` records of the file, not only the first record, where `N` defaults to `8` and is configurable via the `decode.detect_records` configuration key (range `1..=32`) or the `--detect-records` CLI flag. The existing per-record scoring signals (T/R direction consistency with the Type Word, word-count plausibility under each candidate overhead, IRIG field range validity) SHALL be aggregated across the probe set; the format with the higher aggregate score is chosen. The chosen format SHALL be resolved before the first record is decoded and SHALL not change for the rest of the decode invocation, preserving the file-level resolution rule of L2-DEC-011. If fewer than `N` valid records are available before the file ends, the probe SHALL use what exists.
**Rationale**: Single-record scoring is defeated by a borderline first record (one that scores plausibly under both candidate overheads). A wrong choice corrupts timestamp values for the entire file without surfacing any diagnostic — the reader simply emits records with garbage timestamp fields and operators have to notice. Probing multiple records gives the scorer enough signal to disambiguate on files where the first record alone would not. Capping at `N` keeps the probe bounded, and a configurable default lets operators tune the trade-off for unusual recordings.
**Verification Method**: Test (T)

#### L2-DEC-016

**Parent**: L1-DEC-002
**Statement**: When the L2-DEC-015 probe completes with an indecisive result — specifically, when the winning aggregate score is below a low-confidence threshold (`max_score < 4` over the probe set) OR the margin between the two candidate scores is below a minimum-margin threshold (`|irig_score - std_score| < 3`) — a `MieTimestampFormatMismatch` error class SHALL be defined. In strict mode (`--strict` or `decode.strict = true`), this condition SHALL halt decoding with exit class `2` (the "wrong file type" class shared with `MieNoValidRecordsError` and `MieHomogeneousPayloadError` per `L1-EXIT-002`). In lenient mode (the default), the chosen format from L2-DEC-015 SHALL still be used (preserving backwards compatibility on borderline files that decoded acceptably before this requirement landed), but a single WARN SHALL be logged describing the indecisive outcome and naming both candidate scores so the operator can see how marginal the call was.
**Rationale**: The probe in L2-DEC-015 strengthens the common case (clear winner) without addressing the genuinely-ambiguous case (no clear winner). The strict-mode error gives operators who care about correctness a loud failure to act on (e.g., `--time-format` override or "this isn't an MIE recording"). The lenient-mode WARN preserves the current decode-and-hope behavior while making the ambiguity visible. The thresholds are intentionally conservative: they fire only when the probe genuinely could not distinguish, not when the call is decisive but the absolute score is low because of a small probe set.
**Verification Method**: Test (T)

#### L2-DEC-017

**Parent**: L1-DEC-002
**Statement**: The Standard timestamp is a free-running counter whose tick rate is card-dependent and not encoded in the file. When, and only when, a Standard tick rate is supplied out-of-band — via the `decode.standard_tick_rate_hz` configuration key or the `--standard-tick-rate-hz` CLI flag (see L2-CFG-011, L2-CLI-012) — the decoder SHALL convert a raw counter value to microseconds as `microseconds = round(raw_ticks × 1_000_000 / standard_tick_rate_hz)`, where `round` is half-away-from-zero. The supplied rate SHALL be a finite value strictly greater than zero; a non-finite or non-positive rate SHALL be rejected (L2-CFG-011, L2-CLI-012) and never silently treated as uncalibrated. When no rate is supplied, the Standard-to-microseconds conversion SHALL yield "no value" (`Timestamp::to_microseconds` returns `None` / `to_microseconds` returns `None`), preserving the historical behavior in which Standard records do not participate in `DELTA` (see L2-RDR-019). IRIG timestamps SHALL ignore the rate.
**Rationale**: Operators analyzing Standard-format recordings need inter-message timing, but the tick rate genuinely is not in the file, so the decoder cannot invent one. Making calibration explicit and opt-in keeps the default output truthful (an empty `DELTA` rather than a fabricated seconds value) while letting an operator who knows their card's counter frequency recover real timing. Pinning half-away-from-zero rounding keeps the two implementations byte-identical (Rust `f64::round` and Python `int(x + 0.5)` agree for the non-negative tick domain); banker's rounding would diverge at the half-tick boundary.
**Verification Method**: Test (T)

---

## L2-SYN: Synchronization, validation, invariants

#### L2-SYN-001

**Parent**: L1-SYN-001
**Statement**: Record validation SHALL reject unknown message types.
**Rationale**: An unknown message type indicates either a corrupt record or a format the decoder does not understand. Both cases must produce a clean rejection rather than be decoded with wrong assumptions.
**Verification Method**: Test (T)

#### L2-SYN-002

**Parent**: L1-SYN-001
**Statement**: Record validation SHALL reject word counts below the timestamp-format minimum or above 63.
**Rationale**: 63 is the architectural ceiling for the Type Word's 6-bit word-count field. The format-specific minimum guards against degenerate records too short to contain a usable timestamp.
**Verification Method**: Test (T)

#### L2-SYN-003

**Parent**: L1-SYN-001
**Statement**: Record validation SHALL reject records extending past end-of-file.
**Rationale**: A Type Word that declares more bytes than remain in the file indicates either a truncated tail (L1-DEC-005) or an in-record corruption; either way, the record cannot be decoded safely.
**Verification Method**: Test (T)

#### L2-SYN-004

**Parent**: L1-SYN-001
**Statement**: IRIG validation SHALL reject hour values >= 24, minute values >= 60, second values >= 60, day-of-year values < 1 or > 366, and microsecond values > 999,999.
**Rationale**: Calendar ranges are part of the IRIG-B specification. Out-of-range values indicate a corrupt timestamp; failing fast prevents downstream consumers from doing arithmetic on garbage time values.
**Verification Method**: Test (T)

#### L2-SYN-005

**Parent**: L1-SYN-001
**Statement**: Record validation SHALL confirm that the next `(N − 1)` record boundaries each contain a plausible Type Word, where `N` is the configured look-ahead depth per L2-SYN-026 (default `2`). The walk SHALL advance by each candidate's declared `word_count`; if fewer than 2 bytes remain at any candidate position, look-ahead SHALL terminate without rejecting the original candidate, and validation checks 1 through 5 (type, word count, fits-in-file, IRIG range, IRIG day-of-year) SHALL be authoritative for the records that were not reachable within the file. The minimum `N` is `1` (no look-ahead beyond the candidate itself); higher values catch wider classes of consecutive-same-shape corruption at small additional per-record-read cost.
**Rationale**: A single-record validation produces too many false positives during header detection and sync recovery. The two-record look-ahead (the historical default) is what made the validator usable in practice; the parameterization adds defense against two-consecutive same-shape corruption patterns that defeat the historical default. Wording is generalized in place rather than retired and re-issued so the trace matrix and the codebase keep a single canonical identifier for the look-ahead policy.
**Verification Method**: Test (T)

#### L2-SYN-006

**Parent**: L1-SYN-001
**Statement**: Header detection SHALL scan from offset zero in 2-byte, word-aligned increments.
**Rationale**: All record fields are 16-bit aligned. Byte-stepping would multiply the search space by two without finding any record the word-aligned scan misses.
**Verification Method**: Test (T)

#### L2-SYN-007

**Parent**: L1-SYN-002
**Statement**: Header detection SHALL cap its scan at 64 KB.
**Rationale**: Real MIE headers are well under this bound (typically <1 KB). Capping the scan prevents pathological inputs from forcing the decoder to read most of the file before reporting "no header found".
**Verification Method**: Test (T)

#### L2-SYN-008

**Parent**: L1-SYN-001
**Statement**: Header detection SHALL report when no valid record is found within the scan window.
**Rationale**: A failed header detection is a distinct error class from a mid-file failure; operators routinely diagnose this as "wrong file type" or "completely corrupted file".
**Verification Method**: Test (T)

#### L2-SYN-009

**Parent**: L1-SYN-001
**Statement**: Sync recovery SHALL scan forward from an invalid boundary in 2-byte, word-aligned increments.
**Rationale**: Same alignment argument as L2-SYN-006. Recovery uses the same step semantics as header detection to keep the validation path uniform.
**Verification Method**: Test (T)

#### L2-SYN-010

**Parent**: L1-SYN-002
**Statement**: Sync recovery SHALL cap its scan at 64 KB from the invalid boundary.
**Rationale**: Same scan-distance argument as L2-SYN-007. The cap is a per-recovery bound; cumulative bounding is L1-SYN-002.
**Verification Method**: Test (T)

#### L2-SYN-011

**Parent**: L1-SYN-001
**Statement**: Sync recovery SHALL report when no valid record is found within the scan window.
**Rationale**: Same diagnostic-classification argument as L2-SYN-008, applied to mid-file recovery failure. This is the trigger for the L1-EXIT-004 unrecoverable exit class.
**Verification Method**: Test (T)

#### L2-SYN-012

**Parent**: L1-LOG-001
**Statement**: Header detection SHALL log the detected header size at INFO level.
**Rationale**: Header size is operationally useful — it lets the operator confirm the file was recognized as MIE and tells them how many bytes were skipped before the first record.
**Verification Method**: Test (T)

#### L2-SYN-013

**Parent**: L1-LOG-001
**Statement**: Sync recovery SHALL log sync loss at WARNING and successful recovery at INFO. At DEBUG level, a validation failure SHALL additionally log one context hex line capped at 32 bytes.
**Rationale**: A sync loss is operationally noteworthy (the operator should be told the file is not pristine); a successful recovery is informative but does not warrant a warning of its own.
**Verification Method**: Test (T)

#### L2-SYN-014

**Parent**: L1-SYN-001
**Statement**: Header detection, continuous decoding, and sync recovery SHALL use the same full record-validation rules. The implementation SHALL expose both a compatibility boolean result and a detailed result identifying which validation check failed.
**Rationale**: Validators with subtly different semantics would inevitably drift. The boolean compatibility wrapper and additive detailed API share one implementation; header scan, per-record decode, and recovery therefore cannot disagree on validity. The detailed reason lets strict mode report the exact check without reimplementing classification in the reader.
**Verification Method**: Inspection (I)

#### L2-SYN-015

**Parent**: L1-MODE-001
**Statement**: Lenient mode SHALL skip invalid records and continue from a recovered boundary when possible.
**Rationale**: Lenient mode is the field-deployment default. Invalid records are routine; the operator wants the maximum number of valid records extracted regardless.
**Verification Method**: Test (T)

#### L2-SYN-016

**Parent**: L1-MODE-001
**Statement**: Strict mode SHALL stop and surface an error on invalid record validation.
**Rationale**: Strict mode is used in CI and triage contexts where any invalid record is significant and must be reported, not silently elided.
**Verification Method**: Test (T)

#### L2-SYN-017

**Parent**: L1-ERR-001
**Statement**: Valid error records and SPURIOUS_DATA records SHALL remain eligible record boundaries during validation and recovery.
**Rationale**: Error records and SPURIOUS_DATA are first-class records, not failure modes. They pass validation normally and serve as recovery anchor points.
**Verification Method**: Test (T)

#### L2-SYN-018

**Parent**: L1-SYN-001
**Statement**: Header detection SHALL apply additional defenses against homogeneous-payload inputs. When the first N candidate records (with N >= 4) share identical bytes in payload positions (i.e., excluding positions where the timestamp word naturally varies), the implementation SHALL reject the input with a distinct error class.
**Rationale**: A pathological file padded with a single byte value (such as 0x20-fill) parses with a plausible Type Word (`0x20 0x20` is a valid SPURIOUS_DATA Type Word) and passes the two-record look-ahead. The homogeneity check defends against this class of input where every other check would admit it.
**Verification Method**: Test (T)

#### L2-SYN-019

**Parent**: L1-SYN-001
**Statement**: When the IRIG freerun flag (bit 15 of the upper timestamp word) is set, the day-of-year range constraint of L2-SYN-004 SHALL NOT apply. Hour, minute, second, and microsecond constraints continue to apply.
**Rationale**: The card's free-running oscillator is not calendar-locked, so day-of-year carries no calendar meaning when freerun is set. Applying the day-of-year range would falsely reject valid free-run recordings.
**Verification Method**: Test (T)

#### L2-SYN-020

**Parent**: L1-SYN-001
**Statement**: Records with Type Word message type `0x02` (BC→RT) SHALL have a Command Word with `direction = Receive`. Strict mode SHALL surface a record error; lenient mode SHALL log a WARN and skip the record (advance to the next record boundary without emission).
**Rationale**: BC→RT transactions are by definition receive operations at the RT. A transmit-direction Command Word on a `0x02` record is internally inconsistent and indicates corruption. Skipping such records in lenient mode (rather than emitting them) prevents corrupt records from propagating into downstream analysis. (Also derives from L1-MODE-001.)
**Verification Method**: Test (T)

#### L2-SYN-021

**Parent**: L1-SYN-001
**Statement**: Records with Type Word message type `0x04` (RT→BC) SHALL have a Command Word with `direction = Transmit`. Strict mode SHALL surface a record error; lenient mode SHALL log a WARN and skip the record.
**Rationale**: Counterpart to L2-SYN-020 in the opposite direction. (Also derives from L1-MODE-001.)
**Verification Method**: Test (T)

#### L2-SYN-022

**Parent**: L1-SYN-001
**Statement**: Type Word `word_count` SHALL be at least `1 (TypeWord) + ts_words + 1 (CommandWord) + payload_words(format, Cmd.data_word_count)`, where `payload_words` is the per-format declared payload size (e.g., `data_word_count + 1` for `Receive` and `Transmit`, `1` for `ModeCodeNoData`). A record whose Type Word declares a smaller capacity than the Command Word's declared payload is internally inconsistent. Strict mode SHALL surface a record error; lenient mode SHALL log a WARN and skip the record.
**Rationale**: This invariant catches records where the Type Word was corrupted to declare a smaller extent than the Command Word's `data_word_count` would require — a class of corruption that would otherwise be silently truncated. (Also derives from L1-MODE-001.)
**Verification Method**: Test (T)

#### L2-SYN-023

**Parent**: L1-SYN-001
**Statement**: For RT-to-RT (`0x08`) and Broadcast RT-to-RT (`0x18`) records, the second Command Word's `direction` field SHALL be `Receive`. Strict mode SHALL surface a record error; lenient mode SHALL log a WARN and skip the record.
**Rationale**: In an RT-to-RT transaction, the first Command Word targets the transmitting RT (direction = Transmit) and the second targets the receiving RT (direction = Receive). A second Command Word with direction = Transmit is internally inconsistent. (Also derives from L1-MODE-001.)
**Verification Method**: Test (T)

#### L2-SYN-024

**Parent**: L1-ERR-001
**Statement**: When a record carries a Status Word, the implementation SHOULD verify that `Status.rt == Cmd.rt`. On mismatch, the implementation SHALL log a WARN naming the offset, both RTs, and the raw Status Word, and SHALL continue emitting the record in both strict and lenient mode.
**Rationale**: This is an anomaly-class observation (Severity::AnomalyWarn) rather than a corruption rejection because real-bus RT response interference on a multi-drop bus can produce a status word from a different RT than the command targeted; rejecting on this case would produce false negatives on real recordings.
**Verification Method**: Test (T)

#### L2-SYN-025

**Parent**: L1-SYN-001
**Statement**: Type Word bit 15 is reserved. When a record's Type Word has bit 15 set, the implementation SHALL log a WARN naming the offset and the raw Type Word, and SHALL continue emitting the record in both strict and lenient mode.
**Rationale**: `docs/MIE-FORMAT.md` lists bit 15 as "Reserved for future use. Should be 0." Treating a set bit as corruption would prevent decoding any recording that uses an undocumented vendor extension; treating it as a silent no-op would hide a real signal from the operator. The WARN-and-emit compromise gives the operator visibility without breaking decode.
**Verification Method**: Test (T)

#### L2-SYN-026

**Parent**: L1-SYN-001
**Statement**: The look-ahead depth `N` referenced by L2-SYN-005 SHALL be configurable via the `decode.lookahead_records` TOML key or the `--lookahead-records` CLI flag, with valid range `[1, 32]` and default `2` (preserving the historical two-record look-ahead behavior). Values outside the range SHALL be rejected at config-load time or CLI parse time with a clear error naming the offending value and the valid range. The configured `N` SHALL apply uniformly to every sync-validation call site: header detection (`find_first_record`), mid-iteration per-record validation, and sync-recovery scan (`recover_sync`).
**Rationale**: A small number of operators encounter recordings where two consecutive corrupt frames happen to align on plausible-looking Type Words and defeat the default two-record look-ahead. Letting them increase `N` to (say) `4` or `8` catches a wider failure class without changing behavior for the common case. The `[1, 32]` range matches the equivalent range used by L2-DEC-015's `decode.detect_records` for consistency; values above `32` add little benefit (the look-ahead walk is bounded by the file's actual record count anyway).
**Verification Method**: Test (T)

#### L2-SYN-027

**Parent**: L1-SYN-001
**Statement**: For RT-to-RT (`0x08`) and Broadcast RT-to-RT (`0x18`) records, the first and second Command Words SHALL agree on `data_word_count`. A record whose two Command Words declare different counts is internally inconsistent. Strict mode SHALL surface a record error; lenient mode SHALL log a WARN and skip the record. This is a post-extract check (the second Command Word lives inside the payload), evaluated only after the record-bounded payload extraction of L2-DEC-009 has completed.
**Rationale**: An RT-to-RT transaction carries a single data-word count for the transfer; `docs/MIE-FORMAT.md` §6.3 specifies that both Command Words encode it and they must agree. The capacity invariant (L2-SYN-022) only sees the first Command Word, so a second Command Word that declares a different (often larger) count is not caught pre-extraction — including the over-claim that L2-DEC-009's record-bounded reads defend against at the byte level. Rejecting the mismatch turns "silently emit a record with truncated data" into an explicit corruption signal, consistent with the sibling post-extract check L2-SYN-023. (Also derives from L1-MODE-001.)
**Verification Method**: Test (T)

### Invariant severity classes (applies to L2-SYN-020 through L2-SYN-025, L2-SYN-027)

- **Severity::Reject** — Strict mode aborts with a record error class (e.g., `MieError::PayloadError`). Lenient mode logs a WARN and skips the record (advances past it without emission). Applies to L2-SYN-020 through L2-SYN-023 and L2-SYN-027.
- **Severity::AnomalyWarn** — Both strict and lenient modes log a WARN and continue emitting the record. Used when the bus-protocol or vendor-spec ambiguity makes outright rejection unsafe (real-bus noise, undocumented extensions). Applies to L2-SYN-024 and L2-SYN-025.

---

## L2-RDR: Reader behavior

#### L2-RDR-002

**Parent**: L1-DEC-005
**Statement**: Lenient mode SHALL stop cleanly at a truncated final record.
**Rationale**: A truncated tail is the most common form of recording-card termination (operator stop, power loss, disk full). Lenient mode treats it as end-of-stream, emits all preceding valid records, and exits cleanly.
**Verification Method**: Test (T)

#### L2-RDR-003

**Parent**: L1-MODE-001
**Statement**: Strict mode SHALL surface a truncation error when a readable Type Word declares a record extent beyond end-of-file.
**Rationale**: Counterpart to L2-RDR-002 in strict mode. In strict contexts, the operator wants the truncated tail surfaced rather than silently treated as a clean end-of-stream.
**Verification Method**: Test (T)

#### L2-RDR-004

**Parent**: L1-MODE-001
**Statement**: Header detection followed by a first-record truncation (the first valid Type Word's declared extent runs past EOF) SHALL surface a distinct error class in strict mode (e.g., `MieError::FirstRecordTruncated`) and SHALL terminate cleanly with zero records emitted in lenient mode.
**Rationale**: This is the post-header counterpart to L2-RDR-002/003. Treating it identically to "no records found" would obscure the distinction between "no records at all" and "the header parsed but the first record is truncated"; the latter is operationally distinct and worth its own error class.
**Verification Method**: Test (T)

#### L2-RDR-005

**Parent**: L1-EXIT-001
**Statement**: Opening a missing input file SHALL surface a file-not-found error.
**Rationale**: Distinct from format errors and validation errors; usually means the operator typed the path wrong.
**Verification Method**: Test (T)

#### L2-RDR-006

**Parent**: L1-EXIT-001
**Statement**: Opening an empty input file SHALL surface an empty-file error.
**Rationale**: Distinct from "no valid records found" (which implies the file had content but none of it parsed). An empty input file is usually an upstream pipeline failure that the operator can investigate directly.
**Verification Method**: Test (T)

#### L2-RDR-007

**Parent**: L1-DEC-003
**Statement**: Receive records SHALL extract Data Words before Status Word.
**Rationale**: The on-bus ordering of a Receive transaction is Cmd → Data... → Status. The CSV preserves this ordering so the row reads as the bus saw it.
**Verification Method**: Test (T)

#### L2-RDR-008

**Parent**: L1-DEC-003
**Statement**: Transmit records SHALL extract Status Word before Data Words.
**Rationale**: The on-bus ordering of a Transmit transaction is Cmd → Status → Data.... Counterpart to L2-RDR-007.
**Verification Method**: Test (T)

#### L2-RDR-009

**Parent**: L1-DLT-001
**Statement**: `DELTA` SHALL be calculated against the most recent prior message sharing the same RT and MSG identifier.
**Rationale**: The analyst-meaningful inter-arrival time is between transactions on the same RT/subaddress pair; aggregating across different subaddresses would conflate two independent traffic patterns.
**Verification Method**: Test (T)

#### L2-RDR-010

**Parent**: L1-DLT-001
**Statement**: The first occurrence of each RT/MSG key SHALL have `DELTA` equal to `0.000000`.
**Rationale**: A first-occurrence sentinel distinguishes "first time seen" from "previously seen". `0.000000` is the chosen sentinel because it is unambiguous when read in the CSV (no prior arrival means zero elapsed time).
**Verification Method**: Test (T)

#### L2-RDR-015

**Parent**: L1-SYN-001
**Statement**: Every record SHALL pass the full shared validation path before decoding.
**Rationale**: Same uniformity argument as L2-SYN-014 from the reader's perspective. Bypassing validation for any record class would create a class-specific drift surface.
**Verification Method**: Inspection (I), Test (T)

#### L2-RDR-016

**Parent**: L1-DLT-001
**Statement**: Errored records (Type Word bit 14 set) SHALL participate in `DELTA` tracking — they update the per-RT/MSG cursor and SHALL receive a `DELTA` computed against the prior message sharing the same key.
**Rationale**: An errored record still represents a bus transaction that took bus time, even if the data is unusable. Excluding it from DELTA would falsely widen the gap to the next valid record on the same key.
**Verification Method**: Test (T)

#### L2-RDR-017

**Parent**: L1-DLT-001
**Statement**: When a record's timestamp is older than the prior message for the same RT/MSG key, `DELTA` SHALL be empty and the implementation SHALL log a WARN. The WARN SHALL be emitted at most once per RT/MSG key per decoded file to avoid log flooding.
**Rationale**: A timestamp regression on the same key is a corruption signal that the operator should see. Per-key de-duplication keeps the log usable when a recording has hundreds of regressions on one key.
**Verification Method**: Test (T)

#### L2-RDR-018

**Parent**: L1-DLT-001
**Statement**: SPURIOUS_DATA records have no RT/MSG key and SHALL have an empty `DELTA`; they SHALL NOT update any per-key cursor.
**Rationale**: SPURIOUS_DATA is by definition a fragment without a Command Word, so it has no RT or subaddress to key on. Updating any cursor with it would corrupt the key state for unrelated transactions.
**Verification Method**: Test (T)

#### L2-RDR-019

**Parent**: L1-DLT-001
**Statement**: Standard-format timestamps have no tick rate encoded in the file. When no Standard tick rate is configured, records carrying a Standard timestamp SHALL have an empty `DELTA` and SHALL NOT participate in per-key tracking. When a valid Standard tick rate is configured (per L2-DEC-017), Standard timestamps SHALL be converted to microseconds and SHALL participate in per-key `DELTA` tracking on the same terms as IRIG timestamps (L2-RDR-016 through L2-RDR-018), including the first-occurrence `0.0` rule and the non-monotonic empty-`DELTA`-with-WARN rule.
**Rationale**: A numeric DELTA computed from raw 32-bit counter ticks in unknown units would be misleading, so the truthful default per L1-DLT-001 is emptiness. Once an operator supplies the card's counter frequency out-of-band, the ticks acquire a real microsecond basis and there is no longer any reason to withhold `DELTA` — the conversion is well-defined (L2-DEC-017) and the existing tracking rules apply unchanged.
**Verification Method**: Test (T)

#### L2-RDR-020

**Parent**: L1-EXIT-006
**Statement**: Both implementations SHALL open the input file with read-only access semantics. Writable, copy-on-write, or shared-write memory-mapping modes SHALL NOT be used. The specific access mode and API is pinned by L3-PY-009 (Python) and L3-RS-003 (Rust).
**Rationale**: L1-EXIT-006 is the operational contract that the decoder never modifies the input file. Read-only mmap is the implementation-level enforcement of that contract — any other mode would create a code path through which the input could be mutated, undermining the contract regardless of operator intent.
**Verification Method**: Inspection (I)

---

## L2-MSG: Message semantics

#### L2-MSG-001

**Parent**: L1-DEC-003
**Statement**: The decoder SHALL classify all 10 supported MIL-STD-1553 transaction formats plus SPURIOUS_DATA. The supported transaction formats are: (1) BC→RT Receive, (2) RT→BC Transmit, (3) RT-to-RT, (4) Receive Broadcast (BC→RT broadcast), (5) RT-to-RT Broadcast, (6) Mode Code Transmit with data, (7) Mode Code Receive with data, (8) Mode Code with no data, (9) Mode Code Broadcast with no data, (10) Mode Code Broadcast with data. SPURIOUS_DATA is the 11th classification and represents records lacking a Command Word.
**Rationale**: Enumeration prevents accidental omissions and makes the classification space testable. Each format has a distinct payload extraction shape (L2-RDR-007/008 and the mode-code variants).
**Verification Method**: Test (T)

#### L2-MSG-002

**Parent**: L1-DEC-004
**Statement**: Bus SHALL be represented as `A` or `B` in CSV output.
**Rationale**: Single-character A/B is the DDC vendor CSV convention. Both implementations preserve it for column compatibility.
**Verification Method**: Test (T)

#### L2-MSG-003

**Parent**: L1-DEC-003
**Statement**: A decoded message SHALL expose an MSG label in `<subaddress><T|R>` form when a Command Word is present.
**Rationale**: The `<subaddress><T|R>` form is the DDC vendor CSV convention and is used as the secondary key for DELTA tracking. SPURIOUS_DATA has no Command Word and therefore has no MSG label.
**Verification Method**: Test (T)

#### L2-MSG-004

**Parent**: L1-DEC-003
**Statement**: When sub-classifying a Mode Command (L2-MSG-001 formats 6–10), the data-vs-no-data decision SHALL be made relative to the record's **timestamp word count** (IRIG = 3 words, Standard = 2 words), not against absolute word-count thresholds. Specifically, a broadcast mode code (RT 31) carries data iff `word_count ≥ timestamp_words + 3` (otherwise no-data), and a non-broadcast receive mode code carries data iff `word_count ≥ timestamp_words + 4` (otherwise no-data). A transmit mode code is classified by direction independent of word count. Classification SHALL therefore be correct under both timestamp formats.
**Rationale**: A Standard timestamp occupies one fewer word than IRIG, so every mode-code shape's total word count is one smaller under Standard. Fixed IRIG-sized thresholds misclassified Standard mode-code-with-data records (broadcast at `word_count = 5`, receive at `word_count = 6`) as no-data, emitting the data word in the Status position. Deriving the threshold from the resolved timestamp word count makes the classifier correct for both formats while leaving IRIG output byte-identical.
**Verification Method**: Test (T)

---

## L2-ERR: Error record handling

#### L2-ERR-001

**Parent**: L1-ERR-001
**Statement**: Type Word bit 14 SHALL identify an errored record.
**Rationale**: Bit 14 is the DDC card's "this record encountered a bus error" indicator. Both implementations key error-record routing off this bit.
**Verification Method**: Test (T)

#### L2-ERR-002

**Parent**: L1-ERR-001
**Statement**: The final word of an errored record SHALL be decoded as its DDC Error Word.
**Rationale**: When bit 14 is set, the card truncates the payload and appends an Error Word in the last 16-bit slot. The decoder extracts this word as the error class.
**Verification Method**: Test (T)

#### L2-ERR-003

**Parent**: L1-ERR-001
**Statement**: Known DDC Error Word values SHALL be recognized.
**Rationale**: The known set is the `0x01xx` family documented in `docs/MIE-FORMAT.md`. Unknown values are surfaced as `UNKNOWN` in the CSV in lenient mode; strict mode rejects them (L2-ERR-004).
**Verification Method**: Test (T)

#### L2-ERR-004

**Parent**: L1-MODE-001
**Statement**: Strict mode SHALL reject unknown DDC Error Word values.
**Rationale**: An unrecognized error code indicates either a corrupt record or an undocumented card behavior. In strict mode the operator wants this surfaced rather than silently passed through as `UNKNOWN`.
**Verification Method**: Test (T)

#### L2-ERR-005

**Parent**: L1-ERR-001
**Statement**: SPURIOUS_DATA records immediately following an errored record SHALL use decoder code `0x2000`. "Immediately following" refers to the immediately preceding *successfully decoded* record, not the immediately preceding error record. A classification failure or unrecoverable validation error between an error record and a SPURIOUS_DATA record SHALL reset the continuation flag — the corruption itself is treated as a boundary, and the SPURIOUS_DATA SHALL fall through to L2-ERR-006 (standalone, `0x2001`).
**Rationale**: The continuation flag is what distinguishes "leftover data from a truncated errored transaction" from "an unrelated SPURIOUS_DATA fragment". Resetting on a corruption boundary prevents stale state from misclassifying a fragment that is no longer continuous with the prior error.
**Verification Method**: Test (T)

#### L2-ERR-006

**Parent**: L1-ERR-001
**Statement**: Standalone SPURIOUS_DATA records SHALL use decoder code `0x2001`.
**Rationale**: Distinct code from `0x2000` so the analyst can tell continuation fragments from genuinely orphan ones.
**Verification Method**: Test (T)

#### L2-ERR-007

**Parent**: L1-OUT-001
**Statement**: CSV output SHALL include `ERROR` and `ERROR_CODE` columns.
**Rationale**: These columns are part of the DDC vendor CSV layout. They are populated in inline mode and empty in clean rows of the main file in separate mode.
**Verification Method**: Test (T)

#### L2-ERR-008

**Parent**: L1-ERR-001
**Statement**: Separate mode SHALL write normal messages to the main CSV and errored or spurious messages to `<stem>_errors<suffix>`, where `<stem>` is the destination filename up to and excluding the final `.`, and `<suffix>` is the final `.` and extension (or empty if the destination has no extension). Examples: `out.csv` → `out_errors.csv`; `out` → `out_errors`; `data.bar.csv` → `data.bar_errors.csv`.
**Rationale**: The stem/suffix split preserves the operator's chosen extension on the errors file. The split also handles extension-less destinations cleanly.
**Verification Method**: Test (T)

#### L2-ERR-010

**Parent**: L1-OUT-001
**Statement**: CSV `ERROR` SHALL be empty, `ERROR`, or `SPURIOUS` as appropriate; `ERROR_CODE` SHALL contain the corresponding uppercase hexadecimal code.
**Rationale**: Empty / `ERROR` / `SPURIOUS` is the DDC vendor convention; the hex code follows the same `0x` prefix policy as other 16-bit values in the CSV (see L2-WRT-003).
**Verification Method**: Test (T)

#### L2-ERR-011

**Parent**: L1-ERR-001
**Statement**: Inline mode SHALL write normal, errored, and spurious messages to one CSV.
**Rationale**: Inline mode produces a single output for byte-exact diff against the DDC vendor CSV. Separate-mode output by definition does not have a vendor-CSV counterpart.
**Verification Method**: Test (T)

---

## L2-WRT: CSV output and output destination integrity

#### L2-WRT-001

**Parent**: L1-OUT-001
**Statement**: CSV columns SHALL appear in this order: `TIME_STAMP`, `RT`, `MSG`, `WD01`-`WD32`, `STAT`, `CMD`, `MUX`, `TERM_NAME`, `BUS`, `DELTA`, `ERROR`, `ERROR_CODE`, `IM_GAP`, `RCV_GAP`, `XMT_GAP`.
**Rationale**: Column order is dictated by the DDC vendor CSV. Reordering or "cleaning up" the empty vendor columns would break byte-exact diff and the column compatibility contract.
**Verification Method**: Test (T)

#### L2-WRT-002

**Parent**: L1-OUT-001
**Statement**: Unused Data Word columns and unavailable fields SHALL be empty.
**Rationale**: Empty cells are the DDC vendor CSV convention for "no value here"; emitting `0000` would falsely indicate a zero word was on the bus.
**Verification Method**: Test (T)

#### L2-WRT-003

**Parent**: L1-OUT-001
**Statement**: Data Words, Status Word, Command Word, and Error Word SHALL use 4-character uppercase hexadecimal without a `0x` prefix.
**Rationale**: This is the DDC vendor CSV convention. Width 4 zero-pads narrow values and uppercase matches the vendor casing.
**Verification Method**: Test (T)

#### L2-WRT-004

**Parent**: L1-OUT-001
**Statement**: `DELTA` SHALL use exactly six decimal places when populated, and SHALL be an empty CSV cell when no `DELTA` is computed (see L2-RDR-016 through L2-RDR-019).
**Rationale**: Six decimal places is microsecond precision in seconds — matching the IRIG timestamp basis and the DDC vendor CSV convention. Empty cells communicate "no DELTA available" without falsifying a number.
**Verification Method**: Test (T)

#### L2-WRT-007

**Parent**: L1-OUT-001
**Statement**: CSV output SHALL support a file destination and stdout.
**Rationale**: File output is the normal case; stdout is for pipeline integration where the next stage consumes the CSV directly.
**Verification Method**: Test (T)

#### L2-WRT-011

**Parent**: L1-OUT-001
**Statement**: IRIG timestamp text SHALL use `DAY:HH:MM:SS.uuuuuu` formatting.
**Rationale**: This is the DDC vendor convention. Zero-padded fields keep column alignment under monospace rendering.
**Verification Method**: Test (T)

#### L2-WRT-012

**Parent**: L1-OUT-001
**Statement**: CSV output SHALL use LF (`\n`) line endings on every supported platform.
**Rationale**: LF-only line endings make CSV byte-exact diff work across Windows and Linux. CRLF would break the diff and confuse downstream consumers on Linux.
**Verification Method**: Test (T)

#### L2-WRT-013

**Parent**: L1-OUT-001
**Statement**: CSV output SHALL preserve the vendor compatibility columns `TERM_NAME`, `IM_GAP`, `RCV_GAP`, and `XMT_GAP` as empty, and SHALL preserve the `MUX` column in its vendor layout position. `MUX` is populated from the input file name per L2-WRT-020 (and is empty when that population is disabled or yields no value); the other four remain empty.
**Rationale**: These columns are part of the vendor layout and are preserved for column-order fidelity. `MUX` is the first of them to carry decoder-derived content (L2-WRT-020); the rest stay empty until a future version defines a meaning for them.
**Verification Method**: Test (T)

#### L2-WRT-014

**Parent**: L1-OUT-002
**Statement**: The decode output path SHALL NOT resolve to the same canonical path as the input file. Implementations SHALL surface a distinct error class (e.g., `MieError::InputOutputCollision` / `MieOutputPathError`) before opening the output. Stdout output is exempt because it has no filesystem identity.
**Rationale**: Decoding a file onto itself would truncate the input mid-decode and produce undefined behavior under mmap. Catching this before the output is opened is the only safe guard.
**Verification Method**: Test (T)

#### L2-WRT-015

**Parent**: L1-OUT-002
**Statement**: File output SHALL be written via a temporary file in the destination's directory, then renamed atomically over the destination on successful completion. The temp file SHALL live on the same filesystem as the destination so the rename is atomic.
**Rationale**: Atomicity guarantees that a downstream consumer never observes a half-written CSV. Same-filesystem placement is required because cross-filesystem rename is a copy-then-unlink and is not atomic.
**Verification Method**: Test (T)

#### L2-WRT-016

**Parent**: L1-EXIT-004
**Statement**: On a decode failure that triggers the default `partial-unrecoverable` exit class (L1-EXIT-004), the temp file SHALL be unlinked before the process exits. When `--allow-partial` is in effect, the temp file SHALL instead be renamed to `<destination>.partial` so the operator can inspect it; in that case the original `<destination>` SHALL remain untouched.
**Rationale**: Unlinking by default prevents the operator from being handed a partial result they might mistake for a complete one. `--allow-partial` is the explicit opt-in for operators doing forensics on a known-bad recording.
**Verification Method**: Test (T)

#### L2-WRT-017

**Parent**: L1-OUT-002
**Statement**: Overwrite of an existing destination SHALL succeed by default. An optional `--no-clobber` CLI flag (and equivalent `output.no_clobber` configuration key) SHALL refuse the overwrite and surface a distinct error class.
**Rationale**: Overwrite by default matches operator expectation for batch reruns. `--no-clobber` is the explicit guard for pipelines where the operator wants to fail rather than overwrite a possibly-newer result.
**Verification Method**: Test (T)

#### L2-WRT-018

**Parent**: L1-EXIT-001
**Statement**: A broken-pipe condition on stdout output (downstream consumer closed early) SHALL exit `0` with no error. Disk-full and permission errors SHALL surface as a writer error preserving the underlying OS error message.
**Rationale**: Broken pipe on stdout is the expected termination signal in shell pipelines (`mie-decoder ... | head`). Treating it as an error would falsely fail every pipeline that consumes only the first N rows. Disk-full and permission errors are genuine failures.
**Verification Method**: Test (T)

#### L2-WRT-019

**Parent**: L1-OUT-002
**Statement**: In separate (default) error mode, the main CSV and the errors CSV SHALL each be committed via its own atomic temp+rename (L2-WRT-015), and the main CSV SHALL be committed **before** the errors CSV. The two commits are sequential — no cross-file atomic rename exists — so this is explicitly **not** an all-or-nothing guarantee across the two files: a failure of the second (errors) commit SHALL leave the already-committed main CSV in place, and a failure of the first (main) commit SHALL leave neither file (the errors output is still an un-renamed temp and is unlinked). Both implementations SHALL use this main-before-errors order.
**Rationale**: There is no portable way to atomically commit two files together. Since one file may survive a mid-commit failure, the residue must be the main CSV — the primary deliverable — never an orphan errors file with no corresponding main output. Pinning the order also removes a latent cross-implementation divergence: Rust previously committed errors-first while Python committed main-first, so the file left behind on failure differed by implementation.
**Verification Method**: Test (T)

#### L2-WRT-020

**Parent**: L1-OUT-001
**Statement**: The `MUX` column SHALL be populated from a field of each record's **source file name**. The file's basename SHALL be split on a configurable `delimiter` (default `.`) and the field at a configurable 0-based `field` index (default `4`; a negative index counts from the end) SHALL be used as the MUX value, trimmed of surrounding whitespace. When the index is out of range, the selected field is empty, the delimiter is empty, or population is disabled, `MUX` SHALL be empty. Population is **enabled by default** and SHALL be disabled by `[mux] enabled = false` (TOML) or `--no-mux` (CLI), with `[mux] delimiter` / `--mux-delimiter` and `[mux] field` / `--mux-field` overriding the extraction. In multi-file merge mode each record SHALL carry the MUX value of the **file it was decoded from**. A MUX value containing the CSV delimiter, a double quote, or a line break SHALL be RFC4180-quoted identically in both implementations.
**Rationale**: Operators encode a source/recorder identity in a file-name field (e.g. `…1553.aa.unused.mie_irig`); surfacing it in the long-empty `MUX` column lets a decoded CSV identify its origin without an external lookup. Delimiter+index extraction is dependency-free (no regex), preserving the hand-rolled / single-dependency property. Default-on serves the common operator workflow; `--no-mux` restores vendor-exact output for a byte-for-byte vendor-CSV diff (see `docs/VENDOR-CSV-DIFFS.md`). Per-file carry through the merge is what makes the value meaningful when several recorders are combined, and is the first concrete step of the ROADMAP "recorder identity from a parsed file-naming convention" item.
**Verification Method**: Test (T)

---

## L2-CFG: Configuration

#### L2-CFG-001

**Parent**: L1-CFG-001
**Statement**: TOML configuration SHALL support logging level, timestamp format, strict mode, error mode, exclusion filters, and output format.
**Rationale**: These are the operator-facing knobs that vary between recording campaigns. The TOML schema is documented in `config/default.toml` and pinned by the schema reference below.
**Verification Method**: Test (T)

#### L2-CFG-003

**Parent**: L1-CFG-001
**Statement**: Configuration precedence SHALL be CLI values over configuration-file values over built-in defaults.
**Rationale**: CLI overrides are the operator's most explicit signal of intent and must always win. Built-in defaults are the bottom-of-stack fallback.
**Verification Method**: Test (T)

#### L2-CFG-004

**Parent**: L1-CLI-002
**Statement**: CLI filter arguments SHALL merge with configuration-file filters.
**Rationale**: Operators routinely have a base set of exclusions in their site config and want CLI flags to add to that set rather than replace it. Replace semantics would force the operator to re-specify the base set on every invocation.
**Verification Method**: Test (T)

#### L2-CFG-005

**Parent**: L1-CFG-001
**Statement**: The CLI SHALL accept a TOML configuration file path.
**Rationale**: The TOML file is the persistence mechanism for site-wide and campaign-wide configuration. A path argument is the only way to point at it.
**Verification Method**: Test (T)

#### L2-CFG-006

**Parent**: L1-CLI-002
**Statement**: Exclusion filters SHALL support message type, RT address, bus, and subaddress.
**Rationale**: These four axes are the discriminating fields in a 1553 transaction header (per L1-CLI-002).
**Verification Method**: Test (T)

#### L2-CFG-007

**Parent**: L1-CLI-002
**Statement**: Type filters SHALL accept documented symbolic names and hexadecimal type codes.
**Rationale**: Operators think in symbolic names (`Receive`, `Transmit`, `ModeCodeNoData`) but the underlying values are hex codes (`0x02`, `0x04`, `0x40`). Supporting both lets the operator use whichever is convenient.
**Verification Method**: Test (T)

#### L2-CFG-008

**Parent**: L1-CFG-001
**Statement**: The configuration schema and key names demonstrated by `config/default.toml` SHALL remain supported. Implementations MAY add additional keys under namespaces that do not collide with shared keys; such additional keys SHALL be ignored or warned by implementations that do not support them.
**Rationale**: Operators rely on `config/default.toml` as the schema reference. Implementations that want to add features can extend the schema in their own namespace without breaking the shared one.
**Verification Method**: Test (T)

#### L2-CFG-009

**Parent**: L1-CFG-001
**Statement**: Unknown top-level TOML keys SHALL produce a WARN at load time naming the offending `[section] key`, but SHALL NOT fail the load.
**Rationale**: Forward compatibility: an older binary opening a newer config should warn but not break. Failing the load would make config rollouts much harder to manage.
**Verification Method**: Test (T)

#### L2-CFG-010

**Parent**: L1-CFG-001
**Statement**: All schema validations (type, range, enum membership, unknown-key detection) SHALL apply at configuration load time, not at use time. A loaded `DecoderConfig` SHALL represent already-validated state; consumers SHALL NOT perform additional validation.
**Rationale**: Load-time validation produces immediate operator feedback and makes the loaded config a trustworthy value. Use-site validation drifts and inevitably creates inconsistent error messages depending on which code path first observed the bad value.
**Verification Method**: Test (T)

#### L2-CFG-011

**Parent**: L1-CFG-001
**Statement**: The configuration schema SHALL accept an optional `decode.standard_tick_rate_hz` key of numeric type (TOML float, or integer coerced to float). When present, its value SHALL be validated at load time as a finite value strictly greater than zero; a non-finite or non-positive value SHALL be rejected with an error naming the key. When absent, the loaded configuration SHALL leave the rate unset (no calibration), preserving the L2-RDR-019 default. The validated value feeds the Standard tick calibration of L2-DEC-017.
**Rationale**: The tick rate is the one piece of timing information the file cannot supply, so it must come from configuration. Validating it at load time (per L2-CFG-010) keeps a bad rate from silently producing garbage microseconds far from the config that introduced it. Accepting an integer as well as a float lets operators write the natural `1000000` instead of being forced to `1000000.0`.
**Verification Method**: Test (T)

### L2-CFG schema reference

The table below pins the accepted TOML keys, their types, valid ranges, and unknown-value handling. This schema is normative for `L2-CFG-001`, `L2-CFG-008`, `L2-CFG-009`, `L2-CFG-010`, and `L2-CFG-011`.

| Key | Type | Range / Enum | Unknown-value handling |
|-----|------|--------------|------------------------|
| `logging.level` | string | one of `DEBUG`/`INFO`/`WARNING`/`WARN`/`ERROR`/`CRITICAL`/`OFF` (case-insensitive); `CRITICAL`/`OFF` silence all output | reject at load time |
| `decode.time_format` | string | one of `auto`/`irig`/`standard` | reject at load time |
| `decode.strict` | bool | TOML boolean only (not coerced from strings) | reject non-bool |
| `decode.error_mode` | string | one of `separate`/`inline` | reject at load time |
| `decode.allow_partial` | bool | TOML boolean only (see L1-EXIT-004) | reject non-bool |
| `decode.detect_records` | int | `[1, 32]` (see L2-DEC-015); default `8` | reject out-of-range at load time |
| `decode.lookahead_records` | int | `[1, 32]` (see L2-SYN-026); default `2` | reject out-of-range at load time |
| `decode.standard_tick_rate_hz` | float (int coerced) | finite and `> 0` (see L2-DEC-017); unset = no calibration | reject non-finite/non-positive at load time |
| `output.format` | string | `csv` is the only valid value in v1 | reject at load time |
| `output.no_clobber` | bool | TOML boolean only (see L2-WRT-017) | reject non-bool |
| `filter.exclude_types` | array of string\|int | per-element validated against `L2-CFG-007` | reject at load time |
| `filter.exclude_rts` | array of int | each in `[0, 31]` (1553 RT range) | reject out-of-range at load time |
| `filter.exclude_buses` | array of string | each in `{A, B}` | reject at load time |
| `filter.exclude_subaddresses` | array of int | each in `[0, 31]` (1553 subaddress range) | reject out-of-range at load time |
| Any unknown `[section] key` | — | — | WARN at load time per L2-CFG-009 |

---

## L2-FLT: Filtering

#### L2-FLT-001

**Parent**: L1-CLI-002
**Statement**: Filtering SHALL omit messages matching configured exclusion criteria and yield all other messages unchanged.
**Rationale**: Filtering operates on the post-decode message stream; it does not alter validation or decode semantics. Omission is the only effect.
**Verification Method**: Test (T)

#### L2-FLT-002

**Parent**: L1-CLI-002
**Statement**: Exclusion criteria SHALL use OR logic across configured type, RT, bus, and subaddress filters.
**Rationale**: OR is the most useful default — operators usually want to exclude messages matching *any* of the configured criteria. AND would require the operator to specify the full Cartesian product per excluded message.
**Verification Method**: Test (T)

---

## L2-CLI: Shared CLI capabilities

#### L2-CLI-001

**Parent**: L1-CLI-001
**Statement**: Decode capability SHALL accept one input path.
**Rationale**: A decode invocation operates on one file at a time; multi-file decode is delegated to the operator's shell loop or pipeline scheduler.
**Verification Method**: Test (T)

#### L2-CLI-002

**Parent**: L1-CLI-001
**Statement**: Decode capability SHALL accept an optional output path.
**Rationale**: When absent, the implementation writes to stdout (per L2-WRT-007).
**Verification Method**: Test (T)

#### L2-CLI-004

**Parent**: L1-LOG-001
**Statement**: The CLI SHALL accept a configurable logging level.
**Rationale**: Operators want to change the logging level per-invocation without editing a config file. CLI argument is the natural mechanism.
**Verification Method**: Test (T)

#### L2-CLI-005

**Parent**: L1-EXIT-001
**Statement**: Successful commands SHALL return exit code zero; usage or runtime failures SHALL return non-zero.
**Rationale**: Foundational exit-code contract. The specific non-zero codes are pinned by L1-EXIT-002 through L1-EXIT-008 and the L2-CLI-011 table.
**Verification Method**: Test (T)

#### L2-CLI-006

**Parent**: L1-LOG-001
**Statement**: Human-readable diagnostics SHALL be written to stderr rather than mixed into CSV stdout.
**Rationale**: L1-LOG-001 obligates the decoder to provide configurable diagnostic logging; this requirement pins the destination stream. Mixing diagnostics into stdout would corrupt the CSV output and break downstream consumers parsing it.
**Verification Method**: Test (T)

#### L2-CLI-008

**Parent**: L1-CLI-001
**Statement**: The CLI SHALL provide message-counting capability without requiring CSV output.
**Rationale**: Operators often want a record count to sanity-check a file size or compare two recordings. Producing CSV just to count rows is wasteful.
**Verification Method**: Test (T)

#### L2-CLI-009

**Parent**: L1-CLI-001
**Statement**: The CLI SHALL provide raw and record-aware diagnostic dump capability.
**Rationale**: When investigating a corrupt or unusual file, operators want to see the raw bytes (for offset-targeted hex examination) and the record-aware decoded view (for "what did the decoder think this record was"). Both modes are diagnostic.
**Verification Method**: Test (T)

#### L2-CLI-010

**Parent**: L1-CLI-001
**Statement**: The CLI SHALL accept timestamp-format selection, TOML configuration, and shared exclusion filters.
**Rationale**: These are the per-invocation knobs operators use during analysis. Each must be available via a CLI flag even when a config file is in use.
**Verification Method**: Test (T)

#### L2-CLI-011

**Parent**: L1-EXIT-001
**Statement**: Exit codes SHALL follow L1-EXIT-002 through L1-EXIT-009 and SHALL be identical across both implementations for the same condition:

| Code | Class | Condition |
|------|-------|-----------|
| `0` | success | complete decode, recovered decode, or `--allow-partial` partial |
| `1` | runtime/decode error | input I/O (incl. file-not-found), writer failure, strict-mode record or structural-invariant failure |
| `2` | no valid records | input is not an MIE recording (wrong file type, homogeneous-payload, ambiguous timestamp format in strict mode) |
| `3` | unrecoverable sync loss | mid-file sync loss without `--allow-partial` |
| `4` | CLI usage error | unknown/missing/invalid flag or argument, invalid flag value, no subcommand, combined input methods, or more than `MAX_MERGE_FILES` inputs |
| `5` | configuration error | config file not found, malformed TOML, or invalid config value |
| `6` | merge-incompatible inputs | multi-file merge where an input is Standard-format, leads with a freerun IRIG record, or the set mixes timestamp formats (L1-EXIT-009) |

The `count` and `dump` commands inherit `0`, `1`, `2`, `4`, and `5` but SHALL NOT produce exit `3` (they do not write a streaming output that could be partial) or `6` (they do not merge).
**Rationale**: The exit-code taxonomy is the single most operationally useful piece of CLI behavior, so each failure class an operator can act on differently gets its own code: a bad command line (`4`), a bad config file (`5`), a bad input (`2`), and a corruption event (`3`) are distinct situations with distinct fixes. Usage errors use `4` rather than the argparse / Unix default `2` because `2` is the no-valid-records class — overloading it would conflate "you typed a bad flag" with "you pointed me at the wrong file". The count/dump exemption from `3` keeps `3` specifically about a partial output that did not complete.
**Verification Method**: Test (T)

#### L2-CLI-012

**Parent**: L1-CFG-001
**Statement**: The `decode` command SHALL accept a `--standard-tick-rate-hz <HZ>` flag (both space-separated and `=`-joined forms) that overrides `decode.standard_tick_rate_hz` per the standard precedence (CLI over config over default). The supplied value SHALL be validated at parse time as a finite value strictly greater than zero, mirroring the load-time validation of L2-CFG-011; an invalid value SHALL be rejected before decoding begins with a diagnostic naming the flag, following each implementation's existing convention for rejecting a bad flag value (the same path as `--detect-records` / `--lookahead-records`). The validated value enables the Standard tick calibration of L2-DEC-017.
**Rationale**: A per-invocation flag lets an operator calibrate one recording without editing a config file, and matches how the other decode-tuning knobs are exposed. Parse-time validation gives immediate feedback consistent with the config path so the two entry points reject the same inputs.
**Verification Method**: Test (T)

#### L2-CLI-013

**Parent**: L1-CLI-001
**Statement**: The record-aware dump SHALL emit each scan-stop anomaly it encounters — invalid Type Word `word_count`, a record whose declared extent runs past EOF (truncated record), and (where the host integer type can overflow) record-offset overflow — through the logger at `WARN`, in addition to the inline `!! …` note written into the hex report. The log message SHALL name the byte offset. Emission is subject to the configured global log level (default `WARN`); the inline report note is unchanged.
**Rationale**: The record-aware dump previously surfaced these anomalies only inside the report stream, so an operator piping the dump report elsewhere — or any caller that captures the report separately — could not see the diagnostics on the normal stderr log channel the way the reader's diagnostics appear. Routing them through the logger as well makes the dump's diagnostics consistent with the reader's and visible at the configured level, while the inline note is retained for the at-a-glance visual report. (The reader's logger writes to process stderr in Rust and through the `mie_decoder` logger in Python; the dump uses the same channels.)
**Verification Method**: Test (T), Inspection (I)

---

## L2-MRG: Multi-file time-sorted merge

#### L2-MRG-001

**Parent**: L1-MRG-001
**Statement**: The `decode` command SHALL accept the input set via exactly one of three mutually exclusive methods: one or more positional paths, a `--manifest <file>` (one path per line; blank lines and `#`-prefixed comment lines ignored), or a `--glob <pattern>`. Supplying more than one method, or resolving to more than `MAX_MERGE_FILES` inputs, SHALL be a usage error (exit `4`). Resolving to a single input SHALL invoke the existing single-file path unchanged; resolving to two or more SHALL invoke the merge. The `--glob` pattern SHALL be a single-directory pattern supporting `*` and `?` wildcards over the filename only (no recursive `**`, no brace expansion), and both implementations SHALL expand it identically and in a deterministic (lexicographic) order.
**Rationale**: Positionals serve ad-hoc use, a manifest serves large/scripted sets, and a tool-expanded glob serves directories on shells (Windows) that do not expand globs. Mutual exclusivity avoids ambiguous union/ordering semantics. A fixed file-count cap keeps open mappings/descriptors within OS limits. Constraining the glob to a small, identical syntax lets the Rust crate stay dependency-free while keeping cross-implementation behavior byte-identical.
**Verification Method**: Test (T)

#### L2-MRG-002

**Parent**: L1-MRG-001
**Statement**: The merge SHALL be a streaming k-way merge driven by a min-heap holding at most one decoded record per open input. The ordering key SHALL be the tuple `(IRIG total microseconds, input index in resolved order, within-file sequence number)`, giving a total, deterministic order including for equal timestamps. Resident memory SHALL be O(number of inputs) and independent of the total record count.
**Rationale**: Each recording is already chronological within itself, so a heap-merge of k sorted streams yields global order without buffering. The index/sequence tiebreak makes output reproducible regardless of heap internals. The O(k) bound preserves the constant-record-memory guarantee.
**Verification Method**: Test (T)

#### L2-MRG-003

**Parent**: L1-MRG-002
**Statement**: Before emitting any output, the merge SHALL confirm every input resolves to the IRIG timestamp format and that each input's leading valid record is calendar-locked (not freerun). If any input is Standard-format, leads with a freerun record, or the set mixes formats, the merge SHALL reject the whole invocation with exit code `6` (L1-EXIT-009), naming the offending file and its detected format, and SHALL NOT create an output file. A freerun IRIG record encountered after the leading record SHALL emit a WARN and still be ordered by its key.
**Rationale**: Absolute-time ordering requires a shared calendar-anchored clock; Standard counters and freerun IRIG do not provide one. Validating each file's leading record is an O(1)-per-file guard that catches the common case before any work; the mid-stream freerun WARN surfaces the rarer in-file transition without aborting a partially-written merge.
**Verification Method**: Test (T)

#### L2-MRG-004

**Parent**: L1-MRG-001
**Statement**: Per-file failure during a merge SHALL follow the same strict/lenient/`--allow-partial` policy as a single-file decode, applied across the batch: strict mode SHALL surface the first record or structural-invariant failure in any file; lenient mode SHALL skip invalid records; an unrecoverable failure in any file (unrecoverable sync loss, or an unreadable/empty/non-MIE input) SHALL fail the batch unless `--allow-partial`, in which case that file SHALL be truncated at its failure point with a WARN naming it, the merge SHALL complete from the remaining inputs, and the combined output SHALL be written with the `.partial` suffix (and `<stem>_errors.partial` in separate mode), exit `0`.
**Rationale**: Reusing the established within-file semantics means operators learn one failure model. Because the output is a single time-sorted stream, an incomplete batch yields one `.partial` artifact containing everything decoded, consistent with how a single-file partial is surfaced.
**Verification Method**: Test (T)

#### L2-MRG-005

**Parent**: L1-DLT-001
**Statement**: In merge mode, DELTA SHALL be computed on the merged (globally time-ordered) stream rather than per input file, so that each RT/SA/direction inter-arrival gap reflects the unified timeline across all inputs. The merged stream is monotonic per key by construction, so DELTA SHALL be non-negative.
**Rationale**: The analyst intent for a merged timeline is the gap between consecutive same-key transactions across the whole session, not a gap that resets at file boundaries. A future release that adds per-recorder file-naming context may compute per-file DELTA for identified recorders; until then the global timeline is the well-defined default.
**Verification Method**: Test (T)

#### L2-MRG-006

**Parent**: L1-MRG-001
**Statement**: The merge SHALL verify that each input is internally time-sorted: when a record's absolute IRIG microsecond key is strictly less than that of the previous record pulled from the **same** input (capture order), the merge SHALL detect the backward step in O(1) per record. In lenient mode it SHALL emit a WARN naming the offending input, at most once per input, and SHALL still emit every record in heap order (it SHALL NOT re-sort). In strict mode it SHALL surface a record error (the `NonMonotonicInput` / `MieNonMonotonicInputError` class, CLI exit `1`), consistent with the strict/lenient policy of L2-MRG-004. Equal keys (ties) SHALL NOT be treated as a backward step.
**Rationale**: The k-way merge (L2-MRG-002) is only correct if each input is itself chronological; a file made non-monotonic by sync-loss recovery or a day/year rollover otherwise produces silently out-of-order merged rows. Detection mirrors the existing within-file non-monotonic-DELTA advisory (L2-RDR-017): WARN, never re-sort (re-sorting would defeat the streaming O(k) memory guarantee). Strict mode escalates to a failure because a backward step inside one recorder's own file is the same class of data-integrity anomaly that strict mode already rejects at the record level. The one-time-per-input WARN cadence avoids log flooding on a badly corrupted input.
**Verification Method**: Test (T)

#### L2-MRG-007

**Parent**: L1-MRG-003
**Statement**: When cross-recorder duplicate collapsing is enabled (the `merge.collapse_duplicates` config key / `--collapse-duplicates` flag, off by default), the merge SHALL suppress a record whose wire content matches a recently-emitted record from a **different** input within `merge.collapse_window_us` microseconds (`--collapse-window-us`, default `0` = exact-microsecond match). "Wire content" SHALL be the decoded Type Word, Command Word(s), Status Word(s), Error Word, and data words — excluding the timestamp, file offset, MUX, and DELTA. The first record of a duplicate set in heap order SHALL survive. Identical content from the **same** input SHALL NOT be collapsed, and a single-input decode SHALL be unaffected. De-duplication SHALL run **before** the global-DELTA stage (L2-MRG-005) so DELTA is computed over the surviving stream, and SHALL retain only the survivors within the time window (resident memory bounded by the window, preserving the L2-MRG-002 streaming guarantee). The window comparison SHALL use the **absolute** time distance between two records, so a lenient non-monotonic input (L2-MRG-006) whose merged stream steps backward SHALL neither fault (no integer underflow) nor collapse records that lie outside `merge.collapse_window_us`; on such known-bad ordering collapsing is best-effort.
**Rationale**: Recorders on a shared bus see the same transactions; collapsing the cross-recorder copies of one event restores an accurate count. A content key over the wire fields (not the timestamp) recognises the same transaction even when recorder clocks differ slightly; the window absorbs that skew, with an exact-match default that can never over-collapse genuinely distinct traffic. Requiring a *different* input distinguishes "one event seen twice" from a single recorder's own repeated periodic traffic. Running before DELTA keeps inter-arrival gaps measured across the deduped timeline; the bounded window keeps the merge streaming. The window distance is absolute (not a one-sided subtraction) because a lenient non-monotonic input can emit a record whose timestamp is *earlier* than a buffered survivor; a one-sided gap would underflow (a debug-build panic in Rust) and could match a record far outside the window (an over-collapse in Python) — both regressions are pinned by test.
**Verification Method**: Test (T)

---

## L2-CONF: Cross-implementation conformance

#### L2-CONF-001

**Parent**: L1-CONF-001
**Statement**: Shared conformance inputs SHALL be stored as reviewable hexadecimal text rather than committed `.mie` binary recordings.
**Rationale**: Hex text is reviewable in PR diffs; committed binaries are opaque and grow the repository unnecessarily. The conformance runner converts hex to binary at execution time.
**Verification Method**: Inspection (I)

#### L2-CONF-002

**Parent**: L1-CONF-001
**Statement**: The conformance runner SHALL invoke both maintained CLIs and require byte-identical CSV output.
**Rationale**: Byte-identical output is the only contract that prevents silent drift between implementations. "Almost identical" allows trailing whitespace or rounding differences that compound over time.
**Verification Method**: Test (T)

#### L2-CONF-003

**Parent**: L1-CONF-001
**Statement**: Each implementation's output SHALL match the checked-in CSV oracle.
**Rationale**: The oracle is the third party in the diff — it ensures both implementations agree with a frozen expected output, not just with each other.
**Verification Method**: Test (T)

#### L2-CONF-004

**Parent**: L1-CONF-001
**Statement**: Expected CSV oracles SHALL be updated only after both implementations agree.
**Rationale**: Updating the oracle to match one implementation while the other still differs would silently de-couple them. Both must agree before the oracle moves.
**Verification Method**: Inspection (I)

#### L2-CONF-005

**Parent**: L1-CONF-001
**Statement**: CI SHALL run the conformance suite on every push and pull request.
**Rationale**: The whole point of having a conformance suite is to catch drift before merge. Running it post-merge would let drift land in `main`.
**Verification Method**: Inspection (I)

#### L2-CONF-006

**Parent**: L1-CONF-001
**Statement**: Each maintained implementation SHALL expose a documented public library API for programmatic (non-CLI) use, with its primary decode entry point importable from the package/crate root.
**Rationale**: Both implementations are maintained as embeddable libraries, not only as CLIs; downstream code SHALL be able to depend on either implementation's decode entry point from the root without reaching into internal modules. A typed, root-level public surface kept intentional — rather than incidental to module layout — is what keeps the two implementations interchangeable for embedders, the same way the conformance suite (L2-CONF-002..005) keeps their CSV output interchangeable. The per-implementation realizations are pinned by L3-PY-007 (Python) and L3-RS-013 (Rust).
**Verification Method**: Test (T)
