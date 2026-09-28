# Randomize

`bob randomize` re-rolls every due prioritized Obsidian task to its own
random date inside that task's configured priority window. After days or
weeks without reviewing tasks, the vault fills with overdue P1–P4 tasks that
bury the P0 work needing full attention. This command is the headless,
vault-wide version of the Obsidian `Ctrl+Shift+P` picker's same-level 🎲
"roll": it re-schedules the whole backlog at once, then publishes the change
as a single Git commit that cooperates with `bob vault-sync`.

## Contents

- [Usage](#usage)
- [What qualifies](#what-qualifies)
- [What a re-roll writes](#what-a-re-roll-writes)
- [Seeds and previews](#seeds-and-previews)
- [Recipes](#recipes)
- [Git and vault-sync](#git-and-vault-sync)
- [Output](#output)
- [Failure modes](#failure-modes)
- [Environment and exit codes](#environment-and-exit-codes)

## Usage

```bash
bob randomize [-d|--dry-run] [-f|--format human|json] [-l|--level LABEL]...
              [-o|--offline] [-r|--retry-timeout SECONDS] [-s|--seed SEED]
              [-u|--until DATE|+N]
```

| Option | Behavior |
| --- | --- |
| `-d, --dry-run` | Preview the full plan without locking, syncing, or writing |
| `-f, --format` | `human` (default, colored on a TTY, `NO_COLOR` honored) or `json` |
| `-l, --level LABEL` | Repeatable. Only re-roll these configured labels (ASCII case-insensitive, e.g. `-l p2`). An unknown label is a usage error (exit 2) that lists the valid labels |
| `-o, --offline` | Skip both vault-sync cycles; commit locally without pushing |
| `-r, --retry-timeout SECONDS` | Budget for waiting on the maintenance lock and for re-planning after concurrent-edit races (default `60`; `0` = fail fast) |
| `-s, --seed SEED` | Base seed (decimal or `0x` hex). A dry run prints the seed that reproduces its dates |
| `-u, --until DATE\|+N` | Treat tasks scheduled through DATE (or N days from today) as due and roll their windows from that date. Must be today or later (exit 2 otherwise) |

There is no `--bob-dir`. The in-process vault-sync cycle is bound to
`BOB_DIR`, and a flag would let edits and sync target different vaults;
`nightly` and `vault-sync` make the same choice. There is no confirmation
prompt, because the dry run, seed replay, and one revertible commit already
make it safe.

Examples:

```bash
bob randomize --dry-run                 # Preview what would move and where
bob randomize --seed 0x7f3a91c2         # Apply the dates a dry run showed
bob randomize --level P2 --level P3     # Leave P1 tasks for hand triage
bob randomize --until +7                # Clear a week for P0 work
bob randomize --dry-run --format json   # Machine-readable preview
```

## What qualifies

The scan covers every Markdown note the vault walkers already scan, using
the same walker as `bob task-status-hooks`. That walker skips `done/`, dot
directories, `_conflicts`, `_generated`, and `_templates`. Task lines come
from the same scanner the `<ctrl+shift+enter>` toggle, `bob projects sync`,
and the picker use: the Tasks global filter (`#task` by default) applies,
and frontmatter and fenced code are skipped. Only the task's own line is
inspected; child lines are not. Inline fields are matched anywhere on that
line in both `[key:: value]` and `(key:: value)` forms, with any spacing
around `::`. Each task is classified in this order:

1. Ignored (not considered, not reported) when any of these holds:
   - it is not an open task by Tasks status type;
   - it has no `priority` field;
   - it has no `scheduled` field;
   - its block ID is `prj` (`bob projects sync` owns `^prj` scheduling).

   One exception: an open task with no `priority` and exactly one valid
   `scheduled` on or before today increments the summary's **still due P0**
   count.
2. More than one `priority` field or more than one `scheduled` field: skip
   as `duplicate_field`.
3. The single `scheduled` value is not a strict `YYYY-MM-DD` calendar date:
   skip as `invalid_scheduled`.
4. The `scheduled` date is after the cutoff (`--until`, default today):
   ignore it, since the task is not due.
5. The `priority` value (trimmed) matches no configured level `value`: skip
   as `unknown_priority`, with the value as detail.
6. A `due` or `repeat` inline field, or a `📅` or `🔁` emoji, is on the
   line: skip as `hard_date`. A deadline or recurrence makes `scheduled`
   more than a soft date.
7. Next `[*]`: skip as `next`. In Progress `[/]`: skip as `in_progress`.
   Any other open symbol besides Ready `[ ]` and Blocked `[?]`: skip as
   `other_status`.
8. The task's block ID is linked from any open Pomodoro in today's daily
   note: skip as `pomodoro`. A link counts at any depth of the entry's
   child block, including `[[target#^id]]`, `![[target#^id]]`, and aliased
   forms. The target matches the task's note by vault-relative path without
   `.md` or by file stem, both case-insensitive. An empty target means the
   daily note itself. Over-matching is acceptable because skipping is the
   safe side.
9. `--level` was given and the task's level is not selected: count it as
   `not_selected`.
10. Anything left is a **candidate**: a Ready `[ ]` or Blocked `[?]` task.

Skip reasons fall into two groups shown in the human output. **Left alone**
(`next`, `in_progress`, `pomodoro`, `not_selected`) is intentional and only
reported as counts. **Needs a look** (`duplicate_field`,
`invalid_scheduled`, `unknown_priority`, `hard_date`, `other_status`) is
listed with `path:line` so it can be fixed. Neither group is fatal.

The priority labels and windows come from `~/.config/bob/config.yml` (see
[projects.md](projects.md#priority-property-and-scheduled-rolls)):

| Label | Value | Window |
| ----- | ----- | ------ |
| P1 | `high` | 2–7 days |
| P2 | `medium` | 8–30 days |
| P3 | `low` | 31–90 days |
| P4 | `lowest` | 91–365 days |

## What a re-roll writes

Everything is computed in memory, one postimage per note:

1. Replace exactly the 10 date bytes of the `scheduled` value. Keep the
   bracket or paren form, the `::` spacing, and all other text.
2. When the new date is after today, a Ready `[ ]` task becomes Blocked
   `[?]` (and `[?]` stays `[?]`). This matches the status hooks would
   derive for a future date.
3. Write a Schedule Log entry in the picker's grammar, with a reason head
   that names the tool: `🎲 P2 randomize · in **21** (8–30) days`. With
   `--until` after today, `from <until>` is appended:
   `… · in **17** (8–30) days from 2026-10-12`.
   - If the task has a direct-child Schedule Log marker, the entry is
     prepended as that marker's first child (newest first). Legacy
     `**Schedule log**` markers are recognized too.
   - Otherwise `- 🗓️ **SCHEDULE LOG**` plus the entry is appended as the
     task's last direct child, which is how the picker creates a missing
     log.
4. Edits apply bottom-up so offsets stay valid. Each line's `\n` or `\r\n`
   ending and the file's final-newline state are preserved.
5. After all of a note's edits, status grouping runs once with hooks'
   classification, if the note is grouping-eligible. Eligible means
   `[[area]]` or `[[project]]` frontmatter, not a canonical
   `YYYY/YYYYMMDD.md` daily note, and not today's day file. Newly Blocked
   root tasks then move under `### Blocked` and the badge row is
   regenerated in the same write.

One task in a project note, before:

```markdown
## Tasks

- [ ] #task Review the onboarding draft [priority:: medium] [scheduled:: 2026-09-10] ^onboard-review
```

After (rolled 21 days into the P2 8–30 window):

```markdown
## Tasks
<!-- bob:task-status-badges:v1 -->
[`⚪ 0 open`](#Note#Tasks) · ... · [`🔴 1 blocked`](#Note#Tasks#Blocked) · ...

### Blocked
<!-- bob:task-status-group:v1:blocked -->

- [?] #task Review the onboarding draft [priority:: medium] [scheduled:: 2026-10-19] ^onboard-review
  - 🗓️ **SCHEDULE LOG**
    - *2026-09-10 → 2026-10-19* — 🎲 P2 randomize · in **21** (8–30) days
```

If any postimage sets `[?]`, the Tasks status registry must pass hooks'
Blocked-status validation. Otherwise the run fails before any write.

## Seeds and previews

**today** is the same effective day `task-status-hooks` uses: the date in
the `BOB_DAY_FILE` filename when it parses, otherwise the current date
(`BOB_NOW` overrides the clock). **until** is `--until DATE|+N`,
defaulting to today; it must be today or later. It is both the cutoff and
the base the windows are rolled from: **new date** = `until +
level.roll_offset(task_seed)`.

**task_seed** mixes the base seed with a stable 64-bit hash (FNV-1a) of
three inputs: the vault-relative path, the task line's digest, and the
ordinal among identical digests in that note. Dates therefore depend on
task identity, not scan order or line numbers. A dry run and a later live
run with the same `--seed` give every unchanged task the same date, even
after the pre-sync pulls in unrelated edits.

**base seed** is `--seed` (decimal or `0x`-hex u64), else
`BOB_PRIORITY_ROLL_SEED`, else a generated seed printed as 8 hex digits.
The seed is always printed as `0x…` hex.

The loop is: run `bob randomize --dry-run`, inspect the preview, then apply
exactly those dates with `bob randomize --seed <seed>` (repeating the same
`--level` and `--until` flags). If the new date equals the old date
(possible only with `min_days: 0`), the task is counted as `unchanged` and
nothing is written for it.

## Recipes

**Backlog after time away.** Preview first, then apply:

```bash
bob randomize --dry-run
bob randomize --seed 0x7f3a91c2
```

**Keep P1s for hand triage.** Re-roll only the lower levels:

```bash
bob randomize --level P2 --level P3 --dry-run
bob randomize --level P2 --level P3 --seed 0x7f3a91c2
```

**Clear N days for P0 work.** Roll every window from a week out instead of
today:

```bash
bob randomize --until +7 --dry-run
```

The **Next 5 weeks** sparkline in the human output (a `▁▂▃▄▅▆▇█` histogram
of all open tasks' post-run `scheduled` dates for today+1 … today+35, P0
included, plus the peak day) shows before anything is written whether the
narrow P1 2–7 day window piles too many tasks onto one day. When it does,
triage the P1s by hand first (`--level P2 --level P3`), or accept the pile
and work the peak day down.

## Git and vault-sync

A live run holds the shared `bob_sync.lock` maintenance lock (the same lock
vault-sync, nightly, and hooks use) while it runs this sequence:

```text
acquire lock           bounded wait (--retry-timeout), visible "waiting…" line
pre-sync               vault_sync cycle in-process: commits pending edits as
                       vault(<host>), recovers merges, fetches/merges, pushes.
                       On failure: abort, nothing written, exit 1, hint --offline
scan + plan            fresh from disk, pure
apply                  guarded writer: 2 s quiet period, snapshot preflight,
                       recovery copies, atomic renames. On vault_changed,
                       quiet_period, or unstable_read with nothing applied,
                       re-scan and re-plan with the same seed within the budget
commit                 git add -- <applied> ; git commit -F - -- <applied>
post-sync              vault_sync cycle again: fetch/merge if the remote moved,
                       push with retries
release lock
```

randomize never runs `add -A`, merges, rebases, amends, force-pushes, or
pushes on its own; vault-sync remains the only code that does those. After
a run, `master` holds, in order: an optional `vault(<host>)` commit from
the pre-sync, exactly one `bob randomize …` commit containing only the
rewritten notes, and an optional merge commit if the remote moved.

Commit message (follows the `bob move-done-tasks <date>` precedent):

```text
bob randomize 2026-09-28: 222 tasks in 28 notes

P1 85 · P2 132 · P3 5
until 2026-09-28 · seed 0x7f3a91c2

130 sase.md
 14 cash.md
…
```

`--offline` skips both sync cycles but still makes the scoped local commit;
the background vault-sync publishes it later. If the vault is not a Git
worktree, the notes are written with a warning and all Git steps are
skipped.

A post-sync conflict follows vault-sync's policy: the remote copy wins in
place and randomize's version is kept under `_conflicts/`. The command
prints a loud warning naming the notes, says that re-running re-rolls
whatever is still due, and exits 1.

If the push fails after the commit, the command prints
`committed <sha> locally; background vault-sync will publish it` and exits
1. It never rolls back good local edits. If the apply is partial (I/O
error after some renames), the notes that were written are committed (each
one is self-consistent), post-sync runs, and the command reports the
remaining notes and the recovery directory with exit 1. Re-running
converges.

Undo is `git -C ~/bob revert <sha> && bob vault-sync`. It reverts cleanly
because status and grouping live in the same commit.

Dry run: no lock, no sync, no writes, no recovery directory, no status
file.

Because the write holds the lock for only a few seconds, run it where you
edit: avoid typing in touched notes for the few seconds of the write. If
the guarded writer detects concurrent edits, it re-scans and re-plans with
the same seed within the `--retry-timeout` budget. Randomize never runs
from `nightly` or cron: silently deferring tasks would erase the overdue
signal without review.

## Output

The human output uses the same styling helper as the other commands.
Priority heat is P1 red, P2 yellow, P3 blue, P4 dim; dates are cyan; status
is ✓ green, ⚠ yellow, ✗ red; secondary text is dim. Without color, the
layout is identical without ANSI codes. Summary dates use `%a %b %-d`;
per-task lines use ISO dates to match the Schedule Log. Descriptions are
truncated with `…` so each line fits in 100 columns.

Live run:

```text
🎲 bob randomize · Mon Sep 28 · ~/bob

 ✓ Synced vault            committed 2 pending notes separately
 ✓ Re-rolled 222 tasks in 28 notes

   P1  high     85   Wed Sep 30 → Mon Oct 5     2–7 days
   P2  medium  132   Tue Oct 6 → Wed Oct 28     8–30 days
   P3  low       5   Thu Oct 29 → Sun Dec 27    31–90 days

   Next 5 weeks  ▃▅█▇▇▆▄▃▃▂▂▂▂▃▂▂▂▂▂▂▂▂▁▁▁▁▁▁▁▁▁▁▁▁▁   peak 26 · Fri Oct 2
   Notes         sase.md 130 · cash.md 14 · bob.md 12 · +25 more
   Left alone    1 next · 1 in progress
   Still due     40 P0 tasks

 ⚠ 2 tasks need a look
     sase.md:212   duplicate field
     bob.md:40     unknown priority "highest"

 ✓ Committed 4e5f6a7        bob randomize 2026-09-28: 222 tasks in 28 notes
 ✓ Pushed to origin/master

   seed 0x7f3a91c2 · undo: git -C ~/bob revert 4e5f6a7 && bob vault-sync
```

The level table lists only levels that re-rolled at least one task. Each
row shows the min…max of the new dates and the configured window. A line is
omitted when its count is zero. "Left alone" also shows
`N not selected` when `--level` filtered, and `N in today's
Pomodoros`. A dimmed "waiting for another vault maintenance run…" line
prints once when the lock is contended.

A dry run adds a `dry run` tag to the header, prints a per-note task list
first (notes ordered by count descending then path; each note shows as
`sase.md (130)` followed by one `P2  2026-09-10 → 2026-10-19
<description>` line per task), reads "Would re-roll…" in the summary, has
no git lines, and ends with:

```text
Nothing was written. Apply these dates with: bob randomize --seed 0x7f3a91c2
```

adding the same `--level` and `--until` flags when given.

Nothing due prints
`✓ Nothing to re-roll — no prioritized tasks are due by <until>.` plus the
"Left alone", "Still due", and "Needs a look" lines, and exits 0.

Errors go to stderr as `✗ <what failed>: <why>`, followed by one actionable
hint, such as "re-run with --offline to commit locally without syncing" or
"another maintenance run held the lock for 60 s; try again".

### JSON (`--format json`)

One document on stdout, even on failure. All progress and warnings go to
stderr in this mode. `line` and `ref` use the task's original (pre-edit)
position. `schedule_log` is `prepended` or `created`. `git.mode` is `sync`,
`offline`, `not_a_worktree`, or `dry_run`; sections that did not run are
`null`. On failure, `error` is
`{"stage": "lock|config|pre_sync|plan|apply|commit|post_sync", "message":
"…"}`.

```json
{
  "schema_version": 1,
  "ok": true,
  "dry_run": false,
  "today": "2026-09-28",
  "until": "2026-09-28",
  "seed": "0x7f3a91c2",
  "levels": null,
  "summary": {
    "rerolled": 222,
    "notes": 28,
    "unchanged": 0,
    "still_due_p0": 40,
    "by_level": [
      {
        "label": "P1",
        "value": "high",
        "count": 85,
        "min_days": 2,
        "max_days": 7,
        "first": "2026-09-30",
        "last": "2026-10-05"
      }
    ]
  },
  "tasks": [
    {
      "path": "sase.md",
      "line": 17,
      "ref": "17:1a2b3c4d",
      "block_id": null,
      "description": "…",
      "level": "P2",
      "value": "medium",
      "min_days": 8,
      "max_days": 30,
      "offset_days": 21,
      "from": "2026-09-10",
      "to": "2026-10-19",
      "status_from": " ",
      "status_to": "?",
      "schedule_log": "prepended"
    }
  ],
  "skipped": [
    { "path": "bob.md", "line": 40, "reason": "unknown_priority", "detail": "highest" }
  ],
  "notes": [{ "path": "sase.md", "tasks": 130, "regrouped": true }],
  "load": [{ "date": "2026-09-29", "count": 5 }],
  "warnings": [],
  "git": {
    "mode": "sync",
    "pre_sync": { "ok": true, "files_committed": 2, "error": null },
    "commit": {
      "sha": "4e5f6a7…",
      "subject": "bob randomize 2026-09-28: 222 tasks in 28 notes",
      "paths": ["bob.md", "cash.md", "sase.md"]
    },
    "post_sync": { "ok": true, "pushed": true, "conflicts": [], "error": null }
  },
  "recovery_directory": "…",
  "error": null
}
```

## Failure modes

- Missing or invalid priority config, or a missing/incompatible Blocked
  (`?`) Tasks status definition, fails before any write (exit 1).
- A lock held past `--retry-timeout` fails with exit 1 and no writes.
- A failed pre-sync aborts with nothing written (exit 1); re-run with
  `--offline` to commit locally without syncing.
- A post-sync conflict keeps the remote copy in place, stashes randomize's
  version under `_conflicts/`, warns loudly, and exits 1; re-running
  re-rolls whatever is still due and converges.
- A failed push after a good commit prints the local SHA and exits 1;
  background vault-sync publishes it later.
- A partial apply commits the notes that were written, post-syncs, reports
  the remaining notes and the recovery directory, and exits 1.

## Environment and exit codes

Environment: `BOB_DIR`, `BOB_NOW`, `BOB_DAY_FILE`, `BOB_CONFIG_FILE`,
`XDG_CONFIG_HOME`, `BOB_PRIORITY_ROLL_SEED`, `BOB_VAULT_SYNC_LOCK_FILE`,
`BOB_VAULT_SYNC_STATE_FILE`, `NO_COLOR`. See `bob randomize --help` for the
one-line role of each.

Exit codes: `0` is success, including "nothing to re-roll". `1` is runtime
failure: config errors, a missing Blocked status, lock timeout, pre-sync
failure, a partial apply, commit failure, a post-sync conflict, or a push
failure. `2` is usage error: clap errors, an unknown `--level`,
`--until` in the past, or an unparsable `--seed` or `--until`.
