---
origin: requested
scope: project
keyword: meet-filing-follows-google-until-edited
summary: >-
  Google time or parsed buffer moves refile an Event Task under a new
  MEET when its existing filing remains Bob-owned; edited filing stays put.
aliases:
  - gcal meet refile
  - moved meeting refile
metadata:
  status: superseded-in-part
  superseded_by: "decisions:past-day-sync-closes-bob-filed-meetings"
decided: 2026-11-05
---

# MEET Filing Follows Google Until Edited

**Applies to.** bob-cli and the vault.

**Claim.** When sync applies a Google change to a journaled Event Task's
meeting time or parsed buffer, it re-files that row's same-note MEET link by
the new buffered phase, but only when the old filing is Bob-owned. An
untouched link under an open untimed MEET placeholder moves only when the row
remains untouched; a move into Past completes the row and writes a struck
link under a closed MEET. A Bob-shaped closed MEET may be retimed, split, or
merged when every member is a same-note Event Task in its born-done state,
the dedicated struck links are its only children, and the header exactly
matches the members' buffered-span cluster using the header's clock key. A
Past re-file keeps rows done; a moved row entering Current or Future reopens
and follows the ordinary birth placement rules. A link under an open timed
entry is left alone. Other filing is kept and reported once as `kept_meet`.
Clock-tag-only, title, root, or description changes, a kept time or buffer
edit, and the clock passing without a Google change never trigger re-filing.
Refiled links do not append another `gcal.link` journal record.

**Why.** On 2026-11-05, Google moved `^swings` from 08:15–08:30 to
08:00–08:15 after sync had filed it under an untimed placeholder. At 08:17,
sync updated the task's bold span but left the link in the plan and the row
open. Reconcile later settled this love-rooted, off-clock row, while an
on-clock meeting had no such correction. The stale filing came from
write-once MEET links, not from a safety rule. A moved event gives Bob new
evidence for its occupied time, while a session Bryan started, closed, or
edited remains his record.

Rejected alternatives:

- **Write-once filing.** Leaves stale MEET entries and requires hand fixes.
- **Re-file on every run.** Could auto-close an on-clock meeting nobody
  attended even though Google made no change.
- **Re-file from journaled MEET records.** Misses settle-written MEETs and
  adds another journal record kind.
- **Move links out of Bryan-edited sessions.** Rewrites his record of work.

**Cost.** A born-done row moved into Current or Future is reopened. A hand
reorder or hand-written MEET that still exactly matches Bob's closed shape is
treated as Bob's; trimmed settle MEETs do not match and are kept.

**Reopens when.** Attendance becomes knowable, or Bryan wants moved meetings
to keep their original filing.

Evidence: `plan:202611/gcal_refile_moved_meetings.md`; `docs/gcal.md` § Sync
and § Reconciling Google-side changes; `tests/gcal_sync.rs`; the 2026-11-05
`^swings` event move and follow-up reconcile.

[[decisions:ended-off-clock-events-settle]]

[[glossary:event-task]]

Superseded in part: the clause that the clock passing without a Google
change never triggers re-filing is retired for past note days. See
[[decisions:past-day-sync-closes-bob-filed-meetings]].
