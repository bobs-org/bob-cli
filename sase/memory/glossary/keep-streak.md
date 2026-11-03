---
keyword: Keep Streak
aliases:
  - "keeps"
---

The count of consecutive due reviews at which a human confirmed a task as written
on its current rung, stored as `[keeps:: N]` (1–999; absence means 0) right after
the task's `fresh`/`refresh` fields. Due keeps count on every rung — WIP on the
PENDING tier, Next on the NEXT tier, Ready and P-levels on ROTTEN or scheduled
TICKLER — once per review through the single keep helper; a keep on an unblock
tickler (`returned_by: unblocked`) leaves the streak unchanged. Any rung change
clears it, whoever makes it: human lane and priority writers stamp, and
reconcile's lane raises strip it in the same line write (`Unblock` back to Ready
preserves it); hand edits stay unobserved, and automation otherwise never touches
it. At the rung's allowance the next due confirmation opens the [[task-decay]]
card instead of stamping; see [[decisions/task-decay-is-one-ladder]],
[[decisions/unblocked-tasks-resurface-as-ticklers]], and
[[task-freshness]].
