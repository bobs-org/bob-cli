"""Shared isolated Bob runtime for executable documentation examples.

Tutorial and media commands use this module so they exercise the checkout's
current Bob binary against a disposable synthetic vault, never the reader's or
agent's vault.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import tempfile
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path
from typing import Iterator, Mapping

from .errors import DocsError


DEFAULT_NOW = "2026-10-20T09:02:00"
_DESKTOP_ENV = (
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "XAUTHORITY",
    "DBUS_SESSION_BUS_ADDRESS",
    "OBSIDIAN_APP_PATH",
    "OBSIDIAN_URI",
    "SSH_TTY",
    "CLIPBOARD",
    "BOB_CLIPBOARD",
    "BOB_CLIPBOARD_BACKEND",
)


@dataclass(frozen=True)
class BobSandbox:
    """Paths and environment for one disposable Bob run."""

    root: Path
    workdir: Path
    home: Path
    vault: Path
    demo_source: Path
    env: dict[str, str]
    bob_bin: Path
    now: str


def _cargo_target_dir(repo_root: Path, cargo: str) -> Path:
    try:
        completed = subprocess.run(
            [cargo, "metadata", "--no-deps", "--format-version", "1"],
            cwd=repo_root,
            check=False,
            capture_output=True,
            text=True,
            timeout=60,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise DocsError(f"could not ask Cargo for the target directory: {error}") from error
    if completed.returncode:
        detail = completed.stderr.strip() or completed.stdout.strip()
        raise DocsError(f"cargo metadata failed with exit {completed.returncode}: {detail}")
    try:
        target_dir = json.loads(completed.stdout)["target_directory"]
    except (KeyError, TypeError, json.JSONDecodeError) as error:
        raise DocsError("cargo metadata did not report a target directory") from error
    return Path(target_dir).resolve()


def resolve_bob_binary(repo_root: Path, *, source_env: Mapping[str, str] | None = None) -> Path:
    """Use an explicit BOB_BIN or build this checkout and resolve Cargo's target dir."""

    env = os.environ if source_env is None else source_env
    if "BOB_BIN" in env:
        raw = env["BOB_BIN"]
        candidate = Path(raw).expanduser()
        if not raw:
            raise DocsError("BOB_BIN is set but empty; set it to an executable Bob path")
        if not candidate.is_absolute():
            candidate = repo_root / candidate
        candidate = candidate.resolve()
        if not candidate.is_file() or not os.access(candidate, os.X_OK):
            raise DocsError(f"BOB_BIN is not an executable file: {candidate}")
        return candidate

    cargo = shutil.which("cargo", path=env.get("PATH"))
    if cargo is None:
        raise DocsError("the tutorial needs Cargo or BOB_BIN pointing to an executable Bob")
    try:
        built = subprocess.run(
            [cargo, "build", "--locked", "--bin", "bob"],
            cwd=repo_root,
            check=False,
            text=True,
            timeout=600,
        )
    except subprocess.TimeoutExpired as error:
        raise DocsError("cargo build --locked --bin bob timed out after 10 minutes") from error
    except OSError as error:
        raise DocsError(f"could not start Cargo: {error}") from error
    if built.returncode:
        raise DocsError(f"cargo build --locked --bin bob failed with exit {built.returncode}")
    target_dir = _cargo_target_dir(repo_root, cargo)
    executable = target_dir / "debug" / ("bob.exe" if os.name == "nt" else "bob")
    if not executable.is_file() or not os.access(executable, os.X_OK):
        raise DocsError(f"Cargo built Bob, but the expected executable is missing: {executable}")
    return executable.resolve()


def isolated_environment(
    *,
    root: Path,
    bob_bin: Path,
    now: str,
    source_env: Mapping[str, str] | None = None,
) -> dict[str, str]:
    """Construct an environment that cannot inherit vault or desktop redirects."""

    source = os.environ if source_env is None else source_env
    env = dict(source)
    for key in tuple(env):
        if key.startswith("BOB_") or key == "DATE":
            env.pop(key, None)
    for key in _DESKTOP_ENV:
        env.pop(key, None)

    root = root.resolve()
    home = root / "home"
    config = home / ".config"
    data = home / ".local" / "share"
    cache = home / ".cache"
    state = home / ".local" / "state"
    temp = root / "tmp"
    work = root / "work"
    for directory in (home, config, data, cache, state, temp, work):
        directory.mkdir(parents=True, exist_ok=True)

    env.update(
        {
            "HOME": str(home),
            "XDG_CONFIG_HOME": str(config),
            "XDG_DATA_HOME": str(data),
            "XDG_CACHE_HOME": str(cache),
            "XDG_STATE_HOME": str(state),
            "TMPDIR": str(temp),
            "BOB_DIR": str(root / "not-selected-vault"),
            "BOB_CONFIG_FILE": str(config / "bob" / "config.yml"),
            "BOB_NOW": now,
            "BOB_REF_JOBS_KICK": "off",
            "TZ": "UTC",
            "LANG": "en_US.UTF-8",
            "LC_ALL": "en_US.UTF-8",
            "NO_COLOR": "1",
            "PATH": str(bob_bin.parent) + os.pathsep + source.get("PATH", os.defpath),
            "PYTHONPATH": str(Path(__file__).resolve().parents[1]),
        }
    )
    return env


@contextmanager
def bob_sandbox(
    repo_root: Path,
    bob_bin: Path,
    *,
    now: str = DEFAULT_NOW,
    source_env: Mapping[str, str] | None = None,
) -> Iterator[BobSandbox]:
    """Create a temporary home, XDG tree, workdir, and demo-vault copy."""

    fixture = repo_root / "examples" / "demo-vault"
    if not fixture.is_dir():
        raise DocsError(f"synthetic demo vault is missing: {fixture}")
    with tempfile.TemporaryDirectory(prefix="bob-docs-sandbox-") as raw_root:
        root = Path(raw_root).resolve()
        env = isolated_environment(root=root, bob_bin=bob_bin, now=now, source_env=source_env)
        workdir = Path(env["TMPDIR"]).parent / "work"
        demo_source = workdir / "examples" / "demo-vault"
        demo_source.parent.mkdir(parents=True, exist_ok=True)
        shutil.copytree(fixture, demo_source)
        yield BobSandbox(
            root=root,
            workdir=workdir,
            home=Path(env["HOME"]),
            vault=Path(env["BOB_DIR"]),
            demo_source=demo_source,
            env=env,
            bob_bin=bob_bin,
            now=now,
        )
