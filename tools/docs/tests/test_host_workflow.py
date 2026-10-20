"""YAML-aware contract tests for the thin host publish caller."""

import re
import shlex
import shutil
import subprocess
from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parents[3]
CALLER = ROOT / "tools/docs/host/publish.yml"
CALLEE = ROOT / ".github/workflows/docs-publish.yml"


def _load(path: Path) -> dict:
    document = yaml.safe_load(path.read_text(encoding="utf-8"))
    assert isinstance(document, dict)
    return document


def _triggers(document: dict) -> dict:
    if "on" in document:
        return document["on"]
    return document[True]


def test_caller_and_callee_files_exist() -> None:
    assert CALLER.is_file()
    assert CALLEE.is_file()
    assert (ROOT / "tools/docs/host/README.md").is_file()


@pytest.mark.parametrize(
    "document", ["tools/docs/host/README.md", "tools/docs/README.md", "docs/develop/build.md"]
)
def test_documented_host_commands_forward_parseable_arguments(document: str) -> None:
    from bob_docs.cli import _parser

    if shutil.which("just") is None:
        pytest.skip("just is required to exercise documented recipe arguments")
    text = (ROOT / document).read_text(encoding="utf-8")
    commands = re.findall(r"^\s*(just (?:-- )?docs-(?:host-bootstrap|publish)\b.*)$", text, re.M)
    assert commands
    for command in commands:
        result = subprocess.run(
            ["just", "--dry-run", *shlex.split(command, comments=True)[1:]],
            cwd=ROOT, capture_output=True, text=True, check=True,
        )
        forwarded = next(
            line for line in result.stderr.splitlines()
            if line.startswith("uv run --project tools/docs bob-docs ")
        )
        args = shlex.split(forwarded)[5:]
        _parser().parse_args(args)


def test_caller_schedule_and_dispatch_only() -> None:
    triggers = _triggers(_load(CALLER))
    assert set(triggers) == {"schedule", "workflow_dispatch"}
    assert triggers["schedule"] == [{"cron": "17 * * * *"}]
    inputs = triggers["workflow_dispatch"]["inputs"]
    assert set(inputs) >= {"force", "drill", "pinned"}
    for name in ("force", "drill", "pinned"):
        assert inputs[name]["type"] == "boolean"
        assert inputs[name]["default"] is False


def test_caller_with_block_matches_callee_inputs() -> None:
    caller = _load(CALLER)
    callee = _load(CALLEE)
    callee_inputs = set(_triggers(callee)["workflow_call"]["inputs"])
    job = caller["jobs"]["publish"]
    assert job["uses"] == "bobs-org/bob-cli/.github/workflows/docs-publish.yml@master"
    assert set(job["with"]) == callee_inputs == {"force", "drill", "pinned"}


def test_caller_grants_required_permissions_without_concurrency() -> None:
    caller = _load(CALLER)
    assert "concurrency" not in caller
    job = caller["jobs"]["publish"]
    assert "runs-on" not in job
    assert job["permissions"] == {
        "contents": "write",
        "id-token": "write",
        "issues": "write",
        "pages": "write",
    }
    assert set(caller["jobs"]) == {"publish"}


def test_caller_run_name_includes_correlation() -> None:
    text = CALLER.read_text(encoding="utf-8")
    assert "run-name:" in text
    assert "correlation" in text
    assert re.search(r"inputs\.correlation", text)


def test_caller_does_not_trigger_inside_bob_cli() -> None:
    triggers = _triggers(_load(CALLER))
    assert "push" not in triggers
    assert "pull_request" not in triggers
    assert "workflow_call" not in triggers
