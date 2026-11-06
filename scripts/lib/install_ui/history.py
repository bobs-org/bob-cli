from __future__ import annotations

import os
import subprocess
from datetime import datetime
from pathlib import Path
from typing import Optional, Sequence

from install_ui import state


DEPENDENCY_FILES = {
    "bob-cli": ("Cargo.lock", "Cargo.toml", "rust-toolchain.toml"),
    "bob-mac-capture": ("Package.swift", "Package.resolved"),
}


def run_id() -> str:
    value = os.environ.get("BOB_INSTALL_RUN_ID") or os.environ.get("BOB_INSTALL_ALL_RUN_ID")
    if value:
        return value
    return datetime.now().astimezone().strftime("%Y%m%d-%H%M%S") + f"-{os.getpid()}"


def now_iso() -> str:
    return datetime.now().astimezone().isoformat(timespec="seconds")


def host_name() -> str:
    import socket

    return socket.gethostname()


def _step_for_path(step: str) -> str:
    if step in ("bob-cli", "bob-mac-capture"):
        return step
    if "mac" in step.lower():
        return "bob-mac-capture"
    return "bob-cli"


def probe_toolchain(step: str, repo: str, *, timeout: float = 5.0) -> str:
    source = _step_for_path(step)
    if source == "bob-mac-capture":
        command = ["Scripts/xcode-swift.sh", "--version"]
    else:
        command = ["rustc", "-V"]
    try:
        proc = subprocess.run(
            command,
            cwd=repo,
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            timeout=timeout,
        )
    except (OSError, subprocess.TimeoutExpired):
        return "unknown"
    if proc.returncode != 0 or not proc.stdout:
        return "unknown"
    return proc.stdout.splitlines()[0].strip() or "unknown"


def _git(repo: str, *args: str, timeout: float = 8.0) -> Optional[str]:
    try:
        proc = subprocess.run(
            ["git", "-C", repo, *args],
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            timeout=timeout,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    if proc.returncode != 0:
        return None
    return proc.stdout.rstrip()


def _watermark(step: str) -> Optional[dict]:
    result = state.get_watermark(_step_for_path(step))
    return result.value if result.ok and isinstance(result.value, dict) else None


def _changed_files(repo: str, base: Optional[str], head: str) -> set[str]:
    changed: set[str] = set()
    if base and base != head:
        names = _git(repo, "diff", "--name-only", f"{base}..{head}")
        if names:
            changed.update(names.splitlines())
    status = _git(repo, "status", "--porcelain")
    if status:
        for line in status.splitlines():
            if len(line) >= 4:
                path = line[3:].split(" -> ")[-1]
                changed.add(path)
    return changed


def _valid_base(repo: str, candidate: Optional[str], head: str) -> Optional[str]:
    if not candidate or candidate == head:
        return candidate if candidate == head else None
    resolved = _git(repo, "rev-parse", "--verify", f"{candidate}^{{commit}}")
    if not resolved:
        return None
    try:
        proc = subprocess.run(
            ["git", "-C", repo, "merge-base", "--is-ancestor", resolved, head],
            check=False,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=5,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    return resolved if proc.returncode == 0 else None


def _pull_base(step: str, repo: str, head: str) -> Optional[str]:
    old = None
    if _step_for_path(step) == "bob-cli":
        old = os.environ.get("BOB_INSTALL_ALL_SELF_PULL_OLD")
    old = old or os.environ.get("BOB_INSTALL_UI_PULL_OLD")
    return _valid_base(repo, old, head)


def _latest_step_row(step: str) -> Optional[dict]:
    result = state.read_history()
    if not result.ok:
        return None
    rows = [row for row in result.value if row.get("step") == step]
    rows.sort(key=lambda row: str(row.get("ts", "")))
    return rows[-1] if rows else None


def _artifact_exists(step: str) -> bool:
    if _step_for_path(step) == "bob-mac-capture":
        return (Path.home() / "Applications" / "Bob Mac Capture.app").exists()
    root = os.environ.get("CARGO_INSTALL_ROOT") or os.environ.get("CARGO_HOME")
    root_path = Path(root).expanduser() if root else Path.home() / ".cargo"
    return (root_path / "bin" / "bob").exists()


def _build_dir(step: str, repo: str, profile: str) -> Path:
    if _step_for_path(step) == "bob-mac-capture":
        return Path(repo) / ".build" / "release"
    target = Path(os.environ.get("CARGO_TARGET_DIR", "target"))
    if not target.is_absolute():
        target = Path(repo) / target
    return target / (profile or "release")


def predict_kind(
    step: str,
    repo: str,
    profile: str = "local-install",
    *,
    toolchain: Optional[str] = None,
) -> str:
    """Predict the build kind using only local repository and per-host state."""
    head = _git(repo, "rev-parse", "HEAD")
    if not head:
        return "rebuild"
    snap = state.snapshot_repo(repo)
    dirty = not snap.ok or bool(snap.value.get("dirty"))
    watermark = _watermark(step)
    water_sha = str(watermark.get("sha", "")) if watermark else ""
    if water_sha == head and not dirty and _artifact_exists(step):
        return "noop"

    toolchain = toolchain or probe_toolchain(step, repo)
    latest = _latest_step_row(step)
    if not _build_dir(step, repo, profile).exists():
        return "cold"
    if latest and latest.get("toolchain") and latest.get("toolchain") != toolchain:
        return "cold"

    base = _valid_base(repo, water_sha or None, head)
    if base is None:
        base = _pull_base(step, repo, head)
    relevant = set(DEPENDENCY_FILES.get(_step_for_path(step), ()))
    changed = _changed_files(repo, base, head)
    if any(Path(name).name in relevant for name in changed):
        return "deps"
    return "rebuild"


def kind_noun(kind: str) -> str:
    return {
        "noop": "no-change install",
        "cold": "cold build",
        "deps": "dependency rebuild",
        "rebuild": "build",
    }.get(kind, "build")


def incremental_for(step: str) -> Optional[bool]:
    if _step_for_path(step) == "bob-mac-capture":
        return None
    return os.environ.get("CARGO_INCREMENTAL") != "0"


def make_row(
    *,
    step: str,
    phase: str,
    secs: float,
    outcome: str,
    kind: str = "rebuild",
    waited: float = 0.0,
    toolchain: str = "unknown",
    profile: str = "",
    incremental: Optional[bool] = None,
    run: Optional[str] = None,
    slept: bool = False,
) -> dict:
    return {
        "v": 1,
        "run": run or run_id(),
        "ts": now_iso(),
        "host": host_name(),
        "step": step,
        "phase": phase,
        "kind": kind,
        "secs": max(0.0, float(secs)),
        "waited": max(0.0, float(waited)),
        "outcome": outcome,
        "toolchain": toolchain,
        "profile": profile,
        "incremental": incremental,
        "slept": bool(slept),
    }


def append_step_rows(
    *,
    step: str,
    phases: Sequence[str],
    durations: dict[str, float],
    waits: dict[str, float],
    outcome: str,
    kind: str,
    toolchain: str,
    profile: str,
    incremental: Optional[bool],
    run: str,
    slept: bool,
) -> None:
    for phase in phases:
        row = make_row(
            step=step,
            phase=phase,
            secs=durations.get(phase, 0.0),
            waited=waits.get(phase, 0.0),
            outcome=outcome,
            kind=kind,
            toolchain=toolchain,
            profile=profile,
            incremental=incremental,
            run=run,
            slept=slept,
        )
        state.append_history(row)


def record(
    *,
    step: str,
    phase: str,
    secs: float,
    outcome: str,
    kind: str = "rebuild",
    waited: float = 0.0,
    toolchain: str = "unknown",
    profile: str = "",
    incremental: Optional[bool] = None,
) -> state.Result:
    return state.append_history(
        make_row(
            step=step,
            phase=phase,
            secs=secs,
            waited=waited,
            outcome=outcome,
            kind=kind,
            toolchain=toolchain,
            profile=profile,
            incremental=incremental,
        )
    )
