#!/usr/bin/env python3
"""Compare shared Rust and Python MIE decoding behavior."""

from __future__ import annotations

import argparse
import difflib
import json
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[2]
SUITE = Path(__file__).resolve().parent
MANIFEST = SUITE / "manifest.json"

# Closed schema for case objects. ``tests/conformance/README.md``
# specifies that unknown fields SHALL be rejected by the runner so a
# typo (e.g. ``arg`` instead of ``args``) cannot silently disable a
# per-case override. ``FIELD_TYPES`` doubles as the allowed-field set;
# keep it in lockstep with the schema table in the README.
#
# The Rust and Python CLIs share one argument surface, so a single
# ``args`` vector is passed verbatim to both — there is no per-impl
# argument translation.
FIELD_TYPES: dict[str, type | tuple[type, ...]] = {
    "name": str,
    "input": str,
    "inputs": list,  # multi-file merge: list of hex input paths
    "input_names": list,  # override materialized temp file name(s) (L2-WRT-020)
    "expected": str,
    "expected_errors": str,
    "expected_partial": str,  # oracle for the <output>.partial (allow-partial)
    "config": str,
    "mode": str,
    "args": list,
    "expected_stderr_contains": str,
    "expected_exit": int,
}
ALLOWED_MODES: frozenset[str] = frozenset({"decode", "count"})


def validate_case_schema(case: Any, index: int) -> None:
    """Reject malformed manifest cases with a clear, actionable error.

    Two failure classes are caught here so neither becomes a silent
    no-op at run time:

    1. **Unknown field name** — a misspelled key (e.g. ``arg``
       for ``args``) is otherwise ignored by ``case.get(...,
       default)`` and the case runs with default behavior.
    2. **Wrong field type** — e.g. ``"args": "single string"``
       (should be a list) would propagate downstream as a
       ``subprocess`` argument-list shape error far from the
       manifest entry that caused it.

    Fails fast on the first malformed case; rerun after fixing to
    see any subsequent ones.
    """
    if not isinstance(case, dict):
        raise RuntimeError(
            f"manifest case at index {index}: expected an object, "
            f"got {type(case).__name__}"
        )
    name = case.get("name") if isinstance(case.get("name"), str) else None
    label = repr(name) if name else f"at index {index}"
    if "name" not in case:
        raise RuntimeError(f"manifest case {label}: missing required 'name' field")
    # Exactly one of 'input' (single file) or 'inputs' (multi-file merge).
    has_input = "input" in case
    has_inputs = "inputs" in case
    if not (has_input or has_inputs):
        raise RuntimeError(
            f"manifest case {label}: missing required 'input' or 'inputs' field"
        )
    if has_input and has_inputs:
        raise RuntimeError(
            f"manifest case {label}: specify exactly one of 'input' or 'inputs', not both"
        )

    unknown = sorted(set(case) - set(FIELD_TYPES))
    if unknown:
        raise RuntimeError(
            f"manifest case {label}: unknown field(s) {unknown}. "
            f"Allowed fields are {sorted(FIELD_TYPES)}. "
            "Check tests/conformance/README.md for the schema."
        )

    for field, expected_type in FIELD_TYPES.items():
        if field not in case:
            continue
        if not isinstance(case[field], expected_type):
            type_name = (
                expected_type.__name__
                if isinstance(expected_type, type)
                else " or ".join(t.__name__ for t in expected_type)
            )
            raise RuntimeError(
                f"manifest case {label}: field {field!r} must be {type_name}, "
                f"got {type(case[field]).__name__}"
            )
        # list-typed fields must hold strings only — ``args`` ends up as
        # a CLI argument vector, where a non-string element would raise
        # far from the manifest entry.
        if expected_type is list:
            for i, item in enumerate(case[field]):
                if not isinstance(item, str):
                    raise RuntimeError(
                        f"manifest case {label}: field {field!r}[{i}] must be str, "
                        f"got {type(item).__name__}"
                    )

    if "mode" in case and case["mode"] not in ALLOWED_MODES:
        raise RuntimeError(
            f"manifest case {label}: mode {case['mode']!r} is not one of "
            f"{sorted(ALLOWED_MODES)}"
        )


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--rust-bin",
        type=Path,
        help="Use this Rust binary instead of rust/target/debug/mie-decoder.",
    )
    parser.add_argument(
        "--python-bin",
        type=Path,
        help="Use this Python interpreter instead of Poetry's environment.",
    )
    parser.add_argument(
        "--update-expected",
        action="store_true",
        help="Update CSV oracles, but only when Rust and Python outputs match.",
    )
    parser.add_argument(
        "--temp-root",
        type=Path,
        help="Create temporary files under this directory.",
    )
    return parser.parse_args()


def read_hex(path: Path) -> bytes:
    chunks: list[str] = []
    for line in path.read_text(encoding="utf-8").splitlines():
        chunks.append(line.split("#", 1)[0])
    return bytes.fromhex("".join(chunks))


def run_command(
    command: list[str],
    output: Path | None,
    case_name: str,
    implementation: str,
    expected_exit: int = 0,
    read_path: Path | None = None,
) -> tuple[bytes | None, str]:
    """Run one implementation's CLI and assert its exit code matches.

    Returns ``(payload, stderr)`` where ``payload`` is:
      - the CSV bytes from ``output`` when ``output`` is a path and
        ``expected_exit == 0`` (the historic decode-mode behavior);
      - the captured stdout bytes when ``output is None`` (used by the
        ``count`` mode, where stdout *is* the data being compared);
      - ``None`` for negative cases (no payload expected).
    ``stderr`` is always returned so call sites can run substring
    checks against the human-readable status lines.

    Raises RuntimeError on unexpected exit codes, command timeouts,
    or missing output.
    """
    print(f"RUN  {case_name} ({implementation})", flush=True)
    try:
        result = subprocess.run(
            command,
            cwd=ROOT,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            check=False,
            timeout=30,
        )
    except subprocess.TimeoutExpired as exc:
        rendered = subprocess.list2cmdline(command)
        raise RuntimeError(
            f"{case_name}: {implementation} exceeded 30 seconds\n"
            f"command: {rendered}\n"
            f"stdout:\n{exc.stdout or ''}\n"
            f"stderr:\n{exc.stderr or ''}"
        ) from exc
    if result.returncode != expected_exit:
        rendered = subprocess.list2cmdline(command)
        raise RuntimeError(
            f"{case_name}: {implementation} exited {result.returncode}, expected {expected_exit}\n"
            f"command: {rendered}\n"
            f"stdout:\n{result.stdout}\n"
            f"stderr:\n{result.stderr}"
        )
    if expected_exit != 0:
        # Negative case — no payload expected, but stderr is still
        # useful for diagnosing why a positive case unexpectedly fell
        # into this branch.
        return None, result.stderr
    if output is None:
        # Stdout-comparison mode (e.g. `count`). Encode to bytes so the
        # comparison helpers downstream can treat all payloads uniformly.
        return result.stdout.encode("utf-8"), result.stderr
    # An --allow-partial decode lands its output at ``<output>.partial`` rather
    # than ``<output>``; ``read_path`` points the comparison at the real artifact.
    read_target = read_path if read_path is not None else output
    if not read_target.exists():
        raise RuntimeError(f"{case_name}: {implementation} did not create {read_target}")
    return read_target.read_bytes(), result.stderr


def rust_command(
    args: argparse.Namespace,
    case: dict[str, Any],
    sources: list[Path],
    output: Path | None,
) -> list[str]:
    """Build the Rust CLI invocation for a case.

    ``output`` is the per-case scratch CSV path for ``mode == "decode"``
    cases, or ``None`` for ``mode == "count"`` (stdout-comparison mode,
    no -o flag).
    """
    command = [str(args.rust_bin)]
    return _build_command(command, case, sources, output)


def python_command(
    args: argparse.Namespace,
    case: dict[str, Any],
    sources: list[Path],
    output: Path | None,
) -> list[str]:
    """Build the Python CLI invocation for a case.

    The Python and Rust CLIs share one argument surface — global
    ``--config`` before the subcommand, a ``count`` subcommand, and an
    identical decode flag set — so this differs from ``rust_command``
    only in the interpreter/entrypoint prefix. The per-case ``args`` are
    passed verbatim to both.
    """
    command = [str(args.python_bin), "-m", "mie_decoder"]
    return _build_command(command, case, sources, output)


def _build_command(
    command: list[str],
    case: dict[str, Any],
    sources: list[Path],
    output: Path | None,
) -> list[str]:
    """Append the shared subcommand/flag tail to a CLI prefix.

    ``sources`` is one path for a single-input case, or several for a
    multi-file merge (``decode`` takes them as positionals). ``output`` is the
    per-case scratch CSV path for ``mode == "decode"``, or ``None`` for
    ``mode == "count"`` (stdout-comparison mode, no -o). ``--config`` is global
    (before the subcommand) for both CLIs.
    """
    if config := case.get("config"):
        command += ["--config", str((SUITE / config).resolve())]
    mode = case.get("mode", "decode")
    if mode == "count":
        command += ["count", str(sources[0])]
    else:
        command += ["decode", *(str(s) for s in sources), "-o", str(output)]
    command += case.get("args", [])
    return command


def prepare_rust_bin(args: argparse.Namespace) -> None:
    if args.rust_bin:
        args.rust_bin = args.rust_bin.resolve()
    else:
        suffix = ".exe" if sys.platform == "win32" else ""
        args.rust_bin = ROOT / "rust" / "target" / "debug" / f"mie-decoder{suffix}"

    if args.rust_bin.exists():
        return
    if shutil.which("cargo") is None:
        raise RuntimeError("cargo was not found; pass --rust-bin or install Rust")

    print("BUILD Rust CLI", flush=True)
    result = subprocess.run(
        ["cargo", "build", "--quiet", "--locked", "--bin", "mie-decoder"],
        cwd=ROOT / "rust",
        check=False,
        timeout=120,
    )
    if result.returncode != 0 or not args.rust_bin.exists():
        raise RuntimeError("failed to build the Rust CLI")


def prepare_python_bin(args: argparse.Namespace) -> None:
    """Resolve the Python interpreter that will run the Python mie-decoder CLI.

    Default to :data:`sys.executable`. When the runner is invoked under
    ``poetry -C python run python ...`` (as it is in CI), the active
    interpreter already has ``mie_decoder`` installed, so this avoids a
    fragile ``poetry env info --executable`` subprocess that can resolve
    to a different interpreter than the one Poetry installed packages
    into. The interpreter is sanity-checked by importing ``mie_decoder``
    so the runner fails fast with a clear error rather than emitting a
    confusing ``No module named mie_decoder`` for every case.
    """
    if args.python_bin:
        args.python_bin = args.python_bin.resolve()
    else:
        args.python_bin = Path(sys.executable).resolve()

    if not args.python_bin.exists():
        raise RuntimeError(f"Python interpreter was not found: {args.python_bin}")

    probe = subprocess.run(
        [str(args.python_bin), "-c", "import mie_decoder"],
        capture_output=True,
        text=True,
        check=False,
        timeout=10,
    )
    if probe.returncode != 0:
        raise RuntimeError(
            f"mie_decoder is not importable from {args.python_bin}. "
            "Either install the package into this interpreter (e.g. "
            "`poetry -C python sync`) or pass --python-bin pointing at "
            "an interpreter that has it.\n"
            f"stderr:\n{probe.stderr}"
        )


def _errors_path(main_output: Path) -> Path:
    """Derive the split-mode errors path for a given main output path.

    Mirrors the L2-ERR-008 stem/suffix definition (and the matching
    behavior in both implementations' writers): `out.csv` →
    `out_errors.csv`, `out` → `out_errors`.
    """
    stem = main_output.stem
    suffix = main_output.suffix
    if suffix:
        return main_output.with_name(f"{stem}_errors{suffix}")
    return main_output.with_name(f"{stem}_errors")


def diff_bytes(
    expected: bytes,
    actual: bytes,
    expected_name: str,
    actual_name: str,
) -> str:
    return "".join(
        difflib.unified_diff(
            expected.decode("utf-8").splitlines(keepends=True),
            actual.decode("utf-8").splitlines(keepends=True),
            fromfile=expected_name,
            tofile=actual_name,
        )
    )


def require_equal(
    expected: bytes,
    actual: bytes,
    expected_name: str,
    actual_name: str,
) -> None:
    if expected == actual:
        return
    raise AssertionError(
        f"{actual_name} does not match {expected_name}\n"
        f"{diff_bytes(expected, actual, expected_name, actual_name)}"
    )


# Long-option token, e.g. `--exclude-types`. The CLIs expose no flags with
# digits, but the pattern allows them defensively.
_FLAG_RE = re.compile(r"--[a-z][a-z0-9-]*")
# Subcommands whose --help is scanned in addition to the top-level --help.
_HELP_SUBCOMMANDS = ("decode", "count", "dump")


def _help_flags(base_command: list[str]) -> set[str]:
    """Union of long-option flags across a CLI's top-level and per-subcommand
    ``--help`` output. The Rust help is one combined block (every flag appears
    regardless of subcommand), so its union comes from any single ``--help``;
    the Python argparse help is per-subcommand, so the union spans them all."""
    flags: set[str] = set()
    invocations = [base_command + ["--help"]]
    invocations += [base_command + [sub, "--help"] for sub in _HELP_SUBCOMMANDS]
    for inv in invocations:
        result = subprocess.run(inv, capture_output=True, text=True, check=False)
        flags |= set(_FLAG_RE.findall(result.stdout + result.stderr))
    return flags


def check_cli_surface(args: argparse.Namespace) -> None:
    """Assert the Rust and Python CLIs expose an identical long-flag set across
    ``decode`` / ``count`` / ``dump`` (plus global options).

    The two CLIs are kept to one identical argument surface (L1-CLI-001 only
    *requires* matching capabilities, but parity is maintained in practice). A
    flag added to one implementation but not the other — or a help text that
    stops advertising a flag the parser still accepts — fails here, guarding the
    cross-implementation parity against silent drift."""
    rust_flags = _help_flags([str(args.rust_bin)])
    python_flags = _help_flags([str(args.python_bin), "-m", "mie_decoder"])
    if rust_flags != python_flags:
        only_rust = sorted(rust_flags - python_flags) or ["(none)"]
        only_python = sorted(python_flags - rust_flags) or ["(none)"]
        raise AssertionError(
            "CLI flag surface diverged between implementations:\n"
            f"  only in Rust  : {', '.join(only_rust)}\n"
            f"  only in Python: {', '.join(only_python)}"
        )
    print(f"PASS cli-surface-parity ({len(rust_flags)} flags)")


def main() -> int:
    args = parse_args()

    # Load + validate the manifest BEFORE prepare_rust_bin /
    # prepare_python_bin so a malformed manifest fails fast with a
    # schema error instead of getting masked behind a slow Rust
    # build or a Python interpreter probe failure.
    manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    for index, case in enumerate(manifest["cases"]):
        validate_case_schema(case, index)

    prepare_rust_bin(args)
    prepare_python_bin(args)

    # Cross-impl CLI flag-surface parity (independent of the per-case oracles).
    check_cli_surface(args)

    passed = 0
    if args.temp_root:
        args.temp_root.mkdir(parents=True, exist_ok=True)

    with tempfile.TemporaryDirectory(
        prefix="mie-conformance-",
        dir=args.temp_root,
    ) as temp_dir:
        temp = Path(temp_dir)
        for case in manifest["cases"]:
            name = case["name"]
            # One 'input' or several 'inputs' (multi-file merge). Each hex
            # fixture is materialized to its own temp .mie and passed as a
            # positional to both CLIs.
            input_specs = case.get("inputs") or [case["input"]]
            # L2-WRT-020: a case may override the materialized file name(s) so a
            # filename-derived feature (the MUX column) can be exercised. Names
            # are placed in a per-case subdirectory to avoid collisions.
            input_names = case.get("input_names")
            case_dir = temp / name if input_names else temp
            if input_names:
                case_dir.mkdir(parents=True, exist_ok=True)
            sources = []
            for i, spec in enumerate(input_specs):
                fname = input_names[i] if input_names else f"{name}-in{i}.mie"
                src = case_dir / fname
                src.write_bytes(read_hex(SUITE / spec))
                sources.append(src)
            expected_exit = int(case.get("expected_exit", 0))
            mode = case.get("mode", "decode")

            # ``count`` mode compares stdout (the integer count) rather
            # than a CSV file, so the per-impl output paths are unused.
            if mode == "count":
                rust_output = None
                python_output = None
            else:
                rust_output = temp / f"{name}-rust.csv"
                python_output = temp / f"{name}-python.csv"

            # An --allow-partial case commits to ``<output>.partial``; read that
            # artifact for the comparison and oracle against ``expected_partial``.
            partial_oracle = case.get("expected_partial")
            rust_read = Path(f"{rust_output}.partial") if partial_oracle and rust_output else None
            python_read = (
                Path(f"{python_output}.partial") if partial_oracle and python_output else None
            )

            rust, rust_stderr = run_command(
                rust_command(args, case, sources, rust_output),
                rust_output,
                name,
                "Rust",
                expected_exit=expected_exit,
                read_path=rust_read,
            )
            python, python_stderr = run_command(
                python_command(args, case, sources, python_output),
                python_output,
                name,
                "Python",
                expected_exit=expected_exit,
                read_path=python_read,
            )

            if expected_exit != 0:
                # Negative case — exit code alone is the assertion.
                # No CSV oracle is required (and the "expected" key may
                # be omitted from the manifest entry).
                passed += 1
                print(f"PASS {name} (expected_exit={expected_exit})")
                continue

            require_equal(rust, python, f"{name} Rust output", f"{name} Python output")

            # Optional stderr substring assertion. Used by ``count`` mode
            # to pin the "counted N messages in <path>" human-readable
            # status line in both implementations without requiring a
            # byte-exact comparison (the path basename varies with the
            # temp directory and so can't be oracled directly).
            stderr_needle = case.get("expected_stderr_contains")
            if stderr_needle:
                for impl, captured in (("Rust", rust_stderr), ("Python", python_stderr)):
                    if stderr_needle not in captured:
                        raise AssertionError(
                            f"{name}: {impl} stderr does not contain "
                            f"{stderr_needle!r}\n--- stderr ---\n{captured}"
                        )

            expected_path = SUITE / (case.get("expected_partial") or case["expected"])
            if args.update_expected:
                expected_path.parent.mkdir(parents=True, exist_ok=True)
                expected_path.write_bytes(rust)
                print(f"UPDATED {expected_path.relative_to(ROOT)}")

            if not expected_path.exists():
                raise RuntimeError(
                    f"{name}: expected output is missing: {expected_path}"
                )
            expected = expected_path.read_bytes()
            require_equal(expected, rust, str(expected_path), f"{name} Rust output")
            require_equal(expected, python, str(expected_path), f"{name} Python output")

            # Split-output cases (separate error mode) compare an
            # additional <output_stem>_errors.csv against the
            # expected_errors oracle. Both implementations derive the
            # errors path the same way (see L2-ERR-008 stem/suffix
            # definition), so we can reuse the canonical naming here.
            expected_errors_rel = case.get("expected_errors")
            if expected_errors_rel:
                rust_errors_path = _errors_path(rust_output)
                python_errors_path = _errors_path(python_output)
                if not rust_errors_path.exists():
                    raise RuntimeError(
                        f"{name}: Rust did not create errors file {rust_errors_path}"
                    )
                if not python_errors_path.exists():
                    raise RuntimeError(
                        f"{name}: Python did not create errors file {python_errors_path}"
                    )
                rust_errors = rust_errors_path.read_bytes()
                python_errors = python_errors_path.read_bytes()
                require_equal(
                    rust_errors,
                    python_errors,
                    f"{name} Rust errors output",
                    f"{name} Python errors output",
                )
                expected_errors_path = SUITE / expected_errors_rel
                if args.update_expected:
                    expected_errors_path.parent.mkdir(parents=True, exist_ok=True)
                    expected_errors_path.write_bytes(rust_errors)
                    print(f"UPDATED {expected_errors_path.relative_to(ROOT)}")
                if not expected_errors_path.exists():
                    raise RuntimeError(
                        f"{name}: expected_errors oracle is missing: {expected_errors_path}"
                    )
                expected_errors = expected_errors_path.read_bytes()
                require_equal(
                    expected_errors,
                    rust_errors,
                    str(expected_errors_path),
                    f"{name} Rust errors output",
                )
                require_equal(
                    expected_errors,
                    python_errors,
                    str(expected_errors_path),
                    f"{name} Python errors output",
                )

            passed += 1
            print(f"PASS {name}")

    print(f"{passed} conformance cases passed")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (AssertionError, RuntimeError, ValueError) as exc:
        print(f"FAIL: {exc}", file=sys.stderr)
        raise SystemExit(1) from exc
