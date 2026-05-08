#!/usr/bin/env bash
# scripts/install-hooks.sh — point Git at the version-controlled hook directory.
#
# Run once per clone:
#     bash scripts/install-hooks.sh
#
# Equivalent direct command:
#     git config core.hooksPath .githooks

set -euo pipefail

# Run from repo root regardless of where the script was invoked from.
cd "$(git rev-parse --show-toplevel)"

git config core.hooksPath .githooks

# Make hook scripts executable. On Windows file modes are tracked via
# git's index, so this also covers contributors on Linux/macOS.
chmod +x .githooks/* 2>/dev/null || true

echo "Git hooks installed:"
echo "  core.hooksPath = $(git config core.hooksPath)"
echo "  hooks: $(ls .githooks)"
