---
keyword: Task Freshness
aliases:
  - "freshness"
---

The local calendar date a human last confirmed that an open task still needs doing as written, stored as `[fresh:: YYYY-MM-DD]` before the task's trailing Tasks fields. Supported Bob keymaps, Alt+F, and `bob capture` edits of existing tasks stamp it; creation and automation never do. A visible, non-recurring Ready task is due for review when it is new (no stamp), resurfaced (its `scheduled` date arrived after the stamp), or rotten (the stamp is at least its interval old; machine `STALE` until schema 2). The interval comes from `[refresh:: N]`, then the note's `task_refresh`, then `freshness.interval` (7 days). NEW holds unconfirmed tasks on `dash`, READY holds only confirmed/exempt tasks, and `rotten.md` holds RETURNED plus expired ROTTEN; see [[decisions/ready-is-freshness-gated]] and [[decisions/today-is-read-from-the-ledger]]. Freshness never changes a lane, Today, schedule, or priority. It is not Zorg `@FRESHNESS`, `bob vault-sync` freshness, or P-level scheduling windows.
