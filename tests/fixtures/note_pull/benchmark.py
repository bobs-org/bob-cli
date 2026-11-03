#!/usr/bin/env python3
"""Measure release note-pull latency on an isolated Markdown-only vault copy."""

from __future__ import annotations

import argparse
import json
import math
import os
import platform
import re
import shutil
import statistics
import subprocess
import tempfile
import time
from pathlib import Path


NOW = "2026-11-03 09:30:00"
MISSING_CONFIG = "/definitely/missing/bob-cli-note-pull-config.yml"
TASK_SETTINGS = Path(".obsidian/plugins/obsidian-tasks-plugin/data.json")
TYPE_LINE = re.compile(r'(?m)^type:\s*["\']?\[\[(area|project)\]\]')


def root_dir() -> Path:
    return Path(__file__).resolve().parents[3]


def copy_markdown_vault(source: Path, destination: Path) -> tuple[int, int]:
    markdown_count = 0
    dataset_bytes = 0
    for path in source.rglob("*.md"):
        relative = path.relative_to(source)
        if ".git" in relative.parts or path.is_symlink():
            continue
        target = destination / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(path, target)
        markdown_count += 1
        dataset_bytes += path.stat().st_size

    settings = source / TASK_SETTINGS
    if settings.is_file():
        target = destination / TASK_SETTINGS
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(settings, target)
        dataset_bytes += settings.stat().st_size
    return markdown_count, dataset_bytes


def largest_routable_note(vault: Path) -> tuple[Path, int]:
    candidates: list[tuple[int, Path]] = []
    for path in vault.glob("*.md"):
        try:
            contents = path.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        if TYPE_LINE.search(contents[:2048]):
            candidates.append((path.stat().st_size, path))
    if not candidates:
        raise RuntimeError("the isolated vault has no root-level area or project note")
    size, note = max(candidates, key=lambda item: item[0])
    return note, size


def measure(bob: Path, vault: Path, day: Path, route: str) -> list[float]:
    command = [
        str(bob), "capture", "--dry-run", "-f", "json", "-b", str(vault),
        "--", f"@{route}",
    ]
    environment = os.environ.copy()
    environment.update({
        "BOB_CONFIG_FILE": MISSING_CONFIG,
        "BOB_DAY_FILE": str(day),
        "BOB_DIR": str(vault),
        "BOB_NOW": NOW,
        "BOB_REF_JOBS_KICK": "off",
        "BOB_VAULT_SYNC_NOTIFY": "0",
        "NO_COLOR": "1",
        "TZ": "UTC0",
    })

    def run() -> tuple[float, int]:
        start = time.perf_counter()
        result = subprocess.run(command, env=environment, capture_output=True, check=False)
        elapsed_ms = (time.perf_counter() - start) * 1000
        try:
            payload = json.loads(result.stdout)
        except (json.JSONDecodeError, UnicodeDecodeError) as error:
            raise RuntimeError(
                f"{command!r} returned invalid JSON: {result.stdout!r}"
            ) from error
        if result.returncode == 0 and payload.get("ok") is True:
            outcome = 0
        elif result.returncode == 2 and payload.get("code") == "nothing_to_pull_in":
            outcome = 2
        else:
            raise RuntimeError(
                f"{command!r} exited {result.returncode}\n"
                f"stdout:\n{result.stdout.decode(errors='replace')}\n"
                f"stderr:\n{result.stderr.decode(errors='replace')}"
            )
        return elapsed_ms, outcome

    _, warmup_code = run()
    samples: list[float] = []
    codes: dict[int, int] = {0: 0, 2: 0}
    codes[warmup_code] += 1
    for _ in range(20):
        elapsed_ms, code = run()
        samples.append(elapsed_ms)
        codes[code] += 1
    samples.sort()
    p95 = samples[math.ceil(0.95 * len(samples)) - 1]
    route_stats[route] = (statistics.median(samples), p95, max(samples), codes)
    return samples


route_stats: dict[str, tuple[float, float, float, dict[int, int]]] = {}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--vault-source", type=Path, required=True,
                        help="path printed by `sase repo open` for the Bob vault")
    parser.add_argument("--bob", type=Path,
                        default=Path(os.environ.get("CARGO_TARGET_DIR", root_dir() / "target"))
                        / "release" / "bob",
                        help="release executable built from this checkout")
    parser.add_argument("--output", type=Path,
                        default=root_dir() / "tests/fixtures/note_pull/performance.md")
    args = parser.parse_args()
    source = args.vault_source.resolve()
    bob = args.bob.resolve()
    output = args.output.resolve()
    if not source.is_dir():
        parser.error(f"vault source is not a directory: {source}")
    if not bob.is_file():
        parser.error(f"release executable does not exist: {bob}")

    target_dir = Path(os.environ.get("CARGO_TARGET_DIR", root_dir() / "target"))
    target_dir.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="note-pull-release-vault-",
                                     dir=target_dir) as temporary:
        vault = Path(temporary) / "vault"
        vault.mkdir()
        markdown_count, dataset_bytes = copy_markdown_vault(source, vault)
        largest, largest_size = largest_routable_note(vault)
        inbox = vault / "mac_inbox.md"
        if not inbox.is_file():
            raise RuntimeError("the isolated vault has no root-level mac_inbox.md")
        inbox_size = inbox.stat().st_size

        # The synthetic day lives only in the disposable copy. Bob still gets
        # an explicit fixed day path for every measured invocation.
        day = vault / "20261103.md"
        if day.exists():
            dataset_bytes -= day.stat().st_size
        else:
            markdown_count += 1
        day.write_text("## Pomodoros\n- [ ] () — WORK\n", encoding="utf-8")
        dataset_bytes += day.stat().st_size
        route_stats.clear()
        measure(bob, vault, day, largest.stem)
        measure(bob, vault, day, "mac_inbox")

    revision = subprocess.check_output(
        ["git", "rev-parse", "--short", "HEAD"], cwd=root_dir(), text=True
    ).strip()
    dirty = bool(subprocess.check_output(
        ["git", "status", "--porcelain"], cwd=root_dir(), text=True
    ).strip())
    target_ms = 70.0
    result_summary = "; ".join(
        f"`{route}` {'meets' if p95 <= target_ms else 'misses'} the target ({p95:.2f} ms p95)"
        for route, (_, p95, _, _) in route_stats.items()
    )
    output.write_text(
        "\n".join([
            "# Note-pull release latency",
            "",
            f"- Source revision: `{revision}` (verification working-tree changes included: {'yes' if dirty else 'no'})",
            f"- Build: `cargo build --release --locked --bin bob` (`{bob.name}`)",
            f"- Host: `{platform.node()}` ({platform.system()} {platform.release()})",
            f"- Isolated dataset: {markdown_count:,} Markdown notes, {dataset_bytes:,} bytes including the Tasks settings and synthetic day note",
            f"- Largest root area/project note: `{largest.name}` ({largest_size:,} bytes)",
            f"- Inbox route: `mac_inbox.md` ({inbox_size:,} bytes)",
            f"- Command: `bob capture --dry-run -f json -b <isolated-vault> -- @<route>`",
            f"- Clock/day: `BOB_NOW={NOW}`, `BOB_DAY_FILE=<isolated-vault>/20261103.md`, `TZ=UTC0`",
            "- Sampling: one warm-up and 20 timed samples per route; timings include process startup and JSON output.",
            f"- Acceptance: p95 at most {target_ms:.0f} ms per route; {result_summary}.",
            "",
            "| Route | Exit codes (warm-up + samples) | Median | p95 | Maximum |",
            "| --- | --- | ---: | ---: | ---: |",
            *[
            f"| `{route}` | {codes[0]} success / {codes[2]} empty-pull refusals | {median:.2f} ms | {p95:.2f} ms | {maximum:.2f} ms |"
                for route, (median, p95, maximum, codes) in route_stats.items()
            ],
            "",
        ]),
        encoding="utf-8",
    )


if __name__ == "__main__":
    main()
