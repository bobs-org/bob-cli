---
keyword: Answering A Walk Landing Advances The Walk
aliases:
  - answer once advance once
  - auto-advance
  - landed-row advance
summary:
  "A gesture that started on the row ]s just landed on, and whose committed
  write takes that row out of today's walk, advances the walk exactly once to
  the next remaining review item: Ctrl+Enter close, Ctrl+Alt+F, Alt+N, the
  Ctrl+Shift+Enter link, a resolving Task Card commit, and Ctrl+Shift+M.
  Alt+F is the one stay answer; Ctrl+Enter never crosses the PRE/POST
  boundary; a short gesture lock swallows double presses; every walk landing
  records a <C-o> jump."
metadata:
  status: superseded-in-part
  decided: 2026-10-06
  superseded_by:
    - decisions/task-move-never-advances-the-walk
---

**Applies to.** bob-plugins (bob-navigation-hotkeys, task-status-cycler,
block-id-prompt, bob-ledger-tools), bob-cli docs, the Bob vault review
ritual.

**Claim.** On the row `]s` just landed on, a gesture whose committed write
takes that row out of today's walk advances once to the next remaining item.
That covers the Ctrl+Enter close, Ctrl+Alt+F, the Alt+N commit/release, the
Ctrl+Shift+Enter link, a resolving Task Card commit, and Ctrl+Shift+M (which
advances instead of focusing the destination). Alt+F is the one stay answer.
Ctrl+Enter never auto-advances across the PRE/POST checklist boundary.
Esc, refusals, no-op writes, unlink, reopen, Reword, status cycling, and
anything off a landing stay. A short gesture lock swallows double presses
(~350 ms settle window after each advance). Every walk landing records a
`<C-o>` jump back to the answered row. Each advance shows one toast (what
was done, then where the walk landed); Task Card commits keep their rich
notice cards with the landing following on its own.

Rejected alternatives:

- **Binding each key to "action, then `]s`".** A blind second step jumps
  from the post-write cursor, skipping rows after line-shifting writes;
  anchor-only planning never does.
- **A passive queue watcher.** Reacting to any write that drops a row
  cannot tell an answer from a daytime edit and would advance off-landing
  work.
- **A second advance chord per gesture.** One more key to learn per answer
  instead of zero; the stay answer (Alt+F) already covers the opt-out.
- **A global review-mode toggle.** A mode to enter, forget, and leave in
  the wrong state; the landing scope already gates every key.
- **A dedicated triage card.** A new surface for decisions the existing
  gestures already make.
- **Nav performing every write.** Every writer keeps its own write path;
  nav only judges the outcome through capture/continue.
- **Ctrl+Enter crossing the checklist boundary.** One habitual extra press
  would close a real commitment and move away from it; Ctrl+Alt+F still
  crosses.
- **A config kill switch.** Alt+F, `<C-o>`, and a plugin rollback are
  enough.

Evidence:
`research:202610/review_walk_answer_auto_advance/review_walk_answer_auto_advance.md`;
`plan:202610/review_walk_answer_auto_advance.md`;
nav api v3 `reviewWalk` (capture/continue) in bob-navigation-hotkeys 2.6.1;
task-status-cycler 1.26.0; block-id-prompt 1.22.0; bob-ledger-tools 1.29.2;
`docs/freshness.md` §§6/13 in bob-cli.

**Cost.** A fast habitual `]s` after an answer can still skip one row once
the 350 ms window passes (`[s` or `<C-o>` recovers it); Task Card commits
show two toasts; keys are swallowed for about 350 ms after each advance.

**Reopens when.** Bryan asks for Ctrl+Enter to cross the boundary; `<C-o>`
after an answer becomes routine; the lane keep rate stays above 90%, which
questions the Alt+F / Ctrl+Alt+F pair.

Links [[decisions/review-walk-is-tiered]] (the tier order and walk this
advance moves through).

Superseded in part: Ctrl+Shift+M only — a move never advances the walk; see
[[decisions/task-move-never-advances-the-walk]].
