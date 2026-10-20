"""Staged-only broken-link drill for gated documentation publication.

A drill proves the publication gates catch a broken link without risking the
live site or the tracked sources. The probe is a relative link with a bogus
fragment appended to the staged homepage after :func:`assemble` has verified
production source cleanliness. The strict builder tolerates the probe (the
page exists; only the fragment is missing) while offline lychee with
``--include-fragments`` flags it, so the link gate is the expected failure.

The probe never touches tracked sources: only files under the assembled
staging tree (``target/docs/docs/``) are modified. Assembly copies the
authored tree into the staging tree, so any later ``assemble`` or ``build``
call discards the probe.
"""

from __future__ import annotations

from pathlib import Path

from .context import DocsContext
from .errors import DocsError

DRILL_MARKER = "bob-docs-drill-broken-fragment"
DRILL_PAGE = "index.md"
DRILL_LINK_TEXT = "drill probe (do not publish)"


def staged_index(context: DocsContext) -> Path:
    """Return the staged homepage that carries the drill probe."""

    return context.output_dir / "docs" / DRILL_PAGE


def drill_probe_line(*, marker: str = DRILL_MARKER) -> str:
    """Return the staged-only Markdown line injected by a drill."""

    return f"[{DRILL_LINK_TEXT}](./#{marker})"


def drill_marker_present(staged_docs: Path, *, marker: str = DRILL_MARKER) -> bool:
    """Return whether any staged Markdown file already carries the marker."""

    staged = staged_docs.resolve()
    if not staged.is_dir():
        return False
    for path in sorted(staged.rglob("*.md")):
        if not path.is_file() or path.is_symlink():
            continue
        try:
            text = path.read_text(encoding="utf-8")
        except OSError:
            continue
        if marker in text:
            return True
    return False


def inject_drill_link(
    context: DocsContext, *, marker: str = DRILL_MARKER
) -> Path:
    """Append the drill probe to the staged homepage; never touch sources.

    Raises :class:`DocsError` when the staged tree is missing or already
    carries a drill marker, so a repeated injection cannot pile up probes.
    """

    staged_docs = (context.output_dir / "docs").resolve()
    if not staged_docs.is_dir():
        raise DocsError(
            f"drill needs an assembled staging tree at {staged_docs}; run assemble first"
        )
    if drill_marker_present(staged_docs, marker=marker):
        raise DocsError(
            "drill marker is already present in the staging tree; refusing a second injection"
        )
    page = staged_index(context).resolve()
    if not page.is_file():
        raise DocsError(f"drill needs a staged homepage at {page}")
    if not page.is_relative_to(staged_docs):
        raise DocsError(f"drill target escapes the staging tree: {page}")
    try:
        text = page.read_text(encoding="utf-8")
    except OSError as error:
        raise DocsError(f"cannot read staged homepage {page}: {error}") from error
    probe = drill_probe_line(marker=marker)
    page.write_text(text.rstrip("\n") + f"\n\n{probe}\n", encoding="utf-8")
    return page


def drill_link_built(site_dir: Path, *, marker: str = DRILL_MARKER) -> bool:
    """Return whether the built site HTML contains the drill marker."""

    site = site_dir.resolve()
    if not site.is_dir():
        return False
    for path in sorted(site.rglob("*.html")):
        if not path.is_file() or path.is_symlink():
            continue
        try:
            text = path.read_text(encoding="utf-8")
        except OSError:
            continue
        if marker in text:
            return True
    return False
