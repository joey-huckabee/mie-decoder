# MIE-Decoder Roadmap

| Version | Feature |
|---------|---------|
| **v1.1** | Sync recovery, error handling, config, filtering _(Python, current)_ |
| **v2.0** | Rust port. CLI redesign (`--inline-errors`, `--include-*` filters, `count` subcommand, `--format csv` forward-compat). Streaming CSV writer (constant memory). Static musl build for SLES 12. |
| v2.1 | Multi-file input, time-sorted merge to single CSV |
| v3.0 | Data word decoders, additional per-message-type CSVs |
| v4.0 | Apache Parquet output |

## Commitments carried through the Rust port

- **`config/default.toml` and TOML config support remain a first-class feature.** The Rust build ships a hand-rolled TOML loader for our config schema; the file format and key names are stable.
- **CSV column layout matches DDC vendor output byte-for-byte.** No reordering or renaming of columns, including currently-empty columns (`MUX`, `TERM_NAME`, `IM_GAP`, `RCV_GAP`, `XMT_GAP`).
- **Sync recovery semantics preserved.** Two-record look-ahead, 64 KB scan cap, error records and SPURIOUS_DATA continuations remain valid records that pass validation.
