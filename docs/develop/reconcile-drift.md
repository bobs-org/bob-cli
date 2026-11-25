---
title: Reconcile drift meter
description: Replay vault commits through bob task reconcile --dry-run and classify note-local churn.
---

# Reconcile drift meter

`scripts/dev/reconcile-drift` is a read-only developer meter. It replays a window
of vault commits through `bob task reconcile --dry-run -f json` with `BOB_NOW`
pinned to each commit time, classifies the planned changes, attributes category
increments to the review-journal gesture that introduced them, and reports
`bob task tidy --dry-run` counts at the window's end. The live vault is never
written.

Run it from the bob-cli repository:

```bash
scripts/dev/reconcile-drift --vault ~/bob
scripts/dev/reconcile-drift --since '7 days ago' -f json
python3 -B scripts/dev/test_reconcile_drift.py
```

The vault defaults to `$BOB_DIR` or `~/bob`. The default window is the last
seven days. `--every N` samples every Nth commit. `--jobs` runs independent
replays concurrently. Commits whose Markdown-plus-Tasks-settings tree matches
the previously replayed commit are skipped.

Each snapshot is a `git archive` of `*.md` plus
`.obsidian/plugins/obsidian-tasks-plugin/data.json`. Reconcile sees that tree
with `BOB_NOW` set to the commit's committer time and
`BOB_VAULT_SYNC_LOCK_FILE` pointed at a scratch lock, so the live vault lock
is untouched. When `YYYY/YYYYMMDD.md` for that commit date is missing, the
meter points `BOB_DAY_FILE` at the latest daily note on or before that date
so a midnight archive still replays. A per-commit replay error is recorded
and the rest of the window continues.

## Categories

| Category | Reconcile JSON |
| --- | --- |
| status-group move | `grouped_task_sections[].moved_block_count` |
| badge row | a grouped section with no moved blocks |
| local schedule flip | `marked_blocked` with `local_schedule`, no recurrence |
| recurring spawn flip | `marked_blocked` with `local_schedule` and `[repeat::]` or 🔁 |
| LINKED FROM mention strip | `linked_from_line_updates` with `mentions_removed` |
| midnight wake or unblock | `unblocked` |
| ledger hygiene | duplicate lines, empty Pomodoros, struck/moved markers, settled, coalesced, type agreement |
| dependency or field healing | Depends-On, BLOCKS, LINKED FROM (non-mention), `#prj_is_blocked`, `[unblocked::]` |
| other | remaining planned changes, including lane promotions |

A category increment is how much that count grew relative to the previous
replayed snapshot. Heals (counts going down) are not subtracted from
introduced churn. The last snapshot's absolute counts are outstanding drift.
Mornings group commits by America/New_York calendar day; a day is a review
window when it has a `review.answer` journal event or a commit between 05:00
and 09:00. Attribution reads `.bob/stats/review/*.jsonl` lines added in the
commit as `{tier}:{outcome}`. A commit that births `YYYY/YYYYMMDD.md` without
a journal event is `midnight_rollover`. Otherwise the commit subject is used.

## Related

- [Task status hooks](../task-status-hooks.md)
- [Daily stats](../stats.md)
- [bob-cli CI](ci.md)
