#!/usr/bin/env python3
"""Read workspace package versions and format release-train labels.

bob-obsidian and bob-obsidian-assets version independently. Tag names come
from release-plz.toml so preview, the bump commit, and tests stay aligned
with the train config.
"""

from __future__ import annotations

import argparse
import json
import sys
import tomllib
from pathlib import Path

ROOT_PACKAGE = "bob-obsidian"
ASSETS_PACKAGE = "bob-obsidian-assets"
PACKAGE_ORDER = (ROOT_PACKAGE, ASSETS_PACKAGE)

PACKAGE_MANIFESTS = {
    ROOT_PACKAGE: Path("Cargo.toml"),
    ASSETS_PACKAGE: Path("crates/bob-obsidian-assets/Cargo.toml"),
}

CHANGELOG_PATHS = (
    Path("CHANGELOG.md"),
    Path("crates/bob-obsidian-assets/CHANGELOG.md"),
)

DEFAULT_TAG_TEMPLATE = "v{{ version }}"


def load_release_plz_config(root: Path) -> dict:
    """Parse `release-plz.toml` from `root`."""
    path = root / "release-plz.toml"
    try:
        data = tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as exc:
        raise ValueError(f"could not read {path}: {exc}") from exc
    if not isinstance(data, dict):
        raise ValueError(f"{path} is not a TOML table")
    return data


def package_tag_template(config: dict, package_name: str) -> str:
    """Return the release-plz tag template for `package_name`."""
    packages = config.get("package")
    if isinstance(packages, list):
        for entry in packages:
            if not isinstance(entry, dict):
                continue
            if entry.get("name") == package_name:
                template = entry.get("git_tag_name")
                if isinstance(template, str) and template.strip():
                    return template.strip()
                break
    workspace = config.get("workspace")
    if isinstance(workspace, dict):
        template = workspace.get("git_tag_name")
        if isinstance(template, str) and template.strip():
            return template.strip()
    return DEFAULT_TAG_TEMPLATE


def render_tag(template: str, version: str) -> str:
    """Render a release-plz Tera tag template with `version`."""
    return (
        template.replace("{{ version }}", version)
        .replace("{{version}}", version)
        .replace("{{ version}}", version)
        .replace("{{version }}", version)
    )


def tag_for(package_name: str, version: str, config: dict) -> str:
    """Return the git tag release-plz would create for this package version."""
    return render_tag(package_tag_template(config, package_name), version)


def read_package_version(root: Path, package_name: str) -> str:
    """Return the `[package].version` for a workspace member."""
    relative = PACKAGE_MANIFESTS.get(package_name)
    if relative is None:
        raise ValueError(f"unknown workspace package {package_name!r}")
    path = root / relative
    try:
        data = tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as exc:
        raise ValueError(f"could not read {path}: {exc}") from exc
    version = data.get("package", {}).get("version")
    if not isinstance(version, str) or not version.strip():
        raise ValueError(f"{path} is missing [package].version")
    return version.strip()


def read_versions(root: Path) -> dict[str, str]:
    """Return `{package_name: version}` for every train package, in order."""
    return {name: read_package_version(root, name) for name in PACKAGE_ORDER}


def write_versions_json(path: Path, versions: dict[str, str]) -> None:
    """Write a versions snapshot as JSON."""
    path.write_text(json.dumps(versions, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def load_versions_json(path: Path) -> dict[str, str]:
    """Read a versions snapshot written by `write_versions_json`."""
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise ValueError(f"could not read versions snapshot {path}: {exc}") from exc
    if not isinstance(data, dict):
        raise ValueError(f"versions snapshot {path} is not an object")
    versions: dict[str, str] = {}
    for name in PACKAGE_ORDER:
        value = data.get(name)
        if not isinstance(value, str) or not value.strip():
            raise ValueError(f"versions snapshot {path} is missing {name}")
        versions[name] = value.strip()
    return versions


def commit_subject(before: dict[str, str], after: dict[str, str], config: dict) -> str:
    """Return the `chore(release):` subject for packages whose version changed."""
    changed_tags = [
        tag_for(name, after[name], config)
        for name in PACKAGE_ORDER
        if before.get(name) != after[name]
    ]
    if not changed_tags:
        return "chore(release): changelog"
    return "chore(release): " + ", ".join(changed_tags)


def format_no_release(versions: dict[str, str], config: dict) -> str:
    """One line for a clean `release-plz update` (nothing to ship)."""
    parts = [tag_for(name, versions[name], config) for name in PACKAGE_ORDER]
    return "no release: nothing user-visible since " + ", ".join(parts) + "\n"


def format_next_versions(before: dict[str, str], after: dict[str, str], config: dict) -> str:
    """Report each package's version after `release-plz update`."""
    lines = ["next versions:"]
    for name in PACKAGE_ORDER:
        old = before.get(name, "?")
        new = after[name]
        if old == new:
            lines.append(f"- {name}: {new} (unchanged)")
        else:
            lines.append(f"- {name}: {old} -> {new} (tag {tag_for(name, new, config)})")
    return "\n".join(lines) + "\n"


def package_git_release_enable(config: dict, package_name: str) -> bool:
    """Return whether this package creates a GitHub Release (default True)."""
    packages = config.get("package")
    if isinstance(packages, list):
        for entry in packages:
            if not isinstance(entry, dict):
                continue
            if entry.get("name") == package_name:
                enabled = entry.get("git_release_enable")
                if isinstance(enabled, bool):
                    return enabled
                break
    workspace = config.get("workspace")
    if isinstance(workspace, dict) and isinstance(workspace.get("git_release_enable"), bool):
        return workspace["git_release_enable"]
    return True


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--root",
        default=".",
        help="workspace root containing Cargo.toml and release-plz.toml",
    )
    parser.add_argument(
        "--write-json",
        metavar="PATH",
        help="write the current package versions as JSON",
    )
    parser.add_argument(
        "--before",
        metavar="PATH",
        help="previous versions JSON from --write-json",
    )
    parser.add_argument(
        "--commit-subject",
        action="store_true",
        help="print the chore(release) subject for versions that changed",
    )
    parser.add_argument(
        "--preview",
        action="store_true",
        help="print next-version lines (requires --before)",
    )
    parser.add_argument(
        "--no-release",
        action="store_true",
        help="print the no-release line for the current versions",
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    root = Path(args.root).resolve()
    try:
        config = load_release_plz_config(root)
        after = read_versions(root)
        if args.write_json:
            write_versions_json(Path(args.write_json), after)
        if args.commit_subject:
            if not args.before:
                raise ValueError("--commit-subject requires --before")
            before = load_versions_json(Path(args.before))
            sys.stdout.write(commit_subject(before, after, config) + "\n")
        if args.preview:
            if not args.before:
                raise ValueError("--preview requires --before")
            before = load_versions_json(Path(args.before))
            sys.stdout.write(format_next_versions(before, after, config))
        if args.no_release:
            sys.stdout.write(format_no_release(after, config))
        if not (args.write_json or args.commit_subject or args.preview or args.no_release):
            for name in PACKAGE_ORDER:
                sys.stdout.write(f"{name} {after[name]} {tag_for(name, after[name], config)}\n")
    except ValueError as exc:
        sys.stderr.write(f"{exc}\n")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
