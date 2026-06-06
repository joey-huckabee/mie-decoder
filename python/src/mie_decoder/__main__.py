"""Entry point for ``python -m mie_decoder``.

Delegates all CLI logic to :mod:`mie_decoder.cli`.
"""

from __future__ import annotations

import sys

from mie_decoder.cli import main

if __name__ == "__main__":
    sys.exit(main())
