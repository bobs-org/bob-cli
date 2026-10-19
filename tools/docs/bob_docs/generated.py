"""Helpers for deterministic, explicitly marked generated content."""

from __future__ import annotations

from .errors import DocsError


def replace_generated_region(text: str, name: str, content: str, *, yaml: bool = False) -> str:
    """Replace one marked region while preserving every byte outside its markers."""

    if yaml:
        begin = f"# BEGIN GENERATED: {name}"
        end = f"# END GENERATED: {name}"
    else:
        begin = f"<!-- BEGIN GENERATED: {name} -->"
        end = f"<!-- END GENERATED: {name} -->"

    lines = text.splitlines(keepends=True)
    begins = [i for i, line in enumerate(lines) if line.strip() == begin]
    ends = [i for i, line in enumerate(lines) if line.strip() == end]
    if len(begins) != 1 or len(ends) != 1:
        raise DocsError(
            f"generated region {name!r} needs exactly one begin and end marker "
            f"({len(begins)} begin, {len(ends)} end)"
        )
    start, finish = begins[0], ends[0]
    if finish <= start:
        raise DocsError(f"generated region {name!r} has markers in the wrong order")

    body = content.rstrip("\n")
    if body:
        body += "\n"
    return "".join(lines[: start + 1]) + body + "".join(lines[finish:])


def first_heading(text: str) -> str:
    """Return the first level-one heading from a Markdown page."""

    for line in text.splitlines():
        if line.startswith("# "):
            return line[2:].strip()
    raise DocsError("contract page has no level-one heading")
