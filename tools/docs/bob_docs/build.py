"""Run the strict Zensical or Material builder against assembled configuration."""

from __future__ import annotations

import shutil
import subprocess
from pathlib import Path

from .assemble import assemble
from .errors import DocsError


def build(root: Path, *, material: bool = False) -> int:
    context, manifest = assemble(root)
    executable = "mkdocs" if material else "zensical"
    command = [executable, "build", "--strict", "--config-file", str(context.assembled_config)]
    print("running: " + " ".join(command))
    try:
        result = subprocess.run(command, cwd=context.repo_root, check=False)
    except OSError as error:
        hint = " (install with `uv run --project tools/docs --group material ...`)" if material else ""
        raise DocsError(f"could not start {executable}{hint}: {error}") from error
    if result.returncode != 0:
        return result.returncode

    site_dir = context.site_dir
    if not site_dir.is_dir():
        raise DocsError(f"builder succeeded but site directory is missing: {site_dir}")
    shutil.copyfile(manifest, site_dir / "build.json")
    print(f"built site: {site_dir}")
    return 0


def serve(root: Path) -> int:
    context, _manifest = assemble(root)
    command = ["zensical", "serve", "--config-file", str(context.assembled_config)]
    print("running: " + " ".join(command))
    try:
        return subprocess.run(command, cwd=context.repo_root, check=False).returncode
    except OSError as error:
        raise DocsError(f"could not start zensical serve: {error}") from error
