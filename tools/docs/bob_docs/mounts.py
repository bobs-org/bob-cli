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

from .errors import DocsError
from .yaml_tools import load_yaml

MOUNT_KEYS = {"repo", "remote", "branch", "source", "target", "nav_title"}

NAV_ENTRY_KEYS = {"title", "path"}

SOURCE_KEY = "source"

NAV_FILENAME = "nav.yml"
MEDIA_FILENAME = "media.yml"
INDEX_FILENAME = "index.md"

MANIFEST_FILENAME = "mount-manifest.json"

_SHA = re.compile(r"^[0-9a-f]{40}$")


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
        raise DocsError(f"mount repo {repo!r} has no usable name for discovery env vars")
    return f"BOB_DOCS_{name}_DIR"


def _mapping(value: Any, where: str, allowed: set[str], required: set[str]) -> dict[str, Any]:
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
    if not isinstance(value, str) or not value or value.startswith("/") or value.startswith("~"):
        raise DocsError(f"{where}: must be a relative path beneath its parent")
    path = PurePosixPath(value)
    if any(part in ("", ".", "..") for part in path.parts):
        raise DocsError(f"{where}: must be a relative path without empty, '.', or '..' parts")
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
            if target == other or target.is_relative_to(other) or other.is_relative_to(target):
                raise DocsError(f"{where}: target {str(target)!r} overlaps another mount target")
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


def _git(cwd: Path, *args: str) -> str | None:
    try:
        result = subprocess.run(
            ["git", *args],
            cwd=cwd,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            check=False,
        )
    except OSError:
        return None
    if result.returncode != 0:
        return None
    return result.stdout.strip()


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
            return candidate, f"${var}"
        raise DocsError(
            f"mount repo {mount.repo!r}: ${var}={override} is not a usable directory; "
            "an explicit override never falls through to other locations"
        )
    attempted.append(f"${var} (unset)")

    sibling = (root.parent / mount.repo).resolve()
    attempted.append(str(sibling))
    if sibling.is_dir():
        return sibling, str(sibling)

    linked = (root / "sase/repos/linked" / mount.repo).resolve()
    attempted.append(str(linked))
    if linked.is_dir():
        return linked, str(linked)

    cached = cache_root() / mount.repo
    attempted.append(f"{cached} (shallow clone of {mount.remote} branch {mount.branch})")
    if cached.is_dir():
        return cached.resolve(), str(cached)
    try:
        cached.parent.mkdir(parents=True, exist_ok=True)
        result = subprocess.run(
            ["git", "clone", "--depth", "1", "--branch", mount.branch, mount.remote, str(cached)],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
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


def _collect_candidates(mount: Mount, repo_dir: Path) -> tuple[Path, list[tuple[Path, PurePosixPath]]]:
    source_dir = (repo_dir / Path(*mount.source.parts)).resolve()
    if not source_dir.is_dir():
        raise DocsError(
            f"mount {mount.repo!r} -> {str(mount.target)!r}: "
            f"source directory {str(mount.source)!r} does not exist in {repo_dir}"
        )
    if source_dir != repo_dir.resolve() and not source_dir.is_relative_to(repo_dir.resolve()):
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
                raise DocsError(f"mount source has a broken symlink: {path}: {error}") from error
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
    assigned: dict[int, list[tuple[Path, PurePosixPath]]] = {index: [] for index in range(len(mounts))}
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
            raise DocsError(f"{where}: nav path {str(relative)!r} must be a Markdown page")
        if str(relative) in seen:
            raise DocsError(f"{where}: duplicate nav path {str(relative)!r}")
        seen.add(str(relative))
        if relative == PurePosixPath(INDEX_FILENAME) or relative.parts[0] == INDEX_FILENAME:
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
            (relative, PurePosixPath((mount.target / relative).as_posix())) for _, relative in ordered
        )
        by_relative = {str(relative): path for path, relative in assigned[index]}
        ordered_paths = tuple(
            (by_relative[str(relative)], target_relative) for relative, target_relative in files
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
            if target_relative == PurePosixPath(f"{plan.mount.target.as_posix()}/{INDEX_FILENAME}"):
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
    return f"---\n{SOURCE_KEY}: {repo}:{repo_path}\n{front_matter}{body}"


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


def validate_mount_targets(
    plans: list[MountPlan],
    *,
    root: Path,
    docs_root: Path,
) -> None:
    tracked = _tracked_files(root)
    owned = _owned_files(root)
    for plan in plans:
        target_dir = docs_root / Path(*plan.mount.target.parts)
        index = target_dir / INDEX_FILENAME
        if not index.is_file() or index.is_symlink():
            raise DocsError(
                f"mount {plan.mount.repo!r} -> {str(plan.mount.target)!r}: missing committed "
                f"landing page {index}; commit it before mounting"
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
            destination = docs_root / Path(*target_relative.parts)
            if destination.is_symlink():
                raise DocsError(
                    f"mount {plan.mount.repo!r}: destination {destination} is a symlink; "
                    "refusing to follow it"
                )
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
    owned = _owned_files(root)
    expected: set[str] = set()
    writes: list[tuple[Path, Path, str]] = []
    for plan in plans:
        for source_path, target_relative in plan.files:
            expected.add(str(target_relative))
            destination = docs_root / Path(*target_relative.parts)
            repo_relative = source_path.relative_to(plan.repo_dir).as_posix()
            if source_path.suffix == ".md":
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
    for stale in sorted(owned - expected):
        victim = docs_root / Path(*PurePosixPath(stale).parts)
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
        json.dumps({"files": sorted(expected)}, indent=2, sort_keys=True) + "\n", encoding="utf-8"
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

    text = (docs_root / Path(*target.parts) / INDEX_FILENAME).read_text(encoding="utf-8")
    front_matter, _ = _split_front_matter(text)
    if front_matter is None:
        return fallback
    try:
        parsed = load_yaml_text(front_matter, source=str(target / INDEX_FILENAME))
    except DocsError:
        return fallback
    if isinstance(parsed, dict) and isinstance(parsed.get("title"), str) and parsed["title"].strip():
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
    _reject_unresolved_placeholders(config["nav"], docs_root=docs_root, known=set(by_target))
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


def _reject_unresolved_placeholders(nav: Any, *, docs_root: Path, known: set[str]) -> None:
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


def load_production_sources(sources_path: Path, *, root: Path, repos: list[str]) -> list[ProductionSource]:
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
            raise DocsError(f"production source {source.name!r}: could not inspect git state")
        if status:
            preview = "\n".join(status.splitlines()[:10])
            raise DocsError(
                f"production source {source.name!r} at {source.path} is not clean "
                f"(ignored build outputs do not count):\n{preview}"
            )


def reject_production_env_overrides() -> None:
    offenders = sorted(
        key for key in os.environ if key.startswith("BOB_DOCS_") and key.endswith("_DIR")
    )
    if offenders:
        raise DocsError(
            "production assembly refuses discovery env overrides: "
            + ", ".join(f"${name}" for name in offenders)
        )
