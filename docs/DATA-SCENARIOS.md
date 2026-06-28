# MIE-Decoder Data Scenarios

**What this page is.** A plain-language map of *every kind of data condition* the
decoder can meet — clean records, error records, odd timestamps, corruption,
empty or non-MIE files, multi-file merges, and so on — and exactly **how the tool
handles each one**: what it writes to the CSV, what it logs, and what exit code
it returns.

**How to use it.** Find your situation in the [Scenario index](#1-scenario-index),
jump to that section, and read the one- or two-paragraph explanation. Unfamiliar
term? See the [Glossary](#2-glossary) — every piece of jargon on this page
(including words like *oracle*) is defined there in one sentence.

This page **summarizes** behavior and points to the authorities for depth:
[`ERROR-CATALOG.md`](ERROR-CATALOG.md) for the full exit-code and error-code
tables, [`MIE-FORMAT.md`](MIE-FORMAT.md) for the binary format, and
[`USER-GUIDE.md`](USER-GUIDE.md) for step-by-step workflows. Where a rule comes
from a requirement it is cited inline (e.g. `L2-MRG-004`); the requirements live
in [`L1-REQ.md`](L1-REQ.md) / [`L2-REQ.md`](L2-REQ.md) / [`L3-REQ.md`](L3-REQ.md).

A note on modes that recur throughout: **lenient** mode (the default) keeps going
past bad data — it logs a warning and skips the offending record; **strict**
mode (`--strict`) stops at the first problem with a non-zero exit. Each scenario
below calls out where the two differ.

---

## 1. Scenario index

| If your data is / has… | Section | In one line |
|---|---|---|
| A normal recording | [§3 Input files](#3-input-file-scenarios) | Decodes to CSV, exit 0 |
| An empty (0-byte) file | [§3](#3-input-file-scenarios) | Single file: error, exit 1. In a merge with `--allow-partial`: dropped, `.partial`, exit 0 |
| A file that isn't MIE / is all-0xFF garbage | [§3](#3-input-file-scenarios) | "No valid records", exit 2 (single file) |
| A file cut off part-way through | [§3](#3-input-file-scenarios) | Truncated record skipped (lenient) or exit 1 (strict) |
| IRIG vs Standard vs free-running timestamps | [§4 Timestamps](#4-timestamp-scenarios) | Auto-detected; Standard has empty DELTA unless calibrated |
| One of the 11 transaction types | [§5 Record types](#5-record-type-scenarios) | Each maps to specific CSV columns |
| A bus error / error record | [§6 Errors & spurious](#6-error--spurious-data-scenarios) | `ERROR` column set, DDC code `0x01xx` |
| Orphan "spurious" data | [§6](#6-error--spurious-data-scenarios) | `SPURIOUS` row, code `0x2000` or `0x2001` |
| Corruption mid-file (lost sync) | [§7 Sync loss](#7-sync-loss-scenarios) | Recovered (exit 0) or unrecoverable (exit 3, or `.partial` with `--allow-partial`) |
| Several recordings to combine | [§8 Multi-file merge](#8-multi-file-merge-scenarios) | Time-sorted into one CSV |
| Merge inputs that can't share a clock | [§8](#8-multi-file-merge-scenarios) | Rejected, exit 6 |
| A bad / empty / unreadable file inside a merge | [§8](#8-multi-file-merge-scenarios) | Aborts — unless `--allow-partial`, then `.partial`, exit 0 |
| The same event recorded by two recorders | [§8](#8-multi-file-merge-scenarios) | Optional `--collapse-duplicates` |
| A choice of output layout | [§9 Output modes](#9-output-mode-scenarios) | Separate errors file (default), inline, or stdout |
| Records you want to keep or drop | [§10 Filters & MUX](#10-filter--mux-scenarios) | `--include-*` / `--exclude-*` |
| Any exit code, explained | [§11 Exit codes](#11-exit-code-quick-reference) | 0–6 reference |

---

## 2. Glossary

- **MIE file** — the proprietary binary recording produced by a DDC MIL-STD-1553
  PCI card. The tool decodes it to CSV.
- **Record / message** — one bus transaction in the file (a command and its data
  and/or status). Becomes one CSV row.
- **Transaction type** — which kind of 1553 exchange a record is (e.g. BC→RT
  receive, RT→BC transmit, mode code). There are 11; see [§5](#5-record-type-scenarios).
- **RT (Remote Terminal)** — a device address on the bus, 0–31 (31 is broadcast).
- **Subaddress (SA)** — a sub-channel within an RT, 0–31 (0 and 31 are mode-code
  subaddresses).
- **Bus A / Bus B** — the two redundant physical wires of a 1553 bus.
- **Command / Status / Data Word** — the 16-bit words a 1553 transaction is built
  from: the command starts it, the status is the RT's reply, the data words carry
  the payload.
- **Type Word** — MIE's own 16-bit header on each record (word count, message
  type, and the *error* bit). Detailed in [`MIE-FORMAT.md`](MIE-FORMAT.md).
- **IRIG / IRIG-B** — a timecode standard. An IRIG timestamp carries absolute
  wall-clock time (day-of-year, hour, minute, second, microsecond) from an
  external time source.
- **Standard timestamp** — a free-running 32-bit counter with no calendar meaning;
  its tick rate isn't stored in the file.
- **Freerun** — an IRIG record flagged as *not* calendar-locked (no absolute
  time). Cannot anchor a merge timeline.
- **Mode code** — a special 1553 command (no/one data word) used for bus
  housekeeping rather than data transfer.
- **SPURIOUS_DATA** — leftover data words with no command word, written by the
  card after a truncated transaction or as bus noise.
- **Sync / sync loss** — the decoder stays "in sync" by knowing where each record
  starts. Corruption can break that; **recoverable** loss is re-found by scanning
  forward, **unrecoverable** loss exhausts the scan window.
- **DELTA** — the CSV column giving the time since the previous message with the
  same RT / subaddress / direction (an inter-arrival gap).
- **MUX** — a CSV column the tool can fill from a field of the input *file name*
  (e.g. a recorder id), off-by-default-empty for vendor-exact output.
- **Partial output (`.partial`)** — when `--allow-partial` lets a corrupt or
  unreadable input through, the rows decoded *before* the failure are written to
  `<output>.partial` (the real destination is left untouched), exit 0.
- **Merge** — combining two or more recordings into one time-sorted CSV.
- **Recorder** — one input file in a merge; "the same event seen by two
  recorders" means identical content in two different input files.
- **Look-ahead** — to confirm a record is real (not coincidental garbage), the
  decoder checks that the *next* few records also look valid before accepting it.
- **Lenient vs strict** — see the note at the top of this page.
- **Oracle (conformance oracle)** — a byte-exact "known-good" CSV checked into
  [`tests/conformance/`](../tests/conformance/) that **both** the Rust and Python
  tools must reproduce *exactly*. It is the ground truth used to prove the two
  implementations agree (and that output stays vendor-compatible); when this page
  says a behavior is "pinned by an oracle," it means a conformance test would fail
  if either tool's output drifted.

---

## 3. Input-file scenarios

| Your file | What the tool does | Exit |
|---|---|---|
| **Valid MIE recording** | Decodes every record to a CSV row. | 0 |
| **Empty (0 bytes)** | Single-file: reports the file is empty and writes nothing. In a *merge*, the empty file fails at open — see [§8](#8-multi-file-merge-scenarios). | 1 |
| **Not an MIE file** (wrong type) | Scans the first 64 KB, finds no valid record, reports "no valid records." Try `dump` to inspect the bytes. | 2 |
| **All-0xFF / single-byte padding** ("homogeneous payload") | A defensive check rejects a file whose candidate records are byte-identical except for the timestamp — almost always a pad, not data. | 2 |
| **Truncated first record** | A valid header is found but the record runs past end-of-file. Strict: error; lenient: reported and skipped. | 1 / 0 |
| **Truncated mid-file record** | Same idea further in: strict errors; lenient skips the short record and continues. | 1 / 0 |
| **Records start past the 64 KB scan window** | Not detected; reported as "no valid records." | 2 |

These checks live in the reader/sync code; the full error names and the decision
tree by exit code are in [`ERROR-CATALOG.md`](ERROR-CATALOG.md).

---

## 4. Timestamp scenarios

Every record carries a timestamp in one of two on-the-wire formats. By default
the tool **auto-detects** which, by probing the first records and scoring how
well each interpretation produces valid commands.

| Timestamp | `TIME_STAMP` column | `DELTA` column |
|---|---|---|
| **IRIG** (48-bit, absolute) | `DAY:HH:MM:SS.uuuuuu` | Microsecond gaps |
| **Standard** (32-bit free-running counter) | Raw hex ticks | **Empty** — the tick rate isn't in the file |
| **Standard + `--standard-tick-rate-hz N`** | Raw hex ticks | Real microsecond gaps (you supplied the rate) |
| **Freerun IRIG** (no calendar anchor) | Relative IRIG fields | Present, but not wall-clock |

**Auto-detection outcomes** — the probe is scored (`L2-DEC-015/016`):

- **Decisive / marginal** — one format clearly wins; the tool uses it (an INFO log
  notes the choice; a marginal call hints you can force `--time-format` if wrong).
- **Ambiguous** — neither format is convincing. **Strict** mode stops with exit 2
  (`TimestampFormatMismatch`); **lenient** mode logs one WARN and proceeds with
  its best guess (IRIG wins ties — it's the common case in flight test).

Force a format with `--time-format irig|standard` to skip the probe entirely.
Day-of-year has a known per-card firmware quirk — see
[`VENDOR-CSV-DIFFS.md`](VENDOR-CSV-DIFFS.md).

---

## 5. Record-type scenarios

The decoder classifies each record into one of **11 transaction types** from its
Type Word and Command Word, and lays the CSV columns out to match the order the
words appear on the wire. The full byte-level shape of each is in
[`MIE-FORMAT.md`](MIE-FORMAT.md) §6; the short version:

| # | Type | When it appears | CSV signature |
|---|---|---|---|
| 1 | **Receive** (BC→RT) | Controller sends data to a terminal | `RT`, `MSG`, `CMD`, `STAT`, data words |
| 2 | **Transmit** (RT→BC) | Terminal sends data to the controller | as above; status precedes data on the wire |
| 3 | **RT-to-RT** | One terminal to another | two command words, two status words |
| 4 | **Receive broadcast** | Controller to all terminals | `RT`=31, no status |
| 5 | **RT-to-RT broadcast** | One terminal to all | two command words, one status |
| 6–10 | **Mode codes** (5 shapes) | Bus housekeeping (with/without data, unicast/broadcast) | mode-code subaddress; data word only if present |
| 11 | **SPURIOUS_DATA** | Orphan data words (see [§6](#6-error--spurious-data-scenarios)) | `RT`/`MSG`/`CMD`/`STAT` empty |

A record can additionally be flagged as an **error record** (Type Word bit 14) —
that's a property layered on top of any of the above; see next.

---

## 6. Error & spurious-data scenarios

### Error records (a bus error the card detected)

When the DDC card detects a bus error it sets **bit 14 of the Type Word**,
truncates the payload, and appends a 16-bit **Error Word** with a hardware code.
The decoder surfaces this as a row whose **`ERROR` column reads `ERROR`** and
whose **`ERROR_CODE`** is the DDC code:

| Code | Meaning |
|---|---|
| `0x011E` | Manchester / parity / bit-count error on the wire |
| `0x0120` | No response (RT silent, or too few data words) |
| `0x0136` | Inverted sync pattern on a data word |
| `0x0140` | More data words than the command specified |
| `0x0150` | A DDC error not in the list above (catch-all) |
| other `0x01xx` | Unknown firmware code — **strict** errors (exit 1); **lenient** emits the row and WARNs |

### Spurious data (orphan words)

If a transaction is cut short, the card may write the leftover words as a
`SPURIOUS_DATA` record (no command word). The decoder labels it **`SPURIOUS`** in
the `ERROR` column and assigns one of two *decoder* codes based on what came
before it:

| Code | Meaning |
|---|---|
| `0x2000` | **Continuation** — the spurious words follow a preceding *error* record |
| `0x2001` | **Standalone** — no preceding error (genuine bus noise) |

Spurious records are themselves *valid* records and never raise an error; they
pass sync normally and don't change the exit code.

**Where these rows go** depends on the output mode ([§9](#9-output-mode-scenarios)):
by default error and spurious rows are written to a separate `<stem>_errors.csv`;
with `--inline-errors` they stay in the main CSV. The complete code tables and
the operator decision tree are in [`ERROR-CATALOG.md`](ERROR-CATALOG.md).

---

## 7. Sync-loss scenarios

The decoder always knows where the next record should start. When a record there
fails validation, it tries to **recover** by scanning forward in small steps
(using look-ahead to avoid latching onto coincidental garbage), bounded to a
64 KB window.

| Situation | Behavior | Exit | Output |
|---|---|---|---|
| **Recoverable** — a valid record is found within 64 KB | WARN "sync lost … scanning", resumes; the recovery count is reported | 0 (*partial-recovered*) | Main CSV with the records found after recovery |
| **Unrecoverable** — 64 KB pass with nothing valid | Decode stops at the loss point | **3** | **No output file** (so it can't be mistaken for complete) |
| **Unrecoverable + `--allow-partial`** | The rows decoded *before* the loss are kept | 0 | `<output>.partial` (real destination untouched) |

`--allow-partial` is the explicit opt-in for "save what was decoded before the
corruption" — see `L1-EXIT-004`. Tune recovery aggressiveness with
`--lookahead-records N` (default 2). Sync loss is *not* the same as an error
record — an errored record is valid data the card flagged; sync loss is the
stream itself becoming unreadable.

---

## 8. Multi-file merge scenarios

Pass more than one input (positionals, `--manifest`, or `--glob`) and `decode`
merges them into a single time-sorted CSV, streaming so memory stays bounded by
the number of files. A single input is unaffected by everything in this section.

### Ordering and compatibility

| Situation | Behavior | Exit |
|---|---|---|
| **All inputs calendar-locked IRIG** | Records interleaved by absolute time; DELTA recomputed across the unified timeline | 0 |
| **A Standard-format input** | Rejected before any output — no shared clock | **6** |
| **A freerun-leading input** | Rejected — no calendar anchor | **6** |
| **Mixed IRIG + Standard** | Rejected, naming the first incompatible file | **6** |
| **A file whose own records step backward in time** (not internally sorted) | **Lenient**: one WARN, all records still emitted in heap order (never re-sorted). **Strict**: exit 1 (`NonMonotonicInput`). | 0 / 1 |

### Per-file failure with `--allow-partial` (`L2-MRG-004`)

If one input in a merge fails, the batch normally aborts. With `--allow-partial`
the bad input is **dropped with a WARN, the merge completes from the rest, and
the combined output is written as `.partial`, exit 0** — *regardless of where the
failure is detected:*

| Where the input fails | Example | With `--allow-partial` |
|---|---|---|
| **At open** | empty / unreadable / missing file | dropped → `.partial`, exit 0 |
| **At priming** (its first record) | non-MIE / all-0xFF first record | dropped → `.partial`, exit 0 |
| **Mid-file** | unrecoverable sync loss part-way | truncated there → `.partial`, exit 0 |

Without `--allow-partial`, any of these fails the batch (the exit code matches the
underlying failure). This uniform open/priming/mid-file handling is pinned by an
oracle (`merge-allow-partial-priming`) so both implementations behave identically.
> Note: incompatible inputs (Standard / freerun / mixed, exit 6) are a *different*
> class — `--allow-partial` does **not** apply to them; the merge is rejected
> outright.

### Collapsing duplicate recorders (`--collapse-duplicates`, `L2-MRG-007`)

When several recorders witness the same bus transaction, the merge would emit one
row per recorder. `--collapse-duplicates` (off by default — the default never
drops a row) folds those cross-recorder copies into one. A "duplicate" is the
same *wire content* (Type / Command / Status Words, error word, data words — not
the timestamp) from a **different** input file within `--collapse-window-us`
microseconds (default 0 = exact-µs match; widen for recorders whose clocks
differ). Same-file repeats and single-file decodes are never collapsed. The
window uses absolute time distance, so a non-monotonic input neither faults nor
over-collapses. See [`USER-GUIDE.md`](USER-GUIDE.md) for worked examples.

---

## 9. Output-mode scenarios

| Mode | How to get it | What you get |
|---|---|---|
| **Separate** (default) | (nothing) | Clean rows → main CSV; error + spurious rows → `<stem>_errors.csv` (created only if such rows exist). Matches vendor layout. |
| **Inline** | `--inline-errors` | One CSV with the `ERROR` / `ERROR_CODE` columns populated. |
| **Stdout** | `-o -` or piping | Streams to stdout; forces inline mode (a stream can't be split). A consumer that closes early (`… \| head`) is fine — exit 0. |
| **Count** | `count` subcommand | Just the integer message count on stdout; no CSV. |
| **Dump** | `dump` subcommand | A hex view of the bytes for debugging; no CSV. |

Output is written atomically (via a temp file renamed into place), so a failed
run never leaves a half-written CSV. `--no-clobber` refuses to overwrite an
existing output (exit 1).

---

## 10. Filter & MUX scenarios

**Filters** keep or drop rows *after* decoding (filtered rows simply don't appear
and aren't counted):

- `--exclude-types/-rts/-buses/-subaddresses` drop matching records.
- `--include-types/-rts/-buses/-subaddresses` keep *only* matching records.
- Values are comma-separated and the flag repeats to accumulate
  (`--include-rts 15,20 --include-rts 31`).

**MUX** fills the `MUX` column from a field of each input's file name (default on,
`L2-WRT-020`) — handy when file names encode the source recorder. Each merged row
carries the MUX of the file it came from. Turn it off with `--no-mux` (or
`[mux] enabled = false`) for byte-exact vendor output. See
[`CONFIG-REFERENCE.md`](CONFIG-REFERENCE.md) for the keys.

---

## 11. Exit-code quick reference

| Code | Class | Meaning |
|---|---|---|
| **0** | success | Decoded cleanly, *or* recovered from sync loss, *or* wrote a `.partial` under `--allow-partial` |
| **1** | runtime / decode error | A strict-mode record error, a write failure, a clobber refusal, or an unreadable input (single file) |
| **2** | no records | Not an MIE file, homogeneous padding, or (strict) an ambiguous timestamp format |
| **3** | unrecoverable sync loss | Mid-file corruption with no `--allow-partial` (no output written) |
| **4** | usage error | Bad command line (unknown flag, bad value, combined input methods, >256 files) |
| **5** | configuration error | Missing or invalid config file / key |
| **6** | merge-incompatible inputs | A merge whose inputs can't share an absolute IRIG timeline |

[`ERROR-CATALOG.md`](ERROR-CATALOG.md) is the authority for the exit codes, every
error class, and the `0x01xx` / `0x20xx` code tables.
