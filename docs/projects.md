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

`list` is read-only. It scans project notes, prints frontmatter status, open
`#task` count, open unprioritized task count, and the current `^prj` state.

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

## The `^prj` Task

Each active project should contain one task line like:

```markdown
- [ ] #task Ship the project outcome! [p::2] ^prj
```

The trailing block id must be exactly `^prj`. Multiple `^prj` tasks or a `^prj`
line that is not a valid `#task` checkbox are per-file errors.

Task statuses follow the Tasks plugin convention:

```text
[ ]  open
[/]  open
[B]  open
[x]  done
[X]  done
[-]  canceled
```

## Sync Rules

`bob projects sync` applies these rules:

- `[x]` or `[X]` on the `^prj` task sets frontmatter `status: done`.
- `[-]` on the `^prj` task sets frontmatter `status: canceled`.
- Open `^prj` tasks do not change frontmatter status.
- Active projects with zero unprioritized open tasks and no open sub-projects
  have `[p::2]` removed from their open `^prj` task so they surface in
  `dash.md`'s Tasks section.
- Active projects with unprioritized open tasks or open sub-projects get
  `[p::2]` added back to their open `^prj` task immediately before `^prj`.
- Active projects with open `^prj` tasks get one generated Sub-projects line
  nested directly under `^prj`, such as
  `- 🧩 **Sub-projects:** [[alpha_child]] • [[beta_child]]`.
- The marker-prefixed Sub-projects line is fully machine-owned and rewritten
  into canonical form. Duplicate marker lines are removed, and the line is
  deleted when there are no open sub-projects.
- Every other sub-bullet under `^prj` is user-owned, including bare wikilinks
  like `- [[scratch_note]]`; `sync` never removes or uses them to suppress the
  generated line.
- Existing `[scheduled::...]` fields are removed from open `^prj` tasks on
  active projects. `scheduled` is no longer used for project surfacing.
- Terminal projects, `status: done` or `status: canceled`, never get `^prj`
  line edits.

The dash Tasks query hides tasks with any `[p::N]` field. An unprioritized task
is an open `#task` line with no `[p::...]` inline field at all; `[p::0]` is
therefore prioritized for sync purposes. The `^prj` task itself never counts
toward the unprioritized task count.

An open sub-project is another project note whose `parent` wikilink resolves to
this note's file stem and whose own `^prj` task is open. Checked, canceled,
missing, malformed, or multiple `^prj` child tasks do not keep the parent
hidden.

Generated sub-project links use the child note's file stem with its original
casing and no path or alias. Links are sorted case-insensitively and separated
with `•` on the single marker-prefixed line.

In `bob projects list`, the `UNPRI` column is the open unprioritized task count.
An open `^prj` task with a `p` field renders as `open`; an open `^prj` task
without a `p` field renders as `on dash`.

When a project has no `status:` line and the `^prj` task is checked or canceled,
`sync` inserts `status: done` or `status: canceled` immediately after the
`type:` line.

## Warnings

Warnings do not make the command fail and are not auto-fixed:

- An active project has no `^prj` task.
- A terminal project still has an open `^prj` task.
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
  ok bob        removed [p::2] from ^prj  no unprioritized open tasks or open sub-projects
  ok athena     added [p::2] to ^prj  project has open sub-projects
  ok athena     added [[sase_blog]] to ^prj  open sub-project
  ok athena     removed [[old_child]] from ^prj  no longer an open sub-project
  ok athena     updated sub-projects on ^prj  canonical format
  warning outlive  active project has no ^prj task  add `- [ ] #task <completion criteria> [p::2] ^prj`

11 projects - 1 status updated - 5 ^prj edited - 1 warnings
```
