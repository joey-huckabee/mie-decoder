# MIE-Decoder — MIE Binary Format Reference

Comprehensive reference for the DDC MIE binary recording format and the CSV output MIE-Decoder produces from it. Read this when:

- You're reverse-engineering an MIE file by eye (`xxd` / `hexdump` / `dump --raw`).
- You're adding decoder support for a new message format or bit field.
- You need to know exactly how a specific binary record maps to its CSV row.
- You're validating CSV output cell-by-cell against the binary input.

This doc is the deep-reference companion to [`USER-GUIDE.md`](USER-GUIDE.md) (operator workflows) and [`VENDOR-CSV-DIFFS.md`](VENDOR-CSV-DIFFS.md) (alignment with DDC vendor CSV). For per-CLI-flag and per-TOML-key references, see [`CONFIG-REFERENCE.md`](CONFIG-REFERENCE.md). For every error class and exit code, see [`ERROR-CATALOG.md`](ERROR-CATALOG.md).

This doc is the single source of truth for the on-disk MIE binary record layout, CSV column definitions, and error-code tables.

---

## 1. Top-down summary

An MIE file is a sequence of variable-length **records**, optionally preceded by a proprietary **file header** (DDC card identification, recording metadata). Every record begins with a 16-bit **Type Word** that declares the record's total length and message classification, followed by a 48-bit (IRIG) or 32-bit (Standard) **timestamp**, then the message-specific payload (Command Word, optional second Command Word, Status Words, data words, or — for error records — a final Error Word).

All multi-byte fields are **little-endian** per L2-DEC-008. The decoder is byte-stream-oriented: nothing in the format requires random access. A reader can walk record-by-record from the file header onward.

---

## 2. File-level framing

### 2.1 The proprietary header

DDC recording software prepends a header to each MIE file. The header is opaque to the decoder — it doesn't match any documented MIE record format. MIE-Decoder skips it by scanning forward in 2-byte (word-aligned) increments from offset 0 until it finds a structurally-valid Type Word that passes the full validation path (L2-SYN-006).

The scan is bounded:

| Limit | Value | Pinned by |
|-------|-------|-----------|
| Maximum header skip distance | 64 KB (`MAX_SCAN_BYTES`) | L2-SYN-007 |
| Step | 2 bytes (word-aligned) | L2-SYN-006 |

If no valid record is found within the first 64 KB, the decoder surfaces one of two distinct error classes:

- `MieError::NoValidRecords` / `MieNoValidRecordsError` — no Type Word that looks valid was found at all. Typical: wrong file type.
- `MieError::FirstRecordTruncated` / `MieFirstRecordTruncatedError` — a structurally-valid Type Word was found, but its declared extent runs past EOF. Typical: recording aborted mid-write.

A defense against pathological inputs (`L2-SYN-018`): after a candidate is accepted, the decoder compares the first 4 consecutive candidate-sized chunks for byte identity in non-timestamp positions. If they match, the file is rejected as `HomogeneousPayload` (e.g., a 0x20-padded file that happens to parse as a stream of `SPURIOUS_DATA` records).

### 2.2 The record stream

After the header, records are contiguous. Each record's Type Word declares its total length in 16-bit words; the next record begins immediately after. The decoder maintains a single offset cursor and advances by `word_count × 2` bytes per record.

If a record fails validation mid-file, the decoder enters **sync recovery**: scanning forward in 2-byte increments from the failing offset, applying the same full validation path. Recovery is bounded:

| Limit | Value | Pinned by |
|-------|-------|-----------|
| Per-recovery scan distance | 64 KB (`MAX_SCAN_BYTES`) | L2-SYN-010, L1-SYN-002 |
| Cumulative recovery scan distance | File size (recovery scans don't re-traverse) | L1-SYN-002 |

In lenient mode (the default), sync recovery is invisible in the output — the recovered records flow through normally. In strict mode (`decode.strict = true`), the first failed validation raises and stops decoding.

### 2.3 Record validation

Every record (header detection, normal forward decode, and post-recovery) passes through the **same** `validate_record` path (L2-SYN-014). The five checks, in order:

1. **Message type recognized** (one of `0x01`, `0x02`, `0x04`, `0x08`, `0x10`, `0x18`, `0x20`) — L2-SYN-001.
2. **Word count plausible** — at least `1 (TypeWord) + ts_words + 1 (CmdWord)`, at most 63 — L2-SYN-002.
3. **Record fits in file** — `offset + word_count × 2 ≤ file_len` — L2-SYN-003.
4. **IRIG timestamp fields in range** (when timestamp format is IRIG) — hour < 24, minute < 60, second < 60, day-of-year in [1, 366] (except when the freerun bit is set, per L2-SYN-019), microsecond < 1,000,000 — L2-SYN-004.
5. **Look-ahead confirmation** — the following Type Word (at `offset + word_count × 2`, when ≥ 2 bytes remain) must be a **plausible Type Word** — a known message type and a word count within the valid range — per L2-SYN-005. (Look-ahead applies only this lightweight Type-Word plausibility check, not the full checks 1–4; those remain authoritative for the candidate record itself and for any record that is actually decoded.) The look-ahead depth is configurable (default 2, range `[1, 32]` via `decode.lookahead_records` / `--lookahead-records`, L2-SYN-026); a candidate is confirmed only if the next `N-1` records also validate.

The look-ahead is what makes the validator usable. Single-record validation produces too many false positives on plausible-looking junk bytes; confirming the following record(s) drives the false-positive rate to near-zero on real inputs.

---

## 3. Record structure

Every record has the same three-section shape:

```
┌─────────────────┬─────────────────────────┬───────────────────────────┐
│ Type Word (1w)  │ Timestamp (2 or 3 w)    │ Message payload (variable)│
└─────────────────┴─────────────────────────┴───────────────────────────┘
```

- **Type Word** — 1 word (2 bytes). Always present. Bit fields below.
- **Timestamp** — 2 words (Standard, 4 bytes) or 3 words (IRIG, 6 bytes). Format is file-level (L2-DEC-011) — the decoder auto-detects across the first records (the L2-DEC-015 probe) and uses the chosen format for every subsequent record.
- **Message payload** — variable length, format depends on Type Word's message type. Detailed in §6 below.

The Type Word's `word_count` field gives the **total** record length including the Type Word and timestamp. To compute payload length: `payload_words = word_count - 1 - ts_words`.

---

## 4. Type Word (16-bit, little-endian)

The first word of every record. Drives record classification, framing, and the error path.

| Bits | Width | Field | Description |
|------|-------|-------|-------------|
| 0–6 | 7 | Message Type | DDC message type code. Known values: `0x01`, `0x02`, `0x04`, `0x08`, `0x10`, `0x18`, `0x20`. Determines the wire-order layout of command word, status word, and data words within the record payload. |
| 7 | 1 | Bus ID | Identifies which MIL-STD-1553 redundant bus this message was captured on. `0` = Bus A, `1` = Bus B. MIL-STD-1553 defines two electrically independent buses for fault tolerance; both carry the same logical traffic. |
| 8–13 | 6 | Word Count | Total record size in 16-bit words, including the Type Word itself, the timestamp, the command word, the status word, and all data words. Multiply by 2 to get bytes. Minimum 5 (Type Word + Standard timestamp + Command Word); maximum 63. |
| 14 | 1 | Error Flag | Set to 1 if the recording card detected an error in this message. When set, the payload is truncated and the final 16-bit slot of the record contains the **Error Word** (the DDC hardware error code). |
| 15 | 1 | Reserved | Per spec, should be 0. MIE-Decoder treats a set bit as an L2-SYN anomaly (WARN; continue) rather than an error, because the bit may be used by undocumented vendor extensions. |

### 4.1 Known message types

| Code | Symbolic name | Format |
|------|---------------|--------|
| `0x01` | `MODE_COMMAND` | Mode code transactions (see §6.6–6.10 for the five sub-shapes) |
| `0x02` | `BC_TO_RT` | BC→RT Receive |
| `0x04` | `RT_TO_BC` | RT→BC Transmit |
| `0x08` | `RT_TO_RT` | Terminal-to-terminal |
| `0x10` | `BROADCAST_BC_TO_RT` | Broadcast BC→RT |
| `0x18` | `BROADCAST_RT_TO_RT` | Broadcast RT-to-RT |
| `0x20` | `SPURIOUS_DATA` | Orphan data fragment with no Command Word |

The full enumeration of 11 supported transaction shapes (10 message formats plus SPURIOUS_DATA) is in §6.

---

## 5. Timestamps

The MIE format supports two timestamp encodings. Which one a file uses is set at recording time and is the same for every record in the file (L2-DEC-011). The decoder auto-detects across the first records (L2-DEC-015); the `--time-format` CLI flag or `decode.time_format` config key can force a specific format.

### 5.1 IRIG (48-bit, 3 words)

Provides absolute wall-clock time anchored to an external IRIG-B time source. When the time source is available, day-of-year, hour, minute, second, and microsecond decode to real calendar values.

**Upper Word:**

| Bits | Width | Field | Description |
|------|-------|-------|-------------|
| 15 | 1 | Freerun | Set to 1 when the external IRIG time source is unavailable and the card is using its internal free-running oscillator. When freerun is active, day/hour values may not reflect real-world calendar time, but relative timing between messages remains valid. |
| 14 | 1 | Reserved | Reserved for future use. |
| 13–5 | 9 | Day of Year | Day of year (1–366). Validation rejects values outside `[1, 366]` per L2-SYN-004, except when the Freerun flag (bit 15) is set per L2-SYN-019 — a free-running oscillator is not calendar-locked so the day field may carry any value. **NOTE:** empirical testing has shown a discrepancy between the binary-decoded value and vendor CSV output for this field on some DDC card models. The bit extraction is correct per the DDC specification, but the card firmware may use a different encoding (possibly BCD or a different field width) — that investigation is tracked in `docs/ROADMAP.md`. |
| 4–0 | 5 | Hour | Hour of day (0–23). |

**Middle Word:**

| Bits | Width | Field | Description |
|------|-------|-------|-------------|
| 15–10 | 6 | Minutes | Minute of hour (0–59). |
| 9–4 | 6 | Seconds | Second of minute (0–59). |
| 3–0 | 4 | Microseconds [19:16] | Upper 4 bits of the 20-bit microsecond counter. Combined with the Lower Word to form a value from 0 to 999,999. Validation rejects reconstructed microsecond values greater than 999,999 per L2-SYN-004; the formatter `L2-DEC-014` guarantees exactly six microsecond digits in CSV output regardless. |

**Lower Word:**

| Bits | Width | Field | Description |
|------|-------|-------|-------------|
| 15–0 | 16 | Microseconds [15:0] | Lower 16 bits of the 20-bit microsecond counter. Full microsecond value = `(Middle[3:0] << 16) | Lower[15:0]`. |

### 5.2 Standard (32-bit, 2 words)

A 32-bit free-running counter. Tick rate is card-dependent and **not** encoded in the file — without external calibration, raw ticks cannot be truthfully converted to seconds.

| Bits | Width | Field | Description |
|------|-------|-------|-------------|
| 31–16 | 16 | Counter[31:16] | Upper 16 bits of the counter, in the first word. |
| 15–0 | 16 | Counter[15:0] | Lower 16 bits, in the second word. |

Standard timestamps render in CSV as `0x` + 8 uppercase hex characters (e.g., `0x000186A0`). By default DELTA is empty for Standard records (L2-RDR-019) because there's no calibrated tick rate.

**Tick calibration (L2-DEC-017).** If you know the card's counter frequency, supply it via the `decode.standard_tick_rate_hz` config key or the `--standard-tick-rate-hz` CLI flag. The decoder then converts each counter value to microseconds as `round(raw_ticks × 1_000_000 / standard_tick_rate_hz)` (half-away-from-zero) and Standard records participate in DELTA exactly like IRIG records. The rate must be finite and `> 0`; without it, behavior is unchanged (empty DELTA). See `docs/CONFIG-REFERENCE.md` for details.

### 5.3 Auto-detection (L2-DEC-011, L2-DEC-012, L2-DEC-015, L2-DEC-016)

When `time_format = "auto"` (the default), the decoder probes the Command Word position at both candidate offsets (after a 2-word Standard timestamp vs after a 3-word IRIG timestamp) and scores which produces a plausible MIL-STD-1553 Command Word. The winning format is locked for the rest of the file (L2-DEC-011: no per-record re-detection).

**Multi-record probe (L2-DEC-015).** The probe walks up to *N* records by default (configurable via `decode.detect_records` in TOML or `--detect-records N` on the CLI, range `1..=32`, default `8`) and aggregates per-record scoring across the set. Probing multiple records strengthens detection on borderline files where the first record alone scores ambiguously — for example, a `wc=7` record whose Command Word happens to look plausible at both candidate offsets — and a later record whose Cmd Word position only fits one format disambiguates the call.

**Per-record scoring signals** (max `+5` IRIG / `+4` Standard per record):

| Signal | IRIG | Standard | Notes |
|--------|------|----------|-------|
| T/R consistency: the candidate Cmd Word's direction matches the Type Word's expected message-type direction (`BC_TO_RT` → Receive; `RT_TO_BC` → Transmit) | +2 | +2 | Only fires for type codes `0x02` and `0x04`; other types skip this signal. |
| Word-count plausibility: `tw.word_count − overhead == cmd.data_word_count` | +2 | +2 | IRIG overhead = 6, Standard overhead = 5. |
| IRIG range validity: hour < 24, minute < 60, second < 60, microsecond-hi < 16 | +1 | — | IRIG-only; Standard's 32-bit counter has no semantic fields to range-check. |

**Confidence classification (L2-DEC-016).** The aggregate score is classified into one of three buckets:

| Bucket | Condition | Behavior |
|--------|-----------|----------|
| **Decisive** | `max_score ≥ 8` AND `margin ≥ 6` | INFO log with score breakdown; chosen format used silently. |
| **Marginal** | passes the floor (`max_score ≥ 4` AND `margin ≥ 3`) but not Decisive | INFO log with score breakdown + hint to `--time-format` if the call is wrong. |
| **Ambiguous** | `max_score < 4` OR `margin < 3` | **Strict mode**: `MieTimestampFormatMismatchError` / exit class 2 (the "wrong file type" class shared with `NoValidRecords` and `HomogeneousPayload`). **Lenient mode (default)**: single WARN with the score breakdown, then proceed with the chosen format (back-compat for borderline files that decoded acceptably under earlier single-record detection). |

When both formats score equally, **IRIG wins** (L2-DEC-012). Flight-test recordings overwhelmingly use IRIG; this tie-break preserves the most common path.

**Thresholds rationale.** The Decisive thresholds (`8` floor, `6` margin) are clear of any single-record perfect signal — a Decisive call requires either two strong records pointing the same way or one strong record plus consistent weaker signals from others. The Ambiguous thresholds (`4` floor, `3` margin) are conservative: they fire only when the probe genuinely could not distinguish, not when the call is decisive but the absolute score is low because the probe set was small (e.g., a one-record file scores at most `5` IRIG / `4` Standard — that's Marginal, not Ambiguous).

---

## 6. Per-format record layouts

MIE-Decoder classifies every record into one of 11 formats (L2-MSG-001). The Type Word's message type code plus the Command Word's `data_word_count` and broadcast bit determine which.

Identification happens in two layers: the raw Type Word message-type code (§4.1) selects a family, and the Command Word's direction, `data_word_count`, and broadcast bit then pin the exact format. The 11 decoded formats, their source Type Word code, and how each is identified — the per-format byte shapes follow in §6.1–§6.11:

| Decoded format | Source type | How it is identified | Shape |
|---|---|---|---|
| `RECEIVE` | `BC_TO_RT` (`0x02`) | BC→RT; Command direction must be Receive | §6.1 |
| `TRANSMIT` | `RT_TO_BC` (`0x04`) | RT→BC; Command direction must be Transmit | §6.2 |
| `RT_TO_RT` | `RT_TO_RT` (`0x08`) | Two Command Words: transmit-RT status, data, then receive-RT status | §6.3 |
| `RECEIVE_BROADCAST` | `BROADCAST_BC_TO_RT` (`0x10`) | Broadcast data from the BC; no Status Word | §6.4 |
| `RT_TO_RT_BROADCAST` | `BROADCAST_RT_TO_RT` (`0x18`) | One RT transmits, all RTs listen; no receive-status word | §6.5 |
| `MODE_CODE_TX_DATA` | `MODE_COMMAND` (`0x01`) | Non-broadcast mode code, T/R = Transmit, one data word | §6.6 |
| `MODE_CODE_RX_DATA` | `MODE_COMMAND` (`0x01`) | Non-broadcast mode code, T/R = Receive, one data word | §6.7 |
| `MODE_CODE_NO_DATA` | `MODE_COMMAND` (`0x01`) | Non-broadcast mode code, no data word (either direction) | §6.8 |
| `MODE_CODE_BCAST_NO_DATA` | `MODE_COMMAND` (`0x01`) | RT address 31, no data word | §6.9 |
| `MODE_CODE_BCAST_DATA` | `MODE_COMMAND` (`0x01`) | RT address 31, one data word | §6.10 |
| `SPURIOUS_DATA` | `SPURIOUS_DATA` (`0x20`) | No Command / Status structure; `RT`, `MSG`, `CMD`, `STAT` left empty | §6.11 |

A record can additionally be flagged as an **error record** (Type Word bit 14 set) on top of any format above — it is not a separate message type. See §7 for the error-record lifecycle and the `0x01xx` / `0x20xx` code tables.

In every shape below, "TS" denotes the timestamp triple (3 words for IRIG, 2 for Standard). "Cmd" is the Command Word; "Status" is the Status Word; "Cmd2" is the second Command Word for RT-to-RT formats; "Data[N]" is N data words.

### 6.1 Receive — BC→RT (`0x02`)

The Bus Controller sends data to a Remote Terminal. The RT then responds with a Status Word.

```
┌──────┬─────┬─────┬─────────┬────────┐
│ Type │ TS  │ Cmd │ Data[N] │ Status │       N = Cmd.data_word_count
└──────┴─────┴─────┴─────────┴────────┘
```

Total words = `1 + ts_words + 1 + N + 1`. Cmd.direction must be Receive (L2-SYN-020).

### 6.2 Transmit — RT→BC (`0x04`)

The BC requests data from a Remote Terminal; the RT transmits the Status Word followed by data.

```
┌──────┬─────┬─────┬────────┬─────────┐
│ Type │ TS  │ Cmd │ Status │ Data[N] │       N = Cmd.data_word_count
└──────┴─────┴─────┴────────┴─────────┘
```

Cmd.direction must be Transmit (L2-SYN-021).

### 6.3 RT-to-RT (`0x08`)

The BC commands one RT to transmit and another to receive. Two Command Words: the first targets the transmitting RT (direction = Transmit), the second targets the receiving RT (direction = Receive, per L2-SYN-023).

```
┌──────┬─────┬─────┬──────┬───────────┬─────────┬───────────┐
│ Type │ TS  │ Cmd │ Cmd2 │ Status_tx │ Data[N] │ Status_rx │
└──────┴─────┴─────┴──────┴───────────┴─────────┴───────────┘
```

`N` is `Cmd2.data_word_count` (the count from the receiving side). Both Command Words encode the count and MUST agree; a mismatch is rejected as corruption (L2-SYN-027). `Status_tx` is the transmitting RT's status; `Status_rx` is the receiving RT's status.

### 6.4 Receive Broadcast — BC→RT broadcast (`0x10`)

The BC broadcasts data to all RTs (RT 31 is the broadcast address). Broadcast Receive transactions get **no Status Word** because individual RTs aren't allowed to respond on broadcast.

```
┌──────┬─────┬─────┬─────────┐
│ Type │ TS  │ Cmd │ Data[N] │       N = Cmd.data_word_count
└──────┴─────┴─────┴─────────┘
```

### 6.5 RT-to-RT Broadcast (`0x18`)

The BC commands one RT to transmit to all RTs. The transmitting RT responds with a Status; the receiving RTs do not.

```
┌──────┬─────┬─────┬──────┬───────────┬─────────┐
│ Type │ TS  │ Cmd │ Cmd2 │ Status_tx │ Data[N] │
└──────┴─────┴─────┴──────┴───────────┴─────────┘
```

### 6.6 Mode Code Tx with data — `0x01`, T/R = 1, single data word

The BC issues a mode code requesting the RT to transmit one data word (e.g., "transmit BIT word", "transmit last command"). The RT responds with Status then one Data word.

```
┌──────┬─────┬─────┬────────┬─────────┐
│ Type │ TS  │ Cmd │ Status │ Data[1] │
└──────┴─────┴─────┴────────┴─────────┘
```

### 6.7 Mode Code Rx with data — `0x01`, T/R = 0, single data word

The BC issues a mode code that includes one data word (e.g., "synchronize with data word"). The RT receives the data word then responds with Status.

```
┌──────┬─────┬─────┬─────────┬────────┐
│ Type │ TS  │ Cmd │ Data[1] │ Status │
└──────┴─────┴─────┴─────────┴────────┘
```

### 6.8 Mode Code No Data — `0x01`, no data words

Mode codes that carry no data (e.g., "reset RT", "synchronize without data", "transmit status word"). RT acknowledges with Status only. This shape is **direction-independent**: a no-data mode code lands here whether T/R is Receive *or* Transmit (the `CMD` column preserves the direction). Only a mode code long enough to carry a data word becomes `Tx with data` / `Rx with data` — a record that is classified `ModeCodeTxData` but lacks the data word is too short for the L2-SYN-022 capacity check, which is why a no-data transmit mode code must classify here (it was previously dropped, see CHANGELOG).

```
┌──────┬─────┬─────┬────────┐
│ Type │ TS  │ Cmd │ Status │
└──────┴─────┴─────┴────────┘
```

### 6.9 Mode Code Broadcast No Data — `0x01`, RT = 31, no data

Broadcast mode code without data; no Status (broadcast RTs don't respond).

```
┌──────┬─────┬─────┐
│ Type │ TS  │ Cmd │
└──────┴─────┴─────┘
```

### 6.10 Mode Code Broadcast with data — `0x01`, RT = 31, one data word

Broadcast mode code carrying a single data word; no Status.

```
┌──────┬─────┬─────┬─────────┐
│ Type │ TS  │ Cmd │ Data[1] │
└──────┴─────┴─────┴─────────┘
```

### 6.11 SPURIOUS_DATA (`0x20`)

Orphan data words captured by the recording card without an associated Command Word — either the leftover bytes from an aborted transaction (a continuation; see §7) or genuine bus noise (standalone).

```
┌──────┬─────┬─────────┐
│ Type │ TS  │ Data[N] │       N = word_count - 1 - ts_words
└──────┴─────┴─────────┘
```

There is no Command Word, no Status Word. The CSV `RT` and `MSG` columns are empty; `CMD` is empty; `STAT` is empty. The `ERROR` column reads `SPURIOUS`. The `ERROR_CODE` column carries a decoder-assigned code (see §7).

---

## 7. Error records and SPURIOUS continuation

### 7.1 The error-record lifecycle

When the DDC card detects a bus error mid-transaction (parity error, no RT response, etc.), it:

1. **Sets bit 14** of the in-progress record's Type Word (the Error Flag).
2. **Truncates the payload** — recording stops at whatever was received.
3. **Appends a 16-bit Error Word** in the final word slot of the record.
4. **Optionally** records a follow-up SPURIOUS_DATA record containing any remaining bus words from the interrupted transaction.

The resulting error record looks like:

```
┌──────────────┬─────┬─────┬───────────────┬────────────┐
│ Type (bit14) │ TS  │ Cmd │ Truncated... │ Error Word │
└──────────────┴─────┴─────┴───────────────┴────────────┘
```

The Error Word's value names the failure (the DDC hardware error code). MIE-Decoder reads the final word of every error record and emits it in the CSV `ERROR_CODE` column with `ERROR` in the `ERROR` column. Strict mode rejects unknown DDC error codes (L2-ERR-004); lenient mode passes them through as `UNKNOWN`.

### 7.2 DDC hardware error codes (`0x01xx`)

| Code | Constant | Description |
|------|----------|-------------|
| `0x011E` | `ERROR_MANCHESTER_PARITY` | Manchester encoding error, parity error, or bit-count error. |
| `0x0120` | `ERROR_NO_RESPONSE` | No status word from RT, or fewer data words than the Command Word specified. |
| `0x0136` | `ERROR_INVERTED_SYNC` | Inverted sync pattern detected on a data word. |
| `0x0140` | `ERROR_TOO_MANY_WORDS` | More data words received than the Command Word specified. |
| `0x0150` | `ERROR_UNKNOWN_DDC` | Catch-all for undocumented DDC error conditions. |

### 7.3 SPURIOUS continuation (decoder-assigned `0x20xx`)

When a SPURIOUS_DATA record immediately follows an error record, MIE-Decoder assigns it the **continuation** code `0x2000`. When a SPURIOUS_DATA record stands alone (the immediately preceding decoded record was not an error), it's assigned the **standalone** code `0x2001`. The `0x20` prefix matches the SPURIOUS_DATA message type code for visual identification.

"Immediately following" is defined relative to the immediately preceding **successfully decoded** record (L2-ERR-005). A classification failure or unrecoverable validation error between an error record and a SPURIOUS_DATA record resets the continuation flag — the corruption itself is a boundary, and the SPURIOUS_DATA falls through to the standalone code.

| Code | Constant | Meaning |
|------|----------|---------|
| `0x2000` | `ERROR_SPURIOUS_CONTINUATION` | This SPURIOUS_DATA is the tail of a preceding errored transaction. |
| `0x2001` | `ERROR_SPURIOUS_STANDALONE` | Standalone SPURIOUS_DATA — genuine bus noise or partial transmission. |

---

## 8. MIL-STD-1553 Command Word (16-bit, little-endian)

The Command Word is sent by the Bus Controller to initiate every bus transaction. It identifies the target Remote Terminal, transfer direction, subaddress, and number of data words.

| Bits | Width | Field | Description |
|------|-------|-------|-------------|
| 15–11 | 5 | RT Address | Remote Terminal address (0–30). Address 31 is reserved for broadcast commands where all RTs on the bus must respond. Each RT on a 1553 bus has a unique address assigned during system integration. |
| 10 | 1 | T/R | Transfer direction from the RT's perspective. `1` = Transmit (RT sends data to BC); `0` = Receive (BC sends data to RT). This bit determines the wire order of Status Word and Data Words in the record. |
| 9–5 | 5 | Subaddress | Subaddress (0–31). Identifies the specific data set or function within the RT. SA `0` and SA `31` are reserved for mode code messages; SAs 1–30 address data buffers. |
| 4–0 | 5 | Word Count | Number of 16-bit data words in this transfer (1–32). A raw value of `0` encodes 32 words. |

---

## 9. MIL-STD-1553 Status Word (16-bit, little-endian)

The Status Word is returned by the RT to acknowledge a transaction.

| Bits | Width | Field | Description |
|------|-------|-------|-------------|
| 15–11 | 5 | RT Address | Echoes the RT address from the Command Word. A mismatch is an L2-SYN anomaly (WARN; continue per L2-SYN-024) — possible bus interference on a multi-drop bus, not necessarily corruption. |
| 10 | 1 | Message Error | Set if the RT detected an error in the received message. |
| 9 | 1 | Instrumentation | Set if the RT is an instrumentation device. |
| 8 | 1 | Service Request | Set if the RT is requesting service from the BC. |
| 7–5 | 3 | Reserved | Reserved bits. |
| 4 | 1 | Broadcast Received | Set if the RT received a valid broadcast command. |
| 3 | 1 | Busy | Set if the RT is busy and cannot process the command. |
| 2 | 1 | Subsystem Flag | Application-specific flag. |
| 1 | 1 | Dynamic Bus Control Accept | Set if the RT accepts bus controller role. |
| 0 | 1 | Terminal Flag | Application-specific terminal flag. |

---

## 10. CSV output reference

The CSV layout is column-name and column-order compatible with DDC vendor recording software (L1-OUT-001). Forty-six columns in order: `TIME_STAMP`, `RT`, `MSG`, `WD01`–`WD32`, `STAT`, `CMD`, `MUX`, `TERM_NAME`, `BUS`, `DELTA`, `ERROR`, `ERROR_CODE`, `IM_GAP`, `RCV_GAP`, `XMT_GAP`.

For documented divergences from vendor output (vendor-empty columns we preserve, line-ending normalization, the IRIG day-of-year discrepancy), see [`VENDOR-CSV-DIFFS.md`](VENDOR-CSV-DIFFS.md).

### `TIME_STAMP`

**IRIG format:** `DAY:HH:MM:SS.uuuuuu`

- DAY: Day of year (unpadded integer, 1–366).
- HH: Hour (2 digits, zero-padded, 0–23).
- MM: Minutes (2 digits, zero-padded, 0–59).
- SS: Seconds (2 digits, zero-padded, 0–59).
- uuuuuu: Microseconds (6 digits, zero-padded, 0–999999).

**Example:** `192:15:54:50.456225`

**Standard format:** `0x` + 8 uppercase hex digits (the raw 32-bit counter).

**Example:** `0x000186A0`

### `RT`

Remote Terminal address as a decimal integer (0–30, or 31 for broadcast). Empty for SPURIOUS_DATA.

### `MSG`

Message identifier combining subaddress and transfer direction.

**Format:** `<Subaddress><T|R>` (Subaddress is a decimal integer 0–31; T = Transmit, R = Receive)

**Examples:** `11R` (SA 11 Receive), `22T` (SA 22 Transmit)

Empty for SPURIOUS_DATA.

### `WD01` through `WD32`

Raw 16-bit data words in uppercase 4-character hex (no `0x` prefix). Words are in bus wire order. Columns beyond the actual data word count for this record are **empty cells** (not `0000`).

**Example:** `0400`, `CA22`, `0000` (third position has a literal zero data word)

### `STAT`

Raw 16-bit MIL-STD-1553 Status Word in uppercase 4-character hex. Empty when no Status Word is present (e.g., ReceiveBroadcast, ModeCodeBcastNoData, ModeCodeBcastData, SPURIOUS_DATA).

**Example:** `7800` (RT 15, no errors, no flags)

### `CMD`

Raw 16-bit MIL-STD-1553 Command Word in uppercase 4-character hex. Empty for SPURIOUS_DATA.

**Example:** `797E` (RT 15, Receive, SA 11, WC 30)

### `MUX`, `TERM_NAME`

`MUX` is populated from a field of the input **file name** by default (L2-WRT-020) — operators encode a source/recorder id in the name; the decoder splits the basename on a configurable delimiter (default `.`) and emits the configured field (default index `4`). It is empty with `--no-mux` / `[mux] enabled = false`, or when the field is absent. A MUX value containing the CSV delimiter, a quote, or a line break is RFC4180-quoted (identically in both implementations). `TERM_NAME` is a vendor compatibility column, always empty. See [`VENDOR-CSV-DIFFS.md`](VENDOR-CSV-DIFFS.md) §3 for vendor semantics and the `--no-mux` vendor-exact path.

### `BUS`

Single character `A` or `B`. Always populated.

### `DELTA`

Inter-arrival time in seconds with microsecond resolution (`0.000000` format — exactly 6 decimal places), or empty.

DELTA is the elapsed time between the current message and the most recent prior message sharing the **same** `<RT>:<MSG>` key. First-occurrence: `0.000000`. Errored records participate in DELTA tracking (L2-RDR-016). SPURIOUS_DATA records have empty DELTA (no key, L2-RDR-018). Standard-timestamp records have empty DELTA unless a tick rate is configured via `standard_tick_rate_hz` / `--standard-tick-rate-hz`, in which case they participate like IRIG (L2-RDR-019, L2-DEC-017). Non-monotonic timestamps produce empty DELTA + a single WARN per RT/MSG key per file (L2-RDR-017).

**Operational significance:** DELTA directly reveals the BC scheduling rate for each unique message type. ~0.016 s → 60 Hz minor frame; ~0.033 s → 30 Hz; ~0.001 s → adjacent messages in the same frame. Jitter or drift across a recording can indicate bus loading anomalies, missed scheduling cycles, BC priority changes, or intermittent RT response failures.

### `ERROR`

| Value | Meaning |
|-------|---------|
| _(empty)_ | Normal message, no error detected. |
| `ERROR` | DDC card detected an error mid-transaction. Type Word bit 14 is set. Payload is truncated and an Error Word is appended. `ERROR_CODE` contains the DDC hardware code. |
| `SPURIOUS` | Spurious data record (Type Word message type = `0x20`). `ERROR_CODE` contains `2000` (continuation) or `2001` (standalone). |

In **separate** error mode (the default), this column is always empty in the main CSV file — errored / spurious rows go to the `<output>_errors<suffix>` companion file. In **inline** mode, all three values appear in the single output CSV.

### `ERROR_CODE`

DDC hardware error code or MIE-Decoder decoder-assigned code in uppercase 4-character hex. Empty for normal messages. See §7 for the full table.

### `IM_GAP`, `RCV_GAP`, `XMT_GAP`

Vendor compatibility columns for inter-message / receive / transmit timing gaps. Not decoded in MIE-Decoder v1; empty. See [`VENDOR-CSV-DIFFS.md`](VENDOR-CSV-DIFFS.md) §3.

---

## 11. Worked hex-to-CSV decodes

Three representative records walked from raw bytes to the CSV row they produce. These are the canonical fixtures used by the cross-implementation conformance suite (see `tests/conformance/inputs/basic-multi-record.hex`).

### 11.1 RT 15 SA 11 Receive (a `0x02` Receive record)

**Hex bytes (72 bytes total = 36 words):**

```
02 24                                ← Type Word: 0x2402
0F 18 26 DB 21 F6                   ← IRIG timestamp (3 words)
7E 79                                ← Command Word: 0x797E
00 04 00 00 00 00 2F 00 22 CA       ← WD01–WD05
2F 00 22 CA 00 00 00 00 00 00       ← WD06–WD10
00 00 00 00 00 00 00 00 00 00       ← WD11–WD15
00 00 00 00 00 00 00 00 00 00       ← WD16–WD20
00 00 00 00 00 00 00 00 00 00       ← WD21–WD25
00 00 00 00 00 00 00 00 71 C7       ← WD26–WD30
00 78                                ← Status Word: 0x7800
```

**Type Word decode** (0x2402, LE bytes `02 24`):
- Message type: bits 0–6 = `0x02` (`BC_TO_RT` Receive).
- Bus: bit 7 = 0 → Bus A.
- Word count: bits 8–13 = 36. Record bytes = 72.
- Error: bit 14 = 0.
- Reserved: bit 15 = 0.

**IRIG timestamp decode** (upper `0x180F`, middle `0xDB26`, lower `0xF621`):
- Upper: freerun bit = 0; day-of-year = `(0x180F >> 5) & 0x1FF` = 192; hour = `0x180F & 0x1F` = 15.
- Middle: minute = `(0xDB26 >> 10) & 0x3F` = 54; second = `(0xDB26 >> 4) & 0x3F` = 50; microsecond high 4 = `0xDB26 & 0xF` = 6.
- Lower: microsecond low 16 = `0xF621` = 63009.
- Reconstructed microsecond = `(6 << 16) | 63009` = 456225.
- Format: `192:15:54:50.456225` ✓

**Command Word decode** (0x797E):
- RT: bits 15–11 = `(0x797E >> 11) & 0x1F` = 15.
- T/R: bit 10 = `(0x797E >> 10) & 1` = 0 → Receive.
- Subaddress: bits 9–5 = `(0x797E >> 5) & 0x1F` = 11.
- Word Count: bits 4–0 = `0x797E & 0x1F` = 30.

**Format classification:** Type = `0x02`, Cmd.T/R = Receive → `RECEIVE` format (§6.1). Payload layout: `Cmd, Data[30], Status`.

**Status Word:** `0x7800` (RT 15, no flags). Sits at the end of the record per the Receive layout.

**Resulting CSV row** (45 cells; 30 WD columns populated, WD31–WD32 empty, vendor columns empty):

```
192:15:54:50.456225,15,11R,0400,0000,0000,002F,CA22,002F,CA22,0000,0000,0000,0000,0000,0000,0000,0000,0000,0000,0000,0000,0000,0000,0000,0000,0000,0000,0000,0000,0000,0000,C771,,,7800,797E,,,A,0.000000,,,,,
```

DELTA is `0.000000` because this is the first occurrence of the `15:11R` key in the file.

### 11.2 RT 15 SA 22 Receive with fewer words (a `0x02` Receive with WC=11)

**Hex bytes** (34 bytes total = 17 words):

```
02 11 0F 18 26 DB 38 F7   ← Type Word (0x1102, wc=17, no error) + IRIG timestamp
CB 7A                      ← Command Word: 0x7ACB
00 10 00 00 07 00 00 08   ← WD01–WD04
00 00 00 00 00 00 00 00   ← WD05–WD08
00 00 C8 80 E8 03           ← WD09–WD11
00 78                      ← Status Word: 0x7800
```

**Type Word:** `0x1102` → msg_type=0x02, Bus A, wc=17, no error.

**Timestamp:** Same day/hour/minute/second as above. Microseconds reconstruct to 456504. Format: `192:15:54:50.456504`.

**Command Word:** `0x7ACB` → RT 15, T/R = Receive, SA 22, WC = 11.

**Format classification:** RECEIVE. Layout: `Cmd, Data[11], Status`.

**Resulting CSV row** (WD01–WD11 populated, WD12–WD32 empty):

```
192:15:54:50.456504,15,22R,1000,0000,0007,0800,0000,0000,0000,0000,0000,80C8,03E8,,,,,,,,,,,,,,,,,,,,,,7800,7ACB,,,A,0.000000,,,,,
```

DELTA = `0.000000` because `15:22R` is a different key from `15:11R`; this is its first occurrence too.

### 11.3 RT 15 SA 22 Transmit (a `0x04` Transmit record — Status before Data)

**Hex bytes** (72 bytes total = 36 words):

```
04 24 0F 18 26 DB E3 F9   ← Type Word (0x2404) + IRIG timestamp
DE 7E                      ← Command Word: 0x7EDE
00 78                      ← Status Word: 0x7800 (note: BEFORE data on Transmit)
20 10 82 41 00 00 08 15   ← WD01–WD04
... 26 more data words ...
```

**Type Word:** `0x2404` → msg_type = `0x04` (`RT_TO_BC` Transmit), Bus A, wc=36.

**Timestamp:** microseconds 457187 → `192:15:54:50.457187`.

**Command Word:** `0x7EDE` → RT 15, T/R = Transmit, SA 22, WC = 30.

**Format classification:** TRANSMIT. Layout: `Cmd, Status, Data[30]` — note Status comes **before** Data on Transmit (the wire order reflects the on-bus transaction), unlike Receive which has Status after Data.

**Resulting CSV row:**

```
192:15:54:50.457187,15,22T,1020,4182,0000,1508,0000,0000,0000,0000,FE00,0000,0000,0000,0000,0000,0000,0000,0000,0000,0003,0000,0000,0000,0000,0000,0000,2000,0000,0000,0000,0000,,,7800,7EDE,,,A,0.000000,,,,,
```

DELTA = `0.000000` because `15:22T` is yet another key (different direction from `15:22R`).

---

## 12. See also

- [`USER-GUIDE.md`](USER-GUIDE.md) — end-to-end CLI walkthrough for analysts.
- [`EXAMPLES.md`](EXAMPLES.md) — runnable cookbook of common operator tasks.
- [`CONFIG-REFERENCE.md`](CONFIG-REFERENCE.md) — every TOML key.
- [`ERROR-CATALOG.md`](ERROR-CATALOG.md) — every error variant, exit code, DDC code, decoder code.
- [`VENDOR-CSV-DIFFS.md`](VENDOR-CSV-DIFFS.md) — alignment with vendor CSV and divergence-reporting protocol.
- [`MAINTAINER-GUIDE.md`](MAINTAINER-GUIDE.md) — for maintainers adding new message formats or fields.
- [`L1-REQ.md`](L1-REQ.md), [`L2-REQ.md`](L2-REQ.md), [`L3-REQ.md`](L3-REQ.md) — normative spec.
- [`TRACE-MATRIX.md`](TRACE-MATRIX.md) — auto-generated forward trace from requirements to tests.
