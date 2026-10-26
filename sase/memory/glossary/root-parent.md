---
keyword: Root Parent
---

The [[area-note]] a task lives in, else the nearest project marked
`root: true` on the walk up to that area, else the [[top-level-project]]
above its project note. Archive and reference wrappers are walked through, and
an area-less walk falls back to the outermost project. Buckets `GTD
review`, `unlinked`, and `unfiled` stay buckets. Derived at read time
from today's note tree and never written; `rollup:` no longer exists. The
[[day-strip]] bar and the default [[stats-dash]] view group by it; Stats
JSON calls the dimension `root_parent`. It is not the nearest home
(`project`), a top-level area, or a [[pomodoro-theme]]; see
`docs/pomodoro-homes.md`.
