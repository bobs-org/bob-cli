# Archive done tasks from area and project notes

`bob task archive` moves closed `#task` blocks out of live area and project
notes into paired `done/` archives. It then links each source to its archive,
repairs archive metadata, and rewrites Obsidian links and dependency identities
that pointed at moved block ids. The Rust planner lives in
`src/native/collect_done/`. Nightly runs it after the leading `vault-sync` and
before articles, period-note problems, and the trailing `vault-sync`.

This file is the behavior contract. The keep window, rename following, and
plugin restore are later phases; this page describes the selection, pairing,
guarded-write, preview, and JSON rules that ship today.

## Command

```text
bob task archive [-n|--dry-run] [-f|--format human|json] [-t|--threshold N]
  -n, --dry-run         Plan and report only; no lock, writes, or Git
  -f, --format FORMAT   human (default) or json
  -h, --help            Show help
  -t, --threshold N     Minimum closed-root count per source note (default: 10)
```

```bash
bob task archive
bob task archive --threshold 1
bob task archive --dry-run
bob task archive -n -f json
```

`-f` takes its value separated (`-f json`, `--format json`) or attached
(`--format=json`, `-fjson`, `-f=json`). An unknown format or option is a
usage error: exit 2, nothing on stdout, no planning.

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
source. A block id that occurs more than once among one note's moved blocks is
never moved silently with unrepaired links: every block carrying that id is
**held** with reason `duplicate_block_id`, the other moves still apply, and the
run exits 1 with reason `duplicate_block_id` so nightly flags it. A closed
`#task` under an open parent is not a root: it stays with that parent and is
counted under skips as `open_parent`. A closed `^prj` line is skipped as `prj`
and is never moved; siblings still evaluate on their own.

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
ids against the existing archive receive a numeric suffix (`^x` → `^x-1`); a
moved block's own pathless links (`[[#^x]]`, with alias and `!`) are rewritten
to the final id before the append so they do not resolve to the older archived
`^x`.

### Exact replay

Before any id is renamed, each selected root is compared with the complete
blocks already in its archive. A root **replays** only on an exact match: the
same root line, the same optional root id, and byte-identical body (children,
notes, and line endings). A substring, an id-only match, or a match after
normalizing text is not a replay; a different body that reuses an id takes the
numeric-suffix rename above.

A replayed root is removed from the source, appends nothing to the archive, and
renames nothing; inbound links still repair into the archive. Replayed roots
still count toward the threshold. This is what finishes an interrupted run (see
[Recovery](#recovery)): a second successful run is a no-op.

## Link repair

After a move, wikilinks, embeds, aliases, Markdown block links, and matching
`[id::]` / `[dependsOn::]` tokens that pointed at a moved unique block id are
rewritten to the archive path and final id. Note targets follow the shared
`NoteIndex` case rules: exact vault-relative paths match case-sensitively and
basename stems match case-insensitively, the way Obsidian resolves them. The
scan includes `done/` notes and daily notes. A moved block's pathless heading
link (`[[#Heading]]`) whose heading stayed in the source becomes
`[[<source>#Heading]]`, and a pathless `[[#^id]]` link inside a moved block
whose target stayed behind gains the source note path. Fenced blocks,
frontmatter, and inline code spans are never rewritten. Self-healing also
rewrites stale source-path links that already belong to an existing archive.

## Same-write grouping

Every source note the run rewrites is regrouped in the same guarded write, but
only when `grouping_eligible_note` accepts it. The final source postimage goes
through the same status-group transform `bob task reconcile` uses, so the empty
`Done & Canceled` group and its badge chip disappear in the same write that
removed the closed tasks. This also covers a source touched only by link or
metadata repair. A source whose grouping changes is a structural rewrite and
waits out the guarded quiet period. A grandfathered note with no `type:`
frontmatter is never regrouped. Archive does not run full reconcile and never
changes task statuses, logs, priority, or schedule inputs.

## Verification

Planning reads one captured input set: every source note, repair-scope note,
absent archive destination, and the Tasks settings are read once, and every
decision (selection, pairing, replay, link repair, dependency metadata) comes
from those bytes. Before any output is installed, the final postimages are
validated plan-wide:

- each moved final block id resolves exactly once at its destination archive
  (nested ids and replays included) and no moved occurrence remains at its
  source; equal bare ids in unrelated notes are legal;
- no proposed archive postimage holds an open `#task` (unchanged historical
  archives are not inspected here).

A violation refuses the whole plan with the path, line, and id where known: exit
1, reason `plan_refused`, nothing written.

## Guarded writes, lock, and retries

Writes go through the guarded writer with tool name `task-archive`. Standalone
`bob task archive` owns the shared maintenance lock (`bob_sync.lock`) from before
planning through Git completion. It waits up to 30 seconds for a competing run;
on timeout it reports reason `lock_timeout` and exits 1. `bob nightly` already
holds that lock for its whole run, so its archive step plans and applies without
acquiring it again and without another sync cycle. A dry run never takes the
lock.

Archive outputs are installed before source removals, then repair-only outputs,
so an interruption always leaves a recoverable copy of every moved block. When
the vault changed after planning (`vault_changed`, `quiet_period`, or
`unstable_read`) and **no** output has been installed yet, the entire plan is
rebuilt from a fresh capture and retried inside a separate 30-second budget, so
a concurrent edit is never overwritten. When the budget runs out the run exits
1 with reason `retry_exhausted` and nothing is written.

### Recovery

A partial write stops at once. The run reports the exact applied and deferred
paths and the recovery directory, skips Git, and exits 1 with reason
`partial_apply`. It never replays automatically; rerunning `bob task archive` is
an explicit, safe later step, because exact replay detects the archive copies
already installed and only removes the moved blocks from the source. Recovery
state lives under `$XDG_STATE_HOME/bob-cli/task-archive/`.

## Preview

`--dry-run` captures, transforms, verifies, and reports; that is all. It
succeeds while another process holds the maintenance lock, never creates the
lock file, never probes Git, and creates no recovery state, archive directory,
or file. A validation refusal still reports the failure and exits 1. Human
output lists `moves` and the same counts a real run would apply, labeled as
planned.

## JSON

`-f json` prints exactly one JSON document on stdout (compact, one line) for
every run, including runtime failures; progress and subprocess diagnostics go
to stderr. Usage errors are plain stderr with exit 2. The schema is
`schema_version: 1`; additions stay additive.

| Section | Contents |
| --- | --- |
| `schema_version` | `1` |
| `dry_run` | Whether the run was a preview |
| `vault`, `threshold` | Vault root and effective threshold |
| `scan` | Counts for the plan (`null` when planning failed): `markdown_files`, `files_meeting_threshold`, `task_blocks`, `source_done_tasks_updates`, `archive_metadata_repairs`, `moved_block_ids`, `ambiguous_moved_block_ids`, `moved_block_id_renames`, `planned_bytes` |
| `moves` | Deterministic list ordered by `source`; each entry has `source`, `archive`, `task_count`, `writes_source`, `writes_archive`, `creates_archive`, `roots`, `block_ids`, `id_renames`, `replays` |
| `holds` | `path`, one-based `line`, nullable `block_id`, `reason` (`open_subtask` or `duplicate_block_id`) |
| `skips` | Reason-to-count object |
| `grandfathered` | Path list |
| `link_repairs` | `files` (affected paths), `link_count`, `dependency_metadata_count` |
| `git` | See below |
| `exit` | See below |

In a move, `roots` lists every selected root as `{line, block_id}` (original
one-based source line, nullable root id); `replays` is the subset of those that
were exact replays (same shape); `block_ids` is every final destination id; and
`id_renames` is `[{from, to}]` for ids that took a numeric suffix. `moves` is
the **plan**: it says what the run intended. Only `exit.applied_paths` says what
changed on disk. Paths in `moves`, `holds`, `link_repairs`, and `git.paths` are
vault-relative with `/` separators.

`git`:

| Field | Meaning |
| --- | --- |
| `outcome` | `not_run`, `skipped`, `pushed`, or `failed` |
| `skip_reason` | `dry_run`, `no_changes`, `not_worktree`, `git_missing`, `nothing_to_commit`, or `apply_incomplete` (a run that did not finish applying); `null` otherwise |
| `warning` | Message for a skipped non-worktree or missing `git`; else `null` |
| `committed`, `pushed` | Whether each step completed |
| `commit_message` | The commit subject when committed |
| `paths` | Vault-relative paths staged and committed |
| `failure` | `null`, or `{step, message, exit_code}` for the failed Git step |

`exit`:

| Field | Meaning |
| --- | --- |
| `code` | The process exit code |
| `outcome` | `preview`, `noop`, `applied`, or `failed` |
| `reason` | `null` on success; otherwise `lock_timeout`, `lock_error`, `plan_failed`, `plan_refused`, `retry_exhausted`, `write_failed`, `partial_apply`, `git_failed`, or `duplicate_block_id` (moves applied, exit 1) |
| `message` | Human-readable failure detail, else `null` |
| `applied_paths` | Vault-relative paths actually installed, in apply order |
| `deferred_paths` | Planned paths not installed (partial apply, retry exhaustion) |
| `recovery_directory` | Absolute recovery directory, or `null` when none exists |

Exit codes: 0 for preview, no-op, and a fully applied run; 1 for every failure
except `git_failed`, which exits with the failing Git process's code; 2 for
usage errors.

## Git

When the vault is a Git worktree, a complete apply commits exactly the paths it
actually installed as `bob task archive <YYYY-MM-DD>` and pushes, leaving
unrelated staged or dirty files alone. The path list comes from the final
successful plan, never from a retried-away attempt. A vault that is not a
worktree is written with a warning and no commit. An empty plan writes nothing
and skips Git entirely. A partial write skips Git.

A Git failure exits nonzero and says so truthfully: the vault changes are
already applied, the archive is not rerun, and `exit.reason` is `git_failed`.

## What reconcile still owns

`bob task reconcile` still owns Pomodoro-driven lane status, grouping under
`## Tasks` for notes archive did not touch, badge snapshots, and Blocked
recovery. Archive regroups only the sources it rewrites (see
[Same-write grouping](#same-write-grouping)) and never changes open-task
checkboxes, logs, priority, or schedule inputs.

See [Move open tasks between notes](task-move.md) for moving live tasks, and
[Task Status Hooks](task-status-hooks.md) for grouping after a collect.
