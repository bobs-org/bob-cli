---
origin: requested
scope: project
aliases:
  - daily-note Event Tasks
  - gcal daily residence
summary: Synced calendar events live in their daily note's Events section with root attribution and one appended MEET.
metadata:
  status: superseded-in-part
  superseded_by:
    - decisions:synced-meetings-link-by-phase
---

# Calendar Events Are Daily-Note Event Tasks

**Applies to.** bob-cli, bob-plugins, and the vault.

**Claim.** Synced events live in their daily note's `## Events` section, a
recorded residence exception beside `^gtd` (see
[[glossary:event-task]]). Attribution comes from `[root::]`, and Task Links
go under one appended MEET placeholder.

**Why.** Daily residence keeps meetings out of the review walk, freshness,
Ready caps, and sticky Next (R9). Rejected: durable series tasks in
`meet.md` (they would clog review); a read-only fence around the section
(Bryan must be able to edit rows); and one placeholder per meeting (a plain
close still carries on `x`, and duplicate open names are refused).

**Cost.** A residence exception beside `^gtd`; meetings count against the
plan budget; MEET is not exempt.

**Reopens when.** Event Tasks need review or lanes, or same-type Pomodoro
duplicates land.

Superseded in part: Task Links no longer all go under one appended MEET
placeholder; an ended meeting lands under a closed MEET entry and a running
one under the running MEET. See
[[decisions:synced-meetings-link-by-phase]].
