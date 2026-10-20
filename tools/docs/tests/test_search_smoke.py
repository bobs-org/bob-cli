import json

import pytest

from bob_docs.context import DocsContext
from bob_docs.errors import DocsError
from bob_docs.search_smoke import (
    _load_index,
    _query_matches,
    check_search_cases,
    run_search_smoke,
)


def test_case_insensitive_multi_word_search_and_location_prefix() -> None:
    entries = [
        {
            "location": "vault-git-sync/#conflict-policy",
            "title": "Conflict policy",
            "text": "A local conflict copy is retained.",
        },
        {
            "location": "vault-git-sync/#halted-sync",
            "title": "Halted sync",
            "text": "The sync can stop after an interrupted merge.",
        },
    ]
    cases = [
        {"query": "HALTED sync", "location": "/vault-git-sync/#halted-sync"},
        {"query": "conflict copy", "location": "/vault-git-sync/"},
    ]

    assert check_search_cases(cases, r"[\s\-]+", entries) == []


def test_location_prefix_requires_a_path_boundary() -> None:
    entries = [
        {
            "location": "vault-git-sync-extra/#conflict-policy",
            "title": "Conflict policy",
            "text": "A conflict copy is retained.",
        }
    ]
    failures = check_search_cases(
        [{"query": "conflict copy", "location": "/vault-git-sync/"}],
        r"[\s\-]+",
        entries,
    )
    assert len(failures) == 1
    assert "findable at" in failures[0]
    assert "not under '/vault-git-sync/'" in failures[0]


def test_punctuation_only_queries_require_a_literal_character() -> None:
    assert not _query_matches("!", "Task syntax", "ordinary words", r"[\s\-!]+")
    assert not _query_matches("!", "Task syntax", "The ! token completes a task", r"[\s\-!]+")
    assert _query_matches("!", "`!` completes tasks", "Task syntax", r"[\s\-!]+")


def test_search_index_accepts_zensical_docs_shape(tmp_path) -> None:
    path = tmp_path / "search_index.json"
    path.write_text(
        json.dumps(
            {
                "config": {"separator": r"[\s\-]+"},
                "docs": [
                    {"location": "guide/", "title": "Guide", "text": "Body"}
                ],
            }
        ),
        encoding="utf-8",
    )
    separator, entries = _load_index(path)
    assert separator == r"[\s\-]+"
    assert entries[0]["location"] == "guide/"


def test_search_index_accepts_items_shape_and_rejects_empty_entries(tmp_path) -> None:
    path = tmp_path / "search.json"
    path.write_text(
        json.dumps(
            {
                "config": {"separator": r"[\s\-]+"},
                "items": [
                    {
                        "location": "guide/",
                        "path": "guide/",
                        "title": "Guide",
                        "text": "Body",
                        "tags": [],
                    }
                ],
            }
        ),
        encoding="utf-8",
    )
    _separator, entries = _load_index(path)
    assert entries[0]["path"] == "guide/"

    path.write_text(json.dumps({"config": {"separator": r"[\s\-]+"}, "items": []}), encoding="utf-8")
    with pytest.raises(DocsError, match="nonempty items or docs list"):
        _load_index(path)


def test_search_smoke_rejects_empty_cases_and_bad_index(tmp_path) -> None:
    root = tmp_path
    (root / "docs/_meta").mkdir(parents=True)
    (root / "docs/_meta/search_smoke.yml").write_text("[]\n", encoding="utf-8")
    (root / "target/docs/site/search").mkdir(parents=True)
    context = DocsContext(
        root,
        root / "docs",
        root / "mkdocs.yml",
        root / "target/docs",
        {"site_dir": "target/docs/site"},
    )
    with pytest.raises(DocsError, match="nonempty list"):
        run_search_smoke(root, context=context)

    (root / "docs/_meta/search_smoke.yml").write_text(
        "- query: guide\n  location: /guide/\n", encoding="utf-8"
    )
    (root / "target/docs/site/search/search_index.json").write_text("{}", encoding="utf-8")
    with pytest.raises(DocsError, match="no config object"):
        run_search_smoke(root, context=context)


def test_search_smoke_rejects_missing_index(tmp_path) -> None:
    root = tmp_path
    (root / "docs/_meta").mkdir(parents=True)
    (root / "docs/_meta/search_smoke.yml").write_text(
        "- query: guide\n  location: /guide/\n", encoding="utf-8"
    )
    (root / "target/docs/site").mkdir(parents=True)
    context = DocsContext(
        root,
        root / "docs",
        root / "mkdocs.yml",
        root / "target/docs",
        {"site_dir": "target/docs/site"},
    )
    with pytest.raises(DocsError, match="search index is missing"):
        run_search_smoke(root, context=context)


def test_search_smoke_diagnostics_distinguish_unfindable_and_mislocated() -> None:
    entries = [
        {"location": "elsewhere/", "title": "Conflict copy", "text": "A saved copy."}
    ]
    failures = check_search_cases(
        [
            {"query": "conflict copy", "location": "/vault-git-sync/"},
            {"query": "missing phrase", "location": "/vault-git-sync/"},
        ],
        r"[\s\-]+",
        entries,
    )
    assert "findable at elsewhere/" in failures[0]
    assert "is not findable" in failures[1]
