# Contributing to MIE-Decoder

Thanks for working on MIE-Decoder. This repository contains maintained Rust
and Python implementations. This document covers local setup, the pre-commit
workflow, and commit conventions.

> Note: the canonical filename for this document is `CONTRIBUTING.md`
> (Git/GitHub convention). If you arrive here looking for `CONTRIBUTION.md`,
> this is the same file.

## Prerequisites

- Rust toolchain ≥ 1.85 (`rustup toolchain install stable`). The crate
  uses edition 2024.
- Python 3.10 or newer and Poetry for work under `python/`.
- A Bash shell. On Windows, Git for Windows ships **Git Bash**, which
  Git invokes for hooks transparently — no extra setup.

## One-time setup: install the pre-commit hook

The repo carries a pre-commit hook at `.githooks/pre-commit`. Activate
it on your clone with:

```bash
bash scripts/install-hooks.sh
```

This sets `core.hooksPath` to `.githooks/` and marks the hook
executable. You only do this once per clone.

To verify:

```bash
git config core.hooksPath
# → .githooks
```

The equivalent direct command (if you'd rather not run the script):

```bash
git config core.hooksPath .githooks
```

## What the hook checks

On every `git commit`, the hook runs (in order, failing fast). The
checks are split into a file-level group that runs on every commit
and a Rust group that runs only when `.rs` or `.toml` files are
staged.

### File-level (always)

1. **Whitespace + missing-final-newline** — `git diff --cached --check`.
   Reports `file:line` for trailing whitespace, missing trailing
   newline, and (as a side-effect) leftover merge conflict markers.
2. **CRLF line endings** — staged text files must be LF-only.
   Belt-and-suspenders alongside `.gitattributes` if you add one.
3. **Merge conflict markers** — explicit scan for `<<<<<<<`, `=======`,
   `>>>>>>>` in staged blobs. (`git diff --cached --check` also
   catches these; the dedicated check is in case `--check` is ever
   bypassed for one path.)
4. **Large file guard** — staged files over 1 MB are rejected.
   Catches accidental binary commits (`git add -f` on a `*.mie`
   recording, etc.). Use git-lfs or extend `.gitignore` if you
   genuinely need a large file.
5. **`*.mie` recordings** — defense-in-depth on top of `.gitignore`.
   Sample binaries shouldn't be committed.
6. **`Cargo.lock` parity** — if `Cargo.toml` is staged, `Cargo.lock`
   must also be staged (or already match). Catches the common
   "bumped a dep version, forgot to commit the lock update" mistake.
   Uses `cargo metadata --locked --offline` to confirm.
7. **`shellcheck` on hooks/scripts** — runs only if `shellcheck` is
   installed. Lints the hook itself and `scripts/*.sh`. Skipped
   silently if the tool isn't on `$PATH`.

### Rust-only (skipped if no `.rs`/`.toml` staged)

8. **`cargo fmt --check`** — formatting is consistent. Fix locally
   with `cargo fmt`, then re-stage.
9. **`cargo clippy --all-targets -- -D warnings`** — all clippy lints
   pass with warnings treated as errors. Either fix the lint or
   justify the suppression with a scoped `#[allow(...)]` and a
   comment explaining why.
10. **`cargo test --all-targets`** — all unit + integration tests
    pass.
11. **`dbg!()` scan** — staged `.rs` files do not contain forgotten
    `dbg!` macros. (`todo!` and `unimplemented!` are sometimes
    intentional placeholders, so they're not blocked — but you'll
    see them in code review.)
12. **`unsafe` blocks require `// SAFETY:`** — every `unsafe { ... }`
    or `unsafe fn` in a staged `.rs` file must have a comment
    containing `SAFETY:` within the three preceding lines. Catches
    new unsafe code added without justifying its invariants.

If only docs are staged (no `.rs` or `.toml` files), the cargo group
is skipped — doc-only commits are fast.

### A note on `unwrap()` / `expect()`

We don't currently grep for `.unwrap()` calls in pre-commit. There
are two reasons:

1. **False-positive heavy in tests.** Test code legitimately uses
   `unwrap()` because panic-on-failure *is* the desired behavior.
2. **Better tool exists.** The clippy lints `clippy::unwrap_used`
   and `clippy::expect_used` flag every call and force a per-site
   `#[allow(clippy::unwrap_used)]` annotation, which doubles as
   documentation of *why* the unwrap is safe.

Production crates enable both lints outside `cfg(test)`. Test code may
continue to use `unwrap()` / `expect()` because panic-on-failure is the
intended assertion behavior. New production uses must be rewritten to
return a defensive error or carry a narrow documented lint allowance.

### Bypassing the hook

`git commit --no-verify` skips the hook. Reserve this for genuine
emergencies; CI runs the same checks and will fail the merge anyway.

### Why these checks (and not others)

- `cargo doc` is **not** in the hook because it's slow and rarely
  catches issues clippy doesn't already catch. Worth running manually
  before publishing a release.
- `cargo build --release` is **not** in the hook because debug builds
  exercise the same code path. Release builds are a release-time concern.
- `cargo audit` (CVE check) is **not** wired up because we have a
  single dependency. Revisit if the dep tree grows.

## Daily commands

Rust:

```bash
# Build
cargo build               # Dev
cargo build --release     # Optimized

# Test
cargo test                                                # Everything
cargo test --lib                                          # Unit tests only
cargo test --test integration                             # Integration only
cargo test --test integration -- multi_record_stream      # Single integration test
cargo test config::tests::parses_default_toml_from_disk   # Single unit test

# Format / lint
cargo fmt                                  # Auto-format
cargo fmt --check                          # CI-style check (no rewrites)
cargo clippy --all-targets -- -D warnings  # Lint manually
```

Python:

```bash
poetry -C python sync
poetry -C python run pytest
poetry -C python run mie-decoder --help
poetry -P python build
```

Shared Rust/Python conformance:

```bash
python tests/conformance/run.py
```

The conformance runner materializes text-based hexadecimal fixtures, invokes
both CLIs, and compares their CSV output byte-for-byte against checked-in
oracles. Use `--update-expected` only for intentional shared-output changes;
the runner updates an oracle only after Rust and Python already agree.

The current pre-commit hook runs the Rust checks documented above. Run the
Python tests manually when changing `python/`.

## Continuous integration

GitHub Actions runs [`.github/workflows/ci.yml`](.github/workflows/ci.yml) on
every push and pull request:

- **Rust:** `cargo fmt --check`, Clippy with warnings denied, all-target tests,
  and the `cargo cov-ci` 70% line + region coverage gate.
- **Python 3.10 through 3.14:** locked dependency synchronization and the full
  pytest suite on every supported minor version.
- **Python 3.12:** strict package/lockfile validation and wheel + source
  distribution builds.
- **Rust/Python conformance:** both CLIs decode the shared fixtures and must
  produce byte-identical CSV matching the checked-in oracles.

The Python matrix makes the `>=3.10,<3.15` compatibility declaration
enforceable. In particular, Python 3.10 exercises the `tomli` compatibility
path while newer versions use the standard-library `tomllib`.

## Coverage

We use [`cargo-llvm-cov`](https://github.com/taiki-e/cargo-llvm-cov)
for source-based code coverage. It works on stable Rust (no nightly
needed — `-C instrument-coverage` has been stable since Rust 1.60).

### One-time install

```bash
rustup component add llvm-tools-preview
cargo install cargo-llvm-cov
```

### Daily use

Three cargo aliases are pre-wired in `.cargo/config.toml`:

```bash
cargo cov         # Local: build instrumented, run tests, open HTML report
cargo cov-lcov    # Generate target-relative lcov.info (for IDE / CI tooling)
cargo cov-ci      # Enforced gate: --fail-under-lines 70 --fail-under-regions 70
```

Or via the script wrapper, equivalent to `cargo cov`:

```bash
bash scripts/coverage.sh
```

### Thresholds

`cargo cov-ci` enforces:

- **Lines: 70%** floor
- **Regions: 70%** floor

These were set with ~5 percentage points of headroom below the
integration-time baseline (74.81% / 71.55%) so routine refactors
don't trip the gate. Ratchet up by editing the `cov-ci` alias if
we want stricter — do it in increments after watching baseline
readings stabilize.

### Why coverage is NOT in the pre-commit hook

Building an instrumented test binary takes much longer than a normal
`cargo test`. The pre-commit hook is meant to be fast (under a few
seconds for a small change). CI enforces `cargo cov-ci` on every push
and pull request; run it locally before pushing material changes.

### Line-level exclusions

cargo-llvm-cov supports file-level exclusion via `--ignore-filename-regex`
(used in our aliases to skip the binary entry shim). **Line-level
exclusion** (the `#[coverage(off)]` attribute) is **nightly-only** at
present, so `unreachable!()` arms and other defensive branches show as
uncovered on stable. Either accept the percentage hit or refactor the
defensive arm out — don't try to game the threshold.

## Commit conventions

We use [Conventional Commits](https://www.conventionalcommits.org/)
prefixes:

| Prefix | Use for |
|--------|---------|
| `feat(<scope>):` | New feature or capability |
| `fix(<scope>):` | Bug fix |
| `chore:` | Version bumps, repo hygiene, non-code maintenance |
| `docs:` | Documentation only |
| `test:` | Test changes only |
| `build:` | Build system / dependency changes |
| `refactor:` | Code change without behavior change |

Examples from this repo:

- `feat(reader): port mmap-backed iterator with sync recovery`
- `fix(reader): apply full sync::validate_record path per record`
- `docs(roadmap): catalogue robustness corner cases for future work`
- `chore: rename Python project directory to python/`

Body conventions:

- Lead with the **why**, not the **what** — the diff already shows
  what.
- Wrap at ~72 columns.
- Co-author trailers (e.g., from pair programming) at the bottom.

## Code conventions worth preserving

These are codified in `CLAUDE.md`; the highlights:

- **Single external dependency.** Only `memmap2`. Adding crates
  requires justification — argument parsing, CSV, TOML, logging,
  error types are all hand-rolled by design.
- **Streaming CSV.** Don't introduce `Vec<MieMessage>` or row-level
  buffering in `writer.rs` — constant memory is the design point.
- **`DataWords` is fixed-capacity.** MIL-STD-1553B caps a transaction
  at 32 data words. Don't switch to `Vec<u16>` "for flexibility."
- **Two-record look-ahead in `sync.rs`.** Removing it reintroduces
  false-positive resyncs.
- **One validation path.** Header skip, normal forward decode, and
  recovery all share `sync::validate_record`. There is no weaker
  fast path.
- **`sync.rs` is pure.** No logging, no I/O. The reader handles
  user-facing diagnostics based on returned values.
- **CSV columns match DDC vendor output byte-for-byte.** Don't reorder
  or rename, including currently-empty columns
  (`MUX`, `TERM_NAME`, `IM_GAP`, `RCV_GAP`, `XMT_GAP`).
- **Test fixtures are byte-exact** translations of records cross-
  referenced against vendor CSV. Treat them as oracles; if a test
  fails, suspect the code first.
- **Both implementations are maintained.** Changes under `src/` and `tests/`
  apply to Rust; changes under `python/` apply to Python. Preserve shared MIE
  format semantics and vendor-compatible CSV behavior across both.

## Reporting issues / proposing changes

For non-trivial changes:

1. Open an issue describing the change first.
2. If it touches a known robustness gap, link to the entry in
   `docs/ROADMAP.md` ("Robustness & validation backlog" section).
3. Keep the PR focused — one feat/fix per PR.

For trivial doc fixes or single-line bug fixes, a PR without a prior
issue is fine.
