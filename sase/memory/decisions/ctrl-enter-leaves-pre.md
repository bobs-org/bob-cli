---
keyword: Ctrl+Enter Carries The Last PRE Chore Into The Next Group
aliases:
  - pre exit
  - ctrl+enter leaves pre
  - last pre chore advances
summary:
  "On the last PRE row the walk landed on, Ctrl+Enter completes the chore
  and lands where ]s would. A short guard swallows a repeated rapid press.
  A wrap back to a skipped chore still stays, and lane rows still never
  step onto POST."
metadata:
  status: accepted
  decided: 2026-11-16
---

**Applies to.** bob-plugins (bob-navigation-hotkeys), bob-cli docs, and the Bob
vault review ritual.

**Claim.** On the last PRE row the walk landed on, Ctrl+Enter completes the
chore through Tasks and lands where `]s` would: `planReviewJump` from the
group-end anchor, only when the plan is a `jump`, `wrapped` is not true, and
the destination tier is not `pre`. POST is a legal destination. The toast is
one composed notice: `PRE done` (or `end of PRE · {n} skipped`), then a
boundary line when the destination is ROTTEN or POST, then the shared
advance-tail landing notice with its action hint, POST remaining counts, and
quick-roll line. A 1.5 s settle window after that crossing swallows a
repeated rapid press of the Ctrl+Enter claim, reviewWalk capture, and `]s`.
The cursor stays with today's `]s → {LABEL}` group-end toast when nothing
else is due, the plan wraps back to a skipped PRE chore, or the landing
fails. Lane-row Ctrl+Enter still never steps onto a checklist row.

**Why.** Bryan asked: "After I complete the final task in the PRE GTD review
group, we should jump to the first task in the next review group, but I think
we intentionally do not currently." The October checklist-walk plan chose
that stop so one habitual extra press would not close a real task. The review
journal (`~/bob/.bob/stats/review/*.jsonl`, 36 mornings, 2026-10-12 →
2026-11-16) shows every morning continues past PRE (NEW 34, NEXT 1, PENDING
1), so today's stop costs one `]s` every morning and helps on none of them.
On 20 of 36 mornings the final PRE answer came 2 s or less after the previous
one; 82 of 219 consecutive PRE answers were 2 s or less apart. The fastest
next answer after the last PRE chore was 5 s (`]s` included, median 39 s), so
a 1.5 s window swallows the rapid extra press.

Rejected alternatives:

- **Keeping the stop with Ctrl+Alt+F as the crossing key.** It requires
  knowing which chore is last and switching keys mid-run.
- **Crossing with the plain 350 ms settle.** A rapid-fire extra press would
  complete the landed task.
- **Extending the crossing to lane → POST.** Not requested; the "never onto
  a checklist row" outcome rule stands.

Evidence:
`plan:202610/ctrl_enter_checklist_walk.md` D3;
`plan:202611/ctrl_enter_leaves_pre.md`;
bob-navigation-hotkeys 2.83.0 (this change);
`docs/freshness.md` §§6/13 in bob-cli;
the review-journal census above.

**Cost.** An extra Ctrl+Enter more than 1.5 s after the crossing completes
the landed row (`<C-o>` and Ctrl+Enter reopen it); walk keys are swallowed
for about 1.5 s after the crossing.

**Reopens when.** Bryan reports closing a task he landed on after PRE by
mistake, or the guard swallows deliberate keys.

Links [[decisions/answering-advances-the-walk]] (the walk-advance rule this
changes for the last PRE chore) and [[decisions/review-walk-is-tiered]] (the
tier order the exit lands in).
