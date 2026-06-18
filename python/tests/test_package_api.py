"""Installed-package public surface (L3-PY-007 and L3-PY-003).

L3-PY-007 requires the ``mie_decoder`` package to expose its decoder entry
point as a typed callable importable from the package root; L3-PY-003
requires the ``mie-decoder`` console script to be registered via
``[project.scripts]``. These tests assert both directly — the root
re-export, that the entry point is a typed callable, that the public
surface is documented, and that the console-script entry point is
installed — so each requirement traces to a real package-surface check
rather than borrowing an unrelated parent's test. (The broader "all
public APIs carry type annotations" clause is enforced separately by the
CI-gated ``mypy src`` strict run.)
"""

from __future__ import annotations

import inspect
from importlib.metadata import entry_points

import pytest


@pytest.mark.requirement("L3-PY-007")
def test_decoder_entry_point_importable_from_package_root() -> None:
    """``from mie_decoder import MieFileReader`` resolves to the reader."""
    import mie_decoder
    from mie_decoder import MieFileReader
    from mie_decoder.reader import MieFileReader as ReaderModuleClass

    # Same object as the submodule definition — a genuine re-export, not a
    # shadowing stub.
    assert MieFileReader is ReaderModuleClass
    # Advertised in the package's public surface.
    assert "MieFileReader" in mie_decoder.__all__


@pytest.mark.requirement("L3-PY-007")
def test_decoder_entry_point_is_a_typed_callable() -> None:
    """``MieFileReader`` is callable and its constructor is fully typed."""
    from mie_decoder import MieFileReader

    assert callable(MieFileReader)
    sig = inspect.signature(MieFileReader)
    assert sig.parameters, "entry point should take at least an input path"
    unannotated = [
        name
        for name, p in sig.parameters.items()
        if p.annotation is inspect.Parameter.empty
    ]
    assert not unannotated, f"untyped constructor parameters: {unannotated}"


@pytest.mark.requirement("L3-PY-007")
def test_message_type_importable_from_package_root() -> None:
    """The yielded record type is also importable from the root."""
    import mie_decoder
    from mie_decoder import MieMessage
    from mie_decoder.models import MieMessage as ModelsMessage

    assert MieMessage is ModelsMessage
    assert "MieMessage" in mie_decoder.__all__


@pytest.mark.requirement("L3-PY-007")
def test_public_surface_is_documented() -> None:
    """Package and entry point carry docstrings (documented public API)."""
    import mie_decoder
    from mie_decoder import MieFileReader

    assert mie_decoder.__doc__ and mie_decoder.__doc__.strip()
    assert MieFileReader.__doc__ and MieFileReader.__doc__.strip()


@pytest.mark.requirement("L3-PY-003")
def test_console_script_entry_point_registered() -> None:
    """The `mie-decoder` console script is registered (L3-PY-003).

    The conformance suite drives the CLI via ``python -m mie_decoder`` (the
    ``__main__`` shim), so the ``[project.scripts]`` console-script entry
    point is otherwise only exercised manually. This pins that it is
    installed and points at ``cli:main`` — catching a packaging regression
    that the rest of the suite would miss.
    """
    scripts = entry_points(group="console_scripts")
    mie = [e for e in scripts if e.name == "mie-decoder"]
    assert mie, "mie-decoder console script is not registered"
    assert mie[0].value == "mie_decoder.cli:main"
