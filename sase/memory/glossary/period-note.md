---
keyword: Period Note
aliases:
  - "weekly note"
  - "quarterly note"
  - "yearly note"
  - "all_time"
---

A Bob vault note for one period above the day, in whole ISO weeks:

- `YYYY/YYYY-Www.md`: a Mon–Sun ISO week, filed under its ISO week-year;
- `YYYY/YYYY-Qn.md`: 13 ISO weeks (W01–13, W14–26, W27–39, W40–52/53);
- `YYYY.md`: an ISO week-year;
- `all_time.md`: the singleton.

Each `parent` is the next period up (a daily note's is its week, a year's is
`all_time`, and `all_time`'s is `org`), so the tree is also an exact partition of
days. bob creates them, create-only and one week ahead (`bob periods ensure`).

Each opens with a [[stats-strip]] and a live parts table that summarizes every child
with its Headline or the root parents it mostly went to. It then holds:

- Bryan's `Intentions`, `Outcomes`, or `Theme`;
- a `## Review` that an agent may draft only through `bob periods draft`, while it
  is empty or still unedited;
- Bryan's `## Reflection`;
- a folded `[!agent]` callout with the complete, versioned drafting instructions.

Period notes hold no task rows. Monthly notes are retired: months are a Stats
grouping, not a note level. Not a daily note, an [[area-note]], or a
[[project-note]].
