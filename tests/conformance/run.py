#!/usr/bin/env python3
"""Compare shared Rust and Python MIE decoding behavior."""

from __future__ import annotations

import argparse
import difflib
import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[2]
SUITE = Path(__file__).resolve().parent
MANIFEST = SUITE / "manifest.json"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--rust-bin",
        type=Path,
        help="Use this Rust binary instead of target/debug/mie-decoder.",
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
    output: Path,
    case_name: str,
    implementation: str,
) -> bytes:
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
    if result.returncode != 0:
        rendered = subprocess.list2cmdline(command)
        raise RuntimeError(
            f"{case_name}: {implementation} exited {result.returncode}\n"
            f"command: {rendered}\n"
            f"stdout:\n{result.stdout}\n"
            f"stderr:\n{result.stderr}"
        )
    if not output.exists():
        raise RuntimeError(f"{case_name}: {implementation} did not create {output}")
    return output.read_bytes()


def rust_command(
    args: argparse.Namespace,
    case: dict[str, Any],
    source: Path,
    output: Path,
) -> list[str]:
    command = [str(args.rust_bin)]
    if config := case.get("config"):
        command += ["--config", str((SUITE / config).resolve())]
    command += ["decode", str(source), "-o", str(output)]
    command += case.get("rust_args", [])
    return command


def python_command(
    args: argparse.Namespace,
    case: dict[str, Any],
    source: Path,
    output: Path,
) -> list[str]:
    command = [
        str(args.python_bin),
        "-m",
        "mie_decoder",
        "decode",
        str(source),
        "-o",
        str(output),
    ]
    if config := case.get("config"):
        command += ["--config", str((SUITE / config).resolve())]
    command += case.get("python_args", [])
    return command


def prepare_rust_bin(args: argparse.Namespace) -> None:
    if args.rust_bin:
        args.rust_bin = args.rust_bin.resolve()
    else:
        suffix = ".exe" if sys.platform == "win32" else ""
        args.rust_bin = ROOT / "target" / "debug" / f"mie-decoder{suffix}"

    if args.rust_bin.exists():
        return
    if shutil.which("cargo") is None:
        raise RuntimeError("cargo was not found; pass --rust-bin or install Rust")

    print("BUILD Rust CLI", flush=True)
    result = subprocess.run(
        ["cargo", "build", "--quiet", "--locked", "--bin", "mie-decoder"],
        cwd=ROOT,
        check=False,
        timeout=120,
    )
    if result.returncode != 0 or not args.rust_bin.exists():
        raise RuntimeError("failed to build the Rust CLI")


def prepare_python_bin(args: argparse.Namespace) -> None:
    if args.python_bin:
        args.python_bin = args.python_bin.resolve()
    else:
        if shutil.which("poetry") is None:
            raise RuntimeError("poetry was not found; pass --python-bin")
        result = subprocess.run(
            ["poetry", "-C", "python", "env", "info", "--executable"],
            cwd=ROOT,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            check=False,
            timeout=10,
        )
        if result.returncode != 0:
            raise RuntimeError(f"failed to resolve Poetry environment:\n{result.stderr}")
        executable = Path(result.stdout.strip())
        args.python_bin = (
            executable.resolve()
            if executable.is_absolute()
            else (ROOT / "python" / executable).resolve()
        )

    if not args.python_bin.exists():
        raise RuntimeError(f"Python interpreter was not found: {args.python_bin}")


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


def main() -> int:
    args = parse_args()
    prepare_rust_bin(args)
    prepare_python_bin(args)

    manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
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
            source = temp / f"{name}.mie"
            source.write_bytes(read_hex(SUITE / case["input"]))
            rust_output = temp / f"{name}-rust.csv"
            python_output = temp / f"{name}-python.csv"
            expected_path = SUITE / case["expected"]

            rust = run_command(
                rust_command(args, case, source, rust_output),
                rust_output,
                name,
                "Rust",
            )
            python = run_command(
                python_command(args, case, source, python_output),
                python_output,
                name,
                "Python",
            )

            require_equal(rust, python, f"{name} Rust output", f"{name} Python output")

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
