"""Sibling discovery, safe mount copying, nav expansion, and production sources."""

from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Any
from urllib.parse import unquote, urlparse

from .errors import DocsError
from .ignore import path_ignored
from .yaml_tools import load_yaml

MOUNT_KEYS = {"repo", "remote", "branch", "source", "target", "nav_title"}

NAV_ENTRY_KEYS = {"title", "path"}

SOURCE_KEY = "source"

NAV_FILENAME = "nav.yml"
MEDIA_FILENAME = "media.yml"
INDEX_FILENAME = "index.md"

MANIFEST_FILENAME = "mount-manifest.json"

_SHA = re.compile(r"^[0-9a-f]{40}$")

_GITHUB_HOSTS = {"github.com", "www.github.com"}

# Genuine transport failures. Unknown diagnostics fail closed.
_CONNECTIVITY_RE = re.compile(
    r"could not resolve host|"
    r"could not resolve hostname|"
    r"name or service not known|"
    r"temporary failure in name resolution|"
    r"nodename nor servname provided|"
    r"failed to resolve address|"
    r"failed to connect to|"
    r"could not connect to|"
    r"couldn't connect to server|"
    r"connection timed out|"
    r"operation timed out|"
    r"network is unreachable|"
    r"no route to host|"
    r"connection reset by peer|"
    r"connection refused|"
    r"server unexpectedly closed the connection|"
    r"ssh: connect to host",
    re.IGNORECASE,
)


@dataclass(frozen=True)
class Mount:
    repo: str
    remote: str
    branch: str
    source: PurePosixPath
    target: PurePosixPath
    nav_title: str


def env_var_for(repo: str) -> str:
    name = re.sub(r"[^0-9A-Za-z]+", "_", repo).strip("_").upper()
    if not name:
        raise DocsError(
            f"mount repo {repo!r} has no usable name for discovery env vars"
        )
    return f"BOB_DOCS_{name}_DIR"


def _mapping(
    value: Any, where: str, allowed: set[str], required: set[str]
) -> dict[str, Any]:
    if not isinstance(value, dict) or not all(isinstance(key, str) for key in value):
        raise DocsError(f"{where}: expected a mapping")
    unknown = set(value) - allowed
    missing = required - set(value)
    if unknown:
        raise DocsError(f"{where}: unknown keys: {', '.join(sorted(unknown))}")
    if missing:
        raise DocsError(f"{where}: missing keys: {', '.join(sorted(missing))}")
    return value


def _relative_posix(value: Any, where: str) -> PurePosixPath:
    if (
        not isinstance(value, str)
        or not value
        or value.startswith("/")
        or value.startswith("~")
    ):
        raise DocsError(f"{where}: must be a relative path beneath its parent")
    path = PurePosixPath(value)
    if not path.parts or any(part in ("", ".", "..") for part in path.parts):
        raise DocsError(
            f"{where}: must be a relative path without empty, '.', or '..' parts"
        )
    return path


def load_mounts(root: Path) -> list[Mount]:
    path = root / "docs/_meta/mounts.yml"
    raw = load_yaml(path)
    if raw is None:
        raw = []
    if not isinstance(raw, list):
        raise DocsError(f"{path}: expected an ordered list")
    mounts: list[Mount] = []
    seen_targets: list[PurePosixPath] = []
    for index, raw_mount in enumerate(raw, start=1):
        where = f"{path}: mounts[{index}]"
        mount = _mapping(raw_mount, where, MOUNT_KEYS, MOUNT_KEYS)
        for key in ("repo", "remote", "branch", "nav_title"):
            value = mount[key]
            if not isinstance(value, str) or not value.strip():
                raise DocsError(f"{where}: {key} must be a nonempty string")
        source = _relative_posix(mount["source"], f"{where}: source")
        target = _relative_posix(mount["target"], f"{where}: target")
        for other in seen_targets:
            if (
                target == other
                or target.is_relative_to(other)
                or other.is_relative_to(target)
            ):
                raise DocsError(
                    f"{where}: target {str(target)!r} overlaps another mount target"
                )
        seen_targets.append(target)
        mounts.append(
            Mount(
                repo=mount["repo"],
                remote=mount["remote"],
                branch=mount["branch"],
                source=source,
                target=target,
                nav_title=mount["nav_title"],
            )
        )
    return mounts


def _git_env() -> dict[str, str]:
    env = os.environ.copy()
    env["LC_ALL"] = "C"
    env["GIT_TERMINAL_PROMPT"] = "0"
    return env


def _run_git(cwd: Path, *args: str) -> subprocess.CompletedProcess[str]:
    try:
        return subprocess.run(
            ["git", *args],
            cwd=cwd,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
            env=_git_env(),
        )
    except OSError as error:
        return subprocess.CompletedProcess(
            ["git", *args],
            returncode=127,
            stdout="",
            stderr=str(error),
        )


def _git(cwd: Path, *args: str) -> str | None:
    result = _run_git(cwd, *args)
    if result.returncode != 0:
        return None
    return result.stdout.strip()


def _strip_dot_git(value: str) -> str:
    value = value.rstrip("/")
    if value.endswith(".git"):
        return value[:-4]
    return value


def _owner_repo(path: str) -> tuple[str, str] | None:
    cleaned = _strip_dot_git(unquote(path).strip("/"))
    parts = [part for part in cleaned.split("/") if part]
    if len(parts) != 2:
        return None
    return parts[0].lower(), parts[1].lower()


def _github_identity(url: str) -> tuple[str, str] | None:
    raw = url.strip()
    scp = re.match(r"^git@([^:]+):(.+)$", raw)
    if scp and scp.group(1).lower() in _GITHUB_HOSTS:
        return _owner_repo(scp.group(2))
    parsed = urlparse(raw)
    host = (parsed.hostname or "").lower()
    if host in _GITHUB_HOSTS:
        return _owner_repo(parsed.path)
    return None


def _local_identity(url: str) -> Path | None:
    raw = url.strip()
    if raw.startswith("file:"):
        parsed = urlparse(raw)
        if parsed.scheme != "file":
            return None
        host = (parsed.netloc or "").lower()
        if host not in {"", "localhost"}:
            return None
        path = unquote(parsed.path)
        if not path:
            return None
        return Path(path).resolve()
    if raw.startswith("/") or raw.startswith("./") or raw.startswith("../"):
        return Path(raw).resolve()
    if raw.startswith("~"):
        return Path(raw).expanduser().resolve()
    return None


def origin_identity(url: str) -> tuple[str, ...]:
    """Comparable repository identity for a Git remote URL."""

    github = _github_identity(url)
    if github is not None:
        return ("github", *github)
    local = _local_identity(url)
    if local is not None:
        return ("local", str(local))
    return ("other", _strip_dot_git(url.strip()))


def origins_match(left: str, right: str) -> bool:
    """Whether two remotes name the same repository under known equivalent forms."""

    if left == right:
        return True
    return origin_identity(left) == origin_identity(right)


def _is_connectivity_failure(stderr: str) -> bool:
    return bool(_CONNECTIVITY_RE.search(stderr))


def _worktree_failure(candidate: Path) -> str | None:
    """Return a reason if ``candidate`` is not a Git working tree rooted at itself."""

    resolved = candidate.resolve()
    inside = _run_git(candidate, "rev-parse", "--is-inside-work-tree")
    if inside.returncode != 0 or inside.stdout.strip() != "true":
        detail = inside.stderr.strip().splitlines()
        hint = f": {detail[-1]}" if detail else ""
        return f"not a Git working tree rooted at this path{hint}"
    toplevel = _run_git(candidate, "rev-parse", "--show-toplevel")
    if toplevel.returncode != 0:
        detail = toplevel.stderr.strip().splitlines()
        hint = f": {detail[-1]}" if detail else ""
        return f"could not resolve the working-tree root{hint}"
    root = Path(toplevel.stdout.strip()).resolve()
    if root != resolved:
        return f"Git toplevel is {root}, not this directory (enclosing repository is not this source)"
    head = _run_git(candidate, "rev-parse", "--verify", "HEAD")
    if head.returncode != 0:
        detail = head.stderr.strip().splitlines()
        hint = f": {detail[-1]}" if detail else ""
        return f"HEAD is not resolvable{hint}"
    return None


def _require_discovered_source(
    candidate: Path,
    mount: Mount,
    *,
    source: str,
    fallthrough: bool = True,
) -> None:
    """Reject an existing override, sibling, linked, or cache path that is not this repo."""

    suffix = "; it never falls through to other locations" if fallthrough else ""
    reason = _worktree_failure(candidate)
    if reason is not None:
        raise DocsError(
            f"mount repo {mount.repo!r}: {source} {candidate} is not a usable Git "
            f"working tree ({reason}){suffix}"
        )
    origin = _git(candidate, "config", "--get", "remote.origin.url")
    if origin is None:
        raise DocsError(
            f"mount repo {mount.repo!r}: {source} {candidate} has no origin; "
            f"expected {mount.remote}{suffix}"
        )
    if not origins_match(origin, mount.remote):
        raise DocsError(
            f"mount repo {mount.repo!r}: {source} {candidate} has origin {origin!r}, "
            f"not the mount remote {mount.remote!r}{suffix}"
        )


def repo_git_info(repo_dir: Path) -> dict[str, Any]:
    sha = _git(repo_dir, "rev-parse", "HEAD")
    if sha is None:
        return {"sha": None, "dirty": None, "branch": None}
    branch = _git(repo_dir, "branch", "--show-current") or None
    status = _git(repo_dir, "status", "--porcelain", "--untracked-files=all")
    return {"sha": sha, "dirty": bool(status), "branch": branch}


def cache_root() -> Path:
    base = os.environ.get("XDG_CACHE_HOME")
    if base:
        return Path(base) / "bob-docs"
    return Path.home() / ".cache" / "bob-docs"


def _refresh_cached_clone(mount: Mount, cached: Path) -> tuple[Path, str]:
    """Verify a cached clone's identity and refresh it to the remote branch head."""

    _require_discovered_source(cached, mount, source="cached clone", fallthrough=False)
    fetch = _run_git(
        cached,
        "fetch",
        "--depth",
        "1",
        "origin",
        f"+refs/heads/{mount.branch}:refs/remotes/origin/{mount.branch}",
    )
    if fetch.returncode != 0:
        diagnostic = fetch.stderr.strip() or fetch.stdout.strip()
        hint = f": {diagnostic.splitlines()[-1]}" if diagnostic else ""
        if _is_connectivity_failure(diagnostic):
            print(
                f"bob-docs: warning: offline, keeping cached {mount.repo!r} "
                f"without refreshing{hint}",
            )
            return cached.resolve(), str(cached)
        raise DocsError(
            f"mount repo {mount.repo!r}: could not refresh cached clone of "
            f"{mount.remote} branch {mount.branch}{hint}"
        )
    fetched = _run_git(cached, "rev-parse", "--verify", "FETCH_HEAD")
    if fetched.returncode != 0:
        detail = fetched.stderr.strip().splitlines()
        hint = f": {detail[-1]}" if detail else ""
        raise DocsError(
            f"mount repo {mount.repo!r}: fetched {mount.remote} "
            f"branch {mount.branch} but FETCH_HEAD is missing{hint}"
        )
    reset = _run_git(cached, "reset", "--hard", fetched.stdout.strip())
    if reset.returncode != 0:
        detail = reset.stderr.strip().splitlines()
        hint = f": {detail[-1]}" if detail else ""
        raise DocsError(
            f"mount repo {mount.repo!r}: fetched {mount.remote} "
            f"branch {mount.branch} but could not reset the cached clone{hint}"
        )
    return cached.resolve(), str(cached)


def discover_repo(
    mount: Mount,
    *,
    root: Path,
    env: dict[str, str] | None = None,
) -> tuple[Path, str]:
    """Resolve one mount's repo directory, reusing each unique repo across mounts."""

    environment = os.environ if env is None else env
    var = env_var_for(mount.repo)
    override = environment.get(var)
    attempted: list[str] = []
    if override is not None:
        candidate = Path(override)
        if not candidate.is_absolute():
            candidate = (root / candidate).resolve()
        if candidate.is_dir():
            _require_discovered_source(candidate, mount, source=f"${var}")
            return candidate, f"${var}"
        raise DocsError(
            f"mount repo {mount.repo!r}: ${var}={override} is not a usable directory; "
            "an explicit override never falls through to other locations"
        )
    attempted.append(f"${var} (unset)")

    sibling = (root.parent / mount.repo).resolve()
    attempted.append(str(sibling))
    if sibling.is_dir():
        _require_discovered_source(sibling, mount, source="sibling")
        return sibling, str(sibling)

    linked = (root / "sase/repos/linked" / mount.repo).resolve()
    attempted.append(str(linked))
    if linked.is_dir():
        _require_discovered_source(linked, mount, source="linked checkout")
        return linked, str(linked)

    cached = cache_root() / mount.repo
    attempted.append(
        f"{cached} (shallow clone of {mount.remote} branch {mount.branch})"
    )
    if cached.is_dir():
        return _refresh_cached_clone(mount, cached)
    try:
        cached.parent.mkdir(parents=True, exist_ok=True)
        result = subprocess.run(
            [
                "git",
                "clone",
                "--depth",
                "1",
                "--branch",
                mount.branch,
                mount.remote,
                str(cached),
            ],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
            env=_git_env(),
        )
    except OSError as error:
        raise DocsError(
            f"mount repo {mount.repo!r}: could not start git clone of {mount.remote}: {error}; "
            f"attempted: {'; '.join(attempted)}"
        ) from error
    if result.returncode != 0:
        detail = result.stderr.strip().splitlines()
        hint = f": {detail[-1]}" if detail else ""
        raise DocsError(
            f"mount repo {mount.repo!r}: shallow clone of {mount.remote} "
            f"branch {mount.branch} failed{hint}; attempted: {'; '.join(attempted)}"
        )
    return cached.resolve(), str(cached)


@dataclass(frozen=True)
class MountPlan:
    mount: Mount
    repo_dir: Path
    source_dir: Path
    files: tuple[tuple[PurePosixPath, PurePosixPath], ...]
    nav: tuple[dict[str, str], ...]


def _read_source_text(path: Path) -> str:
    try:
        return path.read_text(encoding="utf-8")
    except OSError as error:
        raise DocsError(f"cannot read mount source file {path}: {error}") from error


def _is_skipped(relative: PurePosixPath) -> bool:
    return (
        relative.name in (NAV_FILENAME, MEDIA_FILENAME)
        or relative.name.startswith(".")
        or any(part.startswith(".") for part in relative.parts)
    )


def _collect_candidates(
    mount: Mount, repo_dir: Path
) -> tuple[Path, list[tuple[Path, PurePosixPath]]]:
    lexical_source = repo_dir / Path(*mount.source.parts)
    if lexical_source.is_symlink():
        raise DocsError(
            f"mount {mount.repo!r} -> {str(mount.target)!r}: "
            f"source directory {str(mount.source)!r} is a symlink; "
            "mount sources must be real directories inside their repo"
        )
    source_dir = lexical_source.resolve()
    if not source_dir.is_dir():
        raise DocsError(
            f"mount {mount.repo!r} -> {str(mount.target)!r}: "
            f"source directory {str(mount.source)!r} does not exist in {repo_dir}"
        )
    if source_dir != repo_dir.resolve() and not source_dir.is_relative_to(
        repo_dir.resolve()
    ):
        raise DocsError(
            f"mount {mount.repo!r} -> {str(mount.target)!r}: "
            f"source {str(mount.source)!r} escapes its repo {repo_dir}"
        )
    found: list[tuple[Path, PurePosixPath]] = []
    for path in sorted(source_dir.rglob("*")):
        relative = PurePosixPath(path.relative_to(source_dir).as_posix())
        if _is_skipped(relative):
            continue
        if path.is_symlink():
            try:
                resolved = path.resolve(strict=True)
            except OSError as error:
                raise DocsError(
                    f"mount source has a broken symlink: {path}: {error}"
                ) from error
            if not resolved.is_relative_to(repo_dir.resolve()):
                raise DocsError(
                    f"mount source symlink escapes its repo: {path} -> {resolved}"
                )
            if resolved.is_dir():
                continue
        elif path.is_dir():
            continue
        found.append((path, relative))
    return source_dir, found


def _partition_nested(
    mounts: list[Mount], candidates: dict[int, list[tuple[Path, PurePosixPath]]]
) -> dict[int, list[tuple[Path, PurePosixPath]]]:
    """Keep each file only in the mount with the most specific same-repo source root."""

    by_repo: dict[str, list[int]] = {}
    for index, mount in enumerate(mounts):
        by_repo.setdefault(mount.repo, []).append(index)
    assigned: dict[int, list[tuple[Path, PurePosixPath]]] = {
        index: [] for index in range(len(mounts))
    }
    for indexes in by_repo.values():
        if len(indexes) == 1:
            assigned[indexes[0]] = candidates[indexes[0]]
            continue
        by_source: dict[PurePosixPath, list[tuple[Path, PurePosixPath]]] = {}
        for index in indexes:
            for path, relative in candidates[index]:
                absolute = PurePosixPath(
                    (Path(*mounts[index].source.parts, *relative.parts)).as_posix()
                )
                by_source.setdefault(absolute, []).append((path, relative))
        for absolute, options in by_source.items():
            best: int | None = None
            best_depth = -1
            for index in indexes:
                source = mounts[index].source
                if absolute == source or absolute.is_relative_to(source):
                    depth = len(source.parts)
                    if depth > best_depth:
                        best, best_depth = index, depth
            assert best is not None
            kept = PurePosixPath(absolute.relative_to(mounts[best].source).as_posix())
            if any(previous == kept for _, previous in assigned[best]):
                continue
            first_path, _ = options[0]
            assigned[best].append((first_path, kept))
    return assigned


def load_source_nav(mount: Mount, source_dir: Path) -> list[dict[str, str]]:
    nav_path = source_dir / NAV_FILENAME
    if not nav_path.is_file():
        raise DocsError(
            f"mount {mount.repo!r} -> {str(mount.target)!r}: "
            f"missing source {NAV_FILENAME} next to {str(mount.source)!r}"
        )
    try:
        raw = load_yaml(nav_path)
    except DocsError as error:
        raise DocsError(
            f"mount {mount.repo!r} -> {str(mount.target)!r}: invalid {NAV_FILENAME}: {error}"
        ) from error
    if not isinstance(raw, list) or not raw:
        raise DocsError(f"{nav_path}: expected a nonempty ordered list of pages")
    entries: list[dict[str, str]] = []
    seen: set[str] = set()
    for position, raw_entry in enumerate(raw, start=1):
        where = f"{nav_path}: pages[{position}]"
        entry = _mapping(raw_entry, where, NAV_ENTRY_KEYS, NAV_ENTRY_KEYS)
        title, rel = entry["title"], entry["path"]
        if not isinstance(title, str) or not title.strip():
            raise DocsError(f"{where}: title must be a nonempty string")
        relative = _relative_posix(rel, f"{where}: path")
        if relative.suffix != ".md":
            raise DocsError(
                f"{where}: nav path {str(relative)!r} must be a Markdown page"
            )
        if str(relative) in seen:
            raise DocsError(f"{where}: duplicate nav path {str(relative)!r}")
        seen.add(str(relative))
        if (
            relative == PurePosixPath(INDEX_FILENAME)
            or relative.parts[0] == INDEX_FILENAME
        ):
            raise DocsError(
                f"{where}: {INDEX_FILENAME} is the committed landing page and must not "
                "be listed in nav.yml; it is included exactly once by the assembler"
            )
        entries.append({"title": title, "path": str(relative)})
    return entries


def plan_mounts(
    mounts: list[Mount],
    repo_dirs: dict[str, Path],
) -> list[MountPlan]:
    candidates: dict[int, list[tuple[Path, PurePosixPath]]] = {}
    source_dirs: dict[int, Path] = {}
    for index, mount in enumerate(mounts):
        repo_dir = repo_dirs[mount.repo]
        source_dir, found = _collect_candidates(mount, repo_dir)
        source_dirs[index] = source_dir
        candidates[index] = found
    assigned = _partition_nested(mounts, candidates)
    plans: list[MountPlan] = []
    for index, mount in enumerate(mounts):
        source_dir = source_dirs[index]
        available = {str(relative) for _, relative in assigned[index]}
        nav = load_source_nav(mount, source_dir)
        for entry in nav:
            if entry["path"] not in available:
                raise DocsError(
                    f"mount {mount.repo!r} -> {str(mount.target)!r}: nav.yml page "
                    f"{entry['path']!r} is missing from {str(mount.source)!r} "
                    "(it may be covered by a more specific nested mount source)"
                )
        ordered = sorted(assigned[index], key=lambda item: str(item[1]))
        files = tuple(
            (relative, PurePosixPath((mount.target / relative).as_posix()))
            for _, relative in ordered
        )
        by_relative = {str(relative): path for path, relative in assigned[index]}
        ordered_paths = tuple(
            (by_relative[str(relative)], target_relative)
            for relative, target_relative in files
        )
        plans.append(
            MountPlan(
                mount=mount,
                repo_dir=repo_dirs[mount.repo],
                source_dir=source_dir,
                files=ordered_paths,
                nav=tuple(nav),
            )
        )
    for plan in plans:
        for _, target_relative in plan.files:
            if target_relative == PurePosixPath(
                f"{plan.mount.target.as_posix()}/{INDEX_FILENAME}"
            ):
                raise DocsError(
                    f"mount {plan.mount.repo!r} -> {str(plan.mount.target)!r}: source ships "
                    f"{INDEX_FILENAME}, which collides with the committed landing page; "
                    "keep the landing page in bob-cli or remove it from the source"
                )
    return plans


def _split_front_matter(text: str) -> tuple[str | None, str]:
    lines = text.splitlines(keepends=True)
    if not lines or lines[0].strip() != "---":
        return None, text
    for end in range(1, len(lines)):
        if lines[end].strip() in ("---", "..."):
            return "".join(lines[1:end]), "".join(lines[end + 1 :])
    return None, text


def stamp_markdown(text: str, *, repo: str, repo_path: str) -> str:
    front_matter, body = _split_front_matter(text)
    if front_matter is None:
        return f"---\n{SOURCE_KEY}: {repo}:{repo_path}\n---\n{text}"
    parsed = load_yaml_text_mapping(front_matter)
    if SOURCE_KEY in parsed:
        raise DocsError(
            f"mount source {repo}:{repo_path} already sets {SOURCE_KEY!r}; "
            "authored source spoofing is rejected"
        )
    return f"---\n{SOURCE_KEY}: {repo}:{repo_path}\n{front_matter}---\n{body}"


def load_yaml_text_mapping(text: str) -> dict[str, Any]:
    from .yaml_tools import load_yaml_text

    parsed = load_yaml_text(text, source="<mount front matter>")
    if parsed is None:
        return {}
    if not isinstance(parsed, dict):
        raise DocsError("mount page front matter must be a mapping")
    return parsed


def _tracked_files(root: Path) -> set[str] | None:
    result = _git(root, "ls-files", "-z")
    if result is None:
        return None
    return {part for part in result.split("\0") if part}


def manifest_path(root: Path) -> Path:
    from .context import context_from_root

    return context_from_root(root).output_dir / MANIFEST_FILENAME


def _owned_files(root: Path) -> set[str]:
    try:
        raw = json.loads(manifest_path(root).read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return set()
    files = raw.get("files") if isinstance(raw, dict) else None
    if not isinstance(files, list) or not all(isinstance(item, str) for item in files):
        return set()
    return set(files)


def _check_no_symlink_escape(
    relative: PurePosixPath,
    *,
    docs_root: Path,
    docs_resolved: Path,
    blame: str,
) -> Path:
    """Reject symlink components and resolved parents outside the docs root."""

    destination = docs_root / Path(*relative.parts)
    parts = relative.parts
    for depth in range(1, len(parts) + 1):
        component = docs_root / Path(*parts[:depth])
        if component.is_symlink():
            raise DocsError(f"{blame}: {component} is a symlink; refusing to follow it")
    parent_resolved = destination.parent.resolve()
    if parent_resolved != docs_resolved and not parent_resolved.is_relative_to(
        docs_resolved
    ):
        raise DocsError(
            f"{blame}: {destination} resolves outside the docs root {docs_resolved}; "
            "mount copies stay beneath docs/"
        )
    return destination


def _check_stale_deletable(
    stale: str,
    *,
    root: Path,
    docs_root: Path,
    docs_resolved: Path,
    tracked: set[str] | None,
) -> Path:
    relative = PurePosixPath(stale)
    if relative.is_absolute() or ".." in relative.parts:
        raise DocsError(
            f"mount manifest lists stale path {stale!r}, which is outside docs/; "
            "refusing to delete it"
        )
    victim = _check_no_symlink_escape(
        relative,
        docs_root=docs_root,
        docs_resolved=docs_resolved,
        blame=f"stale mount output {stale!r}",
    )
    tracked_rel = f"docs/{relative.as_posix()}"
    if tracked is not None and tracked_rel in tracked:
        raise DocsError(
            f"stale mount output {stale!r} is a tracked file ({tracked_rel}); "
            "refusing to delete it"
        )
    if not path_ignored(root, tracked_rel):
        raise DocsError(
            f"stale mount output {stale!r} is not gitignored; refusing to delete it"
        )
    if victim.is_symlink() or victim.is_file():
        return victim
    if victim.exists():
        raise DocsError(
            f"stale mount output {stale!r} is not a plain file; refusing to delete it"
        )
    return victim


def validate_mount_targets(
    plans: list[MountPlan],
    *,
    root: Path,
    docs_root: Path,
) -> None:
    tracked = _tracked_files(root)
    owned = _owned_files(root)
    docs_resolved = docs_root.resolve()
    for plan in plans:
        target_dir = docs_root / Path(*plan.mount.target.parts)
        index = target_dir / INDEX_FILENAME
        if not index.is_file() or index.is_symlink():
            raise DocsError(
                f"mount {plan.mount.repo!r} -> {str(plan.mount.target)!r}: missing committed "
                f"landing page {index}; commit it before mounting"
            )
        probe = f"docs/{plan.mount.target.as_posix()}/.__bob_docs_mount_probe.md"
        if not path_ignored(root, probe):
            raise DocsError(
                f"mount {plan.mount.repo!r} -> {str(plan.mount.target)!r}: target is not "
                "gitignored; copied pages must be ignored apart from index.md"
            )
        if tracked is not None:
            offenders = sorted(
                path
                for path in tracked
                if path.startswith(f"docs/{plan.mount.target.as_posix()}/")
                and path != f"docs/{plan.mount.target.as_posix()}/{INDEX_FILENAME}"
            )
            if offenders:
                raise DocsError(
                    f"mount {plan.mount.repo!r} -> {str(plan.mount.target)!r}: target holds "
                    f"tracked files outside the landing page: {', '.join(offenders)}; "
                    "mount targets are gitignored apart from index.md"
                )
        for _, target_relative in plan.files:
            tracked_rel = f"docs/{target_relative.as_posix()}"
            if not path_ignored(root, tracked_rel):
                raise DocsError(
                    f"mount {plan.mount.repo!r} -> {str(plan.mount.target)!r}: "
                    f"{tracked_rel} is not gitignored; copied pages must be ignored "
                    "apart from index.md"
                )
            _check_no_symlink_escape(
                target_relative,
                docs_root=docs_root,
                docs_resolved=docs_resolved,
                blame=f"mount {plan.mount.repo!r}",
            )
            destination = docs_root / Path(*target_relative.parts)
            if destination.exists() and str(target_relative) not in owned:
                raise DocsError(
                    f"mount {plan.mount.repo!r} -> {str(plan.mount.target)!r}: refusing to "
                    f"overwrite authored file {destination}; mount copies own only "
                    "previously copied outputs"
                )


def sync_mount_copies(
    plans: list[MountPlan],
    *,
    root: Path,
    docs_root: Path,
) -> set[str]:
    docs_resolved = docs_root.resolve()
    tracked = _tracked_files(root)
    owned = _owned_files(root)
    expected: set[str] = set()
    source_stamps: dict[str, str] = {}
    writes: list[tuple[Path, Path, str]] = []
    for plan in plans:
        for source_path, target_relative in plan.files:
            expected.add(str(target_relative))
            destination = docs_root / Path(*target_relative.parts)
            repo_relative = source_path.relative_to(plan.repo_dir).as_posix()
            if source_path.suffix == ".md":
                source_stamps[str(target_relative)] = (
                    f"{plan.mount.repo}:{repo_relative}"
                )
                text = _read_source_text(source_path)
                try:
                    stamped = stamp_markdown(
                        text, repo=plan.mount.repo, repo_path=repo_relative
                    )
                except DocsError as error:
                    raise DocsError(
                        f"mount {plan.mount.repo!r} -> {str(plan.mount.target)!r}: {error}"
                    ) from error
                writes.append((source_path, destination, stamped))
            else:
                writes.append((source_path, destination, ""))
    stale_victims: list[Path] = []
    for stale in sorted(owned - expected):
        stale_victims.append(
            _check_stale_deletable(
                stale,
                root=root,
                docs_root=docs_root,
                docs_resolved=docs_resolved,
                tracked=tracked,
            )
        )
    for victim in stale_victims:
        if victim.is_symlink() or victim.is_file():
            victim.unlink()
    for _, destination, text in writes:
        destination.parent.mkdir(parents=True, exist_ok=True)
        if text:
            destination.write_text(text, encoding="utf-8")
        else:
            source_path = next(src for src, dest, _ in writes if dest == destination)
            shutil.copyfile(source_path, destination)
    for stale in sorted(owned - expected):
        parent = (docs_root / Path(*PurePosixPath(stale).parts)).parent
        docs_resolved = docs_root.resolve()
        while parent != docs_resolved and parent.is_dir():
            try:
                parent.rmdir()
            except OSError:
                break
            parent = parent.parent
    manifest_path(root).parent.mkdir(parents=True, exist_ok=True)
    manifest_path(root).write_text(
        json.dumps(
            {"files": sorted(expected), "sources": dict(sorted(source_stamps.items()))},
            indent=2,
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )
    return expected


def _walk_nav(nav: Any, visit: Any) -> Any:
    if isinstance(nav, list):
        return [_walk_nav(item, visit) for item in nav]
    if isinstance(nav, dict) and len(nav) == 1:
        ((title, value),) = nav.items()
        replaced = visit(title, value)
        if replaced is not None:
            return replaced
        if isinstance(value, list):
            return {title: _walk_nav(value, visit)}
    return nav


def index_title(docs_root: Path, target: PurePosixPath, fallback: str) -> str:
    from .yaml_tools import load_yaml_text

    text = (docs_root / Path(*target.parts) / INDEX_FILENAME).read_text(
        encoding="utf-8"
    )
    front_matter, _ = _split_front_matter(text)
    if front_matter is None:
        return fallback
    try:
        parsed = load_yaml_text(front_matter, source=str(target / INDEX_FILENAME))
    except DocsError:
        return fallback
    if (
        isinstance(parsed, dict)
        and isinstance(parsed.get("title"), str)
        and parsed["title"].strip()
    ):
        return parsed["title"].strip()
    return fallback


def expand_mount_nav(
    config: dict[str, Any],
    plans: list[MountPlan],
    *,
    docs_root: Path,
) -> dict[str, Any]:
    nav = config.get("nav")
    if not isinstance(nav, list):
        raise DocsError("mkdocs.yml: nav must be a list to expand mount placeholders")
    by_target = {plan.mount.target.as_posix(): plan for plan in plans}
    expanded: set[str] = set()

    def visit(title: str, value: Any) -> Any:
        if not isinstance(value, str) or not value.endswith(f"/{INDEX_FILENAME}"):
            return None
        key = value[: -(len(INDEX_FILENAME) + 1)]
        plan = by_target.get(key)
        if plan is None:
            return None
        children: list[Any] = [
            {index_title(docs_root, plan.mount.target, plan.mount.nav_title): value}
        ]
        seen = {value}
        for entry in plan.nav:
            qualified = f"{plan.mount.target.as_posix()}/{entry['path']}"
            if qualified in seen:
                raise DocsError(
                    f"mount {plan.mount.repo!r}: nav.yml expands {qualified!r} twice"
                )
            seen.add(qualified)
            children.append({entry["title"]: qualified})
        expanded.add(key)
        return {plan.mount.nav_title: children}

    config["nav"] = _walk_nav(nav, visit)
    missing = [key for key in by_target if key not in expanded]
    if missing:
        raise DocsError(
            "mkdocs.yml: no nav placeholder for mount target(s) "
            + ", ".join(sorted(missing))
            + "; add '- <title>: <target>/index.md' entries"
        )
    _reject_unresolved_placeholders(
        config["nav"], docs_root=docs_root, known=set(by_target)
    )
    return config


def _nav_leaves(nav: Any) -> list[str]:
    leaves: list[str] = []
    if isinstance(nav, list):
        for item in nav:
            leaves.extend(_nav_leaves(item))
    elif isinstance(nav, dict):
        for value in nav.values():
            if isinstance(value, str):
                leaves.append(value)
            else:
                leaves.extend(_nav_leaves(value))
    elif isinstance(nav, str):
        leaves.append(nav)
    return leaves


def _reject_unresolved_placeholders(
    nav: Any, *, docs_root: Path, known: set[str]
) -> None:
    for leaf in _nav_leaves(nav):
        if not leaf.endswith(f"/{INDEX_FILENAME}"):
            continue
        key = leaf[: -(len(INDEX_FILENAME) + 1)]
        if key in known:
            continue
        if (docs_root / Path(*PurePosixPath(leaf).parts)).is_file():
            continue
        raise DocsError(
            f"mkdocs.yml: nav placeholder {leaf!r} matches no mount target and no page; "
            "add a mounts.yml entry or fix the path"
        )


@dataclass(frozen=True)
class ProductionSource:
    name: str
    path: Path
    sha: str


def load_production_sources(
    sources_path: Path, *, root: Path, repos: list[str]
) -> list[ProductionSource]:
    try:
        raw = json.loads(sources_path.read_text(encoding="utf-8"))
    except OSError as error:
        raise DocsError(f"cannot read --sources {sources_path}: {error}") from error
    except ValueError as error:
        raise DocsError(f"--sources {sources_path}: invalid JSON: {error}") from error
    top = _mapping(raw, str(sources_path), {"sources"}, {"sources"})
    if not isinstance(top["sources"], dict):
        raise DocsError(f"{sources_path}: sources must be a mapping of repo names")
    expected = {"bob-cli", *repos}
    entries = top["sources"]
    missing = expected - set(entries)
    unknown = set(entries) - expected
    if missing:
        raise DocsError(
            f"{sources_path}: missing production source entries: {', '.join(sorted(missing))}"
        )
    if unknown:
        raise DocsError(
            f"{sources_path}: unknown production source entries: {', '.join(sorted(unknown))}"
        )
    base = sources_path.parent.resolve()
    parsed: list[ProductionSource] = []
    for name in sorted(entries):
        where = f"{sources_path}: sources.{name}"
        entry = _mapping(entries[name], where, {"path", "sha"}, {"path", "sha"})
        if not isinstance(entry["path"], str) or not entry["path"]:
            raise DocsError(f"{where}: path must be a nonempty string")
        if not isinstance(entry["sha"], str) or not _SHA.fullmatch(entry["sha"]):
            raise DocsError(f"{where}: sha must be a full 40-character commit SHA")
        resolved = (base / entry["path"]).resolve()
        parsed.append(ProductionSource(name=name, path=resolved, sha=entry["sha"]))
    by_name = {source.name: source for source in parsed}
    if by_name["bob-cli"].path != root.resolve():
        raise DocsError(
            f"{sources_path}: sources.bob-cli.path must be the actual build root {root.resolve()}"
        )
    return parsed


def verify_production_clean(sources: list[ProductionSource]) -> None:
    for source in sources:
        if not source.path.is_dir():
            raise DocsError(
                f"production source {source.name!r} is missing: {source.path} does not exist"
            )
        head = _git(source.path, "rev-parse", "HEAD")
        if head is None:
            raise DocsError(
                f"production source {source.name!r} at {source.path} is not a git checkout"
            )
        if head != source.sha:
            raise DocsError(
                f"production source {source.name!r} HEAD {head} does not match "
                f"recorded SHA {source.sha}"
            )
        status = _git(source.path, "status", "--porcelain", "--untracked-files=all")
        if status is None:
            raise DocsError(
                f"production source {source.name!r}: could not inspect git state"
            )
        if status:
            preview = "\n".join(status.splitlines()[:10])
            raise DocsError(
                f"production source {source.name!r} at {source.path} is not clean "
                f"(ignored build outputs do not count):\n{preview}"
            )


def reject_production_env_overrides() -> None:
    offenders = sorted(
        key
        for key in os.environ
        if key.startswith("BOB_DOCS_") and key.endswith("_DIR")
    )
    if offenders:
        raise DocsError(
            "production assembly refuses discovery env overrides: "
            + ", ".join(f"${name}" for name in offenders)
        )
