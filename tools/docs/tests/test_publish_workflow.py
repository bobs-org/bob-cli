"""Structural contract tests for the reusable gated-publication workflow.

Conditions are inspected as parsed YAML data (including the YAML `on` key,
which parses as boolean True), never by matching raw file text.
"""

import re
from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parents[3]
WORKFLOW = ROOT / ".github/workflows/docs-publish.yml"

EXPECTED_CALLER = "bobs-org/bobs-org.github.io"
EXPECTED_ORIGIN = "https://bobs-org.github.io"

ALLOWED_USES = {
    "actions/checkout@v6",
    "actions/configure-pages@v5",
    "actions/upload-pages-artifact@v4",
    "actions/deploy-pages@v4",
    "actions/setup-python@v6",
    "astral-sh/setup-uv@v6",
    "actions/upload-artifact@v4",
    "actions/download-artifact@v4",
}

_EXPR = re.compile(r"\$\{\{\s*(.*?)\s*\}\}")
_REF = re.compile(r"[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_*-]+|\[[^\]]+\])*")


def _load() -> dict:
    return yaml.safe_load(WORKFLOW.read_text(encoding="utf-8"))


def _triggers(document: dict) -> dict:
    if "on" in document:
        return document["on"]
    return document[True]


def _refs_of(mapping: dict) -> set[str]:
    """Dotted expression paths referenced by a step or job mapping.

    Step and job `if` conditions are already expressions, so they are scanned
    whole; other fields are scanned inside their `${{ }}` blocks only.
    """

    found: set[str] = set()
    condition = mapping.get("if", "") or ""
    if isinstance(condition, str):
        found.update(match.group(0) for match in _REF.finditer(condition))
    for key in ("run", "uses"):
        text = mapping.get(key, "") or ""
        if isinstance(text, str):
            for expression in _EXPR.finditer(text):
                found.update(match.group(0) for match in _REF.finditer(expression.group(1)))
    for key in ("with", "env"):
        value = mapping.get(key)
        if isinstance(value, dict):
            for text in (f"{name}={item}" for name, item in value.items()):
                for expression in _EXPR.finditer(text):
                    found.update(
                        match.group(0) for match in _REF.finditer(expression.group(1))
                    )
    return found


def _steps(job: dict) -> list[dict]:
    return [step for step in job.get("steps", []) if isinstance(step, dict)]


def _step_by_name(job: dict, name: str) -> dict:
    for step in _steps(job):
        if step.get("name") == name:
            return step
    raise AssertionError(f"job has no step named {name!r}")


@pytest.fixture(scope="module")
def document() -> dict:
    assert WORKFLOW.is_file(), "reusable publication workflow is missing"
    document = _load()
    assert isinstance(document, dict)
    return document


def test_only_workflow_call_triggers(document: dict) -> None:
    assert set(_triggers(document)) == {"workflow_call"}


def test_inputs_are_optional_booleans_defaulting_false(document: dict) -> None:
    inputs = _triggers(document)["workflow_call"]["inputs"]
    assert set(inputs) == {"force", "drill", "pinned"}
    for name, spec in inputs.items():
        assert spec["type"] == "boolean", name
        assert spec["default"] is False, name
        assert spec.get("required", False) is False, name


def test_single_non_cancelling_pages_concurrency(document: dict) -> None:
    assert document["concurrency"] == {"group": "pages", "cancel-in-progress": False}


def test_all_actions_are_pinned_release_tags(document: dict) -> None:
    for job_name, job in document["jobs"].items():
        for step in _steps(job):
            uses = step.get("uses")
            if uses is None:
                continue
            assert re.fullmatch(r"[^@\s]+@v\d+", uses), f"{job_name}: unpinned {uses}"
            assert uses in ALLOWED_USES, f"{job_name}: unexpected pin {uses}"


def test_deploy_job_holds_the_pages_environment(document: dict) -> None:
    deploy = document["jobs"]["deploy"]
    assert deploy["environment"]["name"] == "github-pages"
    assert any(
        step.get("uses") == "actions/deploy-pages@v4" for step in _steps(deploy)
    )


def test_gates_require_a_planned_build_and_plumb_drill(document: dict) -> None:
    gates = _step_by_name(document["jobs"]["publish"], "Run the gated production checks")
    refs = _refs_of(gates)
    assert any(ref.endswith("should_build") for ref in refs)
    assert "inputs.drill" in refs


def test_upload_and_deploy_gate_on_planner_outputs_not_inputs(document: dict) -> None:
    upload = _step_by_name(document["jobs"]["publish"], "Upload the gated site")
    refs = _refs_of(upload)
    assert any(ref.endswith("should_deploy") for ref in refs)
    assert not any(ref.startswith("inputs.") for ref in refs)

    deploy = document["jobs"]["deploy"]
    assert any(ref.endswith("should_deploy") for ref in _refs_of(deploy))


def test_record_verify_and_report_chain_on_job_outcomes(document: dict) -> None:
    record_refs = _refs_of(document["jobs"]["record"])
    assert "needs.deploy.result" in record_refs
    assert "success" in record_refs
    verify_refs = _refs_of(document["jobs"]["verify"])
    assert "needs.record.result" in verify_refs
    assert "success" in verify_refs
    report = document["jobs"]["report"]
    assert "failure()" in str(report.get("if", ""))


def test_record_commits_only_the_manifest_without_force(document: dict) -> None:
    scripts = [
        step.get("run", "")
        for job in document["jobs"].values()
        for step in _steps(job)
        if isinstance(step.get("run"), str)
    ]
    assert scripts, "workflow has no run scripts"
    for script in scripts:
        assert "push --force" not in script and "push -f" not in script
        for line in script.splitlines():
            stripped = line.strip()
            if stripped.startswith("git add "):
                assert stripped == "git add published.json", stripped


def test_guard_names_caller_repo_and_default_branch(document: dict) -> None:
    guard = _step_by_name(document["jobs"]["publish"], "Guard caller repository and branch")
    run = guard.get("run", "")
    refs = _refs_of(guard)
    assert EXPECTED_CALLER in run
    assert "github.repository" in refs
    assert "default_branch" in run
    assert "github.ref_name" in refs


def test_job_permissions_are_least_privilege(document: dict) -> None:
    jobs = document["jobs"]
    assert jobs["publish"]["permissions"] == {
        "contents": "read",
        "pages": "write",
        "id-token": "write",
        "issues": "read",
    }
    assert jobs["deploy"]["permissions"] == {"pages": "write", "id-token": "write"}
    assert jobs["record"]["permissions"] == {"contents": "write"}
    assert jobs["verify"]["permissions"] == {"contents": "read", "issues": "write"}
    assert jobs["report"]["permissions"] == {"contents": "read", "issues": "write"}


def test_plan_reads_blocked_issues_and_recovers(document: dict) -> None:
    plan = _step_by_name(document["jobs"]["publish"], "Plan immutable publication sources")
    run = plan.get("run", "")
    refs = _refs_of(plan)
    assert "docs-publish" in run
    assert "--recovery" in run
    assert {"inputs.force", "inputs.drill", "inputs.pinned"} <= refs


def test_verify_checks_live_site_then_closes_the_issue(document: dict) -> None:
    verify_steps = _steps(document["jobs"]["verify"])
    runs = " ".join(step.get("run", "") for step in verify_steps)
    assert "publish-verify" in runs
    assert EXPECTED_ORIGIN in runs
    assert "publish-report" in runs and "--close" in runs


def test_report_opens_the_managed_issue(document: dict) -> None:
    runs = " ".join(step.get("run", "") for step in _steps(document["jobs"]["report"]))
    assert "publish-report" in runs and "--open" in runs
