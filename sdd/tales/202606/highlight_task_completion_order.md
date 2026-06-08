---
create_time: 2026-06-08 08:22:25
status: wip
prompt: sdd/prompts/202606/highlight_task_completion_order.md
---
# Plan: Process Highlight Tasks Before Closing PDF Status

## Problem

`bob highlights` currently decides whether to create annotation-derived Obsidian tasks from the final synced projection.
When the generated reference note `^task` line is checked, `apply_pdf_task_status_signal()` mutates that projection from
`status: wip` to `status: read` before `plan_pdf_sync()` decides whether to collect annotation task candidates.

That makes this workflow lose the final task-intake pass:

1. PDF marker/frontmatter are still `status: wip`.
2. The user adds `#task` bullets in PDF highlight comments or standalone notes.
3. The user checks the generated reference note `^task`.
4. `bob highlights sync --write-pdf` or `scan --write-pdfs` writes `read` to the reference note and PDF marker, but
   creates zero annotation tasks.
5. Future runs correctly skip task creation because the PDF is now `read`, so those task notes are never imported.

## Desired Behavior

Annotation task intake should be based on the PDF's selected status before the generated reference-note `^task` checkbox
contributes a closing status. If the marker/frontmatter resolution says the PDF was `wip`, a run that checks `^task` to
`read` should still create all new annotation tasks in that same run, then write the final `status: read` projection and
PDF marker. Future runs should see `read` and skip annotation task creation.

The same pre-signal ordering should apply to any status contribution from the generated `^task` line: if the selected
marker/frontmatter state was `wip`, the last intake pass may happen before the line changes final status to `read` or
`abandoned`; if the selected marker/frontmatter state was already `unread`, `read`, `abandoned`, or `legacy`, tasks
remain skipped.

## Implementation

1. In `src/native/highlights_ref/mod.rs`, update `plan_pdf_sync()` to capture a boolean such as
   `annotation_task_intake_allowed` immediately after `resolve_sync_projection()` succeeds and before calling
   `apply_pdf_task_status_signal()`.

2. Compute that boolean from the pre-signal resolved projection:

   ```rust
   let annotation_task_intake_allowed =
       projection_status_is(&resolution.projection, STATUS_WIP);
   ```

3. Keep `apply_pdf_task_status_signal()` responsible for mutating the final `synced_projection`, conflict detection,
   write-pdf requirements, marker rendering, PDF task checkbox rewriting, and report fields.

4. Replace the current annotation-candidate gate:

   ```rust
   if projection_status_is(&synced_projection, STATUS_WIP)
   ```

   with the captured `annotation_task_intake_allowed` value.

5. Do not change processed-task indexing, `ht-...` source anchors, routed task grouping, duplicate handling, or legacy
   `[highlight_task:: ...]` compatibility. The existing finalization path already inserts accepted candidates into the
   rendered body and refreshes the stable note before writing.

6. Do not change CLI flags or command-line contracts. No `cli_rules` memory read is needed because this plan adds no
   subcommands or options.

## Tests

1. Add a focused CLI regression test for the reported workflow:
   - Create a `wip` PDF marker and initial reference note.
   - Add sidecar task bullets after the reference note exists.
   - Check the generated reference note `^task`.
   - Run `bob highlights sync --write-pdf`.
   - Assert the report includes `pdf_task_contribution: status=read` and
     `annotation_tasks_created`/`annotation_tasks_create` for the new tasks.
   - Assert the final reference note has `status: read`, the checked `^task` line, new task lines with `ht-...` source
     backlinks, and matching managed source anchors.
   - Assert the PDF marker now contains `status: read`.
   - Run sync again and assert the task count stays stable.

2. Include one routed task in that regression, or add a sibling focused test, so the final intake pass covers both
   same-note insertion and routed note writes before future `read` runs skip task creation.

3. Extend the existing checked-task scan coverage, or add a small scan-specific assertion, so
   `scan --dry-run --write-pdfs` and `scan --write-pdfs` report and write annotation tasks when the pre-signal status
   was `wip`.

4. Keep `highlights_ref_sync_skips_annotation_tasks_for_non_wip_statuses` intact to prove PDFs whose selected status is
   already non-`wip` still skip task creation.

5. If a small helper is introduced for the pre-signal gate, add a unit test for it; otherwise rely on the integration
   tests because the bug is in the cross-step planning order.

## Documentation

Update the README and `docs/highlights-ref-sync.md` wording that currently says annotation tasks are created only when
the resolved PDF status is `wip`. Clarify that task intake uses the marker/frontmatter-selected status before the
generated `^task` completion/cancellation signal, so the final run that marks a `wip` PDF as `read` still imports newly
added task notes, and subsequent `read` runs do not.

## Verification

Run:

```bash
cargo fmt --check
cargo test --test cli highlights_ref_task_checked
cargo test --test cli highlights_ref_sync_creates_tasks_from_pdf_note_task_bullets
cargo test --test cli highlights_ref_sync_skips_annotation_tasks_for_non_wip_statuses
cargo test --test cli highlights_ref_scan_groups_routed_tasks_with_parallel_jobs
cargo test highlights_ref
cargo test
cargo clippy --all-targets
```

The expected result is that checked `^task` runs from `wip` both close the PDF status and create pending annotation
tasks once, while later `read` runs create no additional tasks.
