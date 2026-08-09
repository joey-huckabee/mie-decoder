# Cross-Implementation Conformance

This suite verifies behavior shared by the Rust and Python implementations.
Each case provides:

- a text-based hexadecimal MIE input under `inputs/`;
- optional shared TOML configuration under `configs/`;
- expected vendor-compatible CSV output under `expected/`; and
- optional extra CLI arguments in `manifest.json` (the `args` field), passed verbatim to both CLIs — they share one identical argument surface.

The runner materializes temporary `.mie` files, invokes both CLIs, and requires
both outputs to match the checked-in CSV oracle byte-for-byte.

Run from the repository root, **through Poetry**:

```bash
poetry -C python run python ../tests/conformance/run.py
```

The runner drives the Python CLI with `sys.executable` — the interpreter it is
itself running under. A bare `python tests/conformance/run.py` therefore uses
your system Python, which does not have `mie_decoder` after `poetry -C python
sync` installs it into Poetry's virtualenv. The runner probes for the import and
fails fast with the fix, but the form above is the one that works. (CI uses the
bare form because it `pip install -e ./python` into its system interpreter
first.) `--python-bin <path>` overrides the interpreter explicitly.

To use an already-built Rust binary:

```bash
poetry -C python run python ../tests/conformance/run.py --rust-bin ../rust/target/debug/mie-decoder
```

Two path traps here, both easy to hit:

- `poetry -C python run` executes with the working directory set to `python/`,
  so **every relative path is relative to `python/`** — hence the `../` on both
  the script and `--rust-bin`.
- `--rust-bin` is used exactly as given; the runner only appends `.exe` when it
  locates the binary itself. On Windows pass
  `../rust/target/debug/mie-decoder.exe`, or omit the flag and let the runner
  find it. A path that does not exist is reported as `failed to build the Rust
  CLI`, which names the symptom rather than the cause.

### Single-implementation runs (one toolchain)

Because the `expected/` oracles are committed, each implementation can be
validated on its own — useful where only one toolchain is available (e.g. an
air-gapped host with Python but no Rust/cargo). These skip the other
implementation's setup and the cross-impl CLI-surface check; each side is still
held to the same byte-exact oracle.

```bash
poetry -C python run python ../tests/conformance/run.py --python-only   # no cargo needed
python tests/conformance/run.py --rust-only --rust-bin <path>           # no mie_decoder package needed
```

(`--update-expected` still requires both implementations, since it regenerates
the oracles only after confirming Rust and Python agree.)

When intentionally changing shared CSV behavior, update the checked-in
oracles only after both implementations produce identical output:

```bash
poetry -C python run python ../tests/conformance/run.py --update-expected
```

Keep implementation-specific CLI behavior in each implementation's own test
suite. Add cases here only for shared MIE decoding and CSV semantics.

## Config-parser parity

The two implementations parse config with different engines (`tomllib` vs the
minimal Rust parser), so `run.py` cross-checks them when both are present:

- **`config_parity.py`** — a *curated* corpus of TOML snippets, each labeled
  `accept` / `reject`. Every snippet is run through both CLIs; they must land in
  the same class and match the label. Add one whenever a new form could diverge.
- **`config_fuzz.py`** — a differential *fuzzer* that **generates** many small
  config documents (heavily sampling numeric/escape/structural edges) and asserts
  both CLIs agree on accept/reject. Deterministic by default (fixed seed +
  iteration count); on a divergence it prints the exact config so it can be
  pinned in `config_parity.py`. Set `MIE_CONFIG_FUZZ_SEED` / `MIE_CONFIG_FUZZ_ITERS` to explore
  further locally.
- **`config_path_parity.py`** — the layer above: the `--config` **path**, not its
  contents. The two above always hand the CLIs a perfectly ordinary file, so the
  path's own behavior — what counts as usable, which exit code a bad one yields,
  what the operator is told — had no cross-implementation check at all. This one
  compares the **exact exit code** (not just accept/reject) and requires the
  promised message text from both, across the surface documented in
  `docs/CONFIG-REFERENCE.md` §"Trust boundary": regular files only; missing or
  unusable is exit `5`; any readable location is fine, including names with
  spaces, non-ASCII names and `..` segments. Cases needing platform support
  (character devices, symlinks) skip themselves and say so, so a corpus that
  shrinks on one OS is visible rather than silent.

This is what stops config divergences from being found one at a time: the fuzzer
searches the space so CI catches a mismatch before a reviewer does.

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
