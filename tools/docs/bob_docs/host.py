"""Operator controls for the public documentation host repository.

The deploy repository ``bobs-org/bobs-org.github.io`` is a thin publisher:
caller workflows, a README, and the durable ``published.json`` log. This
module bootstraps that host, dispatches the publish workflow, and reports
live status. Preview is the default for bootstrap; ``-y/--yes`` applies.
"""

from __future__ import annotations

import base64
import json
import re
import secrets
import subprocess
import time
import urllib.error
from dataclasses import dataclass, field
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any, Callable
from urllib.parse import quote

from .errors import DocsError
from .publish_issue import (
    PUBLISH_BLOCKED_LABEL,
    PUBLISH_BLOCKED_TITLE,
    CommandResult,
    RunFn,
)
from .publish_verify import FetchedPage, fetch_text

HOST_OWNER = "bobs-org"
HOST_NAME = "bobs-org.github.io"
HOST_REPO = f"{HOST_OWNER}/{HOST_NAME}"
HOST_SSH = f"git@github.com:{HOST_REPO}.git"
SITE_ORIGIN = "https://bobs-org.github.io"
HOST_HOMEPAGE = SITE_ORIGIN + "/"
HOST_DESCRIPTION = (
    "Publishes the Bob documentation site. Source lives in bobs-org/bob-cli."
)
CALLER_WORKFLOW = "publish.yml"
REUSABLE_WORKFLOW = "bobs-org/bob-cli/.github/workflows/docs-publish.yml@master"
PUBLISH_WORKFLOW_PATH = ".github/workflows/publish.yml"
MANIFEST_PATH = "published.json"
DEFAULT_BRANCH_FALLBACK = "master"
ENABLE_ATTEMPTS = 8
DISCOVER_ATTEMPTS = 20
POLL_SECONDS = 2.0
WATCH_TIMEOUT_SECONDS = 3600
LIVE_TIMEOUT_SECONDS = 15

# (path in the deploy repo, filename under tools/docs/host/)
MAINTAINED_FILES: tuple[tuple[str, str], ...] = (
    ("README.md", "README.md"),
    (PUBLISH_WORKFLOW_PATH, CALLER_WORKFLOW),
)

_HTTP_RE = re.compile(r"\(HTTP (\d+)\)")
_SHA1 = re.compile(r"^[0-9a-f]{40}$")


def default_host_run(argv: list[str], stdin_text: str | None = None) -> CommandResult:
    """Run a host command without a shell; JSON bodies travel on stdin."""

    timeout: int | None = 120
    if argv[:3] == ["gh", "run", "watch"]:
        timeout = WATCH_TIMEOUT_SECONDS
    try:
        completed = subprocess.run(
            argv,
            input=stdin_text,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
            timeout=timeout,
        )
    except OSError as error:
        raise DocsError(f"could not start {' '.join(argv[:2])}: {error}") from error
    except subprocess.TimeoutExpired as error:
        raise DocsError(f"{' '.join(argv[:2])} timed out") from error
    return CommandResult(completed.returncode, completed.stdout, completed.stderr)


def host_template_dir(root: Path) -> Path:
    """Return the directory of files synced into the deploy repository."""

    return Path(root).resolve() / "tools" / "docs" / "host"


def load_host_templates(root: Path) -> dict[str, str]:
    """Load maintained host files keyed by their deploy-repository paths."""

    directory = host_template_dir(root)
    files: dict[str, str] = {}
    for destination, source_name in MAINTAINED_FILES:
        path = directory / source_name
        if not path.is_file():
            raise DocsError(f"host template missing: {path}")
        files[destination] = path.read_text(encoding="utf-8")
    return files


def http_status(result: CommandResult) -> int | None:
    """Extract an HTTP status from ``gh api`` output, if present."""

    if result.returncode == 0:
        return 200
    match = _HTTP_RE.search(result.stderr) or _HTTP_RE.search(result.stdout)
    return int(match.group(1)) if match else None


def _failure_text(result: CommandResult) -> str:
    return (result.stderr or result.stdout).strip() or "transport error"


def gh_api(
    run: RunFn,
    method: str,
    path: str,
    *,
    body: dict[str, Any] | None = None,
) -> tuple[int, str]:
    """Call ``gh api`` and return ``(status, body)``.

    A missing HTTP status on a failed command is a transport error, never a
    404 that would authorize creating the host repository.
    """

    argv = ["gh", "api", "--method", method, path]
    stdin = None
    if body is not None:
        argv.extend(["--input", "-"])
        stdin = json.dumps(body)
    result = run(argv, stdin)
    status = http_status(result)
    if result.returncode == 0:
        return status or 200, result.stdout
    if status is None:
        raise DocsError(f"GitHub request failed: {_failure_text(result)}")
    return status, result.stdout or result.stderr


def require_api_ok(status: int, body: str, *, action: str) -> str:
    """Return response text for 2xx; raise a classified error otherwise."""

    if 200 <= status < 300:
        return body
    if status in {401, 403}:
        kind = "authentication failure" if status == 401 else "permission denied"
        raise DocsError(f"{action}: {kind} (HTTP {status})")
    detail = body.strip() or f"HTTP {status}"
    raise DocsError(f"{action}: {detail}")


def _parse_json(text: str, *, what: str) -> Any:
    try:
        return json.loads(text or "null")
    except ValueError as error:
        raise DocsError(f"cannot parse {what}: {error}") from error


def contents_path(path: str) -> str:
    encoded = quote(path, safe="/")
    return f"repos/{HOST_REPO}/contents/{encoded}"


def decode_contents(payload: Any) -> str:
    """Decode a GitHub contents API payload to UTF-8 text."""

    if not isinstance(payload, dict):
        raise DocsError("GitHub contents response is not an object")
    content = payload.get("content")
    if not isinstance(content, str):
        raise DocsError("GitHub contents response has no file content")
    try:
        return base64.b64decode(content, validate=False).decode("utf-8")
    except (ValueError, UnicodeDecodeError) as error:
        raise DocsError(f"cannot decode GitHub file contents: {error}") from error


def new_correlation(now: Callable[[], float] | None = None) -> str:
    stamp = int((now or time.time)())
    return f"{stamp}-{secrets.token_hex(3)}"


# ---------------------------------------------------------------------------
# Bootstrap
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class FileSync:
    path: str
    content: str
    changed: bool
    current: str | None


@dataclass
class BootstrapPlan:
    repo_exists: bool
    default_branch: str
    create_repo: bool
    use_temp_clone: bool
    deploy_dir: Path | None
    files: list[FileSync]
    manifest_present: bool
    configure_pages: bool
    pages_build_type: str | None
    enable_actions: bool
    actions_enabled: bool | None
    actions_note: str
    enable_workflow: bool
    workflow_state: str | None
    operations: list[str] = field(default_factory=list)


def inspect_repo(run: RunFn) -> tuple[int, dict[str, Any] | None]:
    """Return ``(status, repo_json)`` for the host; 404 means absent."""

    status, body = gh_api(run, "GET", f"repos/{HOST_REPO}")
    if status == 404:
        return 404, None
    if status in {401, 403}:
        kind = "authentication failure" if status == 401 else "permission denied"
        raise DocsError(
            f"cannot inspect {HOST_REPO}: {kind} (HTTP {status}); "
            "this is not permission to create the repository"
        )
    if status != 200:
        raise DocsError(
            f"cannot inspect {HOST_REPO} (HTTP {status}): {body.strip() or 'unexpected response'}"
        )
    payload = _parse_json(body, what=f"{HOST_REPO} metadata")
    if not isinstance(payload, dict):
        raise DocsError(f"cannot inspect {HOST_REPO}: expected an object")
    return 200, payload


def _read_remote_file(run: RunFn, path: str) -> str | None:
    status, body = gh_api(run, "GET", contents_path(path))
    if status == 404:
        return None
    text = require_api_ok(status, body, action=f"read {path} from {HOST_REPO}")
    return decode_contents(_parse_json(text, what=path))


def _read_local_file(deploy_dir: Path, path: str) -> str | None:
    target = deploy_dir / path
    if not target.is_file():
        return None
    return target.read_text(encoding="utf-8")


def inspect_pages(run: RunFn) -> tuple[str | None, bool]:
    """Return ``(build_type, needs_configure)``."""

    status, body = gh_api(run, "GET", f"repos/{HOST_REPO}/pages")
    if status == 404:
        return None, True
    text = require_api_ok(status, body, action=f"inspect Pages on {HOST_REPO}")
    payload = _parse_json(text, what="Pages configuration")
    build_type = ""
    if isinstance(payload, dict):
        raw = payload.get("build_type")
        build_type = raw if isinstance(raw, str) else ""
    return build_type or "legacy", build_type != "workflow"


def inspect_actions(run: RunFn) -> tuple[bool | None, bool, str]:
    """Return ``(enabled, needs_enable, note)`` for repository Actions."""

    status, body = gh_api(run, "GET", f"repos/{HOST_REPO}/actions/permissions")
    if status in {401, 403}:
        return None, False, f"could not inspect Actions permissions (HTTP {status})"
    if status == 404:
        return None, True, "Actions permissions were not found; will enable if possible"
    text = require_api_ok(status, body, action=f"inspect Actions permissions on {HOST_REPO}")
    payload = _parse_json(text, what="Actions permissions")
    enabled = True
    if isinstance(payload, dict):
        enabled = bool(payload.get("enabled", True))
    if enabled:
        return True, False, "Actions are enabled"
    return False, True, "Actions are disabled on the repository"


def inspect_workflow_state(run: RunFn) -> str | None:
    status, body = gh_api(run, "GET", f"repos/{HOST_REPO}/actions/workflows")
    if status in {401, 403}:
        raise DocsError(
            f"cannot list workflows on {HOST_REPO}: "
            f"{'authentication failure' if status == 401 else 'permission denied'} (HTTP {status})"
        )
    if status == 404:
        return None
    text = require_api_ok(status, body, action=f"list workflows on {HOST_REPO}")
    payload = _parse_json(text, what="workflow list")
    workflows = payload.get("workflows") if isinstance(payload, dict) else None
    if not isinstance(workflows, list):
        return None
    for item in workflows:
        if not isinstance(item, dict):
            continue
        path = str(item.get("path") or "")
        if path.endswith(f"/{CALLER_WORKFLOW}") or path == CALLER_WORKFLOW:
            state = item.get("state")
            return state if isinstance(state, str) else "unknown"
    return None


def _file_syncs(
    templates: dict[str, str],
    current: dict[str, str | None],
) -> list[FileSync]:
    return [
        FileSync(
            path=path,
            content=templates[path],
            changed=current.get(path) != templates[path],
            current=current.get(path),
        )
        for path in templates
    ]


def build_bootstrap_plan(
    root: Path,
    *,
    deploy_dir: Path | None = None,
    run: RunFn = default_host_run,
) -> BootstrapPlan:
    """Inspect the host and describe the create/sync/Pages/enable operations."""

    templates = load_host_templates(root)
    status, repo = inspect_repo(run)
    repo_exists = status == 200
    default_branch = DEFAULT_BRANCH_FALLBACK
    if isinstance(repo, dict):
        branch = repo.get("default_branch")
        if isinstance(branch, str) and branch.strip():
            default_branch = branch.strip()

    current: dict[str, str | None] = {path: None for path in templates}
    manifest_present = False
    pages_build_type: str | None = None
    configure_pages = True
    actions_enabled: bool | None = None
    enable_actions = False
    actions_note = "repository does not exist yet"
    workflow_state: str | None = None

    if repo_exists:
        if deploy_dir is not None:
            checkout = Path(deploy_dir)
            if not checkout.is_dir():
                raise DocsError(f"--deploy-dir is not a directory: {checkout}")
            git_dir = checkout / ".git"
            if not git_dir.exists():
                raise DocsError(f"--deploy-dir is not a git checkout: {checkout}")
            current = {path: _read_local_file(checkout, path) for path in templates}
            manifest_path = checkout / MANIFEST_PATH
            manifest_present = manifest_path.is_file()
        else:
            current = {path: _read_remote_file(run, path) for path in templates}
            manifest_present = _read_remote_file(run, MANIFEST_PATH) is not None
        pages_build_type, configure_pages = inspect_pages(run)
        actions_enabled, enable_actions, actions_note = inspect_actions(run)
        workflow_state = inspect_workflow_state(run)
    else:
        current = {path: None for path in templates}
        configure_pages = True
        enable_actions = True
        actions_note = "Actions will be enabled on the new repository"

    files = _file_syncs(templates, current)
    operations: list[str] = []
    if not repo_exists:
        operations.append(
            f"create public repository {HOST_REPO} "
            f"(description={HOST_DESCRIPTION!r}, homepage={HOST_HOMEPAGE})"
        )
    else:
        operations.append(f"repository {HOST_REPO} already exists (default branch {default_branch})")

    use_temp_clone = deploy_dir is None
    if use_temp_clone:
        operations.append(
            f"clone {HOST_SSH} into a temporary directory and commit only maintained host files"
        )
    else:
        operations.append(f"sync maintained host files into {deploy_dir}")

    changed = [item.path for item in files if item.changed]
    if changed:
        operations.append("write " + ", ".join(changed))
        operations.append("commit and push maintained host files")
    else:
        operations.append("maintained host files already match the templates; no file commit")

    if manifest_present:
        operations.append(f"preserve existing {MANIFEST_PATH} byte-for-byte")
    else:
        operations.append(f"leave {MANIFEST_PATH} absent (first publish records it)")

    if configure_pages:
        current_pages = pages_build_type or "unconfigured"
        operations.append(
            f"configure GitHub Pages with build_type=workflow (currently {current_pages})"
        )
    else:
        operations.append("GitHub Pages already uses build_type=workflow")

    if enable_actions:
        operations.append(f"enable GitHub Actions ({actions_note})")
    else:
        operations.append(actions_note if repo_exists else "enable GitHub Actions after the repository exists")

    if workflow_state == "active":
        operations.append(f"ensure workflow {CALLER_WORKFLOW} stays enabled (active)")
        enable_workflow = True
    elif workflow_state:
        operations.append(
            f"enable workflow {CALLER_WORKFLOW} (currently {workflow_state})"
        )
        enable_workflow = True
    else:
        operations.append(f"enable workflow {CALLER_WORKFLOW} after it is installed")
        enable_workflow = True

    return BootstrapPlan(
        repo_exists=repo_exists,
        default_branch=default_branch,
        create_repo=not repo_exists,
        use_temp_clone=use_temp_clone,
        deploy_dir=Path(deploy_dir) if deploy_dir is not None else None,
        files=files,
        manifest_present=manifest_present,
        configure_pages=configure_pages,
        pages_build_type=pages_build_type,
        enable_actions=enable_actions,
        actions_enabled=actions_enabled,
        actions_note=actions_note,
        enable_workflow=enable_workflow,
        workflow_state=workflow_state,
        operations=operations,
    )


def format_bootstrap_plan(plan: BootstrapPlan, *, apply: bool) -> str:
    mode = "apply" if apply else "preview (pass -y/--yes to apply)"
    lines = [f"host-bootstrap: {mode}"]
    lines.extend(f"- {item}" for item in plan.operations)
    return "\n".join(lines)


def _require_git(run: RunFn, argv: list[str], *, action: str) -> CommandResult:
    result = run(argv, None)
    if result.returncode != 0:
        raise DocsError(f"{action}: {_failure_text(result)}")
    return result


def _git(run: RunFn, repo: Path, *args: str, action: str) -> CommandResult:
    return _require_git(run, ["git", "-C", str(repo), *args], action=action)


def _write_maintained_files(repo: Path, files: list[FileSync]) -> list[str]:
    written: list[str] = []
    for item in files:
        if not item.changed:
            continue
        destination = repo / item.path
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(item.content, encoding="utf-8")
        written.append(item.path)
    return written


def _commit_host_files(run: RunFn, repo: Path, paths: list[str], branch: str) -> bool:
    """Stage only maintained files and commit if the index changed."""

    if not paths:
        return False
    _git(run, repo, "add", "--", *paths, action="stage host files")
    cached = run(["git", "-C", str(repo), "diff", "--cached", "--quiet"], None)
    if cached.returncode == 0:
        return False
    _git(
        run,
        repo,
        "-c",
        "user.name=bob-docs",
        "-c",
        "user.email=bob-docs@users.noreply.github.com",
        "commit",
        "-m",
        "Sync documentation host templates",
        action="commit host files",
    )
    has_commit = run(["git", "-C", str(repo), "rev-parse", "HEAD"], None)
    if has_commit.returncode != 0:
        raise DocsError("host commit did not create HEAD")
    _git(run, repo, "push", "-u", "origin", f"HEAD:{branch}", action="push host files")
    return True


def _prepare_checkout(run: RunFn, repo: Path, branch: str) -> None:
    head = run(["git", "-C", str(repo), "rev-parse", "HEAD"], None)
    if head.returncode != 0:
        _git(run, repo, "checkout", "-B", branch, action=f"create branch {branch}")
        return
    switched = run(["git", "-C", str(repo), "checkout", branch], None)
    if switched.returncode != 0:
        _git(run, repo, "checkout", "-B", branch, action=f"create branch {branch}")


def _create_repo(run: RunFn) -> str:
    result = run(
        [
            "gh",
            "repo",
            "create",
            HOST_REPO,
            "--public",
            "--description",
            HOST_DESCRIPTION,
            "--homepage",
            HOST_HOMEPAGE,
        ],
        None,
    )
    if result.returncode != 0:
        status = http_status(result)
        if status in {401, 403}:
            kind = "authentication failure" if status == 401 else "permission denied"
            raise DocsError(f"cannot create {HOST_REPO}: {kind} (HTTP {status})")
        raise DocsError(f"cannot create {HOST_REPO}: {_failure_text(result)}")
    status, repo = inspect_repo(run)
    if status != 200 or repo is None:
        return DEFAULT_BRANCH_FALLBACK
    branch = repo.get("default_branch")
    if isinstance(branch, str) and branch.strip():
        return branch.strip()
    return DEFAULT_BRANCH_FALLBACK


def _configure_pages(run: RunFn, *, exists: bool) -> None:
    body = {"build_type": "workflow"}
    method = "PUT" if exists else "POST"
    status, text = gh_api(run, method, f"repos/{HOST_REPO}/pages", body=body)
    if status in {200, 201, 204}:
        return
    if method == "POST" and status == 409:
        status, text = gh_api(run, "PUT", f"repos/{HOST_REPO}/pages", body=body)
        if 200 <= status < 300:
            return
    require_api_ok(status, text, action=f"configure Pages on {HOST_REPO}")


def _enable_actions(run: RunFn) -> None:
    status, text = gh_api(
        run,
        "PUT",
        f"repos/{HOST_REPO}/actions/permissions",
        body={"enabled": True, "allowed_actions": "all"},
    )
    require_api_ok(status, text, action=f"enable Actions on {HOST_REPO}")


def _enable_workflow(
    run: RunFn,
    *,
    sleep: Callable[[float], None],
    attempts: int = ENABLE_ATTEMPTS,
) -> None:
    argv = ["gh", "workflow", "enable", CALLER_WORKFLOW, "--repo", HOST_REPO]
    last = ""
    for attempt in range(attempts):
        result = run(argv, None)
        if result.returncode == 0:
            return
        last = _failure_text(result)
        if attempt + 1 < attempts:
            sleep(POLL_SECONDS)
    raise DocsError(f"cannot enable workflow {CALLER_WORKFLOW}: {last}")


def apply_bootstrap_plan(
    plan: BootstrapPlan,
    *,
    run: RunFn,
    sleep: Callable[[float], None] = time.sleep,
    clone_dir: Path | None = None,
) -> None:
    """Apply a previously inspected bootstrap plan."""

    default_branch = plan.default_branch
    if plan.create_repo:
        default_branch = _create_repo(run)

    deploy = plan.deploy_dir
    temp_clone: Path | None = None
    if deploy is None:
        if clone_dir is None:
            raise DocsError("standalone bootstrap needs a temporary clone directory")
        temp_clone = clone_dir
        temp_clone.mkdir(parents=True, exist_ok=True)
        clone = run(["git", "clone", HOST_SSH, str(temp_clone)], None)
        if clone.returncode != 0:
            raise DocsError(f"cannot clone {HOST_SSH}: {_failure_text(clone)}")
        deploy = temp_clone
    assert deploy is not None
    _prepare_checkout(run, deploy, default_branch)
    written = _write_maintained_files(deploy, plan.files)
    _commit_host_files(run, deploy, written, default_branch)

    if plan.configure_pages:
        _configure_pages(run, exists=plan.pages_build_type is not None)
    if plan.enable_actions:
        _enable_actions(run)
    if plan.enable_workflow:
        _enable_workflow(run, sleep=sleep)


def run_host_bootstrap(
    root: Path,
    *,
    yes: bool = False,
    deploy_dir: Path | str | None = None,
    run: RunFn = default_host_run,
    sleep: Callable[[float], None] = time.sleep,
    clone_dir: Path | None = None,
    stdout_write: Callable[[str], None] | None = None,
) -> int:
    """Preview (default) or apply host bootstrap. Returns a process exit code."""

    write = stdout_write or (lambda text: print(text, end="" if text.endswith("\n") else "\n"))
    checkout = Path(deploy_dir) if deploy_dir else None
    plan = build_bootstrap_plan(root, deploy_dir=checkout, run=run)
    write(format_bootstrap_plan(plan, apply=yes) + "\n")
    if not yes:
        return 0
    apply_dir = clone_dir
    if plan.use_temp_clone and apply_dir is None:
        import tempfile

        with tempfile.TemporaryDirectory(prefix="bob-docs-host-") as tmp:
            apply_bootstrap_plan(plan, run=run, sleep=sleep, clone_dir=Path(tmp) / "deploy")
        write("host-bootstrap: applied\n")
        return 0
    apply_bootstrap_plan(plan, run=run, sleep=sleep, clone_dir=apply_dir)
    write("host-bootstrap: applied\n")
    return 0


# ---------------------------------------------------------------------------
# Publish dispatch and watch
# ---------------------------------------------------------------------------


def _parse_iso(value: str) -> datetime | None:
    text = value.strip()
    if not text:
        return None
    if text.endswith("Z"):
        text = text[:-1] + "+00:00"
    try:
        parsed = datetime.fromisoformat(text)
    except ValueError:
        return None
    if parsed.tzinfo is None:
        return parsed.replace(tzinfo=timezone.utc)
    return parsed.astimezone(timezone.utc)


def enable_publish_workflow(
    run: RunFn,
    *,
    sleep: Callable[[float], None] = time.sleep,
) -> None:
    _enable_workflow(run, sleep=sleep)


def dispatch_publish(
    run: RunFn,
    *,
    force: bool,
    drill: bool,
    pinned: bool,
    correlation: str,
    ref: str,
) -> None:
    argv = [
        "gh",
        "workflow",
        "run",
        CALLER_WORKFLOW,
        "--repo",
        HOST_REPO,
        "--ref",
        ref,
        "-f",
        f"correlation={correlation}",
        "-f",
        f"drill={'true' if drill else 'false'}",
        "-f",
        f"force={'true' if force else 'false'}",
        "-f",
        f"pinned={'true' if pinned else 'false'}",
    ]
    result = run(argv, None)
    if result.returncode != 0:
        raise DocsError(f"cannot dispatch {CALLER_WORKFLOW}: {_failure_text(result)}")


def list_workflow_runs(run: RunFn, *, workflow: str, limit: int = 20) -> list[dict[str, Any]]:
    result = run(
        [
            "gh",
            "run",
            "list",
            "--repo",
            HOST_REPO,
            "--workflow",
            workflow,
            "--json",
            "databaseId,name,displayTitle,event,headBranch,createdAt,status,conclusion,url",
            "--limit",
            str(limit),
        ],
        None,
    )
    if result.returncode != 0:
        raise DocsError(f"cannot list {workflow} runs: {_failure_text(result)}")
    payload = _parse_json(result.stdout or "[]", what=f"{workflow} runs")
    if not isinstance(payload, list):
        raise DocsError(f"cannot list {workflow} runs: expected a list")
    return [item for item in payload if isinstance(item, dict)]


def select_dispatched_run(
    runs: list[dict[str, Any]],
    *,
    correlation: str,
    ref: str,
    dispatched_at: datetime,
) -> dict[str, Any] | None:
    """Pick the run that matches this dispatch, never an unrelated scheduled run."""

    token = f"[{correlation}]"
    window_start = dispatched_at - timedelta(seconds=15)
    matches: list[dict[str, Any]] = []
    for run in runs:
        event = str(run.get("event") or "")
        if event and event != "workflow_dispatch":
            continue
        branch = str(run.get("headBranch") or "")
        if branch and branch != ref:
            continue
        title = f"{run.get('name') or ''} {run.get('displayTitle') or ''}"
        if token not in title:
            continue
        created = _parse_iso(str(run.get("createdAt") or ""))
        if created is not None and created < window_start:
            continue
        matches.append(run)
    if not matches:
        return None
    matches.sort(key=lambda item: str(item.get("createdAt") or ""), reverse=True)
    return matches[0]


def discover_dispatched_run(
    run: RunFn,
    *,
    correlation: str,
    ref: str,
    dispatched_at: datetime,
    sleep: Callable[[float], None],
    attempts: int = DISCOVER_ATTEMPTS,
) -> dict[str, Any]:
    last_count = 0
    for attempt in range(attempts):
        runs = list_workflow_runs(run, workflow=CALLER_WORKFLOW)
        last_count = len(runs)
        selected = select_dispatched_run(
            runs, correlation=correlation, ref=ref, dispatched_at=dispatched_at
        )
        if selected is not None:
            return selected
        if attempt + 1 < attempts:
            sleep(POLL_SECONDS)
    raise DocsError(
        f"could not find dispatched {CALLER_WORKFLOW} run [{correlation}] "
        f"after {attempts} polls ({last_count} recent runs)"
    )


def watch_run(run: RunFn, run_id: int) -> int:
    result = run(
        ["gh", "run", "watch", str(run_id), "--repo", HOST_REPO, "--exit-status"],
        None,
    )
    if result.returncode == 0:
        return 0
    return result.returncode if result.returncode > 0 else 1


def host_default_branch(run: RunFn) -> str:
    status, repo = inspect_repo(run)
    if status == 404:
        raise DocsError(f"{HOST_REPO} does not exist; run bob-docs host-bootstrap first")
    if repo is None:
        return DEFAULT_BRANCH_FALLBACK
    branch = repo.get("default_branch")
    if isinstance(branch, str) and branch.strip():
        return branch.strip()
    return DEFAULT_BRANCH_FALLBACK


def run_publish(
    *,
    force: bool = False,
    drill: bool = False,
    pinned: bool = False,
    run: RunFn = default_host_run,
    sleep: Callable[[float], None] = time.sleep,
    correlation: str | None = None,
    now: Callable[[], datetime] | None = None,
    stdout_write: Callable[[str], None] | None = None,
) -> int:
    """Enable the caller workflow, dispatch it, and watch that run."""

    write = stdout_write or (lambda text: print(text, end="" if text.endswith("\n") else "\n"))
    ref = host_default_branch(run)
    token = correlation or new_correlation()
    dispatched_at = (now or (lambda: datetime.now(timezone.utc)))()
    write(f"publish: enabling {CALLER_WORKFLOW} on {HOST_REPO}\n")
    enable_publish_workflow(run, sleep=sleep)
    write(
        "publish: dispatching "
        f"force={str(force).lower()} drill={str(drill).lower()} "
        f"pinned={str(pinned).lower()} correlation={token}\n"
    )
    dispatch_publish(
        run, force=force, drill=drill, pinned=pinned, correlation=token, ref=ref
    )
    selected = discover_dispatched_run(
        run, correlation=token, ref=ref, dispatched_at=dispatched_at, sleep=sleep
    )
    run_id = selected.get("databaseId")
    if not isinstance(run_id, int):
        raise DocsError(f"dispatched run is missing an id: {selected!r}")
    url = str(selected.get("url") or "")
    write(f"publish: watching run {run_id}{f' {url}' if url else ''}\n")
    status = watch_run(run, run_id)
    if status:
        write(f"publish: run {run_id} failed (exit {status})\n")
    else:
        write(f"publish: run {run_id} succeeded\n")
    return status


# ---------------------------------------------------------------------------
# Status
# ---------------------------------------------------------------------------


def fetch_host_manifest(run: RunFn) -> tuple[str, dict[str, Any] | None, str]:
    """Return ``(state, manifest, detail)`` for deploy-repo ``published.json``."""

    status, body = gh_api(run, "GET", contents_path(MANIFEST_PATH))
    if status == 404:
        return "missing", None, f"{MANIFEST_PATH} is absent; the host has not recorded a publication yet"
    text = require_api_ok(status, body, action=f"read {MANIFEST_PATH} from {HOST_REPO}")
    decoded = decode_contents(_parse_json(text, what=MANIFEST_PATH))
    try:
        payload = json.loads(decoded)
    except ValueError as error:
        raise DocsError(f"{MANIFEST_PATH} in {HOST_REPO} is malformed: {error}") from error
    if not isinstance(payload, dict):
        raise DocsError(f"{MANIFEST_PATH} in {HOST_REPO} is malformed: expected an object")
    return "ok", payload, ""


def fetch_live_build(
    origin: str = SITE_ORIGIN,
    *,
    fetch: Callable[..., FetchedPage] = fetch_text,
) -> tuple[str, dict[str, Any] | None, str]:
    url = origin.rstrip("/") + "/build.json"
    try:
        page = fetch(url, timeout=LIVE_TIMEOUT_SECONDS)
    except DocsError as error:
        return "unreachable", None, str(error)
    except (urllib.error.URLError, TimeoutError, OSError) as error:
        return "unreachable", None, f"{url}: {error}"
    if page.status == 404:
        return "missing", None, "live site has not published /build.json yet"
    if page.status != 200:
        return "missing", None, f"live /build.json returned HTTP {page.status}"
    try:
        payload = json.loads(page.body)
    except ValueError as error:
        raise DocsError(f"live /build.json is malformed: {error}") from error
    if not isinstance(payload, dict):
        raise DocsError("live /build.json is malformed: expected an object")
    return "ok", payload, ""


def list_workflows(run: RunFn) -> list[dict[str, Any]]:
    status, body = gh_api(run, "GET", f"repos/{HOST_REPO}/actions/workflows")
    if status == 404:
        return []
    text = require_api_ok(status, body, action=f"list workflows on {HOST_REPO}")
    payload = _parse_json(text, what="workflow list")
    workflows = payload.get("workflows") if isinstance(payload, dict) else None
    if not isinstance(workflows, list):
        return []
    return [item for item in workflows if isinstance(item, dict)]


def list_open_managed_issues(run: RunFn) -> list[dict[str, Any]]:
    result = run(
        [
            "gh",
            "issue",
            "list",
            "--repo",
            HOST_REPO,
            "--state",
            "open",
            "--label",
            PUBLISH_BLOCKED_LABEL,
            "--json",
            "number,title,url",
            "--limit",
            "20",
        ],
        None,
    )
    if result.returncode != 0:
        raise DocsError(f"cannot list open managed issues: {_failure_text(result)}")
    payload = _parse_json(result.stdout or "[]", what="managed issues")
    if not isinstance(payload, list):
        raise DocsError("cannot list open managed issues: expected a list")
    return [
        item
        for item in payload
        if isinstance(item, dict) and item.get("title") == PUBLISH_BLOCKED_TITLE
    ]


def _summarize_sources(record: dict[str, Any] | None) -> str:
    if not record:
        return "-"
    sources = record.get("sources")
    if not isinstance(sources, dict) or not sources:
        return "-"
    parts: list[str] = []
    for name in sorted(sources):
        info = sources[name]
        sha = ""
        if isinstance(info, dict):
            raw = info.get("sha")
            sha = raw if isinstance(raw, str) else ""
        elif isinstance(info, str) and _SHA1.fullmatch(info):
            sha = info
        parts.append(f"{name}={sha[:12]}" if sha else name)
    return ", ".join(parts)


def collect_status(
    run: RunFn = default_host_run,
    *,
    origin: str = SITE_ORIGIN,
    fetch: Callable[..., FetchedPage] = fetch_text,
) -> dict[str, Any]:
    """Gather host, live, workflow, run, hold, and issue status."""

    inspect_repo(run)
    manifest_state, manifest, manifest_detail = fetch_host_manifest(run)
    live_state, live, live_detail = fetch_live_build(origin, fetch=fetch)
    workflows = list_workflows(run)
    publish_runs: list[dict[str, Any]] = []
    weekly_runs: list[dict[str, Any]] = []
    names = {str(item.get("path") or "") for item in workflows}
    try:
        publish_runs = list_workflow_runs(run, workflow=CALLER_WORKFLOW, limit=5)
    except DocsError:
        if any(path.endswith(CALLER_WORKFLOW) for path in names):
            raise
    if any(path.endswith("weekly.yml") for path in names):
        weekly_runs = list_workflow_runs(run, workflow="weekly.yml", limit=5)
    hold = None
    if isinstance(manifest, dict) and "hold" in manifest:
        hold = bool(manifest.get("hold"))
    return {
        "repository": HOST_REPO,
        "origin": origin,
        "manifest_state": manifest_state,
        "manifest": manifest,
        "manifest_detail": manifest_detail,
        "hold": hold,
        "live_state": live_state,
        "live_build": live,
        "live_detail": live_detail,
        "workflows": [
            {
                "name": item.get("name"),
                "path": item.get("path"),
                "state": item.get("state"),
            }
            for item in workflows
        ],
        "publish_runs": publish_runs,
        "weekly_runs": weekly_runs,
        "open_issues": list_open_managed_issues(run),
    }


def _format_run(run: dict[str, Any]) -> str:
    ident = run.get("databaseId") or run.get("url") or "?"
    event = run.get("event") or "?"
    status = run.get("status") or "?"
    conclusion = run.get("conclusion") or ""
    created = run.get("createdAt") or ""
    name = run.get("displayTitle") or run.get("name") or ""
    extra = f" {conclusion}" if conclusion else ""
    title = f" {name}" if name else ""
    return f"#{ident} {event} {status}{extra} {created}{title}".strip()


def format_status(report: dict[str, Any]) -> str:
    lines = [
        f"status: {report['repository']} → {report['origin']}",
    ]
    hold = report.get("hold")
    hold_text = {True: "true", False: "false", None: "unknown"}[hold if hold in {True, False} else None]
    lines.append(f"hold: {hold_text}")

    manifest_state = report.get("manifest_state")
    if manifest_state == "missing":
        lines.append(f"host manifest: missing ({report.get('manifest_detail')})")
    else:
        manifest = report.get("manifest") or {}
        fingerprint = manifest.get("fingerprint") if isinstance(manifest, dict) else None
        run_url = manifest.get("run_url") if isinstance(manifest, dict) else None
        lines.append(
            "host manifest: "
            f"fingerprint={fingerprint or '-'} sources={_summarize_sources(manifest if isinstance(manifest, dict) else None)}"
            f"{f' run={run_url}' if run_url else ''}"
        )
        lines.append(
            "note: published.json lives in the deploy repository; it is not assumed to be served as /published.json"
        )

    live_state = report.get("live_state")
    if live_state == "ok":
        live = report.get("live_build") or {}
        lines.append(f"live /build.json: sources={_summarize_sources(live if isinstance(live, dict) else None)}")
    else:
        lines.append(f"live /build.json: {live_state} ({report.get('live_detail')})")

    workflows = report.get("workflows") or []
    if not workflows:
        lines.append("workflows: none installed yet")
    else:
        lines.append("workflows:")
        for item in workflows:
            path = item.get("path") or item.get("name") or "?"
            state = item.get("state") or "unknown"
            lines.append(f"  {path}: {state}")

    publish_runs = report.get("publish_runs") or []
    lines.append("recent publish runs:")
    if not publish_runs:
        lines.append("  (none)")
    else:
        lines.extend(f"  {_format_run(item)}" for item in publish_runs)

    weekly_runs = report.get("weekly_runs") or []
    if weekly_runs or any(
        str(item.get("path") or "").endswith("weekly.yml") for item in workflows
    ):
        lines.append("recent weekly runs:")
        if not weekly_runs:
            lines.append("  (none)")
        else:
            lines.extend(f"  {_format_run(item)}" for item in weekly_runs)

    issues = report.get("open_issues") or []
    if not issues:
        lines.append("open managed issues: none")
    else:
        lines.append("open managed issues:")
        for issue in issues:
            lines.append(
                f"  #{issue.get('number')} {issue.get('title')} {issue.get('url') or ''}".rstrip()
            )
    return "\n".join(lines)


def run_status(
    *,
    run: RunFn = default_host_run,
    origin: str = SITE_ORIGIN,
    fetch: Callable[..., FetchedPage] = fetch_text,
    stdout_write: Callable[[str], None] | None = None,
) -> int:
    write = stdout_write or (lambda text: print(text, end="" if text.endswith("\n") else "\n"))
    report = collect_status(run, origin=origin, fetch=fetch)
    write(format_status(report) + "\n")
    return 0
