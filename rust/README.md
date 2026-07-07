# MIE-Decoder (Rust)

The Rust implementation of MIE-Decoder: a streaming, constant-memory decoder for
DDC MIL-STD-1553 MIE binary recording files, with a hand-rolled CLI and a single
native release binary.

Edition 2024, MSRV 1.88 (`memmap2` requires ≥ 1.88; edition 2024 itself only
floors at 1.85). The crate has exactly one external dependency
(`memmap2`); argument parsing, CSV writing, TOML config, logging, and error
types are all hand-rolled.

Shared documentation — the project overview, CLI reference, configuration
schema, supported message formats, error catalog, and vendor-CSV alignment —
lives at the [repository root](../README.md) and under [`docs/`](../docs/).

## Build

```bash
cd rust
cargo build --release
# binary lands at rust/target/release/mie-decoder
```

## Library usage

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

## Development

```bash
cd rust
cargo test                  # All unit + integration tests
cargo test --test integration -- multi_record_stream   # One integration test
cargo build --release       # Release binary at rust/target/release/mie-decoder
cargo clippy --all-targets  # Lint (if installed)
```

Three coverage aliases are pre-wired in `.cargo/config.toml`: `cargo cov`
(local HTML report), `cargo cov-lcov` (writes `lcov.info`), and `cargo cov-ci`
(the enforced 84% line / 83% region gate). See
[`CONTRIBUTING.md`](../CONTRIBUTING.md) for the full development workflow and
[`docs/MAINTAINER-GUIDE.md`](../docs/MAINTAINER-GUIDE.md) for repo conventions.

## Crate structure

```
rust/
├── Cargo.toml / Cargo.lock
├── .cargo/          cargo-llvm-cov coverage aliases (cov / cov-lcov / cov-ci)
├── src/
│   ├── lib.rs           Library entry point and re-exports
│   ├── bin/mie-decoder  Binary entry point
│   ├── cli.rs           Hand-rolled argparse + dispatch
│   ├── config.rs        Hand-rolled TOML loader, DecoderConfig
│   ├── decode.rs        Pure decoders + format classifier
│   ├── dump.rs          Hex dump (raw + record-aware)
│   ├── error.rs         MieError enum + Display/Error impls
│   ├── filter.rs        FilterConfig + Iterator adapter
│   ├── log.rs           Tiny stderr logger
│   ├── models.rs        Data structures, enums, error codes
│   ├── reader.rs        mmap-backed iterator with sync recovery
│   ├── sync.rs          Pure validation + recovery helpers
│   └── writer.rs        Streaming CSV writer
└── tests/
    ├── cli.rs           CLI acceptance tests (built binary as a subprocess)
    └── integration.rs   End-to-end tests with byte-exact fixtures
```

## License

Apache-2.0 — see [LICENSE](../LICENSE).
