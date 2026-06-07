# MIE-Decoder — Maintainer Guide

Operational reference for anyone modifying the MIE-Decoder codebase. Covers the workflows you'll repeat: adding requirements, tests, conformance fixtures, error variants, and CLI flags; running the trace matrix; bumping coverage; releasing.

This guide is for **maintainers**. End-user CLI usage belongs in `docs/USER-GUIDE.md` (not yet written); error/exit-code reference for operators lives in [`docs/ERROR-CATALOG.md`](ERROR-CATALOG.md); LLM-session project conventions live in [`CLAUDE.md`](../CLAUDE.md).

---

## 1. Repo layout

```
mie-decoder/
├── src/                    Rust crate (v1.0.0, edition 2024, MSRV 1.85)
│   ├── reader.rs           mmap-backed sequential reader; the central pipeline
│   ├── sync.rs             validate_record, find_first_record, recover_sync
│   ├── decode.rs           Type Word, IRIG/Standard timestamps, Cmd Word, classification
│   ├── models.rs           plain structs + enums + DDC/decoder error code constants
│   ├── error.rs            single MieError enum + MieErrorKind discriminant
│   ├── writer.rs           streaming CSV writer with atomic temp + rename
│   ├── filter.rs           message exclusion / inclusion filtering
│   ├── config.rs           hand-rolled TOML loader for the L2-CFG schema
│   ├── cli.rs              hand-rolled argparse + run() entry
│   ├── dump.rs             raw + record-aware hex dump
│   └── log.rs              ~50-line stderr logger
├── tests/
│   ├── integration.rs      cargo integration tests (multi-record, fuzz harness)
│   └── conformance/        cross-implementation suite (Rust ↔ Python)
│       ├── manifest.json   case catalog
│       ├── inputs/*.hex    reviewable hex fixtures (NOT committed binaries)
│       ├── expected/*.csv  byte-exact CSV oracles
│       ├── configs/*.toml  per-case TOML config
│       └── run.py          the runner
├── python/                 Python package (v1.0.0, supports 3.10–3.14)
│   ├── pyproject.toml      Poetry + PEP 621 hybrid; pytest markers registered here
│   ├── poetry.lock         pinned dependencies; committed
│   ├── src/mie_decoder/    package source (mirrors Rust module names)
│   └── tests/              pytest suite
├── scripts/
│   ├── build-trace-matrix.py    generates docs/TRACE-MATRIX.md
│   └── pytest-by-requirement.py runs pytest filtered by requirement marker
├── docs/
│   ├── L1-REQ.md / L2-REQ.md / L3-REQ.md   spec docs (source of truth)
│   ├── TRACE-MATRIX.md     auto-generated from L1/L2/L3 + test markers
│   ├── ARCHITECTURE.md     module diagram, error pipeline, configuration hierarchy
│   ├── ERROR-CATALOG.md    operator reference for every error / exit code
│   ├── MIE-FORMAT.md       comprehensive binary format + CSV column reference
│   ├── FIELDS.md           redirect stub for legacy links
│   ├── ROADMAP.md          versioned roadmap with status annotations
│   ├── MAINTAINER-GUIDE.md (this file)
│   └── diagrams/           PlantUML sources and rendered SVGs
├── config/default.toml     fully-commented reference TOML schema
├── .github/workflows/ci.yml
└── Cargo.toml / Cargo.lock
```

---

## 2. Local development setup

### Rust

```bash
rustup update stable
rustup default stable
rustup component add clippy rustfmt llvm-tools-preview
cargo build
cargo test
```

For the production static-musl build (SLES 12 target):

```bash
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
```

### Python

```bash
pipx install poetry==2.3.4   # or via your usual install
poetry -C python sync         # creates the venv, installs locked deps + mie_decoder
poetry -C python run pytest
```

The Python package is installed in editable mode via `poetry sync`'s root-package step. If `python -m mie_decoder` ever fails to import in your local Poetry env, re-run sync.

### Cross-impl conformance (needs both)

```bash
# Build the Rust binary first so the runner doesn't have to:
cargo build
# Then run the suite (uses Poetry's interpreter for the Python side):
poetry -C python run python ../tests/conformance/run.py
```

The runner reads `tests/conformance/manifest.json`, materializes each `.hex` fixture into a temp `.mie` file, invokes both CLIs against it, and diffs the produced CSVs against the checked-in oracle (or asserts the exit code for negative cases).

---

## 3. Daily-command cheat sheet

```bash
# Rust
cargo test --all-targets                         # all unit + integration
cargo test --lib reader::tests::skips_proprietary_header   # single test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
cargo cov-ci                                     # coverage gate (alias in .cargo/config.toml)

# Python (from repo root)
poetry -C python run pytest                      # all tests
poetry -C python run pytest tests/test_e2e.py -k delta -v
poetry -C python run python ../tests/conformance/run.py

# Filter pytest by requirement marker
python scripts/pytest-by-requirement.py L2-WRT-015
python scripts/pytest-by-requirement.py L3-PY-          # whole L3-PY-* family

# Trace matrix
python scripts/build-trace-matrix.py             # regenerate docs/TRACE-MATRIX.md
python scripts/build-trace-matrix.py --check     # what CI does — exits 1 on drift

# PlantUML diagrams
plantuml -tsvg docs/diagrams/*.puml              # regenerate committed SVGs

# CLI dry-runs against a real file
cargo run --release -- decode path/to/recording.mie -o decoded.csv
poetry -C python run mie-decoder decode path/to/recording.mie -o decoded.csv
```

Commit each `docs/diagrams/*.puml` source with its matching rendered
`docs/diagrams/*.svg`. Regenerate the SVG whenever the PlantUML source changes.

---

## 4. Adding a requirement

The three-tier system: **L1** = system SHALL statement; **L2** = architectural derivation with one L1 parent; **L3** = implementation obligation with one L2 parent.

### Choose the tier

- **L1** — a new product-level capability or constraint. Rare. The 12 categories in `docs/L1-REQ.md` (`DEC`, `OUT`, `DLT`, `CLI`, `LOG`, `MODE`, `SYN`, `ERR`, `CFG`, `CONF`, `EXIT`, `ROB`) are stable; pick the closest fit.
- **L2** — a behavior derived from an existing L1. Most new requirements land here. Pick the L2 category that names the behavior (`L2-DEC-*`, `L2-WRT-*`, `L2-SYN-*`, etc.).
- **L3** — an implementation detail. Use `L3-PY-*` for Python-only constraints, `L3-RS-*` for Rust-only, or the L2's category code for cross-impl detail (e.g., `L3-WRT-001` pins the temp-file naming pattern derived from `L2-WRT-015`).

### Add the ID

Pick the next integer in the category — retired IDs are never reused. The current max per category is visible in `docs/L1-REQ.md` / `L2-REQ.md` / `L3-REQ.md` category tables.

**L1 format** (in `L1-REQ.md`):

```markdown
### L1-XXX-NNN

**Statement**: [SHALL obligation]

**Rationale**: [why this requirement exists]

**Verification Method**: Test (T)
```

**L2 format** (in `L2-REQ.md`):

```markdown
#### L2-XXX-NNN

**Parent**: L1-XXX-NNN
**Statement**: ...
**Rationale**: ...
**Verification Method**: Test (T)
```

**L3 format** (in `L3-REQ.md`, compact two-line):

```markdown
**L3-XXX-NNN** · Parent: L2-XXX-NNN · Verification: T
Statement text on the next line.
```

### Choose the verification method

Single letters from DO-178: **T** = Test, **I** = Inspection, **A** = Analysis, **D** = Demonstration. Multiple methods comma-separated.

- **Test (T)** — there's an automated test asserting the behavior. The trace matrix expects a `@pytest.mark.requirement` marker or a `/// Requirements:` doc-comment.
- **Inspection (I)** — verified by reading the source. The trace matrix marks these `Implemented (I)` even without a test marker. Use for structural properties (a single function called from three places, an enum being exhaustively matched, build config declarations).
- **Analysis (A)** — verified by logical/mathematical argument. Use for bounded-loop proofs, memory complexity claims.
- **Demonstration (D)** — verified by operator running the system. Use for things like "static-musl build works on SLES 12".

Don't mark `Test (T)` if no test exists or will exist. The matrix will surface it as **Draft** and the gap will be obvious.

### Tag the test

Once an L1/L2/L3 ID exists, tag whichever tests verify it:

**Python** (in `python/tests/`):

```python
@pytest.mark.requirement("L2-WRT-015")
def test_temp_file_rename_is_atomic(tmp_path: Path) -> None:
    ...
```

Multiple markers stack:

```python
@pytest.mark.requirement("L2-CLI-011")
@pytest.mark.requirement("L1-EXIT-002")
def test_cli_no_valid_records_returns_exit_2(...):
    ...
```

**Rust** (`src/**/*.rs` and `tests/*.rs`):

```rust
/// Requirements: L2-WRT-015
#[test]
fn temp_file_rename_is_atomic() { ... }
```

Multiple IDs comma-separated on one line:

```rust
/// Requirements: L2-WRT-015, L2-WRT-016, L3-WRT-001
#[test]
fn atomic_commit_renames_temp_over_destination() { ... }
```

### Regenerate the trace matrix

After adding the spec / tags:

```bash
python scripts/build-trace-matrix.py
git add docs/TRACE-MATRIX.md
```

If you forget, CI's `trace-matrix` job (`python scripts/build-trace-matrix.py --check`) will fail with a clear message.

---

## 5. Adding a test

### Test pyramid

The project uses four test tiers, narrowest scope at the bottom:

| Tier                  | Subject                                  | Location                            | Run with                                         | Cross-platform |
|-----------------------|------------------------------------------|-------------------------------------|--------------------------------------------------|----------------|
| **Unit**              | one function / one module, in-process    | `src/<module>.rs` `#[cfg(test)] mod tests` (Rust); `python/tests/test_*.py` (Python) | `cargo test --lib` / `pytest`                    | Linux + Windows |
| **Integration**       | multiple modules via the library API, in-process | `tests/integration.rs` (Rust); `python/tests/test_integration_*.py` (Python) | `cargo test --test integration` / `pytest`       | Linux + Windows |
| **CLI acceptance**    | the **built binary** as a subprocess — exit codes, stdout, stderr, filesystem effects | `tests/cli.rs` (Rust)               | `cargo test --test cli`                          | Linux + Windows |
| **Conformance**       | byte-exact cross-impl equivalence (Rust ↔ Python CLI) | `tests/conformance/`                | `python tests/conformance/run.py`                | Linux + Windows |

The two upper tiers both spawn the actual binary, but they serve different purposes:

- **CLI acceptance** (`tests/cli.rs`) is Rust-only. It covers behaviors that conformance can't or doesn't: `--no-clobber`, input/output collision rejection, `--include-*` filter syntax (a Rust-only axis per L3-RS-010), `--help` / `--version`, exit-class taxonomy, and other CLI surfaces where stdout/stderr/exit-code semantics matter more than CSV byte-equality.
- **Conformance** (`tests/conformance/`) holds Rust and Python to byte-identical CSV output (or matching exit code for negative cases). Anything that affects the CSV contract should land here so both implementations stay aligned.

When you add a behavior, ask: **does this need to behave the same in Python?** If yes, add it to conformance. If no (Rust-only feature, atomic-write artifact, exit-class taxonomy detail), add it to `tests/cli.rs`. Both tiers run on Linux and Windows automatically via `cargo test --all-targets`.

### Python

Tests live in `python/tests/test_*.py`. Use the synthetic-record builders in `python/tests/conftest.py` if you need varied IRIG timestamps or errored / SPURIOUS records:

- `normal_record_rt15_sa11_us(microseconds)` — varies only the timestamp on the canonical receive record.
- `errored_record_rt15_sa11_us(microseconds)` — Type Word bit 14 set, 0x011E error code.
- `spurious_record_us(microseconds, data_word)` — SPURIOUS_DATA shape.

For end-to-end CLI tests, use `pytest.LogCaptureFixture` to assert on log messages, e.g. the `decode exit class:` summary line.

### Rust unit and integration

Unit tests live next to the code they test in `src/<module>.rs` under `#[cfg(test)] mod tests { ... }`. Library-level integration tests live in `tests/integration.rs`.

Use the existing `TempFile` helper at the bottom of `src/reader.rs` (private) or `tests/integration.rs` (also private but copy-paste-friendly). For new variants, construct minimal byte sequences from the canonical `RECORD_RT15_SA11_RCV` shape.

Always tag new tests with `/// Requirements:` so the trace matrix credits them.

### Rust CLI acceptance

CLI acceptance tests in `tests/cli.rs` spawn the actual built binary located via `env!("CARGO_BIN_EXE_mie-decoder")` (Cargo populates this per test target and appends `.exe` on Windows automatically — no per-OS code paths needed) and use `std::process::Command::output()` to invoke it. Style conventions:

- Use the `TempDir` helper in the same file: per-test scratch directories under `std::env::temp_dir()`, keyed by pid + atomic counter, removed on drop. Tests can then use plain `dir/input.mie`, `dir/output.csv` paths.
- Use the `run([...])` helper rather than `Command::new` directly. It echoes any captured stderr into test output so a Windows CI failure can be triaged from the runner log without re-running locally.
- Assert on exit code via the `exit_code(&out)` helper, on stdout/stderr via `String::from_utf8_lossy` + `.contains(...)`, and on filesystem effects with `std::fs::read[_to_string]`.
- Don't byte-compare CSV output — that's conformance's job. Acceptance tests should assert on coarser invariants (header row exists, row count >= 2, sentinel preserved when `--no-clobber` refused).
- Cross-platform considerations: never hard-code `/` or `\\` in paths (use `PathBuf::join` and pass paths as `OsStr`); never assert on `\n` vs `\r\n` (use substring `.contains()` on stdout/stderr).

Always tag new tests with `/// Requirements:` so the trace matrix credits them.

Run locally with:

```bash
cargo test --test cli                  # CLI suite only
cargo test --test cli -- --nocapture   # also show stdout / stderr from the spawned binary
cargo test --all-targets               # unit + integration + cli together (what CI runs)
```

---

## 6. Adding a conformance fixture

Cross-implementation conformance fixtures verify byte-identical CSV output (or matching exit code) between Rust and Python. Add a case only for behavior that's specified at L2 as shared.

### Steps

1. Build the hex fixture under `tests/conformance/inputs/`. Include a header comment naming the requirement(s) it exercises. Example: `tests/conformance/inputs/homogeneous-payload.hex`.

2. If the case expects a successful decode (default), generate the CSV oracle. Run both implementations against your fixture and compare manually until they agree, then commit the agreed output to `tests/conformance/expected/<name>.csv`.

3. For negative cases (no oracle, just exit-code check), set `expected_exit` in the manifest and skip the oracle file.

4. Register in `tests/conformance/manifest.json`:

   ```json
   {
     "name": "your-case",
     "input": "inputs/your-case.hex",
     "expected": "expected/your-case.csv"
   }
   ```

   For negative cases:

   ```json
   {
     "name": "your-case",
     "input": "inputs/your-case.hex",
     "expected_exit": 2
   }
   ```

   For strict-mode cases:

   ```json
   {
     "name": "your-case",
     "input": "inputs/your-case.hex",
     "expected_exit": 1,
     "config": "configs/strict.toml"
   }
   ```

5. For cases that require flag differences between Rust and Python CLI syntax, add `rust_args` and `python_args` arrays. Don't add a fixture for an implementation-specific behavior (those go in each impl's own test suite per L1-CONF-001).

6. Run the suite locally to confirm:

   ```bash
   cargo build
   poetry -C python run python ../tests/conformance/run.py
   ```

7. Update the count in any docs that mention "N conformance cases" (this guide, etc.).

See `tests/conformance/README.md` for the full manifest schema.

---

## 7. Adding an error variant

When a new error class is needed (per `docs/ERROR-CATALOG.md` taxonomy), land it in both crates and document.

### Rust (`src/error.rs`)

1. Add the variant to `enum MieError { ... }` with `offset` and any structured detail fields.
2. Add a matching value to `enum MieErrorKind`.
3. Extend `MieError::kind()` to map the new variant.
4. Add a match arm to the `impl fmt::Display for MieError` block with the user-facing message.
5. If the variant is record-class, add it to the `is_record_error()` matches list. If file-class, add to `is_file_error()`.

### Python (`python/src/mie_decoder/exceptions.py`)

1. Add a new class extending `MieFileError` or `MieRecordError` as appropriate. Follow the existing pattern: `__init__` sets typed attributes and calls `super().__init__(message)`.
2. Add a class-level docstring naming the L1/L2 requirement(s) it satisfies.

### CLI exit-code mapping

`src/cli.rs` and `python/src/mie_decoder/cli.py` both have a try/except (or `match`) chain that maps errors to exit codes. Decide which class the new error belongs to (see `docs/ERROR-CATALOG.md` section 1):

- Wrong-file-type / file-shape errors → exit 2 (alongside `NoValidRecords`)
- Unrecoverable mid-file → exit 3 (alongside `UnrecoverableSyncLoss`)
- Generic record / I/O / writer → exit 1 (the default branch)

Add the explicit handler and update the corresponding `decode exit class:` log line.

### Documentation

1. Update `docs/ERROR-CATALOG.md`:
   - Add a row in section 3 (file-level) or section 4 (record-level).
   - If the error introduces new operator-visible behavior, add to the decision tree in section 9.
2. Tag any new tests with the requirement ID and regenerate the trace matrix.
3. If the variant pins a NEW requirement (rather than implementing an existing one), add the L2 / L3 to the spec docs first.

### Cross-impl alignment

Both crates **must** raise the same variant for the same input. Add a conformance fixture (negative case, `expected_exit`) if the exit-code class is new, so future drift is caught.

---

## 8. Adding a CLI flag

The Rust and Python CLIs differ in syntax (per L1-CLI-001) but must offer the same capabilities. If the new flag enables an L1-CLI-001 capability, both must implement it.

### Rust (`src/cli.rs`)

The CLI argparse is hand-rolled. Add the flag in the relevant subcommand's parser. Wire it into the appropriate path (`run_decode`, `run_count`, `run_dump`). Add `parse_*` unit tests for the new flag (greedy / non-greedy / repeats / `=value`) following the existing `filter_flag_*` pattern.

### Python (`python/src/mie_decoder/cli.py`)

Add an `argparse` argument to the relevant subparser. Wire it the same way.

### Config schema

If the flag has a TOML counterpart (which it usually should for site-wide config), update `config/default.toml` with the new key (commented out, with a description), the L2-CFG-008 schema reference in `docs/L2-REQ.md`, and both `config.rs` / `config.py` to load and validate the key.

### Tests

- Per-impl unit tests that the flag is parsed correctly.
- Per-impl end-to-end test that the flag changes behavior.
- A conformance fixture if the resulting behavior is cross-impl visible (typically yes).

---

## 9. CI architecture

`.github/workflows/ci.yml` has five jobs:

| Job | What it gates | Platforms | Failure cost |
|-----|---------------|-----------|--------------|
| `rust` | `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test --all-targets` (unit + `tests/integration.rs` + `tests/cli.rs` CLI acceptance suite — see section 5 for the test pyramid); `cargo cov-ci` (70% line + region coverage floors) Linux-only | `ubuntu-latest`, `windows-latest` | Block merge |
| `python` | `poetry sync` + `poetry run pytest`; `poetry check --strict --lock` + `poetry build` Linux/3.12-only | 5 versions × Linux (3.10–3.14), 2 versions × Windows (3.12, 3.14) | Block merge |
| `conformance` | `pip install -e ./python` then `python tests/conformance/run.py` — every fixture, both impls | `ubuntu-latest`, `windows-latest` | Block merge |
| `trace-matrix` | `python scripts/build-trace-matrix.py --check` — fails if `docs/TRACE-MATRIX.md` is stale relative to the spec docs + test markers | `ubuntu-latest` | Block merge |
| `diagrams` | Re-render every `docs/diagrams/*.puml` with the pinned PlantUML version and `git diff --exit-code` against the committed `*.svg` — fails if a `.puml` source was changed without regenerating the matching `.svg` | `ubuntu-latest` | Block merge |

The Rust and Python deployment targets are Linux (`x86_64-unknown-linux-musl` for Rust; current SLES 12 / RHEL flavors for Python). Windows cells exist to catch path / encoding / line-ending portability bugs early, not because Windows is a production target. Coverage gate, lockfile-and-metadata check, and dist build run on Linux only — Windows is functional smoke.

The `diagrams` job pins PlantUML to the version that produced the committed SVGs (read the `<?plantuml VERSION?>` processing instruction inside any `docs/diagrams/*.svg` to find it). Bumping that pin generally reflows every diagram and requires a matching local re-render + commit of all `*.svg` files in the same PR.

Pre-commit hooks (set up locally via `bash scripts/install-hooks.sh`, which points `core.hooksPath` at `.githooks/`) run a subset of the above on staged content: trailing-whitespace / CRLF / merge-marker scans, Cargo.lock parity, `python scripts/build-trace-matrix.py --check` (whenever Rust source, Python tests, the L1/L2/L3 docs, or the matrix itself are staged), `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --all-targets`, a `dbg!()` scan in staged Rust, and a `// SAFETY:` comment requirement for new `unsafe` blocks. These mirror what CI checks so push-fails are rare. The pre-commit hooks do **not** regenerate diagrams or rebuild SVGs — the `diagrams` CI job is your safety net there.

---

## 10. Coverage workflow

Rust uses `cargo-llvm-cov`. The CI gate is `cargo cov-ci` (alias defined in `.cargo/config.toml`) which fails if line OR region coverage falls below the floors (currently 70/70).

### Local coverage check

```bash
cargo cov-ci         # what CI runs
cargo cov            # interactive HTML report
```

### Ratcheting the floor

When coverage is consistently above the floor by >2pp, bump it. Edit `Cargo.toml` (or wherever the floor is defined) and re-run.

Python doesn't currently have a coverage gate in CI. The 220+ tests provide effective coverage at the integration level; adding a coverage tool would be a separate decision.

---

## 11. Releasing

### Rust crate

The production target is `x86_64-unknown-linux-musl` (static linking, runs on SLES 12 / glibc 2.22):

```bash
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
```

The resulting binary in `target/x86_64-unknown-linux-musl/release/mie-decoder` is self-contained.

For a Cargo.io publish (if applicable), bump version in `Cargo.toml`, tag, and `cargo publish`.

### Python package

```bash
poetry -C python check --strict --lock
poetry -P python build
```

This produces `python/dist/mie_decoder-<version>.tar.gz` and `mie_decoder-<version>-py3-none-any.whl`.

For a PyPI publish, `poetry publish` (with credentials).

### Version coordination

**v1.0.0 is a joint release** — both implementations ship together at v1.0.0 from a single repository tag (`v1.0.0`). Subsequent releases may diverge in version, but the cross-implementation conformance contract (CSV byte-for-byte equivalence on shared behavior) holds at any compatible version pair.

Tagging scheme:

- **`v1.0.0`** — single tag for the v1.0.0 joint cut. Used because both impls ship simultaneously from one commit.
- **`rust-vX.Y.Z` / `python-vX.Y.Z`** — impl-prefixed tags for future divergent releases. Avoid SemVer-style suffix tags like `v1.0.0-rust` because the hyphen marks a pre-release identifier and tools treat such tags as *less than* `v1.0.0`.

Bump versions when:

- **Rust (`Cargo.toml`)** — any change to the public crate API, the CLI surface, or the on-disk output.
- **Python (`python/pyproject.toml` and `python/src/mie_decoder/__init__.py::__version__`)** — same axes for the Python package. Keep `pyproject.toml` and `__init__.py` in lockstep; `poetry check --strict --lock` (run by the `python` CI cell on Linux/3.12) catches `pyproject.toml`/`poetry.lock` drift but does not verify the `__init__.py` constant.

Add a new top-of-file entry to `CHANGELOG.md` in the same commit that bumps the version. The CHANGELOG entry, the version bump, and any user-visible behavior changes all land together so a tag points at a coherent unit of release.

---

## 12. Cross-implementation alignment principles

These are the operating rules that keep the two crates from drifting:

1. **Spec first.** New behavior lands as an L2 / L3 requirement before code. Both implementations then satisfy it.
2. **Conformance fixtures for cross-impl behavior.** Anything that affects CSV output or exit codes belongs in `tests/conformance/`.
3. **Per-impl detail goes in L3.** Python-specific constraints (pandas, tomllib, Poetry) live as `L3-PY-*`. Rust-specific constraints (memmap2, BufWriter, musl) live as `L3-RS-*`. The shared L2 stays implementation-agnostic.
4. **Error variants ship together.** When you add a new variant in one language, add it in the other in the same PR.
5. **Log message wording can drift.** Operators read CSV output and exit codes; log message text isn't part of the contract. Don't over-coordinate it.
6. **CLI syntax can drift** (per L1-CLI-001) — capability parity matters, exact spelling doesn't. Rust uses `count` subcommand and `--inline-errors`; Python uses `decode --count` and `--error-mode inline`. Both reach the same outcomes.
7. **`memmap2` is the only Rust runtime dep.** Argument parsing, CSV writing, TOML loading, logging, and error types are all hand-rolled. Adding a crate requires explicit justification — see `docs/ROADMAP.md` and `CLAUDE.md` "Conventions worth preserving".

---

## 13. Quick links

- [`CLAUDE.md`](../CLAUDE.md) — project conventions for LLM sessions
- [`docs/ARCHITECTURE.md`](ARCHITECTURE.md) — module-level architecture
- [`docs/ERROR-CATALOG.md`](ERROR-CATALOG.md) — every error / exit code
- [`docs/L1-REQ.md`](L1-REQ.md) / [`L2-REQ.md`](L2-REQ.md) / [`L3-REQ.md`](L3-REQ.md) — spec
- [`docs/TRACE-MATRIX.md`](TRACE-MATRIX.md) — auto-generated trace matrix
- [`tests/conformance/README.md`](../tests/conformance/README.md) — conformance suite manifest schema
- [`config/default.toml`](../config/default.toml) — fully-commented reference TOML
