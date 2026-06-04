---
create_time: 2026-06-03 20:40:49
status: wip
prompt: sdd/prompts/202606/highlights_ref_pdf_block_id.md
---
# Plan: Add `^pdf` Block ID to Generated `bob highlights-ref` PDF Lines

## Context

`bob highlights-ref` generates Bob reference notes from Highlights PDF marker notes. New note body rendering is
centralized in `src/native/highlights_ref/mod.rs` in `default_note_body()`. That function currently emits the body
header as:

```md
# <title>

PDF: [[<source_pdf>]]

## Highlights
```

The request is to include `^pdf` at the end of that `PDF:` line so Obsidian users can link or transclude the PDF line
directly, for example via `[[some-ref#^pdf]]` or `![[some-ref#^pdf]]`.

This is not a CLI surface change: no subcommands or flags are being added, so the CLI-rules long-term memory does not
need to be consulted for this work.

## Current Behavior and Relevant Touch Points

- `src/native/highlights_ref/mod.rs`
  - `ParsedNote::render_body()` calls `default_note_body()` only when the target reference note does not exist.
  - Existing notes keep their current body except for managed highlight-region replacement. This means the change will
    naturally affect newly generated reference notes, not retrofit every existing note during ordinary sync.
  - `default_note_body()` writes the `PDF: [[...]]` line.
- `tests/cli.rs`
  - `highlights_ref_sync_renders_sidecar_highlights_and_notes` currently asserts the exact generated `PDF:` line for a
    nested library PDF.
  - `highlights_ref_sync_creates_note_frontmatter_from_marker_pdf_note` verifies new-note creation and frontmatter, but
    does not currently assert the body PDF line for top-level PDFs.
- `docs/highlights-ref-sync.md`
  - Describes generated notes as including a title, PDF wikilink, and `## Highlights`, but does not show the full
    header. It should mention the stable `^pdf` block ID because it is now part of the generated note contract.

## Product Semantics

Generate this line for new notes:

```md
PDF: [[lib/books/example.pdf]] ^pdf
```

The block ID should be stable and literal:

- Always use `^pdf`, not a hash-derived ID.
- Place it on the same line as the PDF wikilink, after one separating space.
- Keep the existing `source_pdf` frontmatter unchanged.
- Keep managed highlight block IDs unchanged.
- Do not alter existing reference-note bodies unless they are being generated from scratch. Existing users can add
  `^pdf` manually if they want it in old notes; broad migration would be a separate behavior change.

## Implementation Steps

1. Update `default_note_body()` in `src/native/highlights_ref/mod.rs`.
   - Change the emitted line from `PDF: [[<source_pdf>]]` to `PDF: [[<source_pdf>]] ^pdf`.
   - Keep surrounding blank lines unchanged so the existing note body layout remains stable.

2. Strengthen integration coverage in `tests/cli.rs`.
   - Update the existing nested sidecar assertion in `highlights_ref_sync_renders_sidecar_highlights_and_notes` to
     expect `PDF: [[lib/books/systems-performance.pdf]] ^pdf`.
   - Add or extend a top-level PDF creation assertion in
     `highlights_ref_sync_creates_note_frontmatter_from_marker_pdf_note` so both top-level and nested library paths are
     covered.
   - Keep the assertions focused on generated notes, since existing notes intentionally preserve manual body content.

3. Update docs in `docs/highlights-ref-sync.md`.
   - Amend the generated-note body description to say the PDF wikilink line includes a stable `^pdf` block ID for direct
     Obsidian linking/transclusion.
   - Avoid implying that existing notes are migrated automatically.

4. Run focused verification.
   - Run `cargo test highlights_ref_sync_creates_note_frontmatter_from_marker_pdf_note`.
   - Run `cargo test highlights_ref_sync_renders_sidecar_highlights_and_notes`.
   - If either targeted test suggests nearby regressions, run the broader `cargo test highlights_ref` subset.

## Risks and Edge Cases

- Obsidian block IDs should be unique within a note. A generated note has one PDF line, so `^pdf` is stable and should
  not collide in newly generated notes unless a user manually adds another `^pdf` elsewhere afterward.
- Existing notes are intentionally not rewritten wholesale by `render_body()`; changing that would risk overwriting
  manual body edits and is outside this request.
- The `source_pdf` frontmatter remains the machine-readable source path. The body line remains a user-facing convenience
  link.

## Acceptance Criteria

- New `bob highlights-ref sync <pdf>` reference notes contain `PDF: [[...]] ^pdf` near the top of the body.
- The generated highlights region still begins after `## Highlights` exactly as before.
- Existing reference notes without `^pdf` are not force-migrated during normal sync.
- Focused highlights-ref tests pass.
