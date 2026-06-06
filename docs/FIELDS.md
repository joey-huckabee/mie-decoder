# MIE-Decoder Field Reference

**Document ID:** MIE-FIELDS-001
**Version:** 1.0.0

---

## Binary Record Fields

### Type Word (16-bit, little-endian)

The Type Word is the first 16-bit word of every binary record. It
describes the message classification, bus source, total record length,
and error status.

| Bits | Width | Field | Description |
|------|-------|-------|-------------|
| 0–6 | 7 | Message Type | DDC message type code. Known values: 0x01, 0x02, 0x04, 0x08, 0x10, 0x18, 0x20. Determines the wire-order layout of command word, status word, and data words within the record payload. Type 0x02 indicates a standard BC-to-RT or RT-to-BC transfer; type 0x04 indicates a transmit; other codes may indicate RT-to-RT, mode codes, or broadcast messages. |
| 7 | 1 | Bus ID | Identifies which MIL-STD-1553 redundant bus this message was captured on. 0 = Bus A, 1 = Bus B. MIL-STD-1553 defines two electrically independent buses for fault tolerance; both carry the same logical traffic. |
| 8–13 | 6 | Word Count | Total record size in 16-bit words, including the Type Word itself, the 3-word IRIG timestamp, the command word, the status word, and all data words. Multiply by 2 to get the record size in bytes. Minimum valid value is 5 (Type Word + Timestamp + Command Word with no data). |
| 14 | 1 | Error Flag | Set to 1 if the recording card detected an error in this message. Errors include: parity errors, Manchester encoding violations, no RT response (timeout), incorrect word count, or status word errors. When set, the record payload may use a different layout than normal messages. |
| 15 | 1 | Reserved | Reserved for future use. Should be 0. |

### IRIG Timestamp (3 × 16-bit words, little-endian)

The IRIG timestamp occupies three consecutive 16-bit words immediately
following the Type Word. It records the time of the first word of the
1553 message on the bus. The format follows the IRIG-B standard time
code with extensions for microsecond resolution.

**Upper Word:**

| Bits | Width | Field | Description |
|------|-------|-------|-------------|
| 15 | 1 | Freerun | Set to 1 when the external IRIG time source is unavailable and the card is using its internal free-running oscillator. When freerun is active, day/hour values may not reflect real-world calendar time, but relative timing between messages remains valid. |
| 14 | 1 | Reserved | Reserved for future use. |
| 13–5 | 9 | Day of Year | Day of year (1–366). NOTE: empirical testing has shown a discrepancy between the binary-decoded value and vendor CSV output for this field on some DDC card models. The bit extraction is correct per the DDC specification, but the card firmware may use a different encoding (possibly BCD or a different field width). All other timestamp fields are validated correct. |
| 4–0 | 5 | Hour | Hour of day (0–23). |

**Middle Word:**

| Bits | Width | Field | Description |
|------|-------|-------|-------------|
| 15–10 | 6 | Minutes | Minute of hour (0–59). |
| 9–4 | 6 | Seconds | Second of minute (0–59). |
| 3–0 | 4 | Microseconds [19:16] | Upper 4 bits of the 20-bit microsecond counter. Combined with the Lower Word to form a value from 0 to 999,999. |

**Lower Word:**

| Bits | Width | Field | Description |
|------|-------|-------|-------------|
| 15–0 | 16 | Microseconds [15:0] | Lower 16 bits of the 20-bit microsecond counter. Full microsecond value = (Middle[3:0] << 16) | Lower[15:0]. |

**Reconstruction:**
```
microsecond = (middle_word & 0xF) << 16 | lower_word
```

### MIL-STD-1553 Command Word (16-bit, little-endian)

The Command Word is sent by the Bus Controller (BC) to initiate every
bus transaction. It identifies the target Remote Terminal, transfer
direction, subaddress, and number of data words.

| Bits | Width | Field | Description |
|------|-------|-------|-------------|
| 15–11 | 5 | RT Address | Remote Terminal address (0–30). Address 31 is reserved for broadcast commands where all RTs on the bus must respond. Each RT on a 1553 bus has a unique address assigned during system integration. |
| 10 | 1 | T/R | Transfer direction from the RT's perspective. 1 = Transmit (RT sends data to BC); 0 = Receive (BC sends data to RT). This bit determines the wire order of Status Word and Data Words in the record. |
| 9–5 | 5 | Subaddress | Subaddress (0–31). Identifies the specific data set or function within the RT. SA 0 and SA 31 are reserved for mode code messages (RT management commands like synchronize, reset, etc.). SAs 1–30 address data buffers. |
| 4–0 | 5 | Word Count | Number of 16-bit data words in this transfer (1–32). A raw value of 0 encodes 32 words. |

### MIL-STD-1553 Status Word (16-bit, little-endian)

The Status Word is returned by the RT to acknowledge a transaction.

| Bits | Width | Field | Description |
|------|-------|-------|-------------|
| 15–11 | 5 | RT Address | Echoes the RT address from the Command Word. |
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

## CSV Output Fields

### TIME_STAMP

IRIG-format timestamp of the first word of this message on the 1553 bus.

**Format:** `DAY:HH:MM:SS.uuuuuu`

- DAY: Day of year (unpadded integer)
- HH: Hour (2 digits, zero-padded)
- MM: Minutes (2 digits, zero-padded)
- SS: Seconds (2 digits, zero-padded)
- uuuuuu: Microseconds (6 digits, zero-padded)

**Example:** `192:15:54:50.456225`

### RT

Remote Terminal address as a decimal integer (0–30).

### MSG

Message identifier combining subaddress and transfer direction.

**Format:** `<Subaddress><T|R>`

- Subaddress: Decimal integer (0–31)
- T: Transmit (RT→BC)
- R: Receive (BC→RT)

**Examples:** `11R` (Subaddress 11, Receive), `22T` (Subaddress 22, Transmit)

### WD01 through WD32

Raw 16-bit data words in uppercase 4-character hexadecimal. Words are
in bus wire order. Columns beyond the actual data word count for this
message are empty strings.

**Example:** `0400`, `CA22`, `0000`

### STAT

Raw 16-bit MIL-STD-1553 Status Word in uppercase 4-character hexadecimal.

**Example:** `7800` (RT 15, no errors, no flags)

### CMD

Raw 16-bit MIL-STD-1553 Command Word in uppercase 4-character hexadecimal.

**Example:** `797E` (RT 15, Receive, SA 11, 30 words)

### MUX

Multiplexer label or subchannel identifier. Derived from external
configuration (TMATS or recording software setup). Not present in the
binary record data. Empty in v1.0.0.

### TERM_NAME

Terminal or equipment name associated with the RT/SA combination. Derived
from external configuration (filename conventions, TMATS setup files, or
recording software database). Not present in the binary record data.
Empty in v1.0.0.

### BUS

Redundant bus identifier: `A` or `B`.

### DELTA

**Inter-arrival time** for each unique message type, measured in seconds
with microsecond resolution (six decimal places). May also be **empty**
under the conditions listed below.

DELTA is the elapsed wall-clock time between the current message and the
most recent prior message that shares the **same** Remote Terminal address
(RT) **and** message identifier (MSG). The MSG identifier is the
combination of Subaddress and Direction (e.g., `11T` for Subaddress 11
Transmit; `22R` for Subaddress 22 Receive).

Messages are grouped by the composite key `<RT>:<MSG>`. For example:
- All messages to RT 15 SA 11 Receive are tracked independently.
- RT 15 SA 11 Transmit is a separate group.
- RT 30 SA 11 Receive is a separate group.

For the **first occurrence** of any RT/MSG combination in a recording
file, DELTA is `0.000000`.

**Errored records** (Type Word bit 14 set) participate in DELTA tracking.
An errored RT 15 SA 11 Receive record advances the `15:11R` cursor, and
the next message with that same key — whether errored or not — computes
its DELTA against the errored record's timestamp. This preserves the
diagnostic signal for flaky RT/MSG pairs whose anomalies cluster in time.

**DELTA is empty** (an empty CSV cell, not `0.000000`) in any of these
cases:

- **SPURIOUS_DATA records.** A spurious record has no Command Word, so
  it has no RT/MSG key to track against. It contributes nothing to the
  per-key cursor.
- **Standard-format timestamps.** The Standard timestamp is a free-running
  counter whose tick rate is card-dependent and not encoded in the file.
  Without a calibration value, raw ticks cannot be truthfully converted
  to seconds. DELTA is therefore empty for every record carrying a
  Standard timestamp. (A future configuration value, expected in a
  follow-up release, will enable DELTA when the tick rate is supplied.)
- **Non-monotonic timestamps.** When a record's timestamp is older than
  the prior record for the same RT/MSG key (year rollover, freerun
  reset, out-of-order capture), DELTA is empty and the implementation
  emits a single WARN per key per file. Subsequent occurrences for the
  same key continue to compute DELTA against the most recent timestamp
  (which may itself have produced an empty DELTA).

**Operational significance:** DELTA directly reveals the Bus Controller's
scheduling rate for each unique message type:
- ~0.016s → 60 Hz minor frame rate
- ~0.033s → 30 Hz minor frame rate
- ~0.001s → adjacent messages in the same minor frame

Jitter or drift in DELTA values across a recording can indicate:
- Bus loading anomalies (overloaded minor frames)
- Missed scheduling cycles (BC dropped a poll)
- BC priority changes (rescheduled message order)
- Intermittent RT response failures (timeout + retry)

### ERROR

Error classification label for the message.

| Value | Meaning |
|-------|---------|
| _(empty)_ | Normal message, no error detected. |
| `ERROR` | DDC card detected an error mid-transaction. Type Word bit 14 is set. The record contains a truncated payload and an appended Error Word. The ERROR_CODE column contains the DDC hardware error code. |
| `SPURIOUS` | Spurious data record (Type Word message type = 0x20). Either a continuation of a preceding errored message (ERROR_CODE = 0x2000) or standalone bus noise (ERROR_CODE = 0x2001). No Command Word or Status Word is present. |

### ERROR_CODE

DDC hardware error code or MIE-Decoder custom error code in uppercase
4-character hexadecimal. Empty for normal messages.

**DDC Hardware Error Codes (0x01xx range):**

| Code | Description |
|------|-------------|
| `011E` | Manchester encoding error, parity error, or incorrect bit count. The Error Word describes the 1553 bus word immediately preceding it in the record. |
| `0120` | No status word response from the RT, or fewer data words received than the Command Word specified. |
| `0136` | Inverted sync pattern detected on a data word. The expected data sync (opposite polarity from command/status sync) was not present. |
| `0140` | More data words received from the RT than the Command Word's word count field specified. |
| `0150` | Unknown or undocumented DDC error condition. |

**MIE-Decoder Custom Error Codes (0x20xx range):**

These codes are assigned by the decoder, not the DDC hardware. The
`0x20` prefix mirrors the SPURIOUS_DATA type code from Type Word bits
0–6, providing immediate visual identification.

| Code | Description |
|------|-------------|
| `2000` | Spurious Data: Continuation of a preceding errored message. The preceding record had bit 14 set and this SPURIOUS_DATA record contains the remaining bus words from the interrupted transaction. |
| `2001` | Spurious Data: Standalone. No preceding error record was detected. This represents bus noise, reflections, or partial transmissions captured by the monitor. |

### IM_GAP

Inter-message gap. Not decoded from the binary record in v1.0.0. Empty.

### RCV_GAP

Receive gap. Not decoded from the binary record in v1.0.0. Empty.

### XMT_GAP

Transmit gap. Not decoded from the binary record in v1.0.0. Empty.
