---
aliases:
  - relation
  - LINKS line
  - LINKED FROM line
---
# Task Relation

*Requested · project*

A directed, optionally labelled, inert link from one task to another. It is authored on
the source task's 🪢 **LINKS:** line and mirrored on the target task's generated 🪢
**LINKED FROM:** line. The generated LINKED FROM line also lists [[task-mention]]s
(`(💬 mentions)` entries), which are not relations. Add one with Capture `~` / `~~` or the Task Card `l` / `⇧L`
gestures. A relation is distinct from a Pomodoro [[task-link]]: it never blocks a task,
counts as a Pomodoro Task Link, or changes its schedule, priority, or freshness.
The status exceptions are committing a configured cancel label through capture or
the Links stage, which cancels the source task at commit time, and committing a
configured complete label through capture or the Links stage, which marks the source
task Done at commit time.
