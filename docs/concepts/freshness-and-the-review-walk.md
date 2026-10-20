---
title: "Freshness and the review walk"
description: "How confirmation dates drive NEW, TICKLER, and ROTTEN review without changing lanes."
search:
  boost: 2
tags:
  - CLI
  - Obsidian
---

# Freshness and the review walk

This concept explains how a human confirmation date drives review without ever moving a task between lanes.

Freshness is the local calendar date a human last confirmed that an open task still needs doing as written, stored as `[fresh:: YYYY-MM-DD]`. It is not creation time, completion, priority, or activity. Absence means NEW: nobody has confirmed the task yet. A visible, non-recurring Ready task becomes due when it is new, when its schedule returns it as a tickler, when an automatic unblock resurfaces it, or when its confirmation grows stale enough to be rotten. Classification never edits the task status; review only decides what the reviewer sees next.

```bob-task
- [ ] #task Harden the cold frame before the frost [fresh:: 2026-10-13] [refresh:: 7] ^harden-frame
- [ ] #task Order seed-starting mix ^seed-mix
```

The first row was confirmed a week ago on a seven-day interval and is due again. The second row has no stamp and is NEW. Neither row changes lanes when review opens; both simply appear in the walk.

## The READY view and lane pressure

The READY view excludes review buckets while whole-lane pressure remains visible. NEW and ROTTEN rows are held out of READY so the reviewer clears them first, but the lane counts, caps, and dashboard pressure still reflect every open task. Freshness itself never silently changes a lane, a priority, or a schedule: stamping a task only records the confirmation, and even an overdue stamp leaves the checkbox, the schedule, and the priority exactly as they were.

## The nine tiers in order

The morning walk visits one shared queue in explicit tiers:

| Order | Tier | What it holds |
| --- | --- | --- |
| 1 | PRE | Opening `#gtd #pre` checklist rows, resolved only by completion. |
| 2 | NEW | Unconfirmed tasks. |
| 3 | PROJECTS | Visible `^prj` outcome trackers on their project cadence. |
| 4 | PENDING | Started work due for its daily lane review. |
| 5 | NEXT | Committed work due for its daily lane review. |
| 6 | RECURRING | Due recurring occurrences, never stamped. |
| 7 | TICKLER | Scheduled returns and automatic unblock recoveries. |
| 8 | ROTTEN | Expired confirmations. |
| 9 | POST | Closing `#gtd #post` checklist rows, resolved only by completion. |

Pending and Next come due daily under their lane intervals (default one day; turning an interval off walks that lane off). PROJECTS has its own project cadence and is never disabled by a lane switch. Due recurring occurrences walk in RECURRING and complete into their next occurrence instead of taking a stamp. References use ordinary tiers: a reading task is reviewed by the same freshness rules as any other task, with no reference-only group. Review order differs from dashboard grouping on purpose: the walk is a structural review sequence, while the dashboard groups work for doing.

Detailed eligibility, intervals, and comparators live in the freshness contract. The model here is only that due work walks in tier order, a stamp taken today drops the row from the walk, and nothing else about the task moves.

## Answering the walk

The human choices are small: confirm, change, defer, link, release, complete, or drop. Ordinary resolving answers on the row the walk just landed on advance the walk exactly once to the next remaining review item: closing the task, consuming its link, releasing it, or resolving it through a Task Card commit. A task move follows the moved task to its destination note and parks the walk, so the next step resumes at the moved row's walk neighbour. Alt+F confirms without advancing and is the stay answer.

Repeated unchanged keeps on a due Ready row can prompt an explicit approved decision — choices such as doing it less often, rewording, dropping, or keeping anyway — through the existing schedule and log writers. The prompt is never automatic decay: nothing changes until a choice is approved, and a keep on an unblock tickler leaves the streak unchanged. Exact keep counting and card behavior belong in the contract.

## Related

- [Your first Bob day](../start/first-bob-day.md) — confirm work during morning review.
- [Tasks and lanes](tasks-and-lanes.md) — why review never changes the checkbox.
- [Today and the Pomodoro ledger](today-and-the-pomodoro-ledger.md) — how Today rows enter review.
- [Understand Bob](index.md) — the eight nouns and where freshness fits.
- [Task freshness](../freshness.md) — the authoritative review queue, tiers, and schema.
- [Plan budget, Today, and lanes](../plan.md) — how Today and lanes relate to review.
