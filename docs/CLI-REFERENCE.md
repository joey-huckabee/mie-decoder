# MIE-Decoder — CLI Reference

Complete reference for every command-line flag the decoder accepts. Use this when:

- You need the full flag surface for a subcommand, with defaults and value ranges.
- You're scripting `mie-decoder` and want the exact semantics of a flag.
- You're mapping a CLI flag to its `mie-decoder.toml` equivalent (or vice-versa).

The **Rust and Python builds expose an identical flag surface** — every flag below
works the same in both. The CLI's own `--help` (`mie-decoder <subcommand> --help`)
is generated from the same definitions and is always current; this document is the
prose companion that explains what each flag *does*.

For the TOML config keys these flags override, see
[`docs/CONFIG-REFERENCE.md`](CONFIG-REFERENCE.md). For exit codes and error
classes, see [`docs/ERROR-CATALOG.md`](ERROR-CATALOG.md). For the underlying
requirement IDs (`L1-*` / `L2-*`), see [`docs/L1-REQ.md`](L1-REQ.md) /
[`docs/L2-REQ.md`](L2-REQ.md). Task-oriented walkthroughs live in
[`docs/USER-GUIDE.md`](USER-GUIDE.md) and [`docs/EXAMPLES.md`](EXAMPLES.md).

Precedence for any setting that also has a config key: **CLI flag > config file >
built-in default**.

---

## Invocation

```
mie-decoder [global options] <subcommand> [subcommand options]
```

Subcommands: [`decode`](#decode) (binary → CSV), [`count`](#count) (print a record
count), [`dump`](#dump) (annotated hex dump).

### Global options

Global options are placed **before** the subcommand.

| Flag | Value | Default | Description |
|------|-------|---------|-------------|
| `--config PATH` | path | *(none)* | TOML configuration file. Applies to `decode`, `count`, and `dump` (the last consumes only its `[logging]` level). |
| `--log-level LEVEL` | `DEBUG` \| `INFO` \| `WARNING` \| `WARN` \| `ERROR` \| `CRITICAL` \| `OFF` | `WARNING` | Log verbosity (case-insensitive). Overrides the config file's `[logging] level`. Validated after `--version` / `--help`. |
| `-V`, `-v`, `--version` | — | — | Print the version and exit. Both short forms are accepted, and `--version` matches in any letter case (`--VERSION`, `--Version`, …). |
| `-h`, `--help` | — | — | Print help for the program or the given subcommand and exit. |

---

## decode

Decode one or more MIE binary files to CSV.

```
mie-decoder decode <INPUT>... [options]
```

### Input selection

Exactly one of these three methods supplies the inputs; they are **mutually
exclusive**. Passing more than one input (by any method) performs a time-sorted
multi-file merge — see [Merge](#merge-multi-file) below.

| Flag | Value | Default | Description |
|------|-------|---------|-------------|
| `INPUT...` (positional) | path(s) | — | One or more MIE recording files. More than one merges them. |
| `--manifest PATH` | path | *(none)* | Read input paths from a file, one per line; blank lines and `#`-comments are ignored. |
| `--glob PATTERN` | glob | *(none)* | Expand a single-directory glob (e.g. `dir/*.mie`); `*` and `?` match over the filename only (no recursion). |

> **`--glob` selects by *filename*, not by content.** Every file whose name
> matches is treated as a recording and decoded — the glob does no content or
> extension sniffing. If the pattern catches a non-recording (a `README.txt`, a
> log file, etc.), the run fails with **exit 2** (`no valid records`); in a
> multi-file merge that failure happens *before any output is written*, so one
> stray file loses the whole batch. Prefer a pattern that matches recordings
> only — `dir/*.mie` (or `dir/*.mie_irig` for the IRIG naming) — over a broad
> `dir/*`. The same applies to a `--manifest` that lists a non-recording path.
>
> If a mixed directory is unavoidable, `--allow-partial` makes a **merge** skip
> an undecodable input (WARN) and commit a `.partial` output instead of aborting
> — but it does not rescue a single-file glob, and it marks the output partial
> (signalling possible data loss), so a precise pattern is the better default.

### Output

| Flag | Value | Default | Description |
|------|-------|---------|-------------|
| `-o`, `--output PATH` | path | *(stdout)* | Output CSV file. If omitted, writes to stdout (which forces inline error mode — you cannot split stdout). |
| `--format FORMAT` | `csv` | `csv` | Output format. Only `csv` is supported at present. Overrides `[output] format`. |
| `--no-clobber` | flag | off | Refuse to overwrite an existing output file (`L2-WRT-017`). Mirrors `[output] no_clobber`. |
| `--inline-errors` | flag | off | Write errored/spurious messages inline in the main CSV with the `ERROR`/`ERROR_CODE` columns populated. Default (omitted): errors go to a separate `<output>_errors.csv`. Stdout output is always inline. |

### Timestamp format & detection

| Flag | Value | Default | Description |
|------|-------|---------|-------------|
| `--time-format FORMAT` | `auto` \| `irig` \| `standard` | `auto` | Timestamp format (case-insensitive). `auto` probes the recording; `irig` / `standard` force the choice. Overrides `[decode] time_format`. |
| `--standard-tick-rate-hz HZ` | float > 0 | *(unset)* | Standard-counter frequency in Hz. When set, Standard timestamps convert to microseconds and join DELTA tracking; unset leaves an empty `DELTA` for Standard records (`L2-DEC-017`). Mirrors `[decode] standard_tick_rate_hz`. |
| `--detect-records N` | int `1..=32` | `8` | Records the `auto` timestamp-format probe walks before committing to IRIG vs Standard (`L2-DEC-015`). Mirrors `[decode] detect_records`. |
| `--lookahead-records N` | int `1..=32` | `2` | Total records checked per sync validation (1 candidate + `N-1` look-ahead), used for header detection, continuous validation, and recovery (`L2-SYN-026`). Mirrors `[decode] lookahead_records`. |

### Error handling

| Flag | Value | Default | Description |
|------|-------|---------|-------------|
| `--strict` | flag | off (lenient) | Raise on invalid records instead of skipping them. Overrides `[decode] strict`. |
| `--allow-partial` | flag | off | On an unrecoverable mid-file sync loss, write `<output>.partial` and exit `0` instead of exit `3` (`L1-EXIT-004`). In a merge, a per-file failure is tolerated and the combined output is committed as `.partial`. Mirrors `[decode] allow_partial`. |

### MUX column

The `MUX` column is populated from a field of each input's **file name** by
default, so a decoded CSV can carry the source/recorder identity encoded in the
name (`L2-WRT-020`). For example, given
`full_loadout.draw.data.1553.aa.unused.mie_irig`, the default (`.` delimiter,
field index `4`) yields `MUX = aa`. In a merge, each row carries the MUX of the
file it was decoded from. An out-of-range field leaves MUX empty. To produce
output that matches the DDC vendor CSV byte-for-byte (empty MUX), use `--no-mux`.

| Flag | Value | Default | Description |
|------|-------|---------|-------------|
| `--no-mux` | flag | off (MUX on) | Leave the `MUX` column empty (vendor-exact). Mirrors `[mux] enabled = false`. |
| `--mux-delimiter D` | string | `.` | Field separator used to split the filename. Mirrors `[mux] delimiter`. |
| `--mux-field N` | int | `4` | 0-based field index into the split filename; negative counts from the end. Mirrors `[mux] field`. |

See [`docs/VENDOR-CSV-DIFFS.md`](VENDOR-CSV-DIFFS.md) for the vendor-exact
workflow.

### Filtering

Filter flags accept **one value**, which may be comma-separated, and are
**repeatable** (`--exclude-rts 15,31` is equivalent to `--exclude-rts 15
--exclude-rts 31`). The `--flag=VAL` form is equivalent to `--flag VAL`. The old
space-separated greedy form (`--include-rts 15 31 file.mie`) is **not** accepted —
it would consume `file.mie` as another value.

`exclude_*` flags **merge with** the config file's `[filter]` section;
`include_*` flags are **CLI-only** (no config key) and, when present, restrict
output to matching records (`L3-PY-013` / `L3-RS-010`).

| Flag | Value | Description |
|------|-------|-------------|
| `--exclude-types VAL` | names or hex | Exclude message types. Accepts names (`MODE_COMMAND`, `BC_TO_RT`, `RT_TO_BC`, `RT_TO_RT`, `BROADCAST_BC_TO_RT`, `BROADCAST_RT_TO_RT`, `SPURIOUS_DATA`) or hex codes (`0x01`, `0x02`, …). |
| `--exclude-rts VAL` | RT addresses | Exclude by RT address. |
| `--exclude-buses VAL` | `A` / `B` | Exclude by bus. |
| `--exclude-subaddresses VAL` | subaddresses | Exclude by subaddress. |
| `--include-types VAL` | names or hex | Include **only** these types (same syntax as `--exclude-types`). CLI-only. |
| `--include-rts VAL` | RT addresses | Include only these RT addresses. CLI-only. |
| `--include-buses VAL` | `A` / `B` | Include only these buses. CLI-only. |
| `--include-subaddresses VAL` | subaddresses | Include only these subaddresses. CLI-only. |

### Merge (multi-file)

Passing more than one input (positionals, `--manifest`, or `--glob`) merges the
recordings into a single time-sorted CSV, streaming in constant memory in the
record count. Records are ordered by absolute IRIG time, so **every input must be
calendar-locked IRIG** — a Standard-format, freerun, or mixed-format set is
rejected before any output with exit code `6`. Up to `256` files may be merged at
once. Per-file failures follow the same `--strict` / lenient / `--allow-partial`
policy as a single decode. Rust and Python produce byte-identical merged output.

| Flag | Value | Default | Description |
|------|-------|---------|-------------|
| `--collapse-duplicates` | flag | off | When several recorders on the same bus overlap, collapse each transaction's duplicate rows into one instead of inflating the count. Off by default, so the default never drops a row (multi-file merge only). Mirrors `[merge] collapse_duplicates`. |
| `--collapse-window-us N` | int µs | `0` | Timestamp tolerance for `--collapse-duplicates`, for recorders whose clocks differ slightly. `0` = exact match. Mirrors `[merge] collapse_window_us`. |

---

## count

Print the number of valid records, without producing a CSV.

```
mie-decoder count <INPUT>
```

| Argument | Value | Description |
|----------|-------|-------------|
| `INPUT` (positional) | path | The MIE recording file to count. |

Streams the file and prints the integer count to stdout, with a status summary on
stderr. Honors the config file's `[filter]` section (CLI filter flags are
decode-only) and the global `--config` / `--log-level`.

---

## dump

Hex dump of the binary, with optional per-record annotations. A diagnostic tool
for inspecting a corrupt or unusual file.

```
mie-decoder dump <INPUT> [options]
```

| Flag | Value | Default | Description |
|------|-------|---------|-------------|
| `INPUT` (positional) | path | — | The MIE file to dump. |
| `--raw` | flag | off | Raw hex dump with no record parsing. |
| `--offset N` | int (accepts `0x…`) | `0` | Start offset in bytes. |
| `--length N` | int (accepts `0x…`) | *(all)* | Number of bytes to dump (raw mode). |
| `--records N` | int | *(all)* | Maximum number of records to dump (record mode). |

`dump` consumes only the `[logging]` level from `--config`; the decode-time keys
(`time_format`, filters, `strict`, …) do not apply to a hex dump.

---

## Examples

```bash
# Decode to CSV
mie-decoder decode recording.mie -o decoded.csv

# Drop spurious + broadcast traffic
mie-decoder decode rec.mie -o clean.csv \
  --exclude-types SPURIOUS_DATA,BROADCAST_BC_TO_RT

# Only Bus A, only RT 15 (positive filters)
mie-decoder decode rec.mie -o rt15.csv --include-buses A --include-rts 15

# Errors inline with normal messages
mie-decoder decode rec.mie -o all.csv --inline-errors

# Force Standard timestamp format with a known counter rate (enables DELTA)
mie-decoder decode rec.mie -o decoded.csv \
  --time-format standard --standard-tick-rate-hz 1000000

# Multi-file, time-sorted merge; de-dup overlapping recorders
mie-decoder decode a.mie b.mie c.mie -o merged.csv --collapse-duplicates
mie-decoder decode --glob 'recordings/*.mie' -o merged.csv
mie-decoder decode --manifest files.txt -o merged.csv

# Custom / disabled MUX
mie-decoder decode rec.mie --mux-delimiter _ --mux-field 1 -o out.csv
mie-decoder decode rec.mie --no-mux -o out.csv          # vendor-exact empty MUX

# Count records; annotated hex dump
mie-decoder count recording.mie
mie-decoder dump recording.mie --records 10

# Debug logging (global flag precedes the subcommand)
mie-decoder --log-level DEBUG decode rec.mie -o decoded.csv
```
