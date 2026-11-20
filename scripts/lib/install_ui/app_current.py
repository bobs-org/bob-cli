from __future__ import annotations

"""Decide whether an installed app still matches what its checkout would build.

Used by ``install_ui app-current``. The verdict compares the install
watermark in ``installed.json`` with the checkout's working tree by content
(a tree diff, not ancestry), drops paths that never reach the app, and
treats every uncertainty as "rebuild".
"""

import fnmatch
import os
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Optional

from install_ui import commits, state

# Repo-relative paths that never reach the installed app. A repo without an
# entry ignores nothing, so every change rebuilds it.
APP_IGNORED_PATHS = {
    "bob-mac-capture": (
        ".github/*", ".gitignore", ".swift-format", "AGENTS.md", "LICENSE", "README.md",
        "Scripts/docs-check.sh", "Scripts/docs-media.sh", "Scripts/docs_media.py",
        "Scripts/generate-*-fixtures.sh", "Scripts/macos-ci-check", "Scripts/test_*.py",
        "Scripts/typecheck*", "Tests/*", "docs/*",
    ),
}

# The watermark's `at` is written after the install with second precision, so
# an artifact from that same install is never more than about a second newer.
MTIME_SLACK_SECS = 2.0


@dataclass(frozen=True)
class Verdict:
    """One app-current decision; ``advance`` is the watermark to write on --advance."""

    current: bool
    reason: str
    advance: Optional[dict] = None


def _rebuild(reason: str) -> Verdict:
    return Verdict(False, reason)


def is_ignored(repo: str, path: str) -> bool:
    """True when *path* (repo-relative, POSIX) matches *repo*'s ignore list."""
    return any(
        fnmatch.fnmatchcase(path, pattern) for pattern in APP_IGNORED_PATHS.get(repo, ())
    )


def display_path(path: str) -> str:
    """Show the enclosing ``.app`` bundle when there is one, with ``~`` for HOME."""
    shown = Path(path)
    for parent in [shown] + list(shown.parents):
        if parent.name.endswith(".app"):
            shown = parent
            break
    text = str(shown)
    home = os.environ.get("HOME") or ""
    if home and home != "/":
        home = home.rstrip("/")
        if text == home:
            return "~"
        if text.startswith(home + "/"):
            return "~/" + text[len(home) + 1 :]
    return text


def _parse_at(raw: object) -> Optional[datetime]:
    if not isinstance(raw, str) or not raw:
        return None
    try:
        parsed = datetime.fromisoformat(raw)
    except ValueError:
        return None
    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=timezone.utc)
    return parsed


def _git_paths(directory: str, args: list) -> Optional[list]:
    proc = commits._run_git(directory, args)
    if proc is None or proc.returncode != 0:
        return None
    return [path for path in (proc.stdout or "").split("\0") if path]


def changed_paths(directory: str, base: str) -> Optional[list]:
    """Paths whose content differs from *base*'s tree, plus untracked files.

    Covers new commits, tracked local edits, and untracked files; ``None``
    when git cannot answer.
    """
    tracked = _git_paths(
        directory, ["diff", "--name-only", "--no-renames", "-z", base, "--"]
    )
    if tracked is None:
        return None
    untracked = _git_paths(
        directory, ["ls-files", "-z", "--full-name", "--others", "--exclude-standard"]
    )
    if untracked is None:
        return None
    return sorted(set(tracked) | set(untracked))


def _is_clean(directory: str) -> bool:
    proc = commits._run_git(directory, ["status", "--porcelain"])
    return proc is not None and proc.returncode == 0 and not (proc.stdout or "").strip()


def evaluate(repo: str, directory: str, artifact: str) -> Verdict:
    """Apply the app-current rules in order; the first match decides."""
    artifact_path = Path(artifact)
    try:
        mtime = artifact_path.stat().st_mtime
    except OSError:
        return _rebuild(f"app not installed at {display_path(artifact)}")

    found = state.get_watermark(repo)
    record = found.value if found.ok and isinstance(found.value, dict) else None
    sha = record.get("sha") if record is not None else None
    at = _parse_at(record.get("at")) if record is not None else None
    if record is None or not isinstance(sha, str) or not sha.strip() or at is None:
        return _rebuild("no install record")
    sha = sha.strip()
    short = sha[:7]

    resolved = str(Path(directory).resolve())
    if record.get("path") != resolved:
        return _rebuild("last install came from another checkout")
    if record.get("dirty"):
        return _rebuild("last install had local changes")
    if mtime > at.timestamp() + MTIME_SLACK_SECS:
        return _rebuild("app was replaced outside install-all")
    if not commits.is_commit(resolved, sha):
        return _rebuild(f"last install @{short} is not in this checkout")

    changed = changed_paths(resolved, sha)
    head = commits.get_head(resolved)
    if changed is None or not head:
        return _rebuild(f"could not compare with @{short}")
    app_changes = [path for path in changed if not is_ignored(repo, path)]
    if app_changes:
        noun = "file" if len(app_changes) == 1 else "files"
        return _rebuild(f"{len(app_changes)} app {noun} changed since @{short}")

    clean = _is_clean(resolved)
    if head == sha and clean:
        return Verdict(True, f"app current @{short}")
    advance = None
    if head != sha and clean:
        advance = {"sha": head, "at": record["at"], "dirty": False, "path": resolved}
    return Verdict(True, f"only docs/tests changed since @{short}", advance)


def apply_advance(repo: str, verdict: Verdict) -> None:
    """Move a current repo's watermark to HEAD, keeping its ``at``; never raises."""
    if not verdict.current or verdict.advance is None:
        return
    try:
        state.write_watermark(repo, verdict.advance)
    except Exception:
        pass
