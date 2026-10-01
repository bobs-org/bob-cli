---
keyword: Task Freshness
aliases:
  - "freshness"
---

The local calendar date a human last confirmed that an open task still needs doing as written, stored as `[fresh:: YYYY-MM-DD]` before the task's trailing Tasks fields. Supported Bob keymaps, Alt+F, and `bob capture` edits of existing tasks stamp it; creation and automation never do. A visible, non-recurring Ready task is due for review when it is new (no stamp), resurfaced (its `scheduled` date arrived after the stamp), or stale (the stamp is at least its interval old). The interval comes from `[refresh:: N]`, then the note's `task_refresh`, then `freshness.interval` (7 days). Freshness never changes a lane, Today, schedule, or priority. It is not Zorg `@FRESHNESS`, `bob vault-sync` freshness, or P-level scheduling windows.
