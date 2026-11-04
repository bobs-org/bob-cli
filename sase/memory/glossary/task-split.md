---
keyword: Task Split
title: Task Split
summary: Promote splittable bullets into new tasks with live stubs in the
  parent. Distinct from splitting a Pomodoro time record.
---

# Task Split

aka split, split out

Task Split promotes a splittable note under a task or eligible note section into a new
task. A task's selected bullet becomes a live task-link stub, and each new task gets a
generated mention back-link; a section bullet has no owning task to mention. Split does
not write a `split from` relation. Use `bob task split` or Ctrl+Shift+T. On a task line,
Ctrl+Shift+T opens a Tab-mark picker; on a drawer title, it scopes the picker to that
drawer. A daily-note task sends new tasks to its root note's `## Tasks` intake, or the
default inbox when it has no root; cross-note writes have a guarded Undo notice. See
`docs/task-split.md` for the contract and exact edits. This creates task identities,
distinct from splitting one [[pomodoro]] session's time record.
