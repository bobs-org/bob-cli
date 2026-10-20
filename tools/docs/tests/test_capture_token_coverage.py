"""Every capture token spelling needs a literal smoke query at its heading.

Parses the Token index table in docs/reference/capture-syntax.md for its
heading anchors and token spellings. The table stays the registry: adding a
token row without a matching query at that heading fails. Grouped spellings
split on ` / ` each need their own query. No token string or ordinary prose
is pinned here beyond that structural check.
"""

from __future__ import annotations

import re
from pathlib import Path

from bob_docs.yaml_tools import load_yaml

ROOT = Path(__file__).resolve().parents[3]
TOKEN_PAGE = ROOT / "docs/reference/capture-syntax.md"
SMOKE_CASES = ROOT / "docs/_meta/search_smoke.yml"

ANCHOR_LINK = re.compile(r"\(#([A-Za-z0-9\-]+)\)")
BACKTICK = re.compile(r"`([^`]+)`")


def _token_index_rows(page: str) -> list[tuple[list[str], str]]:
    """Return (spellings, anchor) for each token-index table row."""

    lines = page.splitlines()
    start = next(
        index for index, line in enumerate(lines) if line.strip() == "## Token index"
    )
    end = next(
        (
            index
            for index, line in enumerate(lines[start + 1 :], start + 1)
            if line.startswith("## ")
        ),
        len(lines),
    )
    rows: list[tuple[list[str], str]] = []
    for line in lines[start:end]:
        if not line.startswith("|"):
            continue
        cells = [cell.strip() for cell in line.strip().strip("|").split("|")]
        if len(cells) < 3 or cells[0].lower() == "token":
            continue
        match = ANCHOR_LINK.search(cells[2])
        if not match:
            continue
        rows.append((_token_spellings(cells[0]), match.group(1)))
    return rows


def _token_spellings(cell: str) -> list[str]:
    """Indexed token spellings, including grouped equivalents split on ` / `."""

    spellings: list[str] = []
    for part in cell.split(" / "):
        part = part.strip()
        if not part:
            continue
        stripped = BACKTICK.sub(r"\1", part).strip()
        if stripped:
            spellings.append(stripped)
    return spellings


def _query_text(raw: object) -> str:
    return str(raw).strip().strip('"').strip("'")


def missing_token_queries(page: str, cases: list[dict[str, object]]) -> list[str]:
    """Spellings whose exact query is missing at the indexed heading."""

    covered: dict[str, set[str]] = {}
    for item in cases:
        location = str(item.get("location", ""))
        if not location.startswith("/reference/capture-syntax/#"):
            continue
        anchor = location.rsplit("#", 1)[-1]
        covered.setdefault(anchor, set()).add(_query_text(item.get("query", "")))

    missing: list[str] = []
    for spellings, anchor in _token_index_rows(page):
        present = covered.get(anchor, set())
        for spelling in spellings:
            if spelling not in present:
                missing.append(f"{spelling!r} at #{anchor}")
    return missing


def test_every_capture_token_has_a_literal_query_at_its_heading() -> None:
    page = TOKEN_PAGE.read_text(encoding="utf-8")
    rows = _token_index_rows(page)
    assert rows, "token index table has no heading anchors"
    anchors = [anchor for _spellings, anchor in rows]
    assert len(anchors) == len(set(anchors)), "token index repeats an anchor"

    raw = load_yaml(SMOKE_CASES)
    assert isinstance(raw, list)
    cases = [item for item in raw if isinstance(item, dict)]
    missing = missing_token_queries(page, cases)
    assert not missing, (
        "capture token spellings without a literal search query at their heading: "
        + ", ".join(missing)
    )


def test_unrelated_query_at_the_heading_does_not_cover_the_token() -> None:
    page = """
## Token index

| Token | Meaning | Heading |
| --- | --- | --- |
| `+query` / `+prep` | **Picker:** choose a parent | [`+query`](#plus-parent-picker) |

## Next
"""
    cases = [
        {
            "query": "completely unrelated words",
            "location": "/reference/capture-syntax/#plus-parent-picker",
        }
    ]
    missing = missing_token_queries(page, cases)
    assert "+query" in " ".join(missing)
    assert "+prep" in " ".join(missing)

    cases = [
        {
            "query": "+query",
            "location": "/reference/capture-syntax/#plus-parent-picker",
        },
        {
            "query": "+prep",
            "location": "/reference/capture-syntax/#plus-parent-picker",
        },
    ]
    assert missing_token_queries(page, cases) == []
