"""Execute the README library examples so they can't silently rot.

The ``python`` code blocks in ``python/README.md`` are extracted verbatim and
run against a real fixture (the placeholder ``"recording.mie"`` is swapped for a
decodable ``.mie``). A stale attribute or signature — e.g. the historical
``message.subaddress`` bug (a ``MieMessage`` has no such attribute) — raises and
fails the test. This is the Python counterpart to the Rust README doctest wired
in via ``include_str!`` in ``rust/src/lib.rs``.
"""

from __future__ import annotations

import re
from pathlib import Path

import pytest

PYTHON_README = Path(__file__).resolve().parents[1] / "README.md"


def _python_blocks(md_path: Path) -> list[str]:
    """Return the bodies of every ```python fenced block in *md_path*."""
    return re.findall(r"```python\n(.*?)```", md_path.read_text(encoding="utf-8"), re.S)


@pytest.mark.requirement("L3-PY-007")
def test_python_readme_examples_execute(tmp_mie_file: Path) -> None:
    """Every ```python block in python/README.md runs without error against a
    real recording (the documented public-API surface actually works)."""
    blocks = _python_blocks(PYTHON_README)
    assert blocks, "no ```python code blocks found in python/README.md"

    for block in blocks:
        # Point the example at a real fixture instead of the placeholder path.
        runnable = block.replace('"recording.mie"', repr(str(tmp_mie_file)))
        namespace: dict[str, object] = {}
        exec(compile(runnable, str(PYTHON_README), "exec"), namespace)  # noqa: S102
