# Move open tasks between notes

`bob task move` moves selected open `#task` blocks into an area or eligible
project note. It keeps task content and review metadata intact while repairing
references and dependency identities across the Markdown vault. Capture inbox
routing reuses this engine inside the capture batch: an `--inbox-route`
answer moves the candidate exactly as a move would (see
[capture.md](capture.md#routing-inbox-tasks-out)).

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

## Refusals and placement

The command validates every selector and destination before writing. It refuses
missing or ambiguous notes, absolute or escaping paths, unsupported filesystem
objects, identical source and destination, and either note under `done/`. It
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

## Identity and reference repair

Existing task status, title, priority, schedule, `[fresh::]`, `[keeps::]`,
`[unblocked::]`, tags, and authored children are preserved. The command does
not stamp freshness, change lanes, promote prerequisites, consume ledger links,
or run reconcile. A task without `[id::]` remains without it.

If an incoming block id collides with an anchor already in the destination, the
moved block gets the next available suffix. The report lists every rename.
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
last. Replacements are atomic per file; a multi-file move is not one atomic
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
  `needs_reconcile`, and `warnings[]`.
- `git.mode` (`sync`, `offline`, `not_a_worktree`, or `dry_run`) and nullable
  `git.pre_sync`, `git.commit`, and `git.post_sync` stage records. Stages not
  run are `null`.
- `applied_paths[]`, `deferred_paths[]`, and `recovery_directory` for write
  outcomes.
- `error` as `null` on success or `{stage, message, hint}` on failure. Usage
  errors are handled by clap and exit 2; operational errors exit 1.

`moves[]` records source and destination paths, source line, title, nullable old
and new block ids, and nullable old and new dependency ids. Rename records
include destination path and old/new block ids. Repaired-file records include
`path`, `link_count`, and `dependency_token_count`. The Git commit record
contains `sha`, `subject`, and the exact `paths`; pre-sync contains `ok`,
`files_committed`, and `error`; post-sync contains `ok`, `pushed`, `conflicts`,
and `error`.
