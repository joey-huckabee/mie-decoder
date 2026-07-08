# MIE-Decoder (Python)

The Python implementation of MIE-Decoder: a decoder for DDC MIL-STD-1553 MIE
binary recording files, exposing both a `mie-decoder` CLI and an importable
`mie_decoder` package. Supports Python 3.10–3.14.

Shared documentation — the project overview, CLI reference, configuration
schema, supported message formats, error catalog, and vendor-CSV alignment —
lives at the [repository root](../README.md) and under [`docs/`](../docs/).

## Install

From a source checkout (editable install, run from the repository root):

```bash
pip install -e ./python
```

Or, for development, via Poetry from the repository root:

```bash
poetry -C python sync     # creates the venv, installs locked deps + the package
```

`poetry sync` installs the exact dependency versions recorded in `poetry.lock`
and removes packages that are not part of the locked environment.

## Library usage

```python
from mie_decoder import MieFileReader

reader = MieFileReader("recording.mie")
for message in reader:
    print(message.timestamp, message.rt, message.subaddress)
```

`MieFileReader` and the `MieMessage` records it yields are importable directly
from the package root (`mie_decoder`).

## Development

```bash
poetry -C python run pytest        # test suite
poetry -C python run mypy src      # strict type check (CI-gated)
poetry -C python run mie-decoder --help
poetry -P python build             # build the wheel + sdist
```

See [`CONTRIBUTING.md`](../CONTRIBUTING.md) for the full development workflow.

## Package structure

```
python/
├── pyproject.toml      Poetry + PEP 621 hybrid; pytest markers registered here
├── poetry.lock         pinned dependencies; committed
├── src/mie_decoder/    package source (mirrors the Rust module names)
└── tests/              pytest suite
```

## License

Apache-2.0 — see [LICENSE](../LICENSE).
