---
keyword: Active Task Statuses Are Derived, Not Authored
aliases:
  - derived task status
  - derived blocked
  - task status source of truth
summary:
  Today's Pomodoro ledger drives Next and In Progress; open dependencies and future
  scheduled dates drive Blocked. bob task-status-hooks reconciles them, so writers
  change those inputs, never just the checkbox.
metadata:
  status: superseded-in-part
  decided: 2026-07-16
  superseded_by:
    - decisions/task-lanes-are-sticky
    - decisions/task-deps-are-depends-on-links
---

**Applies to.** bob-cli, bob-plugins, vault.

**Claim.** Next `[*]`, In Progress `[/]`, and Blocked `[?]` are derived, not authored. A
task is Next while a Task Link to it, or a transcluded dependency path, sits under
today's open Pomodoros. It becomes `[/]` when an `=x` close records work on it, and in
area and project notes `[/]` resets once neither today's ledger nor the previous daily
links it. It is `[?]` while it has an open `dependsOn` target or a `scheduled` date
after today, which overrides the other two. `bob task-status-hooks` is the vault-wide
reconciler and runs unattended (every 15 minutes on the MacBook). Capture and the
plugins' keymaps may apply the same rules at once for feedback, but they change the
inputs — a Task Link, a dependency, a schedule — and the hooks have the final word.
`docs/task-status-hooks.md` owns the full rules and precedence table; see also
[[glossary:task-link]].

**Why.** The authored status failed first: `[B]` Blocked was retired unused — no `#task`
line in the vault carried it — when Next replaced it (bob-plugins `890ed13`, `feat!`,
2026-07-08). Next was derived from open Pomodoros two days later (`bc829fa`), and
Blocked came back on 2026-07-16 as derived state (`fc39562`). Its plan,
`plan:202607/blocked_task_status.md`, calls Blocked "derived state, not a second
dependency model" and keeps the command "the vault-wide source of truth", because only a
whole-vault scan knows every remaining dependency: the `!` keymap sets `[?]` when it
adds a dependency but stays status-neutral when one is removed, for exactly that reason.
In Progress rollback followed on 2026-07-21 (`084bb62`) and future schedules on
2026-07-24 (`9bb625a`). Rejected alternatives: an authored Blocked status; letting each
command or keymap own the final status (an `=x` close deliberately leaves Blocked
recovery to the hooks); and keeping a hidden pre-block status to restore later.

**Cost.** A hand edit to a derived checkbox is silently undone on the next run, and
between runs the vault can look inconsistent. Every writer must keep the inputs
consistent: a hand unblock must also retire the future `scheduled` date, or the hooks
re-derive `[?]` (bob-plugins `786fc1d`). A recovered task returns to its derived rank,
not its pre-block status. Because statuses decay, "this matters this week" cannot live
in a checkbox — that is what [[decisions/now-tag-is-user-owned]] is for.

**Reopens when.** A status is needed that no ledger, dependency, or schedule input can
express, or reconciliation moves off a periodic whole-vault scan (for example, behind a
transactional vault service).

Superseded in part: Next and In Progress are no longer derived — see
[[decisions/task-lanes-are-sticky]]. The transcluded-dependency path and the `!`
removal rationale are retired by [[decisions/task-deps-are-depends-on-links]];
the Blocked rule stands.
