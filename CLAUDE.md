# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

MIE-Decoder contains actively maintained Rust and Python libraries + CLIs that
decode proprietary binary recording files produced by Data Device Corporation
(DDC) MIL-STD-1553 PCI cards. CSV output is column-compatible with DDC's own
recording software so a decoded file can be diffed against vendor output for
validation.

The Rust v1.0.0 implementation lives at the repository root. The Python v1.1.0
implementation lives at `python/`. The Rust implementation was a clean rewrite,
not a transliteration: its CLI was redesigned, its writer is streaming
(constant memory), and its data-words container is an inline `[u16; 32]`
buffer. Maintain each implementation according to its own architecture while
keeping shared format and CSV behavior aligned.

Edition 2024, MSRV 1.85. The crate has exactly one external dependency: `memmap2`. Argument parsing, CSV writing, TOML config, logging, and error types are all hand-rolled — preserve this property when adding features.

## Common Commands

```bash
# Build
cargo build               # Dev build
cargo build --release     # Optimized

# Static musl build for SLES 12 deployment (the production target)
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl

# Test
cargo test                                                     # All tests
cargo test --lib                                               # Unit tests only
cargo test --test integration                                  # Integration only
cargo test --test integration -- multi_record_stream           # Single integration test
cargo test config::tests::parses_default_toml_from_disk        # Single unit test

# Lint
cargo clippy --all-targets -- -D warnings

# Run the CLI
cargo run --release -- decode path/to/recording.mie -o decoded.csv
cargo run --release -- count path/to/recording.mie
cargo run --release -- dump path/to/recording.mie --records 10

# Python setup, test, and CLI
poetry -C python sync
poetry -C python run pytest
poetry -C python run mie-decoder --help
poetry -P python build

# Shared Rust/Python behavior
python tests/conformance/run.py
```

## Architecture

The decoder is a unidirectional pipeline. The big picture is best understood by tracing one record from disk to CSV:

1. **`reader.rs` — `MieFileReader`**: Top-level mmap-backed iterator. Calls `find_first_record()` for header skip, then loops: `validate_record()` → decode → yield `Result<MieMessage>`. On validation failure it calls `recover_sync()` to walk forward in 2-byte steps. Owns the `prev_was_error` flag used to classify SPURIOUS_DATA continuations.
2. **`sync.rs`**: Pure validation helpers (`find_first_record`, `validate_record`, `recover_sync`). Validation uses a **two-record look-ahead** — a candidate is only confirmed valid if the *next* record's Type Word also looks valid. This is critical: a single Type Word match alone produces too many false positives. No logging in this module — the reader emits any messages.
3. **`decode.rs`**: Pure binary → struct conversion. Type Word bit layout, IRIG vs Standard timestamp formats (auto-detected by probing the Command Word at both candidate offsets and scoring), Command Word, message format classification.
4. **`models.rs`**: Plain structs (`MieMessage`, `TypeWord`, `CommandWord`, `IrigTimestamp`, `StandardTimestamp`), `IntEnum`-style enums with explicit `#[repr(u8)]` discriminants, DDC error code constants (0x01xx) and decoder-assigned spurious codes (0x20xx). `DataWords` is the fixed-capacity inline buffer that replaces `Vec<u16>` for the per-record payload.
5. **`filter.rs` — `FilterIterExt::filter_messages`**: Iterator adapter. Both `exclude_*` and `include_*` filters are supported (the include set is the v2 redesign).
6. **`writer.rs`**: `write_csv` (single file) and `write_csv_split` (separate `_errors.csv`). Streams rows through a `BufWriter` — no DataFrame buffering. Column names and ordering match DDC vendor CSV byte-for-byte.
7. **`config.rs`**: Hand-rolled TOML loader for our schema (sections + key=value with strings/ints/bools/primitive arrays). Produces `DecoderConfig`. Precedence: **CLI overrides > config file > defaults**, applied via `DecoderConfig::with_overrides(ConfigOverrides)`.
8. **`cli.rs` — `run(argv)`**: Hand-rolled argparse with three subcommands (`decode`, `count`, `dump`). `count` is its own subcommand in v2 (was `--count` flag in v1). Default error mode is `separate` (was an explicit `--error-mode` flag); `--inline-errors` toggles inline mode.
9. **`log.rs`**: Tiny stderr logger. Single `AtomicU8` for the global level + `log_debug!`/`log_info!`/`log_warn!`/`log_error!` macros that format only when the level passes.

### Error handling model (important and non-obvious)

When the DDC card detects a bus error, it sets **bit 14 of the Type Word**, truncates the payload, and appends a 16-bit Error Word containing the code. If words remain from the original transaction, the card writes them as a separate `SPURIOUS_DATA` (type `0x20`) record immediately after.

The reader tracks `prev_was_error` across records so it can classify a following `SPURIOUS_DATA` as either `0x2000` (continuation of a preceding error) or `0x2001` (standalone). These `0x20xx` codes are decoder-assigned, not DDC hardware codes — see `models.rs` for the full code table.

Error records and SPURIOUS_DATA continuations are **valid records** that pass sync validation normally. Sync loss only happens on truly corrupt data (truncated mid-word, power loss). Don't conflate "errored record" with "sync loss."

### Output modes

- Default (`error_mode = separate`): clean messages → main CSV, errored + spurious → `<stem>_errors<suffix>` (lazy — file isn't created if no error rows). Calls `write_csv_split`.
- `--inline-errors`: everything → one CSV with `ERROR` and `ERROR_CODE` columns populated. Calls `write_csv`. Stdout output forces inline mode (you can't split stdout).

### Error type

All fallible APIs return `Result<T, MieError>`. `MieError` is a single enum (not a hierarchy). `kind()` returns a `MieErrorKind` discriminant. The `is_file_error()` / `is_record_error()` predicates approximate the two intermediate classes from the Python implementation.

## Reference docs

- `docs/ARCHITECTURE.md` — module diagram, four-phase sync strategy, error pipeline, configuration hierarchy, error type, logging levels. Read this when changing the reader/sync code.
- `docs/ERROR-CATALOG.md` — operator-facing reference for every CLI exit code, error class, DDC error code (`0x01xx`), and decoder-assigned code (`0x20xx`). Updated when error variants are added or removed.
- `docs/FIELDS.md` — complete binary field and CSV column reference (still accurate from the Python implementation).
- `docs/L1-REQ.md` — Level 1 SHALL statements (24 system requirements in 12 categories + NR-001 out-of-scope).
- `docs/L2-REQ.md` — Level 2 architectural derivations (102 requirements, each with a single L1 parent).
- `docs/L3-REQ.md` — Level 3 implementation obligations (26 requirements: 2 cross-impl L3-WRT-*, 12 L3-PY-*, 12 L3-RS-*).
- `docs/TRACE-MATRIX.md` — auto-generated trace matrix produced by `scripts/build-trace-matrix.py`. Forward trace from L1 through L2 and L3 to test artifacts (`@pytest.mark.requirement` markers in `python/tests/` and `/// Requirements:` doc-comments above Rust `#[test]` items). Treat as the single source of truth for live status; the source docs hold spec content only.
- `docs/ROADMAP.md` — versioned roadmap with explicit "do not drop" commitments (TOML config, CSV byte-compat, sync semantics).
- `config/default.toml` — fully commented reference configuration; preserved across the port.
- `python/` — maintained Python package and CLI with its own source and tests.
- `tests/conformance/` — shared hexadecimal fixtures and byte-exact CSV
  oracles exercised against both implementations.

## Conventions worth preserving

- **Single external dependency.** Only `memmap2`. Adding crates requires justification — argument parsing, CSV, TOML, logging, error types are all hand-rolled by design and the user values keeping it that way.
- **Production target is `x86_64-unknown-linux-musl`** (statically linked, runs on SLES 12 / glibc 2.22). Native dev builds on Windows/macOS are fine. Don't add anything that breaks the musl target.
- **Streaming CSV.** Rows must flow through a `Write` impl as they are produced. Do not introduce `Vec<MieMessage>` or `Vec<Row>` buffering in the writer — constant memory is the design point.
- **Two-record look-ahead in `sync.rs`.** Don't remove it. Removing the look-ahead reintroduces false-positive resyncs.
- **`DataWords` is fixed-capacity by design.** MIL-STD-1553B caps a single transaction at 32 data words. Don't switch to `Vec<u16>` "for flexibility."
- **CSV column names and order are dictated by DDC vendor output.** Don't "clean up" `MUX`, `TERM_NAME`, `IM_GAP`, `RCV_GAP`, `XMT_GAP` even though they're empty in v1.0.0 — they're columns by spec.
- **`sync.rs` is pure** (no logging, no I/O). The reader handles any user-facing messaging based on returned values. Don't move logging into validation helpers.
- **Shared conformance fixtures are byte-exact.** Treat
  `tests/conformance/` as the cross-implementation oracle; update expected CSV
  only after both implementations agree.
- **Both implementations are maintained.** Keep Rust-specific design decisions
  in the Rust crate and Python-specific design decisions in `python/`; align
  shared format semantics and vendor-compatible CSV behavior.

## Git conventions

Do **not** add `Co-Authored-By: Claude ...` trailers to commit messages on this repo, even if the harness's default instructions suggest it. Commit messages are the human-authored record of intent; tool attribution belongs in tool logs, not history. This overrides the default trailer behavior.
