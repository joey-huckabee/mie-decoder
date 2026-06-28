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
from collections import deque
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


def _dedup_key(msg: MieMessage) -> tuple[object, ...]:
    """Content identity of a message for cross-recorder de-duplication
    (L2-MRG-007): the bits a recorder reads off the wire — Type Word, Command /
    Status Words, Error Word, and the data words. Timestamp, file offset, MUX,
    and DELTA are intentionally excluded — the timestamp drives the window (not
    equality), and the rest are per-recorder. Mirrors ``DedupKey`` in Rust."""
    return (
        msg.type_word,
        msg.command_word,
        msg.command_word_2,
        msg.status_word,
        msg.status_word_2,
        msg.error_word,
        msg.data_words,
    )


class _DedupWindow:
    """Sliding time-window de-duplicator over the merged stream (L2-MRG-007).
    Holds the survivors emitted within ``window_us`` microseconds as
    ``(us, file_index, key)``; in a sorted stream older survivors can never match
    a later record and are evicted from the front, so resident memory stays
    bounded by the window (not the record count). Matching uses the **absolute**
    time distance, so a lenient non-monotonic input (L2-MRG-006) that steps
    backward neither raises nor collapses records outside the window — collapse
    is best-effort on such "known bad" order. Mirrors ``DedupWindow`` in
    ``rust/src/merge.rs``."""

    def __init__(self, window_us: int) -> None:
        self._window_us = window_us
        self._survivors: deque[tuple[int, int, tuple[object, ...]]] = deque()

    def is_duplicate(self, us: int, file_index: int, msg: MieMessage) -> bool:
        """True if ``msg`` (at ``us``, from ``file_index``) duplicates a recent
        survivor from a *different* input within the window — the same bus
        transaction witnessed by another recorder. Same-file identical content is
        never a duplicate; a non-duplicate is recorded as a survivor."""
        # Evict survivors too old to fall within the window of the current (or,
        # in a sorted stream, any later) record. Under lenient non-monotonic
        # input (L2-MRG-006) a backward step makes ``us - survivor_us`` negative,
        # which simply leaves those (future-relative) survivors in place.
        while self._survivors and us - self._survivors[0][0] > self._window_us:
            self._survivors.popleft()
        key = _dedup_key(msg)
        # A survivor matches only if it is within the window in ABSOLUTE time:
        # the merged stream may step backward, so the distance must be
        # order-independent (abs), never a one-sided subtraction. In a sorted
        # stream every retained survivor has ``us - survivor_us <= window``, so
        # this is identical to the previous behavior.
        for survivor_us, file_idx, survivor_key in self._survivors:
            if (
                file_idx != file_index
                and abs(survivor_us - us) <= self._window_us
                and survivor_key == key
            ):
                return True
        self._survivors.append((us, file_index, key))
        return False


def merge_readers(
    readers: list[MieFileReader],
    *,
    standard_tick_rate_hz: float | None = None,
    allow_partial: bool = False,
    strict: bool = False,
    collapse_duplicates: bool = False,
    collapse_window_us: int = 0,
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
    # A priming-time failure under allow_partial arms this terminal so the writer
    # commits a `.partial` (L2-MRG-004), exactly like a mid-file failure. The file
    # contributed no records (truncated at offset 0).
    priming_terminal: MieUnrecoverableSyncLossError | None = None

    # Prime + validate eagerly (before any output).
    for idx, it in enumerate(iters):
        try:
            msg = next(it)
        except StopIteration:
            continue  # empty file contributes nothing
        except MieDecoderError as exc:
            if allow_partial:
                logger.warning(
                    "merge: input #%d (%s) could not be read; truncating it "
                    "from the merge (--allow-partial): %s",
                    idx,
                    readers[idx].path,
                    exc,
                )
                priming_terminal = MieUnrecoverableSyncLossError(0, 0)
                continue
            raise
        _check_mergeable(msg, idx, readers[idx].path)
        us = _merge_micros(msg, standard_tick_rate_hz)
        prev_us[idx] = us
        heapq.heappush(heap, (us, idx, 0, next(counter), msg))
        seqs[idx] = 1

    dedup = _DedupWindow(collapse_window_us) if collapse_duplicates else None
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
        dedup,
        priming_terminal,
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
    dedup: _DedupWindow | None,
    pending_terminal: MieUnrecoverableSyncLossError | None = None,
) -> Iterator[MieMessage]:
    """Drain the primed heap: pop the min, optionally collapse cross-recorder
    duplicates, recompute global DELTA, advance the file that record came from.

    ``pending_terminal`` carries a priming-time --allow-partial failure (set in
    ``merge_readers``); a mid-file failure may overwrite it. Either is raised
    after the heap drains so the writer commits a `.partial` (L2-MRG-004)."""
    tracker: dict[str, int] = {}
    collapsed = 0
    while heap:
        us, idx, _, _, msg = heapq.heappop(heap)
        # Collapse cross-recorder duplicates before the global-DELTA stage
        # (L2-MRG-007): a suppressed duplicate must not advance the per-key DELTA
        # tracker, so DELTA is measured across the deduped timeline.
        if dedup is not None and dedup.is_duplicate(us, idx, msg):
            collapsed += 1
        else:
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

    if collapsed:
        logger.info("merge: collapsed %d duplicate message(s) across recorders", collapsed)
    # An --allow-partial deferred failure surfaces here so the writer commits a
    # `.partial` after all good records are written.
    if pending_terminal is not None:
        raise pending_terminal
