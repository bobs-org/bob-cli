---
keyword: Keep Streak
aliases:
  - "keeps"
---

The count of consecutive due reviews at which a human confirmed a due Ready
task as written, stored as `[keeps:: N]` (1–999; absence means 0) right after
the task's `fresh`/`refresh` fields. Only an exact due Ready rotten/returned
target increments, once per review through the single keep helper; every other
human stamp clears it, and automation never touches it. At the configured
limit the next due confirmation asks an explicit approved-decision question
instead of stamping; see [[decisions/rotten-keeps-use-priority-decay]] and
[[task-freshness]].
