#!/usr/bin/env bash
# scripts/coverage.sh — local coverage convenience wrapper.
#
# Equivalent to running `cargo cov` directly (the alias defined in
# .cargo/config.toml). Forwards any extra arguments through to
# cargo-llvm-cov, e.g.:
#
#     bash scripts/coverage.sh                  # HTML, opens browser
#     bash scripts/coverage.sh --no-clean       # keep prior counters
#
# For the CI-style gated run:   cargo cov-ci
# For lcov.info output:         cargo cov-lcov

set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
exec cargo cov "$@"
