"""Differential *fuzzer* for the Rust and Python config parsers.

The static ``config_parity.py`` corpus only tests forms a human enumerated —
which is exactly why config divergences kept being found one at a time. This
module instead *generates* many small TOML-ish config documents and drives each
through both CLIs, asserting they agree on accept vs reject. It searches the
edges (odd numeric literals, string escapes, exotic structures) so CI finds a
divergence before a reviewer does.

Deterministic by default (fixed seed + iteration count) so CI is reproducible;
override with the ``MIE_FUZZ_SEED`` / ``MIE_FUZZ_ITERS`` environment variables to
explore further locally. On a divergence it prints the exact config that
disagreed, ready to paste into ``config_parity.py`` as a pinned regression.
"""

from __future__ import annotations

import os
import random
import subprocess
from pathlib import Path

_DEFAULT_SEED = 20260711  # fixed → reproducible CI runs
# Each iteration spawns both CLIs, so keep the default modest for CI wall-clock;
# a fixed seed makes it a deterministic generated corpus. Bump MIE_FUZZ_ITERS
# locally to explore further.
_DEFAULT_ITERS = 100

# Section names: mostly real, sometimes junk / dotted / array-table forms.
_SECTIONS = [
    "decode",
    "output",
    "mux",
    "merge",
    "filter",
    "logging",
    "bogus",
    "output.no_clobber",  # dotted header
    "decode.foo",
]
# Keys: real identifiers plus a few that stress the key grammar.
_KEYS = [
    "strict",
    "time_format",
    "error_mode",
    "detect_records",
    "standard_tick_rate_hz",
    "no_clobber",
    "enabled",
    "delimiter",
    "field",
    "exclude_rts",
    "level",
    "unknown_key",
    "decode.strict",  # dotted key
    '"strict"',  # quoted key
]
# Values: valid forms mixed heavily with edge cases that have historically
# diverged (leading zeros, bare trailing dot, hex/oct/bin, underscores, string
# escapes, inline tables, datetimes, multi-line-array openers).
_VALUES = [
    "true",
    "false",
    "8",
    "-1",
    "0",
    "1000000.0",
    "1e6",
    '"auto"',
    '"irig"',
    '"."',
    "[0, 31]",
    '["A", "B"]',
    "[]",
    # edge numerics
    "08",
    "01",
    "1.",
    "0x08",
    "0o10",
    "0b1000",
    "1_000",
    "+.5",
    "00",
    # edge strings
    '"\\""',
    '"\\r"',
    '"\\u002C"',
    '"\\t"',
    '"unterminated',
    # exotic structures
    "{ a = 1 }",
    "1979-05-27",
    "[01]",
    "[\n1,\n]",
]


def _make_document(rng: random.Random) -> str:
    lines: list[str] = []
    for _ in range(rng.randint(0, 3)):
        if rng.random() < 0.08:
            lines.append(f"[[{rng.choice(_SECTIONS)}]]")  # array-of-tables
        else:
            lines.append(f"[{rng.choice(_SECTIONS)}]")
        for _ in range(rng.randint(0, 4)):
            key = rng.choice(_KEYS)
            val = rng.choice(_VALUES)
            sep = rng.choice(["=", " = ", "  =  "])
            trailing = rng.choice(["", "  # comment", " "])
            lines.append(f"{key}{sep}{val}{trailing}")
        if rng.random() < 0.15:
            lines.append(rng.choice(["", "# a comment", "   "]))
    return "\n".join(lines) + "\n"


def _class(returncode: int) -> str:
    return "accept" if returncode == 0 else "reject"


def check_config_parser_fuzz(
    rust_bin: Path, python_bin: Path, root: Path, input_mie: Path, temp: Path
) -> None:
    """Fuzz the two config parsers; raise on the first batch of divergences."""
    seed = int(os.environ.get("MIE_FUZZ_SEED", _DEFAULT_SEED))
    iters = int(os.environ.get("MIE_FUZZ_ITERS", _DEFAULT_ITERS))
    rng = random.Random(seed)
    invocations = {
        "Rust": [str(rust_bin)],
        "Python": [str(python_bin), "-m", "mie_decoder"],
    }
    divergences: list[str] = []
    for i in range(iters):
        doc = _make_document(rng)
        cfg = temp / f"fuzz-{i}.toml"
        cfg.write_text(doc, encoding="utf-8")
        classes: dict[str, str] = {}
        for impl, prefix in invocations.items():
            out = temp / f"fuzz-{i}-{impl}.csv"
            result = subprocess.run(
                [*prefix, "--config", str(cfg), "decode", str(input_mie), "-o", str(out)],
                cwd=root,
                capture_output=True,
                text=True,
                check=False,
                timeout=30,
            )
            classes[impl] = _class(result.returncode)
        if classes["Rust"] != classes["Python"]:
            divergences.append(
                f"Rust={classes['Rust']} Python={classes['Python']} for config:\n"
                + "    " + doc.replace("\n", "\n    ")
            )
            if len(divergences) >= 10:
                break
    if divergences:
        raise AssertionError(
            f"config-parser fuzz found {len(divergences)} divergence(s) "
            f"(seed={seed}, {iters} iterations):\n\n" + "\n".join(divergences)
        )
    print(f"PASS config-parser-fuzz ({iters} generated configs, seed {seed})")
