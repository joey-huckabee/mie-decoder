#!/usr/bin/env bash
# scripts/repo-hygiene.sh — the CI backstop for the pre-commit hook's
# file-level checks.
#
# `.githooks/pre-commit` inspects *staged blobs*, which makes it fast but also
# skippable: `git commit --no-verify` bypasses every one of its checks. Until
# v2.11.2 nothing in CI re-ran them, so CONTRIBUTING.md's "CI runs the same
# checks and will fail the merge anyway" was untrue for most of the list — a
# CRLF file, a stray `dbg!()`, a merge marker, an oversized blob or a committed
# `*.mie` recording could all land unnoticed.
#
# This script re-runs those checks over the whole *tracked tree* rather than a
# diff, so it catches anything already committed, however it got there. The
# hook stays staged-based (it must be fast); this is the safety net.
#
# Usage:  bash scripts/repo-hygiene.sh
# Exit:   0 all clean, 1 one or more checks failed (all are reported, not just
#         the first — a contributor should see the whole list in one run).

set -uo pipefail
cd "$(git rev-parse --show-toplevel)"

RED=$'\033[31m'; GREEN=$'\033[32m'; YELLOW=$'\033[33m'; RESET=$'\033[0m'
[[ -t 2 ]] || { RED=''; GREEN=''; YELLOW=''; RESET=''; }

failures=0
step() { printf '%shygiene:%s %s\n' "$YELLOW" "$RESET" "$1" >&2; }
bad()  { printf '%shygiene: FAIL%s %s\n' "$RED" "$RESET" "$1" >&2; failures=$((failures + 1)); }
list() { printf '  %s\n' "$@" >&2; }

# Tracked files, NUL-safe. Binary-ish suffixes are skipped by the text checks.
mapfile -d '' -t TRACKED < <(git ls-files -z)

is_binary() {
    case "$1" in
        *.png|*.jpg|*.jpeg|*.gif|*.ico|*.pdf|*.bin|*.mie|*.svg) return 0 ;;
        *) return 1 ;;
    esac
}

# ── 1. Final newline ──────────────────────────────────────────────────
step "every tracked text file ends with a newline"
offenders=()
for f in "${TRACKED[@]}"; do
    is_binary "$f" && continue
    [[ -f "$f" ]] || continue
    [[ -s "$f" ]] || continue
    [[ $(tail -c 1 "$f" | od -An -tx1 | tr -d ' \n') == "0a" ]] || offenders+=("$f")
done
(( ${#offenders[@]} )) && { list "${offenders[@]}"; bad "missing final newline"; }

# ── 2. CRLF line endings ──────────────────────────────────────────────
# Inspect the *index* (`i/`) rather than the worktree. On Windows a checkout
# can legitimately hold CRLF while the committed blob is LF — reading the
# worktree would fail here and pass on the Linux runner, which is worse than
# not checking at all.
step "no CRLF line endings in committed blobs"
# `git ls-files --eol` prints "i/<eol> w/<eol> attr/<...>\t<path>"; the path is
# the only tab-separated field, so split on tab and test the index column.
offenders=()
while IFS= read -r path; do
    [[ -n "$path" ]] && offenders+=("$path")
done < <(git ls-files --eol | awk -F'\t' '$1 ~ /^i\/crlf/ {print $2}')
(( ${#offenders[@]} )) && { list "${offenders[@]}"; bad "CRLF in committed content; convert to LF"; }

# ── 3. Merge conflict markers ─────────────────────────────────────────
# This script itself contains the pattern in a string, so exclude it.
step "no merge conflict markers"
offenders=()
for f in "${TRACKED[@]}"; do
    is_binary "$f" && continue
    [[ -f "$f" ]] || continue
    [[ "$f" == "scripts/repo-hygiene.sh" ]] && continue
    grep -qE '^(<{7}|={7}|>{7})( |$)' "$f" && offenders+=("$f")
done
(( ${#offenders[@]} )) && { list "${offenders[@]}"; bad "merge conflict markers committed"; }

# ── 4. Large files (> 1 MB) ───────────────────────────────────────────
step "no tracked file over 1 MB"
LIMIT=$((1024 * 1024))
offenders=()
for f in "${TRACKED[@]}"; do
    [[ -f "$f" ]] || continue
    size=$(wc -c <"$f")
    (( size > LIMIT )) && offenders+=("$f ($size bytes)")
done
(( ${#offenders[@]} )) && { list "${offenders[@]}"; bad "file over 1 MB; use git-lfs or .gitignore"; }

# ── 5. No *.mie recordings ────────────────────────────────────────────
# Fixtures are reviewable hex under tests/conformance/inputs, never binaries.
step "no *.mie recordings tracked"
offenders=()
for f in "${TRACKED[@]}"; do
    [[ "$f" == *.mie ]] && offenders+=("$f")
done
(( ${#offenders[@]} )) && { list "${offenders[@]}"; bad "*.mie recording committed"; }

# ── 6. Cargo.lock parity ──────────────────────────────────────────────
step "rust/Cargo.lock version matches rust/Cargo.toml"
toml_ver=$(awk '/^\[package\]/{p=1;next} /^\[/{p=0} p && /^version *=/{gsub(/[" ]/,""); sub(/version=/,""); print; exit}' rust/Cargo.toml)
lock_ver=$(awk '/^name = "mie-decoder"$/{getline; gsub(/[" ]/,""); sub(/version=/,""); print; exit}' rust/Cargo.lock)
if [[ -z "$toml_ver" || -z "$lock_ver" ]]; then
    bad "could not read version from Cargo.toml ($toml_ver) / Cargo.lock ($lock_ver)"
elif [[ "$toml_ver" != "$lock_ver" ]]; then
    bad "Cargo.toml is $toml_ver but Cargo.lock says $lock_ver — run \`cargo check\`"
fi

# ── 7. No forgotten dbg!() ────────────────────────────────────────────
step "no dbg!() in Rust sources"
offenders=()
while IFS= read -r hit; do offenders+=("$hit"); done < <(
    grep -rn 'dbg!' rust/src rust/tests 2>/dev/null || true
)
(( ${#offenders[@]} )) && { list "${offenders[@]}"; bad "dbg!() left in Rust source"; }

# ── 8. unsafe blocks carry a // SAFETY: comment ───────────────────────
# Detection mirrors `.githooks/pre-commit` exactly — `unsafe` at the start of
# a line, with a SAFETY note in the 3 lines above. Deliberately identical
# rather than stricter: a backstop that rejects what the hook accepts would
# just move the surprise from commit time to merge time.
#
# Known limitation, shared with the hook: a mid-line `unsafe` (as in
# `let m = unsafe { ... }`) is not detected by either. Today's only such site,
# reader.rs, does carry a SAFETY note.
step "every line-leading unsafe block has a // SAFETY: comment"
offenders=()
while IFS=: read -r file line _; do
    [[ -z "${file:-}" ]] && continue
    start=$(( line > 3 ? line - 3 : 1 ))
    if ! sed -n "${start},${line}p" "$file" | grep -q 'SAFETY:'; then
        offenders+=("$file:$line")
    fi
done < <(grep -rnE '^[[:space:]]*unsafe[[:space:]]*([{(]|fn[[:space:]])' rust/src rust/tests 2>/dev/null || true)
(( ${#offenders[@]} )) && { list "${offenders[@]}"; bad "unsafe block without a // SAFETY: comment"; }

# ── 9. CI job table in MAINTAINER-GUIDE matches ci.yml ────────────────
# §9 of the guide documents every CI job in a table. That table stayed
# accurate while the prose above it did not ("seven jobs" when there were
# fifteen), so the number is gone and the table is the list — which only works
# if something keeps it honest. Adding a job without a row now fails here.
step "MAINTAINER-GUIDE §9 lists every ci.yml job"
mapfile -t yml_jobs < <(
    awk '/^jobs:/{inj=1;next} inj && /^  [a-z0-9-]+:/{gsub(/[ :]/,"");print}' \
        .github/workflows/ci.yml
)
missing_rows=()
for job in "${yml_jobs[@]}"; do
    grep -qF "| \`$job\` |" docs/MAINTAINER-GUIDE.md || missing_rows+=("$job")
done
if (( ${#missing_rows[@]} )); then
    list "${missing_rows[@]}"
    bad "ci.yml job(s) with no row in MAINTAINER-GUIDE.md section 9"
fi

# ── 10. Config key set agrees across its three text sources ───────────
# CONFIG-REFERENCE.md calls itself the reference for *every* TOML key, and
# states each one twice: a copyable quick-reference block and a normative
# table. config/default.toml is the third copy. merge.delta_scope shipped in
# v2.11.0 into two of the three, which is exactly the drift this catches.
# (The two loaders are the fourth and fifth copies; they are code, and their
# own parity is covered by tests/conformance/config_parity.py.)
step "config keys agree: CONFIG-REFERENCE block, table, and default.toml"
if ! python - <<'PY'
import re, sys, pathlib

def block_keys(text):
    sec, keys = None, set()
    for line in text.splitlines():
        s = line.strip()
        m = re.match(r'^\[([a-z_]+)\]', s)
        if m:
            sec = m.group(1)
            continue
        m = re.match(r'^#?\s*([a-z_]+)\s*=', s)
        if m and sec:
            keys.add(f"{sec}.{m.group(1)}")
    return keys

doc = pathlib.Path("docs/CONFIG-REFERENCE.md").read_text(encoding="utf-8")
quick = block_keys(doc.split("## Quick reference", 1)[1].split("```")[1])
table = set(re.findall(r'^\| `([a-z_]+\.[a-z_]+)` \|', doc, re.M))
default = block_keys(pathlib.Path("config/default.toml").read_text(encoding="utf-8"))

sources = {"quick-reference block": quick, "normative table": table,
           "config/default.toml": default}
union = set().union(*sources.values())
bad = False
for key in sorted(union):
    absent = [n for n, s in sources.items() if key not in s]
    if absent:
        bad = True
        print(f"  {key} missing from: {', '.join(absent)}", file=sys.stderr)
sys.exit(1 if bad else 0)
PY
then
    bad "config key set differs between CONFIG-REFERENCE.md and config/default.toml"
fi

# ── Summary ───────────────────────────────────────────────────────────
if (( failures )); then
    printf '%shygiene: %d check(s) failed%s\n' "$RED" "$failures" "$RESET" >&2
    exit 1
fi
printf '%shygiene: all checks passed%s\n' "$GREEN" "$RESET" >&2
