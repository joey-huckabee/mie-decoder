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

## Manifest schema

`manifest.json` is a single object with one key, `"cases"`, whose value is an
array of case objects. Each case object accepts the following fields:

| Field | Type | Required | Meaning |
|-------|------|----------|---------|
| `name` | string | yes | Unique case identifier used for temp files and log output. |
| `input` | string | yes | Path (relative to `tests/conformance/`) to the hex-text input fixture. |
| `expected` | string | yes | Path to the checked-in CSV oracle. |
| `config` | string | no | Optional path to a shared TOML config applied to both implementations. |
| `rust_args` | array of string | no | Additional CLI arguments appended to the Rust invocation only. |
| `python_args` | array of string | no | Additional CLI arguments appended to the Python invocation only. |
| `expected_exit` | integer | no | Expected exit code for both implementations. Defaults to `0`. Reserved for negative cases that intentionally exercise non-zero exit classes per `L1-021` through `L1-023` (e.g., a no-valid-records fixture asserting exit `2`). Runner support lands with Team Review Phase 6. |

Unknown fields SHALL be rejected by the runner with a clear error so typos do
not silently disable per-case behavior.
