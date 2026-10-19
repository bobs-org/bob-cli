"""Load and validate the ordered public contract registry."""

from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from .errors import DocsError
from .yaml_tools import load_yaml

_STEM = re.compile(r"^[a-z0-9]+(?:-[a-z0-9]+)*$")


@dataclass(frozen=True)
class Contract:
    stem: str
    summary: str
    order: int
    reader: str | None = None
    historical: bool = False


@dataclass(frozen=True)
class ContractGroup:
    title: str
    entries: tuple[Contract, ...]


@dataclass(frozen=True)
class ContractRegistry:
    groups: tuple[ContractGroup, ...]

    @property
    def entries(self) -> tuple[Contract, ...]:
        return tuple(sorted((entry for group in self.groups for entry in group.entries), key=lambda x: x.order))


def _mapping(value: Any, where: str, allowed: set[str], required: set[str]) -> dict[str, Any]:
    if not isinstance(value, dict) or not all(isinstance(key, str) for key in value):
        raise DocsError(f"{where}: expected a mapping")
    unknown = set(value) - allowed
    missing = required - set(value)
    if unknown:
        raise DocsError(f"{where}: unknown keys: {', '.join(sorted(unknown))}")
    if missing:
        raise DocsError(f"{where}: missing keys: {', '.join(sorted(missing))}")
    return value


def load_contract_registry(root: Path) -> ContractRegistry:
    path = root / "docs/_meta/contracts.yml"
    raw = load_yaml(path)
    top = _mapping(raw, str(path), {"groups"}, {"groups"})
    if not isinstance(top["groups"], list) or not top["groups"]:
        raise DocsError(f"{path}: groups must be a nonempty ordered list")

    groups: list[ContractGroup] = []
    group_titles: set[str] = set()
    stems: set[str] = set()
    orders: set[int] = set()
    docs_root = (root / "docs").resolve()
    for group_index, raw_group in enumerate(top["groups"], start=1):
        where = f"{path}: groups[{group_index}]"
        group = _mapping(raw_group, where, {"title", "entries"}, {"title", "entries"})
        title = group["title"]
        entries_raw = group["entries"]
        if not isinstance(title, str) or not title.strip():
            raise DocsError(f"{where}: title must be a nonempty string")
        if title in group_titles:
            raise DocsError(f"{where}: duplicate group title {title!r}")
        group_titles.add(title)
        if not isinstance(entries_raw, list) or not entries_raw:
            raise DocsError(f"{where}: entries must be a nonempty ordered list")

        entries: list[Contract] = []
        for entry_index, raw_entry in enumerate(entries_raw, start=1):
            entry_where = f"{where}: entries[{entry_index}]"
            entry = _mapping(
                raw_entry,
                entry_where,
                {"stem", "summary", "order", "reader", "historical"},
                {"stem", "summary", "order"},
            )
            stem, summary, order = entry["stem"], entry["summary"], entry["order"]
            if not isinstance(stem, str) or not _STEM.fullmatch(stem):
                raise DocsError(f"{entry_where}: stem must be a safe lowercase Markdown stem")
            if stem in stems:
                raise DocsError(f"{entry_where}: duplicate contract stem {stem!r}")
            stems.add(stem)
            if not isinstance(summary, str) or not summary.strip():
                raise DocsError(f"{entry_where}: summary must be a nonempty string")
            if isinstance(order, bool) or not isinstance(order, int) or order < 1:
                raise DocsError(f"{entry_where}: order must be a positive integer")
            if order in orders:
                raise DocsError(f"{entry_where}: duplicate order {order}")
            orders.add(order)
            reader = entry.get("reader")
            if reader is not None and (not isinstance(reader, str) or not reader.strip()):
                raise DocsError(f"{entry_where}: reader must be a nonempty string")
            historical = entry.get("historical", False)
            if historical is not False and historical is not True:
                raise DocsError(f"{entry_where}: historical must be true when present")

            page = root / "docs" / f"{stem}.md"
            if page.is_symlink() or not page.is_file():
                raise DocsError(f"{entry_where}: missing or unsafe contract page {page}")
            if not page.resolve().is_relative_to(docs_root):
                raise DocsError(f"{entry_where}: contract path escapes docs/: {page}")
            entries.append(Contract(stem, summary, order, reader, historical))
        groups.append(ContractGroup(title, tuple(entries)))

    if orders != set(range(1, len(orders) + 1)):
        raise DocsError(f"{path}: order values must be contiguous starting at 1")
    return ContractRegistry(tuple(groups))


def registry_extra(registry: ContractRegistry) -> dict[str, Any]:
    return {
        "total": len(registry.entries),
        "groups": [
            {
                "title": group.title,
                "contracts": [
                    {
                        "stem": entry.stem,
                        "summary": entry.summary,
                        **({"reader": entry.reader} if entry.reader else {}),
                        **({"historical": True} if entry.historical else {}),
                    }
                    for entry in sorted(group.entries, key=lambda item: item.order)
                ],
            }
            for group in registry.groups
        ],
    }
