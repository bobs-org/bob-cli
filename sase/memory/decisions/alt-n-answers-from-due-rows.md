---
keyword: Alt+N Answers From Due Rows
aliases:
  - alt-n answers from due rows
  - due-row advance
summary:
  "Alt+N answers from the ]s landing or from any due walk row (same note
  and line text in the pre-write queue), advancing once to that row's walk
  successor; PRE/POST and RECURRING still stay, and every other answer key
  stays landing-gated."
metadata:
  status: accepted
  decided: 2026-10-31
---

**Applies to.** bob-plugins (bob-navigation-hotkeys), bob-cli docs, and the Bob
vault review ritual.

**Claim.** `Alt+N` answers from the `]s` landing or from any due walk row: the
cursor row counts when the pre-write freshness queue holds an entry with the
same note path and the exact current line text, in any review group. After a
resolving write the walk advances exactly once to the answered row's walk
successor, planned anchor-only from the pre-write queue, with the same
composed toast, `<C-o>` jump record, journal entry, gesture lock, and settle
window as a landed answer. `reviewOutcomeResolves` is unchanged: PRE/POST and
RECURRING rows still stay. The rule covers counted `N<Alt+N>` and routed
inbox answers, and excludes the Task Link path. Every other answer key
(`Ctrl+Enter`, the Task Card, `Ctrl+Shift+Enter`, `Ctrl+Shift+M`) stays
landing-gated.

**Why.** Bryan asked for it in the plan's prompt: on 2026-10-31 two `Alt+N`
advances were missed on due rows off the landing — a reworded landed NEW row
(09:24:53, the landing match required the exact landed line text) and a
hand-reached row after a `Ctrl+Shift+M` parked the walk (09:27:21). Only
`Ctrl+Alt+F` advanced off a landing before this change.

Rejected alternatives:

- **Landing-only (ship only the inbox routing).** Misses reworded and
  hand-reached rows — exactly the two 10-31 misses.
- **A passive queue watcher.** Still rejected, for the reason in
  [[decisions/answering-advances-the-walk]]: it cannot tell an answer from a
  daytime edit and would advance off-landing work.
- **Extending the rule to every answer key.** Not requested; each other key
  keeps its landing gate.

Evidence:
`plan:202610/alt_n_inbox_route_and_walk_advance.md`;
bob-navigation-hotkeys 2.63.0 (this change);
`docs/freshness.md` §§6/13 in bob-cli.

**Cost.** A daytime `Alt+N` on a due row jumps the cursor (`<C-o>` returns),
and a just-edited row can miss while the Tasks cache lags (the "when in
doubt, stay" rule).

**Reopens when.** Bryan reports daytime jumps that cost more than the missed
advances saved, or asks for another key to answer from due rows.

Links [[decisions/answering-advances-the-walk]] (the landing rule this
extends for one key) and [[decisions/review-walk-is-tiered]] (the tier order
the advance walks through).
