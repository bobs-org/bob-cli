"""The ordered local documentation gate."""

from __future__ import annotations

import shutil
import subprocess
from pathlib import Path
from typing import Callable

from .assemble import assemble
from .build import build_assembled
from .errors import DocsError
from .gen_contracts import check_or_generate
from .links import run_links
from .lint import run_lint
from .search_smoke import run_search_smoke
from .urls import run_urls


def _run_unit_tests(root: Path) -> int:
    command = ["uv", "run", "--project", "tools/docs", "pytest", "tools/docs/tests"]
    print("running: " + " ".join(command))
    try:
        return subprocess.run(command, cwd=root, check=False).returncode
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
    print("skipped: CLI reference golden not implemented in this slice")
    print("skipped: tutorial not implemented in this slice")
    print("skipped: media checks not implemented in this slice")
    if shutil.which("cargo") is None:
        print("skipped: optional Rust runtime unavailable (cargo not found)")


def run_check(root: Path, *, production: bool = False, sources: Path | str | None = None) -> int:
    """Run all implemented documentation gates against one assembled source context."""

    repo_root = Path(root).resolve()
    print("docs-check: assemble and validate source context", flush=True)
    context, manifest = assemble(repo_root, production=production, sources=sources)

    gates: list[tuple[str, Callable[[], int]]] = [
        ("lint", lambda: run_lint(repo_root, docs_context=context)),
        ("generated drift", lambda: check_or_generate(repo_root, check=True)),
        ("strict Zensical build", lambda: build_assembled(context, manifest)),
        ("published URLs", lambda: run_urls(repo_root, context=context)),
        ("offline lychee with fragments", lambda: run_links(repo_root, context=context)),
        ("search smoke", lambda: run_search_smoke(repo_root, context=context)),
        ("documentation unit tests", lambda: _run_unit_tests(repo_root)),
    ]
    for label, run in gates:
        status = _gate(label, run)
        if status:
            return status
    _report_unimplemented_gates()
    print("\ndocs-check: all implemented gates passed")
    return 0
