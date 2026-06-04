---
create_time: 2026-06-03 22:53:58
status: pending
prompt: sdd/prompts/202606/highlights_ref_task_done_status.md
---

# Plan: Sync Checked Highlights PDF Tasks to Done Status

## Context

`bob highlights-ref` now creates new reference notes with a PDF task line:

```md
- [ ] #task [[lib/books/example.pdf]] ^task
```

That line gives the user a natural Obsidian Tasks affordance for marking an article, paper, or book as read. The
requested next behavior is:

- when the generated PDF task line is checked, treat the reference as `done`;
- update the reference note frontmatter `status` field to `done`;
- write `- status: done` back to the PDF marker note when PDF writes are explicitly enabled.

The existing `highlights-ref` implementation already has a strong marker/frontmatter synchronization model:

- the PDF marker note and reference-note frontmatter are represented as synced projections;
- projections are hashed and stored as `highlights_marker_hash` and `highlights_marker_base`;
- marker/frontmatter conflicts are detected with a three-way merge;
- PDF marker writes are opt-in through `sync --write-pdf`;
- `scan` remains note-oriented and never enables PDF marker write-back.

The new checked-task signal should fit into that projection model instead of being implemented as an after-the-fact
string rewrite. That keeps hashes, conflict detection, dry-run reporting, and write safety coherent.

This plan does not add a new subcommand, flag, or CLI option, so `memory/long/cli_rules.md` is not required. I did use
the required audited Obsidian memory read because this feature depends on Bob vault note, frontmatter, and task workflow
semantics.

## Product Semantics

1. Recognize the generated PDF task line in existing reference-note bodies.
   - The intended line is a Markdown checkbox task containing `#task`, a PDF wikilink, and the stable `^task` block ID.
   - A checked checkbox (`[x]` or `[X]`) means the reference should be marked `status: done`.
   - An unchecked checkbox (`[ ]`) is not enough information to infer a non-done status, so it should not by itself
     change `status` away from `done`.

2. Treat a checked PDF task as a status-only user signal.
   - It contributes exactly one synced projection field: `status: done`.
   - It should compose with marker/frontmatter changes to other fields.
   - It should be idempotent after the marker, frontmatter, and stored base all agree on `status: done`.

3. Preserve the existing PDF write safety contract.
   - `bob highlights-ref sync <pdf> --dry-run` should preview the status and marker updates without writing.
   - `bob highlights-ref sync <pdf>` should fail without modifying either file when the checked task requires a PDF
     marker update and `--write-pdf` was not supplied.
   - `bob highlights-ref sync <pdf> --write-pdf` should update both the PDF marker and the reference note.
   - `bob highlights-ref scan --dry-run` should report would-update marker work.
   - `bob highlights-ref scan` should continue to refuse work that would need PDF marker writes, rather than silently
     writing PDFs during a library scan.

4. Keep task-line body ownership narrow.
   - Existing notes should not be bulk-migrated from old `PDF: ... ^pdf` lines to the new task line as part of this
     change.
   - Notes without a recognizable `^task` PDF task line simply have no task-completion signal.
   - For notes with the generated `^task` line, the command may update only the checkbox marker on that exact line so
     the body task reflects `status: done`.
   - The managed Highlights region remains the only large body region the command owns.

5. Define conflict behavior explicitly.
   - If the stored base has `status: wip`, marker/frontmatter are unchanged, and the task is checked, select
     `status: done`.
   - If marker/frontmatter have compatible non-status edits and the task is checked, auto-merge those edits with
     `status: done`.
   - If marker or frontmatter changed `status` to a different non-`done` value while the task is checked, fail without
     writes and report a status conflict. The user can resolve by unchecking the task or making the authoritative
     marker/frontmatter status `done`.
   - Existing `--prefer marker` and `--prefer frontmatter` should still resolve marker/frontmatter disagreements, but
     they should not silently ignore a checked task-line signal.

## Implementation Design

### Task-Line Parsing

Add a small parser in `src/native/highlights_ref/mod.rs` for the reference-note body PDF task line.

The parser should:

- scan note body lines outside frontmatter;
- identify candidate lines containing the stable `^task` block ID and a checkbox task marker;
- require `#task` and a PDF wikilink on the same line before treating it as the generated PDF task;
- report the line state as missing, unchecked, checked, or malformed/ambiguous;
- reject multiple recognizable `^task` PDF task lines, because duplicate block IDs make completion state ambiguous.

Keep this parser intentionally narrower than a full Obsidian Tasks parser. The feature only needs to recognize the
generated affordance.

### Projection Resolution

Extend planning so `plan_pdf_sync()` derives a task-completion signal from the parsed note body before finalizing the
synced projection.

Implementation shape:

1. Parse marker projection as today.
2. Parse frontmatter projection as today.
3. Resolve marker/frontmatter with the existing hash/base algorithm.
4. Apply the checked-task signal as a status-only contribution:
   - if the task is missing or unchecked, leave the selected projection alone;
   - if the selected projection has no status conflict, set `status` to `done`;
   - if marker/frontmatter made a competing non-`done` status change from the stored base, return a clear conflict
     error.
5. Validate the final projection with existing required-field and allowed-status validation.
6. Compute `synced_hash`, `rendered_marker`, marker write necessity, rendered note, and note action from that final
   projection.

The plan object should keep enough metadata to explain and safely execute the new behavior, for example:

- whether a generated PDF task was found;
- whether it was checked;
- whether the checked task contributed `status: done`;
- whether the rendered note body will update the exact task checkbox.

`sync_reason` can include the task contribution, or a new report line can be added. The important user-facing property
is that dry runs make it obvious why a `status: done`/PDF marker update is planned.

### Body Rendering

Update body rendering narrowly:

- `default_note_body()` should render the generated task line as checked when the selected projection has
  `status: done`; otherwise render it unchecked.
- For existing notes, after any managed-region replacement, rewrite only the exact generated PDF task line checkbox to
  match whether the selected projection status is `done`.
- If no generated task line is present, leave the body unchanged.
- If the generated task line is malformed or duplicated, fail before writes rather than guessing.

This gives consistent output when status becomes `done` from any source while still avoiding broad body migrations.

### Dirty Worktree Safety

The normal user workflow will dirty the reference note body by changing:

```md
- [ ] #task [[...]] ^task
```

to:

```md
- [x] #task [[...]] ^task
```

The current dirty-file allowance only permits tracked frontmatter-only edits. Without an update, a checked task in a
clean Git worktree would be rejected as a dirty target note.

Extend the dirty-note allowance in `dirty_entry_allowed_for_plans()` so it also allows the exact generated task checkbox
toggle, optionally combined with frontmatter-only edits.

The allowance should stay strict:

- tracked modified files only, as today;
- target path must still be under the configured `ref_dir`;
- current file contents must match the planner's read snapshot;
- compare `HEAD` to current and permit only:
  - frontmatter changes, and/or
  - a single generated PDF task checkbox change on the recognized `^task` line;
- no arbitrary manual body edits, managed-region edits, added paragraphs, or unrelated block-ID changes should be
  allowed through this special case.

Untracked notes and staged changes should remain refused by the existing dirty write safety behavior.

### PDF Marker Write-Back

No new write path is needed. Once the final projection contains `status: done`, the existing `render_marker()` and
`write_pdf_marker()` path can update the marker note.

Keep the existing guard:

- if marker write is needed and the command is not dry-run and `--write-pdf` is absent, fail before writes;
- when `--write-pdf` is supplied, write the PDF first, recompute the source PDF SHA-256, then render/write the note with
  the updated pipeline metadata.

This preserves the existing atomicity model and metadata correctness.

## Implementation Steps

1. Add task-line parsing helpers and unit tests in `src/native/highlights_ref/mod.rs`.
   - Cover missing, unchecked, checked, malformed, and duplicate `^task` candidates.

2. Add task completion metadata to the sync planning data model.
   - Keep this scoped to `PdfSyncPlan`/`SyncDecision` or a small adjacent struct.
   - Avoid changing CLI options.

3. Extend projection resolution with the checked-task contribution.
   - Support the simple unchanged-marker/frontmatter case.
   - Support compatible auto-merge with non-status fields.
   - Reject checked-task conflicts with non-`done` status edits.

4. Update note body rendering.
   - Render new notes with `[x]` when selected status is `done`.
   - For existing notes, update only the exact generated `^task` checkbox when present.
   - Preserve all other body content outside the managed highlights region.

5. Extend dirty target safety.
   - Add a helper that detects changes confined to frontmatter and/or the exact generated task checkbox toggle.
   - Use it from `dirty_entry_allowed_for_plans()` for planned task-completion writes.

6. Update integration tests in `tests/cli.rs`.
   - Checked task dry run previews `note_action: update` and `pdf_marker_action: would-update` without modifying files.
   - Checked task without `--write-pdf` fails and leaves note/PDF unchanged.
   - Checked task with `--write-pdf` updates frontmatter, PDF marker, keeps the task checked, refreshes
     `source_pdf_sha256`, and settles on a repeat sync.
   - A tracked note dirtied only by the generated task checkbox is allowed by write safety.
   - A checked task plus a competing marker/frontmatter status edit fails without writes.
   - A marker status change to `done` checks the generated task line if it is present.

7. Update `docs/highlights-ref-sync.md`.
   - Document the generated `^task` line as a status completion affordance.
   - Explain that checked means `status: done`; unchecked alone does not infer a replacement status.
   - Reiterate that PDF marker write-back still requires targeted `sync --write-pdf` and that `scan` does not write
     PDFs.
   - Clarify that existing notes without the generated task line are not migrated by this feature.

8. Run focused verification.
   - `cargo fmt --check`
   - `cargo test highlights_ref_task`
   - `cargo test highlights_ref_sync`
   - `cargo test highlights_ref_scan`
   - If the focused subsets are noisy or miss renamed tests, run `cargo test highlights_ref`.
   - Run `git diff --check`.

## Acceptance Criteria

- Checking the generated PDF task line in a ref note causes `bob highlights-ref sync <pdf> --write-pdf` to write
  `status: done` in both the reference note frontmatter and the PDF marker note.
- The same checked-task state is visible in dry-run output without modifying the note or PDF.
- Running without `--write-pdf` refuses before writes whenever the checked task requires a PDF marker update.
- A checked generated task line is allowed as a safe tracked dirty-note edit; unrelated body edits are still refused.
- Re-running sync after a successful write reports no further note or marker changes.
- Existing notes without the generated `^task` line are not bulk-migrated.
- Existing marker/frontmatter status validation and conflict behavior remain intact.

## Risks and Mitigations

- Risk: treating an arbitrary task as the PDF completion task. Mitigation: require the stable `^task` block ID plus
  `#task` and a PDF wikilink, and reject duplicates/malformed candidates.

- Risk: checked task plus marker/frontmatter status edits silently chooses the wrong source. Mitigation: model the
  checked task as a status-only projection contribution and fail on competing non-`done` status changes.

- Risk: allowing dirty body edits weakens the write safety model. Mitigation: permit only the exact generated task
  checkbox toggle, optionally with frontmatter changes already allowed by the current workflow.

- Risk: rewriting body content outside the managed highlights region surprises users. Mitigation: rewrite only the
  checkbox marker on the exact generated `^task` line; do not migrate old `PDF:` lines or otherwise rearrange note
  headers.

- Risk: library `scan` starts writing PDFs unexpectedly. Mitigation: keep `scan`'s existing `write_pdf: false` behavior.
  Use dry run to discover checked tasks, then targeted `sync --write-pdf` for PDF marker updates.
