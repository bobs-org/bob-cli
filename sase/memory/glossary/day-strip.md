---
keyword: Day Strip
aliases:
  - "day stats"
---

The read-only daily Stats summary at the top of the `bob-plan` block on
canonical daily notes, above the badges, in Live Preview and Reading
view, or wherever a `bob-day-stats` fence embeds it. Its contents: five
LABEL-value facts, each numeric one over a quiet `typical` line (the median of
the seven days before, with a ▲/▼/= mark once the day is finished), then a
[[root-parent]] lane and a [[pomodoro-theme]] lane, each a legend over a
full-width bar, shown together or one at a time by the bars switch on
the top lane row beside `so far · HH:MM`; the last bar separates it from the badges. bob
owns every value, and it never writes the note. `## Pomodoros` no longer
repeats its total. Facts and the bars open the [[stats-dash]] with the day
pinned. "Day stats" in UI copy (`Toggle Day stats`, `stats.day_strip`).
Not the [[morning-gtd-review-footer]].
