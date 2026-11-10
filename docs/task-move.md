# Move open tasks between notes

`bob task move` moves selected open `#task` blocks into an area or eligible
project note. It keeps task content and review metadata intact while repairing
references and dependency identities across the Markdown vault. Capture inbox
routing reuses this engine inside the capture batch: an `--inbox-route`
answer moves the candidate exactly as a move would (see
[capture.md](capture.md#routing-inbox-tasks-out)).

Moving a task without an explicit `[type::]` field to a note whose chain
yields a different inherited type relocates its live Task Link via the
shared relocation rule (D8): a backlog task never gains a link, an already
agreeing link stays, a running sole-link entry renames in place, a running
shared entry gains ` + TYPE`, otherwise the link moves to the entry
`place(TYPE)` would choose, renames a sole-link placeholder, or creates a
`— TYPE` placeholder directly below the source. A task with an explicit
field keeps its type, so no relocation happens. Frontmatter `parent:`,
`task_type:`, and config edits are not gestures; the `type_mismatch` lint
covers them. Type creations are never refused: the relocation always
applies and the whole edit is written.

## Command

```text
bob task move [OPTIONS] <SOURCE> <DEST> (-b ID | -m TEXT)...
  -b, --block ID               Select by trailing block id without ^; repeatable
  -d, --dry-run                Plan and print only
  -f, --format human|json      Default: human
  -h, --help                   Show help
  -m, --match TEXT             Select by literal task-line substring; repeatable
  -o, --offline                Skip both sync cycles; commit locally
  -r, --retry-timeout SECONDS  Lock wait and re-plan budget; default 60, 0 fails fast
```

`SOURCE` and `DEST` accept a note stem or vault-relative path, with or without
`.md`. Paths must resolve uniquely inside `BOB_DIR`. A bare stem that matches
more than one note is ambiguous and lists every candidate; an explicit root
filename (`source.md`) or directory path (`folder/source`) selects one of them.
A missing explicit path such as `missing/source` fails; it does not fall back
to another note with the same basename.

Selectors can be combined (`-b` and `-m` together). Each one must identify
exactly one top-level open task line with checkbox `[ ]`, `[*]`, `[/]`, or
`[?]`. `--match` is literal and case-sensitive. Repeated and overlapping
selectors are deduplicated, and selected blocks move in source order. A match
can select a task without an Obsidian block id; the command does not create
one.

```bash
bob task move --dry-run -f json inbox projects/launch -b draft -m 'Check readiness'
bob task move inbox projects/launch -b draft -o
```

## Many tasks into one note (`-t`)

`bob task move -t <NOTE> <TASK>...` moves many tasks into one note in typed
order, on the shared command shell. Each positional is a task locator
(`note:block-id`, split on the last `:`; `[[note#^id]]` is also accepted).
The run plans one `plan_task_move` per locator against staged snapshots, in
typed order, with freshness stamped and Task Links relocated exactly as
`Ctrl+Shift+M` does. A source already in the destination note is refused
(`move_same_note`).

```bash
bob task move -t db inbox:bench work:pgdump
```

`move -t db:upgrade` is refused as `move_to_task` with "to fold tasks into
db:upgrade, use `bob task merge -i db:upgrade …`". A bare `@c` transfer
destination (`@a+x > @c`) and `bob task move -t` share this path; a task,
drawer, or section destination (`> @c+id[#drawer]`, `> @c#section`) merges
instead (see [Merge tasks into a task, drawer, or section](task-merge.md)).
The command commits as `bob task move -> db (2 tasks)` and reuses the move
report with a `↪ Moved N tasks into db.md › Tasks` header.

## Refusals and placement

The command validates every selector and destination before writing. It refuses
missing or ambiguous notes, absolute or escaping paths, unsupported filesystem
objects, identical source and destination, and a destination under `done/`
(a source under `done/` moves only through
[bob task restore](task-archive.md#restore)). It
also refuses closed, nested, non-task, and `^prj` selections; an ambiguous block
identity; a non-canonical existing `[id::]`, including a stale extra field on
the same task; an encoded identity that collides with another live note
(`a/b.md#^x` and `a__b.md#^x` both encode `a__b__x`); an unrepresentable
required identity such as a path that cannot be encoded; and a selected task
whose managed Depends-On child targets the destination's `^prj`. Code fences,
frontmatter, and inline code are not live identities.

The destination must be an area, or a project with frontmatter status `wip` or
`waiting`. A project needs an existing `## Tasks` section. An area without one
gets `## Tasks` appended. Moved tasks are inserted at the end of the unfenced
`## Tasks` intake, after existing tasks, trailing prose, and the badge strip,
and before the first real `###` status-group heading or the next `##` section.
Fenced example tasks and headings do not count as intake. Deeper-indented
children, nested tasks, managed child blocks, and child anchors travel with the
selected task.

When the destination `^prj` is Blocked (`[?]` or `#prj_is_blocked`), only `[?]`
tasks can move there. For Ready, Next, and Pending tasks, use Ctrl+Shift+M when
the move also needs its interactive promotion and today's Task Link pruning.
The command does not reconcile status or project blocking automatically; run
`bob task reconcile` when the report says reconciliation is pending.

Capture project-note move bullets reuse the task-move engine with two
capture-only rules. `- ^id` and `- :id` place the existing task block at the
bullet's draft-order slot in the destination `## Tasks` section; normal
`bob task move` and inbox routing keep their intake placement. Capture refuses
an incoming task or descendant ID collision instead of applying the command's
suffix-renaming rule. The shared movable-root eligibility check exposes open,
top-level `#task` rows (including rows without IDs) and excludes closed,
nested, ambiguous, and `^prj` tasks. A blocked-at-birth project accepts only
Blocked tasks. Answers to `- ^^` expand server-side into those same move
bullets at the `^^` slot in parent order, and ID-less picks gain their minted
ID in the same guarded batch.

## Identity and reference repair

Existing task status, title, priority, schedule, `[fresh::]`, `[keeps::]`,
`[unblocked::]`, tags, and authored children are preserved. The command does
not stamp freshness, change lanes, promote prerequisites, consume ledger links,
or run reconcile. A task without `[id::]` remains without it.

If an incoming block id collides with an anchor already in the destination, the
moved block gets the next available suffix. The report lists every rename.
Capture move bullets use refuse-on-collision so the authored destination IDs
remain stable and the task is never silently renamed.
Existing canonical `[id::]` fields and exact matching `[dependsOn::]` tokens
are updated to the destination path and final block id. Links to moved blocks
are repaired across Markdown notes, including `done/`, daily notes, and `ref/`:
wikilinks, embeds, aliases, Markdown block links, and applicable pathless links.
Pathless links are resolved using the original note context so co-moved targets
and destination links retain their intended meanings.

## Preview, writes, and Git

`--dry-run` uses the live planner without acquiring the lock, syncing, probing
Git, creating recovery state, writing notes, or committing. Its output marks
the result as a plan. A live invocation acquires the shared `bob_sync.lock`
maintenance lock with bounded waiting, pre-syncs a Git vault, then scans and
plans the synchronized snapshot. Guarded writes validate the complete Markdown
read-set and membership, stage each replacement, wait through the structural
quiet period, and retry a fresh scan and plan after concurrent-change,
quiet-period, or unstable-read failures while budget remains.

The destination is written first, repaired link-only notes next, and the source
last. Capture batches extend this to a stable topological order: every move
destination is replaced before its source, including chained project-note
moves and inbox routing through the shared capture helper. Other touched files
retain first-touch order. Replacements are atomic per file; a multi-file move is not one atomic
transaction. If a write stops partway through, the command does not replay the
selectors. It reports exactly which paths were applied and deferred, preserves
the writer's recovery directory, and requires inspection before another move
attempt.

After a successful write, a Git vault commits exactly the changed paths in one
`bob task move …` commit and runs a post-sync cycle. Pre-sync changes are
committed separately by vault-sync. `--offline` keeps the lock and scoped local
commit but skips both sync cycles. A non-Git vault is written safely with a
warning and no commit or sync. Commit or post-sync errors leave the local write
in place and are reported. If a write already stopped partway through, a later
commit or sync failure is added as a warning on that same report; the primary
partial-apply error stays in `error`, the exit status stays nonzero, and the
command does not imply the move completed.

## Output

Human output lists each task, source line and destination, block-id renames,
per-file link and dependency-token counts, the commit SHA when present, and the
reconcile hint when needed. Set `NO_COLOR` to disable color.

`-f json` writes exactly one JSON object to stdout; progress and warnings go to
stderr. Schema version 1 contains:

- `ok`, `dry_run`, `moves[]`, `renamed_block_ids[]`, `repaired_files[]`,
  `needs_reconcile`, `warnings[]`, and `relocated[]` (omitted when empty).
- `git.mode` (`sync`, `offline`, `not_a_worktree`, or `dry_run`) and nullable
  `git.pre_sync`, `git.commit`, and `git.post_sync` stage records. Stages not
  run are `null`.
- `applied_paths[]`, `deferred_paths[]`, and `recovery_directory` for write
  outcomes.
- `error` as `null` on success or `{stage, message, hint}` on failure. Usage
  errors are handled by clap and exit 2; operational errors exit 1.

`moves[]` records source and destination paths, source line, `destination_line`
(the 1-based landing line), `destination_section` (the H2 the landing line sits
under, or null), `status_symbol`/`status_name` (the moved task's final status),
title, nullable old and new block ids, and nullable old and new dependency ids. Rename records
include destination path and old/new block ids. Repaired-file records include
`path`, `link_count`, and `dependency_token_count`. `relocated[]` records
each moved link as `{route, block_id, from, to, type}` where `from` is the
source entry name (null when unnamed), `to` the destination entry name, and
`type` the new inherited type name; human output appends
`· link moved to TYPE` to each relocated move. The Git commit record
contains `sha`, `subject`, and the exact `paths`; pre-sync contains `ok`,
`files_committed`, and `error`; post-sync contains `ok`, `pushed`, `conflicts`,
and `error`.
