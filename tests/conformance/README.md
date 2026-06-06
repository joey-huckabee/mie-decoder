# Cross-Implementation Conformance

This suite verifies behavior shared by the Rust and Python implementations.
Each case provides:

- a text-based hexadecimal MIE input under `inputs/`;
- optional shared TOML configuration under `configs/`;
- expected vendor-compatible CSV output under `expected/`; and
- per-implementation CLI arguments in `manifest.json` where syntax differs.

The runner materializes temporary `.mie` files, invokes both CLIs, and requires
both outputs to match the checked-in CSV oracle byte-for-byte.

Run from the repository root:

```bash
python tests/conformance/run.py
```

To use an already-built Rust binary:

```bash
python tests/conformance/run.py --rust-bin target/debug/mie-decoder
```

When intentionally changing shared CSV behavior, update the checked-in
oracles only after both implementations produce identical output:

```bash
python tests/conformance/run.py --update-expected
```

Keep implementation-specific CLI behavior in each implementation's own test
suite. Add cases here only for shared MIE decoding and CSV semantics.
