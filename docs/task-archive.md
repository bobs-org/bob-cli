# Archive done tasks from area and project notes

`bob task archive` moves closed `#task` blocks out of live area and project
notes into paired `done/` archives. It then links each source to its archive,
repairs archive metadata, and rewrites Obsidian links and dependency identities
that pointed at moved block ids. The Rust planner lives in
`src/native/collect_done/`. Nightly runs it after the leading `vault-sync` and
before articles, period-note problems, and the trailing `vault-sync`.

This file is the behavior contract. Preview, JSON, the keep window, rename
following, and plugin restore are later phases; this page describes the
selection and pairing rules that already ship.

## Command

```text
bob task archive [-t|--threshold N]
  -h, --help        Show help
  -t, --threshold N Minimum closed-root count per source note (default: 10)
```

```bash
bob task archive
bob task archive --threshold 1
```

`-t` still counts only archivable roots in one source note. A note below the
threshold is left unchanged unless it already owns a paired archive (then
source `done_tasks` and archive `parent` / `type` metadata can still be
repaired).

## Eligibility

A Markdown note feeds `done/` only when
`task_status_hooks::grouping_eligible_note` accepts it: frontmatter `type:`
names `[[area]]` or `[[project]]`, and the path is not a canonical
`YYYY/YYYYMMDD.md` daily note or today's day file.

Finished projects are skipped entirely. A project is finished when its `^prj`
line is closed (`[x]` / `[X]` / `[-]`) or its frontmatter `status` is
terminal (`done` or `canceled`). Remaining closed tasks on that note stay put.

A note with no `type:` frontmatter that already owns its paired archive
(today `obsidian.md` → `done/obsidian_done.md`) stays eligible and is reported
as grandfathered. Every other note is skipped and counted by reason:

| Reason | When |
| --- | --- |
| `daily` | Canonical daily path or today's day file |
| `finished_project` | Closed `^prj` or terminal project status |
| `not_area_or_project` | Missing or non-area/project `type:` |

Source discovery uses `task_status_hooks::markdown_files`: it skips `done/`,
every dot-directory, and the always-excluded directories (`_conflicts`,
`_generated`, `_templates`, `.bob`, `.git`, `.obsidian`). Link repair still
walks `done/` and still skips dot-directories.

Closed and open statuses come from
`.obsidian/plugins/obsidian-tasks-plugin/data.json` via
`task_status_hooks::read_tasks_settings`. Closed means `Done` or `Cancelled`.
Open means `Todo`, `In Progress`, or `On Hold`. Default settings still treat
`x`, `X`, and `-` as closed.

YAML frontmatter, fenced code, and `%%` / `<!-- -->` comments are never task
lines.

## Archivable unit, holds, and skips

A closed `#task` line moves with its indented block only when:

- no ancestor list item is an open `#task`;
- no descendant line is an open `#task`;
- the line does not carry the trailing `^prj` anchor.

Untagged checklist items and prose under that root travel with it.

A closed root with an open `#task` descendant is **held**. The run names it as
`path:line` with reason `open_subtask` and leaves the whole block in the
source. A closed `#task` under an open parent is not a root: it stays with
that parent and is counted under skips as `open_parent`. A closed `^prj` line
is skipped as `prj` and is never moved; siblings still evaluate on their own.

The human scan report always prints `holds`, `skips`, and `grandfathered`
after the file counts.

## Destination and pairing

The archive path is `done/<source-parent>/<stem>_done.md`. A new archive
opens with:

```yaml
---
parent: "[[source/path]]"
type: "[[done]]"
---
```

The source gains `done_tasks: "[[done/..._done]]"` in frontmatter. An existing
archive appends newly moved blocks after its current body. Duplicate block
ids in the archive receive a numeric suffix; ambiguous moved ids do not
rewrite links.

## Link repair

After a move, wikilinks, embeds, aliases, Markdown block links, and matching
`[id::]` / `[dependsOn::]` tokens that pointed at a moved unique block id are
rewritten to the archive path and final id. The scan includes `done/` notes
and daily notes. Pathless `[[#^id]]` links inside a moved block whose target
stayed behind gain the source note path. Fenced and inline code are left
alone. Self-healing also rewrites stale source-path links that already belong
to an existing archive.

## Git

When the vault is a Git worktree, a non-empty plan commits exactly the
touched paths as `bob task archive <YYYY-MM-DD>` and pushes. A vault that is
not a worktree is written with a warning and no commit. An empty plan (no
moves, metadata repairs, or link repairs) writes nothing and skips Git.

Archive does not acquire the task-maintenance lock, does not dry-run, and
does not emit JSON. Those arrive with the guarded writer.

## What reconcile still owns

`bob task reconcile` still owns Pomodoro-driven lane status, grouping under
`## Tasks`, badge snapshots, Blocked recovery, and shrinking the gap after
archive empties `Done & Canceled`. Archive never regroups a note and never
changes open-task checkboxes. Run reconcile after archive when a source still
shows a stale Done group or badge counts.

See [Move open tasks between notes](task-move.md) for moving live tasks, and
[Task Status Hooks](task-status-hooks.md) for grouping after a collect.
