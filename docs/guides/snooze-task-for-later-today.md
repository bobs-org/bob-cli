---
title: "Snooze a task until later today"
description: "Park an open task until a time today, then return it to the Pomodoro plan."
tags:
  - CLI
  - Obsidian
---

# Snooze a task until later today

Snoozing commits an open task to today while keeping it out of the active
Pomodoro plan until a clock time. Bob then returns its Task Link and notes to
an open Pomodoro. For another day, schedule the task with `s:<N>` instead.

## Before you start

Use a committed, open task that is not blocked or an Event Task. A Ready task
becomes Next when snoozed; Next and In Progress tasks keep their lanes. Snooze
times use Bob's configured timezone and must be later today. Set `timezone` in
Bob's config for automatic wake sweeps; without it, automatic sweeps are
skipped.

## Snooze from Obsidian

Select a task, one of its dedicated Pomodoro Task Links, or its Snoozed row,
then press `Alt+Z` to open the Snooze Card. Enter a time and confirm. You can
also open the card with `z` from the Task Card. If `Alt+Z` is already assigned,
use the Task Card or a command below.

## Snooze from the command line

Give Bob the task's route and block ID, followed by the time:

```bash
bob snooze add jobs:email-dan-eod 15:30
```

Use `bob snooze` to see today's queue. To create a task and snooze it in one
capture, add `z:<when>` to the routed task text:

```bash
bob capture 'Draft the follow-up @jobs z:1530'
```

For an existing task, capture `@route:block-id z:<when>`, such as
`@jobs:email-dan-eod z:1530`. Adding an existing task again retimes its active
row.

Times include `15:30`, `3p`, and relative offsets such as `+45m`. Relative
offsets round up to the next five-minute increment before being added to the
current minute. At 12:03, `+45m` resolves to 12:48; `+2m` rounds to five
minutes and resolves to 12:08. Past times, tomorrow, and daylight-saving gaps
are refused.

## What you see

- **Asleep:** the row and its notes wait in today's `## Snoozed` section. The
  task has an alarm clock and target time, and its link is outside the
  Pomodoro ledger.
- **Due:** after the target time, the row remains outside the ledger until a
  safe write can happen. Bob marks it due while the wake is pending.
- **Woken:** Bob moves the link and its notes to the top of an untimed open
  Pomodoro of the task's type, never the running Pomodoro. The Snoozed row is
  struck with the actual wake time, and the task's live link gets an `⏰ woke`
  mark. Waking does not start a work session. If you snooze that task again,
  Bob replaces its struck row with the new active row.

One notice announces a wake. When Bob Mac Capture owns notifications, it
shows a non-activating HUD. Otherwise Bob uses its desktop notification
fallback; Obsidian shows an eight-second Notice when its own wake spawn reports
that no notification was sent. Mac Capture holds its notice while the screen
is locked, displays are asleep, the session is inactive, you have been idle,
or a calendar meeting is running. It delivers the notice after you return or
the meeting ends, once Bob confirms the task is still open and woken.

## Wake, retime, or remove a task

`bob tick` runs the snooze wake step on its configured cadence, five minutes
by default. To process due rows immediately, run:

```bash
bob snooze wake
```

To wake one task before its target time, pass its route and block ID:

```bash
bob snooze wake jobs:email-dan-eod
```

Change an active row's time by running `bob snooze add` again. Remove the row
without waking the task with:

```bash
bob snooze drop jobs:email-dan-eod
```

## When a wake seems late

First check the queue:

```bash
bob snooze
```

An `asleep` row is not due yet; a `due` row has passed its time and is waiting
for a safe write. The `bob tick` snooze step runs every five minutes by
default, when the scheduler runs it on the allowed writer host. If that host
is asleep, including a Mac with its lid shut, it cannot run the wake until it
wakes. A `deferred` wake report means Bob did not write the planned move; run
`bob snooze wake` again. An unset `timezone` makes automatic sweeps skip with
`zone_unconfigured`; configure Bob's timezone to enable them.

The task can already be woken while its notice is held. Mac Capture waits to
show the notice until you are present and any running meeting ends. Check
`bob snooze` again; if the row is still due, run `bob snooze wake` to process
it now. The wake command is safe to repeat when nothing is due.

## Related

- [Snoozed task contract](../snooze.md) — row rules, time grammar, and wake behavior.
- [The `bob snooze` command](../reference/cli/snooze.md) — exact options and reports.
- [Today and the Pomodoro ledger](../concepts/today-and-the-pomodoro-ledger.md) — how ledger links select today's work.
