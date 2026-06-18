"""Package-root public API contract (L3-PY-007).

L3-PY-007 requires the ``mie_decoder`` package to expose its decoder entry
point as a typed callable importable from the package root. These tests
assert exactly that — the root re-export, that the entry point is a typed
callable, and that the public surface is documented — so the requirement
traces to a real root-API check rather than borrowing its parent's
conformance-wiring test. (The broader "all public APIs carry type
annotations" clause is enforced separately by the CI-gated ``mypy src``
strict run.)
"""

from __future__ import annotations

import inspect

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
