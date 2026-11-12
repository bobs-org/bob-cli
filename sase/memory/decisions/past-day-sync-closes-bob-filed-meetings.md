---
aliases:
  - past-day calendar close
  - stale MEET close
summary: "Past-day sync closes eligible untouched Bob-filed Event Tasks, with safe-shape rules for running and unlinked rows."
---

# Past-Day Sync Closes Bob-Filed Meetings

**Applies to.** bob-cli and the vault.

**Claim.** When `bob gcal sync` syncs a note day strictly before today, it
closes each included, journaled or adopted, untouched open Event Task whose
buffered span has ended and whose filing is still Bob-owned. An exact plain
link under the open untimed MEET placeholder is removed, the row becomes
`[x]` with `[completion:: <note day>]`, and a struck link is placed under a
closed MEET clustered by buffered span and split by effective clock. The
emptied placeholder is removed. A stale running MEET closes in place only
when its header has Bob's exact timed-MEET shape and every child is an exact,
childless link to an included, past, untouched Event Task. It keeps its start
and clock key, extends its end through the latest member's buffered end, and
recomputes its duration without splitting by clock. Hand-filed rows and
running entries that fail that shape check stay open and are reported once
as `kept_open`. An open unlinked row stays open, receives no new link, and is
reported once as `kept_open`. Today's and future notes are unchanged by this
pass; a passing clock alone does not re-file a same-day row.

**Why.** Bryan's 2026-11-12 request asks `bob gcal sync -D yesterday` to
close an open event task and its MEET Pomodoro. A past-day sync runs after
ordinary events have ended, so the earlier rejection that sync usually does
not run after an event ends no longer applies. `bob task reconcile` remains
today-only and cannot close rows in past ledgers.

Rejected alternatives:

- **Extend reconcile to past days.** The selected command is the explicit
  Google sync for that past note, and reconcile remains today's ledger
  maintenance.
- **Trim stale running MEETs like settle.** The close preserves Bob's
  existing session start and clock and accounts through the latest event end;
  trimming would invent a different session record.
- **Close today's ended events too.** Today's sessions keep their existing
  timed-close and off-clock-settle behavior.

**Cost.** Attendance is unknowable: a skipped meeting is closed and counted.
Uncheck or cancel it by hand if it was missed.

**Reopens when.** Sync can learn whether Bryan attended.

Evidence: `plan:202611/gcal_past_day_close.md`; `docs/gcal.md` § Sync;
`tests/gcal_sync.rs`.

[[decisions:ended-synced-meetings-are-born-done]]

[[decisions:meet-filing-follows-google-until-edited]]

[[decisions:ended-off-clock-events-settle]]

[[glossary:event-task]]
