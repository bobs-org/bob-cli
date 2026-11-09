---
keyword: Task Merge
title: Task Merge
summary: Fold source tasks into a target task, drawer, or note section,
  retiring each source as a cancelled tombstone. Distinct from a Merged Pomodoro.
---

# Task Merge

aka merge, merged into

Task Merge folds one or more source tasks into a target task, one of its drawers, or a
note section through `bob task merge`, capture's `>` or `<`, or the configured
`merged into` relation. It copies the source's notes to the destination and retires
each source as a cancelled tombstone that keeps them.
Dependent tasks and affected links are repaired, and the source records the target in
its `🪢 LINKS: merged into` line. See `docs/task-merge.md` for the contract and edge
cases. This task-outline operation is distinct from a [[merged-pomodoro]], which
combines work sessions.
