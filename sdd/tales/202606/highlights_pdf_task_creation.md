---
create_time: 2026-06-07 10:32:52
status: wip
prompt: sdd/prompts/202606/highlights_pdf_task_creation.md
---
# Plan: Add PDF Note Task Creation to `bob highlights`

## Objective

Extend `bob highlights sync` and `bob highlights scan` so user-authored PDF note bullets containing the `#task` tag
create corresponding task bullets in the PDF's reference note. A source bullet such as:

```markdown
- #task Foo bar baz
```

should create a line like:

```markdown
- #task Foo bar baz [created::2026-06-07]
```

under the existing generated PDF task line identified by `^task`.

This is not a CLI surface change. No new subcommands, flags, or option semantics are needed.

## Current Context

The native Highlights workflow is centralized in `src/native/highlights_ref/mod.rs`.

The current sync pipeline:

- reads the first standalone page-1 `/Text` PDF note as the marker/frontmatter source;
- reads Highlights Markdown sidecars (`.md` or `.textbundle/text.md`) when present;
- parses sidecar highlights into `SidecarAnnotation` values;
- renders highlights and notes into the managed `<!-- highlights:begin -->` region;
- creates a generated PDF reading-status task line near the top of the reference note:

```markdown
- [ ] #task [[lib/example.pdf]] [p::2] ^task
```

Highlight comments currently render as blockquote comments with a `[comment] ` prefix. Standalone sticky-note style
sidecar notes render as `[note] `. Linked Highlights sidecar comments exported as Markdown list items are normalized by
stripping the list marker before rendering the `[comment]` line.

The new feature should hook into the sidecar/PDF-note parsing and note-body rendering path, not introduce a separate
command.

## Source Task Grammar

Recognize task source bullets in user-authored note text when every source line is in one of the PDF note surfaces:

- highlight comments associated with a highlighted quote;
- standalone sticky-note comments/notes, excluding the first marker mirror note that is already reserved for metadata.

Task bullets should be detected before existing comment list-marker stripping loses the fact that the user wrote a
bullet. Supported source bullets:

```markdown
- #task Foo

* #task Foo
```

The parser should treat `#task` as a Markdown token, not as a substring inside another word or tag. Multiple task
bullets in one comment/note should all be captured. Non-task bullets should keep the current comment/note rendering
behavior.

I will keep the initial scope to bullets containing `#task`, matching the prompt. I will not convert arbitrary `TODO:`
text or non-`#task` bullets into tasks.

## Destination Rendering

Tasks should be added under the existing generated PDF task line (`^task`) in the corresponding reference note.
Concretely, the renderer should find the generated PDF task line and insert missing task bullets as child lines below
it, before the next non-child top-level content.

The destination line should preserve the source task text and append `[created::YYYY-MM-DD]` when the task is first
created. The date should be the command's current local date; tests can assert the shape rather than a hard-coded
wall-clock value where needed.

Example destination block:

```markdown
- [ ] #task [[lib/example.pdf]] [p::2] ^task
  - #task Foo bar baz [created::2026-06-07]

## Highlights
```

The source highlight/comment/note should continue to render in the managed Highlights region so the task's provenance
remains visible in the PDF annotation context.

## Idempotence and Preservation

This feature should create missing tasks, not continuously re-render a managed task list.

Policy:

- Do not duplicate a task on repeated `sync` or `scan`.
- Do not delete destination task bullets when a PDF note is later removed or changed.
- Do not rewrite existing created dates or user-added task metadata.
- Detect existing destination tasks by normalizing the task text with task list marker, leading indentation, and
  `[created::...]` removed.
- Append only genuinely missing task bullets under `^task`.

This avoids resetting user progress and keeps the existing managed Highlights region as the only aggressively
regenerated body section.

## Implementation Shape

1. Add a small captured-task model near the sidecar annotation types.
   - Store the normalized source task text.
   - Keep enough source context for deterministic ordering: annotation order, task ordinal within the annotation, and
     page label if helpful for tests or future reporting.

2. Extract tasks from sidecar annotation note text.
   - For highlight annotations, scan `annotation.comment`.
   - For standalone sticky-note annotations, scan `annotation.text`.
   - Run extraction after marker-mirror detection so the page-1 marker mirror is not treated as a task source.
   - Ensure list-marker stripping for normal comments still behaves as existing tests expect.

3. Thread captured tasks through planning.
   - Extend the `RenderedHighlights` or nearby planning data to include captured tasks, or return a sibling structure
     from sidecar rendering.
   - Keep `highlights_count` tied to rendered annotation block IDs, not captured task count, unless tests reveal current
     behavior expects otherwise.

4. Add task insertion to `ParsedNote::render_body`.
   - For new notes, have `default_note_body()` render the generated `^task` line and then any captured task child
     bullets before `## Highlights`.
   - For existing notes, after replacing the managed highlights region and rewriting the PDF task checkbox, insert only
     missing captured task bullets under the existing `^task` line.
   - If the existing note has no `^task` line, rely on the current `parse_pdf_task_line`/rewrite behavior and avoid
     inventing a second parent line. The existing malformed/missing generated-task handling should remain intact.

5. Keep safety checks aligned with existing behavior.
   - Planned task insertions are note writes and should participate in `note_action`.
   - Git dirty-target checks should treat this as an ordinary note body change, not as the special "frontmatter or
     generated-task checkbox only" safe dirty exception.
   - PDF marker write-back should not be required merely because new note tasks were created.

6. Update docs.
   - Document that `#task` bullets in highlight comments and sticky notes create child bullets under `^task`.
   - Document idempotence: existing created tasks are preserved, not deleted, and repeated sync does not duplicate them.
   - Clarify that this is one-way from PDF notes/sidecar into the reference note and does not write task state back to
     PDFs.

## Test Plan

Add focused integration coverage in `tests/cli.rs` using existing PDF and sidecar helpers:

1. Highlight comment task creation.
   - Sidecar highlight with a comment containing `- #task Foo bar baz`.
   - Assert the reference note contains a child task under `^task` with `[created::YYYY-MM-DD]`.
   - Assert the comment still renders in the managed Highlights region with `[comment]`.

2. Multiple tasks in one note.
   - A single comment or standalone note with two `#task` bullets creates two destination task bullets in source order.

3. Standalone sticky-note task creation.
   - A non-marker standalone note containing `- #task Follow up` creates the destination task.
   - The first marker mirror note remains excluded.

4. Idempotence.
   - Run `sync` twice and assert no duplicate child task is added.
   - Pre-populate an existing created task under `^task` and assert its `[created::...]` value and extra metadata are
     preserved.

5. Non-task bullets remain comments.
   - A comment like `- Support sase tool call replay?` still renders as `[comment] Support...` and does not create a
     destination task.

6. Marker/PDF write behavior.
   - A new captured task causes a note write but does not require `--write-pdf` and does not change the PDF marker
     action.

Run verification:

```bash
cargo fmt --check
cargo test highlights_ref
```

If the focused subset is stable, run:

```bash
cargo test
cargo clippy --all-targets --all-features
```

## Acceptance Criteria

- `bob highlights sync <pdf>` creates child `#task` bullets under the reference note's `^task` line from `#task` bullets
  in highlight comments and sticky notes.
- Multiple task bullets in one source note are captured.
- Re-running sync does not duplicate tasks.
- Existing destination task metadata is preserved.
- New task creation does not require PDF marker write-back.
- Existing highlight/comment/sticky-note rendering tests continue to pass.
