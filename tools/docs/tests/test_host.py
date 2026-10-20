"""Operator host bootstrap, dispatch, and status tests.

GitHub calls are mocked. Apply tests use a real git checkout for file
survival and commit idempotence. Preview must not mutate.
"""

from __future__ import annotations

import base64
import json
import os
import subprocess
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

import pytest

from bob_docs.cli import _parser
from bob_docs.errors import DocsError
from bob_docs.host import (
    CALLER_WORKFLOW,
    HOST_REPO,
    MANIFEST_PATH,
    WEEKLY_CALLER_WORKFLOW,
    WEEKLY_WORKFLOW_PATH,
    build_bootstrap_plan,
    collect_status,
    default_host_run,
    format_status,
    inspect_repo,
    list_open_weekly_issues,
    load_host_templates,
    run_host_bootstrap,
    run_publish,
    run_status,
    select_dispatched_run,
)
from bob_docs.publish_issue import CommandResult
from bob_docs.publish_verify import FetchedPage

ROOT = Path(__file__).resolve().parents[3]
MANIFEST = json.dumps(
    {
        "schema_version": 1,
        "hold": True,
        "sources": {
            "bob-cli": {
                "remote": "https://github.com/bobs-org/bob-cli.git",
                "branch": "master",
                "sha": "a" * 40,
            }
        },
        "fingerprint": "b" * 64,
        "run_url": "https://example.test/runs/1",
    },
    indent=2,
) + "\n"


def _git_env() -> dict[str, str]:
    return {
        **os.environ,
        "GIT_AUTHOR_NAME": "Host tests",
        "GIT_AUTHOR_EMAIL": "host-tests@example.invalid",
        "GIT_COMMITTER_NAME": "Host tests",
        "GIT_COMMITTER_EMAIL": "host-tests@example.invalid",
    }


def _run_git(cwd: Path, *args: str) -> str:
    result = subprocess.run(
        args,
        cwd=cwd,
        check=True,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=_git_env(),
    )
    return result.stdout.strip()


def _init_deploy(path: Path, files: dict[str, str], branch: str = "master") -> tuple[Path, Path]:
    bare = path.with_suffix(".git")
    bare.parent.mkdir(parents=True, exist_ok=True)
    _run_git(path.parent, "git", "init", "--bare", "--initial-branch", branch, str(bare))
    working = path
    working.mkdir(parents=True)
    _run_git(working, "git", "init", "--initial-branch", branch)
    for relative, content in files.items():
        destination = working / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(content, encoding="utf-8")
    _run_git(working, "git", "add", ".")
    _run_git(working, "git", "commit", "-m", "seed")
    _run_git(working, "git", "remote", "add", "origin", str(bare))
    _run_git(working, "git", "push", "-u", "origin", branch)
    return bare, working


def _b64(text: str) -> str:
    return base64.b64encode(text.encode("utf-8")).decode("ascii")


def _http_error(status: int, message: str = "") -> CommandResult:
    label = {404: "Not Found", 403: "Forbidden", 401: "Unauthorized"}.get(status, "Error")
    return CommandResult(1, "", f"gh: {message or label} (HTTP {status})\n")


class FakeHost:
    """Script GitHub responses and execute real git commands."""

    def __init__(self) -> None:
        self.calls: list[tuple[list[str], str | None]] = []
        self.repo_status = 200
        self.repo: dict[str, Any] = {"default_branch": "master", "full_name": HOST_REPO}
        self.files: dict[str, str] = {}
        self.pages_status = 404
        self.pages: dict[str, Any] | None = None
        self.actions_status = 200
        self.actions: dict[str, Any] = {"enabled": True, "allowed_actions": "all"}
        self.workflows_status = 200
        self.workflows: list[dict[str, Any]] = []
        self.runs: list[dict[str, Any]] = []
        self.issues: list[dict[str, Any]] = []
        self.create_error: CommandResult | None = None
        self.enable_error: str | None = None
        self.dispatch_error: str | None = None
        self.watch_code = 0
        self.created = False
        self.enabled: list[str] = []
        self.dispatched: list[list[str]] = []

    def mutating_calls(self) -> list[list[str]]:
        mutations: list[list[str]] = []
        for argv, _stdin in self.calls:
            if argv[0] == "git" and any(part in {"clone", "commit", "push", "add"} for part in argv):
                mutations.append(argv)
            elif tuple(argv[:3]) in {
                ("gh", "repo", "create"),
                ("gh", "workflow", "enable"),
                ("gh", "workflow", "run"),
            }:
                mutations.append(argv)
            elif argv[:2] == ["gh", "api"] and len(argv) > 3 and argv[3] in {"POST", "PUT", "PATCH", "DELETE"}:
                mutations.append(argv)
        return mutations

    def __call__(self, argv: list[str], stdin_text: str | None) -> CommandResult:
        self.calls.append((list(argv), stdin_text))
        if argv[0] == "git":
            completed = subprocess.run(
                argv,
                input=stdin_text,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
                env=_git_env(),
            )
            return CommandResult(completed.returncode, completed.stdout, completed.stderr)
        if argv[0] != "gh":
            raise AssertionError(f"unexpected command: {argv}")
        if argv[:3] == ["gh", "repo", "create"]:
            if self.create_error is not None:
                return self.create_error
            self.created = True
            self.repo_status = 200
            return CommandResult(0, f"https://github.com/{HOST_REPO}\n", "")
        if argv[:3] == ["gh", "workflow", "enable"]:
            if self.enable_error is not None:
                return CommandResult(1, "", self.enable_error)
            self.enabled.append(argv[3])
            for item in self.workflows:
                if str(item.get("path", "")).endswith(argv[3]):
                    item["state"] = "active"
            if not self.workflows:
                self.workflows.append(
                    {
                        "name": "Publish documentation",
                        "path": f".github/workflows/{argv[3]}",
                        "state": "active",
                    }
                )
            return CommandResult(0, "", "")
        if argv[:3] == ["gh", "workflow", "run"]:
            if self.dispatch_error is not None:
                return CommandResult(1, "", self.dispatch_error)
            self.dispatched.append(list(argv))
            return CommandResult(0, "", "")
        if argv[:3] == ["gh", "run", "list"]:
            return CommandResult(0, json.dumps(self.runs), "")
        if argv[:3] == ["gh", "run", "watch"]:
            return CommandResult(self.watch_code, "", "" if self.watch_code == 0 else "run failed")
        if argv[:3] == ["gh", "issue", "list"]:
            return CommandResult(0, json.dumps(self.issues), "")
        if argv[:2] == ["gh", "api"]:
            return self._api(argv, stdin_text)
        raise AssertionError(f"unexpected gh call: {argv}")

    def _api(self, argv: list[str], stdin_text: str | None) -> CommandResult:
        method = argv[argv.index("--method") + 1]
        path = argv[argv.index("--method") + 2]
        if method in {"POST", "PUT", "PATCH", "DELETE"}:
            return self._api_write(method, path, stdin_text)
        return self._api_read(path)

    def _api_read(self, path: str) -> CommandResult:
        if path == f"repos/{HOST_REPO}":
            if self.repo_status != 200:
                return _http_error(self.repo_status)
            return CommandResult(0, json.dumps(self.repo), "")
        prefix = f"repos/{HOST_REPO}/contents/"
        if path.startswith(prefix):
            relative = path[len(prefix) :]
            if relative not in self.files:
                return _http_error(404)
            payload = {
                "content": _b64(self.files[relative]),
                "encoding": "base64",
                "path": relative,
            }
            return CommandResult(0, json.dumps(payload), "")
        if path == f"repos/{HOST_REPO}/pages":
            if self.pages_status != 200:
                return _http_error(self.pages_status)
            return CommandResult(0, json.dumps(self.pages or {"build_type": "workflow"}), "")
        if path == f"repos/{HOST_REPO}/actions/permissions":
            if self.actions_status != 200:
                return _http_error(self.actions_status)
            return CommandResult(0, json.dumps(self.actions), "")
        if path == f"repos/{HOST_REPO}/actions/workflows":
            if self.workflows_status != 200:
                return _http_error(self.workflows_status)
            return CommandResult(0, json.dumps({"workflows": self.workflows}), "")
        raise AssertionError(f"unexpected API GET {path}")

    def _api_write(self, method: str, path: str, stdin_text: str | None) -> CommandResult:
        if path == f"repos/{HOST_REPO}/pages":
            body = json.loads(stdin_text or "{}")
            self.pages = {"build_type": body.get("build_type", "workflow")}
            self.pages_status = 200
            return CommandResult(0, json.dumps(self.pages), "")
        if path == f"repos/{HOST_REPO}/actions/permissions":
            body = json.loads(stdin_text or "{}")
            self.actions = {
                "enabled": bool(body.get("enabled", True)),
                "allowed_actions": body.get("allowed_actions", "all"),
            }
            self.actions_status = 200
            return CommandResult(0, json.dumps(self.actions), "")
        raise AssertionError(f"unexpected API {method} {path}")


def test_inspect_repo_accepts_ansi_colored_json() -> None:
    colored = (
        "\x1b[1;37m{\x1b[m\n"
        '  \x1b[1;34m"default_branch"\x1b[m: \x1b[32m"main"\x1b[m\n'
        "}\n"
    )

    def run(argv: list[str], stdin_text: str | None) -> CommandResult:
        assert argv[:4] == ["gh", "api", "--method", "GET"]
        return CommandResult(0, colored, "")

    status, repo = inspect_repo(run)
    assert status == 200
    assert repo == {"default_branch": "main"}


def test_cli_help_and_short_aliases() -> None:
    parser = _parser()
    bootstrap = parser.parse_args(["host-bootstrap", "-y", "-d", "/tmp/deploy"])
    assert bootstrap.command == "host-bootstrap"
    assert bootstrap.yes is True
    assert bootstrap.deploy_dir == "/tmp/deploy"
    publish = parser.parse_args(["publish", "-f", "-d", "-p"])
    assert publish.force and publish.drill and publish.pinned
    status = parser.parse_args(["status"])
    assert status.command == "status"
    help_text = parser.format_help()
    for name in ("host-bootstrap", "publish", "status"):
        assert name in help_text


def test_preview_makes_no_mutations(tmp_path: Path) -> None:
    host = FakeHost()
    host.repo_status = 404
    chunks: list[str] = []
    status = run_host_bootstrap(
        ROOT,
        yes=False,
        run=host,
        sleep=lambda _seconds: None,
        stdout_write=chunks.append,
        clone_dir=tmp_path / "clone",
    )
    assert status == 0
    assert any("preview" in chunk for chunk in chunks)
    assert any(HOST_REPO in chunk for chunk in chunks)
    assert host.mutating_calls() == []
    assert host.created is False
    git_calls = [argv for argv, _ in host.calls if argv[0] == "git"]
    assert git_calls == []


def test_403_and_transport_errors_do_not_create() -> None:
    host = FakeHost()
    host.repo_status = 403
    with pytest.raises(DocsError, match="permission denied"):
        build_bootstrap_plan(ROOT, run=host)
    assert not any(argv[:3] == ["gh", "repo", "create"] for argv, _ in host.calls)

    def transport(argv: list[str], stdin_text: str | None) -> CommandResult:
        host.calls.append((list(argv), stdin_text))
        return CommandResult(1, "", "connection reset by peer")

    with pytest.raises(DocsError, match="GitHub request failed"):
        build_bootstrap_plan(ROOT, run=transport)
    assert not any(argv[:3] == ["gh", "repo", "create"] for argv, _ in host.calls)


def test_apply_is_idempotent_and_preserves_unrelated_files(tmp_path: Path) -> None:
    templates = load_host_templates(ROOT)
    published = MANIFEST
    notes = "operator notes\n"
    _bare, working = _init_deploy(
        tmp_path / "deploy",
        {
            MANIFEST_PATH: published,
            "NOTES.md": notes,
            "README.md": "stale readme\n",
        },
    )
    host = FakeHost()
    host.pages_status = 404
    host.actions = {"enabled": False, "allowed_actions": "all"}
    chunks: list[str] = []
    first = run_host_bootstrap(
        ROOT,
        yes=True,
        deploy_dir=working,
        run=host,
        sleep=lambda _seconds: None,
        stdout_write=chunks.append,
    )
    assert first == 0
    assert (working / "README.md").read_text(encoding="utf-8") == templates["README.md"]
    workflow = working / ".github/workflows/publish.yml"
    assert workflow.read_text(encoding="utf-8") == templates[".github/workflows/publish.yml"]
    weekly = working / ".github/workflows/weekly.yml"
    assert weekly.read_text(encoding="utf-8") == templates[".github/workflows/weekly.yml"]
    assert (working / MANIFEST_PATH).read_text(encoding="utf-8") == published
    assert (working / "NOTES.md").read_text(encoding="utf-8") == notes
    log_after_first = _run_git(working, "git", "rev-parse", "HEAD")
    add_calls = [
        argv for argv, _ in host.calls if argv[0] == "git" and "add" in argv
    ]
    for argv in add_calls:
        assert MANIFEST_PATH not in argv
        assert "NOTES.md" not in argv
        assert "." not in argv[argv.index("add") + 1 :]
    assert host.pages is not None and host.pages["build_type"] == "workflow"
    assert host.actions["enabled"] is True
    assert CALLER_WORKFLOW in host.enabled
    assert WEEKLY_CALLER_WORKFLOW in host.enabled

    host.pages_status = 200
    host.actions = {"enabled": True, "allowed_actions": "all"}
    host.workflows = [
        {
            "name": "Publish documentation",
            "path": ".github/workflows/publish.yml",
            "state": "active",
        },
        {
            "name": "Check weekly documentation",
            "path": ".github/workflows/weekly.yml",
            "state": "active",
        },
    ]
    second_chunks: list[str] = []
    second = run_host_bootstrap(
        ROOT,
        yes=True,
        deploy_dir=working,
        run=host,
        sleep=lambda _seconds: None,
        stdout_write=second_chunks.append,
    )
    assert second == 0
    assert _run_git(working, "git", "rev-parse", "HEAD") == log_after_first
    assert (working / MANIFEST_PATH).read_text(encoding="utf-8") == published
    assert (working / "NOTES.md").read_text(encoding="utf-8") == notes
    assert any("no file commit" in chunk for chunk in second_chunks)


def test_apply_commits_only_maintained_files_when_index_is_dirty(tmp_path: Path) -> None:
    templates = load_host_templates(ROOT)
    published = MANIFEST
    _bare, working = _init_deploy(
        tmp_path / "deploy",
        {
            MANIFEST_PATH: published,
            "README.md": "stale readme\n",
        },
    )
    (working / MANIFEST_PATH).write_text(
        published.replace('"hold": true', '"hold": false'),
        encoding="utf-8",
    )
    _run_git(working, "git", "add", MANIFEST_PATH)
    assert MANIFEST_PATH in _run_git(working, "git", "diff", "--cached", "--name-only")

    host = FakeHost()
    host.pages_status = 404
    host.actions = {"enabled": False, "allowed_actions": "all"}
    status = run_host_bootstrap(
        ROOT,
        yes=True,
        deploy_dir=working,
        run=host,
        sleep=lambda _seconds: None,
        stdout_write=lambda _text: None,
    )
    assert status == 0
    committed = _run_git(
        working, "git", "diff-tree", "--no-commit-id", "--name-only", "-r", "HEAD"
    ).splitlines()
    assert MANIFEST_PATH not in committed
    assert "README.md" in committed
    assert (working / "README.md").read_text(encoding="utf-8") == templates["README.md"]
    staged = _run_git(working, "git", "diff", "--cached", "--name-only").splitlines()
    assert MANIFEST_PATH in staged
    assert '"hold": false' in (working / MANIFEST_PATH).read_text(encoding="utf-8")
    remote_manifest = _run_git(working, "git", "show", f"origin/master:{MANIFEST_PATH}")
    assert remote_manifest == published.strip()


def test_partial_bootstrap_recovers_missing_files(tmp_path: Path) -> None:
    templates = load_host_templates(ROOT)
    _bare, working = _init_deploy(
        tmp_path / "deploy",
        {MANIFEST_PATH: MANIFEST, "unrelated.txt": "keep me\n"},
    )
    host = FakeHost()
    host.files = {MANIFEST_PATH: MANIFEST}
    host.pages_status = 404
    host.workflows = []
    status = run_host_bootstrap(
        ROOT,
        yes=True,
        deploy_dir=working,
        run=host,
        sleep=lambda _seconds: None,
        stdout_write=lambda _text: None,
    )
    assert status == 0
    assert (working / "README.md").read_text(encoding="utf-8") == templates["README.md"]
    assert (working / ".github/workflows/publish.yml").is_file()
    assert (working / ".github/workflows/weekly.yml").is_file()
    assert (
        (working / ".github/workflows/weekly.yml").read_text(encoding="utf-8")
        == templates[".github/workflows/weekly.yml"]
    )
    assert (working / MANIFEST_PATH).read_text(encoding="utf-8") == MANIFEST
    assert (working / "unrelated.txt").read_text(encoding="utf-8") == "keep me\n"
    assert host.pages_status == 200


def test_preview_with_deploy_dir_reads_local_files_only(tmp_path: Path) -> None:
    templates = load_host_templates(ROOT)
    _bare, working = _init_deploy(
        tmp_path / "deploy",
        {
            "README.md": templates["README.md"],
            ".github/workflows/publish.yml": templates[".github/workflows/publish.yml"],
            ".github/workflows/weekly.yml": templates[".github/workflows/weekly.yml"],
            MANIFEST_PATH: MANIFEST,
        },
    )
    host = FakeHost()
    host.pages_status = 200
    host.pages = {"build_type": "workflow"}
    host.workflows = [
        {
            "name": "Publish documentation",
            "path": ".github/workflows/publish.yml",
            "state": "active",
        },
        {
            "name": "Check weekly documentation",
            "path": ".github/workflows/weekly.yml",
            "state": "active",
        },
    ]
    plan = build_bootstrap_plan(ROOT, deploy_dir=working, run=host)
    assert plan.create_repo is False
    assert all(not item.changed for item in plan.files)
    assert plan.manifest_present is True
    assert plan.configure_pages is False
    assert not any(argv[0] == "git" for argv, _ in host.calls)


def test_dispatch_watches_correlated_run_not_scheduled() -> None:
    host = FakeHost()
    host.workflows = [
        {
            "name": "Publish documentation",
            "path": ".github/workflows/publish.yml",
            "state": "disabled_inactivity",
        }
    ]
    host.runs = [
        {
            "databaseId": 1,
            "name": "Docs publish",
            "displayTitle": "Docs publish",
            "event": "schedule",
            "headBranch": "master",
            "createdAt": "2026-10-20T12:00:05Z",
            "status": "in_progress",
            "url": "https://example.test/runs/1",
        },
        {
            "databaseId": 2,
            "name": "Docs publish [abc123]",
            "displayTitle": "Docs publish [abc123]",
            "event": "workflow_dispatch",
            "headBranch": "master",
            "createdAt": "2026-10-20T12:00:04Z",
            "status": "in_progress",
            "url": "https://example.test/runs/2",
        },
        {
            "databaseId": 3,
            "name": "Docs publish [other]",
            "displayTitle": "Docs publish [other]",
            "event": "workflow_dispatch",
            "headBranch": "master",
            "createdAt": "2026-10-20T12:00:06Z",
            "status": "in_progress",
            "url": "https://example.test/runs/3",
        },
    ]
    chunks: list[str] = []
    status = run_publish(
        force=True,
        run=host,
        sleep=lambda _seconds: None,
        correlation="abc123",
        now=lambda: datetime(2026, 10, 20, 12, 0, 4, tzinfo=timezone.utc),
        stdout_write=chunks.append,
    )
    assert status == 0
    assert host.enabled == [CALLER_WORKFLOW]
    assert host.dispatched
    flags = " ".join(host.dispatched[0])
    assert "-f correlation=abc123" in flags
    assert "-f force=true" in flags
    watch = [argv for argv, _ in host.calls if argv[:3] == ["gh", "run", "watch"]]
    assert watch and watch[0][3] == "2"
    assert any("watching run 2" in chunk for chunk in chunks)


def test_dispatch_preserves_nonzero_run_status_and_api_failures() -> None:
    host = FakeHost()
    host.watch_code = 1
    host.runs = [
        {
            "databaseId": 9,
            "name": "Docs publish [fail1]",
            "displayTitle": "Docs publish [fail1]",
            "event": "workflow_dispatch",
            "headBranch": "master",
            "createdAt": "2026-10-20T12:00:00Z",
            "status": "completed",
            "conclusion": "failure",
            "url": "https://example.test/runs/9",
        }
    ]
    status = run_publish(
        drill=True,
        run=host,
        sleep=lambda _seconds: None,
        correlation="fail1",
        now=lambda: datetime(2026, 10, 20, 12, 0, 0, tzinfo=timezone.utc),
        stdout_write=lambda _text: None,
    )
    assert status == 1

    host.dispatch_error = "API rate limit (HTTP 403)"
    with pytest.raises(DocsError, match="cannot dispatch"):
        run_publish(
            run=host,
            sleep=lambda _seconds: None,
            correlation="x",
            now=lambda: datetime(2026, 10, 20, 12, 0, 0, tzinfo=timezone.utc),
            stdout_write=lambda _text: None,
        )


def test_select_dispatched_run_ignores_unrelated_newest() -> None:
    runs = [
        {
            "databaseId": 8,
            "name": "Docs publish",
            "event": "schedule",
            "headBranch": "master",
            "createdAt": "2026-10-20T12:00:10Z",
        },
        {
            "databaseId": 7,
            "name": "Docs publish [wanted]",
            "event": "workflow_dispatch",
            "headBranch": "master",
            "createdAt": "2026-10-20T12:00:01Z",
        },
    ]
    selected = select_dispatched_run(
        runs,
        correlation="wanted",
        ref="master",
        dispatched_at=datetime(2026, 10, 20, 12, 0, 0, tzinfo=timezone.utc),
    )
    assert selected is not None
    assert selected["databaseId"] == 7


def test_status_reports_missing_site_hold_and_inactivity() -> None:
    host = FakeHost()
    host.files = {MANIFEST_PATH: MANIFEST}
    host.workflows = [
        {
            "name": "Publish documentation",
            "path": ".github/workflows/publish.yml",
            "state": "disabled_inactivity",
        }
    ]
    host.runs = [
        {
            "databaseId": 4,
            "name": "Docs publish",
            "event": "schedule",
            "headBranch": "master",
            "createdAt": "2026-10-19T17:00:00Z",
            "status": "completed",
            "conclusion": "success",
            "url": "https://example.test/runs/4",
        }
    ]
    host.issues = [
        {
            "number": 12,
            "title": "Docs publish blocked",
            "url": "https://example.test/issues/12",
        }
    ]

    def fetch(url: str, *, timeout: int) -> FetchedPage:
        assert url.endswith("/build.json")
        return FetchedPage(404, "", url)

    chunks: list[str] = []
    status = run_status(run=host, fetch=fetch, stdout_write=chunks.append)
    assert status == 0
    text = "".join(chunks)
    assert "hold: true" in text
    assert "disabled_inactivity" in text
    assert "live /build.json: missing" in text
    assert "not assumed to be served as /published.json" in text
    assert "#12 Docs publish blocked" in text
    report = collect_status(host, fetch=fetch)
    assert report["hold"] is True
    assert report["manifest_state"] == "ok"
    assert report["live_state"] == "missing"


def test_status_authentication_failure_is_an_error() -> None:
    host = FakeHost()
    host.repo_status = 401
    with pytest.raises(DocsError, match="authentication failure"):
        collect_status(host)


def test_status_missing_manifest_is_clear() -> None:
    host = FakeHost()
    host.files = {}
    host.workflows = []

    def fetch(url: str, *, timeout: int) -> FetchedPage:
        return FetchedPage(404, "", url)

    report = collect_status(host, fetch=fetch)
    text = format_status(report)
    assert report["manifest_state"] == "missing"
    assert "has not recorded a publication yet" in text
    assert "open managed issues: none" in text


def test_default_host_run_rejects_unknown_binaries() -> None:
    with pytest.raises(DocsError, match="could not start"):
        default_host_run(["/nonexistent-bob-docs-host-binary"], None)


def test_host_templates_include_both_callers() -> None:
    templates = load_host_templates(ROOT)
    assert set(templates) == {
        "README.md",
        ".github/workflows/publish.yml",
        ".github/workflows/weekly.yml",
    }
    assert (ROOT / "tools/docs/host/weekly.yml").is_file()
    assert "41 6 * * 1" in templates[".github/workflows/weekly.yml"]


def test_status_lists_publish_and_weekly_issues() -> None:
    host = FakeHost()
    host.files = {MANIFEST_PATH: MANIFEST}
    host.workflows = []
    host.issues = [
        {"number": 12, "title": "Docs publish blocked", "url": "https://example.test/issues/12"},
        {"number": 13, "title": "Docs weekly checks blocked", "url": "https://example.test/issues/13"},
        {"number": 99, "title": "Unrelated", "url": "https://example.test/issues/99"},
    ]

    def fetch(url: str, *, timeout: int) -> FetchedPage:
        return FetchedPage(404, "", url)

    report = collect_status(host, fetch=fetch)
    numbers = [item["number"] for item in report["open_issues"]]
    assert numbers == [12, 13]
    text = format_status(report)
    assert "#12 Docs publish blocked" in text
    assert "#13 Docs weekly checks blocked" in text
    assert list_open_weekly_issues(host) == [
        {"number": 13, "title": "Docs weekly checks blocked", "url": "https://example.test/issues/13"}
    ]


def test_bootstrap_enables_both_workflows(tmp_path: Path) -> None:
    templates = load_host_templates(ROOT)
    _bare, working = _init_deploy(
        tmp_path / "deploy",
        {MANIFEST_PATH: MANIFEST},
    )
    host = FakeHost()
    host.files = {MANIFEST_PATH: MANIFEST}
    host.pages_status = 404
    host.workflows = []
    status = run_host_bootstrap(
        ROOT,
        yes=True,
        deploy_dir=working,
        run=host,
        sleep=lambda _seconds: None,
        stdout_write=lambda _text: None,
    )
    assert status == 0
    assert CALLER_WORKFLOW in host.enabled
    assert WEEKLY_CALLER_WORKFLOW in host.enabled
    assert (working / WEEKLY_WORKFLOW_PATH).read_text(encoding="utf-8") == templates[WEEKLY_WORKFLOW_PATH]
