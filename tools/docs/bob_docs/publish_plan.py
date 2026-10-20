"""Select immutable documentation publication sources and produce a build plan."""

from __future__ import annotations

import hashlib
import json
import os
import re
import subprocess
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Iterable

from .errors import DocsError
from .mounts import Mount, load_mounts, origin_identity

SCHEMA_VERSION = 1
PLAN_FILENAME = "publish-plan.json"
SOURCES_FILENAME = "production-sources.json"
NEXT_PUBLICATION_FILENAME = "next-publication.json"

_SHA1 = re.compile(r"^[0-9a-f]{40}$")
_SHA256 = re.compile(r"^[0-9a-f]{64}$")
_REPO = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.-]*$")
_INPUT_KEYS = {"repo", "path", "kind", "object_id", "target"}


@dataclass(frozen=True)
class Source:
    """One repository identity resolved to a commit before its contents are read."""

    repo: str
    remote: str
    branch: str
    sha: str

    def as_dict(self) -> dict[str, str]:
        return {
            "remote": self.remote,
            "branch": self.branch,
            "sha": self.sha,
        }


@dataclass(frozen=True)
class PublishedState:
    hold: bool
    sources: dict[str, Source]
    inputs: list[dict[str, str]]
    fingerprint: str
    raw: dict[str, Any]


def _run(
    cwd: Path,
    *args: str,
    timeout: int = 120,
) -> subprocess.CompletedProcess[str]:
    try:
        return subprocess.run(
            list(args),
            cwd=cwd,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
            timeout=timeout,
            env={**os.environ, "LC_ALL": "C", "GIT_TERMINAL_PROMPT": "0"},
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        message = "command timed out" if isinstance(error, subprocess.TimeoutExpired) else str(error)
        return subprocess.CompletedProcess(list(args), 127, "", message)


def _git(root: Path, *args: str, timeout: int = 120) -> str:
    result = _run(root, "git", *args, timeout=timeout)
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip() or "no diagnostic"
        raise DocsError(f"git {' '.join(args)} failed in {root}: {detail.splitlines()[-1]}")
    return result.stdout.strip()


def _remote(root: Path) -> str:
    try:
        return _git(root, "config", "--get", "remote.origin.url")
    except DocsError as error:
        raise DocsError(f"publish-plan needs an origin remote on the tooling checkout: {error}") from error


def _valid_repo_name(repo: str) -> None:
    if repo in {".", ".."} or not _REPO.fullmatch(repo):
        raise DocsError(f"mount repo name {repo!r} is not a safe single directory name")


def _resolve_default(remote: str, *, cwd: Path) -> tuple[str, str]:
    result = _run(cwd, "git", "ls-remote", "--symref", remote, "HEAD", timeout=60)
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip() or "no diagnostic"
        raise DocsError(
            f"cannot resolve the default branch for bob-cli remote {remote!r}: "
            f"{detail.splitlines()[-1]}"
        )
    branch: str | None = None
    sha: str | None = None
    for line in result.stdout.splitlines():
        fields = line.split("\t", 1)
        if len(fields) != 2:
            continue
        value, ref = fields
        if ref == "HEAD" and _SHA1.fullmatch(value):
            sha = value
        elif ref == "HEAD" and value.startswith("ref: refs/heads/"):
            branch = value.removeprefix("ref: refs/heads/")
    if not branch or not sha:
        raise DocsError(
            f"bob-cli remote {remote!r} did not advertise both a default branch and its SHA"
        )
    check = _run(cwd, "git", "check-ref-format", f"refs/heads/{branch}")
    if check.returncode == 0:
        return branch, sha
    raise DocsError(f"bob-cli remote {remote!r} advertised an invalid default branch {branch!r}")


def _resolve_branch(remote: str, branch: str, *, cwd: Path) -> str:
    check = _run(cwd, "git", "check-ref-format", f"refs/heads/{branch}")
    if check.returncode != 0:
        raise DocsError(f"mount branch {branch!r} is not a valid Git branch")
    result = _run(
        cwd,
        "git",
        "ls-remote",
        "--heads",
        remote,
        f"refs/heads/{branch}",
        timeout=60,
    )
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip() or "no diagnostic"
        raise DocsError(
            f"cannot resolve branch {branch!r} for mount remote {remote!r}: "
            f"{detail.splitlines()[-1]}"
        )
    matches = [
        line.split("\t", 1)[0]
        for line in result.stdout.splitlines()
        if len(line.split("\t", 1)) == 2
        and line.split("\t", 1)[1] == f"refs/heads/{branch}"
    ]
    if len(matches) != 1 or not _SHA1.fullmatch(matches[0]):
        raise DocsError(f"branch {branch!r} is unavailable on mount remote {remote!r}")
    return matches[0]


def _dedupe_mounts(mounts: Iterable[Mount]) -> dict[str, tuple[str, str]]:
    repositories: dict[str, tuple[str, str]] = {}
    for mount in mounts:
        _valid_repo_name(mount.repo)
        if mount.repo == "bob-cli":
            raise DocsError("mount table cannot declare bob-cli as a mounted source")
        identity = (mount.remote, mount.branch)
        existing = repositories.get(mount.repo)
        if existing is None:
            repositories[mount.repo] = identity
            continue
        if (
            origin_identity(existing[0]) != origin_identity(mount.remote)
            or existing[1] != mount.branch
        ):
            raise DocsError(
                f"mount table has conflicting remote or branch declarations for {mount.repo!r}"
            )
    return repositories


def _inside(path: Path, parent: Path) -> bool:
    try:
        path.resolve().relative_to(parent.resolve())
        return True
    except ValueError:
        return False


def _assert_disjoint(output_dir: Path, work_dir: Path) -> None:
    if _inside(output_dir, work_dir) or _inside(work_dir, output_dir):
        raise DocsError("--output-dir and --work-dir must be separate directories")


def _checkout(
    *,
    repo: str,
    remote: str,
    sha: str,
    work_dir: Path,
    cwd: Path,
) -> Path:
    """Create or advance a dedicated clean checkout to exactly the observed SHA."""

    _valid_repo_name(repo)
    if not _SHA1.fullmatch(sha):
        raise DocsError(f"source {repo!r} has an invalid commit SHA")
    destination = work_dir / repo
    if destination.is_symlink():
        raise DocsError(f"source work path {destination} is a symlink")
    work_dir.mkdir(parents=True, exist_ok=True)
    if destination.exists():
        try:
            top = Path(_git(destination, "rev-parse", "--show-toplevel")).resolve()
        except DocsError as error:
            raise DocsError(f"refusing to reuse non-Git source work path {destination}: {error}") from error
        if top != destination.resolve():
            raise DocsError(f"source work path {destination} is inside another Git checkout")
        current_remote = _git(destination, "config", "--get", "remote.origin.url")
        if origin_identity(current_remote) != origin_identity(remote):
            raise DocsError(
                f"source work path {destination} has origin {current_remote!r}, "
                f"not selected remote {remote!r}"
            )
        status = _git(destination, "status", "--porcelain", "--untracked-files=all")
        if status:
            preview = "\n".join(status.splitlines()[:10])
            raise DocsError(f"source work path {destination} is dirty; refusing to reset it:\n{preview}")
    else:
        result = _run(
            cwd,
            "git",
            "clone",
            "--depth=1",
            "--no-checkout",
            "--no-tags",
            "--",
            remote,
            str(destination),
        )
        if result.returncode != 0:
            detail = result.stderr.strip() or result.stdout.strip() or "no diagnostic"
            raise DocsError(
                f"cannot create clean source checkout for {repo!r} from {remote!r}: "
                f"{detail.splitlines()[-1]}"
            )

    fetch = _run(destination, "git", "fetch", "--no-tags", "origin", sha)
    if fetch.returncode != 0:
        detail = fetch.stderr.strip() or fetch.stdout.strip() or "no diagnostic"
        raise DocsError(
            f"cannot fetch observed SHA {sha} for {repo!r} from {remote!r}: "
            f"{detail.splitlines()[-1]}"
        )
    checkout = _run(destination, "git", "checkout", "--detach", "--force", sha)
    if checkout.returncode != 0:
        detail = checkout.stderr.strip() or checkout.stdout.strip() or "no diagnostic"
        raise DocsError(f"cannot check out pinned source {repo!r} at {sha}: {detail.splitlines()[-1]}")
    actual = _git(destination, "rev-parse", "HEAD")
    if actual != sha:
        raise DocsError(f"source {repo!r} checked out {actual}, expected observed SHA {sha}")
    status = _git(destination, "status", "--porcelain", "--untracked-files=all")
    if status:
        raise DocsError(f"source checkout {repo!r} is not clean after checkout")
    return destination.resolve()


def _source_paths(
    *,
    tooling_root: Path,
    source_set: dict[str, Source],
    work_dir: Path,
) -> dict[str, Path]:
    paths: dict[str, Path] = {}
    for repo in sorted(source_set):
        source = source_set[repo]
        paths[repo] = _checkout(
            repo=repo,
            remote=source.remote,
            sha=source.sha,
            work_dir=work_dir,
            cwd=tooling_root,
        )
    return paths


def _git_object(repo_dir: Path, sha: str, path: str) -> tuple[str, str]:
    result = _run(repo_dir, "git", "rev-parse", "--verify", f"{sha}:{path}")
    if result.returncode != 0:
        raise DocsError(f"selected source {repo_dir.name!r} is missing required input {path!r}")
    object_id = result.stdout.strip()
    kind = _git(repo_dir, "cat-file", "-t", object_id)
    if kind not in {"tree", "blob"}:
        raise DocsError(f"Git input {path!r} resolved to unsupported object type {kind!r}")
    return kind, object_id


def _workflow_paths(cli_dir: Path, sha: str) -> list[str]:
    result = _run(cli_dir, "git", "ls-tree", "-r", "--name-only", "-z", sha, "--", ".github/workflows")
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip() or "no diagnostic"
        raise DocsError(f"cannot list publication workflow inputs: {detail.splitlines()[-1]}")
    paths = [item for item in result.stdout.split("\0") if item]
    return sorted(
        path
        for path in paths
        if path.startswith(".github/workflows/docs-") and path.endswith(".yml")
    )


def _input_record(
    *,
    repo: str,
    repo_dir: Path,
    sha: str,
    path: str,
    target: str | None = None,
) -> dict[str, str]:
    kind, object_id = _git_object(repo_dir, sha, path)
    if target is not None and kind != "tree":
        raise DocsError(f"mounted source {repo}:{path} is not a Git tree")
    record = {"repo": repo, "path": path, "kind": kind, "object_id": object_id}
    if target is not None:
        record["target"] = target
    return record


def _sort_inputs(inputs: Iterable[dict[str, str]]) -> list[dict[str, str]]:
    return sorted(
        (dict(item) for item in inputs),
        key=lambda item: (item["repo"], item["path"], item.get("target", ""), item["kind"]),
    )


def fingerprint_inputs(inputs: Iterable[dict[str, str]]) -> str:
    """Hash canonical input identities and Git object IDs; local paths never enter."""

    canonical = json.dumps(
        _sort_inputs(inputs),
        sort_keys=True,
        separators=(",", ":"),
        ensure_ascii=False,
    ).encode("utf-8")
    return hashlib.sha256(canonical).hexdigest()


def _collect_inputs(
    *,
    source_set: dict[str, Source],
    source_dirs: dict[str, Path],
    mounts: list[Mount],
) -> list[dict[str, str]]:
    cli = source_set["bob-cli"]
    cli_dir = source_dirs["bob-cli"]
    inputs = [
        _input_record(repo="bob-cli", repo_dir=cli_dir, sha=cli.sha, path=path)
        for path in ("docs", "mkdocs.yml", "tools/docs")
    ]
    inputs.extend(
        _input_record(
            repo="bob-cli",
            repo_dir=cli_dir,
            sha=cli.sha,
            path=path,
        )
        for path in _workflow_paths(cli_dir, cli.sha)
    )
    for mount in mounts:
        source = source_set[mount.repo]
        inputs.append(
            _input_record(
                repo=mount.repo,
                repo_dir=source_dirs[mount.repo],
                sha=source.sha,
                path=mount.source.as_posix(),
                target=mount.target.as_posix(),
            )
        )
    return _sort_inputs(inputs)


def _canonical_sources(sources: dict[str, Source]) -> dict[str, dict[str, str]]:
    return {name: sources[name].as_dict() for name in sorted(sources)}


def _load_published(path: Path) -> PublishedState | None:
    try:
        raw = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return None
    except (OSError, UnicodeError, ValueError) as error:
        raise DocsError(f"publication manifest {path} is unreadable or invalid JSON: {error}") from error
    if not isinstance(raw, dict):
        raise DocsError(f"publication manifest {path} must be a JSON object")
    expected_keys = {
        "schema_version",
        "hold",
        "sources",
        "inputs",
        "fingerprint",
        "tools",
        "built_at",
        "run_url",
    }
    if set(raw) != expected_keys:
        unknown = sorted(set(raw) - expected_keys)
        missing = sorted(expected_keys - set(raw))
        details = []
        if unknown:
            details.append(f"unknown keys: {', '.join(unknown)}")
        if missing:
            details.append(f"missing keys: {', '.join(missing)}")
        raise DocsError(f"publication manifest {path} has an invalid shape ({'; '.join(details)})")
    if type(raw.get("schema_version")) is not int or raw["schema_version"] != SCHEMA_VERSION:
        raise DocsError(
            f"publication manifest {path} has unsupported schema_version "
            f"{raw.get('schema_version')!r}; expected {SCHEMA_VERSION}"
        )
    if not isinstance(raw.get("hold"), bool):
        raise DocsError(f"publication manifest {path}: hold must be a boolean")
    raw_sources = raw.get("sources")
    if not isinstance(raw_sources, dict) or "bob-cli" not in raw_sources:
        raise DocsError(f"publication manifest {path}: sources must include bob-cli")
    sources: dict[str, Source] = {}
    for repo, value in raw_sources.items():
        if not isinstance(repo, str):
            raise DocsError(f"publication manifest {path}: source names must be strings")
        _valid_repo_name(repo)
        if not isinstance(value, dict) or set(value) != {"remote", "branch", "sha"}:
            raise DocsError(
                f"publication manifest {path}: sources.{repo} must contain remote, branch, and sha"
            )
        remote, branch, sha = value["remote"], value["branch"], value["sha"]
        if not isinstance(remote, str) or not remote.strip():
            raise DocsError(f"publication manifest {path}: sources.{repo}.remote is invalid")
        if not isinstance(branch, str) or not branch.strip():
            raise DocsError(f"publication manifest {path}: sources.{repo}.branch is invalid")
        if not isinstance(sha, str) or not _SHA1.fullmatch(sha):
            raise DocsError(f"publication manifest {path}: sources.{repo}.sha must be a full commit SHA")
        sources[repo] = Source(repo, remote, branch, sha)

    raw_inputs = raw.get("inputs")
    if not isinstance(raw_inputs, list):
        raise DocsError(f"publication manifest {path}: inputs must be a list")
    inputs: list[dict[str, str]] = []
    for position, entry in enumerate(raw_inputs, 1):
        where = f"publication manifest {path}: inputs[{position}]"
        if not isinstance(entry, dict) or set(entry) - _INPUT_KEYS:
            raise DocsError(f"{where} has an unsupported input shape")
        if not {"repo", "path", "kind", "object_id"} <= set(entry):
            raise DocsError(f"{where} is missing repo, path, kind, or object_id")
        if not all(isinstance(entry[key], str) and entry[key] for key in entry):
            raise DocsError(f"{where} values must be nonempty strings")
        if entry["repo"] not in sources:
            raise DocsError(f"{where} names a repository not present in sources")
        if entry["kind"] not in {"blob", "tree"}:
            raise DocsError(f"{where}.kind must be blob or tree")
        if not _SHA1.fullmatch(entry["object_id"]):
            raise DocsError(f"{where}.object_id must be a Git object ID")
        inputs.append(dict(entry))
    inputs = _sort_inputs(inputs)
    fingerprint = raw.get("fingerprint")
    if not isinstance(fingerprint, str) or not _SHA256.fullmatch(fingerprint):
        raise DocsError(f"publication manifest {path}: fingerprint must be a SHA-256 digest")
    if fingerprint != fingerprint_inputs(inputs):
        raise DocsError(f"publication manifest {path}: fingerprint does not match its inputs")
    if not isinstance(raw.get("tools"), dict) or not raw["tools"] or not all(
        isinstance(key, str) and key and isinstance(value, str) and value
        for key, value in raw["tools"].items()
    ):
        raise DocsError(f"publication manifest {path}: tools must map names to versions")
    if not isinstance(raw.get("built_at"), str) or not raw["built_at"]:
        raise DocsError(f"publication manifest {path}: built_at must be a timestamp")
    try:
        timestamp = datetime.fromisoformat(raw["built_at"].replace("Z", "+00:00"))
    except ValueError as error:
        raise DocsError(f"publication manifest {path}: built_at is not an ISO timestamp") from error
    if timestamp.utcoffset() is None:
        raise DocsError(f"publication manifest {path}: built_at must include a timezone")
    if not isinstance(raw.get("run_url"), str) or not raw["run_url"].strip():
        raise DocsError(f"publication manifest {path}: run_url must be a nonempty string")
    return PublishedState(raw["hold"], sources, inputs, fingerprint, raw)


def _validate_mount_table(mounts: list[Mount], sources: dict[str, Source]) -> None:
    declarations = _dedupe_mounts(mounts)
    expected = {"bob-cli", *declarations}
    if set(sources) != expected:
        missing = sorted(expected - set(sources))
        extra = sorted(set(sources) - expected)
        details = []
        if missing:
            details.append(f"missing {', '.join(missing)}")
        if extra:
            details.append(f"unexpected {', '.join(extra)}")
        raise DocsError("publication source set does not match the selected mount table: " + "; ".join(details))
    for repo, (remote, branch) in declarations.items():
        selected = sources[repo]
        if origin_identity(selected.remote) != origin_identity(remote) or selected.branch != branch:
            raise DocsError(
                f"publication source {repo!r} does not match the selected mount identity "
                f"({remote!r}, branch {branch!r})"
            )


def _resolve_latest_sources(
    *,
    root: Path,
    mounts: list[Mount],
    cli_remote: str,
    cli_branch: str,
    cli_sha: str,
) -> dict[str, Source]:
    sources = {"bob-cli": Source("bob-cli", cli_remote, cli_branch, cli_sha)}
    for repo, (remote, branch) in sorted(_dedupe_mounts(mounts).items()):
        sha = _resolve_branch(remote, branch, cwd=root)
        sources[repo] = Source(repo, remote, branch, sha)
    return sources


def _pinned_sources(state: PublishedState, mounts: list[Mount]) -> dict[str, Source]:
    _validate_mount_table(mounts, state.sources)
    return state.sources


def _write_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    payload = json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False) + "\n"
    temporary = path.with_name(f".{path.name}.tmp")
    temporary.write_text(payload, encoding="utf-8")
    temporary.replace(path)


def _output_values(plan: dict[str, Any]) -> dict[str, str]:
    return {
        "should_build": str(plan["should_build"]).lower(),
        "should_deploy": str(plan["should_deploy"]).lower(),
        "should_record": str(plan["should_record"]).lower(),
        "reason": plan["reason"],
        "fingerprint": plan["fingerprint"] or "",
        "sources_file": plan["sources_file"] or "",
        "next_publication_file": plan["next_publication_file"] or "",
    }


def _write_github_output(path: Path, values: dict[str, str]) -> None:
    lines: list[str] = []
    for key, value in values.items():
        if "\n" in value or "\r" in value:
            raise DocsError(f"GitHub output {key} contains a newline")
        lines.append(f"{key}={value}\n")
    try:
        with path.open("a", encoding="utf-8") as output:
            output.writelines(lines)
    except OSError as error:
        raise DocsError(f"cannot append GitHub outputs to {path}: {error}") from error


def _finish_plan(
    plan: dict[str, Any],
    *,
    output_dir: Path,
    github_output: Path | None,
) -> dict[str, Any]:
    plan_path = output_dir / PLAN_FILENAME
    plan["plan_file"] = str(plan_path.resolve())
    _write_json(plan_path, plan)
    if github_output is not None:
        _write_github_output(github_output, _output_values(plan))
    return plan


def run_publish_plan(
    root: Path,
    *,
    manifest: Path,
    output_dir: Path,
    work_dir: Path,
    force: bool = False,
    drill: bool = False,
    pinned: bool = False,
    recovery: bool = False,
    github_output: Path | None = None,
) -> dict[str, Any]:
    """Resolve and stage an immutable publication plan without writing the manifest."""

    root = root.resolve()
    manifest = (root / manifest if not manifest.is_absolute() else manifest).resolve()
    output_dir = (root / output_dir if not output_dir.is_absolute() else output_dir).resolve()
    work_dir = (root / work_dir if not work_dir.is_absolute() else work_dir).resolve()
    github_output = (
        (root / github_output if not github_output.is_absolute() else github_output).resolve()
        if github_output is not None
        else None
    )
    _assert_disjoint(output_dir, work_dir)
    if github_output is None and os.environ.get("GITHUB_OUTPUT"):
        configured = Path(os.environ["GITHUB_OUTPUT"])
        github_output = (root / configured if not configured.is_absolute() else configured).resolve()
    reserved_outputs = {
        (output_dir / PLAN_FILENAME).resolve(),
        (output_dir / SOURCES_FILENAME).resolve(),
        (output_dir / NEXT_PUBLICATION_FILENAME).resolve(),
    }
    if manifest in reserved_outputs:
        raise DocsError("--manifest must not point at a planner output file")
    output_dir.mkdir(parents=True, exist_ok=True)
    state = _load_published(manifest)
    flags = {"force": force, "drill": drill, "pinned": pinned, "recovery": recovery}
    cli_remote = _remote(root)
    if state is not None and origin_identity(state.sources["bob-cli"].remote) != origin_identity(cli_remote):
        raise DocsError(
            "publication manifest bob-cli remote does not match the tooling checkout origin"
        )

    def skip(reason: str, *, sources: dict[str, Source] | None = None, fingerprint: str = "") -> dict[str, Any]:
        return _finish_plan(
            {
                "schema_version": SCHEMA_VERSION,
                "should_build": False,
                "should_deploy": False,
                "should_record": False,
                "reason": reason,
                "flags": flags,
                "fingerprint": fingerprint,
                "sources": _canonical_sources(sources or {}),
                "manifest_file": str(manifest),
                "sources_file": None,
                "next_publication_file": None,
            },
            output_dir=output_dir,
            github_output=github_output,
        )

    # A hold is an operator decision. No unpinned flag can release it.
    if state is not None and state.hold and not pinned:
        return skip("held", sources=state.sources, fingerprint=state.fingerprint)
    if pinned and state is None:
        raise DocsError("--pinned requires an existing valid publication manifest")

    if pinned:
        assert state is not None
        cli_source = state.sources["bob-cli"]
        source_dirs = _source_paths(
            tooling_root=root,
            source_set={"bob-cli": cli_source},
            work_dir=work_dir,
        )
        mounts = load_mounts(source_dirs["bob-cli"])
        selected = _pinned_sources(state, mounts)
        source_dirs = _source_paths(
            tooling_root=root,
            source_set=selected,
            work_dir=work_dir,
        )
        inputs = _collect_inputs(source_set=selected, source_dirs=source_dirs, mounts=mounts)
        fingerprint = fingerprint_inputs(inputs)
        if fingerprint != state.fingerprint:
            raise DocsError("pinned source contents do not match the published fingerprint")
        reason = "drill-pinned" if drill else "pinned"
    else:
        cli_branch, cli_sha = _resolve_default(cli_remote, cwd=root)
        cli_dir = _checkout(
            repo="bob-cli",
            remote=cli_remote,
            sha=cli_sha,
            work_dir=work_dir,
            cwd=root,
        )
        mounts = load_mounts(cli_dir)
        selected = _resolve_latest_sources(
            root=root,
            mounts=mounts,
            cli_remote=cli_remote,
            cli_branch=cli_branch,
            cli_sha=cli_sha,
        )
        _validate_mount_table(mounts, selected)
        same_shas = (
            state is not None
            and set(state.sources) == set(selected)
            and all(state.sources[name].sha == source.sha for name, source in selected.items())
        )
        requested = force or drill or recovery
        if same_shas and not requested:
            return skip("same-sources", sources=selected, fingerprint=state.fingerprint)
        source_dirs = _source_paths(
            tooling_root=root,
            source_set=selected,
            work_dir=work_dir,
        )
        inputs = _collect_inputs(source_set=selected, source_dirs=source_dirs, mounts=mounts)
        fingerprint = fingerprint_inputs(inputs)
        if state is not None and fingerprint == state.fingerprint and not requested:
            return skip("same-inputs", sources=selected, fingerprint=fingerprint)
        if drill:
            reason = "drill"
        elif force:
            reason = "forced"
        elif recovery:
            reason = "recovery"
        elif state is None:
            reason = "first-publish"
        else:
            reason = "inputs-changed"

    sources_path = output_dir / SOURCES_FILENAME
    source_entries: dict[str, dict[str, str]] = {}
    for name, source in sorted(selected.items()):
        relative = os.path.relpath(source_dirs[name], start=sources_path.parent)
        source_entries[name] = {"path": Path(relative).as_posix(), "sha": source.sha}
    sources_manifest = {"sources": source_entries}
    next_publication = {
        "schema_version": SCHEMA_VERSION,
        "hold": state.hold if state is not None else False,
        "sources": _canonical_sources(selected),
        "inputs": inputs,
        "fingerprint": fingerprint,
    }
    next_path = output_dir / NEXT_PUBLICATION_FILENAME
    _write_json(sources_path, sources_manifest)
    _write_json(next_path, next_publication)
    should_deploy = not drill
    plan = {
        "schema_version": SCHEMA_VERSION,
        "should_build": True,
        "should_deploy": should_deploy,
        "should_record": should_deploy,
        "reason": reason,
        "flags": flags,
        "fingerprint": fingerprint,
        "sources": _canonical_sources(selected),
        "manifest_file": str(manifest),
        "sources_file": str(sources_path.resolve()),
        "next_publication_file": str(next_path.resolve()),
    }
    return _finish_plan(
        plan,
        output_dir=output_dir,
        github_output=github_output,
    )
