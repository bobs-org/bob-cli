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
- on periods, aligned Roots and Types charts that share one child-period axis:
  Roots shows configured root-parent series, and Types shows configured task types.
  Their legends distinguish named series, remainder, and unattributed time; a column
  opens its child's note;
- a period-only switch between Both, Roots, and Types, remembered independently of
  the [[day-strip]] controls.

Because periods are whole ISO weeks, the parts always add up to the whole. Worked
counts closed on-clock [[pomodoro]]s; [[off-clock-pomodoro]] time stays a quiet
separate hint.

The [[day-strip]] is the day member: it has no parts row and mounts itself in
`bob-plan`. Facts and lanes open the [[stats-dash]] with the period pinned. bob owns
every value, and the strip never writes the note.

Not the parts table (`bob-period-parts`), the [[stats-dash]], `bob stats` terminal
output, or the [[morning-gtd-review-footer]].
