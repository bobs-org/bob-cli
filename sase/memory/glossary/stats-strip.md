---
keyword: Stats Strip
aliases:
  - "period strip"
  - "week strip"
  - "quarter strip"
  - "year strip"
---

The read-only Stats summary that heads a [[period-note]] or a daily note, or appears
wherever a `bob-stats-strip` fence embeds one, for exactly one period: a day, an ISO
week, a 13- or 14-week quarter, an ISO year, or all time.

Every member has the same tiers, all painted from bob's values:

- five facts (Worked, Reviewed, Done, Created, Review closed), totalled over the
  period and marked `so far` while it is open. Each numeric fact sits over a quiet
  `typical` line: the median of the same-kind periods just before it, marked ▲/▼/=
  only once the period has closed;
- a [[root-parent]] lane and a [[task-type]] lane;
- above the day, a parts row with one column per child period, stacked by root
  parent, that opens that child's note.

Because periods are whole ISO weeks, the parts always add up to the whole. Worked
counts closed on-clock [[pomodoro]]s; [[off-clock-pomodoro]] time stays a quiet
separate hint.

The [[day-strip]] is the day member: it has no parts row and mounts itself in
`bob-plan`. Facts and lanes open the [[stats-dash]] with the period pinned. bob owns
every value, and the strip never writes the note.

Not the parts table (`bob-period-parts`), the Stats Dash, `bob stats` terminal
output, or the [[morning-gtd-review-footer]].
