"""Structural contract tests for the weekly Material + external-link workflows.

Conditions are inspected as parsed YAML data (including the YAML `on` key,
which parses as boolean True), never by matching raw file text. Shell blocks
are additionally validated with `bash -n` where a shell is available.
"""

import re
import shutil
import subprocess
import tempfile
from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parents[3]
WEEKLY_REUSABLE = ROOT / ".github/workflows/docs-weekly.yml"
WEEKLY_CALLER = ROOT / "tools/docs/host/weekly.yml"
PUBLISH_REUSABLE = ROOT / ".github/workflows/docs-publish.yml"
PUBLISH_CALLER = ROOT / "tools/docs/host/publish.yml"

EXPECTED_CALLER = "bobs-org/bobs-org.github.io"
EXPECTED_ORIGIN = "https://bobs-org.github.io"
WEEKLY_TITLE = "Docs weekly checks blocked"
WEEKLY_LABEL = "docs-weekly"
PUBLISH_TITLE = "Docs publish blocked"
PUBLISH_LABEL = "docs-publish"

WEEKLY_ALLOWED_USES = {
    "actions/checkout@v6",
    "actions/setup-python@v6",
    "astral-sh/setup-uv@v6",
}


def _load(path: Path) -> dict:
    document = yaml.safe_load(path.read_text(encoding="utf-8"))
    assert isinstance(document, dict), f"{path} must parse to a mapping"
    return document


def _triggers(document: dict) -> dict:
    if "on" in document:
        value = document["on"]
        return {} if value is None else value
    value = document.get(True)
    return {} if value is None else value


def _refs_of(mapping: dict) -> set[str]:
    found: set[str] = set()
    _expr = re.compile(r"\{\{\s*(.*?)\s*\}\}")
    _ref = re.compile(r"[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_*-]+|\[[^\]]+\])*")
    condition = mapping.get("if", "") or ""
    if isinstance(condition, str):
        found.update(match.group(0) for match in _ref.finditer(condition))
    for key in ("run", "uses"):
        text = mapping.get(key, "") or ""
        if isinstance(text, str):
            for expression in _expr.finditer(text):
                found.update(match.group(0) for match in _ref.finditer(expression.group(1)))
    for key in ("with", "env"):
        value = mapping.get(key)
        if isinstance(value, dict):
            for text in (f"{name}={item}" for name, item in value.items()):
                for expression in _expr.finditer(text):
                    found.update(match.group(0) for match in _ref.finditer(expression.group(1)))
    return found


def _steps(job: dict) -> list[dict]:
    return [step for step in job.get("steps", []) if isinstance(step, dict)]


def _runs_text(document: dict) -> str:
    parts: list[str] = []
    for job in document.get("jobs", {}).values():
        if not isinstance(job, dict):
            continue
        for step in _steps(job):
            run = step.get("run", "")
            if isinstance(run, str):
                parts.append(run)
    return "\n".join(parts)


@pytest.fixture(scope="module")
def weekly() -> dict:
    assert WEEKLY_REUSABLE.is_file(), "reusable weekly workflow is missing"
    return _load(WEEKLY_REUSABLE)


@pytest.fixture(scope="module")
def caller() -> dict:
    assert WEEKLY_CALLER.is_file(), "weekly caller is missing"
    return _load(WEEKLY_CALLER)


def test_only_workflow_call_triggers_weekly(weekly: dict) -> None:
    assert set(_triggers(weekly)) == {"workflow_call"}


def test_weekly_call_has_no_inputs(weekly: dict) -> None:
    call = _triggers(weekly)["workflow_call"]
    assert call in (None, {}, {"inputs": None}, {"inputs": {}})
    if isinstance(call, dict):
        assert call.get("inputs") in (None, {})


def test_weekly_concurrency_is_separate_group(weekly: dict) -> None:
    assert weekly["concurrency"] == {"group": "docs-weekly", "cancel-in-progress": False}


def test_weekly_actions_are_pinned_release_tags(weekly: dict) -> None:
    for job_name, job in weekly["jobs"].items():
        for step in _steps(job):
            uses = step.get("uses")
            if uses is None:
                continue
            assert re.fullmatch(r"[^@\s]+@v\d+", uses), f"{job_name}: unpinned {uses}"
            assert uses in WEEKLY_ALLOWED_USES, f"{job_name}: unexpected pin {uses}"


def test_weekly_has_no_publish_capabilities(weekly: dict) -> None:
    assert set(weekly["jobs"]) == {"weekly", "report"}
    for job in weekly["jobs"].values():
        permissions = job.get("permissions", {})
        assert permissions.get("contents") == "read"
        assert permissions.get("issues") == "write"
        assert "pages" not in permissions
        assert "id-token" not in permissions
        assert permissions.get("contents") != "write"
    runs = _runs_text(weekly)
    for forbidden in (
        "upload-pages-artifact",
        "deploy-pages",
        "configure-pages",
        "published.json",
        "git push",
        "push --force",
        "push -f",
    ):
        assert forbidden not in runs, f"weekly must not contain {forbidden!r}"


def test_weekly_guards_caller_repo_and_branch(weekly: dict) -> None:
    guard = None
    for step in _steps(weekly["jobs"]["weekly"]):
        if step.get("name") == "Guard caller repository and branch":
            guard = step
            break
    assert guard is not None
    run = guard.get("run", "")
    refs = _refs_of(guard)
    assert EXPECTED_CALLER in run
    assert "github.repository" in refs
    assert "default_branch" in run
    assert "github.ref_name" in refs


def test_weekly_builds_strict_material_locked(weekly: dict) -> None:
    runs = _runs_text(weekly)
    assert "bob-docs build --material" in runs
    assert "--group material" in runs
    assert "--locked" in runs


def test_weekly_runs_online_links_bounded(weekly: dict) -> None:
    runs = _runs_text(weekly)
    assert "bob-docs links --online" in runs
    assert "--offline" not in runs.replace("bob-docs links --online", "")
    # online_argv keeps fragment checking on the built tree
    assert "--include-fragments" not in runs  # fragments live in bob-docs, not the workflow
    # bounded retries/timeouts are the CLI defaults; the workflow must not add broad excludes
    assert "--exclude " not in runs
    assert "--exclude=" not in runs


def test_weekly_issue_is_isolated_from_publish(weekly: dict) -> None:
    runs = _runs_text(weekly)
    assert WEEKLY_TITLE in runs
    assert WEEKLY_LABEL in runs
    assert PUBLISH_TITLE not in runs
    assert PUBLISH_LABEL not in runs
    assert "publish-report --close" in runs
    assert "publish-report --open" in runs
    assert "failure()" in str(weekly["jobs"]["report"].get("if", ""))
    assert weekly["jobs"]["report"].get("needs") == "weekly"


def test_weekly_report_closes_only_its_issue(weekly: dict) -> None:
    weekly_steps = " ".join(
        step.get("run", "") for step in _steps(weekly["jobs"]["weekly"])
    )
    assert "--close" in weekly_steps
    assert WEEKLY_LABEL in weekly_steps
    report_steps = " ".join(
        step.get("run", "") for step in _steps(weekly["jobs"]["report"])
    )
    assert "--open" in report_steps
    assert WEEKLY_TITLE in report_steps


def test_caller_schedule_and_dispatch_only(caller: dict) -> None:
    triggers = _triggers(caller)
    assert set(triggers) == {"schedule", "workflow_dispatch"}
    assert triggers["schedule"] == [{"cron": "41 6 * * 1"}]
    dispatch = triggers["workflow_dispatch"]
    assert dispatch in (None, {}, {"inputs": None}, {"inputs": {}})
    if isinstance(dispatch, dict) and dispatch.get("inputs"):
        raise AssertionError("weekly caller must not take publish inputs")


def test_caller_uses_reusable_without_inputs(caller: dict) -> None:
    job = caller["jobs"]["weekly"]
    assert job["uses"] == "bobs-org/bob-cli/.github/workflows/docs-weekly.yml@master"
    assert job.get("with", None) in (None, {})
    assert job["permissions"] == {"contents": "read", "issues": "write"}
    assert "concurrency" not in caller
    assert set(caller["jobs"]) == {"weekly"}


def test_caller_does_not_trigger_inside_bob_cli(caller: dict) -> None:
    triggers = _triggers(caller)
    assert "push" not in triggers
    assert "pull_request" not in triggers
    assert "workflow_call" not in triggers


def test_all_four_workflows_validate() -> None:
    for path in (WEEKLY_REUSABLE, WEEKLY_CALLER, PUBLISH_REUSABLE, PUBLISH_CALLER):
        assert path.is_file(), f"missing workflow file: {path}"
        document = _load(path)
        assert isinstance(document.get("jobs"), dict), f"{path} has no jobs"
        assert isinstance(document.get("name"), str) and document["name"].strip()


def test_shell_blocks_parse_with_bash() -> None:
    if shutil.which("bash") is None:
        pytest.skip("bash is not available")
    for path in (WEEKLY_REUSABLE, WEEKLY_CALLER):
        document = _load(path)
        for job_name, job in document["jobs"].items():
            for step in _steps(job):
                run = step.get("run")
                if not isinstance(run, str) or not run.strip():
                    continue
                with tempfile.NamedTemporaryFile(
                    mode="w", suffix=".sh", delete=False, encoding="utf-8"
                ) as handle:
                    handle.write(run)
                    name = handle.name
                try:
                    completed = subprocess.run(
                        ["bash", "-n", name],
                        stdout=subprocess.PIPE,
                        stderr=subprocess.PIPE,
                        text=True,
                        timeout=30,
                        check=False,
                    )
                finally:
                    Path(name).unlink(missing_ok=True)
                assert completed.returncode == 0, (
                    f"{path}:{job_name} shell does not parse: {completed.stderr.strip()}"
                )


def test_tool_versions_are_pinned_without_extra_framework(weekly: dict) -> None:
    runs = _runs_text(weekly)
    assert 'python-version: "3.12"' in WEEKLY_REUSABLE.read_text(encoding="utf-8")
    assert "actions/checkout@v6" in runs or "checkout@v6" in WEEKLY_REUSABLE.read_text(
        encoding="utf-8"
    )
    assert "astral-sh/setup-uv@v6" in WEEKLY_REUSABLE.read_text(encoding="utf-8")
    assert "actions/setup-python@v6" in WEEKLY_REUSABLE.read_text(encoding="utf-8")
