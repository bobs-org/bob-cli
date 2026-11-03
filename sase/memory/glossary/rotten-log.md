---
keyword: Rotten Log
aliases:
  - "keep log"
---

A task-local history of rung events, stored as a `🥀 **ROTTEN LOG**` direct child
with newest-first entries `*YYYY-MM-DD* — <body>`. Every counted keep, Stay, step,
and drop writes exactly one entry in the same transaction as its stamp, with one
undo; a blank reason always logs `🤷 no reason given`, and Esc writes nothing. A
counted keep logs `<reason> · kept N× in <Rung>` (`at` for P-levels; WIP, Next,
and Ready use `in`), N the [[keep-streak]] after that keep; a Stay logs `<reason>
· stayed in <Rung> after N keeps`; a step logs `🍂 <from> → <to> · after N keeps`
(`🍂 rolled again at P4 · after N keeps` at the bottom); a drop logs `🍂 <Rung> →
dropped · after N keeps` plus the existing Cancel Log entry. WIP keeps log here,
never in the [[work-log]]; freshness gestures never write or prompt for the Work
Log. The decay card's header derives per-rung keep tallies from these entries at
read time. The entry count never resets, is never stored as a field, and nothing
reads it to change decay, lanes, Today, schedule, priority, or
[[task-freshness]]. NEW, early, same-day repeat, Today-linked, PROJECTS,
recurring, checklist, closed, and unblock-tickler keeps never prompt and never
write it; deferrals and rewords stay in the [[schedule-log]]. `bob capture`,
Rust, hooks, randomize, and seed never write it. There is no backfill: keeps
before rollout leave no entries. See [[decisions/task-decay-is-one-ladder]].
