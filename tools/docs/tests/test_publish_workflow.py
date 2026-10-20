"""Structural contract tests for the reusable gated-publication workflow.

Conditions are inspected as parsed YAML data (including the YAML `on` key,
which parses as boolean True), never by matching raw file text.
"""

import json
import os
import re
import shutil
import subprocess
import sys
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
    "actions/setup-node@v7",
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


def test_every_job_declares_ubuntu_latest(document: dict) -> None:
    jobs = document["jobs"]
    assert jobs, "reusable publication workflow has no jobs"
    for job_name, job in jobs.items():
        assert job.get("runs-on") == "ubuntu-latest", f"{job_name} missing runs-on"


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


def test_publish_sets_up_node_before_production_gates(document: dict) -> None:
    steps = _steps(document["jobs"]["publish"])
    setup = next(i for i, step in enumerate(steps) if step.get("uses") == "actions/setup-node@v7")
    gates = next(i for i, step in enumerate(steps) if step.get("name") == "Run the gated production checks")
    assert setup < gates
    assert steps[setup].get("with", {}).get("node-version") == "24"


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


@pytest.mark.parametrize("concurrent_hold", [False, True])
def test_record_script_pushes_only_manifest_and_preserves_remote_changes(
    document: dict, tmp_path: Path, concurrent_hold: bool
) -> None:
    """Exercise the real record script against an isolated Git remote."""
    env = {
        **os.environ,
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_CONFIG_GLOBAL": os.devnull,
        "GIT_AUTHOR_NAME": "Fixture",
        "GIT_AUTHOR_EMAIL": "fixture@example.invalid",
        "GIT_COMMITTER_NAME": "Fixture",
        "GIT_COMMITTER_EMAIL": "fixture@example.invalid",
    }

    def git(cwd: Path, *args: str) -> str:
        return subprocess.run(
            ["git", *args], cwd=cwd, env=env, check=True,
            capture_output=True, text=True,
        ).stdout.strip()

    remote = tmp_path / "remote.git"
    git(tmp_path, "init", "--bare", "--initial-branch=main", str(remote))
    operator = tmp_path / "operator"
    git(tmp_path, "clone", str(remote), str(operator))
    (operator / "README.md").write_text("host\n")
    git(operator, "add", "README.md")
    git(operator, "commit", "-m", "Seed synthetic host")
    git(operator, "push", "origin", "main")
    checkout = tmp_path / "record"
    git(tmp_path, "clone", str(remote), str(checkout))
    (checkout / "handoff").mkdir()
    manifest = '{"hold":false,"fingerprint":"fixture"}\n'
    (checkout / "handoff/published.json").write_text(manifest)
    (checkout / "README.md").write_text("unrelated local change\n")
    (checkout / "unrelated.txt").write_text("do not publish\n")

    hold = '{"hold":true,"fingerprint":"operator"}\n'
    if concurrent_hold:
        (operator / "published.json").write_text(hold)
        git(operator, "add", "published.json")
        git(operator, "commit", "-m", "Hold publication")
        git(operator, "push", "origin", "main")

    deploy_sha = git(checkout, "rev-parse", "HEAD")
    git(checkout, "checkout", "--detach", deploy_sha)

    script = _step_by_name(
        document["jobs"]["record"], "Commit only the publication manifest"
    )["run"]
    script = script.replace("${{ github.repository }}", EXPECTED_CALLER)
    script = script.replace("${{ github.run_id }}", "42")
    script = script.replace("${{ github.ref_name }}", "main")

    def record() -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["bash", "-e", "-c", script], cwd=checkout,
            env={
                key: value for key, value in env.items()
                if not key.startswith(("GIT_AUTHOR_", "GIT_COMMITTER_"))
            },
            capture_output=True, text=True,
        )

    result = record()
    if concurrent_hold:
        assert result.returncode != 0
        assert "[rejected]" in result.stderr, result.stderr
        assert git(remote, "show", "main:published.json") == hold.strip()
        return

    assert result.returncode == 0, result.stderr
    assert git(remote, "show", "main:published.json") == manifest.strip()
    assert git(remote, "show", "main:README.md") == "host"
    assert git(remote, "log", "-1", "--format=%an <%ae>") == (
        "github-actions[bot] <41898282+github-actions[bot]@users.noreply.github.com>"
    )
    assert git(remote, "diff-tree", "--no-commit-id", "--name-only", "-r", "main") == (
        "published.json"
    )
    assert git(remote, "log", "-1", "--format=%B").splitlines() == [
        "Record documentation publication",
        "",
        f"Run: https://github.com/{EXPECTED_CALLER}/actions/runs/42",
    ]
    previous = git(remote, "rev-parse", "main")
    result = record()
    assert result.returncode == 0, result.stderr
    assert "no manifest change" in result.stdout
    assert git(remote, "rev-parse", "main") == previous


def test_record_checks_out_the_planned_deploy_sha(document: dict) -> None:
    assert "deploy_sha" in document["jobs"]["publish"]["outputs"]
    checkout = next(
        step
        for step in _steps(document["jobs"]["record"])
        if str(step.get("uses", "")).startswith("actions/checkout")
    )
    assert checkout["with"]["ref"] == "${{ needs.publish.outputs.deploy_sha }}"
    assert checkout["with"]["fetch-depth"] == 0


def test_record_script_succeeds_when_queued_behind_an_earlier_recorder(
    document: dict, tmp_path: Path
) -> None:
    """A queued run records on the planned deploy SHA, not the stale trigger."""
    env = {
        **os.environ,
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_CONFIG_GLOBAL": os.devnull,
        "GIT_AUTHOR_NAME": "Fixture",
        "GIT_AUTHOR_EMAIL": "fixture@example.invalid",
        "GIT_COMMITTER_NAME": "Fixture",
        "GIT_COMMITTER_EMAIL": "fixture@example.invalid",
    }

    def git(cwd: Path, *args: str) -> str:
        return subprocess.run(
            ["git", *args], cwd=cwd, env=env, check=True,
            capture_output=True, text=True,
        ).stdout.strip()

    remote = tmp_path / "remote.git"
    git(tmp_path, "init", "--bare", "--initial-branch=main", str(remote))
    operator = tmp_path / "operator"
    git(tmp_path, "clone", str(remote), str(operator))
    (operator / "README.md").write_text("host\n")
    git(operator, "add", "README.md")
    git(operator, "commit", "-m", "Seed synthetic host")
    git(operator, "push", "origin", "main")
    trigger = git(operator, "rev-parse", "HEAD")

    earlier = '{"hold":false,"fingerprint":"earlier"}\n'
    (operator / "published.json").write_text(earlier)
    git(operator, "add", "published.json")
    git(operator, "commit", "-m", "Record documentation publication")
    git(operator, "push", "origin", "main")
    deploy_sha = git(operator, "rev-parse", "HEAD")
    assert deploy_sha != trigger

    checkout = tmp_path / "record"
    git(tmp_path, "clone", str(remote), str(checkout))
    git(checkout, "checkout", "--detach", deploy_sha)
    (checkout / "handoff").mkdir()
    manifest = '{"hold":false,"fingerprint":"queued"}\n'
    (checkout / "handoff/published.json").write_text(manifest)

    script = _step_by_name(
        document["jobs"]["record"], "Commit only the publication manifest"
    )["run"]
    script = script.replace("${{ github.repository }}", EXPECTED_CALLER)
    script = script.replace("${{ github.run_id }}", "42")
    script = script.replace("${{ github.ref_name }}", "main")
    result = subprocess.run(
        ["bash", "-e", "-c", script], cwd=checkout,
        env={
            key: value for key, value in env.items()
            if not key.startswith(("GIT_AUTHOR_", "GIT_COMMITTER_"))
        },
        capture_output=True, text=True,
    )
    assert result.returncode == 0, result.stderr
    assert git(remote, "show", "main:published.json") == manifest.strip()
    assert git(remote, "rev-parse", "main^") == deploy_sha
    assert git(remote, "diff-tree", "--no-commit-id", "--name-only", "-r", "main") == (
        "published.json"
    )
    assert git(remote, "log", "-1", "--format=%B").splitlines() == [
        "Record documentation publication",
        "",
        f"Run: https://github.com/{EXPECTED_CALLER}/actions/runs/42",
    ]


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
    assert "--json number,title" in run
    assert 'select(.title == "Docs publish blocked")' in run
    assert "--recovery" in run
    assert {"inputs.force", "inputs.drill", "inputs.pinned"} <= refs


@pytest.mark.parametrize("fail", [False, True])
def test_plan_runs_from_tooling_checkout(
    document: dict, tmp_path: Path, fail: bool
) -> None:
    """Run the actual plan script from the Actions workspace, outside either repo."""
    (tmp_path / "tooling").mkdir()
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    uv = bin_dir / "uv"
    uv.write_text(
        f"#!{sys.executable}\n"
        "import os, sys\n"
        "from pathlib import Path\n"
        "from bob_docs import cli\n"
        "from bob_docs.errors import DocsError\n"
        "def plan(root, **kwargs):\n"
        "    assert root == Path(os.environ['GITHUB_WORKSPACE']) / 'tooling', root\n"
        "    assert kwargs['manifest'].is_absolute()\n"
        f"    if {fail!r}: raise DocsError('unavailable source')\n"
        "    return {'should_build': True}\n"
        "cli.run_publish_plan = plan\n"
        "sys.exit(cli.main(sys.argv[sys.argv.index('bob-docs') + 1:]))\n"
    )
    uv.chmod(0o755)
    gh = bin_dir / "gh"
    gh.write_text("#!/bin/sh\nprintf '0\\n'\n")
    gh.chmod(0o755)
    step = _step_by_name(
        document["jobs"]["publish"], "Plan immutable publication sources"
    )
    script = step["run"].replace("${{ github.repository }}", EXPECTED_CALLER)
    for flag in ("force", "drill", "pinned"):
        script = script.replace("${{ inputs." + flag + " }}", "false")
    result = subprocess.run(
        ["bash", "-e", "-c", script],
        cwd=tmp_path,
        env={
            **os.environ,
            "PATH": f"{bin_dir}{os.pathsep}{os.environ['PATH']}",
            "PYTHONPATH": str(ROOT / "tools/docs"),
            "GITHUB_WORKSPACE": str(tmp_path),
            "GITHUB_OUTPUT": str(tmp_path / "outputs"),
        },
        capture_output=True,
        text=True,
    )
    assert result.returncode == (2 if fail else 0), result.stderr
    assert "stage=plan" in (tmp_path / "outputs").read_text()


@pytest.mark.parametrize("workflow", ["docs-publish.yml", "docs-weekly.yml"])
def test_issue_reporting_targets_caller_despite_tooling_checkout(workflow: str) -> None:
    document = yaml.safe_load((WORKFLOW.parent / workflow).read_text())
    reporting_steps = [
        step
        for job in document["jobs"].values()
        for step in _steps(job)
        if "bob-docs publish-report" in step.get("run", "")
    ]
    assert len(reporting_steps) == 2  # open on failure and close on verified success
    for step in reporting_steps:
        assert step["env"].get("GH_REPO") == "${{ github.repository }}", step["name"]


def test_verify_checks_live_site_then_closes_the_issue(document: dict) -> None:
    verify_steps = _steps(document["jobs"]["verify"])
    runs = " ".join(step.get("run", "") for step in verify_steps)
    assert "publish-verify" in runs
    assert EXPECTED_ORIGIN in runs
    assert "publish-report" in runs and "--close" in runs


def test_publication_handoff_supplies_live_verification_inputs(
    document: dict, tmp_path: Path
) -> None:
    """Run the producer and consumer scripts with network operations stubbed."""
    from bob_docs.publish_plan import fingerprint_inputs

    inputs = [
        {"repo": "bob-cli", "path": "docs", "kind": "tree", "object_id": "a" * 40}
    ]
    candidate = {
        "schema_version": 1,
        "hold": False,
        "sources": {
            "bob-cli": {
                "remote": "https://example.invalid/bob-cli.git",
                "branch": "master",
                "sha": "b" * 40,
            }
        },
        "inputs": inputs,
        "fingerprint": fingerprint_inputs(inputs),
    }
    plan = tmp_path / ".publish-plan"
    plan.mkdir()
    (plan / "next-publication.json").write_text(json.dumps(candidate))
    site = tmp_path / ".publish-work/bob-cli/target/docs/site"
    site.mkdir(parents=True)
    (site / "urls.txt").write_text(EXPECTED_ORIGIN + "/\n")
    (tmp_path / "tooling").mkdir()
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    uv = bin_dir / "uv"
    uv.write_text(
        f"#!{sys.executable}\n"
        "import sys\n"
        "from bob_docs import cli\n"
        "from bob_docs.publish_verify import _read_expected_publication, _read_expected_urls\n"
        "def verify(root, *, expected_file, site_urls_file, **kwargs):\n"
        "    assert _read_expected_publication(expected_file)['sources']['bob-cli']['sha'] == 'b' * 40\n"
        f"    assert _read_expected_urls(site_urls_file) == [{EXPECTED_ORIGIN + '/'!r}]\n"
        "    print('handoff verified')\n"
        "    return 0\n"
        "cli.run_publish_verify = verify\n"
        "args = sys.argv[sys.argv.index('bob-docs') + 1:]\n"
        "if args[0] == 'publish-report':\n"
        "    assert '--close' in args\n"
        "    sys.exit(0)\n"
        "sys.exit(cli.main(args))\n"
    )
    uv.chmod(0o755)
    env = {
        **os.environ,
        "PATH": f"{bin_dir}{os.pathsep}{os.environ['PATH']}",
        "PYTHONPATH": str(ROOT / "tools/docs"),
        "GITHUB_WORKSPACE": str(tmp_path),
        "GITHUB_OUTPUT": str(tmp_path / "outputs"),
    }
    prepare = _step_by_name(
        document["jobs"]["publish"], "Prepare the publication record"
    )["run"]
    prepare = prepare.replace("${{ github.repository }}", EXPECTED_CALLER)
    prepare = prepare.replace("${{ github.run_id }}", "42")
    subprocess.run(
        ["bash", "-e", "-c", prepare], cwd=tmp_path, env=env, check=True,
        capture_output=True, text=True,
    )
    upload = _step_by_name(
        document["jobs"]["publish"], "Upload the publication handoff"
    )
    # upload-artifact v4 excludes dot directories unless explicitly enabled.
    if not upload["with"].get("include-hidden-files", False):
        assert not any(
            part.startswith(".") for part in Path(upload["with"]["path"]).parts
        ), "publication handoff would be excluded from the artifact"
    download = _step_by_name(
        document["jobs"]["verify"], "Download the publication handoff"
    )
    shutil.copytree(
        tmp_path / upload["with"]["path"], tmp_path / download["with"]["path"]
    )
    verify = _step_by_name(
        document["jobs"]["verify"], "Verify live pages, fragments, and provenance"
    )["run"]
    verify = verify.replace("${{ github.repository }}", EXPECTED_CALLER)
    verify = verify.replace("${{ github.run_id }}", "42")
    result = subprocess.run(
        ["bash", "-e", "-c", verify], cwd=tmp_path, env=env,
        capture_output=True, text=True,
    )
    assert result.returncode == 0, result.stderr
    assert "handoff verified" in result.stdout


def test_report_opens_the_managed_issue(document: dict) -> None:
    runs = " ".join(step.get("run", "") for step in _steps(document["jobs"]["report"]))
    assert "publish-report" in runs and "--open" in runs
