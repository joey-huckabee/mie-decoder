# Cross-Implementation Conformance

This suite verifies behavior shared by the Rust and Python implementations.
Each case provides:

- a text-based hexadecimal MIE input under `inputs/`;
- optional shared TOML configuration under `configs/`;
- expected vendor-compatible CSV output under `expected/`; and
- optional extra CLI arguments in `manifest.json` (the `args` field), passed verbatim to both CLIs — they share one identical argument surface.

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
| `expected` | string | when `expected_exit == 0` | Path to the checked-in oracle. For `mode == "decode"` (default) this is the expected CSV; for `mode == "count"` this is a text file containing the expected integer count plus a trailing newline. |
| `expected_errors` | string | no | Path to the expected `<stem>_errors.csv` oracle for split-error-mode (`mode == "decode"` only). |
| `config` | string | no | Optional path to a shared TOML config applied to both implementations. |
| `mode` | string | no | Either `"decode"` (default — both impls run their decode pipeline; CSV output is compared) or `"count"` (both impls run the `count` subcommand; stdout is compared). |
| `args` | array of string | no | Additional CLI arguments appended to both invocations verbatim. The Rust and Python CLIs share one argument surface, so a single vector serves both — there is no per-impl argument translation. |
| `expected_stderr_contains` | string | no | Substring assertion applied to each impl's captured stderr. Used by `mode == "count"` cases to pin the human-readable status line without byte-comparing a temp path. |
| `expected_exit` | integer | no | Expected exit code for both implementations. Defaults to `0`. Negative cases (exit `1`/`2`/`3` per `L1-EXIT-002`..`L1-EXIT-004`) may omit `expected`; the exit code alone is the assertion. |

Unknown fields SHALL be rejected by the runner with a clear error so typos do
not silently disable per-case behavior.
