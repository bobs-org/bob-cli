"""Run the bob-plugins inventory generator from an already resolved source."""

from __future__ import annotations

import shutil
import subprocess
from pathlib import Path

from .errors import DocsError
from .mounts import discover_repo, load_mounts

PLUGIN_REPO = "bob-plugins"
GENERATOR_REL = Path("scripts/docs-inventory.mjs")


def _plugin_mounts(root: Path):
    return [mount for mount in load_mounts(root) if mount.repo == PLUGIN_REPO]


def resolve_plugin_source(
    root: Path,
    *,
    source_dirs: dict[str, Path] | None = None,
    production: bool = False,
) -> Path | None:
    """Return the canonical bob-plugins checkout, or None when it is not mounted."""

    mounts = _plugin_mounts(root)
    if not mounts:
        return None
    if source_dirs is not None and PLUGIN_REPO in source_dirs:
        plugin_root = Path(source_dirs[PLUGIN_REPO])
        if not plugin_root.is_dir():
            raise DocsError(
                f"resolved {PLUGIN_REPO} source {plugin_root} is not a usable directory"
            )
        return plugin_root
    if production:
        raise DocsError(
            "production check cannot rediscover bob-plugins from the environment "
            "or cache; pin it in --sources"
        )
    if source_dirs is not None:
        raise DocsError(
            "plugin mount is configured but bob-plugins is missing from the "
            "assembled source context; it will not be rediscovered from the "
            "environment or cache"
        )
    plugin_root, _ = discover_repo(mounts[0], root=root)
    return plugin_root


def run_plugin_inventory(
    root: Path,
    *,
    check: bool,
    source_dirs: dict[str, Path] | None = None,
    production: bool = False,
) -> int:
    """Generate or check plugin inventory in the canonical bob-plugins tree.

    Mount copies are never written. Returns 0 when the plugin mount is absent.
    """

    plugin_root = resolve_plugin_source(
        root, source_dirs=source_dirs, production=production
    )
    if plugin_root is None:
        return 0
    generator = plugin_root / GENERATOR_REL
    if not generator.is_file() or generator.is_symlink():
        raise DocsError(
            f"plugin inventory generator is missing or not a regular file: {generator}"
        )
    node = shutil.which("node")
    if node is None:
        raise DocsError(
            "plugin inventory generation requires Node; 'node' is not on PATH"
        )
    command = [node, str(generator)]
    if check:
        command.append("--check")
    try:
        result = subprocess.run(command, cwd=plugin_root, check=False)
    except OSError as error:
        raise DocsError(
            f"could not start plugin inventory generator {generator}: {error}"
        ) from error
    return result.returncode
