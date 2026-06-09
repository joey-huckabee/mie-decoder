# MIE-Decoder

Decoder for DDC MIL-STD-1553 MIE binary recording files.

MIE-Decoder reads proprietary binary files produced by Data Device Corporation (DDC) MIL-STD-1553 PCI recording cards and outputs decoded messages in CSV format compatible with DDC's own recording software output.

MIE-Decoder is maintained in two implementations:

- **Rust v1.0.0** — streaming CSV writer (constant memory), hand-rolled
  CLI, single native release binary.
- **Python v1.0.0** — the Python package and CLI, maintained in
  [`python/`](python/).

Both implementations ship together as the joint **v1.0.0** release from a
single repository tag (`v1.0.0`). Future releases may diverge in version
via impl-prefixed tags (`rust-vX.Y.Z`, `python-vX.Y.Z`). The implementations
share the MIE format documentation, the vendor-compatible CSV behavior, and
a 20-case byte-exact cross-implementation conformance suite. See
[`CHANGELOG.md`](CHANGELOG.md) for the full v1.0.0 deliverables.

## Rust Build

```bash
cargo build --release
# binary lands at target/release/mie-decoder
```

The crate has exactly one external dependency (`memmap2`); everything else — argument parsing, CSV writing, TOML config, logging, error types — is hand-rolled.

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
mie-decoder decode <input> [options]

  -o, --output PATH                Output CSV (default stdout)
  --inline-errors                  Errors inline in main CSV
                                   (default: separate <stem>_errors.csv)
  --time-format auto|irig|standard Default auto
  --strict                         Raise on invalid records
  --format csv                     Output format (csv only at present)
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

### count

```
mie-decoder count <input>
```

Streams the file and prints the message count to stderr.

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
--log-level DEBUG|INFO|WARNING|ERROR  Default WARNING
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

# Debug logging
mie-decoder --log-level DEBUG decode rec.mie -o decoded.csv
```

## Library Usage

```rust
use mie_decoder::{
    filter::{FilterConfig, FilterIterExt},
    reader::MieFileReader,
    writer::write_csv,
};

let reader = MieFileReader::new("recording.mie")?;

// Basic iteration
for msg in reader.iter() {
    let msg = msg?;
    println!("{} RT{:?} {}", msg.timestamp.format(), msg.rt(), msg.msg_label());
}

// With filtering, streaming straight to CSV
let reader = MieFileReader::new("recording.mie")?;
let filters = FilterConfig {
    exclude_types: vec![0x20], // skip spurious
    ..Default::default()
};
let messages = reader.iter().filter_messages(filters);
write_csv(messages, Some(std::path::Path::new("decoded.csv")))?;
```

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
- **Unified validation**: The same five-check `validate_record` path is used for header skip, normal forward decode, and post-loss recovery. Every record passes through the same heuristics — there is no weaker fast path that could let corrupt-but-plausible records through.
- **Validation checks** (in order): valid message type → plausible word count → record fits in file → IRIG timestamp fields in range (hour < 24, minute < 60, second < 60) → two-record look-ahead (next record's Type Word also looks valid).
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

[filter]
exclude_types = ["SPURIOUS_DATA"]
exclude_rts = [31]
exclude_buses = []
exclude_subaddresses = []

[output]
format = "csv"
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
src/
├── lib.rs           Library entry point and re-exports
├── bin/mie-decoder  Binary entry point
├── cli.rs           Hand-rolled argparse + dispatch
├── config.rs        Hand-rolled TOML loader, DecoderConfig
├── decode.rs        Pure decoders + format classifier
├── dump.rs          Hex dump (raw + record-aware)
├── error.rs         MieError enum + Display/Error impls
├── filter.rs        FilterConfig + Iterator adapter
├── log.rs           Tiny stderr logger
├── models.rs        Data structures, enums, error codes
├── reader.rs        mmap-backed iterator with sync recovery
├── sync.rs          Pure validation + recovery helpers
└── writer.rs        Streaming CSV writer

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
└── integration.rs   End-to-end tests with byte-exact fixtures

python/              Maintained Python package and CLI
```

## Roadmap

See [docs/ROADMAP.md](docs/ROADMAP.md).

## Development

Rust:

```bash
cargo test                  # All unit + integration tests
cargo test --test integration -- multi_record_stream   # One integration test
cargo build --release       # Release binary at target/release/mie-decoder
cargo clippy --all-targets  # Lint (if installed)
```

Python:

```bash
poetry -C python sync
poetry -C python run pytest
poetry -C python run mie-decoder --help
```

Shared Rust/Python behavior:

```bash
python tests/conformance/run.py
```

## Known Limitations

- The Day field in IRIG timestamps may not decode correctly on all DDC card models.
- `MUX`, `TERM_NAME`, `IM_GAP`, `RCV_GAP`, `XMT_GAP` columns are present for format compatibility but empty in v1.0.0.
- Standard timestamp tick-to-microsecond conversion requires external calibration.
- SPURIOUS_DATA payload structure is raw words with no further interpretation.

## License

MIT
