---
create_time: 2026-06-08 07:30:23
status: done
prompt: sdd/prompts/202606/highlights_processed_index_perf.md
---
# Plan: Keep `bob highlights` Fast After Annotation-Task Routing

## Context

The previous agent implemented highlight task routing plus a vault-wide processed-task index (commit `7a5bee3`, plan
`sdd/tales/202606/highlight_task_routing_processed_index.md`). Functionally the work looks complete and correct:
routing, `wip` gating, processed markers, grouped routed writes, dirty-write safety, and unit/CLI coverage are all in
place and the full test suite, clippy, and fmt pass.

The remaining concern is performance. This task is a focused performance review of that change plus implementation of
the obviously beneficial optimizations it surfaces.

### What the change added to the hot path

`finalize_annotation_task_plans` (`src/native/highlights_ref/mod.rs:877`) is called once per `bob highlights sync <pdf>`
and once per `bob highlights scan`, _before_ the dry-run check and before any write-safety gating. Its very first action
is:

```rust
let mut processed = processed_task_index(config)?;   // mod.rs:881
```

`processed_task_index` → `collect_processed_task_index_from_dir` recursively walks the **entire** vault under
`config.bob_dir` (including `done/` and all nested note folders), and for every `.md` file calls `fs::read_to_string`
and scans every line for task markers.

This is a real regression relative to the pre-commit behavior, where duplicate detection was local to the single
generated reference note body and **no vault-wide scan happened at all**. The new full-vault walk now runs:

- on **every** `sync`/`scan`, including `--dry-run`;
- for **non-`wip`** PDFs, which never produce annotation-task candidates;
- for `wip` PDFs whose sidecar/notes contain **no `#task` bullets**;

i.e. in the overwhelmingly common case where there are zero candidate tasks to create, the index it builds is never
consulted (the index is only read through `ProcessedTaskIndex::accept`). The walk is pure wasted I/O proportional to
total vault size, paid on every invocation of the command.

### Verification that the feature goal itself was met

As part of this task I confirmed the routing/processed-index behavior is wired correctly (routed-target validation
happens during per-PDF planning via `route_name_to_note_path`; the index is built once per `finalize` call, not per PDF;
accepted ids/identities are inserted into the in-memory index so duplicates within one run are suppressed). The
optimizations below must not change any of this observable behavior.

## Goal

Eliminate the unconditional full-vault scan from the common path so `bob highlights` is no slower than before the
routing change whenever no new annotation tasks are being created, while preserving identical behavior when tasks _are_
created. Apply small, clearly-correct cleanups discovered along the way. Do not regress correctness, reporting, or test
coverage.

## Design Decisions

1. **Skip the vault scan when there are no candidates (headline fix).** In `finalize_annotation_task_plans`,
   short-circuit before building the processed index when no plan carries any annotation-task candidates:
   `plans.iter().all(|p| p.annotation_task_candidates.is_empty())`. In that case there is nothing to accept/reject, no
   routed groups to form, and no reference-note bodies to mutate, so the function can return `Ok(())` immediately.
   - This is safe because the processed index is _only_ consulted via `accept(&candidate)`; with zero candidates it is
     provably unused.
   - Per-plan counters (`annotation_tasks_created`, `annotation_tasks_skipped`, `routed_task_note_writes`) are already
     initialized to zero/empty when a plan is constructed in `plan_pdf_sync`, and `finalize` runs exactly once per plan
     (only two call sites, each once), so the early return leaves them correct. Dry-run reports of
     `created: 0 / skipped: 0` remain accurate.
   - Behavior change worth noting: with no candidates, an unreadable unrelated vault file no longer aborts the command.
     This is strictly an improvement — we should not fail a sync over a file we never needed to read — but it will be
     called out in the implementation summary.

2. **Avoid cloning the candidate vector per plan.** The finalize loop currently does
   `for candidate in plan.annotation_task_candidates.clone()` (mod.rs:892) solely to dodge a borrow conflict with the
   `plan.annotation_tasks_*` mutations in the loop body. Replace the clone with
   `std::mem::take(&mut plan.annotation_task_candidates)` (the field is never read after `finalize`; its only readers
   are the constructor and this loop). This removes a deep clone of every candidate (each holds several
   `String`/`PathBuf`s) and reads more clearly. Pure micro-optimization + simplification with no behavior change.

3. **Leave the vault walk itself sequential (explicitly out of scope).** Parallelizing
   `collect_processed_task_index_from_dir` (e.g. collect paths then read in a scoped-thread pool like `plan_pdfs`,
   merging partial indexes) would speed the _task-creation_ path on very large vaults. I am deliberately **not** doing
   this now because:
   - After decision (1) the walk only runs when tasks are actually being created, which is the infrequent, intentionally
     heavier path.
   - It adds concurrency, index-merging, and error-aggregation complexity to a feature that shipped two days ago; the
     risk/benefit is poor without a profile showing the walk is a real bottleneck on this vault.
   - The current walk already relies on `read_dir`'s `d_type` (via `entry.file_type()`), so it does not pay extra `stat`
     calls per entry. If future profiling shows the create-path walk is slow, parallelization is the natural follow-up;
     this plan notes it rather than implementing it.

## Implementation Steps

1. **Add the candidate short-circuit** in `finalize_annotation_task_plans` (`src/native/highlights_ref/mod.rs`):
   - Before `let mut processed = processed_task_index(config)?;`, return `Ok(())` if no plan has candidates.
   - Keep the rest of the function unchanged.

2. **Replace the per-plan clone** with `std::mem::take` on `plan.annotation_task_candidates` inside the finalize loop.

3. **Verify no other reader depends on the cleared field.** Confirm (already checked) that `annotation_task_candidates`
   is read only by the constructor and the finalize loop, so `mem::take` is safe.

## Testing

- Existing unit tests (`processed_task_index_*`, route parsing, candidate extraction) and the highlights CLI tests must
  continue to pass unchanged — they exercise the create path, where the scan still runs.
- Add one focused CLI regression test asserting the no-candidate path still behaves correctly and is unaffected by
  unrelated/unreadable vault content, e.g.: running `sync` on a `wip` PDF whose notes contain **no** `#task` bullets (or
  a non-`wip` PDF) succeeds and creates no tasks even when the vault also contains a sibling note that would otherwise
  be walked. This pins the "skip when no candidates" behavior so a future refactor can't silently reintroduce the
  unconditional scan dependency.
  - If a deterministic assertion on "did not scan" proves awkward at the CLI layer, fall back to a small unit-level
    assertion that `finalize_annotation_task_plans` succeeds for a plan with empty candidates without requiring a
    readable vault.

## Verification

- `cargo fmt --check`
- focused highlights unit + CLI tests
- `cargo test`
- `cargo clippy --all-targets`

## Risks and Tradeoffs

- **Skipped scan hides unrelated read errors when no tasks are created.** Considered an improvement (we only surface
  read errors for work we actually need to do), but it is a behavior change and will be stated in the summary.
- **No parallelization of the walk.** Accepted: the walk now only runs on the task-creation path; parallelizing is
  deferred to a future profile-driven change rather than added speculatively to a freshly shipped feature.
- **Scope is intentionally small.** This is a targeted perf fix, not a re-architecture of the processed index; the
  routing feature's behavior is preserved exactly.
