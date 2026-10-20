"""Every capture token in the index needs a smoke case at its heading.

Parses the Token index table in docs/reference/capture-syntax.md for its
heading anchors and requires one search-smoke case per anchor under
/reference/capture-syntax/. The table stays the registry: adding a token row
without a smoke case fails, but no token string or prose is pinned here.
"""

from __future__ import annotations

import re
from pathlib import Path

from bob_docs.yaml_tools import load_yaml

ROOT = Path(__file__).resolve().parents[3]
TOKEN_PAGE = ROOT / "docs/reference/capture-syntax.md"
SMOKE_CASES = ROOT / "docs/_meta/search_smoke.yml"

ANCHOR_LINK = re.compile(r"\(#([A-Za-z0-9\-]+)\)")


def _token_anchors(page: str) -> list[str]:
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
    anchors: list[str] = []
    for line in lines[start:end]:
        if not line.startswith("|"):
            continue
        cells = [cell.strip() for cell in line.strip().strip("|").split("|")]
        if len(cells) < 3 or cells[0].lower() == "token":
            continue
        match = ANCHOR_LINK.search(cells[2])
        if match:
            anchors.append(match.group(1))
    return anchors


def test_every_capture_token_has_a_smoke_case() -> None:
    anchors = _token_anchors(TOKEN_PAGE.read_text(encoding="utf-8"))
    assert anchors, "token index table has no heading anchors"
    assert len(anchors) == len(set(anchors)), "token index repeats an anchor"

    raw = load_yaml(SMOKE_CASES)
    assert isinstance(raw, list)
    locations = [
        str(item.get("location", ""))
        for item in raw
        if isinstance(item, dict)
    ]
    covered = {
        location.rsplit("#", 1)[-1]
        for location in locations
        if location.startswith("/reference/capture-syntax/#")
    }
    missing = [anchor for anchor in anchors if anchor not in covered]
    assert not missing, (
        "capture tokens without a search smoke case at their heading: "
        + ", ".join(f"#{anchor}" for anchor in missing)
    )
