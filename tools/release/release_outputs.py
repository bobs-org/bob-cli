#!/usr/bin/env python3
"""Parse `release-plz release -o json` and emit GitHub Actions outputs.

release-plz prints a JSON object with a `releases` list. That object may
follow log lines; this helper takes the last line that starts with `{`.
When the named package shipped, it writes `tag` and `version`. When the
list is empty or holds only other packages, both outputs are empty.

Stdlib-only (Python >= 3.11). Tests inject files and env paths, so they
are offline and deterministic.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

DEFAULT_PACKAGE = "bob-obsidian"


class ReleaseOutputsError(ValueError):
    """release-plz output is missing or not the expected JSON object."""


def last_json_object(text: str) -> object:
    """Return the last JSON value from a line that starts with `{`."""
    candidates = [
        line.strip()
        for line in text.splitlines()
        if line.strip().startswith("{")
    ]
    if not candidates:
        raise ReleaseOutputsError("no JSON object found in release-plz output")
    try:
        return json.loads(candidates[-1])
    except json.JSONDecodeError as exc:
        raise ReleaseOutputsError(
            f"malformed JSON in release-plz output: {exc}"
        ) from exc


def select_release(payload: object, package_name: str) -> tuple[str, str]:
    """Return `(tag, version)` for `package_name`, or empty strings.

    An empty `releases` list, or a list with no matching package, is "nothing
    shipped". A matching entry that lacks `tag` or `version` is malformed.
    """
    if not isinstance(payload, dict):
        raise ReleaseOutputsError("release-plz JSON must be an object")
    releases = payload.get("releases")
    if not isinstance(releases, list):
        raise ReleaseOutputsError("release-plz JSON is missing a releases list")
    for entry in releases:
        if not isinstance(entry, dict):
            continue
        if entry.get("package_name") != package_name:
            continue
        tag = entry.get("tag")
        version = entry.get("version")
        if not isinstance(tag, str) or not tag.strip():
            raise ReleaseOutputsError(
                f"release for {package_name!r} is missing a tag"
            )
        if not isinstance(version, str) or not version.strip():
            raise ReleaseOutputsError(
                f"release for {package_name!r} is missing a version"
            )
        return tag.strip(), version.strip()
    return "", ""


def format_summary(*, tag: str, version: str, mode: str) -> str:
    """Render the Markdown written to `$GITHUB_STEP_SUMMARY`."""
    if not tag:
        return "No release: release-plz found nothing new to release\n"
    lines = [
        f"### Release: {tag}",
        "",
        f"- version: `{version}`",
        f"- tag: `{tag}`",
        f"- mode: `{mode}`",
        "",
    ]
    return "\n".join(lines)


def append_output(name: str, value: str) -> None:
    """Append one `name=value` line to `$GITHUB_OUTPUT` (or stdout locally)."""
    path = os.environ.get("GITHUB_OUTPUT")
    line = f"{name}={value}\n"
    if path:
        with open(path, "a", encoding="utf-8") as handle:
            handle.write(line)
    else:
        sys.stdout.write(line)


def append_summary(markdown: str) -> None:
    """Append Markdown to `$GITHUB_STEP_SUMMARY` (or stdout locally)."""
    path = os.environ.get("GITHUB_STEP_SUMMARY")
    if path:
        with open(path, "a", encoding="utf-8") as handle:
            handle.write(markdown)
    else:
        sys.stdout.write(markdown)


def parse_file(path: Path, package_name: str) -> tuple[str, str]:
    """Read `path` and return `(tag, version)` for `package_name`."""
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as exc:
        raise ReleaseOutputsError(
            f"could not read release-plz output {path}: {exc}"
        ) from exc
    if not text.strip():
        raise ReleaseOutputsError("release-plz output is empty")
    return select_release(last_json_object(text), package_name)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "json_path",
        help="path to the captured `release-plz release -o json` stdout",
    )
    parser.add_argument(
        "--package",
        default=DEFAULT_PACKAGE,
        help=f"package_name to select (default {DEFAULT_PACKAGE})",
    )
    parser.add_argument(
        "--mode",
        default=os.environ.get("MODE", ""),
        help="train mode for the step summary (rehearsal or live)",
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    path = Path(args.json_path)
    try:
        tag, version = parse_file(path, args.package)
    except ReleaseOutputsError as exc:
        sys.stderr.write(f"{exc}\n")
        return 1
    append_output("tag", tag)
    append_output("version", version)
    summary = format_summary(tag=tag, version=version, mode=args.mode)
    append_summary(summary)
    if tag:
        sys.stdout.write(f"released tag={tag} version={version}\n")
    else:
        sys.stdout.write("no release\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
