# Contributing to MIE-Decoder

Thanks for working on MIE-Decoder. This document covers local setup,
the pre-commit workflow, commit conventions, and how to produce the
SLES 12 production build.

> Note: the canonical filename for this document is `CONTRIBUTING.md`
> (Git/GitHub convention). If you arrive here looking for `CONTRIBUTION.md`,
> this is the same file.

## Prerequisites

- Rust toolchain ≥ 1.85 (`rustup toolchain install stable`). The crate
  uses edition 2024.
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

On every `git commit` with at least one `.rs` or `.toml` file staged,
the hook runs (in order, failing fast):

1. **`cargo fmt --check`** — formatting is consistent. Fix locally with
   `cargo fmt`, then re-stage.
2. **`cargo clippy --all-targets -- -D warnings`** — all clippy lints
   pass with warnings treated as errors. Either fix the lint or
   justify the suppression with a scoped `#[allow(...)]` and a comment
   explaining why.
3. **`cargo test --all-targets`** — all unit + integration tests pass.
4. **`dbg!()` scan** — staged `.rs` files do not contain forgotten
   `dbg!` macros. (`todo!` and `unimplemented!` are sometimes
   intentional placeholders, so they're not blocked — but you'll see
   them in code review.)

If only docs are staged (no `.rs` or `.toml` files), the hook exits
early — doc-only commits are fast.

### Bypassing the hook

`git commit --no-verify` skips the hook. Reserve this for genuine
emergencies; CI runs the same checks and will fail the merge anyway.

### Why these checks (and not others)

- `cargo doc` is **not** in the hook because it's slow and rarely
  catches issues clippy doesn't already catch. Worth running manually
  before publishing a release.
- `cargo build --release` is **not** in the hook because debug builds
  exercise the same code path. The musl static build is a release-time
  concern (see "Production build" below).
- `cargo audit` (CVE check) is **not** wired up because we have a
  single dependency. Revisit if the dep tree grows.

## Daily commands

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

## Production build (SLES 12 / static musl)

The deployment target is SLES 12 (glibc 2.22). Native release builds on
Windows/macOS/modern Linux **will not run there** because they link
against the host's newer glibc. Use the static-musl target instead:

```bash
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
# binary: target/x86_64-unknown-linux-musl/release/mie-decoder
```

The musl binary is statically linked, has no glibc dependency, and runs
on any x86_64 Linux. Intentionally separate from `cargo build --release`
so contributors don't pay the static-link cost on every build.

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
- `chore: relocate Python implementation to python-reference/`

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
- **Static musl is the production target.** Don't add anything that
  breaks `--target x86_64-unknown-linux-musl`.
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

## Reporting issues / proposing changes

For non-trivial changes:

1. Open an issue describing the change first.
2. If it touches a known robustness gap, link to the entry in
   `docs/ROADMAP.md` ("Robustness & validation backlog" section).
3. Keep the PR focused — one feat/fix per PR.

For trivial doc fixes or single-line bug fixes, a PR without a prior
issue is fine.
