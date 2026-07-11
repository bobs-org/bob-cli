# Project Task Sync

`bob projects` manages Bob project notes through one completion-criteria task
anchored with `^prj`.

This mirrors the `bob highlights` `^task` convention for `[[ref]]` notes: the
task line is the interaction point, and the command reconciles frontmatter from
that task instead of asking users to edit machine-facing metadata directly.

## Commands

```bash
bob projects list [-b|--bob-dir DIR]
bob projects sync [-b|--bob-dir DIR] [-d|--dry-run]
```

`list` is read-only. It scans project notes, validates project scheduling,
prints frontmatter status, open `#task` count, open non-hidden task count, and
the current `^prj` state.

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
- A valid `scheduled` frontmatter date overrides both of the preceding `^prj`
  surfacing rules. When the date is later than the machine's local current
  date, every Markdown task line in the project gets exactly one whole-token
  `#hide`. On the scheduled date and afterward, all whole-token `#hide` tags
  are removed from ordinary tasks. The `^prj` task keeps its existing `#hide`
  state when the note contains any other Markdown task; if `^prj` is the only
  task, its `#hide` tags are removed too. `BOB_NOW` overrides the clock for
  deterministic previews and tests.
- Schedule visibility applies to open, in-progress, completed, canceled,
  nested, ordered-list, and lifecycle tasks. It preserves list markers,
  indentation, inline fields, trailing block IDs, line endings, and unrelated
  tags such as `#hidden`. Frontmatter, fenced code examples, and checkbox-like
  prose are ignored. Repeated syncs are idempotent.
- Active projects with open `^prj` tasks get one generated Sub-projects line
  nested directly under `^prj`, such as
  `- 🧩 **Sub-projects:** [[alpha_child]] • [[beta_child]]`.
- A child with a valid `scheduled` frontmatter date later than the machine's
  local current date is prefixed with `🗓️`, such as
  `- 🧩 **Sub-projects:** 🗓️ [[future_child]] • [[ordinary_child]]`.
  Today, past, absent, and invalid schedules do not receive the marker.
  `BOB_NOW` controls this date boundary as it does for task visibility.
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
- Existing `[scheduled::...]` fields are removed from open `^prj` tasks on
  active projects. Frontmatter `scheduled` is the sole schedule and visibility
  source.
- Terminal projects, `status: done` or `status: canceled`, get no `^prj` line
  edits while their `^prj` task stays closed or missing. Reopening the `^prj`
  task makes the project active again in the same run, so the surfacing,
  `#hide`, and Sub-projects rules above apply from the reopened `wip` status.

The dash Tasks query hides tasks with the `#hide` tag. A non-hidden task is an
open `#task` line with no `#hide` tag at all. The `^prj` task itself never
counts toward the non-hidden task count.

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

In `bob projects list`, the `SHOWN` column is the open non-hidden task count.
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
immediately applies the same future-versus-due `#hide` visibility policy as
`bob projects sync`. Other picker properties, including `dependsOn`, remain
inline Dataview fields on the task.

Pressing `Ctrl+D` on the project-backed `scheduled` item removes the YAML
property and any stale inline schedule field, but deliberately leaves existing
`#hide` tags unchanged. Once a project is unscheduled, the broader `^prj`
surfacing decision depends on open tasks and sub-project relationships and
remains owned by `bob projects sync`.

## Warnings

Warnings do not make the command fail and are not auto-fixed:

- An active project has no `^prj` task.
- The `^prj` description is still
  `<short_project_completion_criteria_goes_here>`.

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
  ok roadmap    hid 4 tasks  scheduled 2026-07-16 is future
  warning outlive  active project has no ^prj task  add `- [ ] #task #prj <completion criteria> #hide ^prj`

11 projects - 1 status updated - 9 ^prj edited - 4 task visibility updated - 1 warnings
```

Scheduled visibility is reported once per project rather than once per task.
The action includes the schedule date, direction (`future` or `due`), and the
number of affected tasks; the summary totals affected task lines.
