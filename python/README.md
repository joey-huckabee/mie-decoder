# MIE-Decoder Python

This directory contains the actively maintained Python implementation of
MIE-Decoder.

From the repository root:

```bash
poetry -C python sync
poetry -C python run pytest
poetry -C python run mie-decoder --help
poetry -P python build
```

`poetry sync` installs the exact dependency versions recorded in
`poetry.lock` and removes packages that are not part of the locked environment.

Shared format documentation, project guidance, and the Rust implementation
live at the [repository root](../README.md).
