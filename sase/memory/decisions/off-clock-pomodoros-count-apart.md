---
keyword: Off-Clock Pomodoros Are Recorded And Counted Apart
aliases:
  - off-clock clock
  - off-clock ledger
summary: "A timed row with [off:: Nm] records off-clock minutes; separate clock views keep those minutes out of Worked metrics."
metadata:
  status: accepted
  decided: 2026-11-02
---

# Off-Clock Pomodoros Are Recorded And Counted Apart

**Applies to.** bob-cli and the vault.

**Claim.** A timed ledger row with `[off:: Nm]` is an Off-Clock Pomodoro.
Writers stamp it by the counted-links rule at start, close, sync, and settling.
Closed rows are history: later tag edits are reported as drift and never
rewrite the key. Worked, Sessions, `bob pomodoro --worked`, the Day strip, and
the agenda summary exclude off-clock minutes (`untracked_counts = exclude`
continues to govern Worked). The Off-clock and Total clocks count them. A bare
closed span keeps the earlier off-clock meaning and is flagged unstamped.

The counted-links rule uses the Task Links the session keeps: it is off-clock
only when at least one remains and every link resolves to a task whose own
line has the standalone `#offclock` tag or an alias. An unlinked, unresolved,
or mixed session is Worked in full.

**Why.** The `[off::]` key records the clock choice and its minutes with the
session, so later changes to task tags cannot silently move closed time.
Keeping Off-clock separate gives a way to see where that time went while
preserving Worked as the work signal. Bare spans retain the established f6
meaning until they can be restamped, and the diagnostic makes missing keys
visible.

Rejected alternatives:

- **Derive the clock from current task tags on every read.** Later tag edits
  would change historical metrics and require each reader to resolve links.
- **Split mixed sessions across clocks by task.** The session is one timed
  event; a mixed lineup stays Worked until per-task minute accounting exists.
- **Treat a bare span as Worked.** This would change the meaning of existing
  f6 rows and hide missing clock stamps.

**Cost.** Mixed sessions count as Worked in full. Closed history is frozen at
write time, so later tag changes appear as drift. Total sums sessions and can
double-count overlapping spans.

**Reopens when.** Per-task minute accounting is needed, or off-clock time
should come from an area property instead of a task tag.

Evidence: `plan:202611/off_clock_pomodoros.md`;
`research:202611/off_clock_pomodoro_stats/off_clock_pomodoro_stats.md`;
`docs/reference/vault-format.md`; `docs/stats.md`; and the pomodoro-homes and
meet-event-run vectors.
