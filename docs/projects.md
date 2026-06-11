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
`#task` count, open P0 task count, and the current `^prj` state.

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
- Active projects with zero open P0 tasks get `[scheduled::YYYY-mm-dd]`
  inserted immediately before `^prj`.
- Existing `[scheduled::...]` fields are never overwritten or removed.
- Terminal projects, `status: done` or `status: canceled`, are never scheduled.

A task without `[p::N]` is implicitly P0. The `^prj` task itself does not count
as an open P0 task, even though it normally carries `[p::2]`.

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
  scheduled bob  scheduled ^prj for 2026-06-11  no open P0 tasks
  warning outlive  active project has no ^prj task  add `- [ ] #task <completion criteria> [p::2] ^prj`

11 projects - 1 status updated - 1 scheduled - 1 warnings
```
