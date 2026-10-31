---
keyword: Synced Meetings Are Linked By Phase At Birth
summary:
  "A new Event Task is filed under a past, running, or planned MEET at birth:
  closed clusters for ended meetings, one open MEET for current and future."
metadata:
  status: accepted
title: Synced Meetings Are Linked By Phase At Birth
aliases:
  - synced-meetings-link-by-phase
  - meet by phase
---

# Synced Meetings Are Linked By Phase At Birth

**Applies to.** bob-cli and the vault.

**Claim.** Whenever `bob gcal sync` creates an Event Task (a new row, or a
Maybe context row upgraded in place), it files that row under a MEET Pomodoro
in the same write, on any day:

| Phase at sync time | Pomodoro | Link line | Row checkbox |
| --- | --- | --- | --- |
| Past: the event ended (`end ≤ now`) | A closed `- [x] (**SSSS-EEEE** [t:: Nm]) — MEET` entry | `🍅 [[#^id]]` | `[/]` |
| Current: `start ≤ now < end`, today's note only | The running MEET entry | `[[#^id]]` | `[*]` |
| Future: the event has not started | The open MEET placeholder `- [ ] () — MEET` | `[[#^id]]` | `[*]` |

Back-to-back or overlapping past meetings share one closed MEET whose span is
the cluster (next start ≤ cluster end). All current meetings share one
running MEET, and all future meetings share one placeholder. The run never
leaves a second open MEET: current meetings with no running Pomodoro reuse an
existing MEET placeholder if one is open, otherwise they create a running
MEET; future meetings go under the running MEET when that running entry is
named MEET, otherwise under the placeholder (created when missing). Sync
never creates a running entry while any open timed Pomodoro exists. Sync
only creates MEET entries and appends links; it never starts, closes,
retimes, renames, or moves an existing entry.

**Why.** Bryan asked that every Event Task `bob gcal sync` creates get a
past, current, or future Pomodoro with a matching task link, grouping
same-type meetings, so every created Event Task starts `[*]` or `[/]`.
`bob task reconcile` exits 1 when a daily note has more than one open timed
Pomodoro, `bob capture =…` refuses to start a session while one is running,
and `duplicate_open_pomodoro_name` lints two open MEET entries.

Rejected alternatives:

- **One closed MEET per past meeting.** Touching or overlapping meetings are
  one truthful span; a gap still splits clusters.
- **A running MEET plus a separate MEET placeholder.** That leaves two open
  MEETs, which lints and fights the one-running-session guard.
- **`[x]` for ended meetings.** Sync cannot know Bryan attended, and the
  request names `[/]`.

**Cost.** A Past row is worked from birth (`[/]` and a closed-entry link), so
a later Google decline, delete, or move leaves it alone and reports it once.
`[/]` rows count in the Pending lane and show in `list`'s `past_open` footer
until they are completed or cancelled. A closed or running entry built from
calendar spans may overlap a session already logged. R9 may clear future-day
`[*]` rows to `[ ]` until that day is today.

**Reopens when.** Same-type Pomodoro duplicates land, or sync can learn
attendance.

Evidence: `plan:202610/gcal_meeting_pomodoros.md`; `docs/gcal.md` § Sync;
`tests/gcal_sync.rs`.

[[decisions:calendar-events-are-daily-note-event-tasks]]

[[decisions:event-task-links-close-by-the-clock]]

[[glossary:event-task]]
