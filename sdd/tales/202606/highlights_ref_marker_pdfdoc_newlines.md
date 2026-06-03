---
create_time: 2026-06-03 08:57:32
status: done
prompt: sdd/prompts/202606/highlights_ref_marker_pdfdoc_newlines.md
---
# Plan: Preserve PDF Literal Marker Newlines

## Problem

`bob highlights-ref marker /home/bryan/bob/lib/docs/obsidian_docs.pdf` and the targeted `sync --dry-run` both fail with:

```text
missing required marker key: parent
```

even though page 1 contains this standalone `/Text` annotation:

```text
- status: wip
- parent: obsidian
- title: Obsidian Docs
```

Inspection shows the reference note `/home/bryan/bob/ref/docs/obsidian_docs.md` does not exist, so stale frontmatter is
not involved. Raw PDF inspection with `mutool` shows page 1's annotation array resolves to
`[30 0 R, 31 0 R, 4011 0 R, 4010 0 R]`; `4011 0 R` is the first `/Subtype /Text` annotation and its `/Contents` is:

```pdf
(- status: wip\012- parent: obsidian\012- title: Obsidian Docs)
```

The root cause is text decoding, not marker selection. `lopdf` parses `\012` to byte `0x0A`, then
`lopdf::decode_text_string` treats the no-BOM annotation text as PDFDocEncoding. In lopdf 0.40.0, `PDF_DOC_ENCODING[10]`
is `None`, so `decode_text_string` drops the line separators. The marker parser receives one collapsed line, parses only
the `status` key, and then reports the missing `parent` key.

The existing test PDFs do not catch this because their marker contents are written as UTF-16 hexadecimal strings via
`pdf_text_string`, where newlines survive `decode_text_string`.

## Approach

Fix marker annotation decoding so marker list structure survives for literal/PDFDocEncoding annotation strings while
preserving the existing behavior for Unicode text strings.

1. Add a focused marker-content decoder in `src/native/highlights_ref/mod.rs`.
   - Use the existing `lopdf::decode_text_string` for UTF-16BE and UTF-8 BOM strings.
   - For no-BOM `Object::String` values, split the raw bytes on PDF line separators (`\r\n`, `\r`, `\n`), decode each
     non-line segment with `decode_text_string`, and rejoin lines with `\n`.
   - Keep this helper scoped to annotation marker contents, since the scanner specifically depends on line-oriented
     marker parsing.

2. Update `read_pdf_marker` to call the new helper for `/Contents`.
   - Keep standalone-note selection unchanged: first `/Subtype /Text` annotation on page 1 remains the marker.
   - Keep error wording and PDF write behavior unchanged except that valid literal-string markers now parse correctly.

3. Add regression coverage.
   - Add a unit test for decoding a no-BOM PDF literal string containing line separators into three marker lines.
   - Add a CLI regression test using a synthetic PDF whose marker `/Contents` is stored as a literal string instead of
     UTF-16 hexadecimal, then assert `bob highlights-ref sync --dry-run` succeeds and sees `parent`.
   - Leave the existing UTF-16 marker tests in place to prove the current Highlights-writeback path still works.

4. Validate.
   - Run `cargo fmt --check`.
   - Run targeted highlights-ref tests, especially the new literal-string marker case.
   - Run `cargo test` if the targeted suite is clean.
   - Re-run:

```bash
cargo run --quiet -- highlights-ref marker /home/bryan/bob/lib/docs/obsidian_docs.pdf
cargo run --quiet -- highlights-ref sync /home/bryan/bob/lib/docs/obsidian_docs.pdf --dry-run
```

## Risks

- The fix should not reinterpret arbitrary PDF text globally; limiting it to marker `/Contents` keeps the behavior
  local.
- Normalizing `\r` and `\r\n` to `\n` is acceptable for marker lists because `parse_marker` is line-oriented and the
  renderer already emits `\n`.
- Non-ASCII PDFDocEncoding bytes in marker values should still decode through lopdf per segment, so this does not
  regress accented text or symbols that lopdf already supports.
