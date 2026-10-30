---
origin: requested
scope: project
aliases:
  - Event Task clock close
  - meeting clock close
summary: Timed closes classify same-day Event Task links by meeting and session clocks; explicit picks win, except not-started events stay out of wildcard completion and parking.
---
# Event Task Links Close By The Clock

**Applies to.** bob-cli and bob-plugins.

**Claim.** On a timed Pomodoro close, an unlisted plain Task Link to an Event
Task ([[glossary:event-task]]) in the same canonical daily note is classified
by its local span and the session start `S` and close time `C`:

| Class | Rule | Default outcome |
| --- | --- | --- |
| `ended_inside` | end ≤ C and end > S | Complete and count the meeting |
| `running` | start < C < end | In Progress and carried |
| `not_started` | start ≥ C | Defer, carry, and do not count or start |
| `ended_before` | end ≤ S | Defer, carry, and do not count or start |

An explicit numbered selection wins for that link. The `!` and `*` wildcards
do not select a `not_started` Event Task; other wildcard outcomes still
apply. Ordinary Task Links, embedded links, authored `#` deferrals, and the
note-free `x0` reset keep their existing behavior.

**Why.** A plain close previously treated every linked meeting as worked,
carried it, and split the session minutes evenly. Per-meeting placeholders
still carry unselected meetings on `x` and duplicate open names are refused;
manual peeling adds repeated work to every close.

**Cost.** Rust and JavaScript must stay in lockstep through the shared
`tests/fixtures/event_task_close/vectors.json`. Back-to-back meetings in one
session still split its minutes evenly.

**Reopens when.** Same-type Pomodoro duplicates land, or meetings need
minute-level time attribution.

Evidence: `plan:202610/bob_gcal_daily_events.md`,
`docs/capture.md` § Closing the running Pomodoro,
`docs/reference/vault-format.md` § Events section and Event Tasks, and
`tests/fixtures/event_task_close/vectors.json`.
