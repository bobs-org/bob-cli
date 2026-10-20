"""Assemble the authored config into ignored build output."""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
from datetime import datetime, timezone
from importlib.metadata import PackageNotFoundError, version
from pathlib import Path
from typing import Any

from .context import DocsContext, context_from_root
from .contracts import load_contract_registry, registry_extra
from .errors import DocsError
from .mounts import (
    discover_repo,
    expand_mount_nav,
    load_mounts,
    load_production_sources,
    plan_mounts,
    reject_production_env_overrides,
    repo_git_info,
    sync_mount_copies,
    validate_mount_targets,
    verify_production_clean,
)
from .yaml_tools import dump_yaml


def _git(root: Path, *args: str) -> str | None:
    try:
        result = subprocess.run(
            ["git", *args],
            cwd=root,
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


def _tool_version(name: str, fallback: str) -> str:
    try:
        return version(name)
    except PackageNotFoundError:
        return fallback


def build_manifest(
    root: Path,
    mounted: dict[str, Path] | None = None,
    *,
    builder: str = "zensical",
) -> dict[str, Any]:
    sha = _git(root, "rev-parse", "HEAD")
    branch = _git(root, "branch", "--show-current")
    status = _git(root, "status", "--porcelain", "--untracked-files=all")
    sources: dict[str, Any] = {
        "bob-cli": {
            "path": ".",
            "sha": sha,
            "dirty": bool(status),
            "branch": branch or None,
        }
    }
    for repo in sorted((mounted or {})):
        repo_dir = mounted[repo] if mounted else Path()
        info = repo_git_info(repo_dir)
        try:
            shown = (repo_dir.resolve().relative_to(root.resolve())).as_posix()
        except ValueError:
            shown = str(repo_dir.resolve())
        sources[repo] = {"path": shown, **info}
    tools: dict[str, Any] = {
        "bob-docs": _tool_version("bob-docs", "0.1.0"),
        "python": ".".join(map(str, sys.version_info[:3])),
        "zensical": _tool_version("zensical", "unknown"),
    }
    for optional in ("mkdocs", "mkdocs-material"):
        try:
            tools[optional] = version(optional)
        except PackageNotFoundError:
            continue
    return {
        "sources": sources,
        "built_at": datetime.now(timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z"),
        "builder": builder,
        "tools": tools,
    }


def _resolve_authored_path(value: Any, base: Path, field: str) -> Path:
    if not isinstance(value, str) or not value:
        raise DocsError(f"mkdocs.yml: {field} must be a nonempty path string")
    path = Path(value)
    if not path.is_absolute():
        path = base / path
    return path.resolve()


def _rebase_paths(context: DocsContext, config: dict[str, Any]) -> None:
    base = context.authored_config.parent
    output_root = context.output_dir.resolve()
    authored_docs = _resolve_authored_path(config.get("docs_dir", "docs"), base, "docs_dir")
    site_dir = _resolve_authored_path(config.get("site_dir", "site"), base, "site_dir")
    if not site_dir.is_relative_to(output_root):
        raise DocsError(f"mkdocs.yml: site_dir must stay under target/docs, got {site_dir}")
    config["docs_dir"] = "docs"
    config["site_dir"] = Path(os.path.relpath(site_dir, output_root)).as_posix()
    theme = config.get("theme")
    if isinstance(theme, dict) and "custom_dir" in theme:
        authored_custom = _resolve_authored_path(theme["custom_dir"], base, "theme.custom_dir")
        if not authored_custom.is_relative_to(authored_docs):
            raise DocsError("mkdocs.yml: theme.custom_dir must be inside docs_dir for staged builds")
        theme["custom_dir"] = (Path("docs") / authored_custom.relative_to(authored_docs)).as_posix()


def _stage_docs(context: DocsContext) -> None:
    """Copy the authored source tree inside the assembled Zensical project root."""

    stage = context.output_dir / "docs"
    for path in context.docs_root.rglob("*"):
        if path.is_symlink():
            try:
                target = path.resolve(strict=True)
            except OSError as error:
                raise DocsError(f"docs source has a broken symlink: {path}: {error}") from error
            if not target.is_relative_to(context.docs_root):
                raise DocsError(f"docs source symlink escapes docs/: {path} -> {target}")

    if stage.is_symlink():
        stage.unlink()
    elif stage.exists():
        shutil.rmtree(stage)
    shutil.copytree(context.docs_root, stage, symlinks=False)


def assemble(
    root: Path,
    *,
    production: bool = False,
    sources: Path | str | None = None,
    builder: str = "zensical",
) -> tuple[DocsContext, Path]:
    """Write target/docs/mkdocs.yml and build.json without editing tracked sources."""

    if bool(sources) != production:
        if sources is not None:
            raise DocsError("--sources FILE requires --production")
        raise DocsError("--production requires --sources FILE")
    context = context_from_root(root)
    mounts = load_mounts(context.repo_root)
    registry = load_contract_registry(context.repo_root)
    repo_dirs: dict[str, Path] = {}
    production_sources = None
    if production:
        assert sources is not None
        reject_production_env_overrides()
        production_sources = load_production_sources(
            Path(sources), root=context.repo_root, repos=sorted({mount.repo for mount in mounts})
        )
        repo_dirs = {source.name: source.path for source in production_sources if source.name != "bob-cli"}
        verify_production_clean(production_sources)
    else:
        for mount in mounts:
            if mount.repo not in repo_dirs:
                repo_dir, _ = discover_repo(mount, root=context.repo_root)
                repo_dirs[mount.repo] = repo_dir
    context = DocsContext(
        context.repo_root,
        context.docs_root,
        context.authored_config,
        context.output_dir,
        context.config,
        {"bob-cli": context.repo_root, **repo_dirs},
    )
    plans = plan_mounts(mounts, repo_dirs)
    validate_mount_targets(plans, root=context.repo_root, docs_root=context.docs_root)
    sync_mount_copies(plans, root=context.repo_root, docs_root=context.docs_root)
    if production_sources is not None:
        verify_production_clean(production_sources)
    context.output_dir.mkdir(parents=True, exist_ok=True)
    _stage_docs(context)
    config = context.config.copy()
    expand_mount_nav(config, plans, docs_root=context.output_dir / "docs")
    _rebase_paths(context, config)
    extra = config.setdefault("extra", {})
    if not isinstance(extra, dict):
        raise DocsError("mkdocs.yml: extra must be a mapping")
    manifest = build_manifest(context.repo_root, repo_dirs, builder=builder)
    extra["build"] = manifest
    extra["contracts"] = registry_extra(registry)

    manifest_path = context.output_dir / "build.json"
    payload = json.dumps(manifest, indent=2, sort_keys=True) + "\n"
    manifest_path.write_text(payload, encoding="utf-8")
    # Publish the same manifest inside the staged docs tree so build and serve
    # both expose it as the static file /build.json from one copy path.
    (context.output_dir / "docs" / "build.json").write_text(payload, encoding="utf-8")
    context.assembled_config.write_text(dump_yaml(config), encoding="utf-8")
    return context, manifest_path
