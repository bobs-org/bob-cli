---
keyword: Code Copy Reads bob's Render-Time Catalog
aliases:
  - code catalog source of truth
  - exact code block copy
summary: >-
  bob records code blocks and page anchors during PDF rendering, and serves the
  catalog through bob ref code; clients present those bytes and never extract code
  from the PDF text layer or re-parse Markdown at read time.
metadata:
  status: accepted
  decided: 2026-10-26
---

**Applies to.** bob-cli PDF rendering and `bob ref code`, plus Bob Refs code
copy and Highlights integration.

**Claim.** Capture code and Mermaid blocks during the pandoc render that creates
the PDF. The render-time catalog records exact block text, language, attributes,
section, and anchor-derived page range, paired with the preserved source. Serve
the verified catalog through `bob ref code`; that command never runs pandoc.
Bob Mac Capture decodes bob's result, previews the payload, and copies the
reported bytes. Plain copy preserves tabs, indentation, blank lines, trailing
spaces, Unicode, and all other recorded content, with LF line endings and no
added trailing newline. Fenced copy is an explicit alternate. The PDF text layer
is only a locator during verified attachment and is never a code-copy source.

**Why.** Pandoc's render is the only point that knows which blocks reached the
PDF after transclusion, section filtering, and Mermaid conversion; it also has
the block text before PDF layout changes it and can attach page anchors. Reading
the PDF text layer silently changes code through wrapped lines, lost tabs, and
dropped glyphs. Parsing Markdown later with a second parser would disagree with
pandoc on its dialect, nested containers, and rendered block order. A shared
JSON read surface leaves one semantic authority in bob while letting the Mac
client show exactly what it will copy.

**Cost.** The catalog is derived data tied to a specific PDF and source digest,
so it must be regenerated when rendering changes and validated before use. The
Mac depends on a compatible `bob` JSON contract; clients must handle missing or
invalid catalogs without inventing fallback code.

**Reopens when.** A renderer change makes the catalog fail to match the rendered
PDF, or a future shared parser can prove exact agreement with pandoc for every
supported input route and preserve the same page and byte guarantees.

Evidence: `research:202610/ref_code_block_copy/ref_code_block_copy.md`; bob-cli
`c7ced497` (render-time catalog), `009060f9` (catalog storage), `fb28faa1`
(`bob ref code` read contract), `36ae0ef0` (verified attachment); Bob Mac
Capture `d8a828f` (catalog and picker models), `c7a5f37` (Copy card and preview
well), `7b9bbdd` (Highlights Command-Y entry).
