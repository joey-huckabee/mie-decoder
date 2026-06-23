# MIE-Decoder — User Guide

End-to-end walkthrough for analysts and operators who need to turn a DDC MIE binary recording into CSV. Covers:

- Picking an implementation and installing it.
- Decoding your first file.
- The three CLI subcommands and when to use each.
- The common workflows: stdout piping, error separation, partial decoding, filtering, site-wide config.
- Reading the CSV output.
- Diagnosing failures.

If you're modifying the code, see [`MAINTAINER-GUIDE.md`](MAINTAINER-GUIDE.md). For the full TOML schema, see [`CONFIG-REFERENCE.md`](CONFIG-REFERENCE.md). For every CLI exit code and error class, see [`ERROR-CATALOG.md`](ERROR-CATALOG.md).

---

## 1. What this is

MIE-Decoder reads proprietary binary recording files produced by Data Device Corporation (DDC) MIL-STD-1553 PCI cards and emits CSV output. The CSV layout is column-compatible with DDC's own recording software, so you can:

- Open the CSV in Excel, pandas, or any tooling that consumes flat tabular data.
- `diff` decoded output against vendor-generated CSV for validation.
- Feed it into a downstream analysis pipeline.

The decoder is shipped as two interoperable implementations — a Rust crate + CLI, and a Python package + CLI. Both produce byte-identical CSV for the same input (verified by a cross-implementation conformance suite). Pick whichever fits your platform.

---

## 2. Pick an implementation

| Implementation | Use when |
|----------------|----------|
| **Rust** | You want a native compiled binary, a dependency-free static install, or the fastest decode throughput. |
| **Python** | You want to drop into an existing Python analysis pipeline, you're on Windows / macOS for ad-hoc work, or you'd rather `pip install` than build from source. |

Both decode in constant memory and handle multi-GB / 10M+-record recordings — the choice is about ecosystem, not file size (see [§10 Performance and large recordings](#10-performance-and-large-recordings)).

CSV output is byte-identical between the two — your choice doesn't change the result.

---

## 3. Install

### Rust binary

If a prebuilt binary is available for your platform, just download and run it. To build from source:

```bash
git clone <repo-url>
cd mie-decoder
cargo build --release
./target/release/mie-decoder --help
```

### Python package

Install from a source checkout:

```bash
pip install -e ./python
mie-decoder --help
```

If you prefer Poetry:

```bash
poetry -C python sync
poetry -C python run mie-decoder --help
```

The Python package supports Python 3.10 through 3.14.

---

## 4. Decode your first file

The minimum command:

```bash
mie-decoder decode flight.mie -o flight.csv
```

That's it. The decoder finds the MIE record stream (skipping any proprietary file header), auto-detects whether the recording uses IRIG-B or Standard timestamps, decodes every record into one CSV row, and writes the output atomically (so a crash or kill mid-run leaves no half-written file).

On success the CLI exits 0 with no output to stderr. If you want a one-line summary, add `--log-level INFO`:

```bash
$ mie-decoder --log-level INFO decode flight.mie -o flight.csv
INFO  beginning decode of flight.mie
INFO  auto-detected timestamp format: Irig
INFO  decode complete: 14523 messages, 0 sync recoveries, format=Irig
INFO  decode exit class: complete (sync_losses=0)
```

The `decode exit class:` line is always emitted at INFO; it names one of `complete`, `partial-recovered`, `partial-unrecoverable`, or `no-records` so pipeline logs can grep for it.

---

## 5. The three subcommands

### `decode` — primary

Reads an MIE file and writes CSV. The command you'll use most.

```bash
mie-decoder decode flight.mie -o flight.csv
mie-decoder decode flight.mie > flight.csv    # stdout
mie-decoder decode flight.mie --inline-errors -o everything.csv
mie-decoder --config site.toml decode flight.mie -o flight.csv
```

### `count` — message count, no CSV output

Counts decodable records without producing CSV. Useful for sanity-checking a file size before a long decode or comparing two recordings.

Both implementations follow a two-channel output contract (L3-RS-008 / L3-PY-010): **stdout** contains only the integer count followed by a newline (so it pipes cleanly), and **stderr** carries a human-readable status line with the input path so an interactive operator still sees context.

```bash
$ mie-decoder count flight.mie
14523
# (stderr, always emitted: "counted 14523 messages in flight.mie")

$ n=$(mie-decoder count flight.mie); echo "got $n"
got 14523
```

Both CLIs share the `count` subcommand, meet the L1-CLI-001 message-counting capability, and produce identical stdout output.

### `dump` — diagnostic hex dump

Two modes for investigating files the decoder rejects or behaves oddly on:

```bash
# Record-aware: parses each record header + IRIG timestamp + Cmd Word, then hex
mie-decoder dump suspect.mie --records 10

# Raw hex: classic `hexdump -C` over any byte range
mie-decoder dump suspect.mie --raw --offset 0 --length 256
```

Record-aware mode is the default and what you want most of the time — it annotates each record with its Type Word, timestamp, Command Word, RT/SA/direction, and word count, then dumps the record's bytes. Raw mode is for the cases where validation rejects everything and you want to look at the literal bytes.

---

## 6. Common workflows

### Stream to stdout for pipelining

Omit `-o` to write to stdout. The decoder forces inline-error mode (you can't split stdout into two streams), and a broken pipe (downstream consumer closed) exits 0 with no error.

```bash
mie-decoder decode flight.mie | head -100
mie-decoder decode flight.mie | awk -F, '$2=="15"'   # only RT 15
```

### Separate vs inline error handling

By default, **errored records** (DDC card detected a bus error) and **SPURIOUS_DATA** records (orphan data fragments) are written to a *separate* file so the main CSV stays clean. The errors file is named `<output_stem>_errors<output_suffix>`:

```bash
$ mie-decoder decode flight.mie -o flight.csv
$ ls
flight.csv flight_errors.csv      # errors file only created if error rows exist
```

For diffing against vendor-generated CSV (which is always inline), or for any analysis pipeline that wants every row in one file, use inline mode:

```bash
mie-decoder decode flight.mie --inline-errors -o flight.csv
```

In inline mode the `ERROR` column contains `ERROR` / `SPURIOUS` / empty and `ERROR_CODE` contains the hardware code (`011E`, `0120`, etc.) or the decoder-assigned code (`2000` continuation, `2001` standalone). See `ERROR-CATALOG.md` sections 6 and 7 for the full code reference.

### Recovering data from a corrupt recording

If a recording has unrecoverable mid-file corruption, the default behavior is to exit 3 with no output (so you can't accidentally treat a partial result as complete). To preserve what was decoded before the corruption point:

```bash
mie-decoder decode corrupt.mie --allow-partial -o decoded.csv
```

On unrecoverable loss, instead of exit 3 and an unlinked temp file, you get:

- A `decoded.csv.partial` file containing all rows decoded before the loss.
- The main `decoded.csv` is **not** created (the `.partial` suffix is deliberate — downstream consumers shouldn't pick it up automatically).
- The CLI exits 0 with a WARN summary naming the sync-loss count.

Inspect `decoded.csv.partial` to see what was salvageable; investigate the source recording separately.

### Filtering messages

All four filter axes use exclude lists and OR logic — a message is dropped if it matches **any** configured criterion. CLI flags **add to** config-file filters; they don't replace them (L2-CFG-004).

```bash
# Drop all SPURIOUS_DATA records:
mie-decoder decode flight.mie --exclude-types SPURIOUS_DATA -o cleaned.csv

# Drop broadcast (RT 31) and the unused RT 0:
mie-decoder decode flight.mie --exclude-rts 0,31 -o cleaned.csv

# Only Bus A (exclude Bus B):
mie-decoder decode flight.mie --exclude-buses B -o busa.csv

# Drop mode-code subaddresses (SA 0 and SA 31):
mie-decoder decode flight.mie --exclude-subaddresses 0,31 -o nomodes.csv

# Combine — drop anything matching ANY criterion:
mie-decoder decode flight.mie \
    --exclude-types SPURIOUS_DATA,MODE_COMMAND \
    --exclude-rts 31 \
    -o filtered.csv
```

Type filter accepts both symbolic names (`SPURIOUS_DATA`, `BC_TO_RT`, etc.) and hex codes (`0x20`, `0x02`) interchangeably.

### Calibrating Standard timestamps

Some recordings use the **Standard** timestamp format — a 32-bit free-running counter — instead of IRIG. The counter ticks at a card-dependent rate that is **not stored in the file**, so the decoder cannot turn raw ticks into elapsed seconds on its own. By default, the `DELTA` column is therefore left empty for every Standard record:

```bash
mie-decoder decode counter.mie --time-format standard -o out.csv
# TIME_STAMP in 0xNNNNNNNN form; DELTA column empty for all rows
```

If you know your card's counter frequency, pass it with `--standard-tick-rate-hz` (in Hz). The decoder then converts ticks to microseconds and fills in `DELTA` just as it would for an IRIG recording:

```bash
# Card runs a 1 MHz counter (1 tick = 1 microsecond):
mie-decoder decode counter.mie --time-format standard --standard-tick-rate-hz 1000000 -o out.csv
```

With calibration on, two consecutive records of the same RT/MSG that are 16 ticks apart show `DELTA = 0.000016` at 1 MHz; the first occurrence of each RT/MSG key is still `0.000000`. The rate must be greater than 0, and it has no effect on IRIG recordings.

**Finding the rate.** The tick rate comes from the recording card's configuration (often documented in the card datasheet or your acquisition setup), not from the file. If you don't know it, leave the flag off — an empty `DELTA` is the honest answer, and the raw counter value is still shown in `TIME_STAMP`.

The same setting is available in a config file as `decode.standard_tick_rate_hz`:

```toml
[decode]
time_format = "standard"
standard_tick_rate_hz = 1000000.0
```

### Site-wide configuration

If you find yourself repeating the same flags across recordings, put them in a TOML file:

```toml
# /etc/mie-decoder/site.toml
[logging]
level = "INFO"

[decode]
error_mode = "inline"

[filter]
exclude_types = ["SPURIOUS_DATA"]
exclude_rts   = [31]
```

```bash
mie-decoder --config /etc/mie-decoder/site.toml decode flight.mie -o flight.csv
```

CLI arguments still take precedence over config-file values per L2-CFG-003. For the full TOML schema with every key documented, see [`CONFIG-REFERENCE.md`](CONFIG-REFERENCE.md). Copy [`config/default.toml`](../config/default.toml) as a fully-commented starting point.

### Merging multiple recordings into one timeline

When a session produced several recordings (e.g. one per recorder), `decode`
can merge them into a single CSV in global time order. Give it more than one
input, in any of three ways:

```bash
# 1. Positional paths (ad-hoc):
mie-decoder decode flight-1.mie flight-2.mie flight-3.mie -o session.csv

# 2. A manifest file (one path per line; blank lines and #-comments ignored):
mie-decoder decode --manifest session-files.txt -o session.csv

# 3. A glob the tool expands itself (works on Windows; * and ? over the
#    filename in one directory — no recursion):
mie-decoder decode --glob 'recordings/*.mie' -o session.csv
```

The Python CLI takes the exact same forms (`python -m mie_decoder decode …`
or the `mie-decoder` console script). The three methods are **mutually
exclusive** — pick one. A single input behaves exactly as a normal decode.

#### How the rows get ordered (it is not arbitrary)

Each input file comes from an independent recorder, and each recorder may log
any given message at a different rate across the timeline. The merge does **not**
pull a row from each file in turn, or interleave them blindly — every row it
writes is the one with the earliest timestamp out of *all* the files at that
moment. It works like this:

1. It opens all the files at once and looks at the **first (earliest) record of
   each** — so it always has the current front record of every recorder in
   front of it simultaneously.
2. It writes out the single record with the **smallest timestamp** among those
   fronts.
3. It then pulls the **next** record from *only the file that record came
   from*, and adds it to the set of fronts being compared.
4. It repeats until every file is used up.

So before any row is written, its timestamp has been compared against the
current front of every other file, and the earliest one wins. A recorder that
goes quiet for a stretch simply doesn't win any rows until its next record's
time comes due; a recorder logging at a high rate contributes many rows in a
row — both end up correctly interleaved by time. This works because each
individual recording is already written in chronological order (bus traffic is
captured as it happens), so the tool only has to merge already-sorted streams,
never re-sort the whole set — which is also why memory stays flat no matter how
many total records there are.

When two records carry the **exact same timestamp** (common at coarse time
resolution), the tie is broken deterministically by the file's **position in
the input list** (the order you gave the files on the command line / in the
manifest, or lexicographic filename order for `--glob`) and then by the
record's position within that file — so the output is reproducible run to run.

What to expect:

- **Ordering** is by absolute IRIG time, so **every input must be
  calendar-locked IRIG.** If any file is Standard-format, leads with a freerun
  record, or the set mixes formats, the merge refuses before writing anything
  and exits **6**, naming the offending file — it never emits a misleadingly
  "sorted" CSV. Decode such files individually instead.
- **Memory** stays flat regardless of total record count (it holds one record
  per open file), so merging many multi-GB recordings is fine. Up to **256**
  files per invocation; combining input methods or exceeding that is a usage
  error (exit 4).
- **DELTA** is computed across the merged timeline (one unified
  inter-arrival gap per RT/SA), not reset at file boundaries.
- **A bad file among many:** by default the batch fails. Add `--allow-partial`
  to skip/truncate the failing file with a warning, finish the merge from the
  rest, and write the combined result as `<output>.partial` (exit 0). Use
  `--strict` to fail on the first invalid record in any file.
- **Same year only:** IRIG carries day-of-year but no year, so a set spanning a
  New-Year boundary cannot be ordered from the timestamp alone.
- **A file that isn't internally time-sorted:** the merge assumes each input is
  in chronological capture order (true for normal recordings). If an input's own
  timestamps step backward — rare, from sync-loss recovery or a day/year
  rollover — the tool detects it and, by default (lenient), prints a one-time
  WARN naming that file and still merges everything (the out-of-order rows sort
  to their key positions; it never re-sorts a whole file). Under `--strict` (or
  `[decode] strict = true`) a backward step inside one file is treated as a
  record error and the merge fails (exit **1**), on the principle that a
  recorder's own clock running backward is corruption you probably want to know
  about before trusting the merge.

Both implementations produce byte-identical merged output.

### Labeling output by recorder (MUX from the file name)

If your recordings are named so a field identifies the source or recorder, the
decoder can copy that field into the `MUX` column. For the convention
`full_loadout.draw.data.1553.aa.unused.mie_irig` — where the 5th dot-separated
field (`aa`, `gun`, `s2`, …) is the recorder — the **defaults already do the
right thing**:

```bash
mie-decoder decode full_loadout.draw.data.1553.aa.unused.mie_irig -o out.csv
# every row's MUX column is "aa"
```

The default splits the file name on `.` and takes field index `4`. Adjust for a
different scheme with `--mux-delimiter` and `--mux-field` (a negative index
counts from the end), or set them in a config `[mux]` section. **In a merge**,
each row carries the MUX of the file it came from — so a merged CSV of several
recorders is self-labeling:

```bash
mie-decoder decode --glob 'flight/*.mie_irig' -o merged.csv
# rows from …aa…  → MUX "aa";  rows from …gun… → MUX "gun";  etc.
```

MUX population is **on by default**. To produce output that matches a DDC vendor
CSV byte-for-byte (empty MUX), turn it off with **`--no-mux`** (or
`[mux] enabled = false`). See [`VENDOR-CSV-DIFFS.md`](VENDOR-CSV-DIFFS.md).

---

## 7. Reading the CSV

The column layout matches DDC vendor output byte-for-byte. Columns in order:

| Column | Contents |
|--------|----------|
| `TIME_STAMP` | IRIG: `DAY:HH:MM:SS.uuuuuu` (e.g. `192:15:54:50.456225`). Standard: `0xNNNNNNNN` raw counter. |
| `RT` | Remote Terminal address (0–31), or empty for SPURIOUS_DATA. |
| `MSG` | `<subaddress><T\|R>` (e.g. `11R` for SA 11 Receive, `22T` for SA 22 Transmit). Empty for SPURIOUS_DATA. |
| `WD01`–`WD32` | Up to 32 data words, 4-character uppercase hex without `0x` prefix. Unused trailing columns are empty (not `0000`). |
| `STAT` | Status Word, 4-character uppercase hex. Empty when not present (e.g. some Mode Code formats). |
| `CMD` | Command Word, 4-character uppercase hex. Empty for SPURIOUS_DATA. |
| `MUX` | Source/recorder label derived from the input **file name** by default (see [Labeling output by recorder](#labeling-output-by-recorder-mux-from-the-file-name)). Empty with `--no-mux`. |
| `TERM_NAME` | Vendor compatibility column. Always empty; reserved for future per-card metadata. |
| `BUS` | `A` or `B`. |
| `DELTA` | Seconds since the previous message on the same RT/MSG key. `0.000000` on first occurrence. Empty when the timestamp basis is unknown (uncalibrated Standard format — see [Calibrating Standard timestamps](#calibrating-standard-timestamps)), the record is SPURIOUS_DATA, or the timestamp is non-monotonic. |
| `ERROR` | `ERROR`, `SPURIOUS`, or empty. Empty in clean rows of separate-mode CSV. |
| `ERROR_CODE` | DDC hardware code (`011E`, `0120`, `0136`, `0140`, `0150`) or decoder-assigned code (`2000`, `2001`). Empty in clean rows of separate-mode CSV. |
| `IM_GAP`, `RCV_GAP`, `XMT_GAP` | Vendor compatibility columns. Always empty in v1; reserved for future inter-message gap timing. |

A typical receive row looks like:

```
192:15:54:50.456225,15,11R,0400,,,002F,CA22,...,7800,797E,,,A,0.000000,,,,,
```

Line endings are LF (`\n`) on every platform — including Windows — so the CSV diffs cleanly between machines (L2-WRT-012).

For the binary-level field reference (what's in the Type Word, how IRIG packing works, etc.), see [`MIE-FORMAT.md`](MIE-FORMAT.md).

---

## 8. When something goes wrong

The CLI exits with one of six codes (L1-EXIT-001 through L1-EXIT-008), identical across the Rust and Python implementations:

| Code | Class | Likely cause |
|------|-------|--------------|
| **0** | `complete` / `partial-recovered` | Decoded successfully (possibly after auto-recovery from in-stream corruption). |
| **0** | `complete (broken-pipe)` | stdout consumer closed early. Not an error. |
| **1** | runtime / decode error | Per-record validation failed in strict mode, the input couldn't be opened, or the output sink failed. Read the stderr error line. |
| **2** | `no-records` | The input file isn't an MIE recording at all (wrong file type, single-byte pad). No output file created. |
| **3** | `partial-unrecoverable` | Mid-file sync loss that couldn't be recovered. Re-run with `--allow-partial` to keep what was decoded. |
| **4** | usage error | The command line is wrong — unknown/invalid flag or argument, bad flag value, or no subcommand. Run `--help`. |
| **5** | configuration error | The `--config` TOML file can't be found, parsed, or fails validation. Fix the file named in the error. |

The `decode exit class:` summary log line names the class explicitly, even when stderr is captured to a pipeline log.

### Common diagnoses

**"No valid records found in flight.mie (scanned first 65536 bytes)"** (exit 2): The file isn't actually MIE, or the MIE records begin past the 64 KB header scan window. Use `mie-decoder dump flight.mie --raw --length 256` to see what the file actually starts with.

**"Pathological homogeneous-payload input rejected"** (exit 2): The file is a single-byte pad (e.g. zero-fill, 0x20-fill from a botched recording transfer). Re-export from the source.

**"Unrecoverable mid-file sync loss at offset 0x... after N recovery attempts"** (exit 3): The recording has corruption the decoder can't skip past. Re-run with `--allow-partial` to inspect what was decoded before the loss; investigate the source recording for storage / transmission issues.

**"First record after header detection is truncated"** (exit 1 in strict mode): The first valid Type Word's declared extent runs past EOF. Usually means the recording was aborted before the first complete record was written. Lenient mode terminates cleanly with zero records (and exits 0); strict mode raises so it's visible.

**WARN lines like `non-monotonic timestamp at 0x...` or `L2-SYN anomaly at 0x...`**: These don't fail the decode — they're observations about the recording. The first means a record's timestamp went backwards on the same RT/MSG key (DELTA is left empty for that row); the second means a Status Word RT didn't match its Command Word RT (possible bus interference). If you see high rates of either, investigate the recording source.

For the full error catalog with every variant, exit code, and "what to do" guidance, see [`ERROR-CATALOG.md`](ERROR-CATALOG.md).

---

## 9. Configuration — quick overview

Most workflows don't need a config file — the defaults are sensible and CLI flags cover the common overrides. But for repeated runs against many files, a config file removes the per-invocation noise.

Minimum-viable config:

```toml
[decode]
error_mode = "inline"     # everything in one CSV

[logging]
level = "INFO"            # see the exit-class summary in stderr
```

```bash
mie-decoder --config my.toml decode flight.mie -o flight.csv
```

CLI arguments override matching config keys (L2-CFG-003). Filter arrays are the one exception — CLI values **add to** config values rather than replacing them (L2-CFG-004), so a site-wide `exclude_types = ["SPURIOUS_DATA"]` plus a CLI `--exclude-rts 31` yields both filters active.

For every accepted key, its type, default, validation behavior, and CLI override, see [`CONFIG-REFERENCE.md`](CONFIG-REFERENCE.md).

---

## 10. Performance and large recordings

Both implementations produce byte-identical CSV, decode at broadly similar speed, and decode in **constant memory** — rows stream straight to the output, so a 10 GB recording uses the same memory as a 10 MB one.

| Implementation | Memory while decoding | Practical ceiling |
|----------------|-----------------------|-------------------|
| **Rust** | Constant — `O(1)` in the record count. | Bounded by disk, not RAM. |
| **Python** | Constant — `O(1)` in the record count. Each row streams to the output through the standard-library `csv` module; nothing accumulates across records. | Bounded by disk, not RAM. |

**Rule of thumb:** either CLI handles multi-GB recordings and **10M+ record** files without memory becoming a concern — the output is identical. Choose by ecosystem: the Rust CLI for a dependency-free static binary, the Python CLI to stay inside a Python pipeline.

The constant-memory guarantee is tracked as `L3-PY-012` (Python) / `L3-RS-012` (Rust) and is load-bearing: a change that buffers rows would regress it. See [`ARCHITECTURE.md`](ARCHITECTURE.md) §12 (memory profile) and §14 (operational limits) for the underlying detail.

---

## 11. What's next

- **Hit a column you don't recognize?** [`MIE-FORMAT.md`](MIE-FORMAT.md) is the per-column reference (binary layout + CSV format).
- **Hit an exit code or error message you don't recognize?** [`ERROR-CATALOG.md`](ERROR-CATALOG.md) covers every variant with operator guidance.
- **Setting up site or campaign config?** [`CONFIG-REFERENCE.md`](CONFIG-REFERENCE.md) is the normative TOML schema.
- **Modifying the decoder itself?** [`MAINTAINER-GUIDE.md`](MAINTAINER-GUIDE.md) covers the development workflows.
- **Curious how decoding works under the hood?** [`ARCHITECTURE.md`](ARCHITECTURE.md) walks the reader/sync/writer pipeline.
- **Need to know what the spec says?** [`L1-REQ.md`](L1-REQ.md), [`L2-REQ.md`](L2-REQ.md), [`L3-REQ.md`](L3-REQ.md), and [`TRACE-MATRIX.md`](TRACE-MATRIX.md).
