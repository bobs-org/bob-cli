"""Generate the glossary and design-decision pages from SASE memory webs."""

from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path

from .errors import DocsError
from .lint import BEAD_ID_RE
from .markdown import (
    INLINE_CODE_RE,
    iter_fences,
    mask_for_hygiene,
    split_front_matter,
)
from .yaml_tools import load_yaml_text

GLOSSARY_PATH = "reference/glossary.md"
DECISIONS_PATH = "develop/decisions.md"

WIKI_LINK_RE = re.compile(r"\[\[([^\[\]\n]+)\]\]")
ROSTER_TITLE_RE = re.compile(r"\*\*(?P<title>.+?)\*\*\s*\(`(?P<slug>[^`]+)`\)")


@dataclass(frozen=True)
class Strand:
    """One memory-web record with its file-stem slug."""

    slug: str
    title: str
    aliases: tuple[str, ...]
    summary: str
    status: str
    successors: tuple[str, ...]
    body: str


def _collapse_whitespace(text: str) -> str:
    return " ".join(text.split())


def _strand_front_matter(path: Path) -> tuple[dict[str, object], str]:
    text = path.read_text(encoding="utf-8")
    raw, body, _ = split_front_matter(text)
    if raw is None:
        raise DocsError(f"{path}: memory strand has no front matter")
    front = load_yaml_text(raw, source=str(path))
    if not isinstance(front, dict):
        raise DocsError(f"{path}: memory strand front matter must be a mapping")
    return front, body.strip() + "\n"


def _string_list(value: object, *, path: Path, key: str) -> tuple[str, ...]:
    if value is None:
        return ()
    if isinstance(value, str):
        return (value,)
    if isinstance(value, list) and all(isinstance(item, str) for item in value):
        return tuple(value)
    raise DocsError(f"{path}: front matter key {key!r} must be a string or list of strings")


def load_roster_titles(descriptor: Path) -> dict[str, str]:
    """Map strand slugs to the descriptor roster's bold titles."""

    try:
        text = descriptor.read_text(encoding="utf-8")
    except OSError as error:
        raise DocsError(f"cannot read memory descriptor {descriptor}: {error}") from error
    return {
        match.group("slug"): match.group("title")
        for match in ROSTER_TITLE_RE.finditer(text)
    }


def load_strands(memory_dir: Path, web: str) -> list[Strand]:
    """Load every `*.md` strand of one memory web, sorted by slug."""

    web_dir = memory_dir / web
    if not web_dir.is_dir():
        raise DocsError(f"memory web is missing: {web_dir}")
    roster = load_roster_titles(memory_dir / f"{web}.md")
    strands: list[Strand] = []
    for path in sorted(web_dir.glob("*.md")):
        slug = path.stem
        front, body = _strand_front_matter(path)
        title = front.get("keyword", front.get("title"))
        if not isinstance(title, str) or not title.strip():
            # Older strands carry their title as a body heading instead.
            for line in body.splitlines():
                if line.startswith("# "):
                    title = line[2:].strip()
                    body = "\n".join(
                        later
                        for later in body.splitlines()
                        if later.strip() != line.strip()
                    ).strip() + "\n"
                    break
            else:
                title = roster.get(slug)
        if not isinstance(title, str) or not title.strip():
            raise DocsError(f"{path}: memory strand needs a keyword or title")
        summary = front.get("summary", "")
        if not isinstance(summary, str):
            raise DocsError(f"{path}: memory strand summary must be a string")
        metadata = front.get("metadata")
        if metadata is None:
            metadata = {}
        if not isinstance(metadata, dict):
            raise DocsError(f"{path}: memory strand metadata must be a mapping")
        status = metadata.get("status", "accepted")
        if not isinstance(status, str) or not status.strip():
            raise DocsError(f"{path}: memory strand status must be a nonempty string")
        successors = _string_list(
            metadata.get("superseded_by"), path=path, key="superseded_by"
        )
        aliases = _string_list(
            front.get("aliases", front.get("aka")), path=path, key="aliases"
        )
        strands.append(
            Strand(
                slug=slug,
                title=title.strip(),
                aliases=aliases,
                summary=_collapse_whitespace(summary),
                status=status.strip().lower().replace("_", "-"),
                successors=successors,
                body=body,
            )
        )
    if not strands:
        raise DocsError(f"memory web has no strands: {web_dir}")
    return strands


def _code_spans(text: str) -> list[tuple[int, int]]:
    spans = [(fence.start, fence.end) for fence in iter_fences(text)]
    spans.extend(
        (match.start(), match.end()) for match in INLINE_CODE_RE.finditer(text)
    )
    return spans


def _in_spans(offset: int, spans: list[tuple[int, int]]) -> bool:
    return any(start <= offset < end for start, end in spans)


def resolve_wiki_link(
    target: str, *, page: str, webs: dict[str, dict[str, Strand]]
) -> str | None:
    """Map one `[[...]]` target to Markdown, or None to leave it literal."""

    display, _, alias = target.partition("|")
    display = display.strip()
    alias = alias.strip()
    for separator in (":", "/"):
        if separator in display:
            web_name, _, slug = display.partition(separator)
            break
    else:
        web_name, slug = page, display
    slug, _, _fragment = slug.partition("#")
    web = webs.get(web_name)
    if web is None:
        return None
    strand = web.get(slug.strip())
    if strand is None:
        # Explicit cross-web references must resolve; bare same-web
        # vocabulary that names no strand stays literal, as in contracts.
        if web_name != page:
            raise DocsError(
                f"memory {web_name}:{slug.strip()} has no strand "
                f"(linked as [[{target}]])"
            )
        return None
    text = alias or strand.title
    if web_name == page:
        return f"[{text}](#{strand.slug})"
    if page == "glossary" and web_name == "decisions":
        relative = "../develop/decisions.md"
    elif page == "decisions" and web_name == "glossary":
        relative = "../reference/glossary.md"
    else:  # pragma: no cover - only two memory webs feed generated pages
        raise DocsError(f"cannot link [[{target}]] from the {page} page")
    return f"[{text}]({relative}#{strand.slug})"


def convert_wiki_links(
    body: str, *, page: str, webs: dict[str, dict[str, Strand]]
) -> str:
    """Turn `[[web:slug]]` links into anchors, skipping code spans."""

    spans = _code_spans(body)

    def replace(match: re.Match[str]) -> str:
        if _in_spans(match.start(), spans):
            return match.group(0)
        converted = resolve_wiki_link(match.group(1), page=page, webs=webs)
        return converted if converted is not None else match.group(0)

    return WIKI_LINK_RE.sub(replace, body)


def check_no_bead_ids(text: str, *, path: str) -> None:
    """Reference pages must not name bead IDs; fail the generation."""

    masked = mask_for_hygiene(text)
    match = BEAD_ID_RE.search(masked)
    if match:
        raise DocsError(f"{path}: generated page names bead ID {match.group(0)!r}")


def render_glossary_page(
    strands: list[Strand], webs: dict[str, dict[str, Strand]]
) -> str:
    """Render `reference/glossary.md`: terms A-Z with anchors and aliases."""

    ordered = sorted(strands, key=lambda strand: (strand.title.casefold(), strand.slug))
    lines = [
        "---",
        "title: Glossary",
        "description: Bob's shared vocabulary, generated from the glossary memory web.",
        "---",
        "<!-- generated by bob-docs; run just docs-gen -->",
        "",
        "# Glossary",
        "",
        "Bob's shared vocabulary, generated from the glossary memory web.",
        "",
    ]
    for strand in ordered:
        lines.append(f"### {strand.title} {{#{strand.slug}}}")
        lines.append("")
        if strand.aliases:
            lines.append(f"*Aliases: {', '.join(strand.aliases)}.*")
            lines.append("")
        body = convert_wiki_links(strand.body, page="glossary", webs=webs).rstrip("\n")
        lines.append(body)
        lines.append("")
    return "\n".join(lines).rstrip("\n") + "\n"


def _successor_ref(ref: str, *, decisions: dict[str, Strand]) -> str:
    for separator in (":", "/"):
        if separator in ref:
            _, _, slug = ref.partition(separator)
            break
    else:
        slug = ref
    slug = slug.strip()
    strand = decisions.get(slug)
    if strand is None:
        raise DocsError(f"decisions:{slug} names an unknown successor {ref!r}")
    return f"[{strand.title}](#{strand.slug})"


def render_decisions_page(
    strands: list[Strand], webs: dict[str, dict[str, Strand]]
) -> str:
    """Render `develop/decisions.md`: one anchored section per record."""

    decisions = {strand.slug: strand for strand in strands}
    ordered = sorted(strands, key=lambda strand: strand.slug)
    lines = [
        "---",
        "title: Design decisions",
        "description: Accepted architecture and policy decisions for Bob, generated from the decisions memory web.",
        "---",
        "<!-- generated by bob-docs; run just docs-gen -->",
        "",
        "# Design decisions",
        "",
        "Accepted architecture and policy decisions for Bob, generated from the decisions memory web.",
        "",
    ]
    for strand in ordered:
        lines.append(f"## {strand.title} {{#{strand.slug}}}")
        lines.append("")
        if strand.status == "accepted":
            lines.append("**Status:** Accepted")
        elif strand.status in ("superseded", "superseded-in-part"):
            label = "Superseded" if strand.status == "superseded" else "Superseded in part"
            if not strand.successors:
                raise DocsError(
                    f"decisions:{strand.slug} is {strand.status} with no successors"
                )
            links = ", ".join(
                _successor_ref(ref, decisions=decisions) for ref in strand.successors
            )
            lines.append(f"**Status:** {label} by {links}")
        else:
            raise DocsError(
                f"decisions:{strand.slug} has unknown status {strand.status!r}"
            )
        lines.append("")
        if not strand.summary:
            raise DocsError(f"decisions:{strand.slug} has no roster summary")
        lines.append(strand.summary)
        lines.append("")
        body = convert_wiki_links(strand.body, page="decisions", webs=webs).rstrip("\n")
        lines.append(body)
        lines.append("")
    return "\n".join(lines).rstrip("\n") + "\n"


def generate_reference(root: Path, *, check: bool = False) -> list[Path]:
    """Generate the glossary and decisions pages; report drift with check=True."""

    memory_dir = root / "sase" / "memory"
    glossary = load_strands(memory_dir, "glossary")
    decisions = load_strands(memory_dir, "decisions")
    webs = {
        "glossary": {strand.slug: strand for strand in glossary},
        "decisions": {strand.slug: strand for strand in decisions},
    }
    glossary_page = render_glossary_page(glossary, webs)
    check_no_bead_ids(glossary_page, path=GLOSSARY_PATH)
    decisions_page = render_decisions_page(decisions, webs)
    desired = {
        root / "docs" / GLOSSARY_PATH: glossary_page,
        root / "docs" / DECISIONS_PATH: decisions_page,
    }
    drift: list[Path] = []
    for path, content in desired.items():
        try:
            current = path.read_text(encoding="utf-8")
        except FileNotFoundError:
            current = None
        if current != content:
            drift.append(path)
            if not check:
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(content, encoding="utf-8")
    return drift
