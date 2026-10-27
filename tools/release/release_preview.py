#!/usr/bin/env python3
"""Preview what tomorrow's release train would do, with no side effects.

Clones `origin/master` into a temp dir, requires release-plz at the pinned
version, runs `release-plz update` there, and prints both package versions
(or "no release"), each changelog that changed, and `git diff --stat`. Then
runs the guard in explain-only mode against live data to show what the next
scheduled run would decide, degrading gracefully without `gh` auth.

Nothing outside the temp dir is touched: no pushes, tags, publishes, or
working-tree changes.
"""

from __future__ import annotations

import argparse
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from workspace_versions import (
    CHANGELOG_PATHS,
    format_next_versions,
    format_no_release,
    load_release_plz_config,
    read_versions,
)

RELEASE_PLZ_VERSION = "0.3.173"

ROOT = Path(__file__).resolve().parents[2]


def _run(command: list[str], *, cwd: Path | None = None) -> subprocess.CompletedProcess[str]:
    return subprocess.run(command, cwd=str(cwd) if cwd else None,
                          check=False, text=True, capture_output=True)


def release_plz_ok() -> tuple[bool, str]:
    """Check that `release-plz` exists at exactly the pinned version."""
    resolved = shutil.which("release-plz")
    if resolved is None:
        return False, ""
    result = _run(["release-plz", "--version"])
    output = (result.stdout or "") + (result.stderr or "")
    # `release-plz 0.3.173 (...)` -- match the exact pinned version.
    version = ""
    for token in output.replace(",", " ").split():
        if token and token[0].isdigit():
            version = token.strip()
            break
    return version == RELEASE_PLZ_VERSION, version


def install_hint() -> str:
    return (
        f"release-plz {RELEASE_PLZ_VERSION} is required.\n"
        f"Install it with:\n"
        f"  cargo binstall release-plz@{RELEASE_PLZ_VERSION} --locked\n"
        f"or:\n"
        f"  cargo install release-plz --version {RELEASE_PLZ_VERSION} --locked"
    )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--origin", default=None,
                        help="git remote to clone (default: origin of this checkout)")
    args = parser.parse_args(argv)

    ok, found = release_plz_ok()
    if not ok:
        if found:
            sys.stderr.write(f"found release-plz {found}, need {RELEASE_PLZ_VERSION}.\n")
        else:
            sys.stderr.write("release-plz is not on PATH.\n")
        sys.stderr.write(install_hint() + "\n")
        return 1

    with tempfile.TemporaryDirectory(prefix="bob-release-preview-") as temp:
        clone = Path(temp) / "bob-cli"
        origin = args.origin
        if origin is None:
            probe = _run(["git", "remote", "get-url", "origin"], cwd=ROOT)
            origin = probe.stdout.strip() or "origin"
        sys.stdout.write(f"Cloning {origin} (master) into {clone} ...\n")
        cloned = _run(["git", "clone", "--branch", "master", "--depth", "1", origin, str(clone)])
        if cloned.returncode != 0:
            sys.stderr.write(cloned.stderr or cloned.stdout)
            sys.stderr.write("Could not clone origin/master.\n")
            return cloned.returncode
        # release-plz needs full history to decide the bump.
        _run(["git", "fetch", "--unshallow"], cwd=clone)

        before = read_versions(clone)
        config = load_release_plz_config(clone)
        update = _run(["release-plz", "update"], cwd=clone)
        sys.stdout.write(update.stdout or "")
        sys.stderr.write(update.stderr or "")
        if update.returncode != 0:
            return update.returncode

        after = read_versions(clone)
        for changelog_path in CHANGELOG_PATHS:
            if (clone / changelog_path).exists():
                _run(["git", "add", "-N", "--", str(changelog_path)], cwd=clone)
        status = _run(["git", "status", "--porcelain"], cwd=clone)
        if not status.stdout.strip():
            sys.stdout.write(format_no_release(after, config))
        else:
            sys.stdout.write("\n")
            sys.stdout.write(format_next_versions(before, after, config))
            sys.stdout.write("\n")
            diff_stat = _run(["git", "diff", "--stat"], cwd=clone)
            sys.stdout.write(diff_stat.stdout or "")
            for changelog_path in CHANGELOG_PATHS:
                changelog = _run(
                    ["git", "diff", "--", str(changelog_path)], cwd=clone
                )
                if changelog.stdout.strip():
                    sys.stdout.write(f"\n--- {changelog_path} (new section) ---\n")
                    sys.stdout.write(changelog.stdout)

        sys.stdout.write("\n--- guard explain (live data) ---\n")
        guard = _run(
            [sys.executable, str(ROOT / "tools" / "release" / "train_guard.py"),
             "--explain", "--repo", "bobs-org/bob-cli"],
            cwd=ROOT,
        )
        sys.stdout.write(guard.stdout or "")
        sys.stderr.write(guard.stderr or "")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
