---
keyword: Off-Clock Note Defaults Filter By Task Type
aliases:
- offclock_types
summary: Nearest area or project notes can filter off-clock time by effective task
  type; the list wins over a same-note boolean.
metadata:
  status: accepted
  decided: 2026-11-05
---

# Off-Clock Note Defaults Filter By Task Type

**Applies to.** bob-cli writers, bob-plugins, Google Calendar, and the vault.

**Claim.** A task's clock is decided by the first match: `#on` makes it
on-clock; `#off` or a deprecated alias makes it off-clock; otherwise, the
nearest area or project note on the task's root walk that declares a clock
policy decides. If that note has `offclock_types`, the task is off-clock exactly
when its effective type is listed; any other type, including no type, is
on-clock, and the walk stops at that note. The effective type is resolved from
the task field, reading rule, nearest `task_type:` / `theme:` default, then
`plan.default_type`. A list wins over `offclock:` on the same note. Without a
list, `offclock: true|false` keeps its existing behavior. An invalid list is
on-clock and stops the walk. A non-area/project note's policy keys are ignored
and the walk continues. Writers stamp the effective clock into `[t::]` /
`[off::]`; closed rows stay frozen.

**Why.** The area-and-type list expresses “meetings in life areas are off the
clock, while work meetings and focused chores stay Worked” without changing
the root walk or writing tags onto tasks. A global “MEET is off-clock” rule
would affect job meetings; putting the list on `meet.md` would miss tasks whose
root walk never reaches it. Making `offclock:` accept booleans and lists would
give one Obsidian property multiple types and expand today's invalid values.
Filtering by Event Task would miss hand-captured MEET tasks and keep retyped
events off-clock. `onclock_types` reverses the desired default. Having Calendar
sync write `#off` conflicts with Google-owned titles and would rewrite history
when routing changes. A general rules engine is too large for this need.

**Cost.** Type becomes an accounting input: retyping a task, changing an
inherited type, or editing its root can change the clock written for future
sessions. Non-MEET off-clock time in `love` and `body` becomes Worked unless its
task gets an explicit off-clock policy. Mixed sessions are more common and
remain Worked in full when any linked task is on-clock.

**Reopens when.** Per-type exceptions within one note's list are needed, or
membership in `## Events` should imply MEET.

Evidence: `plan:202611/offclock_types.md`;
`research:202611/offclock_types_area_type_clock/offclock_types_area_type_clock.md`;
phase commits `ecc3d32d` (resolver and vectors), `e6122f7a` (gcal and naming
restamps), and `832e1246` (rename and split coverage).

See [[decisions:off-clock-pomodoros-count-apart]],
[[glossary:off-clock-pomodoro]], [[glossary:event-task]],
[[glossary:area-note]], and [[glossary:project-note]].
