---
create_time: 2026-06-12 11:53:47
status: wip
prompt: sdd/prompts/202606/projects_prj_priority_sync.md
---
# Plan: `bob projects sync` — manage `^prj` `[p::2]` instead of `[scheduled::...]`

## Problem & Product Context

Today, `bob projects sync` surfaces stalled projects by inserting `[scheduled::YYYY-mm-dd]` on the open `^prj` task when
a project has no open P0 tasks. This mechanism is being retired in favor of one that works directly with the
`~/bob/dash.md` "Tasks" section.

The dash "Tasks" query shows open, unblocked tasks that have **no `[p::N]` inline field** (filter:
`!/(^|[^\[])\[p::\s*\d+\s*\](?!\])/`). So the natural way to surface a stalled project's completion task is to **remove
its `[p::2]` field**; the natural way to hide it again (because the project has real unprioritized tasks showing on the
dash) is to **add `[p::2]` back**.

New sync behavior, for every active project with a single well-formed open `^prj` task:

1. **No unprioritized open tasks in the file** → remove the `p` inline field from the `^prj` line (it then appears in
   dash's "Tasks" section).
2. **Unprioritized open tasks exist in the file** → ensure the `^prj` line carries `[p::2]` (insert it immediately
   before `^prj` when missing, matching the template `- [ ] #task <criteria> [p::2] ^prj`).
3. **Never insert `[scheduled::...]`** anymore. As a one-time-per-file migration, sync also strips any existing
   `[scheduled::...]` field from open `^prj` tasks on active projects — the property is deprecated for `^prj` tasks and
   would otherwise linger forever (every prior sync run left one behind).

"Unprioritized" means an open `#task` line (excluding the `^prj` line itself) with **no `[p::...]` inline field at
all**. Note this is a deliberate semantic change from the current P0 counting (`p` missing _or_ `[p::0]`): dash hides
any task with a `[p::N]` field, including `[p::0]`, so the sync trigger must match the dash filter exactly.

All rules remain idempotent (a second run produces zero actions) and keep the existing guards: terminal projects
(effective status `done`/`canceled` after this run's status flip) never get `^prj` line edits;
missing/malformed/multiple `^prj` handling, status reconciliation from `[x]`/`[-]`, and all warnings are unchanged.

## Implementation

All work is in `src/native/projects.rs`, `tests/cli.rs`, and `docs/projects.md`.

### 1. Parsing (`parse_project`, `classify_prj_task`, structs)

- Rename `Project.open_p0_count` → `open_unprioritized_count`; count open non-`^prj` `#task` lines where
  `inline_field_value(text, "p").is_none()` (was `task_priority(...).unwrap_or(0) == 0`).
- `PrjTask`: replace `priority: Option<usize>` with the raw field `priority: Option<String>` (presence is what matters;
  `[p:: 2]` spacing variants and non-numeric values all count as "has a `p` field"). Keep `scheduled` so the plan step
  can detect stale fields needing cleanup.

### 2. Sync planning (`plan_project_sync`)

Replace `ProjectChange::Schedule` with three change kinds on the open `^prj` line of an active project:

- `RemovePriority` — `p` field present and `open_unprioritized_count == 0`.
- `AddPriority` — `p` field absent and `open_unprioritized_count > 0`.
- `RemoveScheduled` — `scheduled` field present (independent of the priority rules).

### 3. Text edits (`apply_project_changes`)

- Insertion reuses the old `schedule_edit` approach: insert `"[p::2] "` at the `^prj` anchor position.
- Removal needs a new helper: a span-returning variant of `inline_field_value` that locates the `[key::value]` byte
  range on the `^prj` line, then deletes the field plus one adjacent whitespace run so `... criteria [p::2] ^prj`
  becomes `... criteria ^prj` (works for both the `p` and `scheduled` removals, CRLF-safe).

### 4. Events & output (`SyncEvent`, `print_sync_report`, summary)

- Replace `SyncEvent::Schedule` with a `^prj`-edit event per change, e.g.:
  - `ok <project>  removed [p::2] from ^prj  no unprioritized open tasks`
  - `ok <project>  added [p::2] to ^prj  unprioritized open tasks exist`
  - `ok <project>  removed [scheduled::2026-06-01] from ^prj  scheduled is no longer used`
  - Dry-run renders `would remove`/`would add` with the `[dry-run]` prefix, as today.
- Summary line becomes `N projects - N status updated - N ^prj edited - N warnings` (one counter for all `^prj` line
  edits keeps the summary compact); errors segment unchanged.
- Drop the now-unused `schedule_prefix`/`scheduled_label` styling.

### 5. `bob projects list`

- Rename the `P0` column to `UNPRI` (open unprioritized count) to match the new semantics.
- The `^PRJ` column no longer shows `scheduled <date>` / `📅 <date>`. Open tasks render as today's `open` label when the
  `p` field is present, and as a distinct blue `on dash` label when unprioritized (i.e. currently surfaced in dash's
  Tasks section). Missing/malformed/multiple/done/canceled/placeholder rendering is unchanged.

### 6. Help text & docs

- `build_cli` / `sync_command` long_about: replace the scheduling sentence with the new priority-management behavior
  ("Active projects with no unprioritized open tasks have the [p::2] field removed from their open ^prj task so it
  surfaces in dash.md's Tasks section; projects with unprioritized open tasks get [p::2] added back.").
- `docs/projects.md`: rewrite the Sync Rules bullets, the "implicitly P0" paragraph (now: tasks with any `[p::N]` field
  are hidden from the dash; the `^prj` task never counts toward the unprioritized count), the `list` column description,
  and the example output block.

### 7. Tests

Unit tests (`projects.rs`):

- Update parser tests for the renamed count/`priority` field and removed scheduled label.
- `plan_project_sync` cases: remove-priority when no unprioritized tasks; add-priority when unprioritized tasks exist;
  no change when state already correct (idempotency); scheduled-field cleanup; explicit `[p::0]` task now counts as
  prioritized; terminal / drift projects get no line edits.
- `apply_project_changes` cases: field removal whitespace handling (incl. `[p:: 2]` spacing and CRLF), insertion before
  `^prj`, combined status + line edits.

Integration tests (`tests/cli.rs`):

- Rework `projects_sync_updates_status_schedules_warns_and_is_idempotent`: `Stalled.md` / `ZeroOpen.md` now lose
  `[p::2]`; `HasP0.md` (rename to `HasUnprioritized.md`) keeps `[p::2]`; add a fixture whose `^prj` lacks `p` while
  unprioritized tasks exist (gains `[p::2]`); `ExistingScheduled.md` loses both `[scheduled::...]` and `[p::2]`; update
  expected output lines, summary counts, and the idempotent re-run.
- Update `projects_list_scans_project_notes_and_renders_counts` for the `UNPRI` header, `on dash` label, and removed
  scheduled assertion.

### 8. Validation

`just all` (cargo fmt --check, clippy --all-targets --all-features, cargo test), plus a manual
`bob projects sync --dry-run -b <tmp fixture vault>` sanity pass.

## Out of Scope

- No changes to dash.md or the Tasks-plugin query itself.
- No changes to `scheduled` handling anywhere else (it remains a normal Tasks-plugin property for non-`^prj` tasks).
- No new CLI subcommands or options.
