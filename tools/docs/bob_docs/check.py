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
from .media import run_media
from .publish_drill import (
    drill_link_built,
    drill_marker_present,
    inject_drill_link,
)
from .search_smoke import run_search_smoke
from .sandbox import MISSING_BOB_RUNTIME, resolve_bob_binary
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


def run_check(
    root: Path,
    *,
    production: bool = False,
    sources: Path | str | None = None,
    drill: bool = False,
) -> int:
    """Run all implemented documentation gates against one assembled source context.

    With ``drill=True``, run the clean gates, inject a staged-only broken
    fragment link, then require the strict build and URL gates to pass and
    the link gate to fail. A drill never returns success: the expected link
    failure still exits nonzero so no workflow can deploy or record it.
    """

    repo_root = Path(root).resolve()
    print("docs-check: assemble and validate source context", flush=True)
    context, manifest = assemble(repo_root, production=production, sources=sources)
    if drill:
        return _run_drill_gates(repo_root, context, manifest, production=production)

    tutorial_binary: Path | None = None

    def tutorial_gate() -> int:
        nonlocal tutorial_binary
        if "BOB_BIN" not in os.environ and shutil.which("cargo") is None:
            print(f"skipped: tutorial {MISSING_BOB_RUNTIME}")
            return 0
        tutorial_binary = resolve_bob_binary(repo_root)
        return run_tutorial(repo_root, bob_bin=tutorial_binary)

    def media_gate() -> int:
        nonlocal tutorial_binary
        if "BOB_BIN" not in os.environ and shutil.which("cargo") is None:
            print(f"skipped: media {MISSING_BOB_RUNTIME}")
            return 0
        if tutorial_binary is None:
            tutorial_binary = resolve_bob_binary(repo_root)
        return run_media(repo_root, check=True, bob_bin=tutorial_binary)

    def unit_tests() -> int:
        return _run_unit_tests(repo_root, bob_bin=tutorial_binary)

    gates: list[tuple[str, Callable[[], int]]] = [
        ("lint", lambda: run_lint(repo_root, docs_context=context)),
        (
            "generated drift",
            lambda: check_or_generate(
                repo_root,
                check=True,
                source_dirs=context.source_dirs,
                production=production,
            ),
        ),
        ("strict Zensical build", lambda: build_assembled(context, manifest)),
        ("published URLs", lambda: run_urls(repo_root, context=context)),
        (
            "offline lychee with fragments",
            lambda: run_links(repo_root, context=context),
        ),
        ("search smoke", lambda: run_search_smoke(repo_root, context=context)),
        ("executable first-day tutorial", tutorial_gate),
        ("CLI media determinism", media_gate),
        ("documentation unit tests", unit_tests),
    ]
    for label, run in gates:
        status = _gate(label, run)
        if status:
            return status
    _report_unimplemented_gates()
    print("\ndocs-check: all implemented gates passed")
    return 0


def _run_drill_gates(
    root: Path, context, manifest: Path, *, production: bool = False
) -> int:
    """Run the clean gates, inject the staged probe, and require link failure."""

    for label, run in [
        ("lint", lambda: run_lint(root, docs_context=context)),
        (
            "generated drift",
            lambda: check_or_generate(
                root,
                check=True,
                source_dirs=context.source_dirs,
                production=production,
            ),
        ),
    ]:
        status = _gate(label, run)
        if status:
            print("drill: clean gate failed before injection; drill is inconclusive")
            return status
    staged_docs = context.output_dir / "docs"
    try:
        injected = inject_drill_link(context)
    except DocsError as error:
        print(f"drill: FAILED to inject the staged probe: {error}")
        return 2
    print(f"\ndrill: injected staged-only probe into {injected}")
    for label, run in [
        ("strict Zensical build", lambda: build_assembled(context, manifest)),
        ("published URLs", lambda: run_urls(root, context=context)),
    ]:
        status = _gate(label, run)
        if status:
            print(
                "drill: gate failed before the link check; "
                "the probe may be visible outside the link gate"
            )
            return status
    print("\ndrill: offline lychee with fragments", flush=True)
    links_status = run_links(root, context=context)
    if links_status == 0:
        if drill_link_built(context.site_dir):
            print(
                "drill: FAILED: the injected broken link unexpectedly passed "
                "the link gate; refusing deployment"
            )
        else:
            print(
                "drill: FAILED: the link gate passed and the probe is missing "
                "from the built site; the drill proved nothing"
            )
        return 1
    if drill_marker_present(staged_docs) and drill_link_built(context.site_dir):
        print(
            f"\ndrill: expected broken-link diagnostic observed "
            f"(link gate exit {links_status}); upload and deployment remain blocked"
        )
    else:
        print(
            f"\ndrill: link gate failed (exit {links_status}) but the probe is missing "
            "from the staging tree or built site; treating it as an ordinary failure"
        )
    return links_status
