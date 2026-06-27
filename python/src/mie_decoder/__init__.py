"""MIE-Decoder: DDC MIL-STD-1553 MIE binary recording file decoder.

This package provides a decoder for proprietary binary files produced by
DDC (Data Device Corporation) MIL-STD-1553 PCI recording cards. These
files use the ``.mie`` or ``.mie_alta`` extension and contain timestamped
1553 bus monitor captures with IRIG-format time tags.

The binary format consists of fixed-length records whose size is
determined by a Type Word at the start of each record. Each record
contains an IRIG timestamp, a MIL-STD-1553 command word, an optional
status word, and data words captured from the bus.

Typical usage::

    from mie_decoder import MieFileReader

    reader = MieFileReader("recording.mie")
    for message in reader:
        print(message.timestamp, message.rt, message.subaddress)

The decoder entry point ``MieFileReader`` and the ``MieMessage`` records it
yields are importable directly from the package root (``mie_decoder``).

Version history:
    1.0.0 - Joint Rust + Python initial release. See CHANGELOG.md.
"""

from importlib.metadata import PackageNotFoundError, version as _pkg_version

# Public package-root API (L3-PY-007). The decoder entry point —
# ``MieFileReader``, a typed callable: ``MieFileReader(path)`` constructs a
# reader that decodes the file lazily into ``MieMessage`` records — and that
# record type are re-exported here so library consumers can write
# ``from mie_decoder import MieFileReader`` without reaching into submodules.
# (The submodule paths remain importable and unchanged.)
from mie_decoder.models import MieMessage
from mie_decoder.reader import MieFileReader

try:
    __version__ = _pkg_version("mie-decoder")
except PackageNotFoundError:
    # Source-tree fallback: the package isn't installed (e.g. imported
    # directly from a clone before `pip install -e ./python` or
    # `poetry sync` has run). All standard usage paths install the
    # package first, so this branch is rarely hit; the sentinel value
    # makes it obvious that the version came from this fallback rather
    # than from real package metadata.
    __version__ = "0.0.0+source"

del PackageNotFoundError, _pkg_version

__all__ = ["MieFileReader", "MieMessage", "__version__"]
