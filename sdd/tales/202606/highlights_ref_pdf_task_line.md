---
create_time: 2026-06-03 22:32:16
status: pending
prompt: sdd/prompts/202606/highlights_ref_pdf_task_line.md
---

# Plan: Render Generated PDF Links as Obsidian Tasks

## Context

`bob highlights-ref` generates Bob reference notes from Highlights PDF marker notes. New reference-note body rendering
is centralized in `src/native/highlights_ref/mod.rs` in `default_note_body()`.

The current generated note header is:

```md
# <title>

PDF: [[<source_pdf>]] ^pdf

## Highlights
```

The requested behavior is to make that PDF link line a proper Obsidian Tasks item:

```md
- [ ] #task [[<source_pdf>]] ^task
```

This removes the `PDF:` label and replaces the stable block ID from `^pdf` to `^task`.

This is not a CLI surface change. No new `bob highlights-ref` subcommands, flags, or option semantics are being added,
so the CLI-rules long-term memory is not required for this work.

## Current Behavior and Relevant Touch Points

- `src/native/highlights_ref/mod.rs`
  - `ParsedNote::render_body()` calls `default_note_body()` only when the target reference note does not already exist.
  - Existing notes keep their current body except for managed highlight-region replacement. This means this change will
    affect newly generated reference notes, not automatically migrate existing note headers.
  - `default_note_body()` currently writes the `PDF: [[...]] ^pdf` line.
- `tests/cli.rs`
  - `highlights_ref_sync_creates_note_frontmatter_from_marker_pdf_note` asserts the generated top-level PDF line.
  - `highlights_ref_sync_renders_sidecar_highlights_and_notes` asserts the generated nested-library PDF line.
- `docs/highlights-ref-sync.md`
  - The generated-note body documentation currently describes a PDF wikilink line with stable `^pdf` block ID. It should
    describe the generated Obsidian task line and stable `^task` block ID instead.
- `sdd/tales/202606/highlights_ref_pdf_block_id.md`
  - This is historical context for the prior `^pdf` change. It should not be edited as part of this implementation
    unless explicitly requested, because it records an already-completed prior plan.

## Product Semantics

For newly generated notes, emit:

```md
- [ ] #task [[lib/books/example.pdf]] ^task
```

The line should have these exact properties:

- It starts at the beginning of the line with `- [ ]`.
- It includes the `#task` tag before the PDF wikilink.
- It keeps the existing source PDF wikilink target exactly as currently derived from `source_pdf`.
- It uses the stable literal block ID `^task`.
- It no longer includes the `PDF:` label.
- It remains between the title and `## Highlights`, with the surrounding blank-line layout unchanged.

Existing reference notes should not be rewritten just to convert `PDF: [[...]] ^pdf` into a task line. That would be a
separate migration behavior and would risk changing user-managed body content outside the managed highlights region.

## Implementation Steps

1. Update `default_note_body()` in `src/native/highlights_ref/mod.rs`.
   - Replace the current `PDF: [[<source_pdf>]] ^pdf` construction with `- [ ] #task [[<source_pdf>]] ^task`.
   - Preserve the existing blank line before `## Highlights`.

2. Update integration coverage in `tests/cli.rs`.
   - Change the top-level generated-note assertion in
     `highlights_ref_sync_creates_note_frontmatter_from_marker_pdf_note` to expect:
     `- [ ] #task [[lib/systems-performance.pdf]] ^task`
   - Change the nested sidecar assertion in `highlights_ref_sync_renders_sidecar_highlights_and_notes` to expect:
     `- [ ] #task [[lib/books/systems-performance.pdf]] ^task`
   - Keep these as generated-note assertions; do not add migration assertions unless implementation scope changes.

3. Update documentation in `docs/highlights-ref-sync.md`.
   - Replace the `^pdf` generated-note description with wording that says new generated notes include a PDF wikilink
     Obsidian task line with stable `^task` block ID.
   - Avoid implying existing notes are automatically converted.

4. Run focused verification.
   - Run `cargo fmt --check`.
   - Run `cargo test highlights_ref_sync_creates_note_frontmatter_from_marker_pdf_note`.
   - Run `cargo test highlights_ref_sync_renders_sidecar_highlights_and_notes`.
   - If the focused tests reveal nearby behavior changes, run the broader `cargo test highlights_ref` subset.

## Risks and Edge Cases

- Obsidian block IDs must be unique within a note. A generated note has one generated PDF task line, so `^task` is
  stable unless a user manually creates another `^task` block in the same note.
- `#task` and `- [ ]` can make the line visible to Obsidian Tasks queries. That is the requested behavior, but it also
  means generated reference notes may now appear in task-oriented workflows.
- Existing notes will continue to have whatever body header they already had unless regenerated from scratch. This is
  consistent with the current body-preservation model.
- The machine-readable `source_pdf` frontmatter should remain unchanged; only the human-facing body line changes.

## Acceptance Criteria

- New `bob highlights-ref sync <pdf>` reference notes contain `- [ ] #task [[...]] ^task` near the top of the body.
- The generated line no longer contains `PDF:` or `^pdf`.
- The managed highlights region still begins after `## Highlights` as before.
- Existing reference notes are not force-migrated during normal sync.
- Focused highlights-ref tests pass.
