"""Differential config-parser parity check across the Rust and Python CLIs.

The two implementations parse TOML with structurally different parsers — Python
uses the full-TOML ``tomllib``; Rust uses a minimal hand-rolled parser for the
flat ``[section]`` + ``key = value`` schema. Aligning them one divergent form at
a time (a blacklist) does not converge, so this module drives a fixed corpus of
config snippets through *both* CLIs and asserts they land in the same class —
either both **accept** (exit 0) or both **reject** (non-zero config/usage error)
— and that the class matches the schema's intent.

Run automatically by ``run.py`` when both implementations are under test. A
divergence here is the systematic signal the manual per-bug conformance cases
were catching reactively.
"""

from __future__ import annotations

import subprocess
from pathlib import Path

# ``accept`` = a valid config for the flat schema (both CLIs decode, exit 0).
# ``reject`` = outside the flat schema (both CLIs must refuse with a config or
# usage error). Every ``reject`` snippet is a full-TOML form ``tomllib`` accepts
# but the schema does not — the class this corpus exists to keep aligned.
CORPUS: list[tuple[str, str, str]] = [
    # ── valid flat forms (accept) ──────────────────────────────────────────
    ("flat-strict", "[decode]\nstrict = true\n", "accept"),
    ("comment-only", "# just a comment\n", "accept"),
    ("empty-file", "", "accept"),
    ("string-value", '[decode]\ntime_format = "irig"\n', "accept"),
    ("int-value", "[decode]\ndetect_records = 8\n", "accept"),
    ("float-value", "[decode]\nstandard_tick_rate_hz = 1000000.0\n", "accept"),
    ("bool-value", "[output]\nno_clobber = true\n", "accept"),
    ("int-array", "[filter]\nexclude_rts = [0, 31]\n", "accept"),
    ("string-array", '[filter]\nexclude_buses = ["A", "B"]\n', "accept"),
    ("trailing-comment", "[decode]\nstrict = true  # yes\n", "accept"),
    ("extra-whitespace", "[decode]\n  strict   =   true  \n", "accept"),
    ("blank-lines", "[decode]\n\n\nstrict = true\n", "accept"),
    ("negative-mux-field", "[mux]\nfield = -1\n", "accept"),
    # ── outside the flat schema (reject on both) ───────────────────────────
    ("inline-table", "[decode]\nx = { a = 1 }\n", "reject"),
    ("multiline-array", "[filter]\nexclude_rts = [\n  1,\n]\n", "reject"),
    ("int-underscores", "[decode]\ndetect_records = 1_0\n", "reject"),
    ("hex-int", "[decode]\ndetect_records = 0x08\n", "reject"),
    ("octal-int", "[decode]\ndetect_records = 0o10\n", "reject"),
    ("binary-int", "[decode]\ndetect_records = 0b1000\n", "reject"),
    ("datetime-value", "[decode]\nx = 1979-05-27\n", "reject"),
    # `1e6` is a plain float both parsers accept — kept as an `accept` guard that
    # the whitelist must not over-reject scientific notation.
    ("exponent-float", "[decode]\nstandard_tick_rate_hz = 1e6\n", "accept"),
    ("dotted-key", "decode.strict = true\n", "reject"),
    ("dotted-key-in-section", "[decode]\nfoo.bar = 1\n", "reject"),
    ("dotted-header", "[output.no_clobber]\nenabled = true\n", "reject"),
    ("array-of-tables", "[[decode]]\nstrict = true\n", "reject"),
    ("duplicate-key", "[decode]\nstrict = true\nstrict = false\n", "reject"),
    (
        "duplicate-section",
        "[decode]\nstrict = true\n[decode]\nallow_partial = true\n",
        "reject",
    ),
    ("section-as-scalar", "decode = true\n", "reject"),
    ("non-string-enum", "[decode]\ntime_format = 1\n", "reject"),
    ("quoted-key", '[decode]\n"stri.ct" = true\n', "reject"),
    ("trailing-after-header", "[decode] junk\nstrict = true\n", "reject"),
    ("unterminated-section", "[decode\nstrict = true\n", "reject"),
    ("empty-section-name", "[]\nstrict = true\n", "reject"),
    # ── numeric literals Rust's native i64/f64 accept but TOML rejects ──────
    ("leading-zero-int", "[decode]\ndetect_records = 08\n", "reject"),
    ("bare-trailing-dot", "[decode]\nstandard_tick_rate_hz = 1.\n", "reject"),
    ("leading-zero-in-array", "[filter]\nexclude_rts = [01]\n", "reject"),
    ("zero-then-zero", "[decode]\ndetect_records = 00\n", "reject"),
    # ── string escapes: only \" \\ \n \t are supported on both ─────────────
    ("escaped-quote-string", '[mux]\ndelimiter = "\\""\n', "accept"),
    ("carriage-return-escape", '[mux]\ndelimiter = "\\r"\n', "reject"),
    ("unicode-escape", '[mux]\ndelimiter = "\\u002C"\n', "reject"),
]


def _class_for(returncode: int) -> str:
    return "accept" if returncode == 0 else "reject"


def check_config_parser_parity(
    rust_bin: Path, python_bin: Path, root: Path, input_mie: Path, temp: Path
) -> None:
    """Drive ``CORPUS`` through both CLIs; raise on any divergence or mismatch.

    ``input_mie`` is a materialized, valid single-record recording so an accepted
    config decodes to exit 0. Only the config differs between snippets.
    """
    invocations = {
        "Rust": [str(rust_bin)],
        "Python": [str(python_bin), "-m", "mie_decoder"],
    }
    failures: list[str] = []
    for name, toml, expect in CORPUS:
        cfg = temp / f"parity-{name}.toml"
        cfg.write_text(toml, encoding="utf-8")
        classes: dict[str, str] = {}
        codes: dict[str, int] = {}
        for impl, prefix in invocations.items():
            out = temp / f"parity-{name}-{impl}.csv"
            command = [
                *prefix,
                "--config",
                str(cfg),
                "decode",
                str(input_mie),
                "-o",
                str(out),
            ]
            result = subprocess.run(
                command,
                cwd=root,
                capture_output=True,
                text=True,
                check=False,
                timeout=30,
            )
            codes[impl] = result.returncode
            classes[impl] = _class_for(result.returncode)
        if classes["Rust"] != classes["Python"]:
            failures.append(
                f"{name}: DIVERGENT — Rust {classes['Rust']} (exit {codes['Rust']}) "
                f"vs Python {classes['Python']} (exit {codes['Python']})"
            )
        elif classes["Rust"] != expect:
            failures.append(
                f"{name}: both {classes['Rust']}, expected {expect} "
                f"(Rust exit {codes['Rust']}, Python exit {codes['Python']})"
            )
    if failures:
        raise AssertionError(
            "config-parser parity failures:\n  " + "\n  ".join(failures)
        )
    print(f"PASS config-parser-parity ({len(CORPUS)} snippets)")
