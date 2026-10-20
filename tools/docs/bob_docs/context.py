"""Explicit repository and output paths shared by docs commands."""

from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from .errors import DocsError
from .yaml_tools import load_yaml


@dataclass(frozen=True)
class DocsContext:
    repo_root: Path
    docs_root: Path
    authored_config: Path
    output_dir: Path
    config: dict[str, Any]
    source_dirs: dict[str, Path] = field(default_factory=dict)
    assembled: bool = False

    @property
    def assembled_config(self) -> Path:
        return self.output_dir / "mkdocs.yml"

    @property
    def site_dir(self) -> Path:
        site_dir = self.config.get("site_dir")
        if not isinstance(site_dir, str):
            raise DocsError("assembled config has no string site_dir")
        path = Path(site_dir)
        if not path.is_absolute():
            path = self.authored_config.parent / path
        return path.resolve()


def context_from_root(root: Path) -> DocsContext:
    repo_root = root.resolve()
    authored = repo_root / "mkdocs.yml"
    config = load_yaml(authored)
    if not isinstance(config, dict):
        raise DocsError(f"{authored}: configuration must be a mapping")
    docs_dir = config.get("docs_dir", "docs")
    if not isinstance(docs_dir, str) or not docs_dir:
        raise DocsError(f"{authored}: docs_dir must be a path string")
    docs_root = Path(docs_dir)
    if not docs_root.is_absolute():
        docs_root = authored.parent / docs_root
    docs_root = docs_root.resolve()
    if not docs_root.is_dir():
        raise DocsError(f"configured docs_dir does not exist: {docs_root}")
    return DocsContext(repo_root, docs_root, authored, repo_root / "target/docs", config)
