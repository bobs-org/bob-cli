#!/usr/bin/env python3
"""Build and enforce Bob's crates.io package contents and size budget."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path, PurePosixPath
import subprocess
import sys
import tarfile


ROOT = Path(__file__).resolve().parents[2]
CAP_BYTES = 10 * 1024 * 1024
WARN_BYTES = 8 * 1024 * 1024
FAIL_BYTES = 9 * 1024 * 1024
FORBIDDEN_DIRS = ("docs", "tests", "tools", "vault", "sase", ".github")
REQUIRED_NOTICES = (
    "LICENSE",
    "assets/print_fonts/OFL.txt",
    "scripts/web_clip/fonts/OFL.txt",
    "scripts/web_clip/fonts/DEJAVU_LICENSE.txt",
    "scripts/web_clip/vendor/DEFUDDLE_LICENSE",
    "scripts/web_clip/vendor/MERMAID_LICENSE",
    "src/native/dataview/tasks/vendor/MOMENT_LICENSE",
)


def normalize_package_path(value: str) -> str:
    """Normalize Cargo's package-list paths for comparisons."""
    value = value.strip().replace("\\", "/")
    while value.startswith("./") or value.startswith("/"):
        value = value[2:] if value.startswith("./") else value[1:]
    return value


def package_listing_errors(paths: list[str]) -> list[str]:
    normalized = {normalize_package_path(path) for path in paths if path.strip()}
    errors: list[str] = []
    for path in sorted(normalized):
        for directory in FORBIDDEN_DIRS:
            if path == directory or path.startswith(directory + "/"):
                errors.append(f"forbidden package path included: {path}")
                break
    for required in REQUIRED_NOTICES:
        if required not in normalized:
            errors.append(f"required license or notice missing from package: {required}")
    return errors


def size_level(size_bytes: int) -> str:
    if size_bytes > FAIL_BYTES:
        return "error"
    if size_bytes > WARN_BYTES:
        return "warning"
    return "ok"


def largest_contributors(crate_path: Path, limit: int = 10) -> list[tuple[str, int]]:
    """Return top-level package directories by their uncompressed file bytes."""
    totals: dict[str, int] = {}
    with tarfile.open(crate_path, mode="r:gz") as archive:
        for member in archive.getmembers():
            if not member.isfile():
                continue
            parts = PurePosixPath(member.name).parts
            if not parts:
                continue
            directory = parts[1] if len(parts) > 2 else "<root metadata>"
            totals[directory] = totals.get(directory, 0) + member.size
    return sorted(totals.items(), key=lambda item: (-item[1], item[0]))[:limit]


def _run(command: list[str], *, capture: bool = False) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        command,
        cwd=ROOT,
        check=False,
        text=True,
        capture_output=capture,
    )


def _current_package() -> tuple[str, str, Path] | None:
    result = _run(
        ["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"],
        capture=True,
    )
    if result.returncode != 0:
        if result.stdout:
            print(result.stdout, end="", file=sys.stderr)
        if result.stderr:
            print(result.stderr, end="", file=sys.stderr)
        print("Could not read Cargo metadata.", file=sys.stderr)
        return None
    try:
        metadata = json.loads(result.stdout)
        manifest = (ROOT / "Cargo.toml").resolve()
        package = next(
            item
            for item in metadata["packages"]
            if Path(item["manifest_path"]).resolve() == manifest
        )
        return package["name"], package["version"], Path(metadata["target_directory"])
    except (KeyError, StopIteration, TypeError, json.JSONDecodeError) as error:
        print(f"Cargo metadata did not describe the root package: {error}", file=sys.stderr)
        return None


def _print_contributors(crate_path: Path) -> None:
    print("Largest packaged directories (uncompressed bytes):")
    try:
        contributors = largest_contributors(crate_path)
    except (OSError, tarfile.TarError) as error:
        print(f"  Could not inspect package contents: {error}")
        return
    for directory, size_bytes in contributors:
        print(f"  {size_bytes:>10,}  {directory}")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--no-verify",
        action="store_true",
        help="skip cargo package's build verification for a faster local size check",
    )
    args = parser.parse_args(argv)

    command = ["cargo", "package", "--locked", "--allow-dirty"]
    if args.no_verify:
        command.append("--no-verify")
    package_result = _run(command)
    if package_result.returncode != 0:
        return package_result.returncode

    listing_result = _run(
        ["cargo", "package", "--locked", "--allow-dirty", "--list"],
        capture=True,
    )
    if listing_result.returncode != 0:
        if listing_result.stdout:
            print(listing_result.stdout, end="", file=sys.stderr)
        if listing_result.stderr:
            print(listing_result.stderr, end="", file=sys.stderr)
        print("Could not read the Cargo package file list.", file=sys.stderr)
        return listing_result.returncode

    package = _current_package()
    if package is None:
        return 1
    name, version, target_directory = package
    crate_path = target_directory / "package" / f"{name}-{version}.crate"
    if not crate_path.is_file():
        print(f"Cargo package archive was not found: {crate_path}", file=sys.stderr)
        return 1

    paths = listing_result.stdout.splitlines()
    errors = package_listing_errors(paths)
    if errors:
        for error in errors:
            print(f"ERROR: {error}", file=sys.stderr)

    size_bytes = crate_path.stat().st_size
    percent = size_bytes / CAP_BYTES * 100
    mib = size_bytes / (1024 * 1024)
    print(f"Package: {name} {version}")
    print(f"Archive: {crate_path}")
    print(f"Size: {size_bytes:,} bytes ({mib:.2f} MiB; {percent:.1f}% of {CAP_BYTES:,}-byte cap)")

    level = size_level(size_bytes)
    if level == "warning":
        message = f"package is above the 8 MiB warning threshold ({size_bytes:,} bytes)"
        if os.environ.get("GITHUB_ACTIONS"):
            print(f"::warning::{message}")
        else:
            print(f"WARNING: {message}")
    elif level == "error":
        errors.append(f"package exceeds the 9 MiB failure threshold ({size_bytes:,} bytes)")
        print(f"ERROR: {errors[-1]}", file=sys.stderr)

    if errors or level in ("warning", "error"):
        _print_contributors(crate_path)
    return 1 if errors else 0


if __name__ == "__main__":
    raise SystemExit(main())
