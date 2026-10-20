"""Small, deterministic checks that important goals are represented in search."""

from __future__ import annotations

import json
import re
from html import unescape
from pathlib import Path
from typing import Any
from urllib.parse import unquote

from .context import DocsContext, context_from_root
from .errors import DocsError
from .yaml_tools import load_yaml

SEARCH_CASES = Path("docs/_meta/search_smoke.yml")
SEARCH_INDEX_CANDIDATES = (
    Path("search.json"),
    Path("search/search_index.json"),
    Path("search_index.json"),
)


def _load_cases(path: Path) -> list[dict[str, str]]:
    raw = load_yaml(path)
    if not isinstance(raw, list) or not raw:
        raise DocsError(f"{path}: expected a nonempty list of search smoke cases")
    cases: list[dict[str, str]] = []
    for number, item in enumerate(raw, start=1):
        if not isinstance(item, dict):
            raise DocsError(f"{path}: case {number} must be a mapping")
        query, location = item.get("query"), item.get("location")
        if not isinstance(query, str) or not query.strip():
            raise DocsError(f"{path}: case {number} query must be a nonempty string")
        if not isinstance(location, str) or not location.strip():
            raise DocsError(f"{path}: case {number} location must be a nonempty string")
        cases.append({"query": query.strip(), "location": location.strip()})
    return cases


def _index_path(site_dir: Path) -> Path:
    for relative in SEARCH_INDEX_CANDIDATES:
        candidate = site_dir / relative
        if candidate.is_file():
            return candidate
    tried = ", ".join(str(site_dir / path) for path in SEARCH_INDEX_CANDIDATES)
    raise DocsError(f"search index is missing; looked for {tried}")


def _load_index(path: Path) -> tuple[str, list[dict[str, Any]]]:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise DocsError(f"cannot read search index {path}: {error}") from error
    if not isinstance(data, dict):
        raise DocsError(f"{path}: search index must be a JSON object")
    config = data.get("config")
    if not isinstance(config, dict):
        raise DocsError(f"{path}: search index has no config object")
    separator = config.get("separator")
    if not isinstance(separator, str) or not separator:
        raise DocsError(f"{path}: config.separator must be a nonempty regular expression")
    try:
        separator_re = re.compile(separator)
    except re.error as error:
        raise DocsError(f"{path}: config.separator is invalid: {error}") from error
    if separator_re.search("") is not None:
        raise DocsError(f"{path}: config.separator must not match an empty string")

    entries = data.get("items")
    if entries is None:
        # Zensical 0.0.71 writes the Material-compatible "docs" shape to
        # search/search_index.json; accept the planned "items" shape too.
        entries = data.get("docs")
    if not isinstance(entries, list) or not entries:
        raise DocsError(f"{path}: search index entries must be a nonempty items or docs list")
    validated: list[dict[str, Any]] = []
    for number, entry in enumerate(entries, start=1):
        if not isinstance(entry, dict):
            raise DocsError(f"{path}: search entry {number} must be an object")
        location = entry.get("location", entry.get("path"))
        if not isinstance(location, str):
            raise DocsError(f"{path}: search entry {number} needs a string location or path")
        if not isinstance(entry.get("title"), str) or not isinstance(entry.get("text"), str):
            raise DocsError(f"{path}: search entry {number} needs string title and text fields")
        tags = entry.get("tags", [])
        if not isinstance(tags, (list, str)):
            raise DocsError(f"{path}: search entry {number} tags must be a list or string")
        validated.append({**entry, "location": location})
    return separator, validated


def _query_matches(query: str, title: str, text: str, separator: str) -> bool:
    needle = query.casefold()
    title_folded = unescape(title).casefold()
    text_folded = unescape(text).casefold()
    if needle in title_folded:
        return True

    words = [part.casefold() for part in re.split(separator, query) if part]
    if len(words) > 1:
        searchable = f"{title_folded}\n{text_folded}"
        return all(word in searchable for word in words)
    if not words:
        # A punctuation-only query must match a literal character in the
        # indexed item; separator splitting must not turn it into a match-all.
        return needle in title_folded
    return False


def _location_matches(actual: str, expected: str) -> bool:
    actual_value = "/" + unquote(actual.strip()).lstrip("/")
    expected_value = "/" + unquote(expected.strip()).lstrip("/")
    if actual_value == expected_value:
        return True
    if not actual_value.startswith(expected_value):
        return False
    if expected_value.endswith(("/", "#")):
        return True
    return len(actual_value) > len(expected_value) and actual_value[len(expected_value)] in "/#"


def check_search_cases(
    cases: list[dict[str, str]], separator: str, entries: list[dict[str, Any]]
) -> list[str]:
    failures: list[str] = []
    for case in cases:
        query = case["query"]
        expected_location = case["location"]
        matching = [
            entry
            for entry in entries
            if _query_matches(query, entry["title"], entry["text"], separator)
        ]
        located = [
            entry for entry in matching if _location_matches(entry["location"], expected_location)
        ]
        if located:
            continue
        if matching:
            found_at = ", ".join(sorted({entry["location"] or "/" for entry in matching}))
            failures.append(
                f"query {query!r} is findable at {found_at}, not under {expected_location!r}"
            )
        else:
            failures.append(f"query {query!r} is not findable under {expected_location!r}")
    return failures


def run_search_smoke(root: Path, *, context: DocsContext | None = None) -> int:
    """Check curated queries against the generated search index, not UI ranking."""

    docs_context = context or context_from_root(root)
    cases_path = docs_context.repo_root / SEARCH_CASES
    cases = _load_cases(cases_path)
    index_path = _index_path(docs_context.site_dir)
    separator, entries = _load_index(index_path)
    failures = check_search_cases(cases, separator, entries)
    for failure in failures:
        print(f"{cases_path}: {failure}")
    if failures:
        print(f"search smoke: {len(failures)} of {len(cases)} case(s) failed")
        return 1
    print(f"search smoke: OK ({len(cases)} cases; index {index_path})")
    return 0
