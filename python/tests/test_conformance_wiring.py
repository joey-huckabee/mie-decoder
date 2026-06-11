"""Meta-tests that the cross-implementation conformance suite is wired up.

L2-CONF-003 ("each implementation's output SHALL match the checked-in CSV
oracle") is verified by `tests/conformance/run.py`, which is invoked by
the CI ``conformance`` job. The runner is not itself a pytest test and
therefore can't carry a ``@pytest.mark.requirement`` marker; these
pytest meta-tests assert that the conformance contract is present,
discoverable, and exercises at least one fixture so the trace matrix
credits L2-CONF-003 with a verifiable artifact.

A failing run of the conformance runner in CI is the authoritative
test; these meta-tests guarantee the runner stays wired up.
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest

_REPO_ROOT = Path(__file__).resolve().parents[2]
_CONFORMANCE_DIR = _REPO_ROOT / "tests" / "conformance"


@pytest.mark.requirement("L2-CONF-003")
def test_conformance_runner_exists() -> None:
    """The cross-implementation runner script SHALL exist."""
    runner = _CONFORMANCE_DIR / "run.py"
    assert runner.is_file(), f"conformance runner missing at {runner}"


@pytest.mark.requirement("L2-CONF-002")
def test_conformance_runner_invokes_both_clis_and_compares_outputs() -> None:
    """The runner SHALL execute both implementations and compare them."""
    body = (_CONFORMANCE_DIR / "run.py").read_text(encoding="utf-8")
    for required in [
        "rust_command(args, case, source, rust_output)",
        "python_command(args, case, source, python_output)",
        "require_equal(rust, python",
    ]:
        assert required in body, f"conformance runner missing {required!r}"


@pytest.mark.requirement("L2-CONF-003")
def test_conformance_manifest_has_cases_with_oracles() -> None:
    """The manifest SHALL list at least one case, each with both a
    hexadecimal input fixture and a checked-in CSV oracle path."""
    manifest_path = _CONFORMANCE_DIR / "manifest.json"
    assert manifest_path.is_file(), f"manifest missing at {manifest_path}"
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    cases = manifest["cases"] if isinstance(manifest, dict) else manifest
    assert cases, "manifest must contain at least one conformance case"
    inputs_dir = _CONFORMANCE_DIR / "inputs"
    expected_dir = _CONFORMANCE_DIR / "expected"
    for case in cases:
        name = case.get("name", "<unnamed>")
        assert "input" in case, f"case {name!r} missing 'input'"
        # Negative cases (expected_exit != 0) may not have an oracle.
        if case.get("expected_exit", 0) == 0:
            assert "expected" in case, f"case {name!r} missing 'expected'"
            oracle = expected_dir / Path(case["expected"]).name
            assert oracle.is_file(), (
                f"oracle for case {name!r} missing at {oracle}"
            )
        fixture = inputs_dir / Path(case["input"]).name
        assert fixture.is_file(), (
            f"input fixture for case {name!r} missing at {fixture}"
        )


@pytest.mark.requirement("L2-CONF-005")
def test_conformance_job_present_in_ci() -> None:
    """The CI workflow SHALL contain a job that invokes the conformance
    runner, so L2-CONF-005 ("CI runs the conformance suite on every push
    and pull request") has live evidence beyond spec review."""
    ci_path = _REPO_ROOT / ".github" / "workflows" / "ci.yml"
    assert ci_path.is_file(), f"CI workflow missing at {ci_path}"
    body = ci_path.read_text(encoding="utf-8")
    assert "tests/conformance/run.py" in body, (
        "CI workflow does not appear to invoke the conformance runner"
    )
