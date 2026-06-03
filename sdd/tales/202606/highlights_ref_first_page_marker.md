---
create_time: 2026-06-03 08:31:25
status: wip
prompt: sdd/prompts/202606/highlights_ref_first_page_marker.md
---
# First-Page-Only Highlights Marker Scan

## Goal

Make `bob highlights-ref` faster by relying on the newly clarified invariant: if a PDF marker note exists, it is on the
first page. The scanner should stop looking after page 1 while preserving the existing sync semantics for valid
Highlights library PDFs.

This is a narrower optimization than the previous scan speedup. The current implementation already reads each PDF once
and hashes the same byte buffer, then parallelizes planning across PDFs. This plan targets the remaining unnecessary
page traversal inside marker discovery.

## Context Reviewed

- Project short memory: work inside the ephemeral `bob-cli_<N>` workspace only.
- Current implementation: `read_pdf_marker` in `src/native/highlights_ref/mod.rs` reads the PDF bytes once, computes
  `source_pdf_sha256`, parses with `lopdf::Document::load_mem`, then calls `document.get_pages()` and scans every page's
  `/Annots` array until it finds the first standalone `/Text` annotation.
- `lopdf::Document::get_pages()` is eager: it builds a `BTreeMap` of every page by collecting `page_iter()`.
  `page_iter()` itself is lazy, so `page_iter().next()` can resolve only the first page object instead of enumerating
  the full page tree.
- `lopdf::Document::load_mem` still parses the whole document before returning. Therefore, first-page-only scanning will
  avoid full page-tree enumeration and non-first-page annotation checks, but it will not avoid reading or parsing the
  rest of the PDF. A true "only parse enough objects for page 1" optimization would be a larger partial-reader change.
- Existing reporting prints `marker_page` and `marker_note` for `sync`/`marker`. With the invariant, valid marker PDFs
  should report `marker_page: 1`; `marker_note` should remain the ordinal among standalone `/Text` annotations on
  page 1.
- Existing tests generate one-page PDF fixtures. New coverage is needed for both page-1 success and a marker only on
  page 2 being treated as missing under the new invariant.

## Product Decision

Treat the invariant as part of the command contract:

- The marker is the first standalone `/Text` note annotation on page 1.
- Standalone `/Text` notes on later pages are not marker candidates.
- If page 1 has no standalone `/Text` annotation, return the existing style of marker-not-found error, updated to make
  the page-1 expectation clear.

This intentionally changes behavior for malformed/out-of-contract PDFs that only have a marker-like note later in the
document. That tradeoff is acceptable because the user has guaranteed marker placement, and it avoids surprising
slowdowns from scanning arbitrary later annotations.

## Plan

### Phase 1 - Make marker lookup first-page-only

- Replace the eager `document.get_pages()` traversal in `read_pdf_marker` with a first-page lookup based on
  `document.page_iter().next()`.
- If the PDF has no resolvable first page, return a clear marker discovery error for that PDF.
- Inspect only the first page's annotation IDs using the existing `annotation_ids_for_page` helper.
- Preserve the existing standalone note definition (`/Subtype /Text`) and content decoding.
- Set `PdfMarker.page_number` to `1` for found markers and increment `note_number` only for standalone `/Text`
  annotations encountered on page 1 before returning the first one.
- Leave SHA-256 handling unchanged: continue reading the PDF bytes once and hashing the full file, because
  `source_pdf_sha256` must continue to represent the complete source PDF.

### Phase 2 - Update command docs and diagnostics

- Update the `marker`/`sync` help text that currently says "first standalone note annotation in the PDF" so it says the
  first standalone `/Text` annotation on the first page is the marker.
- Update the marker-not-found message to mention page 1. Keep the format compatible with existing aggregation in
  `scan_library`, so scan failures still render deterministically as `path: error` entries.
- Do not add a new CLI option. This is now the command's behavior, not a tunable mode.

### Phase 3 - Add focused tests

- Add a test fixture helper that can generate a two-page PDF with configurable annotations per page.
- Cover the success case where page 1 contains the marker and page 2 contains another standalone `/Text` note; assert
  the command selects the page-1 marker, reports `marker_page: 1`, and does not inspect/choose the later note.
- Cover the out-of-contract case where page 1 has no standalone `/Text` note and page 2 does; assert `marker` or
  `scan --dry-run` fails with the new page-1-specific missing-marker message.
- Keep existing one-page tests passing unchanged, which verifies normal sync/scan behavior remains intact.

### Phase 4 - Measure and decide whether a deeper partial reader is worth it

- Run the normal validation suite first (`cargo build`, `cargo clippy --all-targets`, `cargo test`).
- Time `bob highlights-ref scan --dry-run` on a representative real library if available.
- If the measured improvement is small, document why: `lopdf::Document::load_mem` still parses all normal xref objects
  and object streams. The first-page change avoids page enumeration and annotation scanning, not full PDF parsing.
- If more speed is still needed, plan a separate deeper optimization: a lightweight marker reader that parses the xref,
  resolves the catalog/pages tree just far enough to find page 1, reads only that page's `/Annots` and referenced
  annotation dictionaries, and falls back to full `lopdf` loading for encrypted, compressed-object, malformed, or
  otherwise unsupported PDFs. That is intentionally out of scope for this low-risk change.

## Validation

- `cargo build`
- `cargo clippy --all-targets`
- `cargo test`
- Targeted test runs for the new marker-page fixtures while developing.
- Optional real-library timing before and after, reported with enough context to distinguish this speedup from the
  already-landed one-read and multi-job scan changes.

## Risks & Mitigations

- **Behavior change for PDFs with only later-page notes**: make the error explicit about page 1 and cover it in tests.
- **Assuming `page_iter().next()` is cheap enough**: confirmed from lopdf 0.40 source that `get_pages()` eagerly
  collects `page_iter()`, while `page_iter()` itself is lazy over the page tree.
- **Overstating performance gains**: keep the implementation scoped and report honestly that full `load_mem` parsing
  remains. Treat a true partial-reader as a separate follow-up.
- **Marker ordinal regression**: compute `marker_note` within page 1 using the existing standalone-note filter and test
  the value where relevant.
