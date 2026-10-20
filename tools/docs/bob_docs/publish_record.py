"""Prepare the durable publication record after a successful deploy.

The planner writes a transient ``next-publication.json`` candidate holding
the schema version, hold flag, immutable source identities, input records,
and fingerprint. After the gated site deploys, this module attaches the
build metadata (toolchain versions, build time, run URL) and writes the
durable ``published.json`` manifest. The hold flag is preserved verbatim;
only a manifest commit records a publication, never this module alone.
"""

from __future__ import annotations

import json
from datetime import datetime, timezone
from importlib.metadata import PackageNotFoundError, version
from pathlib import Path
from typing import Any

from .errors import DocsError
from .publish_plan import SCHEMA_VERSION, fingerprint_inputs

BOB_DOCS_VERSION_FALLBACK = "0.1.0"


def collect_tool_versions() -> dict[str, str]:
    """Collect the toolchain versions stamped into the publication record."""

    import sys

    try:
        bob_docs = version("bob-docs")
    except PackageNotFoundError:
        bob_docs = BOB_DOCS_VERSION_FALLBACK
    tools = {
        "bob-docs": bob_docs,
        "python": ".".join(map(str, sys.version_info[:3])),
    }
    try:
        tools["zensical"] = version("zensical")
    except PackageNotFoundError:
        tools["zensical"] = "unknown"
    return tools


def parse_tool_assignments(assignments: list[str]) -> dict[str, str]:
    """Parse ``--tool KEY=VALUE`` assignments into a version mapping."""

    tools: dict[str, str] = {}
    for assignment in assignments:
        name, separator, value = assignment.partition("=")
        if not separator or not name.strip() or not value.strip():
            raise DocsError(
                f"--tool must be KEY=VALUE with nonempty key and value, got {assignment!r}"
            )
        tools[name.strip()] = value.strip()
    return tools


def _require_candidate_shape(candidate: Any, *, source: str) -> dict[str, Any]:
    if not isinstance(candidate, dict):
        raise DocsError(f"publication candidate {source} must be a JSON object")
    expected = {"schema_version", "hold", "sources", "inputs", "fingerprint"}
    if set(candidate) != expected:
        unknown = sorted(set(candidate) - expected)
        missing = sorted(expected - set(candidate))
        details = []
        if unknown:
            details.append(f"unknown keys: {', '.join(unknown)}")
        if missing:
            details.append(f"missing keys: {', '.join(missing)}")
        raise DocsError(
            f"publication candidate {source} has an invalid shape ({'; '.join(details)})"
        )
    if candidate["schema_version"] != SCHEMA_VERSION:
        raise DocsError(
            f"publication candidate {source} has unsupported schema_version "
            f"{candidate['schema_version']!r}; expected {SCHEMA_VERSION}"
        )
    if not isinstance(candidate.get("hold"), bool):
        raise DocsError(f"publication candidate {source}: hold must be a boolean")
    sources = candidate.get("sources")
    if not isinstance(sources, dict) or not sources:
        raise DocsError(f"publication candidate {source}: sources must be a nonempty mapping")
    for repo, value in sources.items():
        if not isinstance(value, dict) or set(value) != {"remote", "branch", "sha"}:
            raise DocsError(
                f"publication candidate {source}: sources.{repo} must contain "
                "remote, branch, and sha"
            )
    inputs = candidate.get("inputs")
    if not isinstance(inputs, list) or not inputs:
        raise DocsError(f"publication candidate {source}: inputs must be a nonempty list")
    fingerprint = candidate.get("fingerprint")
    if not isinstance(fingerprint, str) or not fingerprint:
        raise DocsError(f"publication candidate {source}: fingerprint must be a nonempty string")
    if fingerprint != fingerprint_inputs(inputs):
        raise DocsError(
            f"publication candidate {source}: fingerprint does not match its inputs"
        )
    return candidate


def build_record(
    candidate: dict[str, Any],
    *,
    source: str,
    tools: dict[str, str],
    built_at: str,
    run_url: str,
) -> dict[str, Any]:
    """Attach build metadata to a validated planner candidate."""

    checked = _require_candidate_shape(candidate, source=source)
    if not tools or not all(
        isinstance(key, str) and key and isinstance(value, str) and value
        for key, value in tools.items()
    ):
        raise DocsError("publication record tools must map names to nonempty versions")
    if not isinstance(built_at, str) or not built_at:
        raise DocsError("publication record built_at must be a timestamp")
    try:
        timestamp = datetime.fromisoformat(built_at.replace("Z", "+00:00"))
    except ValueError as error:
        raise DocsError(
            f"publication record built_at is not an ISO timestamp: {built_at!r}"
        ) from error
    if timestamp.utcoffset() is None:
        raise DocsError("publication record built_at must include a timezone")
    if not isinstance(run_url, str) or not run_url.strip():
        raise DocsError("publication record run_url must be a nonempty string")
    return {
        "schema_version": SCHEMA_VERSION,
        "hold": checked["hold"],
        "sources": checked["sources"],
        "inputs": checked["inputs"],
        "fingerprint": checked["fingerprint"],
        "tools": dict(sorted(tools.items())),
        "built_at": built_at,
        "run_url": run_url,
    }


def write_record(path: Path, record: dict[str, Any]) -> Path:
    """Atomically write the durable publication record."""

    resolved = path.resolve()
    resolved.parent.mkdir(parents=True, exist_ok=True)
    payload = json.dumps(record, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
    temporary = resolved.with_name(f".{resolved.name}.tmp")
    temporary.write_text(payload + "\n", encoding="utf-8")
    temporary.replace(resolved)
    return resolved


def current_built_at() -> str:
    """Return the current UTC build time in publication-record format."""

    return datetime.now(timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z")


def run_publish_record(
    candidate_file: Path,
    output_file: Path,
    *,
    tools: dict[str, str] | None = None,
    built_at: str | None = None,
    run_url: str,
) -> dict[str, Any]:
    """Read a planner candidate and write the durable publication record."""

    try:
        candidate = json.loads(candidate_file.read_text(encoding="utf-8"))
    except FileNotFoundError:
        raise DocsError(
            f"publication candidate {candidate_file} does not exist"
        ) from None
    except (OSError, UnicodeError, ValueError) as error:
        raise DocsError(
            f"publication candidate {candidate_file} is unreadable or invalid JSON: {error}"
        ) from error
    record = build_record(
        candidate,
        source=str(candidate_file),
        tools=tools if tools is not None else collect_tool_versions(),
        built_at=built_at if built_at is not None else current_built_at(),
        run_url=run_url,
    )
    write_record(output_file, record)
    return record
