# Project Task Sync

`bob projects` manages Bob project notes through one completion-criteria task
anchored with `^prj`.

This mirrors the `bob highlights` `^ref` convention for `[[ref]]` notes: the
task line is the interaction point, and the command reconciles frontmatter from
that task instead of asking users to edit machine-facing metadata directly.

## Commands

```bash
bob projects list [-b|--bob-dir DIR]
bob projects sync [-b|--bob-dir DIR] [-d|--dry-run]
```

`list` is read-only. It scans project notes, validates project scheduling,
prints frontmatter status, open `#task` count, dashboard-visible task count,
and the current `^prj` state.

`sync` mutates only the exact lines it needs to change. It prints one line for
each action or warning, then a summary. Per-file errors are reported without
stopping the rest of the scan, and the command exits 1 when any file error
occurred.

## Project Notes

A project note is any Markdown file in the vault whose frontmatter has:

```yaml
type: "[[project]]"
```

Bare `type: [[project]]` is accepted too. The scan skips `done/`, `.git/`,
`.obsidian/`, `_templates/`, and `_generated/`.

`sync` also reads an optional `parent` frontmatter field when it is an Obsidian
wikilink, such as `parent: "[[Parent Project]]"`.

Project scheduling is an optional frontmatter date:

```yaml
scheduled: 2026-07-16
```

Quoted values such as `scheduled: "2026-07-16"` are also accepted. The value
must be exactly `YYYY-MM-DD` and must be a real calendar date. Empty values,
timestamps, shortened dates, and impossible dates such as `2026-02-30` are
per-file scan errors for both `list` and `sync`. `sync` leaves a project with an
invalid schedule untouched and continues processing other project files.

## The `^prj` Task

Each active project should contain one task line like:

```markdown
- [ ] #task #prj Ship the project outcome! #hide ^prj
```

The trailing block id must be exactly `^prj`. The `#prj` tag immediately after
`#task` marks this as the machine-managed project lifecycle task so Obsidian
task views can tell it apart from ordinary follow-up tasks; it is additive, and
legacy lines without `#prj` are still recognized. Multiple `^prj` tasks or a
`^prj` line that is not a valid `#task` checkbox are per-file errors.

Task statuses follow the Tasks plugin convention:

```text
[ ]  open
[/]  open
[*]  open
[x]  done
[X]  done
[-]  canceled
```

## Sync Rules

`bob projects sync` applies these rules:

- `[x]` or `[X]` on the `^prj` task sets frontmatter `status: done`.
- `[-]` on the `^prj` task sets frontmatter `status: canceled`.
- An open `^prj` task on a terminal project, `status: done` or
  `status: canceled`, reopens it to `status: wip`. Open `^prj` tasks on `wip`,
  `waiting`, or other non-terminal projects leave the status unchanged.
- Active projects with zero non-hidden open tasks and no open sub-projects
  have `#hide` removed from their open `^prj` task so they surface in
  `dash.md`'s Tasks section.
- Active projects with non-hidden open tasks or open sub-projects get
  `#hide` added back to their open `^prj` task immediately before `^prj`.
- A valid `scheduled: P` frontmatter date overrides both preceding `^prj`
  surfacing rules. Every ordinary task with an open marker (`[ ]`, `[*]`,
  `[/]`, or `[?]`) receives `[scheduled:: P]`, unless its one existing
  square-bracket or parenthesized `scheduled` field is a valid date equal to or
  later than `P`. Missing, malformed, and earlier values are written in the
  canonical square-bracket form. Emoji Tasks dates such as `⏳ 2026-08-01`
  are not inline Dataview fields and are not considered.
- Ordinary tasks lose all whole-token `#hide` tags at every checkbox status;
  near-matches such as `#hidden` and `#hideaway` remain. An ordinary task with
  multiple `scheduled` fields is left completely unchanged and reported as a
  non-fatal warning. Done, canceled, and unknown/custom-status tasks otherwise
  receive no schedule field.
- The `^prj` lifecycle task never receives an inline schedule. A future
  project date forces exactly one `#hide` on it. On or after the date, its
  `#hide` state stays unchanged unless it is the note's only Markdown task, in
  which case `#hide` is removed. `#hide` therefore remains the lifecycle
  surfacing mechanism for `^prj`, not for scheduled ordinary tasks.
- Schedule propagation applies only to non-terminal projects. It preserves
  list markers, indentation, descriptions, other inline fields, trailing block
  IDs, CRLF line endings, and unrelated tags. Frontmatter, fenced examples,
  non-task lines, and checkbox-like prose are ignored. `BOB_NOW` overrides the
  local date boundary for deterministic previews and tests; repeated syncs are
  idempotent.

For a valid project date `P`, the ordinary-task contract is:

| Task state | Result |
| --- | --- |
| Open, absent/malformed schedule or valid schedule before `P` | Set `[scheduled:: P]`; remove `#hide` |
| Open, one valid schedule on/after `P` | Keep that schedule; remove `#hide` |
| Open, multiple `scheduled` fields | Leave the line unchanged; warn |
| Done, canceled, or unknown/custom | Remove `#hide` only |
| `^prj`, `P` in the future | Force exactly one `#hide`; never add an inline schedule |
| `^prj`, `P` due/past | Keep `#hide`, except remove it when `^prj` is the note's only task |
- Active projects with open `^prj` tasks get one generated Sub-projects line
  nested directly under `^prj`, such as
  `- 🧩 **Sub-projects:** [[alpha_child]] • [[beta_child]]`.
- A child with a valid `scheduled` frontmatter date later than the machine's
  local current date is prefixed with `🗓️`, such as
  `- 🧩 **Sub-projects:** 🗓️ [[future_child]] • [[ordinary_child]]`.
  Today, past, absent, and invalid schedules do not receive the marker.
  `BOB_NOW` controls this date boundary as it does for task schedules.
- The marker-prefixed Sub-projects line is fully machine-owned and rewritten
  into canonical form. Duplicate marker lines are removed. The line is deleted
  only when there are no open sub-projects and no tracked closed sub-projects
  left to show. Sync adds or removes `🗓️` as schedules change and removes it
  automatically on the scheduled date.
- Closed sub-projects already present on the generated line are retained as a
  ledger: done children render as `~~[[child]]~~ ✅`, and canceled children
  render as `~~[[child]]~~ ❌`. Schedule and lifecycle decorations are
  independent, so a retained future-scheduled done child renders as
  `🗓️ ~~[[child]]~~ ✅`.
- Every other sub-bullet under `^prj` is user-owned, including bare wikilinks
  like `- [[scratch_note]]`; `sync` never removes or uses them to suppress the
  generated line.
- Existing inline `scheduled` fields are removed from open `^prj` tasks on
  active projects. Frontmatter `scheduled` is the sole project schedule.
- Terminal projects, `status: done` or `status: canceled`, get no `^prj` line
  edits while their `^prj` task stays closed or missing. Reopening the `^prj`
  task makes the project active again in the same run, so the surfacing,
  `#hide`, and Sub-projects rules above apply from the reopened `wip` status.

`bob projects sync` writes task schedules but does not write checkbox markers.
`bob task-status-hooks` remains the single CLI owner of derived `[?]` Blocked
state: run it after `projects sync` to block future-scheduled tasks or recover
matured ones once no dependency or schedule reason remains. The dashboard
already excludes future schedules and Blocked tasks, so `dash.md` needs no
query change. These tasks now appear in `blocked.md`; that query also needs no
change. Derived Blocked status outranks Pomodoro promotion.

An open sub-project is another project note whose `parent` wikilink resolves to
this note's file stem and whose own `^prj` task is open. A child with terminal
frontmatter but an open `^prj` task counts as open in the same run, because the
open task reopens it to `wip`. Checked or canceled child projects do not keep the
parent hidden; missing, malformed, or multiple non-terminal `^prj` child tasks
are excluded from the generated line.

Generated sub-project links use the child note's file stem with its original
casing and no path or alias. Open children are always shown first, sorted
case-insensitively. Closed children that were already listed are shown after
open children, also sorted case-insensitively. Links are separated with `•` on
the single marker-prefixed line.

Closed children are preserve-and-mark only: `sync` marks a terminal child if it
is already on the generated line, but it does not resurrect older closed
children that are not listed. Deleting a closed entry by hand prunes it
permanently unless that child is reopened.

In `bob projects list`, the `SHOWN` column counts open, non-`^prj`, non-hidden
`#task` lines that are neither `[?]` nor validly scheduled later than the local
current date. The separate open non-hidden count still drives `^prj` surfacing,
so dependency-blocked tasks do not accidentally surface the lifecycle task.
An open `^prj` task with a `#hide` tag renders as `open`; an open `^prj` task
without a `#hide` tag renders as `on dash`.

When a project has no `status:` line and the `^prj` task is checked or canceled,
`sync` inserts `status: done` or `status: canceled` immediately after the
`type:` line.

The Bob Navigation Hotkeys "Create project note from task" command transfers a
valid `[scheduled:: YYYY-MM-DD]` source-task field into the new project's
frontmatter and removes it from the completion criteria. Invalid or duplicate
schedule fields stop creation with a focused notice. In the `<ctrl+=>` child
note picker, future-scheduled projects show a `calendar-clock` chip immediately
before the status pill; the chip says `Tomorrow`, `Jul 16`, or `Jul 16, 2027`
while its tooltip and accessible label expose the full date. Today, past,
missing, and invalid dates do not receive a chip. The compact `🗓️` in the
generated parent ledger represents the same future-only state without the
picker's labeled date chip.

### Scheduling from the `^prj` task

With the cursor on a valid `#task ... ^prj` lifecycle task, Bob Navigation
Hotkeys' `Ctrl+Shift+P` **Set bullet property** picker treats `scheduled` as a
project-note property. Choosing a date writes canonical `scheduled: YYYY-MM-DD`
YAML, removes any stale inline `[scheduled:: ...]` field from `^prj`, and
immediately propagates task-level schedules. It also applies the derived status
decision in the same guarded editor transaction: future-scheduled tasks become
Blocked, while due tasks recover to a safely proven Ready, Next, or In Progress
rank. A later task-owned schedule is preserved and remains Blocked. When the
vault snapshot cannot prove recovery, the property edit proceeds and `[?]` is
left for `bob task-status-hooks`. Other picker properties, including
`dependsOn`, remain inline Dataview fields on the task.

Pressing `Ctrl+D` on the project-backed `scheduled` item removes the YAML
property, removes inline schedules exactly equal to that project date from
ordinary open tasks, and reconciles their Blocked markers. Other task-owned
schedule values remain. The `^prj` `#hide` surfacing decision remains owned by
`bob projects sync`.

Removing or editing project frontmatter outside `Ctrl+D` cannot identify which
task fields were propagated, so those fields remain. Prefer `Ctrl+D` when
unscheduling a project.

## Warnings

Warnings do not make the command fail and are not auto-fixed:

- An active project has no `^prj` task.
- The `^prj` description is still
  `<short_project_completion_criteria_goes_here>`.
- An ordinary task has multiple inline `scheduled` fields. That line is left
  unchanged until it contains exactly one.

Terminal projects are allowed to be missing `^prj`; `bob move-done-tasks` may
archive the checked or canceled task later.

## Examples

Preview changes:

```bash
bob projects sync --dry-run
```

Use a temporary vault fixture:

```bash
bob projects list --bob-dir /tmp/bob-vault
bob projects sync --dry-run --bob-dir /tmp/bob-vault
```

Typical action output:

```text
  ok sase_blog  status: wip -> done  ^prj task checked
  ok bob        removed #hide from ^prj  no non-hidden open tasks or open sub-projects
  ok athena     added #hide to ^prj  project has open sub-projects
  ok athena     added [[sase_blog]] to ^prj  open sub-project
  ok athena     updated [[sase_blog]] on ^prj  sub-project completed
  ok athena     updated [[old_plan]] on ^prj  sub-project canceled
  ok athena     removed [[old_child]] from ^prj  no longer a sub-project
  ok athena     added 🗓️ [[future_child]] to ^prj  sub-project scheduled in future
  ok athena     removed 🗓️ [[due_child]] from ^prj  sub-project no longer scheduled in future
  ok athena     updated sub-projects on ^prj  canonical format
  ok roadmap    scheduled 4 tasks 2026-07-16  frontmatter scheduled is future
  ok roadmap    removed #hide from 4 tasks  task schedules replace #hide
  hint: run `bob task-status-hooks` to reconcile derived [?] Blocked markers
  warning outlive  active project has no ^prj task  add `- [ ] #task #prj <completion criteria> #hide ^prj`

11 projects - 1 status updated - 9 ^prj edited - 4 task schedules updated - 1 warnings
```

Schedule propagation is reported once per project rather than once per task.
The summary totals task lines whose inline schedule was added or updated;
legacy `#hide` removals are reported separately.
