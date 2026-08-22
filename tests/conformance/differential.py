"""Shared machinery for the differential config checks.

Three checks — a curated corpus (`config_parity`), a generated fuzzer
(`config_fuzz`), and the config *path* (`config_path_parity`) — all do the same
thing: run one input through every implementation and compare the verdicts. They
each had their own copy of "compare Rust to Python", written when there were
exactly two implementations and the comparison was a single `!=`.

A third implementation makes the comparison a real operation rather than an
inline test, and this is where it lives.

ALL-PAIRS, NOT MAJORITY. Every implementation must agree with every other; any
disagreement fails. The alternatives were considered and rejected:

  * *Majority* — two implementations sharing a bug outvote the correct one, and
    the gate reports success. It also destroys the finding: you learn "the
    minority is wrong" rather than "these two disagree", which is the opposite
    of what a differential check is for.

  * *Reference implementation* — nominating one as the oracle makes its quirks
    normative. A bug in the reference becomes a requirement the others must
    reproduce.

Because agreement here is plain equality, all-pairs is *equivalent* to "every
implementation returned the same verdict". The reason to think of it as pairs is
the failure message: "3 implementations disagreed" is not actionable, and
"accept: Rust, Python | reject: C++" is — it says which side to look at first
without claiming which side is right.

WHAT IS COMPARED depends on the check, so these helpers are generic over the
compared value:

  * `config_parity` and `config_fuzz` compare the accept/reject VERDICT. The
    implementations legitimately differ in wording, and a rejection may be exit
    5 (config error) or exit 4 (usage) depending on where the value was caught —
    but whether a document is *admissible* is the contract the hand-rolled
    parsers share. Exit codes are still carried into the failure text, because
    they are usually the fastest clue to why a divergence happened.

  * `config_path_parity` compares the EXACT exit code, because its cases pin
    specific codes rather than mere admissibility.
"""

from __future__ import annotations

from collections.abc import Mapping


def classify(returncode: int) -> str:
    """The verdict a run represents: it either took the config or it did not."""
    return "accept" if returncode == 0 else "reject"


def group_by_value(values: Mapping[str, str]) -> dict[str, list[str]]:
    """Implementation names grouped by the value they produced.

    Insertion-ordered so the report reads in a stable order across runs rather
    than in whatever order a set iterated.
    """
    groups: dict[str, list[str]] = {}
    for impl, value in values.items():
        groups.setdefault(value, []).append(impl)
    return groups


def describe_divergence(
    values: Mapping[str, str], codes: Mapping[str, int] | None = None
) -> str | None:
    """``None`` when every implementation agreed; otherwise the split.

    The description names each verdict and who returned it — deliberately
    without nominating a winner. Which side is *correct* is a judgement the
    maintainer makes from the snippet; what this can state as fact is that the
    implementations disagree, and that alone is the bug.
    """
    if len(set(values.values())) <= 1:
        return None

    parts: list[str] = []
    for value, impls in group_by_value(values).items():
        if codes is not None:
            named = ", ".join(f"{impl} (exit {codes[impl]})" for impl in impls)
        else:
            named = ", ".join(impls)
        parts.append(f"{value}: {named}")
    return " | ".join(parts)


def describe_agreement(classes: Mapping[str, str], codes: Mapping[str, int]) -> str:
    """How a unanimous verdict was reached, for a message about the *expected*
    result being wrong rather than about a divergence."""
    verdict = next(iter(classes.values()))
    detail = ", ".join(f"{impl} exit {codes[impl]}" for impl in sorted(classes))
    return f"all {verdict} ({detail})"
