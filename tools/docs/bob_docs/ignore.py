"""Shared gitignore helpers for mount assembly and the docs contract lint."""

from __future__ import annotations

import fnmatch
import subprocess
from pathlib import Path


def gitignore_ignored(root: Path, rel: str) -> bool | None:
    """Whether ``git check-ignore`` marks ``rel`` ignored (None when git cannot say)."""

    try:
        result = subprocess.run(
            ["git", "check-ignore", "-q", "--no-index", rel],
            cwd=root,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        )
    except OSError:
        return None
    if result.returncode == 0:
        return True
    if result.returncode == 1:
        return False
    return None


def gitignore_file_ignored(root: Path, rel: str) -> bool:
    """Best-effort `.gitignore` match used when git cannot answer."""

    path = root / ".gitignore"
    if not path.is_file():
        return False
    posix = rel.replace("\\", "/")
    ignored = False
    for raw in path.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        negate = line.startswith("!")
        pattern = line[1:] if negate else line
        if pattern.endswith("/"):
            pattern += "*"
        anchored = pattern.startswith("/")
        check_pattern = pattern.lstrip("/")
        matched = fnmatch.fnmatch(posix, check_pattern) or fnmatch.fnmatch(posix, pattern)
        if anchored:
            matched = fnmatch.fnmatch("/" + posix, pattern) or fnmatch.fnmatch(posix, check_pattern)
        if matched:
            ignored = not negate
    return ignored


def path_ignored(root: Path, rel: str) -> bool:
    """Whether ``rel`` is gitignored, preferring `git check-ignore` over file parsing."""

    checked = gitignore_ignored(root, rel)
    if checked is not None:
        return checked
    return gitignore_file_ignored(root, rel)
