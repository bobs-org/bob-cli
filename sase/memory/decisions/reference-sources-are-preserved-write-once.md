---
keyword: Reference Sources Are Preserved Write-Once
aliases:
  - reference source store
  - saved reference Markdown
summary: >-
  bob preserves render-input Markdown and its derived code catalog in a hidden,
  content-addressed .bob store; source bytes are write-once and reference notes carry
  a note-local source_markdown pointer.
metadata:
  status: accepted
  decided: 2026-10-26
---

**Applies to.** bob-cli reference creation, intake, overwrite, backfill, and the
Bob Refs clients.

**Claim.** Preserve the exact Markdown bytes consumed by pandoc in
`.bob/ref-sources/<ref_type>/<birth-stem>--<hash12>/source.md`, alongside the
render-time `code.json` catalog. The source file is write-once: matching content
reuses it, while a digest mismatch refuses replacement. Keep the source outside
`lib/` and exclude `.bob/` from bob's note walkers. A reference note's
`source_markdown` frontmatter field points to the store. Derive that pointer at
birth by matching reference type, stem, and PDF digest; overwrite and verified
attach set it explicitly. Preserve old stores as history when content changes.

**Why.** The research found that 449 of 538 reports had code, with 70 blocks
containing literal tabs and 47% containing lines longer than 80 columns. PDF text
extraction loses tabs, wraps lines, and drops glyphs, so keeping the original
render input preserves exact copy bytes. A content-addressed hidden store keeps
those bytes and their catalog together, survives overwrite history, and avoids
the Highlights sidecar namespace. `lib/<stem>.md` conflicts with Highlights
annotations; other files under `lib/` are indexed and walked. Embedded PDF files
add a coordinated PDF write path and may not survive Highlights saves. The
note-local pointer keeps source metadata out of the PDF marker and avoids
rewriting annotated PDFs during backfill; it is derived once from the PDF digest
at note birth.

**Cost.** Source Markdown, including comments and frontmatter, is synced in the
private vault. Stores accumulate as history, and readers must validate digests
and tolerate missing catalogs. The pointer depends on birth-time PDF identity
and remains note-local.

**Reopens when.** A real collision or recovery issue shows content-addressed
history is unsafe, or the vault's privacy/sync model changes enough that syncing
the preserved Markdown is no longer acceptable.

Evidence: `research:202610/ref_code_block_copy/ref_code_block_copy.md`; bob-cli
`01f7ec97` (walker exclusion), `c7ced497` (render-time catalog), `009060f9`
(write-once source store and note pointer), `a6d3ecbf` (overwrite lifecycle),
`36ae0ef0` (verified attachment); Bob Mac Capture `d8a828f` (catalog models),
`c7a5f37` (Copy card integration).
