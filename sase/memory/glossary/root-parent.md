---
keyword: Root Parent
---

A task's root parent is derived at read time from its note chain, unless its
explicit `[root:: NOTE]` field names the note where Bob should begin that walk.
The field is reporting-only: it attributes Pomodoro time and task metrics as if
the checkbox lived in the named note, while the checkbox stays in its original
file. Archive and reference wrappers are walked through. An area at any depth,
a project marked `root: true`, or the outermost project names itself as the root;
a subproject without `root: true` groups under its nearest root project, or the
outermost project when none is marked. A missing, invalid, or duplicate root
field falls back to the task's own note. A walk with no area or project is
`@unfiled`; unlinked Pomodoros remain `@unlinked`. The synthetic `GTD` bucket
(`@daily`) and named review buckets are retired. Daily review time is now the
time attributed to tasks rooted at `gtd_daily`.

Root parents are derived from the current note tree and never stored as a
separate status. `rollup:` no longer exists. The [[day-strip]]'s root-parent
bar and the default [[stats-dash]] view group by it; Stats JSON calls the
dimension `root_parent`. It is not the nearest home (`project`), a top-level
area, or a [[task-type]]; see `docs/pomodoro-homes.md`.
