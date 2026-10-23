---
keyword: Rotten Log
aliases:
  - "keep log"
---

A task-local history of rotten keeps, stored as a `🥀 **ROTTEN LOG**` direct child
with newest-first `*YYYY-MM-DD* — <reason> · kept N×` entries. Alt+F or Ctrl+Alt+F on
an exact due Ready ROTTEN or scheduled TICKLER row (the same keep that increments the
[[keep-streak]]) and Keep on the approved-decay card first ask "Why keep it as
written?": the reason is optional (blank logs `🤷 no reason given`) but the entry is
not, and Esc writes nothing. N is the streak after that keep. The entry count is the
number of keeps logged since rollout: unlike `[keeps:: N]` it never resets, and
nothing reads it to change decay, lanes, Today, schedule, priority, or
[[task-freshness]]. NEW, early, Pending, Next, PROJECTS, and unblock-tickler keeps
never write it; Pending summaries stay in the [[work-log]] and deferrals in the
[[schedule-log]]. See [[decisions/rotten-keeps-are-logged]].
