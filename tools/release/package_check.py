#!/usr/bin/env python3
"""Build and enforce Bob's crates.io package contents and size budget."""

from __future__ import annotations

import argparse
import json
import os
import re
from pathlib import Path, PurePosixPath
import subprocess
import sys
import tarfile


ROOT = Path(__file__).resolve().parents[2]
CAP_BYTES = 10 * 1024 * 1024
WARN_BYTES = 8 * 1024 * 1024
FAIL_BYTES = 9 * 1024 * 1024
FORBIDDEN_DIRS = ("docs", "tests", "tools", "vault", "sase", ".github")
# Publishable workspace members in packaging order.
EXPECTED_PACKAGES = ("bob-obsidian", "bob-obsidian-assets")
ROOT_REQUIRED_NOTICES = (
    "LICENSE",
    "src/native/dataview/tasks/vendor/MOMENT_LICENSE",
)
ASSETS_REQUIRED_NOTICES = (
    "LICENSE",
    "assets/print_fonts/OFL.txt",
    "scripts/web_clip/fonts/OFL.txt",
    "scripts/web_clip/fonts/DEJAVU_LICENSE.txt",
    "scripts/web_clip/vendor/DEFUDDLE_LICENSE",
    "scripts/web_clip/vendor/MERMAID_LICENSE",
)
# Union of every required notice; kept for backward compatibility.
REQUIRED_NOTICES = tuple(
    dict.fromkeys((*ROOT_REQUIRED_NOTICES, *ASSETS_REQUIRED_NOTICES))
)
# Payload trees owned by the assets crate; the root archive must not contain them.
MOVED_PAYLOAD_PREFIXES = (
    "assets/print_fonts/",
    "scripts/web_clip/fonts/",
    "scripts/web_clip/vendor/",
)
REQUIRED_NOTICES_BY_PACKAGE = {
    "bob-obsidian": ROOT_REQUIRED_NOTICES,
    "bob-obsidian-assets": ASSETS_REQUIRED_NOTICES,
}


def normalize_package_path(value: str) -> str:
    """Normalize Cargo's package-list paths for comparisons."""
    value = value.strip().replace("\\", "/")
    while value.startswith("./") or value.startswith("/"):
        value = value[2:] if value.startswith("./") else value[1:]
    return value


def package_listing_errors(
    paths: list[str],
    required_notices: tuple[str, ...] = REQUIRED_NOTICES,
) -> list[str]:
    normalized = {normalize_package_path(path) for path in paths if path.strip()}
    errors: list[str] = []
    for path in sorted(normalized):
        for directory in FORBIDDEN_DIRS:
            if path == directory or path.startswith(directory + "/"):
                errors.append(f"forbidden package path included: {path}")
                break
    for required in required_notices:
        if required not in normalized:
            errors.append(f"required license or notice missing from package: {required}")
    return errors


def workspace_payload_errors(package_name: str, paths: list[str]) -> list[str]:
    """Enforce payload ownership between the root and assets archives."""
    normalized = [normalize_package_path(p) for p in paths if p.strip()]
    errors: list[str] = []
    if package_name == "bob-obsidian":
        for path in sorted(normalized):
            for prefix in MOVED_PAYLOAD_PREFIXES:
                if path.startswith(prefix):
                    errors.append(
                        f"moved payload tree must live in bob-obsidian-assets: {path}"
                    )
                    break
    elif package_name == "bob-obsidian-assets":
        for prefix in MOVED_PAYLOAD_PREFIXES:
            if not any(path.startswith(prefix) for path in normalized):
                errors.append(
                    f"assets package missing payload tree: {prefix}"
                )
    return errors


_INCLUDE_BYTES_RE = re.compile(r'include_bytes!\s*\(\s*"([^"]+)"\s*\)')


def root_include_bytes_inputs() -> list[str]:
    """Collect root `include_bytes!` file inputs as package-relative paths."""
    inputs: list[str] = []
    for source in sorted((ROOT / "src").rglob("*.rs")):
        try:
            text = source.read_text(encoding="utf-8")
        except OSError:
            continue
        for match in _INCLUDE_BYTES_RE.finditer(text):
            candidate = (source.parent / match.group(1)).resolve()
            try:
                rel = candidate.relative_to(ROOT.resolve()).as_posix()
            except ValueError:
                continue
            inputs.append(rel)
    return sorted(set(inputs))


def root_include_bytes_errors(paths: list[str]) -> list[str]:
    """Ensure every root `include_bytes!` input is still packaged."""
    normalized = {normalize_package_path(p) for p in paths if p.strip()}
    errors: list[str] = []
    for rel in root_include_bytes_inputs():
        if (ROOT / rel).is_file() and rel not in normalized:
            errors.append(f"root include_bytes input missing from package: {rel}")
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


def _workspace_packages() -> dict[str, tuple[str, Path]] | None:
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
        target_directory = Path(metadata["target_directory"])
        packages: dict[str, tuple[str, Path]] = {}
        for item in metadata["packages"]:
            if item.get("publish") is False:
                continue
            packages[item["name"]] = (item["version"], target_directory)
        return packages
    except (KeyError, TypeError, json.JSONDecodeError) as error:
        print(f"Cargo metadata did not describe workspace packages: {error}", file=sys.stderr)
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

    packages = _workspace_packages()
    if packages is None:
        return 1
    missing = [name for name in EXPECTED_PACKAGES if name not in packages]
    if missing:
        for name in missing:
            print(f"ERROR: expected workspace package missing: {name}", file=sys.stderr)
        return 1

    command = ["cargo", "package", "--workspace", "--locked", "--allow-dirty"]
    if args.no_verify:
        command.append("--no-verify")
    package_result = _run(command)
    if package_result.returncode != 0:
        return package_result.returncode

    overall_errors: list[str] = []
    exit_code = 0
    for name in EXPECTED_PACKAGES:
        version, target_directory = packages[name]
        listing_result = _run(
            ["cargo", "package", "--locked", "--allow-dirty", "--list", "-p", name],
            capture=True,
        )
        if listing_result.returncode != 0:
            if listing_result.stdout:
                print(listing_result.stdout, end="", file=sys.stderr)
            if listing_result.stderr:
                print(listing_result.stderr, end="", file=sys.stderr)
            print(f"Could not read the Cargo package file list for {name}.", file=sys.stderr)
            return listing_result.returncode

        crate_path = target_directory / "package" / f"{name}-{version}.crate"
        if not crate_path.is_file():
            print(f"Cargo package archive was not found: {crate_path}", file=sys.stderr)
            return 1

        paths = listing_result.stdout.splitlines()
        required = REQUIRED_NOTICES_BY_PACKAGE[name]
        errors = package_listing_errors(paths, required)
        errors.extend(workspace_payload_errors(name, paths))
        if name == "bob-obsidian":
            errors.extend(root_include_bytes_errors(paths))
        if errors:
            for error in errors:
                print(f"ERROR [{name}]: {error}", file=sys.stderr)

        size_bytes = crate_path.stat().st_size
        percent = size_bytes / CAP_BYTES * 100
        mib = size_bytes / (1024 * 1024)
        print(f"Package: {name} {version}")
        print(f"Archive: {crate_path}")
        print(f"Size: {size_bytes:,} bytes ({mib:.2f} MiB; {percent:.1f}% of {CAP_BYTES:,}-byte cap)")

        level = size_level(size_bytes)
        if level == "warning":
            message = f"{name} is above the 8 MiB warning threshold ({size_bytes:,} bytes)"
            if os.environ.get("GITHUB_ACTIONS"):
                print(f"::warning::{message}")
            else:
                print(f"WARNING: {message}")
        elif level == "error":
            errors.append(f"{name} exceeds the 9 MiB failure threshold ({size_bytes:,} bytes)")
            print(f"ERROR [{name}]: {errors[-1]}", file=sys.stderr)

        if errors or level in ("warning", "error"):
            _print_contributors(crate_path)
        if errors:
            overall_errors.extend(f"[{name}] {e}" for e in errors)
            exit_code = 1
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
