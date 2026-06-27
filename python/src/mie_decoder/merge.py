"""Multi-file, time-sorted streaming k-way merge (L1-MRG-*, L2-MRG-*).

Accepts several decoded recordings and yields one stream of ``MieMessage``s in
global time order, holding at most one record per open file in a min-heap
(resident memory O(number of files), independent of total record count —
L2-MRG-002). The merged stream feeds the existing ``write_csv`` /
``write_csv_split`` unchanged.

Merge requires every input to be calendar-locked IRIG; Standard-format,
freerun-leading, or mixed-format inputs are rejected up front
(:class:`MieIncompatibleMergeInputsError`, CLI exit 6 — L2-MRG-003). DELTA is
recomputed on the merged global timeline (L2-MRG-005).

Mirrors ``rust/src/merge.rs``; the heap is the standard-library :mod:`heapq` and the
``--glob`` matcher is hand-rolled to the same single-directory ``*``/``?``
semantics as Rust (L3-PY-014) — no new dependency.
"""

from __future__ import annotations

import dataclasses
import heapq
import itertools
import logging
import os
from collections.abc import Iterator
from pathlib import Path

from mie_decoder.exceptions import (
    MieDecoderError,
    MieIncompatibleMergeInputsError,
    MieNonMonotonicInputError,
    MieUnrecoverableSyncLossError,
)
from mie_decoder.models import IrigTimestamp, MieMessage, StandardTimestamp
from mie_decoder.reader import MieFileReader

logger = logging.getLogger(__name__)

#: Maximum number of input files a single merge invocation may process. Bounds
#: open mappings / file descriptors; exceeding it is a usage error (the CLI
#: maps it to exit 4). Shared in value with the Rust constant (L3-PY-014).
MAX_MERGE_FILES = 256


# ── Input resolution helpers ──────────────────────────────────────────────


def read_manifest(path: str | Path) -> list[Path]:
    """Read a manifest into a list of paths: one path per line, in order.

    Blank lines and lines whose first non-whitespace character is ``#`` are
    ignored; surrounding whitespace is trimmed (L2-MRG-001).
    """
    text = Path(path).read_text(encoding="utf-8")
    out: list[Path] = []
    for line in text.splitlines():
        trimmed = line.strip()
        if not trimmed or trimmed.startswith("#"):
            continue
        out.append(Path(trimmed))
    return out


def glob_match(pattern: str, name: str) -> bool:
    """Whole-string wildcard match: ``*`` matches any run (incl. empty), ``?``
    matches exactly one character; no other metacharacters are special.

    Iterative backtracking matcher with identical semantics to the Rust
    implementation (L3-RS-014).
    """
    p = t = 0
    star: int | None = None
    mark = 0
    while t < len(name):
        if p < len(pattern) and pattern[p] in ("?", name[t]):
            p += 1
            t += 1
        elif p < len(pattern) and pattern[p] == "*":
            star = p
            mark = t
            p += 1
        elif star is not None:
            p = star + 1
            mark += 1
            t = mark
        else:
            return False
    while p < len(pattern) and pattern[p] == "*":
        p += 1
    return p == len(pattern)


def expand_glob(pattern: str) -> list[Path]:
    """Expand a single-directory glob ``DIR/PATTERN`` (or ``PATTERN`` for the
    current directory). Wildcards apply to the filename only — no recursive
    ``**``, no brace expansion. Returns matching regular files sorted
    lexicographically (deterministic across implementations, L2-MRG-001).
    """
    p = Path(pattern)
    name_pat = p.name
    directory = p.parent if str(p.parent) else Path(".")
    out: list[Path] = []
    with os.scandir(directory) as it:
        for entry in it:
            if not entry.is_file():
                continue
            if glob_match(name_pat, entry.name):
                out.append(Path(entry.path))
    out.sort(key=str)
    return out


# ── k-way merge ────────────────────────────────────────────────────────────


def _merge_micros(msg: MieMessage, tick: float | None) -> int:
    """Microsecond merge key. IRIG always yields an int; the 0 fallback is
    unreachable for a validated (IRIG-only) merge."""
    us = msg.timestamp.to_microseconds(tick)
    return 0 if us is None else us


def _check_mergeable(msg: MieMessage, file_index: int, path: Path) -> None:
    """Reject an input whose leading record cannot anchor an absolute timeline."""
    ts = msg.timestamp
    if isinstance(ts, StandardTimestamp):
        raise MieIncompatibleMergeInputsError(
            file_index, str(path), "resolves to the Standard timestamp format"
        )
    if isinstance(ts, IrigTimestamp) and ts.freerun:
        raise MieIncompatibleMergeInputsError(
            file_index,
            str(path),
            "leads with a freerun IRIG record (no calendar time)",
        )


def _apply_global_delta(msg: MieMessage, tick: float | None, tracker: dict[str, int]) -> MieMessage:
    """Recompute DELTA on the merged global timeline (L2-MRG-005). The stream
    is timestamp-sorted, so per-key gaps are non-negative."""
    key = msg.delta_key
    if not key:
        return dataclasses.replace(msg, delta=None)  # SPURIOUS_DATA — no key
    curr = msg.timestamp.to_microseconds(tick)
    if curr is None:
        return dataclasses.replace(msg, delta=None)
    prev = tracker.get(key)
    tracker[key] = curr
    if prev is None:
        return dataclasses.replace(msg, delta=0.0)
    if curr >= prev:
        return dataclasses.replace(msg, delta=(curr - prev) / 1_000_000.0)
    return dataclasses.replace(msg, delta=None)


def merge_readers(
    readers: list[MieFileReader],
    *,
    standard_tick_rate_hz: float | None = None,
    allow_partial: bool = False,
    strict: bool = False,
) -> Iterator[MieMessage]:
    """Stream a time-sorted k-way merge over ``readers``.

    Validation of each input's leading record (L2-MRG-003) happens **eagerly**
    when this is called — so an incompatible set raises
    :class:`MieIncompatibleMergeInputsError` before any output is written,
    matching the Rust reader. The returned iterator then yields ``MieMessage``s
    in global time order so the existing writer consumes them unchanged. With
    ``allow_partial`` a file that fails is skipped / truncated with a WARN and
    the merge completes, deferring the terminal
    :class:`MieUnrecoverableSyncLossError` so the writer commits a ``.partial``
    (L2-MRG-004). The heap key ``(microseconds, file index, sequence)`` gives a
    deterministic total order (L2-MRG-002). A within-file backward timestamp
    step (L2-MRG-006) WARNs once per file in lenient mode and raises
    :class:`MieNonMonotonicInputError` in ``strict`` mode.
    """
    iters = [iter(r) for r in readers]
    seqs = [0] * len(readers)
    counter = itertools.count()
    # Microsecond key of the previous record pulled from each file, in capture
    # order, plus a one-time WARN guard — for L2-MRG-006 backward-step detection.
    prev_us: list[int | None] = [None] * len(readers)
    warned: list[bool] = [False] * len(readers)
    # Heap items: (us, file_index, seq, tiebreak_counter, msg). The unique
    # counter guarantees msg is never compared.
    heap: list[tuple[int, int, int, int, MieMessage]] = []

    # Prime + validate eagerly (before any output).
    for idx, it in enumerate(iters):
        try:
            msg = next(it)
        except StopIteration:
            continue  # empty file contributes nothing
        except MieDecoderError as exc:
            if allow_partial:
                logger.warning(
                    "merge: skipping input #%d (%s): %s",
                    idx,
                    readers[idx].path,
                    exc,
                )
                continue
            raise
        _check_mergeable(msg, idx, readers[idx].path)
        us = _merge_micros(msg, standard_tick_rate_hz)
        prev_us[idx] = us
        heapq.heappush(heap, (us, idx, 0, next(counter), msg))
        seqs[idx] = 1

    return _merge_drain(
        readers,
        iters,
        seqs,
        counter,
        heap,
        standard_tick_rate_hz,
        allow_partial,
        strict,
        prev_us,
        warned,
    )


def _merge_drain(
    readers: list[MieFileReader],
    iters: list[Iterator[MieMessage]],
    seqs: list[int],
    counter: "itertools.count[int]",
    heap: list[tuple[int, int, int, int, MieMessage]],
    tick: float | None,
    allow_partial: bool,
    strict: bool,
    prev_us: list[int | None],
    warned: list[bool],
) -> Iterator[MieMessage]:
    """Drain the primed heap: pop the min, recompute global DELTA, advance the
    file that record came from."""
    tracker: dict[str, int] = {}
    pending_terminal: MieUnrecoverableSyncLossError | None = None
    while heap:
        _, idx, _, _, msg = heapq.heappop(heap)
        yield _apply_global_delta(msg, tick, tracker)
        try:
            nxt = next(iters[idx])
        except StopIteration:
            continue  # file exhausted
        except MieUnrecoverableSyncLossError as exc:
            if allow_partial:
                logger.warning("merge: input #%d truncated at its failure point: %s", idx, exc)
                pending_terminal = exc  # defer until the heap drains
                continue
            raise
        # L2-MRG-006: each input is assumed internally time-sorted (capture
        # order is chronological). A backward step means the merged output may
        # be out of order for this file — strict fails the batch, lenient WARNs
        # once per file.
        curr = _merge_micros(nxt, tick)
        prev = prev_us[idx]
        if prev is not None and curr < prev:
            if strict:
                raise MieNonMonotonicInputError(idx, str(readers[idx].path), prev, curr)
            if not warned[idx]:
                warned[idx] = True
                logger.warning(
                    "merge: input #%d (%s) is not internally time-sorted: "
                    "timestamp stepped backward (prev_us=%d curr_us=%d) — "
                    "merged output may be out of order for this input "
                    "(further occurrences suppressed)",
                    idx,
                    readers[idx].path,
                    prev,
                    curr,
                )
        prev_us[idx] = curr
        seq = seqs[idx]
        seqs[idx] = seq + 1
        # next() on the infinite itertools.count() tiebreak never raises.
        tiebreak = next(counter)  # pylint: disable=stop-iteration-return
        heapq.heappush(heap, (curr, idx, seq, tiebreak, nxt))

    # An --allow-partial deferred failure surfaces here so the writer commits a
    # `.partial` after all good records are written.
    if pending_terminal is not None:
        raise pending_terminal
