# Archive done tasks from area and project notes

`bob task archive` moves closed `#task` blocks out of live area and project
notes into paired `done/` archives. It then links each source to its archive,
repairs archive metadata, and rewrites Obsidian links and dependency identities
that pointed at moved block ids. The Rust planner lives in
`src/native/collect_done/`. Nightly runs it after the leading `vault-sync` and
before articles, period-note problems, and the trailing `vault-sync`.

This file is the behavior contract for selection, the keep window, pairing,
rename following, guarded writes, preview, JSON output, task restore, and
the done/ path rule.

## Command

```text
bob task archive [-n|--dry-run] [-f|--format human|json] [-t|--threshold N]
  -n, --dry-run         Plan and report only; no lock, writes, or Git
  -f, --format FORMAT   human (default) or json
  -h, --help            Show help
  -t, --threshold N     Optional minimum selected-root batch per source note
```

```bash
bob task archive
bob task archive --threshold 2
bob task archive --dry-run
bob task archive -n -f json
```

`-f` takes its value separated (`-f json`, `--format json`) or attached
(`--format=json`, `-fjson`, `-f=json`). An unknown format or option is a
usage error: exit 2, nothing on stdout, no planning.

For each source note, Bob keeps the ten newest dated closures from the last
seven calendar days. Older closures move to `done/`; when more than ten roots
closed during that window, the older roots in the window move too. An explicit
`-t` sets the minimum number of selected roots that must be ready to move from
one source before any of them move. Without `-t`, there is no batch minimum.
Exact archive replays remain eligible so a prior interrupted write can remove
its source copy safely.

New archive content is grouped under `## YYYY-MM` headings based on each
root's close date. Existing archive bodies stay in place; Bob does not regroup
old flat content. Undated or future-dated roots stay in their source because
their age and close month cannot be established safely. An eligible source
with an existing archive can still receive metadata repairs even when no
roots are selected.

## Eligibility

A Markdown note feeds `done/` only when
`task_status_hooks::grouping_eligible_note` accepts it: frontmatter `type:`
names `[[area]]` or `[[project]]`, and the path is not a canonical
`YYYY/YYYYMMDD.md` daily note or today's day file.

Finished projects are skipped entirely. A project is finished when its `^prj`
line is closed (`[x]` / `[X]` / `[-]`) or its frontmatter `status` is
terminal (`done` or `canceled`). Remaining closed tasks on that note stay put.

A note with no `type:` frontmatter that already owns a bound archive stays
eligible and is reported as grandfathered. The bound archive may be
noncanonical. Every other note is skipped and counted by reason:

| Reason | When |
| --- | --- |
| `daily` | Canonical daily path or today's day file |
| `finished_project` | Closed `^prj` or terminal project status |
| `not_area_or_project` | Missing or non-area/project `type:` |

Source discovery uses `task_status_hooks::markdown_files`: it skips the
top-level `done/` archive, every dot-directory, and the always-excluded
directories (`_conflicts`, `_generated`, `_templates`, `.bob`, `.git`,
`.obsidian`). Link repair still walks `done/` and still skips
dot-directories. A nested folder named `done` is not an archive and is
not skipped.

Closed and open statuses come from
`.obsidian/plugins/obsidian-tasks-plugin/data.json` via
`task_status_hooks::read_tasks_settings`. Closed means `Done` or `Cancelled`.
Open means `Todo`, `In Progress`, or `On Hold`. Default settings still treat
`x`, `X`, and `-` as closed.

YAML frontmatter, fenced code, and `%%` / `<!-- -->` comments are never task
lines.

## Done path

A vault path is a `done/` archive when its first normal path component
is exactly `done`, compared case-sensitively. `done/x_done.md` and
`done/2026/20260528_done.md` are archives. A root `done.md`, a nested
`projects/done/`, `Done/`, `.done/`, and `donex/` are not. Every Rust
path check and note walker uses this rule; walkers skip only the
top-level `done/` directory.

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

The human scan report always prints `holds`, `skips`, `grandfathered`, and
`diagnostics` after the file counts.

## Destination and pairing

Pairing is metadata-first. One shared pair index is built from the captured
census before eligibility filtering. Reciprocal `done_tasks` / `parent`
frontmatter wins even when the archive filename is old; unique filename
fallback applies only when no metadata contradicts it. Every bound pair is
resolved before eligibility can skip its source, so daily, reference,
untyped, and finished-project sources keep archive maintenance without
becoming eligible for task extraction.

A binding conflict (contradictory or ambiguous metadata, two archives claiming
one source, two sources claiming one archive, or an occupied canonical
destination) skips writes and inferred historical link repairs for the
affected pair. Unrelated unambiguous work still proceeds, and the run exits 1
with reason `archive_binding_conflict`.

A new archive opens with:

```yaml
---
parent: "[[source/path]]"
type: "[[done]]"
---
```

The source gains `done_tasks: "[[done/..._done]]"` in frontmatter. An existing
archive appends newly moved blocks after its current body, under close-month
headings. Duplicate block
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
count toward an explicit minimum batch. This is what finishes an interrupted run (see
[Recovery](#recovery)): a second successful run is a no-op.

## Rename following

A bound noncanonical archive moves to `done/<source-parent>/<stem>_done.md`
in the same run that moves tasks. The canonical destination is created from
the captured old archive with its body, custom frontmatter, line endings, and
block ids preserved verbatim; only the managed `parent`, `type`, and source
`done_tasks` fields are updated. An occupied destination refuses the rename
even when its contents look identical: diagnosing duplicates never licenses
deleting a copy.

One overlay carries rename maintenance, current task moves, and historical
repairs. Inbound note, heading, and block links to the old archive are
retargeted with alias, embed, and fragment syntax preserved, covering wikilinks
and Markdown destinations; targets resolve against the pre-rename index so
shortened and case-variant forms still identify the old note, and an ambiguous
basename is never guessed. Archived anchors keep their block ids while their
path-qualified `[id::]` / `[dependsOn::]` identities move to the canonical
path, composed with current-move id suffixing; no source-of-truth dependency
link may point at the removed archive. Generic repairs stay out of fenced
blocks, frontmatter, inline code spans, and `%%` / `<!-- -->` comments; managed
YAML links are updated deliberately. Drafted period review digests survive via
the existing carry helper.

The final overlay is verified with old paths removed: renamed archives join the
existing postimage checks (including rename-only runs), and every changed path
is emitted once. Destination archives install first, then source and repair
outputs, then guarded old-path removals with before-images in the recovery
manifest. Both created and removed paths flow through the touched-path Git
commit. A successful second run is a no-op with no renames planned.

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

A partial rename recovers separately from that task-block replay guarantee.
When the deletion did not complete, both the canonical copy and the old path
stay on disk with the recovery manifest holding the before-image of every
operation (a missing before-image for the create, the captured original bytes
for the removal). A later ordinary run refuses the unresolved duplicate claims
instead of guessing which copy is authoritative; an explicit rerun after
inspecting the recovery directory completes the move without duplicating
blocks.

## Preview

`--dry-run` captures, transforms, verifies, and reports; that is all. It
succeeds while another process holds the maintenance lock, never creates the
lock file, never probes Git, and creates no recovery state, archive directory,
or file. A validation refusal still reports the failure and exits 1. Human
output lists `moves`, `renames`, and the same counts a real run would apply,
labeled as planned.

## JSON

`-f json` prints exactly one JSON document on stdout (compact, one line) for
every run, including runtime failures; progress and subprocess diagnostics go
to stderr. Usage errors are plain stderr with exit 2. The schema is
`schema_version: 2`; the keep-window change makes `threshold` nullable and
renames the scan count to `files_selected`.

| Section | Contents |
| --- | --- |
| `schema_version` | `2` |
| `dry_run` | Whether the run was a preview |
| `vault`, `threshold` | Vault root and nullable explicit minimum batch (`null` when `-t` was omitted) |
| `scan` | Counts for the plan (`null` when planning failed): `markdown_files`, `files_selected`, `task_blocks`, `source_done_tasks_updates`, `archive_metadata_repairs`, `moved_block_ids`, `ambiguous_moved_block_ids`, `moved_block_id_renames`, `archive_renames`, `planned_bytes` |
| `moves` | Deterministic list ordered by `source`; each entry has `source`, `archive`, `task_count`, `writes_source`, `writes_archive`, `creates_archive`, `roots`, `block_ids`, `id_renames`, `replays` |
| `holds` | `path`, one-based `line`, nullable `block_id`, `reason` (`open_subtask` or `duplicate_block_id`) |
| `skips` | Reason-to-count object |
| `grandfathered` | Path list |
| `link_repairs` | `files` (affected paths), `link_count`, `dependency_metadata_count` |
| `renames` | Deterministic list ordered by `source`; each entry has `source`, `from` (old archive), `to` (canonical archive) |
| `diagnostics` | See below |
| `git` | See below |
| `exit` | See below |

`diagnostics` is present on every captured run, including preview, no-op, and
plan refusal (`scan` may then be `null` and `moves` empty). Capture failures
set `complete` to `false` so an empty finding list is not a clean census.

| Field | Meaning |
| --- | --- |
| `complete` | Whether the census finished |
| `counts` | Every diagnostic code mapped to its finding count, including zeros |
| `findings` | Deterministic list of `{code, path, line, block_id, related}` |

`line` is a one-based content line or `null`. `block_id` is the trailing
anchor when one exists. `related` lists competing claimant paths. Codes:

| Code | Meaning |
| --- | --- |
| `archive_orphan` | An archive has no resolvable source |
| `archive_duplicate` | Two or more archives claim one source; `related` lists the claimants. Writes and inferred historical repairs for that source skip |
| `open_task_in_archive` | A content `#task` in `done/` is open under captured Tasks settings |
| `duplicate_block_id_in_done` | An anchor occurs more than once in one archive note |
| `undated_closed_row` | An eligible source's closed root has no valid `ref_tasks::close_date` (`YYYY-MM-DD` from `[completion::]`, `[cancelled::]`, `✅`/`✔`, or `❌`/`✖`). Counted independently of the batch threshold |
| `archive_binding_conflict` | Other unsafe bindings (contradictory or ambiguous metadata, competing sources, one-sided rename, occupied destination) |

Health warnings (`archive_orphan`, `open_task_in_archive`,
`duplicate_block_id_in_done`, `undated_closed_row`) do not change exit status.
`archive_duplicate` and `archive_binding_conflict` skip writes for the
affected source and set `exit.reason` to `archive_binding_conflict` with exit
1; unrelated unambiguous work still proceeds.

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
| `reason` | `null` on success; otherwise `lock_timeout`, `lock_error`, `plan_failed`, `plan_refused`, `retry_exhausted`, `write_failed`, `partial_apply`, `git_failed`, `duplicate_block_id` (moves applied, exit 1), or `archive_binding_conflict` (unrelated unambiguous work may still apply, exit 1) |
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

## Restore

`bob task restore` moves selected closed `#task` blocks out of a `done/`
archive back into a live note. It reuses the move selector grammar, plan,
guarded apply, lock, sync, commit, and report: moving from an archive repairs
links vault-wide exactly as any move does, which reverses the archive's link
repair, and it keeps the block's logs, children, and recorded time.

```text
bob task restore [OPTIONS] <SOURCE> (-b ID | -m TEXT)...
  -b, --block ID               Select by trailing block id without ^; repeatable
  -d, --dest NOTE               Restore into this area or project note instead
  -n, --dry-run                 Plan and print only
  -f, --format human|json       Default: human
  -h, --help                    Show help
  -m, --match TEXT              Select by literal task-line substring; repeatable
  -o, --offline                 Skip both sync cycles; commit locally
  -r, --reopen                  Restore the task open: set [ ] and remove its close field
```

```bash
bob task restore --dry-run done/work_done -b old-task
bob task restore done/work_done -m 'Quarterly review'
bob task restore done/work_done -b old-task -r
bob task restore -d projects/launch done/work_done -b old-task
```

The destination defaults to the archive's bound source through the pairing
resolver ([Destination and pairing](#destination-and-pairing)). With no
resolvable source and no `-d`, restore refuses (`restore_no_source`) instead
of guessing. Destinations in `done/` stay refused, as do non-area/project
notes, finished projects, and notebook notes.

With `-r/--reopen` the restored root returns open (`[ ]`) with its
`[completion::]`/`[cancelled::]` field removed. Without it the task returns
closed. Reopen never auto-links today's Pomodoros; reconcile derives Blocked
from the task's own dependencies and schedule afterwards.

Restore refuses recurring occurrences (`restore_recurring_task`), merged-task
tombstones (`restore_merge_tombstone`), and `^prj` lines (`move_project_task`).
A restore that empties a keep-window month heading (`## YYYY-MM` or
`## Undated`) removes that heading; other headings and fenced examples stay
untouched.

`-n` previews without locking, syncing, probing Git, or writing. `-f json`
prints the same `schema_version: 1` move report as
[bob task move](task-move.md#output), with the restored task's final status
in `moves[].status_symbol` and the `bob task restore …` commit subject.

## What reconcile still owns

`bob task reconcile` still owns Pomodoro-driven lane status, grouping under
`## Tasks` for notes archive did not touch, badge snapshots, and Blocked
recovery. Archive regroups only the sources it rewrites (see
[Same-write grouping](#same-write-grouping)) and never changes open-task
checkboxes, logs, priority, or schedule inputs.

See [Move open tasks between notes](task-move.md) for moving live tasks, and
[Task Status Hooks](task-status-hooks.md) for grouping after a collect.
