---
keyword: Off-Clock Pomodoro
aliases:
  - off-clock session
  - untracked pomodoro
  - untracked session
---

A [[pomodoro]] whose time Bob records but keeps off the work clock — lunch, the gym, school pickup. Its timespan carries `[off:: Nm]` where a worked session carries `[t:: Nm]`: `- [x] (**1200-1300** [off:: 60m]) — MEET`. A session goes off the clock exactly when it keeps at least one [[task-link]] and every one resolves off-clock: `#off` (deprecated `#offclock`, `#off-clock`, `#untracked`) or an inherited area/project `offclock: true`, with `#on` and `offclock: false` as overrides; an unlinked session, an unresolved link, or one on-clock link keeps the whole session worked. Writers decide when they write the span (starts, closes, and `bob gcal sync` for an off-clock [[event-task]]); a closed row's key is history, and later tag or note-default edits are reported, never applied. An ended off-clock [[event-task]] nobody started — tagged or inherited — settles into one on the next `bob task reconcile`. Worked, Sessions, `bob pomodoro --worked`, and the [[day-strip]] facts leave it out; its [[root-parent]] still receives its minutes. The Off-clock and Total clocks of `bob stats -k` and the [[stats-dash]] count it, grouped by [[root-parent]] like any session. Drawn dashed; `◌` in text. A bare closed `(**HHMM-HHMM**)` row is an older off-clock session counted by its span. Not a `()` placeholder, a Stats day before tracking began (`not_tracked`), or the tracked-task mark on a `#task` row.
