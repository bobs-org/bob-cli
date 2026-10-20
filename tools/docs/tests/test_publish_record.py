import json
from pathlib import Path

import pytest

from bob_docs.errors import DocsError
from bob_docs.publish_plan import fingerprint_inputs
from bob_docs.publish_record import (
    build_record,
    collect_tool_versions,
    parse_tool_assignments,
    run_publish_record,
)


def _inputs() -> list[dict[str, str]]:
    return [
        {"repo": "bob-cli", "path": "docs", "kind": "tree", "object_id": "a" * 40},
        {"repo": "bob-cli", "path": "mkdocs.yml", "kind": "blob", "object_id": "b" * 40},
    ]


def _candidate(*, hold: bool = False) -> dict[str, object]:
    inputs = _inputs()
    return {
        "schema_version": 1,
        "hold": hold,
        "sources": {
            "bob-cli": {"remote": "https://example.invalid/bob-cli.git", "branch": "main", "sha": "c" * 40}
        },
        "inputs": inputs,
        "fingerprint": fingerprint_inputs(inputs),
    }


def _tools() -> dict[str, str]:
    return {"bob-docs": "0.1.0", "python": "3.12.0", "zensical": "0.0.71"}


def test_build_record_preserves_hold_and_attaches_metadata() -> None:
    record = build_record(
        _candidate(hold=True),
        source="next-publication.json",
        tools=_tools(),
        built_at="2026-10-20T00:00:00Z",
        run_url="https://example.invalid/runs/1",
    )

    assert record["hold"] is True
    assert record["tools"] == _tools()
    assert record["built_at"] == "2026-10-20T00:00:00Z"
    assert record["fingerprint"] == fingerprint_inputs(_inputs())


def test_build_record_rejects_fingerprint_mismatch() -> None:
    candidate = _candidate()
    candidate["fingerprint"] = "0" * 64

    with pytest.raises(DocsError, match="does not match its inputs"):
        build_record(
            candidate,
            source="next-publication.json",
            tools=_tools(),
            built_at="2026-10-20T00:00:00Z",
            run_url="https://example.invalid/runs/1",
        )


def test_build_record_rejects_unsupported_schema() -> None:
    candidate = _candidate()
    candidate["schema_version"] = 2

    with pytest.raises(DocsError, match="unsupported schema_version"):
        build_record(
            candidate,
            source="next-publication.json",
            tools=_tools(),
            built_at="2026-10-20T00:00:00Z",
            run_url="https://example.invalid/runs/1",
        )


@pytest.mark.parametrize(
    "tools,built_at,run_url",
    [
        ({}, "2026-10-20T00:00:00Z", "https://example.invalid/runs/1"),
        ({"bob-docs": ""}, "2026-10-20T00:00:00Z", "https://example.invalid/runs/1"),
        (_tools(), "not-a-time", "https://example.invalid/runs/1"),
        (_tools(), "2026-10-20T00:00:00", "https://example.invalid/runs/1"),
        (_tools(), "2026-10-20T00:00:00Z", ""),
        (_tools(), "2026-10-20T00:00:00Z", "   "),
    ],
)
def test_build_record_rejects_bad_metadata(tools, built_at, run_url) -> None:
    with pytest.raises(DocsError):
        build_record(
            _candidate(),
            source="next-publication.json",
            tools=tools,
            built_at=built_at,
            run_url=run_url,
        )


def test_run_publish_record_writes_canonical_json(tmp_path: Path) -> None:
    candidate_file = tmp_path / "next-publication.json"
    candidate_file.write_text(json.dumps(_candidate()), encoding="utf-8")
    output = tmp_path / "deploy" / "published.json"

    record = run_publish_record(
        candidate_file,
        output,
        tools=_tools(),
        built_at="2026-10-20T00:00:00Z",
        run_url="https://example.invalid/runs/1",
    )

    assert json.loads(output.read_text(encoding="utf-8")) == record
    assert output.read_text(encoding="utf-8").endswith("\n")


def test_run_publish_record_missing_candidate_fails_closed(tmp_path: Path) -> None:
    with pytest.raises(DocsError, match="does not exist"):
        run_publish_record(
            tmp_path / "absent.json",
            tmp_path / "published.json",
            run_url="https://example.invalid/runs/1",
        )


def test_run_publish_record_rejects_malformed_json(tmp_path: Path) -> None:
    candidate_file = tmp_path / "next-publication.json"
    candidate_file.write_text("{not json", encoding="utf-8")

    with pytest.raises(DocsError, match="invalid JSON"):
        run_publish_record(
            candidate_file, tmp_path / "published.json", run_url="https://example.invalid/runs/1"
        )


def test_parse_tool_assignments() -> None:
    assert parse_tool_assignments(["a=1", "b = 2 "]) == {"a": "1", "b": "2"}
    with pytest.raises(DocsError, match="KEY=VALUE"):
        parse_tool_assignments(["novalue"])
    with pytest.raises(DocsError, match="KEY=VALUE"):
        parse_tool_assignments(["=empty"])


def test_collect_tool_versions_reports_runtimes() -> None:
    tools = collect_tool_versions()
    assert tools["bob-docs"] and tools["python"] and tools["zensical"]
