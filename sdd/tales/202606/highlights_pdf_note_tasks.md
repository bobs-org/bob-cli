---
create_time: 2026-06-07 10:48:49
status: wip
prompt: sdd/prompts/202606/highlights_pdf_note_tasks.md
---
# Plan: Create Obsidian Tasks from PDF Note Bullets

## Context

`bob highlights` is implemented in `src/native/highlights_ref/mod.rs`. It already:

- reads the first page-1 standalone `/Text` PDF annotation as the marker note;
- syncs marker/frontmatter state into a reference note under `ref/`;
- renders a top-level generated PDF reading-status task line with the stable `^task` block ID;
- renders Highlights sidecar annotations into the managed body region between `<!-- highlights:begin -->` and
  `<!-- highlights:end -->`;
- represents highlight comments as `SidecarAnnotation.comment` and standalone non-marker notes as
  `SidecarAnnotationKind::StandaloneNote`.

The requested behavior is not a new CLI surface. It adds behavior to existing `bob highlights sync` and
`bob highlights scan`, so `memory/long/cli_rules.md` is not required.

## Goal

When a PDF note/comment contains one or more Markdown bullet lines tagged with `#task`, create corresponding top-level
Obsidian task lines in the reference note for that PDF.

Example source bullet:

```md
- #task Foo bar baz
```

Expected created task:

```md
- [ ] #task Foo bar baz [created::2026-06-07]
```

The new task lines should appear immediately after the generated PDF reading-status task line that carries `^task`, with
no indentation, so they are sibling top-level tasks rather than sub-bullets or subtasks.

## Non-Goals

- Do not add a new subcommand, option, environment variable, or interactive prompt.
- Do not reinterpret untagged bullets, `TODO:` prose, or arbitrary note text as tasks.
- Do not make completed generated annotation tasks update PDF annotations or marker/frontmatter status.
- Do not change the existing `^task` reading-status semantics.
- Do not place task metadata in note frontmatter.

## Source Parsing

Add a small task-extraction layer around the existing annotation model.

Task candidates come from:

- highlight comments, using `SidecarAnnotation.comment`;
- standalone non-marker sticky-note-style notes, using `SidecarAnnotation.text` when `kind == StandaloneNote`.

The parser should inspect each source note/comment line-by-line before rendered blockquote prefixes such as `[comment] `
or `[note] ` are added. A task source line is an unordered Markdown bullet whose item body contains `#task` as a
whitespace-delimited Markdown token.

Accepted input shapes should include:

```md
- #task Foo

* #task Foo

- [ ] #task Foo
- [x] #task Foo
```

The rendered created task should normalize all accepted source shapes to an unchecked top-level task:

```md
- [ ] #task Foo [created::YYYY-MM-DD]
```

Multiple matching bullet lines in one PDF note/comment should produce multiple task candidates. Non-task bullets should
continue to render as comments/notes exactly as they do today.

## Creation and Idempotency

Treat this as task creation, not managed task regeneration.

Implementation policy:

- Build task candidates during `plan_pdf_sync` after sidecar parsing and before rendering the final note body.
- Render missing tasks into the note body immediately after the existing generated PDF `^task` line.
- Preserve all existing task lines once created, including checkbox state and user-added fields such as
  `[completion::]`, `[cancelled::]`, `[due::]`, or edited priority fields.
- Avoid duplicates on repeated syncs by comparing normalized task identity, not by byte-for-byte line equality.
- Suggested identity: normalize the task text after stripping the list marker, optional source checkbox, and existing
  `[created::YYYY-MM-DD]` field; collapse whitespace; require the remaining text to include the `#task` token.
- Existing `- [x] #task Foo [created::...]` or `- [-] #task Foo [created::...]` should count as already created and must
  not be recreated.
- If a user edits the task text, treat the source bullet as missing and create a new task on the next sync. That is
  safer than trying to reconcile arbitrary user edits without stable task IDs.

Use the local current date for `[created::YYYY-MM-DD]`, matching Obsidian task-property style and the user example.

## Body Placement

Add a helper that updates rendered/existing note bodies after the normal `render_body` path:

1. Parse the body for exactly one generated PDF task line with `^task` using the existing `parse_pdf_task_line`
   validation.
2. Insert any missing created task lines immediately after that line.
3. Keep inserted lines unindented and separated only by normal newlines.
4. For existing notes, do not move manual sections or the managed Highlights region; only add missing task lines after
   `^task`.
5. For new notes, the default body already contains `^task`, so the same helper can be used.

This keeps the existing managed Highlights region ownership unchanged. The generated highlight/note block rendering may
still show the original source bullet as quoted/comment content; the new top-level task is the actionable copy.

## Implementation Steps

1. Introduce a small internal `AnnotationTaskCandidate` type.
   - Store at least the normalized task text and rendered task text.
   - Derive candidates from sidecar annotations after skipping marker mirrors.

2. Add task-line parsing helpers.
   - Recognize unordered bullets.
   - Strip optional Markdown task checkbox from source bullets.
   - Detect `#task` with the existing whitespace-token approach.
   - Normalize identities for duplicate detection.

3. Add note-body insertion helpers.
   - Reuse `parse_pdf_task_line` to find the `^task` line.
   - Collect existing top-level task identities from the note body.
   - Insert only candidates whose identity is absent.
   - Append `[created::YYYY-MM-DD]` only to newly created lines.

4. Wire the helper into `plan_pdf_sync`.
   - Keep the existing `render_body` behavior for frontmatter sync, PDF task checkbox rewrites, managed region
     replacement, and tombstones.
   - Apply task insertion to the rendered body before `render_with_projection`.
   - Ensure dry-run reports naturally show `note_action: update` when new tasks would be created and `writes: none`
     remains honored.

5. Update docs.
   - Extend `README.md` and `docs/highlights-ref-sync.md` to describe `#task` bullet extraction from highlight comments
     and sticky-note-style standalone notes.
   - Clarify that created annotation tasks are top-level sibling tasks under the generated `^task` line and are not used
     for reading-status sync.

6. Add focused tests.
   - Highlight comment with `- #task ...` creates a top-level `- [ ] #task ... [created::YYYY-MM-DD]` after the `^task`
     line.
   - A standalone non-marker note with multiple task bullets creates multiple top-level tasks.
   - Non-task bullets remain rendered as comments/notes and do not create tasks.
   - Re-running `bob highlights sync` does not duplicate created tasks.
   - Completed or canceled existing created tasks are preserved and not recreated.
   - Existing manual sections and the managed Highlights region remain in place.

## Verification

Run focused checks first:

```bash
cargo fmt --check
cargo test highlights_ref_sync_renders_sidecar_highlights_and_notes
cargo test highlights_ref_sync_supports_linked_sidecar_style
```

Then run the broader highlights subset:

```bash
cargo test highlights_ref
```

If task-date assertions need the actual current date, make the tests compute the expected local date rather than
hard-code `2026-06-07`, while keeping docs/examples concrete.

## Risks

- Existing notes might contain user-authored `^task` duplicates. The existing parser already rejects duplicate generated
  PDF task lines, so this feature should preserve that safety check.
- Without stable block IDs for created annotation tasks, idempotency depends on normalized text. That matches the
  user-facing example and avoids adding hidden syntax, but edited task text will intentionally create a new task.
- Direct PDF annotation extraction beyond the current sidecar/marker paths is larger than this feature. If sticky-note
  comments are not present in sidecars, a follow-up should extend PDF annotation scanning to non-marker `/Text`
  annotations across pages without changing marker semantics.
