---
create_time: 2026-06-08 08:45:16
status: wip
prompt: sdd/prompts/202606/highlight_task_h_property.md
---
# Plan: Restore property-based highlight task tracking with `h`

## Objective

Stop generating task-specific `^ht-...` block IDs for annotation-created tasks because those block references do not
resolve reliably in Obsidian. Restore the earlier property-based processed marker, but write it as the short inline
Dataview property `[h:: ...]` instead of `[highlight_task:: ...]`.

The end state should be:

- Created annotation tasks link back to the annotation-level generated highlight/note block (`^h-...`), not to a
  task-specific `^ht-...` block.
- Created annotation tasks include a stable processed marker `[h:: <id>]` plus `[created::YYYY-MM-DD]`.
- Existing tasks with old `[highlight_task:: <id>]` remain recognized.
- Existing tasks created during the `^ht` era remain recognized so sync does not duplicate them, but new sync output no
  longer writes `^ht` anchors or `#^ht` links.

## Current Behavior

The current implementation made `^ht-...` source-task anchors the durable processed marker:

- `AnnotationTaskCandidate` carries both `source_task_block_id` and `legacy_highlight_task_id`.
- Rendered highlight comment/note task source lines get appended `^ht-...` block IDs.
- Created tasks link to `[[ref/...#^ht-...|🔖]]`.
- New tasks omit `[highlight_task:: ...]`.
- The processed-task index primarily scans `#^ht-...` backlinks and keeps `[highlight_task:: ...]` only as legacy
  compatibility.

That is the wrong primary mechanism if Obsidian cannot link to those generated `^ht` blocks.

## Implementation Plan

1. Rename and clarify task processed-marker constants.
   - Replace the new-task field key with `HIGHLIGHT_TASK_FIELD: "h"`.
   - Keep a separate `LEGACY_HIGHLIGHT_TASK_FIELD: "highlight_task"` for compatibility.
   - Keep the existing deterministic digest inputs and version (`v1`) so the value remains stable across the property
     rename.

2. Reshape `AnnotationTaskCandidate` around a property id again.
   - Replace `source_task_block_id` / `legacy_highlight_task_id` as candidate primary fields with `processed_id`.
   - Keep `source_block_id` as the link target, because created tasks should point to the annotation-level `^h-...`
     block.
   - Preserve route behavior exactly: same-note tasks stay in the reference note; `@name` routed tasks append to the
     existing root note.

3. Restore annotation-level source links in created task lines.
   - Same-note tasks should render a compact same-file link like `[[#^h-...|🔖]]`.
   - Routed tasks should render a vault-relative note link like `[[ref/books/example#^h-...|🔖]]`.
   - Render new tasks as `- [ ] <task text> <source link>[h:: <processed_id>] [created::<date>]`.
   - Keep the link before the processed/created fields so task identity stripping remains stable.

4. Stop rendering task-specific source anchors in managed highlight blocks.
   - Remove the source-task-anchor map/rendering path that appends `^ht-...` to blockquoted comment/note lines.
   - Keep annotation-level `^h-...` generated blocks unchanged.
   - This should avoid introducing duplicate or unresolved task-line block IDs in the managed region.

5. Update processed-task indexing with compatibility layers.
   - Primary processed ids come from `[h:: ...]`.
   - Legacy processed ids also come from `[highlight_task:: ...]`.
   - Continue scanning existing `#^ht-...` backlinks as a transitional compatibility marker for tasks created by the bad
     `^ht` version; this prevents duplicate task creation for edited, moved, completed, cancelled, or archived tasks
     that have no property.
   - Keep the existing normalized-identity fallback for older link/property-less tasks.

6. Update insertion/idempotency behavior.
   - `ProcessedTaskIndex::accept()` should reject candidates whose `processed_id` appears in either `[h::]` or
     `[highlight_task::]`.
   - It should also reject candidates whose derived old `ht-...` anchor appears in an existing source backlink, for
     compatibility with tasks already emitted by commit `e136541`.
   - The test-only insertion helper should assert the same rendered shape as production task rendering.

7. Update tests.
   - Change CLI and unit expectations so new tasks contain `[h:: ...]` and no `[highlight_task:: ...]`.
   - Change source-link expectations from `#^ht-...` to annotation-level `#^h-...`.
   - Add or adjust coverage proving managed highlight blocks no longer contain `^ht-...`.
   - Keep a regression showing old `[highlight_task:: ...]` tasks are recognized and not recreated.
   - Add compatibility coverage showing an existing task with only a `#^ht-...` backlink is not recreated.
   - Preserve the recently fixed checked-`^task` closing-order regression.

8. Update documentation.
   - README and `docs/highlights-ref-sync.md` should describe `[h:: ...]` as the durable processed marker.
   - Document that `[highlight_task:: ...]` and old `#^ht-...` links are compatibility inputs only.
   - Replace examples with `^h-...` source backlinks and `[h:: ...]`.
   - Remove text claiming new tasks no longer carry a processed property or that `^ht` backlinks are the durable marker.

9. Verification.
   - Run `cargo fmt --check`.
   - Run focused CLI tests around annotation task creation, routing, legacy property recognition, checked-`^task`
     closing order, and scan/write behavior.
   - Run `cargo test highlights_ref`.
   - Run the full `cargo test`.
   - Run `cargo clippy --all-targets`.

## Non-Goals

- Do not bulk-edit existing user tasks to rename `[highlight_task:: ...]` to `[h:: ...]`.
- Do not bulk-edit existing `#^ht-...` backlinks.
- Do not change PDF `^task` status semantics, annotation-level `^h-...` IDs, marker/frontmatter projection rules, route
  syntax, or task completion/cancellation preservation.
