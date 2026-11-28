---
keyword: Snoozed Task
title: Snoozed Task
summary:
  An open, committed, non-Event task planned for later today; its Task Link waits in
  the daily note's Snoozed section until a clock time, then wakes into the ledger.
aliases:
  - snooze
  - snoozed row
  - later-today task
---

# Snoozed Task

_Requested · project_

aka snooze, snoozed row, later-today task

An open Next or In Progress task planned for **today, but not before a clock time**.
Its [[task-link]] waits as one row in the `## Snoozed` section after today's ledger —
`- **1530** [[note#^id]]`, with the link's notes beneath — instead of under a
[[pomodoro]]. While it waits the task is **not Today**: it has no 🍅 mark, is absent
from the `=`/`x` lineups, `]s` and `]w` walks, and the dash PENDING/NEXT sections, and
does not count in `plan.max_links`. Its definition wears an alarm clock and the time
(`⏰ 15:30`, amber once due) where a Today task wears 🍅. Snoozing with Alt+Z on a
Task Link or task, `z:` in capture, or `bob snooze add` raises Ready to Next and stamps
freshness, like linking.

At or after the time, `bob snooze wake` — run by `bob tick`, Bob Mac Capture, or
Obsidian — moves the link and its notes to the top of an untimed open Pomodoro of the
task's type, never the running one, and strikes the row with the actual time
(`- ~~**1530** …~~ · woke 15:32`), which paints `⏰ woke 15:32` on the woken link.
The task is now Today and counts normally. Re-snoozing replaces the struck row with
the new active row. A single notice announces the wake: the Mac HUD when Bob Mac
Capture owns notifications, Bob's desktop fallback otherwise, or Obsidian's Notice
when its wake spawn receives no notification. Mac notices are held while the person
is away or in a meeting, then delivered when available. A passed time still awaiting
a safe write is **due**, not ready. Waking never starts work, stamps freshness, drops
prerequisites, or revives a task that left Next or In Progress.

Same-day only: an unwoken row becomes history at midnight and the sticky Next lane
returns the task to review; multi-day deferral is `[scheduled::]`. Never an
[[event-task]], whose span already says when it applies; not the Reading Board's
Snoozed lane or a project-frontmatter `snooze:` hold. See `docs/snooze.md`.
