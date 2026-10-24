---
title: "Today and the Pomodoro ledger"
description: "How the daily Pomodoro ledger selects Today without moving tasks or changing lanes."
search:
  boost: 2
tags:
  - CLI
  - Obsidian
---

# Today and the Pomodoro ledger

This concept explains how the daily note's Pomodoro ledger selects today's work while tasks stay in their home notes.

The daily note holds a `## Pomodoros` section: one column-0 checkbox entry per work session, with indented Task Links beneath each open entry naming the tasks planned there. Tasks themselves never move into the daily note; an area task in `garden.md` or a project task in `Projects/seedlings.md` is selected for today by linking it, not by copying it. Today is the open tasks with a dedicated Task Link under today's open entries, computed at read time.

```markdown
## Pomodoros

- [ ] Seed trays (0900-0930)
  - [[garden#^label-trays]]
- [ ] Cold frame (1000-1100)
  - [[workshop#^harden-frame]]
- [x] Early session (0800-0830)
  - [[garden#^seed-mix]]
```

The two open entries plan two tasks for today. The closed entry is work history: links under closed Pomodoros never count toward Today. Only a dedicated Task Link counts — the only content of its sub-bullet once markers are stripped — and it must sit at the entry's first child indentation under an open entry.

## Live plan, history, and commitment

Three ideas share the ledger, and they behave differently. The live plan is the set of open entries and their dedicated links right now; editing it changes what `bob plan` reports. Session history is the closed entries and their struck or marked links; closing a Pomodoro retires its links into history without touching the tasks. Task commitment is the lane: linking a Ready task promotes it to Next, while removing the last live link removes Today membership but leaves the sticky Next or Pending lane in place.

Starting or closing a session and completing its task are therefore distinct outcomes. Closing a Pomodoro ends the time block and archives its links; completing a task changes the task line itself to Done and may offer follow-up work elsewhere. A session can close with work unfinished, and a task can complete outside any session.

## Today is not a lane, tag, or folder

Today never changes the checkbox, never writes a tag, and never means "tasks stored in the daily note." Residence in the daily note is not enough: an unlinked task that happens to live there is not Today, and a linked area task is Today even though it lives elsewhere. Tasks link into Today from any note type except quarantined copies, and the ledger is the only membership list.

Exempt GTD entries can still contribute Today tasks. The plan budget ignores exempt components when counting themes, but Today counts dedicated links under every open entry, including exempt ones. Theme and link budgets and Today use different counting rules on the same ledger: the budget guards how much is planned, while Today names which tasks were chosen.

## Reading the ledger

`bob plan` is read-only. It reports themes, distinct Task Links, Today, and lane counts from the ledger as written; it never promotes, releases, or cleans up links. Reconcile and capture gestures perform the writes, and a second `bob plan` run shows their result. Exact caps, entry syntax, markers, and resolution rules belong in the contract; the model here is only that the ledger is input, the plan is a view, and writers act separately.

## Related

- [Your first Bob day](../start/first-bob-day.md) — link work into today's ledger.
- [Tasks and lanes](tasks-and-lanes.md) — what linking promotes and what unlinking keeps.
- [Freshness and the review walk](freshness-and-the-review-walk.md) — how Today rows are reviewed.
- [Understand Bob](index.md) — the eight nouns and where Today fits.
- [Plan budget, Today, and lanes](../plan.md) — the authoritative Today and budget definitions.
- [Pomodoro homes](../pomodoro-homes.md) — how ledger entries derive their labels.
- [Pomodoro themes](../pomodoro-themes.md) — remembered names, hues, and `bob themes`.
- [Today marks](../today-marks.md) — the display-only mark on Today tasks.
