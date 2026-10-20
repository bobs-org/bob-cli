"""Demo-vault Tasks status registry matches what bob capture requires."""

from __future__ import annotations

import json
from pathlib import Path

REPO = Path(__file__).resolve().parents[3]
DEMO = REPO / "examples/demo-vault/.obsidian/plugins/obsidian-tasks-plugin/data.json"


def _statuses() -> dict:
    payload = json.loads(DEMO.read_text(encoding="utf-8"))
    assert payload.get("taskFormat") == "dataview"
    assert payload.get("globalFilter") == "#task"
    settings = payload["statusSettings"]
    entries = [*settings.get("coreStatuses", []), *settings.get("customStatuses", [])]
    return {entry["symbol"]: entry for entry in entries}


def test_demo_blocked_status_matches_bob_requirements() -> None:
    blocked = _statuses()["?"]
    assert blocked["symbol"] == "?"
    assert blocked["name"] == "Blocked"
    assert blocked["type"] == "ON_HOLD"
    assert blocked["nextStatusSymbol"] == " "
    assert blocked["availableAsCommand"] is True


def test_demo_next_status_matches_bob_requirements() -> None:
    nxt = _statuses()["*"]
    assert nxt["symbol"] == "*"
    assert nxt["name"] == "Next"
    assert nxt["type"] == "ON_HOLD"
    assert nxt["nextStatusSymbol"] == "x"
    assert nxt["availableAsCommand"] is True
