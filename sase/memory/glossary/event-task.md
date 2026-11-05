---
origin: requested
scope: project
aliases:
  - event row
  - calendar task
---
# Event Task

A checkbox `#task` row inside a canonical daily note's `## Events` section,
whose description begins with a local `**HHMM-HHMM**` span. Hand-written
rows qualify. Its `[root:: NOTE]` attributes meeting time to an area or
project, and a Pomodoro Task Link under the daily note's ledger records the
session. `#off`, `#on`, or an inherited area/project `offclock:` through
`[root::]` decides whether the session that links it is an
[[off-clock-pomodoro]], so its `[root::]` attributes those minutes. A synced MEET Event Task can match a root note's `offclock_types` list.
On timed close, Bob compares the span with the session's start and
close times: a meeting that ended inside is completed, one still running
stays In Progress and carried, and one not started or ended before the
session is deferred. Explicit numbered selections take precedence; wildcard
completion and parking do not select a not-started meeting. See
`docs/reference/vault-format.md` and `docs/capture.md`.

An Event Task is a daily-note residence exception beside `^gtd`. See the
[[area-note]], [[pomodoro]], [[task-link]], and [[root-parent]].
