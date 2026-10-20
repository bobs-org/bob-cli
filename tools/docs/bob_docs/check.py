"""The ordered local documentation gate."""

from __future__ import annotations

import shutil
import subprocess
import os
from pathlib import Path
from typing import Callable

from .assemble import assemble
from .build import build_assembled
from .errors import DocsError
from .gen_contracts import check_or_generate
from .links import run_links
from .lint import run_lint
from .search_smoke import run_search_smoke
from .sandbox import resolve_bob_binary
from .tutorial import run_tutorial
from .urls import run_urls


def _run_unit_tests(root: Path, *, bob_bin: Path | None = None) -> int:
    command = ["uv", "run", "--project", "tools/docs", "pytest", "tools/docs/tests"]
    print("running: " + " ".join(command))
    env = None
    if bob_bin is not None:
        env = dict(os.environ)
        env["BOB_BIN"] = str(bob_bin)
    try:
        return subprocess.run(command, cwd=root, env=env, check=False).returncode
    except OSError as error:
        raise DocsError(f"could not start documentation unit tests: {error}") from error


def _gate(label: str, run: Callable[[], int]) -> int:
    print(f"\ndocs-check: {label}", flush=True)
    status = run()
    if status:
        print(f"docs-check: FAILED ({label}, exit {status})")
        return status
    print(f"docs-check: passed ({label})")
    return 0


def _report_unimplemented_gates() -> None:
    """Name planned gates that later docs phases have not installed yet."""

    print("\ndocs-check: follow-on gates")
    print("skipped: CLI reference golden is a cargo test, not a docs-check gate")
    print("skipped: media checks not implemented in this slice")


def run_check(root: Path, *, production: bool = False, sources: Path | str | None = None) -> int:
    """Run all implemented documentation gates against one assembled source context."""

    repo_root = Path(root).resolve()
    print("docs-check: assemble and validate source context", flush=True)
    context, manifest = assemble(repo_root, production=production, sources=sources)

    tutorial_binary: Path | None = None

    def tutorial_gate() -> int:
        nonlocal tutorial_binary
        if "BOB_BIN" not in os.environ and shutil.which("cargo") is None:
            print("skipped: tutorial needs Cargo or BOB_BIN pointing to an executable Bob")
            return 0
        tutorial_binary = resolve_bob_binary(repo_root)
        return run_tutorial(repo_root, bob_bin=tutorial_binary)

    def unit_tests() -> int:
        return _run_unit_tests(repo_root, bob_bin=tutorial_binary)

    gates: list[tuple[str, Callable[[], int]]] = [
        ("lint", lambda: run_lint(repo_root, docs_context=context)),
        ("generated drift", lambda: check_or_generate(repo_root, check=True)),
        ("strict Zensical build", lambda: build_assembled(context, manifest)),
        ("published URLs", lambda: run_urls(repo_root, context=context)),
        ("offline lychee with fragments", lambda: run_links(repo_root, context=context)),
        ("search smoke", lambda: run_search_smoke(repo_root, context=context)),
        ("executable first-day tutorial", tutorial_gate),
        ("documentation unit tests", unit_tests),
    ]
    for label, run in gates:
        status = _gate(label, run)
        if status:
            return status
    _report_unimplemented_gates()
    print("\ndocs-check: all implemented gates passed")
    return 0
