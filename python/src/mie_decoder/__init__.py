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

    from mie_decoder.reader import MieFileReader

    reader = MieFileReader("recording.mie")
    for message in reader:
        print(message.timestamp, message.rt, message.subaddress)

Version history:
    1.0.0 - Joint Rust + Python initial release. See CHANGELOG.md.
"""

from importlib.metadata import PackageNotFoundError, version as _pkg_version

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
