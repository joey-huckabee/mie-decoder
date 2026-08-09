# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

MIE-Decoder contains actively maintained Rust and Python libraries + CLIs that
decode proprietary binary recording files produced by Data Device Corporation
(DDC) MIL-STD-1553 PCI cards. CSV output is column-compatible with DDC's own
recording software so a decoded file can be diffed against vendor output for
validation.

Both implementations ship together as a joint cut from a single
repository tag; future releases may diverge via impl-prefixed tags
(`rust-vX.Y.Z`, `python-vX.Y.Z`). The Rust
implementation lives under `rust/`; the Python
implementation lives under `python/`. See `CHANGELOG.md` for the release
history and `git tag` for the current version. The Rust implementation was a clean rewrite,
not a transliteration: its CLI was redesigned, its writer is streaming
(constant memory), and its data-words container is an inline `[u16; 32]`
buffer. Maintain each implementation according to its own architecture while
keeping shared format and CSV behavior aligned.

Edition 2024, MSRV 1.88 (`memmap2` requires ≥1.88; edition 2024 itself only floors at 1.85). The crate has exactly one external dependency: `memmap2`. Argument parsing, CSV writing, TOML config, logging, and error types are all hand-rolled — preserve this property when adding features.

## Common Commands

```bash
# Build (Rust commands run from the rust/ crate directory)
cd rust
cargo build               # Dev build
cargo build --release     # Optimized

# Test
cargo test                                                     # All tests
cargo test --lib                                               # Unit tests only
cargo test --test integration                                  # Integration only (library API)
cargo test --test cli                                          # CLI acceptance only (spawns built binary)
cargo test --test cli -- --nocapture                           # CLI suite + show subprocess stderr
cargo test --test integration -- multi_record_stream           # Single integration test
cargo test config::tests::parses_default_toml_from_disk        # Single unit test

# Lint
cargo clippy --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps    # doc-link check (CI-gated)
cargo +1.88 check --all-targets                   # MSRV 1.88 floor (CI-gated)
cargo deny check                                  # supply-chain audit: advisories/licenses (CI-gated)
cargo semver-checks check-release --baseline-rev "$(git describe --tags --abbrev=0)" --release-type minor  # public-API break check (CI-gated)

# Run the CLI
cargo run --release -- decode path/to/recording.mie -o decoded.csv
cargo run --release -- count path/to/recording.mie
cargo run --release -- dump path/to/recording.mie --records 10

# Python setup, test, and CLI (run from the repo root)
cd ..
poetry -C python sync
poetry -C python run pytest
poetry -C python run mypy src    # strict type check (CI-gated)
poetry -C python run pylint src/mie_decoder    # lint (CI-gated, must stay 10/10)
poetry -C python run ruff check                # ruff lint (CI-gated)
poetry -C python run ruff format               # auto-format (CI runs ruff format --check)
poetry -C python run vulture                   # dead-code scan (CI-gated)
poetry -C python run bandit -r src/mie_decoder # security scan / SAST (CI-gated)
poetry -C python run mie-decoder --help
poetry -P python build   # -P (not -C): -C doubles the src path on Windows; -P needs Poetry >= 2.0

# Shared Rust/Python behavior (needs the Rust binary built and an interpreter
# with mie_decoder installed; `--python-bin` overrides the interpreter)
(cd rust && cargo build)
poetry -C python run python ../tests/conformance/run.py
```

## Architecture

The decoder is a unidirectional pipeline. The big picture is best understood by tracing one record from disk to CSV:

1. **`reader.rs` — `MieFileReader`**: Top-level mmap-backed iterator. Calls `find_first_record()` for header skip, then loops: `validate_record()` → decode → yield `Result<MieMessage>`. On validation failure it calls `recover_sync()` to walk forward in 2-byte steps. Owns the `prev_was_error` flag used to classify SPURIOUS_DATA continuations.
2. **`sync.rs`**: Pure validation helpers (`find_first_record`, `validate_record`, `recover_sync`). Validation uses a **configurable N-record look-ahead** (default 2, per L2-SYN-026) — a candidate is only confirmed valid if the next `N-1` records' Type Words also look valid. This is critical: a single Type Word match alone produces too many false positives. No logging in this module — the reader emits any messages.
3. **`decode.rs`**: Pure binary → struct conversion. Type Word bit layout, IRIG vs Standard timestamp formats (auto-detected by probing the Command Word at both candidate offsets and scoring), Command Word, message format classification.
4. **`models.rs`**: Plain structs (`MieMessage`, `TypeWord`, `CommandWord`, `IrigTimestamp`, `StandardTimestamp`), `IntEnum`-style enums with explicit `#[repr(u8)]` discriminants, DDC error code constants (0x01xx) and decoder-assigned spurious codes (0x20xx). `DataWords` is the fixed-capacity inline buffer that replaces `Vec<u16>` for the per-record payload.
5. **`filter.rs` — `FilterIterExt::filter_messages`**: Iterator adapter. Both `exclude_*` and `include_*` filters are supported (the include set is the v2 redesign).
6. **`order.rs` — `OrderIterExt::order_rows`** (mirrored by `python/src/mie_decoder/order.py`): canonical row order (L1-OUT-003 / L2-WRT-021). The **last** iterator stage before the writer, on both the single-file and merge paths. Buffers one run of *consecutive* equal-`TIME_STAMP` records and stable-sorts it by `(RT, subaddress, direction)` — `Direction::Receive = 0` / `Transmit = 1` gives R-before-T for free in both languages. Records with no Command Word (`SPURIOUS_DATA`) are **pinned**: excluded from the sort and carried along by whichever record preceded them, which is what keeps an errored record adjacent to its `0x2000` continuation. Buffering is capped by `max_sort_group` (L2-WRT-022, default 4096); at the cap the run is flushed in arrival order with one WARN.
7. **`writer.rs`**: `write_csv` (single file) and `write_csv_split` (separate `_errors.csv`). Streams rows through a `BufWriter` — no DataFrame buffering. Column names and ordering match DDC vendor CSV byte-for-byte.
8. **`config.rs`**: Hand-rolled TOML loader for our schema (sections + key=value with strings/ints/bools/primitive arrays). Produces `DecoderConfig`. Precedence: **CLI overrides > config file > defaults**, applied via `DecoderConfig::with_overrides(ConfigOverrides)`.
9. **`cli.rs` — `run(argv)`**: Hand-rolled argparse with three subcommands (`decode`, `count`, `dump`). `count` is its own subcommand in v2 (was `--count` flag in v1). Default error mode is `inline` (every record in one CSV with `ERROR`/`ERROR_CODE` populated); `--separate-errors` toggles the split-file mode. The polarity was reversed and the former `--inline-errors` flag removed — passing it is a usage error.
10. **`log.rs`**: Tiny stderr logger. Single `AtomicU8` for the global level + `log_debug!`/`log_info!`/`log_warn!`/`log_error!` macros that format only when the level passes.
11. **`merge.rs` — `MergedRecordIter`** (mirrored by `python/src/mie_decoder/merge.py`): multi-file time-sorted k-way merge (L1-MRG / L2-MRG). When `decode` resolves more than one input (positionals / `--manifest` / `--glob`, mutually exclusive, capped at `MAX_MERGE_FILES = 256`), this holds one record per open reader in a min-heap (`BinaryHeap`+`Reverse` in Rust, `heapq` in Python), ordered by absolute IRIG microseconds with a `(us, file_index, seq)` tiebreak — O(files) memory, O(1) in records. That tiebreak is the **heap's internal** order only; the equal-timestamp order the CSV shows is `order.rs`'s canonical RT/MSG order (a heap key can't do it — the heap never holds two same-timestamp records from one input at once). It validates every input is calendar-locked IRIG up front (Standard / freerun / mixed → `IncompatibleMergeInputs`, exit 6) and applies the L2-MRG-005 DELTA scope: `per-file` (the default) leaves each reader's own DELTA in place — which is what makes a merged record's value identical to a single-file decode — while `global` (`--delta-scope global`) recomputes on the merged timeline. A single input bypasses this module entirely. No new dependency (hand-rolled `*`/`?` glob matcher in Rust).

### Error handling model (important and non-obvious)

When the DDC card detects a bus error, it sets **bit 14 of the Type Word**, truncates the payload, and appends a 16-bit Error Word containing the code. If words remain from the original transaction, the card writes them as a separate `SPURIOUS_DATA` (type `0x20`) record immediately after.

The reader tracks `prev_was_error` across records so it can classify a following `SPURIOUS_DATA` as either `0x2000` (continuation of a preceding error) or `0x2001` (standalone). These `0x20xx` codes are decoder-assigned, not DDC hardware codes — see `models.rs` for the full code table.

Error records and SPURIOUS_DATA continuations are **valid records** that pass sync validation normally. Sync loss only happens on truly corrupt data (truncated mid-word, power loss). Don't conflate "errored record" with "sync loss."

### Output modes

- **Default (`error_mode = inline`)**: every record goes to the one CSV, with the `ERROR` / `ERROR_CODE` columns populated on errored and spurious rows. Calls `write_csv`. This is the mode a vendor-CSV diff uses, since the vendor tool also emits a single file.
- `--separate-errors` (`error_mode = separate`): clean messages → main CSV, errored + spurious → `<stem>_errors<suffix>` (lazy — the file isn't created if there are no error rows). Calls `write_csv_split`. Ignored on stdout (you can't split stdout), with a WARN.

The polarity was reversed in v2.8.0 — `separate` was the old default and the former `--inline-errors` flag was removed, so passing it is a usage error.

### Error type

All fallible APIs return `Result<T, MieError>`. `MieError` is a single enum (not a hierarchy). `kind()` returns a `MieErrorKind` discriminant. `is_record_error()` matches Python's `MieRecordError` exactly; `is_file_error()` is deliberately narrower than `MieFileError` (input I/O only — the whole-file rejections and destination guards answer `false` to both predicates). Both implementations pin the full classification by test, so a new variant can't be added without classifying it on both sides.

## Reference docs

- `docs/ARCHITECTURE.md` — module diagram, four-phase sync strategy, error pipeline, configuration hierarchy, error type, logging levels. Read this when changing the reader/sync code.
- `docs/USER-GUIDE.md` — end-to-end walkthrough for analysts and operators: install, decode-your-first-file, the three subcommands, common workflows (stdout / inline errors / allow-partial / filtering / site config), reading the CSV, diagnosing failures. The "front door" for non-maintainer readers.
- `docs/VENDOR-CSV-DIFFS.md` — alignment statement between MIE-Decoder's CSV output and DDC vendor-generated CSV: which columns match byte-for-byte, the five vendor-empty columns we preserve as placeholders, the known IRIG day-of-year firmware discrepancy, the validation workflow, and the protocol for reporting a divergence as a bug.
- `docs/EXAMPLES.md` — runnable cookbook of common operator tasks: first-time decode, record counting, inline error output for vendor diff, RT-focused filtering, recovering from corrupt recordings with `--allow-partial`, stdout piping into pandas/awk, site-wide config plus per-invocation overrides, CI batch scripts with proper exit-code handling, investigating rejected files with `dump`, full vendor-CSV diff workflow, and a handful of shell ad-hoc filter patterns. Pairs with USER-GUIDE.md (which explains how the pieces work) by showing the pieces composed for real workflows.
- `docs/CLI-REFERENCE.md` — complete per-flag reference for the `decode` / `count` / `dump` subcommands and global options: value, default, range, and config-key equivalent for every flag. The canonical home for CLI parameter docs (the root README links here rather than duplicating the flag surface); mirror of `CONFIG-REFERENCE.md` on the CLI side.
- `docs/CONFIG-REFERENCE.md` — normative reference for every TOML key the decoder accepts, with type / default / CLI override / validation behavior per key, plus precedence and unknown-key handling.
- `docs/ERROR-CATALOG.md` — operator-facing reference for every CLI exit code, error class, DDC error code (`0x01xx`), and decoder-assigned code (`0x20xx`). Updated when error variants are added or removed.
- `docs/DATA-SCENARIOS.md` — plain-language, scenario-indexed map of how the tool handles every data condition (clean / error / spurious records, IRIG / Standard / freerun timestamps, empty / truncated / non-MIE files, multi-file merge including per-file `--allow-partial` and duplicate collapsing, output modes, filters, MUX), each with its CSV / log / exit outcome and a glossary that defines the jargon (including "oracle"). Summarizes and links to ERROR-CATALOG.md (codes / exits) and MIE-FORMAT.md (binary). The "which scenario am I in, and what will the tool do?" front door.
- `docs/MAINTAINER-GUIDE.md` — repo layout, local dev setup, command cheat sheet, workflows for adding requirements / tests / conformance fixtures / error variants / CLI flags, CI architecture, coverage workflow, release process, cross-impl alignment principles. Start here when onboarding to make changes to the codebase.
- `docs/MIE-FORMAT.md` — comprehensive MIE binary format reference: file-level framing, the three-section record shape, Type Word / IRIG and Standard timestamp / Command Word / Status Word bit layouts, per-format payload shapes for all 11 transaction types, error-record lifecycle (Type Word bit 14 → truncated payload → Error Word → optional SPURIOUS continuation), the DDC `0x01xx` and decoder-assigned `0x20xx` error code tables, full CSV output reference, three worked hex-to-CSV decodes. The deep reference for reverse-engineering or adding format support.
- `docs/L1-REQ.md` — Level 1 SHALL statements (system requirements grouped by category, plus the NR-001 out-of-scope note).
- `docs/L2-REQ.md` — Level 2 architectural derivations (each with a single L1 parent).
- `docs/L3-REQ.md` — Level 3 implementation obligations (cross-impl `L3-WRT-*`, plus per-impl `L3-PY-*` / `L3-RS-*`; `L3-RS-007` is withdrawn and its ID reserved, from when static-musl support was retired).
- `docs/TRACE-MATRIX.md` — auto-generated trace matrix produced by `scripts/build-trace-matrix.py`. Forward trace from L1 through L2 and L3 to test artifacts (`@pytest.mark.requirement` markers in `python/tests/` and `/// Requirements:` doc-comments above Rust `#[test]` items). Treat as the single source of truth for live status; the source docs hold spec content only.
- `docs/ROADMAP.md` — forward-looking roadmap: planned work plus pinned "do not drop" commitments (TOML config, CSV byte-compat, sync semantics). Completed work is not tracked here — it lives in `CHANGELOG.md` and the L1/L2/L3 requirements.
- `config/default.toml` — fully commented reference configuration; preserved across the port.
- `python/` — maintained Python package and CLI with its own source and tests.
- `tests/conformance/` — shared hexadecimal fixtures and byte-exact CSV
  oracles exercised against both implementations.

## Conventions worth preserving

- **Single external dependency.** Only `memmap2`. Adding crates requires justification — argument parsing, CSV, TOML, logging, error types are all hand-rolled by design and the user values keeping it that way.
- **Streaming CSV.** Rows must flow through a `Write` impl as they are produced. Do not introduce `Vec<MieMessage>` or `Vec<Row>` buffering in the writer — constant memory is the design point.
- **N-record look-ahead in `sync.rs`** (default 2, configurable per L2-SYN-026). Don't remove it. Removing the look-ahead reintroduces false-positive resyncs.
- **`DataWords` is fixed-capacity by design.** MIL-STD-1553B caps a single transaction at 32 data words. Don't switch to `Vec<u16>` "for flexibility."
- **The canonical-order stage is run-scoped and capped.** `order.rs` / `order.py` permute only a run of
  *consecutive* equal-`TIME_STAMP` records, and only up to `max_sort_group` of them. Don't turn it into a
  whole-file sort (that breaks the constant-memory design point and L2-MRG-006's "never re-sort" rule) and
  don't remove the cap (a corrupt file whose timestamps all decode alike would buffer everything). Don't
  "simplify" the pinning of Command-Word-less records into a sentinel sort key either: a pin's position is
  defined relative to its *predecessor*, so it has to travel with that record — preserving its index is not
  the same thing, and getting this wrong silently breaks the `0x2000` continuation semantics.
- **CSV column names and order are dictated by DDC vendor output — for the first 44 columns.** Don't "clean up" `MUX`, `TERM_NAME`, `IM_GAP`, `RCV_GAP`, `XMT_GAP` — they're columns by spec (`L2-WRT-013`). `TERM_NAME`/`IM_GAP`/`RCV_GAP`/`XMT_GAP` stay empty; `MUX` is populated from the input file name by default (`L2-WRT-020`) and is restored to empty (vendor-exact) by `--no-mux` / `[mux] enabled = false`.
- **Decoder-added columns go at the TAIL, never inside the vendor block** (`L1-OUT-001`, `L2-WRT-001`). The vendor layout is columns 1–44, `TIME_STAMP` through `XMT_GAP`; `ERROR` and `ERROR_CODE` (45–46) are decoder features with no vendor counterpart — the DDC tool doesn't emit them at all. Through v2.9.0 they sat between `DELTA` and `IM_GAP`, which silently shifted the three gap columns two positions off their vendor indices and made every positional comparison past `DELTA` wrong while all the column *names* still matched. If you add a column, append it; and prefer resolving columns by header name over index in tests (`writer.rs`'s `vendor_block_precedes_decoder_added_columns` and its Python mirror pin the boundary).
- **The sync modules are pure** (no logging, no I/O) in **both** implementations — `rust/src/sync.rs` and `python/src/mie_decoder/sync.py`. The reader handles all user-facing messaging based on the values they return. Don't move logging into validation helpers: they lack the caller's context, so they narrate outcomes wrongly (`find_first_record` returning `None` is the *expected* result for a valid empty recording, and logging "no valid record found" there contradicted the reader's own correct message).
- **Shared conformance fixtures are byte-exact.** Treat
  `tests/conformance/` as the cross-implementation oracle; update expected CSV
  only after both implementations agree.
- **Both implementations are maintained.** Keep Rust-specific design decisions
  in the Rust crate and Python-specific design decisions in `python/`; align
  shared format semantics and vendor-compatible CSV behavior.

## Git conventions

Do **not** add `Co-Authored-By: Claude ...` trailers to commit messages on this repo, even if the harness's default instructions suggest it. Commit messages are the human-authored record of intent; tool attribution belongs in tool logs, not history. This overrides the default trailer behavior.
