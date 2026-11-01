---
keyword: Rotten Log
aliases:
  - "keep log"
---

A task-local history of keeps, stored as a `🥀 **ROTTEN LOG**` direct child with
newest-first counted entries `*YYYY-MM-DD* — <reason> · kept N×` and Next lane
entries `*YYYY-MM-DD* — <reason> · kept in Next`. Alt+F or Ctrl+Alt+F on an exact
due Ready ROTTEN or scheduled TICKLER row (the same keep that increments the
[[keep-streak]]) and Keep on the approved-decay card ask "Why keep it as written?":
the reason is optional (blank logs `🤷 no reason given`) but a counted entry is not,
and Esc writes nothing. N is the streak after that keep. Marking any Next (`[*]`)
task fresh also asks "Why keep it in Next?"; only a nonblank reason writes a Next
entry, and it never changes `[keeps::]`. The entry count is the number of counted
and Next entries logged since rollout: unlike `[keeps:: N]` it never resets, and
nothing reads it to change decay, lanes, Today, schedule, priority, or
[[task-freshness]]. NEW, early Ready, Pending, PROJECTS, and unblock-tickler keeps
never write it; Pending summaries stay in the [[work-log]] and deferrals in the
[[schedule-log]]. See [[decisions/rotten-keeps-are-logged]] and
[[decisions/next-keeps-ask-an-optional-rotten-reason]].
