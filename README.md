# MIE-Decoder

[![Quality Gate Status](https://sonarcloud.io/api/project_badges/measure?project=mie-decoder&metric=alert_status)](https://sonarcloud.io/summary/new_code?id=mie-decoder)

Decoder for DDC MIL-STD-1553 MIE binary recording files.

MIE-Decoder reads proprietary binary files produced by Data Device Corporation (DDC) MIL-STD-1553 PCI recording cards and outputs decoded messages in CSV format compatible with DDC's own recording software output.

MIE-Decoder is maintained in two implementations:

- **Rust** — streaming CSV writer (constant memory), hand-rolled CLI, single
  native release binary. See [`rust/README.md`](rust/README.md).
- **Python** — the Python package and CLI. See
  [`python/README.md`](python/README.md).

Both implementations ship together as a joint cut from a single
repository tag. Future releases may diverge via impl-prefixed tags
(`rust-vX.Y.Z`, `python-vX.Y.Z`). The implementations share the MIE format
documentation, the vendor-compatible CSV behavior, and a byte-exact
cross-implementation conformance suite (`tests/conformance/`). See
[`CHANGELOG.md`](CHANGELOG.md) for the release history.

## Building

Build, install, and library-usage instructions live with each implementation:

- **Rust** — [`rust/README.md`](rust/README.md): native release binary, crate /
  library API, `cargo` workflow.
- **Python** — [`python/README.md`](python/README.md): `mie-decoder` CLI plus the
  importable `mie_decoder` package, Poetry workflow.

The CLI surface, configuration schema, and CSV output documented below are
shared by both implementations.

## Quick Start

```bash
# Decode to CSV
mie-decoder decode recording.mie -o decoded.csv

# Count messages (no CSV output)
mie-decoder count recording.mie

# Hex dump with record annotations
mie-decoder dump recording.mie --records 10

# Decode with config file
mie-decoder --config config/default.toml decode recording.mie -o decoded.csv
```

## CLI Reference

### decode

```
mie-decoder decode <input>... [options]

  -o, --output PATH                Output CSV (default stdout)
  --manifest PATH                  Read input paths from a file (one per line;
                                   blank lines and #-comments ignored)
  --glob PATTERN                   Expand a single-directory *.mie-style glob
                                   (* and ? over the filename; no recursion)
  --inline-errors                  Errors inline in main CSV
                                   (default: separate <stem>_errors.csv)
  --time-format auto|irig|standard Default auto
  --standard-tick-rate-hz HZ       Standard-counter Hz; enables DELTA for
                                   Standard timestamps (default: unset)
  --strict                         Raise on invalid records
  --format csv                     Output format (csv only at present)
  --no-mux                         Leave the MUX column empty (vendor-exact);
                                   default: MUX is derived from the file name
  --mux-delimiter D                MUX field separator (default '.')
  --mux-field N                    0-based MUX field index; negative counts
                                   from the end (default 4)
  --exclude-types VAL              Comma-separated names or 0xNN hex codes
  --exclude-rts VAL                Comma-separated RT addresses
  --exclude-buses VAL              Comma-separated A|B
  --exclude-subaddresses VAL       Comma-separated subaddresses
  --include-types VAL              (same syntax as --exclude-types)
  --include-rts VAL
  --include-buses VAL
  --include-subaddresses VAL
```

Filter flags accept ONE value, which may be comma-separated. Repeating
the flag accumulates. The `--flag=VAL` form is equivalent to `--flag VAL`.

```bash
mie-decoder decode rec.mie --include-rts 15,20,31    # single flag, comma list
mie-decoder decode rec.mie --include-rts 15 --include-rts 31   # repeat
mie-decoder decode rec.mie --include-rts=15,31       # = form
```

The previous space-separated greedy form (`--include-rts 15 31 file.mie`)
is no longer accepted — it would consume `file.mie` as another RT
value.

#### Multi-file merge

Pass more than one input and `decode` merges them into a single
time-sorted CSV (streaming, constant memory in the record count):

```bash
mie-decoder decode a.mie b.mie c.mie -o merged.csv   # positionals
mie-decoder decode --manifest files.txt -o merged.csv  # one path per line
mie-decoder decode --glob 'recordings/*.mie' -o merged.csv  # tool-expanded
```

The three input methods are mutually exclusive. Records are ordered by
absolute IRIG time, so **every input must be calendar-locked IRIG** —
a Standard-format, freerun, or mixed-format set is rejected before any
output with exit code 6. A single input behaves exactly as before; up to
256 files may be merged at once. Per-file failures follow the same
`--strict` / lenient / `--allow-partial` policy as a single decode
(`--allow-partial` writes the combined `.partial` output). Rust and
Python produce byte-identical merged output.

#### MUX from the file name

The `MUX` column is filled from a field of each input's **file name**, so a
decoded CSV can carry the source/recorder identity encoded in the name. For
example, given `full_loadout.draw.data.1553.aa.unused.mie_irig`, the default
splits on `.` and takes field index `4` → `MUX = aa`:

```bash
mie-decoder decode full_loadout.draw.data.1553.aa.unused.mie_irig -o out.csv  # MUX=aa
mie-decoder decode rec.mie --mux-delimiter _ --mux-field 1 -o out.csv         # custom
mie-decoder decode rec.mie --no-mux -o out.csv                                # MUX empty
```

This is **on by default**. In a multi-file merge each row carries the MUX of
the file it was decoded from. A negative `--mux-field` counts from the end; an
out-of-range field leaves MUX empty. To produce output that matches the DDC
vendor CSV byte-for-byte (empty MUX), pass `--no-mux` (or set
`[mux] enabled = false`). See [`docs/VENDOR-CSV-DIFFS.md`](docs/VENDOR-CSV-DIFFS.md).

### count

```
mie-decoder count <input>
```

Streams the file and prints the message count to stdout (with a status summary on stderr).

### dump

```
mie-decoder dump <input> [options]

  --raw         Raw hex dump (no record parsing)
  --offset N    Start offset in bytes (supports 0xHEX)
  --length N    Bytes to dump (raw mode)
  --records N   Max records to dump (record mode)
```

### Global options

```
--log-level LEVEL                     DEBUG|INFO|WARNING|WARN|ERROR|CRITICAL|OFF
                                      (default WARNING; case-insensitive)
--config PATH                         TOML configuration file
-V, --version
-h, --help
```

### Examples

```bash
# Drop spurious + broadcast traffic
mie-decoder decode rec.mie -o clean.csv \
  --exclude-types SPURIOUS_DATA,BROADCAST_BC_TO_RT

# Only Bus A, only RT 15 (positive filters)
mie-decoder decode rec.mie -o rt15.csv \
  --include-buses A --include-rts 15

# Errors inline with normal messages
mie-decoder decode rec.mie -o all.csv --inline-errors

# Force Standard timestamp format
mie-decoder decode rec.mie -o decoded.csv --time-format standard

# Standard format with a known counter rate (enables the DELTA column)
mie-decoder decode rec.mie -o decoded.csv --time-format standard --standard-tick-rate-hz 1000000

# Debug logging
mie-decoder --log-level DEBUG decode rec.mie -o decoded.csv
```

Library usage (the Rust crate API and the Python `mie_decoder` package) is
documented in each implementation's README — [`rust/README.md`](rust/README.md)
and [`python/README.md`](python/README.md).

## Error Handling

When the DDC card detects an error mid-transaction (Manchester error, parity error, missing response), it writes a truncated record with bit 14 set in the Type Word and appends an Error Word containing the error code.

### Error modes

- **Default (separate)**: Normal messages go to the main CSV. Errored and spurious records go to `<output>_errors.csv`.
- **`--inline-errors`**: All messages in one CSV. `ERROR` column is `"ERROR"` or `"SPURIOUS"`; `ERROR_CODE` holds the code.

### Error codes

| Code | Source | Description |
|------|--------|-------------|
| 0x011E | DDC | Manchester/Parity Error or Bit Count Error |
| 0x0120 | DDC | No Status Response or Too Few Data Words |
| 0x0136 | DDC | Inverted Sync on Data Word |
| 0x0140 | DDC | Too Many Data Words |
| 0x0150 | DDC | Unknown DDC Error |
| 0x2000 | Decoder | Spurious Data: Continuation of preceding error |
| 0x2001 | Decoder | Spurious Data: Standalone (no preceding error) |

## Sync Recovery

MIE-Decoder automatically handles:

- **File headers**: Scans from offset 0 to find the first valid record, skipping proprietary headers.
- **Mid-file corruption**: If a record fails validation, scans forward in 2-byte steps to find the next valid record.
- **Unified validation**: The same validation rules are used for header skip, normal forward decode, and post-loss recovery. The additive detailed API reports a `ValidationFailure` reason while the existing boolean API remains compatible.
- **Validation checks** (in order): valid message type → plausible word count → record fits in file → IRIG timestamp fields in range → configurable N-record look-ahead.
- **DEBUG diagnostics**: Validation failures include one context hex line capped at 32 bytes.
- **Error records maintain sync**: Error records (bit 14) and SPURIOUS_DATA continuations are valid records with valid Type Words.

## Configuration

Copy `config/default.toml` and customize:

```toml
[logging]
level = "INFO"

[decode]
time_format = "auto"      # auto, irig, standard
strict = false
error_mode = "separate"   # separate, inline
# standard_tick_rate_hz = 1000000.0   # Standard counter Hz; enables DELTA (default: unset)

[filter]
exclude_types = ["SPURIOUS_DATA"]
exclude_rts = [31]
exclude_buses = []
exclude_subaddresses = []

[output]
format = "csv"

[mux]                       # MUX column from the file name (L2-WRT-020)
enabled = true             # false (or --no-mux) leaves MUX empty (vendor-exact)
delimiter = "."            # field separator applied to the basename
field = 4                  # 0-based field index (negative counts from the end)
```

CLI args override config file values; config file values override built-in defaults.

## Supported Message Formats

All 10 MIL-STD-1553 message formats plus SPURIOUS_DATA:

| Type Code | Format | Payload Layout |
|-----------|--------|----------------|
| 0x02 | Receive (BC→RT) | Cmd → Data(N) → Status |
| 0x04 | Transmit (RT→BC) | Cmd → Status → Data(N) |
| 0x08 | RT-to-RT | RxCmd → TxCmd → TxStatus → Data(N) → RxStatus |
| 0x10 | Broadcast Receive | Cmd → Data(N) |
| 0x18 | Broadcast RT-to-RT | RxCmd → TxCmd → TxStatus → Data(N) |
| 0x01 | Mode Code TX Data | ModeCmd → Status → DataWord |
| 0x01 | Mode Code RX Data | ModeCmd → DataWord → Status |
| 0x01 | Mode Code No Data | ModeCmd → Status |
| 0x01 | Mode Code Bcast No Data | ModeCmd |
| 0x01 | Mode Code Bcast Data | ModeCmd → DataWord |
| 0x20 | Spurious Data | Raw bus words (no command structure) |

## Project Structure

```
rust/                Rust crate (single dependency: memmap2) — see rust/README.md

config/
└── default.toml     Fully commented reference configuration

docs/
├── ARCHITECTURE.md     Module diagram, sync strategy, data flow
├── CONFIG-REFERENCE.md Normative TOML key reference (type / default / CLI override)
├── ERROR-CATALOG.md    Operator reference: exit codes, error classes, DDC codes
├── EXAMPLES.md         Runnable cookbook of common operator tasks
├── L1-REQ.md           Level 1 SHALL statements (system requirements)
├── L2-REQ.md           Level 2 architectural derivations
├── L3-REQ.md           Level 3 implementation obligations (incl. PY/RS)
├── MAINTAINER-GUIDE.md Repo layout, dev setup, workflows for adding things
├── MIE-FORMAT.md       Comprehensive binary format + CSV column reference
├── USER-GUIDE.md       End-to-end CLI walkthrough for analysts and operators
├── VENDOR-CSV-DIFFS.md Alignment statement vs DDC vendor CSV (column-by-column)
├── TRACE-MATRIX.md     Auto-generated trace matrix (L1 -> L2 -> L3 -> tests)
├── ROADMAP.md          Versioned roadmap
└── diagrams/           PlantUML sources and rendered SVGs

tests/
└── conformance/     Cross-implementation suite (Rust ↔ Python oracle)

python/              Python package and CLI — see python/README.md
```

## Roadmap

See [docs/ROADMAP.md](docs/ROADMAP.md).

## Development

Per-implementation development commands (build, test, lint, coverage) live in
[`rust/README.md`](rust/README.md) and [`python/README.md`](python/README.md);
[`CONTRIBUTING.md`](CONTRIBUTING.md) and
[`docs/MAINTAINER-GUIDE.md`](docs/MAINTAINER-GUIDE.md) cover the full workflow.

Shared Rust/Python conformance suite:

```bash
python tests/conformance/run.py
```

## Known Limitations

- The Day field in IRIG timestamps may not decode correctly on all DDC card models.
- `TERM_NAME`, `IM_GAP`, `RCV_GAP`, `XMT_GAP` columns are present for format compatibility but empty (by spec). `MUX` is populated from the input file name by default (L2-WRT-020); pass `--no-mux` for vendor-exact (empty) output.
- Standard timestamp tick-to-microsecond conversion requires external calibration.
- SPURIOUS_DATA payload structure is raw words with no further interpretation.

## License

MIT
