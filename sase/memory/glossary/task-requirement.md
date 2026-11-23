---
keyword: Task Requirement
aliases:
  - req
  - task req
---

A Markdown checkbox child of a `#task` line or of another requirement that does not
itself carry `#task`. Following structural parents reaches exactly one owner task.
Depth is 1 under the owner and grows with each req ancestor; it is computed, never
stored. A nested `#task` child is a subtask, not a requirement: it starts its own
ownership boundary.

A req is **met** or **moot** with its owner and never written as completed. Pending
`[ ]` bytes stay `[ ]` under a done or cancelled owner. The only Bob writes are drop
(`[ ]` → `[-]` plus an optional Cancel Log), restore (`[-]` → `[ ]`, keeping the
log), and carry (the line becomes a `[>]` stub via [[task-split]]). A descendant
under a dropped or carried ancestor is Excluded, never called met. A req never has
`#task`, `[created::]`, a lane, freshness, priority, a schedule, dependencies,
[[task-link]]s, walk rows, Ready-cap or Today membership, or stats. It may have
child notes, child reqs, a trailing `^anchor`, and a Cancel Log when dropped.
Depth above 16 is refused.

Obsidian and Bob Mac Capture draw a tightly kerned hash run in the owner's ink over
the checkbox: `⌗⌗` at depth 1, `⌗⌗⌗` at 2, `⌗⌗⌗⌗` at 3 or more. The checkbox cannot
be clicked. Teaching copy: *Part of this task's scope. Met when the task closes.
Drop it or make it a task.*

A task that owns any req keeps its notes in a `NOTES` [[task-drawer]], born only
when a note lands in it. Drawers sit after reqs and before logs. A checkbox
inside a drawer, under a note, in a fence, or in a quote is never a requirement.

On a managed Done, the successor card audits every Pending req once. Default /
Ctrl+X is Met (no write); Ctrl+Shift+X drops; Ctrl+Shift+T carries into its own
task; Space/Tab carries and pulls in as a [[successor-offer]]. Promotion maps
each outer req to a `## Tasks` row (nested reqs stay reqs) and notes plus
`NOTES` into `## Notes`.

Not a nested-`#task` subtask, a [[task-drawer]] entry, a GTD `#pre`/`#post`
checklist row, or a [[pomodoro]]. See `docs/task-requirements.md` and
[[decisions:task-reqs-are-met-with-their-task]].
