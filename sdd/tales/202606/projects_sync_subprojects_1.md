---
create_time: 2026-06-12 13:46:05
status: done
prompt: sdd/prompts/202606/projects_sync_subprojects_1.md
---
# Plan: `bob projects sync` — consider sub-projects for the `^prj` `[p::2]` field

## Problem

`bob projects sync` currently decides whether the open `^prj` task carries `[p::2]` purely from the project's own open
unprioritized `#task` count (`plan_project_sync()`, `src/native/projects.rs:790-808`):

- zero open unprioritized tasks → remove `[p::2]` so the `^prj` task surfaces in `dash.md`'s Tasks section ("this
  project needs triage: add tasks or close it"),
- one or more open unprioritized tasks → add `[p::2]` so the `^prj` task stays hidden.

This is wrong for **parent projects**. A project that contains sub-projects doesn't need tasks of its own — its work
lives in the children. Today sync strips `[p::2]` from such parents and they nag on the dash forever.

A project _contains sub-projects_ when any other project note links to it via its `parent` frontmatter field
(`parent: "[[<area-or-project>]]"`, the convention from the obsidian_projects epic). The CLI does not currently parse
`parent` at all.

## New rule

Reframe the `[p::2]` logic as a single "should this project surface on the dash?" predicate. For an active project with
an open `^prj` task:

- **surface** (ensure no `p` field on `^prj`) iff `open_unprioritized_count == 0` **and** the project has **no open
  sub-projects**;
- **hide** (ensure `[p::2]` present) otherwise — i.e. having open sub-projects now keeps/adds `[p::2]` exactly like
  having open unprioritized tasks does.

Note the second half is a real behavior addition, not just suppression of removal: a parent project whose `^prj`
currently lacks a `p` field gets `[p::2]` **added**, even with zero unprioritized tasks of its own.

### Which children count: those with an open `^prj` task

**A sub-project counts iff its own `^prj` task is open** — i.e. its file parses as a project, its `parent` wikilink
resolves to this note, and its `^prj` classification is `PrjTaskState::Open` (exactly one well-formed open
`#task ... ^prj` checkbox, per `classify_prj_task()`, `projects.rs:1162-1203`). Consequences:

- A child whose `^prj` is checked (`[x]`/`[X]`) or canceled (`[-]`) does **not** count. This also makes the parent/child
  interaction converge in a single run with no extra machinery: the same sync run that flips the child's frontmatter to
  done/canceled already sees the child's `^prj` as not-open, so the parent's `[p::2]` is removed in that run and the
  second run is a no-op (the idempotency property the existing tests assert). No "effective status" computation is
  needed for the children set — the `^prj` checkbox state is read directly from the file.
- A child with a **missing, malformed, or multiple** `^prj` does not count: there is no open `^prj` task to satisfy the
  requirement. Such children already produce their own warning/error (`active project has no ^prj task`,
  `malformed ^prj task`, `multiple ^prj tasks`), which is the user's prompt to fix them; while broken they push the
  parent onto the dash rather than silently hiding it.
- The child's frontmatter status is **not** part of the criterion. The one divergent case — terminal frontmatter status
  with a still-open `^prj` — is a drift state sync already warns about (`^prj task is still open`,
  `projects.rs:760-768`) and never auto-fixes; counting it keeps the rule exactly "open `^prj` task" and resolves itself
  when the user clears the warning.
- When every child's `^prj` is done/canceled, the parent resurfaces on the dash — the user needs to either close it or
  queue more work, mirroring the existing zero-tasks rule.
- The scan already skips the `done/` directory, so archived children naturally drop out.

### Parent-link resolution

`parent` values are Obsidian wikilinks. Resolve the link target to a project by note name:

1. Read the `parent` frontmatter value via the existing `frontmatter_value()` + `trim_yaml_scalar()` helpers (handles
   quoted and bare values).
2. Strip the `[[...]]` brackets; ignore values that aren't wikilinks.
3. Inside the brackets, drop any `|alias` suffix and any `#heading`/`#^block` suffix, then take the final `/` path
   segment (Obsidian accepts both `[[bob]]` and `[[gtd/bob]]`).
4. Match that name against each scanned project's **file stem** (final path component of `relative_path` without `.md`),
   case-insensitively (Obsidian link resolution is case-insensitive). Note: the existing `project_name()` returns the
   slash-joined relative path, which is _not_ what wikilinks carry — matching must use the stem.

Parents pointing at areas or non-project notes simply never match a project — no error, no warning. A missing/empty
`parent` field is likewise fine (no behavior change for that note as a child; it just isn't anyone's child).

## Implementation

All changes live in `src/native/projects.rs` unless noted.

### 1. Parse `parent` and expose the file stem

- Add to `Project`: `parent_target: Option<String>` (the resolved link-target name from the `parent` wikilink,
  lowercased) and a way to get the note's own link name (lowercased file stem) — either a stored field or a small method
  on `Project`.
- New helper `wikilink_target(value: &str) -> Option<String>` implementing steps 2–3 above, plus parsing in
  `parse_project()` (`projects.rs:1050-1100`).

### 2. Restructure sync into two passes

`sync_markdown_file()` currently reads → parses → plans → writes one file at a time inside the directory walk
(`projects.rs:686-745`). Planning now needs global knowledge, so:

- **Pass 1 (collect):** walk directories exactly as today (same ordering, same exclusions), but only read + parse,
  accumulating `(path, relative_path, contents, Option<Project>)` plus any read/parse issues into the report. Keep the
  existing per-file semantics: files whose parse produced issues still increment `project_count` but are excluded from
  planning.
- **Between passes:** build the set of link-target names referenced as `parent` by projects whose `^prj` state is
  `Open`, excluding self-links (a project naming itself as parent must not count as its own child). Files with parse
  issues never qualify — an issue implies a `Malformed`/`Multiple` `^prj` state, not `Open`.
- **Pass 2 (plan + apply):** iterate the collected files in the original walk order; for each cleanly-parsed project,
  compute `has_open_subprojects = children-set contains its own stem`, then plan, apply edits, and write exactly as
  today. Event/issue ordering in the output is preserved because pass 2 follows the walk order, and the parent/child
  outcome is independent of walk order because the children set is complete before any planning happens.

Vault notes are small and few; holding contents in memory for one pass is fine.

### 3. Planning logic

- `plan_project_sync()` gains a `has_open_subprojects: bool` parameter. The priority block (`projects.rs:793-801`)
  becomes: if `open_unprioritized_count == 0 && !has_open_subprojects` → `RemovePriority` when a `p` field exists; else
  → `AddPriority` when none exists.
- `ProjectChange::AddPriority` carries the reason so the output stays truthful: keep `"unprioritized open tasks exist"`
  when that's the trigger, use `"project has open sub-projects"` when only sub-projects justify it
  (`ProjectChange::event()`, `projects.rs:590-595`).
- Update the `RemovePriority` reason from `"no unprioritized open tasks"` to also reflect the new condition (e.g.
  `"no unprioritized open tasks or open sub-projects"`).

### 4. Documentation

- Update `docs/projects.md`:
  - Sync Rules: rewrite the two `[p::2]` bullets around the surface/hide predicate, defining "open sub-project" (another
    project note whose `parent` wikilink resolves to this note **and** whose own `^prj` task is open) and noting that
    open sub-projects keep/add `[p::2]` on the parent.
  - Project Notes section: mention the `parent` frontmatter field is read by sync.
  - Example output: add an `added [p::2]` line with the sub-projects reason.
- Update the `sync` subcommand's `long_about` text (`projects.rs:82-98`), which currently states the old tasks-only
  rule.

### 5. Tests

Unit tests (existing `#[cfg(test)]` module in `projects.rs`):

- `wikilink_target` extraction: quoted/bare, alias, heading/block suffix, path-qualified, non-wikilink values, case
  normalization.
- `plan_project_sync` with `has_open_subprojects = true`: no `RemovePriority` at zero unprioritized tasks; `AddPriority`
  (with the sub-projects reason) when the `p` field is missing; unchanged behavior when `has_open_subprojects = false`.
- Children-set computation: child with an open `^prj` counts; checked/canceled `^prj` child doesn't; missing-`^prj`
  child doesn't; malformed/multiple-`^prj` child doesn't; self-link doesn't; parent pointing at an area matches nothing;
  case-insensitive and path-qualified link matching; terminal-status child with an open `^prj` still counts.

Integration tests (`tests/cli.rs`, following the existing inline-fixture style of
`projects_sync_updates_status_prj_priority_warns_and_is_idempotent`, `tests/cli.rs:1710-1876`):

- Parent project with zero own tasks + one open-`^prj` child: `[p::2]` is kept, and added if missing; run twice to
  assert idempotency.
- Same parent after the child's `^prj` is checked: a single sync run both flips the child to `status: done` and removes
  the parent's `[p::2]`; second run is a no-op.
- Parent whose only child has no open `^prj` (missing or checked): parent is treated as childless and surfaces.
- Dry-run prints the new add/keep decisions without writing.

## Out of scope

- `bob projects list` output is unchanged (no sub-project column; the existing `on dash` rendering of the current file
  state stays accurate).
- No validation/warnings on `parent` values (missing parent, parent pointing at a nonexistent note, cycles) — that
  belongs to the broader obsidian_projects epic, not this change.
- No new CLI flags or subcommands.
