import hashlib
import shutil
from pathlib import Path

import pytest

from bob_docs.contracts import load_contract_registry
from bob_docs.errors import DocsError
from bob_docs.gen_contracts import check_or_generate, generate_contracts
from bob_docs.yaml_tools import dump_yaml, load_yaml

ROOT = Path(__file__).resolve().parents[3]
README_TABLE_SHA256 = "9c9b378872d18636166d052f444f90e7b6a091a772514b5a6aa83c627d094d1a"


def _generated_region(text: str, name: str) -> str:
    begin = f"<!-- BEGIN GENERATED: {name} -->\n"
    end = f"<!-- END GENERATED: {name} -->"
    return text.split(begin, 1)[1].split(end, 1)[0].rstrip("\n")


def _nav_pages(nav: list[object]) -> list[str]:
    pages: list[str] = []
    for item in nav:
        if isinstance(item, dict):
            for value in item.values():
                if isinstance(value, str) and value.endswith(".md"):
                    pages.append(value)
                elif isinstance(value, list):
                    pages.extend(_nav_pages(value))
    return pages


def test_current_registry_and_generated_nav_are_complete_and_unique() -> None:
    registry = load_contract_registry(ROOT)
    assert len(registry.entries) == 30
    assert sum(entry.historical for entry in registry.entries) == 1
    assert next(
        entry for entry in registry.entries if entry.stem == "obsidian-sync-exclusions"
    ).historical

    config = load_yaml(ROOT / "mkdocs.yml")
    nav_pages = _nav_pages(config["nav"])
    for entry in registry.entries:
        assert nav_pages.count(f"{entry.stem}.md") == 1


def test_llmstxt_contracts_section_lists_exactly_registry_stems() -> None:
    registry = load_contract_registry(ROOT)
    expected = sorted(f"{entry.stem}.md" for entry in registry.entries)
    config = load_yaml(ROOT / "mkdocs.yml")
    llmstxt = next(
        plugin["llmstxt"] for plugin in config["plugins"] if isinstance(plugin, dict) and "llmstxt" in plugin
    )
    actual = sorted(llmstxt["sections"]["Contracts (optional)"])
    assert actual == expected


def test_readme_table_matches_original_contract_index_snapshot() -> None:
    table = _generated_region(
        (ROOT / "docs/README.md").read_text(encoding="utf-8"), "contract table"
    )
    assert hashlib.sha256(table.encode()).hexdigest() == README_TABLE_SHA256


def test_generated_outputs_have_no_drift() -> None:
    assert generate_contracts(ROOT, check=True) == []


def test_check_mode_reports_generated_page_drift_without_writing(
    tmp_path: Path, capsys
) -> None:
    shutil.copytree(ROOT / "docs", tmp_path / "docs")
    shutil.copy2(ROOT / "mkdocs.yml", tmp_path / "mkdocs.yml")
    shutil.copytree(ROOT / "sase" / "memory", tmp_path / "sase" / "memory")
    (tmp_path / "docs/_meta/mounts.yml").write_text("[]\n", encoding="utf-8")
    page = tmp_path / "docs/develop/contracts.md"
    original = page.read_text(encoding="utf-8")
    changed = original.replace(
        "These pages specify behavior", "Drifted pages specify behavior"
    )
    page.write_text(changed, encoding="utf-8")

    assert check_or_generate(tmp_path, check=True) == 1
    assert page.read_text(encoding="utf-8") == changed
    assert "out of date: docs/develop/contracts.md" in capsys.readouterr().out


@pytest.mark.parametrize(
    ("entries", "expected_error"),
    [
        (
            [
                {"stem": "one", "summary": "One", "order": 1},
                {"stem": "one", "summary": "Again", "order": 2},
            ],
            "duplicate contract stem",
        ),
        (
            [{"stem": "missing", "summary": "Missing", "order": 1}],
            "missing or unsafe contract page",
        ),
        (
            [{"stem": "../escape", "summary": "Unsafe", "order": 1}],
            "safe lowercase Markdown stem",
        ),
    ],
)
def test_registry_rejects_duplicate_missing_and_unsafe_stems(
    tmp_path: Path, entries: list[dict[str, object]], expected_error: str
) -> None:
    (tmp_path / "docs/_meta").mkdir(parents=True)
    (tmp_path / "docs/one.md").write_text("# One\n", encoding="utf-8")
    registry_path = tmp_path / "docs/_meta/contracts.yml"
    registry_path.write_text(
        dump_yaml({"groups": [{"title": "Group", "entries": entries}]}),
        encoding="utf-8",
    )
    with pytest.raises(DocsError, match=expected_error):
        load_contract_registry(tmp_path)
